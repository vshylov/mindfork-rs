//! Inference-server connection status. Lives in `shared` because it's needed
//! by both the orchestrator (`app`) and the status bar (`widgets`) —
//! dependency strictly downward.

/// Inference-server connection status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    /// The server isn't configured (no URL/binary).
    NotConfigured,
    /// Connecting/starting up.
    Connecting,
    /// Ready to work.
    Ready,
    /// Unavailable (with a reason).
    Disconnected(String),
}

/// A snapshot of every inference server's status, for the status bar. The
/// chat server is always shown (with the disconnect reason, since it blocks
/// generation); embeddings and impersonation get their own chips and are
/// shown **only when configured** (`NotConfigured`, including impersonation
/// in `shared` mode, → the chip is hidden, doesn't clutter the line). Emitted
/// by the orchestrator on any change to any of the statuses. See spec §11.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatuses {
    /// The assistant's primary (chat) server — always shown.
    pub chat: ServerStatus,
    /// The embedding server (RAG). `NotConfigured` → the chip is hidden.
    pub embed: ServerStatus,
    /// The impersonation server. `NotConfigured` (including `shared` mode) →
    /// the chip is hidden.
    pub impersonation: ServerStatus,
}
