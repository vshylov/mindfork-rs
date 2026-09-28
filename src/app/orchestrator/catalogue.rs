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
        source(slot, mode, external, cloud, key)
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
        ImpersonationMode::OpenRouter => Some(ServerMode::OpenRouter),
    }
}

/// The request for a mode, or why there is none.
///
/// The base URL is the slot's override when it has one — a proxy or a gateway
/// serves its own catalogue, and that is the whole point of the override —
/// **except for Gemini**, whose override addresses the OpenAI-compatible path
/// used for embeddings while the catalogue worth reading is the native one: it
/// is the only Gemini list that publishes `supportedGenerationMethods` (§2.2).
///
/// `slot` matters to one provider: the OpenRouter gateway keeps its embedding
/// models in a list of their own, and its catalogues are public — so that slot
/// is the one cloud whose picker opens before a key was entered
/// (docs/research/openrouter-mode.md, fork F7).
fn source(
    slot: ModelSlot,
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
        ServerMode::OpenRouter => {
            let shape = match slot {
                ModelSlot::Embedder => CatalogueShape::OpenRouterEmbeddings,
                ModelSlot::Assistant | ModelSlot::Impersonation => CatalogueShape::OpenRouter,
            };
            let base = cloud
                .and_then(|c| c.url.as_deref())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| mode_base(mode))
                .to_string();
            Ok(CatalogueRequest { shape, base, key })
        }
        ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude | ServerMode::Grok => {
            let provider = mode.cloud_provider().ok_or(CatalogueError::NotConfigured)?;
            let shape = match mode {
                ServerMode::Gemini => CatalogueShape::Gemini,
                ServerMode::Claude => CatalogueShape::Anthropic,
                ServerMode::Grok => CatalogueShape::Xai,
                // Named rather than left to `_`: a provider added later must
                // choose its shape here, not inherit this one in silence.
                ServerMode::OpenAi => CatalogueShape::OpenAi,
                ServerMode::Managed | ServerMode::External | ServerMode::OpenRouter => {
                    return Err(CatalogueError::NotConfigured);
                }
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

/// The provider's own chat base for a cloud mode (empty for a mode that has
/// none, which no caller passes).
fn mode_base(mode: ServerMode) -> &'static str {
    mode.cloud_provider()
        .map(|p| p.chat_base_url())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot every test below asks for unless it is about the slot.
    const CHAT: ModelSlot = ModelSlot::Assistant;

    fn external(url: Option<&str>) -> ExternalSettings {
        ExternalSettings {
            url: url.map(str::to_string),
            ..ExternalSettings::default()
        }
    }

    #[test]
    fn a_managed_slot_has_no_catalogue() {
        assert_eq!(
            source(CHAT, ServerMode::Managed, &external(None), None, None),
            Err(CatalogueError::NotConfigured),
            "the managed row is a file on this machine, not a name"
        );
    }

    #[test]
    fn an_external_slot_asks_its_own_url_with_no_key_of_its_own() {
        let req = source(
            CHAT,
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
            source(
                CHAT,
                ServerMode::External,
                &external(Some("   ")),
                None,
                None
            ),
            Err(CatalogueError::NotConfigured)
        );
    }

    #[test]
    fn a_cloud_without_a_key_says_so_instead_of_buying_a_401() {
        assert_eq!(
            source(CHAT, ServerMode::OpenAi, &external(None), None, None),
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
            let req =
                source(CHAT, mode, &external(None), None, Some("k".into())).expect("a request");
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
            CHAT,
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
            CHAT,
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

    // ---------- the OpenRouter mode (docs/research/openrouter-mode.md, fork F7) ----------

    const GATEWAY_BASE: &str = "https://openrouter.ai/api/v1";

    /// What the gateway mode asks for a slot, with the section and the key it
    /// was given.
    fn gateway(
        slot: ModelSlot,
        cloud: Option<&CloudSettings>,
        key: Option<&str>,
    ) -> Result<CatalogueRequest, CatalogueError> {
        let external = external(Some("http://127.0.0.1:8000/v1"));
        let key = key.map(str::to_string);
        source(slot, ServerMode::OpenRouter, &external, cloud, key)
    }

    /// The gateway's catalogue is public (§3.2: `GET /models` needs no key), so
    /// its picker opens **before** a key was entered — choosing a model is how
    /// one decides to get a key. Every other cloud keeps answering "no key"
    /// without a request, since asking would buy a `401`.
    #[test]
    fn the_gateways_catalogue_is_asked_before_a_key_was_entered() {
        assert_eq!(
            gateway(CHAT, None, None),
            Ok(CatalogueRequest {
                shape: CatalogueShape::OpenRouter,
                base: GATEWAY_BASE.to_string(),
                key: None,
            })
        );
        // A key, once there is one, travels: it is what makes the list the
        // account's own.
        let keyed = gateway(CHAT, None, Some("k")).expect("a request");
        assert_eq!(keyed.key.as_deref(), Some("k"));

        let keyless: Vec<_> = ServerMode::ALL
            .into_iter()
            .filter(|mode| {
                source(CHAT, *mode, &external(None), None, None) == Err(CatalogueError::NoKey)
            })
            .collect();
        assert_eq!(
            keyless,
            [
                ServerMode::OpenAi,
                ServerMode::Gemini,
                ServerMode::Claude,
                ServerMode::Grok
            ],
            "the modes that refuse to ask without a key"
        );
    }

    /// The gateway keeps its embedding models in a list of their own, so the
    /// slot decides the shape — for this provider alone: another cloud's
    /// embedder row reads the one catalogue that cloud has.
    #[test]
    fn the_embedder_slot_asks_the_gateway_for_its_embedding_models() {
        let shape = |slot| gateway(slot, None, None).map(|req| req.shape);
        assert_eq!(shape(ModelSlot::Assistant), Ok(CatalogueShape::OpenRouter));
        assert_eq!(
            shape(ModelSlot::Impersonation),
            Ok(CatalogueShape::OpenRouter)
        );
        assert_eq!(
            shape(ModelSlot::Embedder),
            Ok(CatalogueShape::OpenRouterEmbeddings)
        );
        let openai = source(
            ModelSlot::Embedder,
            ServerMode::OpenAi,
            &external(None),
            None,
            Some("k".into()),
        );
        assert_eq!(openai.map(|req| req.shape), Ok(CatalogueShape::OpenAi));
    }

    /// The section's base URL moves the gateway's catalogue with the chat — a
    /// regional host or a proxy in front of it serves its own list — while a
    /// blank override is no override, and the `external` section's address,
    /// which belongs to another mode, is never what is asked.
    #[test]
    fn an_override_moves_the_gateways_catalogue_and_a_blank_one_does_not() {
        let section = |url: &str| CloudSettings {
            url: Some(url.to_string()),
            ..CloudSettings::default()
        };
        let base = |url: &str, slot| gateway(slot, Some(&section(url)), None).map(|req| req.base);
        for slot in [CHAT, ModelSlot::Embedder] {
            assert_eq!(
                base(" https://eu.openrouter.ai/api/v1 ", slot).as_deref(),
                Ok("https://eu.openrouter.ai/api/v1"),
                "{slot:?}"
            );
            assert_eq!(base("   ", slot).as_deref(), Ok(GATEWAY_BASE), "{slot:?}");
        }
        let unset = gateway(CHAT, Some(&CloudSettings::default()), None);
        assert_eq!(unset.map(|req| req.base).as_deref(), Ok(GATEWAY_BASE));
    }

    /// Impersonation carries a mode enum of its own, and the picker reads it
    /// through this mapping — so a variant mapped to a neighbour would ask
    /// another provider's catalogue for the impersonation row. The two enums
    /// spell one engine the same way, so each mode is read from the word the
    /// settings file holds and has to come back as the engine of that word;
    /// `shared` has no engine, and no row.
    #[test]
    fn impersonation_names_the_engine_its_own_mode_names() {
        let engine = |word: &str| {
            let mode: ImpersonationMode =
                serde_json::from_value(serde_json::json!(word)).expect(word);
            impersonation_mode(mode).map(ServerMode::key)
        };
        assert_eq!(engine("shared"), None);
        let named: Vec<&str> = ServerMode::ALL.into_iter().map(ServerMode::key).collect();
        assert_eq!(named.last(), Some(&"openrouter"), "the mode under test");
        for word in named {
            assert_eq!(engine(word), Some(word));
        }
    }
}
