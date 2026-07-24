//! [`RestartQueue`] — a debounce queue for deferred (re)launch of the inference/
//! embedding servers on engine-settings edits (a mirror of `SaveQueue`).
//! The settings screen applies an edit on every field's commit, so a series
//! like "binary → model → -ngl" without a debounce would produce three heavy
//! managed `llama-server` restarts back to back. The config itself is saved and re-emitted
//! to the UI right away (see `Orchestrator::handle_update_config`) — only the
//! expensive process restart is deferred; it itself stays with the orchestrator
//! (`Orchestrator::flush_restarts`), the queue just tracks which servers to
//! restart and when. The initial server bring-up (before the `run` loop) doesn't
//! go through the queue — it's immediate.

use std::time::Duration;

use tokio::time::Instant;

/// The quiet pause after the last engine-settings edit, before a (re)launch.
const RESTART_DEBOUNCE: Duration = Duration::from_millis(1200);

#[derive(Default)]
pub(super) struct RestartQueue {
    /// Is the chat server (`config.engine`) awaiting a (re)launch.
    chat: bool,
    /// Is the embedding server (`config.embed`) awaiting a (re)launch.
    embed: bool,
    /// Is the impersonation server (`config.impersonation_engine`) awaiting a (re)launch.
    impersonation: bool,
    /// Are the MCP servers (`config.mcp`) awaiting a (re)raise.
    mcp: bool,
    /// The trigger moment (extended by every mark — a debounce from the latest one).
    deadline: Option<Instant>,
}

impl RestartQueue {
    /// Flags the chat server for a deferred (re)launch and extends the deadline.
    pub(super) fn mark_chat(&mut self) {
        self.chat = true;
        self.bump();
    }

    /// Flags the embedding server for a deferred (re)launch and extends the deadline.
    pub(super) fn mark_embed(&mut self) {
        self.embed = true;
        self.bump();
    }

    /// Flags the impersonation server for a deferred (re)launch and extends the deadline.
    pub(super) fn mark_impersonation(&mut self) {
        self.impersonation = true;
        self.bump();
    }

    /// Flags the MCP servers for a deferred (re)raise and extends the deadline.
    pub(super) fn mark_mcp(&mut self) {
        self.mcp = true;
        self.bump();
    }

    fn bump(&mut self) {
        self.deadline = Some(Instant::now() + RESTART_DEBOUNCE);
    }

    /// The current deadline (for the loop's `select!` timer). `None` — the queue is empty.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Takes the `(chat, embed, impersonation, mcp)` flags and resets the queue.
    pub(super) fn take(&mut self) -> (bool, bool, bool, bool) {
        self.deadline = None;
        let out = (self.chat, self.embed, self.impersonation, self.mcp);
        self.chat = false;
        self.embed = false;
        self.impersonation = false;
        self.mcp = false;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Repeated marks coalesce: `take` hands out each server exactly once,
    /// unmarked ones are left alone, the deadline is reset.
    #[tokio::test]
    async fn marks_coalesce_and_take_resets() {
        let mut q = RestartQueue::default();
        assert!(q.deadline().is_none(), "an empty queue has no deadline");
        q.mark_chat();
        q.mark_chat();
        q.mark_impersonation();
        q.mark_mcp();
        assert!(q.deadline().is_some());
        assert_eq!(q.take(), (true, false, true, true));
        assert!(q.deadline().is_none(), "take resets the deadline");
        assert_eq!(
            q.take(),
            (false, false, false, false),
            "a repeated take is empty"
        );
    }

    /// Every mark extends the deadline — the restart happens from the latest edit,
    /// not the first (otherwise a long series of edits would catch a restart in the middle).
    #[tokio::test(start_paused = true)]
    async fn each_mark_extends_deadline() {
        let mut q = RestartQueue::default();
        q.mark_chat();
        let d1 = q.deadline().unwrap();
        tokio::time::advance(Duration::from_millis(500)).await;
        q.mark_embed();
        let d2 = q.deadline().unwrap();
        assert_eq!(
            d2 - d1,
            Duration::from_millis(500),
            "the deadline is extended"
        );
        assert_eq!(q.take(), (true, true, false, false));
    }
}
