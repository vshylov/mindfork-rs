//! The provider's model catalogue, on the settings screen's request
//! ([docs/research/model-picker.md](../../../docs/research/model-picker.md),
//! stage 4b).
//!
//! The one place the UI asks the network for something. It goes through the
//! orchestrator rather than being fetched by the screen for two reasons: the
//! screen cannot await anything (it runs inside the terminal loop), and the API
//! key must not travel on the command channel — the orchestrator already holds
//! both the config and the stored secrets, so a slot is all the request needs to
//! name.

use std::sync::Arc;

use crate::app::events::AppEvent;
use crate::shared::api::catalogue::{
    self, CatalogueError, CatalogueRequest, CatalogueShape, ModelSlot,
};
use crate::shared::config::{CloudSettings, ExternalSettings, ImpersonationMode, ServerMode};

use super::Orchestrator;

impl Orchestrator {
    /// Answers [`AppCommand::ListModels`](crate::app::events::AppCommand::ListModels).
    ///
    /// Always answers: a failure is an `Err` in the same event, never silence —
    /// the row is showing "fetching…" and has nothing else to end it with.
    pub(super) fn handle_list_models(&mut self, slot: ModelSlot) {
        let request = match self.catalogue_request(slot) {
            Ok(request) => request,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::ModelCatalogue {
                    slot,
                    models: Err(err),
                });
                return;
            }
        };
        let tx = self.evt_tx.clone();
        tokio::spawn(async move {
            let models: Result<Arc<[_]>, _> = catalogue::fetch(&request)
                .await
                .map(|models| Arc::from(catalogue::for_slot(models, slot)));
            match &models {
                Ok(list) => tracing::info!(
                    ?slot,
                    offered = list.len(),
                    "the provider's catalogue answered"
                ),
                Err(err) => tracing::info!(?slot, ?err, "no catalogue for this slot"),
            }
            let _ = tx.send(AppEvent::ModelCatalogue { slot, models });
        });
    }

    /// Where this slot's catalogue lives, and what opens it.
    ///
    /// The key follows exactly the rule a request follows — a stored key first,
    /// then the environment variable the slot names
    /// ([`resolve_api_key`](crate::app::supervisor::resolve_api_key)) — so the
    /// picker cannot claim "no key" for a setup that chats perfectly well.
    fn catalogue_request(&self, slot: ModelSlot) -> Result<CatalogueRequest, CatalogueError> {
        use crate::shared::config::SecretSlot;
        let cfg = &self.config;
        let (mode, external, cloud, secret) = match slot {
            ModelSlot::Assistant => (
                cfg.engine.mode,
                &cfg.engine.external,
                cfg.engine.cloud(),
                cfg.engine.secret_key(),
            ),
            ModelSlot::Impersonation => (
                // Impersonation carries its own mode, with a `Shared` arm on top
                // of the six: it runs on the assistant's engine and has no model
                // row of its own to fill.
                impersonation_mode(cfg.impersonation_engine.mode)
                    .ok_or(CatalogueError::NotConfigured)?,
                &cfg.impersonation_engine.external,
                cfg.impersonation_engine.cloud(),
                cfg.impersonation_engine.secret_key(),
            ),
            ModelSlot::Embedder => (
                cfg.embed.mode,
                &cfg.embed.external,
                cfg.embed.cloud(),
                cfg.embed.secret_key(),
            ),
        };
        // The key follows exactly the rule a request follows — a stored key
        // first, then the variable the slot names — so the picker cannot claim
        // "no key" for a setup that chats perfectly well.
        let env = match mode {
            ServerMode::External => external.api_key_env.clone(),
            _ => cloud.and_then(|c| c.api_key_env.clone()),
        };
        let stored = secret
            .and_then(|k| crate::shared::secrets::stored_key(&cfg.api_keys, &k.storage_name()));
        let key = crate::app::supervisor::resolve_api_key(stored.as_deref(), env.as_deref()).ok();
        source(mode, external, cloud, key)
    }
}

/// The impersonation slot's mode as a [`ServerMode`], or `None` for `shared` —
/// which has no engine, and no row, of its own.
fn impersonation_mode(mode: ImpersonationMode) -> Option<ServerMode> {
    match mode {
        ImpersonationMode::Shared => None,
        ImpersonationMode::Managed => Some(ServerMode::Managed),
        ImpersonationMode::External => Some(ServerMode::External),
        ImpersonationMode::OpenAi => Some(ServerMode::OpenAi),
        ImpersonationMode::Gemini => Some(ServerMode::Gemini),
        ImpersonationMode::Claude => Some(ServerMode::Claude),
        ImpersonationMode::Grok => Some(ServerMode::Grok),
    }
}

/// The request for a mode, or why there is none.
///
/// The base URL is the slot's override when it has one — a proxy or a gateway
/// serves its own catalogue, and that is the whole point of the override —
/// **except for Gemini**, whose override addresses the OpenAI-compatible path
/// used for embeddings while the catalogue worth reading is the native one: it
/// is the only Gemini list that publishes `supportedGenerationMethods` (§2.2).
fn source(
    mode: ServerMode,
    external: &ExternalSettings,
    cloud: Option<&CloudSettings>,
    key: Option<String>,
) -> Result<CatalogueRequest, CatalogueError> {
    let blank = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .is_none()
    };
    match mode {
        // The managed row is a GGUF path on this machine (N1).
        ServerMode::Managed => Err(CatalogueError::NotConfigured),
        ServerMode::External => {
            if blank(&external.url) {
                return Err(CatalogueError::NotConfigured);
            }
            Ok(CatalogueRequest {
                shape: CatalogueShape::OpenAi,
                base: external.url.clone().unwrap_or_default().trim().to_string(),
                // A local `llama-server` needs none, and sending nothing is what
                // every other request to it does.
                key,
            })
        }
        ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude | ServerMode::Grok => {
            let provider = mode.cloud_provider().ok_or(CatalogueError::NotConfigured)?;
            let shape = match mode {
                ServerMode::Gemini => CatalogueShape::Gemini,
                ServerMode::Claude => CatalogueShape::Anthropic,
                ServerMode::Grok => CatalogueShape::Xai,
                _ => CatalogueShape::OpenAi,
            };
            // Every cloud needs a key, and asking without one buys a `401` the
            // user cannot read as "add a key".
            let key = key.ok_or(CatalogueError::NoKey)?;
            let override_url = cloud
                .and_then(|c| c.url.as_deref())
                .map(str::trim)
                .filter(|u| !u.is_empty());
            let base = match (shape, override_url) {
                (CatalogueShape::Gemini, _) | (_, None) => provider.chat_base_url().to_string(),
                (_, Some(url)) => url.to_string(),
            };
            Ok(CatalogueRequest {
                shape,
                base,
                key: Some(key),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external(url: Option<&str>) -> ExternalSettings {
        ExternalSettings {
            url: url.map(str::to_string),
            ..ExternalSettings::default()
        }
    }

    #[test]
    fn a_managed_slot_has_no_catalogue() {
        assert_eq!(
            source(ServerMode::Managed, &external(None), None, None),
            Err(CatalogueError::NotConfigured),
            "the managed row is a file on this machine, not a name"
        );
    }

    #[test]
    fn an_external_slot_asks_its_own_url_with_no_key_of_its_own() {
        let req = source(
            ServerMode::External,
            &external(Some(" http://127.0.0.1:8000/v1 ")),
            None,
            None,
        )
        .expect("a request");
        assert_eq!(
            req,
            CatalogueRequest {
                shape: CatalogueShape::OpenAi,
                base: "http://127.0.0.1:8000/v1".to_string(),
                key: None,
            }
        );
    }

    #[test]
    fn an_external_slot_without_an_address_has_nothing_to_ask() {
        assert_eq!(
            source(ServerMode::External, &external(Some("   ")), None, None),
            Err(CatalogueError::NotConfigured)
        );
    }

    #[test]
    fn a_cloud_without_a_key_says_so_instead_of_buying_a_401() {
        assert_eq!(
            source(ServerMode::OpenAi, &external(None), None, None),
            Err(CatalogueError::NoKey)
        );
    }

    #[test]
    fn each_cloud_gets_its_own_shape_and_base() {
        for (mode, shape, base) in [
            (
                ServerMode::OpenAi,
                CatalogueShape::OpenAi,
                "https://api.openai.com/v1",
            ),
            (
                ServerMode::Gemini,
                CatalogueShape::Gemini,
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            (
                ServerMode::Claude,
                CatalogueShape::Anthropic,
                "https://api.anthropic.com",
            ),
            (ServerMode::Grok, CatalogueShape::Xai, "https://api.x.ai/v1"),
        ] {
            let req = source(mode, &external(None), None, Some("k".into())).expect("a request");
            assert_eq!((req.shape, req.base.as_str()), (shape, base), "{mode:?}");
        }
    }

    #[test]
    fn an_override_moves_the_catalogue_except_on_gemini() {
        let proxy = CloudSettings {
            url: Some("https://proxy.test/v1".to_string()),
            ..CloudSettings::default()
        };
        let openai = source(
            ServerMode::OpenAi,
            &external(None),
            Some(&proxy),
            Some("k".into()),
        )
        .expect("a request");
        assert_eq!(
            openai.base, "https://proxy.test/v1",
            "a gateway serves its own catalogue — that is what the override is for"
        );
        let gemini = source(
            ServerMode::Gemini,
            &external(None),
            Some(&proxy),
            Some("k".into()),
        )
        .expect("a request");
        assert_eq!(
            gemini.base, "https://generativelanguage.googleapis.com/v1beta",
            "Gemini's override addresses the compat path, which publishes no methods"
        );
    }
}
