//! Assistant-reply generation lifecycle state machine (`Idle/Generating/
//! Cancelling`) — split out of the orchestrator. A pure type with no I/O: transitions
//! are valid by construction, side effects (spawning tasks, cancelling the token,
//! dispatching events, writing to `Chat`) stay with the orchestrator — the sole
//! owner of the state (spec §4.4, §4.4.2). This keeps the state machine unit-testable
//! without a tokio runtime.
//!
//! Impersonation and RAG indexing are separate concurrent sub-states of the
//! orchestrator (`imp_gen`, `rag_cancel`); they **deliberately** aren't part of this
//! state machine — it only describes the assistant reply lifecycle for the active chat.

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Generation state (a state machine for the active chat). The variants themselves
/// carry the data: the generation `id` (for dropping stale stream events, spec §4.4)
/// and the HTTP-stream cancellation token.
pub enum GenState {
    /// No generation running; `SendMessage`/`RegenerateLast`/`Impersonate` are accepted.
    Idle,
    /// Generation with this `id` is running; `cancel` interrupts the HTTP stream.
    Generating { id: Uuid, cancel: CancellationToken },
    /// Cancellation was requested, but the task is still "landing"; a partial reply
    /// will be saved once `GenResult` arrives with the same `id`.
    Cancelling { id: Uuid },
}

impl GenState {
    /// The `id` of the current generation (for the "apply only the result of your own
    /// request" gate). `None` in `Idle`.
    pub fn current_id(&self) -> Option<Uuid> {
        match self {
            GenState::Idle => None,
            GenState::Generating { id, .. } | GenState::Cancelling { id, .. } => Some(*id),
        }
    }

    /// `true` if no generation is running (the send/regenerate/impersonate gate).
    pub fn is_idle(&self) -> bool {
        matches!(self, GenState::Idle)
    }

    /// `Idle → Generating`. Returns `false` (no transition) if the state machine is
    /// already busy — protection against a parallel start. Called after the `is_idle`
    /// gate.
    pub fn begin(&mut self, id: Uuid, cancel: CancellationToken) -> bool {
        if !self.is_idle() {
            return false;
        }
        *self = GenState::Generating { id, cancel };
        true
    }

    /// `Generating → Cancelling`. Returns the cancellation token (the orchestrator
    /// actually triggers it — cancellation is a side effect). `None` if no generation
    /// is running or cancellation was already requested (a repeat `Cancel` — a no-op).
    pub fn request_cancel(&mut self) -> Option<CancellationToken> {
        if let GenState::Generating { id, cancel } = self {
            let id = *id;
            let token = cancel.clone();
            *self = GenState::Cancelling { id };
            Some(token)
        } else {
            None
        }
    }

    /// The cancellation token of the current generation, without changing state (for
    /// `Quit` — mute the stream on exit, no need to transition into `Cancelling`).
    pub fn active_cancel(&self) -> Option<&CancellationToken> {
        match self {
            GenState::Generating { cancel, .. } => Some(cancel),
            GenState::Idle | GenState::Cancelling { .. } => None,
        }
    }

    /// `Generating|Cancelling → Idle`, only if `id` matches the current generation
    /// (anti-staleness: the tail of an old stream after `Stop → Send` won't reset the
    /// new state). Returns `true` if the transition happened.
    pub fn finish(&mut self, id: Uuid) -> bool {
        if self.current_id() == Some(id) {
            *self = GenState::Idle;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_only_from_idle() {
        let mut s = GenState::Idle;
        let id = Uuid::new_v4();
        assert!(s.begin(id, CancellationToken::new()));
        assert_eq!(s.current_id(), Some(id));
        assert!(!s.is_idle());

        // A repeat begin doesn't overwrite the running generation.
        let other = Uuid::new_v4();
        assert!(!s.begin(other, CancellationToken::new()));
        assert_eq!(s.current_id(), Some(id));
    }

    #[test]
    fn request_cancel_transitions_and_returns_token() {
        let mut s = GenState::Idle;
        // Nothing to cancel in Idle.
        assert!(s.request_cancel().is_none());

        let id = Uuid::new_v4();
        let token = CancellationToken::new();
        s.begin(id, token.clone());

        let returned = s
            .request_cancel()
            .expect("a cancellation token from Generating");
        returned.cancel();
        assert!(
            token.is_cancelled(),
            "the returned token is exactly the live generation token"
        );
        assert!(matches!(s, GenState::Cancelling { .. }));
        assert_eq!(s.current_id(), Some(id));

        // A repeat cancel from Cancelling — a no-op.
        assert!(s.request_cancel().is_none());
    }

    #[test]
    fn finish_matches_current_id() {
        let mut s = GenState::Idle;
        let id = Uuid::new_v4();
        s.begin(id, CancellationToken::new());

        // A stale id doesn't reset the state.
        assert!(!s.finish(Uuid::new_v4()));
        assert!(!s.is_idle());

        // Its own id finishes the generation.
        assert!(s.finish(id));
        assert!(s.is_idle());
    }

    #[test]
    fn finish_works_from_cancelling() {
        let mut s = GenState::Idle;
        let id = Uuid::new_v4();
        s.begin(id, CancellationToken::new());
        s.request_cancel();
        // A partial result of a cancelled generation still finishes the state machine.
        assert!(s.finish(id));
        assert!(s.is_idle());
    }

    #[test]
    fn active_cancel_only_in_generating() {
        let mut s = GenState::Idle;
        assert!(s.active_cancel().is_none());

        let token = CancellationToken::new();
        s.begin(Uuid::new_v4(), token.clone());
        assert!(s.active_cancel().is_some());
        s.active_cancel().unwrap().cancel();
        assert!(token.is_cancelled());

        s.request_cancel();
        assert!(
            s.active_cancel().is_none(),
            "no token is returned in Cancelling"
        );
    }
}
