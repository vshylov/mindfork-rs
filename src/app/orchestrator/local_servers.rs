//! A local Ollama or LM Studio, found and offered
//! (docs/research/local-servers.md §4, stage 2).
//!
//! A chat with no engine configured looks at this machine's own ports at the
//! start, and `/local` looks whenever it is typed. What answers is offered as a
//! list on the chat screen; a pick is written into the settings through the
//! same path the settings screen takes, so the file is saved atomically and the
//! engine comes up as any external server does. Nothing is written unasked.

use crate::app::events::AppEvent;
use crate::shared::api::local_servers::{self, LocalOffer};
use crate::shared::config::ServerMode;
use crate::shared::server::ServerStatus;

use super::Orchestrator;
use super::engines::Server;

impl Orchestrator {
    /// At the start: a chat whose engine is not configured looks for one. A
    /// configured engine that does not answer is `Disconnected`, not
    /// `NotConfigured`, and is not second-guessed (F3).
    pub(super) fn offer_local_servers_at_start(&mut self) {
        if matches!(
            self.engines.status_of(Server::Chat),
            ServerStatus::NotConfigured
        ) {
            self.find_local_servers(false);
        }
    }

    /// Asks Ollama and LM Studio on this machine, in the background, and sends
    /// what answered as the list's rows. `asked` — the user typed `/local`, and
    /// is owed an answer when nothing is found; at the start nothing found is
    /// silence, and the empty feed stays as it is.
    pub(super) fn find_local_servers(&mut self, asked: bool) {
        let embed_free = self.embedder_free();
        let loc = self.ui_locale();
        let tx = self.evt_tx.clone();
        let look = self.engines.find_local_servers();
        tokio::spawn(async move {
            let found = look.await;
            let offers = local_servers::offers(&found, embed_free);
            tracing::info!(
                servers = found.len(),
                offers = offers.len(),
                asked,
                "looked for local servers"
            );
            if !offers.is_empty() {
                let _ = tx.send(AppEvent::LocalServers(offers));
            } else if asked {
                // A server that answered with no chat model is not "nothing".
                let key = if found.is_empty() {
                    "ui.local.none"
                } else {
                    "ui.local.no_models"
                };
                let _ = tx.send(AppEvent::Notice(loc.t(key).into()));
            }
        });
    }

    /// Answers [`AppCommand::UseLocalServer`](crate::app::events::AppCommand::UseLocalServer):
    /// the external section takes the server and the model, and the embedder's
    /// takes the same server and the row's embedder — only while the embedder
    /// is still not configured, asked again here since the list was built
    /// (F5: a configured embedder is never replaced).
    pub(super) fn use_local_server(&mut self, offer: LocalOffer) {
        let mut config = self.config.clone();
        config.engine.mode = ServerMode::External;
        config.engine.external.url = Some(offer.url.clone());
        config.engine.external.model_name = Some(offer.model.clone());
        let embedder = offer.embedder.filter(|_| self.embedder_free());
        if let Some(name) = &embedder {
            config.embed.mode = ServerMode::External;
            config.embed.external.url = Some(offer.url.clone());
            config.embed.external.model_name = Some(name.clone());
        }
        tracing::info!(
            server = ?offer.server,
            url = %offer.url,
            model = %offer.model,
            embedder = ?embedder,
            "a local server was picked"
        );
        self.handle_update_config(config);
        // The list only ever holds the two servers it knows; the URL stands in
        // for a name should that change.
        let server = offer.server.product().unwrap_or(&offer.url);
        let loc = self.ui_locale();
        let text = match &embedder {
            Some(name) => loc.tf(
                "ui.local.used_with_embedder",
                &[
                    ("server", server),
                    ("model", &offer.model),
                    ("embedder", name),
                ],
            ),
            None => loc.tf(
                "ui.local.used",
                &[("server", server), ("model", &offer.model)],
            ),
        };
        let _ = self.evt_tx.send(AppEvent::Notice(text));
    }

    /// The embedder is not configured — the default, a managed section with no
    /// model — so a pick may set it.
    fn embedder_free(&self) -> bool {
        matches!(
            self.engines.status_of(Server::Embed),
            ServerStatus::NotConfigured
        )
    }
}
