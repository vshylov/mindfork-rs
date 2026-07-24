//! [`SaveQueue`] — a debounce queue for deferred saving of chats to disk. Extracted
//! from the orchestrator (Phase 3): holds the set of "dirty" chats and the write
//! deadline. The actual write stays with the orchestrator (the owner of `Chat`
//! and `Storage`) — the queue just tracks what and when to flush.

use std::collections::HashSet;
use std::time::Duration;

use tokio::time::Instant;
use uuid::Uuid;

/// Debounce for saving changed chats to disk.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

#[derive(Default)]
pub(super) struct SaveQueue {
    /// Chats waiting to be written to disk.
    dirty: HashSet<Uuid>,
    /// The moment the queue should be flushed (extended by every mark).
    deadline: Option<Instant>,
}

impl SaveQueue {
    /// Marks a chat for deferred saving and extends the deadline (debounce).
    pub(super) fn mark(&mut self, id: Uuid) {
        self.dirty.insert(id);
        self.deadline = Some(Instant::now() + SAVE_DEBOUNCE);
    }

    /// Removes a chat from the queue (e.g. on deletion — nothing left to save).
    pub(super) fn forget(&mut self, id: Uuid) {
        self.dirty.remove(&id);
    }

    /// The current flush deadline (for the loop's `select!` timer). `None` — the queue is empty.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Takes all pending ids and resets the deadline (for writing to disk).
    pub(super) fn take(&mut self) -> Vec<Uuid> {
        self.deadline = None;
        self.dirty.drain().collect()
    }

    /// Whether a chat is queued (used by tests).
    #[cfg(test)]
    pub(super) fn is_dirty(&self, id: Uuid) -> bool {
        self.dirty.contains(&id)
    }
}
