//! [`EngineManager`] — the lifecycle of the inference/embedding servers: owns the
//! engines (`backend`/`imp_backend`/`embedder`), handles to managed processes
//! (`*_handle`, `kill_on_drop`), readiness statuses, and background-probe channels.
//! Extracted from the orchestrator (Phase 3): groups ~11 fields and the server logic
//! into a cohesive unit, a counterpart to the [`ServerSupervisor`] trait. The orchestrator
//! remains the sole owner of `Chat`; here — only servers, no domain state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::events::{ServerStatus, ServerStatuses};
use crate::app::supervisor::ServerSupervisor;
use crate::shared::api::{Embedder, EngineBackend, ServerHandle};
use crate::shared::config::{
    EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode,
};
use crate::shared::i18n::Locale;
use crate::shared::secrets::{ApiKeyEntry, SecretKey};

/// Decrypts the stored key of whichever secret the slot's active mode reads
/// (`settings.secret_key()` — a cloud provider's key, or an external server's own;
/// see `config::mode_secret_key`). `None` — the mode needs no key (managed), the key
/// isn't stored, or the entry is a foreign one → the supervisor falls back to env.
/// The resolution lives here so the supervisor doesn't need to know the
/// secret-storage format (`shared::secrets`). See docs/research/api-key-storage.md,
/// docs/history/external-api-key.md §5.3.
fn stored_key(api_keys: &[ApiKeyEntry], key: Option<SecretKey>) -> Option<String> {
    crate::shared::secrets::stored_key(api_keys, &key?.storage_name())
}

/// Max relaunches of one managed server within [`RESTART_WINDOW`]; beyond that it
/// stays `Disconnected` until manual intervention (editing settings re-applies it).
pub(super) const RESTART_BUDGET: usize = 3;
/// The restart budget's window.
const RESTART_WINDOW: Duration = Duration::from_secs(300);

/// Which managed server a relaunch budget belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Server {
    Chat,
    Embed,
    Impersonation,
}

/// A crash-loop guard for relaunching a managed server: a dead child can only be
/// revived by launching a new process, and a server that dies *because* of its
/// configuration (a corrupt GGUF, an OOM) would otherwise be respawned forever.
///
/// The same shape as `McpManager::allow_restart` — deliberately a second small
/// implementation rather than a shared one: unifying them is a mechanical refactor
/// and shouldn't ride along with a behavior change (AGENTS.md §2).
#[derive(Default)]
struct RestartBudget {
    marks: Vec<Instant>,
}

impl RestartBudget {
    /// Whether one more relaunch is allowed now: prunes marks older than the window,
    /// records this attempt if there's room.
    fn allow(&mut self, now: Instant) -> bool {
        self.marks
            .retain(|t| now.duration_since(*t) < RESTART_WINDOW);
        if self.marks.len() < RESTART_BUDGET {
            self.marks.push(now);
            true
        } else {
            false
        }
    }

    /// A server that came up healthy again starts with a clean budget — otherwise a
    /// machine that goes down once a week would eventually exhaust it.
    fn reset(&mut self) {
        self.marks.clear();
    }
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
    /// What each server was last **actually launched with**: its settings plus the key
    /// blob they were resolved from (`stored_key` reads the key inside `apply_*`, so
    /// both together decide what a relaunch would produce).
    ///
    /// [`Orchestrator::flush_restarts`] compares against this rather than against the
    /// previous edit: the debounce flag only says "something was edited", and an edit
    /// followed by its undo (`Ctrl+Z`, spec §11.6) raises it twice while leaving the
    /// config exactly as the server is already running. Recorded by `apply_*` itself,
    /// so a crash relaunch — which re-applies the same settings — keeps it accurate.
    ///
    /// `None` before the first apply, so the initial launch always happens.
    ///
    /// [`Orchestrator::flush_restarts`]: super::Orchestrator::flush_restarts
    applied_chat: Option<(EngineSettings, Vec<ApiKeyEntry>)>,
    applied_embed: Option<(EmbedSettings, Vec<ApiKeyEntry>)>,
    applied_imp: Option<(ImpersonationEngineSettings, Vec<ApiKeyEntry>)>,
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
    /// Relaunch budgets for the managed servers (chat/embed/impersonation).
    restarts: [RestartBudget; 3],
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
            applied_chat: None,
            applied_embed: None,
            applied_imp: None,
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
            restarts: Default::default(),
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
        let key = stored_key(api_keys, settings.secret_key());
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
        self.applied_chat = Some((settings.clone(), api_keys.to_vec()));
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
        let key = stored_key(api_keys, settings.secret_key());
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
        self.applied_embed = Some((settings.clone(), api_keys.to_vec()));
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
                let key = stored_key(api_keys, settings.secret_key());
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
        // Recorded for both arms: `shared` mode is a state the server can be *in*, so
        // switching away from it and back must not read as "nothing to do".
        self.applied_imp = Some((settings.clone(), api_keys.to_vec()));
    }

    /// Whether a server is already running exactly this configuration — i.e. whether
    /// re-applying it would change anything. `false` before the first apply.
    ///
    /// The comparison covers the key blob as well as the settings: the key is resolved
    /// inside `apply_*`, so equal settings with a different blob still warrant a
    /// relaunch. Comparing the stored ciphertext (never the plaintext) is deliberate —
    /// it errs towards restarting, which is the safe direction.
    pub(super) fn chat_is_current(&self, s: &EngineSettings, keys: &[ApiKeyEntry]) -> bool {
        self.applied_chat
            .as_ref()
            .is_some_and(|(a, k)| a == s && k == keys)
    }

    pub(super) fn embed_is_current(&self, s: &EmbedSettings, keys: &[ApiKeyEntry]) -> bool {
        self.applied_embed
            .as_ref()
            .is_some_and(|(a, k)| a == s && k == keys)
    }

    pub(super) fn impersonation_is_current(
        &self,
        s: &ImpersonationEngineSettings,
        keys: &[ApiKeyEntry],
    ) -> bool {
        self.applied_imp
            .as_ref()
            .is_some_and(|(a, k)| a == s && k == keys)
    }

    /// Updates the chat-server status (from the background monitor).
    pub(super) fn set_chat_status(&mut self, status: ServerStatus) {
        self.note_recovery(Server::Chat, &status);
        self.server_status = status;
    }

    /// Updates the impersonation-server status (from the background monitor).
    pub(super) fn set_imp_status(&mut self, status: ServerStatus) {
        self.note_recovery(Server::Impersonation, &status);
        self.imp_status = status;
    }

    /// Updates the embedding-server status (from the background monitor).
    pub(super) fn set_embed_status(&mut self, status: ServerStatus) {
        self.note_recovery(Server::Embed, &status);
        self.embed_status = status;
    }

    /// A server that reached `Ready` gets a clean relaunch budget: the budget exists
    /// to stop a crash *loop*, not to count a machine's lifetime outages.
    fn note_recovery(&mut self, server: Server, status: &ServerStatus) {
        if matches!(status, ServerStatus::Ready) {
            self.restarts[server as usize].reset();
        }
    }

    /// Whether a dead managed `server` may be relaunched right now (crash-loop guard).
    pub(super) fn allow_relaunch(&mut self, server: Server, now: Instant) -> bool {
        self.restarts[server as usize].allow(now)
    }

    /// The last published status of `server` — for deciding whether it needs reviving.
    pub(super) fn status_of(&self, server: Server) -> &ServerStatus {
        match server {
            Server::Chat => &self.server_status,
            Server::Embed => &self.embed_status,
            Server::Impersonation => &self.imp_status,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard's whole job: allow a few relaunches, then stop. Without the cap, a
    /// server that dies *because* of its configuration would be respawned forever.
    #[test]
    fn budget_allows_up_to_the_cap_then_refuses() {
        let mut b = RestartBudget::default();
        let now = Instant::now();
        for i in 0..RESTART_BUDGET {
            assert!(b.allow(now), "relaunch {i} should be allowed");
        }
        assert!(!b.allow(now), "the cap should stop the crash loop");
    }

    /// The cap is per window, not per lifetime: an outage a week later starts fresh.
    #[test]
    fn budget_forgets_marks_older_than_the_window() {
        let mut b = RestartBudget::default();
        let now = Instant::now();
        for _ in 0..RESTART_BUDGET {
            b.allow(now);
        }
        assert!(!b.allow(now));
        assert!(
            b.allow(now + RESTART_WINDOW + Duration::from_secs(1)),
            "marks older than the window should be pruned"
        );
    }

    /// Reaching `Ready` clears the budget — otherwise a machine that reboots once a
    /// month would eventually exhaust it and stop recovering.
    #[test]
    fn reaching_ready_clears_the_budget() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
        let (tx3, _rx3) = tokio::sync::mpsc::unbounded_channel();
        let mut m = EngineManager::new(
            Arc::new(crate::app::supervisor::MockSupervisor::with_backend(None)),
            tx,
            tx2,
            tx3,
        );
        let now = Instant::now();
        for _ in 0..RESTART_BUDGET {
            assert!(m.allow_relaunch(Server::Chat, now));
        }
        assert!(!m.allow_relaunch(Server::Chat, now));
        m.set_chat_status(ServerStatus::Ready);
        assert!(
            m.allow_relaunch(Server::Chat, now),
            "a recovered server should get a clean budget"
        );
    }

    /// Budgets are per server: a flapping embedding server mustn't spend the chat
    /// server's allowance.
    #[test]
    fn budgets_are_independent_per_server() {
        let mut m = EngineManager::new(
            Arc::new(crate::app::supervisor::MockSupervisor::with_backend(None)),
            tokio::sync::mpsc::unbounded_channel().0,
            tokio::sync::mpsc::unbounded_channel().0,
            tokio::sync::mpsc::unbounded_channel().0,
        );
        let now = Instant::now();
        for _ in 0..RESTART_BUDGET {
            m.allow_relaunch(Server::Embed, now);
        }
        assert!(!m.allow_relaunch(Server::Embed, now));
        assert!(m.allow_relaunch(Server::Chat, now));
    }
}
