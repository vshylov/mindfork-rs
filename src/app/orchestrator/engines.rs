//! [`EngineManager`] — the lifecycle of the inference/embedding servers: owns the
//! engines (`backend`/`imp_backend`/`embedder`), handles to managed processes
//! (`*_handle`, `kill_on_drop`), readiness statuses, and background-probe channels.
//! Extracted from the orchestrator (Phase 3): groups ~11 fields and the server logic
//! into a cohesive unit, a counterpart to the [`ServerSupervisor`] trait. The orchestrator
//! remains the sole owner of `Chat`; here — only servers, no domain state.

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::events::{ServerStatus, ServerStatuses};
use crate::app::supervisor::ServerSupervisor;
use crate::shared::api::{Embedder, EngineBackend, ServerHandle};
use crate::shared::config::{
    CloudProvider, EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode,
};
use crate::shared::i18n::Locale;
use crate::shared::secrets::ApiKeyEntry;

/// Decrypts the provider's stored key (**this** machine's entry). `None` —
/// local mode (no provider), the key isn't stored, or the entry is a foreign one → the
/// supervisor falls back to env. The resolution lives here so the supervisor doesn't need
/// to know the secret-storage format (`shared::secrets`). See docs/research/api-key-storage.md.
fn stored_key(api_keys: &[ApiKeyEntry], provider: Option<CloudProvider>) -> Option<String> {
    crate::shared::secrets::stored_key(api_keys, provider?.key())
}

pub(super) struct EngineManager {
    /// The server supervisor (for restarting on a model/server change).
    supervisor: Arc<dyn ServerSupervisor>,
    /// The assistant's engine. `None` — external/not configured.
    pub(super) backend: Option<Arc<dyn EngineBackend>>,
    /// A handle to the managed chat process (drop → kill).
    chat_handle: Option<ServerHandle>,
    /// A handle to the managed embedding process.
    embed_handle: Option<ServerHandle>,
    /// The impersonation engine for managed/external modes (`None` in `shared` mode —
    /// then the assistant's `backend` is used). See spec §11.8.
    imp_backend: Option<Arc<dyn EngineBackend>>,
    /// A handle to the managed process of the impersonation server.
    imp_handle: Option<ServerHandle>,
    /// The current chat-server status. Generation only starts in `Ready`: a request to
    /// a still-loading (`Connecting`) managed server would return a 503 ("error
    /// status"), and for regeneration it would also wipe out the previous reply for nothing.
    pub(super) server_status: ServerStatus,
    /// The impersonation-server status (for managed/external; in `shared` —
    /// `NotConfigured`, the chip in the status line is hidden).
    imp_status: ServerStatus,
    /// The embedding-server status: `NotConfigured` (`UnavailableEmbedder`, the chip is
    /// hidden) or, for a configured one, `Connecting` → `Ready`/`Disconnected` from the
    /// background probe (the cloud is `Ready` at once). Doesn't gate anything — RAG is
    /// lazy (ADR 0002), the status is informational.
    embed_status: ServerStatus,
    /// The chat-server status channel for the supervisor's background probe.
    status_tx: UnboundedSender<ServerStatus>,
    /// The impersonation-server status channel (background probe).
    imp_status_tx: UnboundedSender<ServerStatus>,
    /// The embedding-server status channel (background probe).
    embed_status_tx: UnboundedSender<ServerStatus>,
    /// The invalidation token for the current chat server's background probe: a mode/model
    /// change marks the previous probe stale, so its late result (e.g. a timeout of an
    /// intermediate external server while flipping through managed→external→openai) doesn't
    /// overwrite the new server's status.
    chat_probe_cancel: Option<CancellationToken>,
    /// The invalidation token for the impersonation server's background probe (analogous).
    imp_probe_cancel: Option<CancellationToken>,
    /// The invalidation token for the embedding server's background probe (analogous).
    embed_probe_cancel: Option<CancellationToken>,
    /// The embeddings source for RAG (a dedicated server — ADR 0002).
    pub(super) embedder: Arc<dyn Embedder>,
}

impl EngineManager {
    /// Creates a manager with no servers raised (statuses `NotConfigured`, the embedder —
    /// [`UnavailableEmbedder`]). Servers are raised by the subsequent `apply_*` calls.
    pub(super) fn new(
        supervisor: Arc<dyn ServerSupervisor>,
        status_tx: UnboundedSender<ServerStatus>,
        imp_status_tx: UnboundedSender<ServerStatus>,
        embed_status_tx: UnboundedSender<ServerStatus>,
    ) -> Self {
        Self {
            supervisor,
            backend: None,
            chat_handle: None,
            embed_handle: None,
            imp_backend: None,
            imp_handle: None,
            server_status: ServerStatus::NotConfigured,
            imp_status: ServerStatus::NotConfigured,
            embed_status: ServerStatus::NotConfigured,
            status_tx,
            imp_status_tx,
            embed_status_tx,
            chat_probe_cancel: None,
            imp_probe_cancel: None,
            embed_probe_cancel: None,
            embedder: Arc::new(crate::shared::api::UnavailableEmbedder),
        }
    }

    /// (Re-)raises the chat server from settings: kills the previous managed process,
    /// asks the supervisor to set up a new one, stores the immediate status. The caller
    /// takes the status snapshot for the UI via [`Self::statuses`]. `api_keys` —
    /// stored keys from the config (see [`stored_key`]).
    pub(super) fn apply_chat(
        &mut self,
        settings: &EngineSettings,
        api_keys: &[ApiKeyEntry],
        loc: &'static Locale,
    ) {
        self.chat_handle = None; // drop the old managed process (kill_on_drop)
        // Invalidate the previous server's probe and raise a new token.
        if let Some(tok) = self.chat_probe_cancel.take() {
            tok.cancel();
        }
        let cancel = CancellationToken::new();
        self.chat_probe_cancel = Some(cancel.clone());
        let key = stored_key(api_keys, settings.mode.cloud_provider());
        let setup = self.supervisor.apply_chat(
            settings,
            key.as_deref(),
            cancel,
            self.status_tx.clone(),
            loc,
        );
        self.backend = setup.backend;
        self.chat_handle = setup.handle;
        self.server_status = setup.status;
    }

    /// (Re-)raises the embedding server from settings. Like [`Self::apply_chat`]: the
    /// previous managed process is dropped, the previous probe is invalidated, and the
    /// immediate status is stored (real readiness arrives via `embed_status_tx`).
    pub(super) fn apply_embed(
        &mut self,
        settings: &EmbedSettings,
        api_keys: &[ApiKeyEntry],
        loc: &'static Locale,
    ) {
        self.embed_handle = None; // drop the old managed process (kill_on_drop)
        if let Some(tok) = self.embed_probe_cancel.take() {
            tok.cancel();
        }
        let cancel = CancellationToken::new();
        self.embed_probe_cancel = Some(cancel.clone());
        let key = stored_key(api_keys, settings.mode.cloud_provider());
        let setup = self.supervisor.apply_embed(
            settings,
            key.as_deref(),
            cancel,
            self.embed_status_tx.clone(),
            loc,
        );
        self.embedder = setup.embedder;
        self.embed_handle = setup.handle;
        self.embed_status = setup.status;
    }

    /// (Re-)raises the impersonation server. In `shared` mode a separate server isn't
    /// needed — the assistant's chat server is reused.
    pub(super) fn apply_impersonation(
        &mut self,
        settings: &ImpersonationEngineSettings,
        api_keys: &[ApiKeyEntry],
        loc: &'static Locale,
    ) {
        self.imp_handle = None; // drop the previous managed process (kill_on_drop)
        // Invalidate the previous impersonation server's probe (as with the chat server).
        if let Some(tok) = self.imp_probe_cancel.take() {
            tok.cancel();
        }
        match settings.mode {
            ImpersonationMode::Shared => {
                self.imp_backend = None;
                self.imp_status = ServerStatus::NotConfigured;
            }
            _ => {
                let cancel = CancellationToken::new();
                self.imp_probe_cancel = Some(cancel.clone());
                let key = stored_key(api_keys, settings.mode.cloud_provider());
                let setup = self.supervisor.apply_impersonation(
                    settings,
                    key.as_deref(),
                    cancel,
                    self.imp_status_tx.clone(),
                    loc,
                );
                self.imp_backend = setup.backend;
                self.imp_handle = setup.handle;
                self.imp_status = setup.status;
            }
        }
    }

    /// Updates the chat-server status (from the background probe).
    pub(super) fn set_chat_status(&mut self, status: ServerStatus) {
        self.server_status = status;
    }

    /// Updates the impersonation-server status (from the background probe).
    pub(super) fn set_imp_status(&mut self, status: ServerStatus) {
        self.imp_status = status;
    }

    /// Updates the embedding-server status (from the background probe).
    pub(super) fn set_embed_status(&mut self, status: ServerStatus) {
        self.embed_status = status;
    }

    /// A snapshot of all server statuses for the status bar (chat always, embeddings/
    /// impersonation — as chips hidden when `NotConfigured`). See spec §11.1.
    pub(super) fn statuses(&self) -> ServerStatuses {
        ServerStatuses {
            chat: self.server_status.clone(),
            embed: self.embed_status.clone(),
            impersonation: self.imp_status.clone(),
        }
    }

    /// Returns the assistant's engine if the chat server is ready (`Ready`); otherwise — `Err`
    /// with a clear message (not configured / still connecting / unavailable), localized in
    /// the interface language (`loc`, axis B — this is shown to the human, not the model).
    /// Emits nothing itself — the caller decides where to route the error. See spec §7.
    pub(super) fn backend_if_ready(
        &self,
        loc: &'static Locale,
    ) -> Result<Arc<dyn EngineBackend>, String> {
        match &self.server_status {
            ServerStatus::Ready => self
                .backend
                .clone()
                .ok_or_else(|| loc.t("ui.err.server.not_configured").to_string()),
            ServerStatus::Connecting => Err(loc.t("ui.err.server.connecting").to_string()),
            ServerStatus::NotConfigured => Err(loc.t("ui.err.server.not_configured").to_string()),
            ServerStatus::Disconnected(reason) => {
                Err(loc.tf("ui.err.server.unavailable", &[("reason", reason)]))
            }
        }
    }

    /// Returns the impersonation engine if it's ready; otherwise — `Err` with a clear,
    /// localized message (axis B). In `shared` mode the assistant's chat server is used.
    pub(super) fn impersonation_backend_if_ready(
        &self,
        mode: ImpersonationMode,
        loc: &'static Locale,
    ) -> Result<Arc<dyn EngineBackend>, String> {
        if mode == ImpersonationMode::Shared {
            return self.backend_if_ready(loc);
        }
        match &self.imp_status {
            ServerStatus::Ready => self
                .imp_backend
                .clone()
                .ok_or_else(|| loc.t("ui.err.server.imp_not_configured").to_string()),
            ServerStatus::Connecting => Err(loc.t("ui.err.server.imp_connecting").to_string()),
            ServerStatus::NotConfigured => {
                Err(loc.t("ui.err.server.imp_not_configured").to_string())
            }
            ServerStatus::Disconnected(reason) => {
                Err(loc.tf("ui.err.server.imp_unavailable", &[("reason", reason)]))
            }
        }
    }

    /// A clone of the embeddings source for RAG background tasks / the tool context.
    pub(super) fn embedder(&self) -> Arc<dyn Embedder> {
        self.embedder.clone()
    }
}
