//! What the engine says about how many requests it serves at once.
//!
//! The `sessions` field of every engine section (spec §11.6) is typed by the
//! user; a `llama-server` also *reports* its slot count (`total_slots` on
//! `/props`), and the settings screen shows that next to the field as a hint
//! — never as the value (docs/research/parallel-subagents.md fork F8: what the
//! app opens is what is written, and a server that cannot say leaves the hint
//! empty). This module asks the engine and carries the answer to the UI.
//!
//! The same shape as [`ModelDiscovery`](super::model_name::ModelDiscovery), for
//! the same reasons: an `epoch` so an answer that belongs to a replaced engine
//! is dropped, a `pending` flag so the question is asked once per engine. Asked
//! eagerly, when the engine is applied and when its readiness flips — a server
//! that was down could not answer and one that just came up can — and asked of
//! every backend: the clouds answer "cannot say" without a round trip (the
//! trait's default), so there is nothing to gate.

use super::Orchestrator;
use crate::app::events::AppEvent;

/// What the engine has said about its slot count.
#[derive(Default)]
pub(super) struct SlotsDiscovery {
    /// Bumped by [`Self::invalidate`]; an answer for an older epoch is dropped.
    epoch: u64,
    /// A question is in flight — don't ask again.
    pending: bool,
    /// The count the engine reported, when it could.
    known: Option<u32>,
}

impl SlotsDiscovery {
    /// Forgets what was learned: the engine changed, or its readiness flipped.
    fn invalidate(&mut self) {
        self.epoch += 1;
        self.pending = false;
        self.known = None;
    }

    /// Applies an answer if it belongs to the current engine.
    fn apply(&mut self, epoch: u64, slots: Option<u32>) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.pending = false;
        self.known = slots;
        true
    }
}

impl Orchestrator {
    /// The engine changed, or its readiness flipped: drop what was learned, tell
    /// the UI the hint is gone, and ask again.
    pub(super) fn refresh_engine_slots(&mut self) {
        self.slots.invalidate();
        self.emit_engine_slots();
        // `backend` rather than a readiness gate, like the context budget: a
        // server still loading its weights answers `/props` perfectly well.
        let Some(backend) = self.engines.backend.clone() else {
            return;
        };
        self.slots.pending = true;
        let epoch = self.slots.epoch;
        let tx = self.slots_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send((epoch, backend.parallel_slots().await));
        });
    }

    /// Records what the engine answered, and tells the UI.
    pub(super) fn handle_slots_result(&mut self, epoch: u64, slots: Option<u32>) {
        if !self.slots.apply(epoch, slots) {
            return;
        }
        if let Some(n) = self.slots.known {
            tracing::info!(slots = n, "engine reported its slot count");
        }
        self.emit_engine_slots();
    }

    /// Sends the discovered count to the UI — the hint, never the setting.
    fn emit_engine_slots(&self) {
        let _ = self.evt_tx.send(AppEvent::EngineSlots(self.slots.known));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer from a replaced engine is dropped; the current one lands.
    #[test]
    fn a_stale_answer_is_dropped_and_a_current_one_lands() {
        let mut d = SlotsDiscovery::default();
        let old = d.epoch;
        d.pending = true;
        d.invalidate();
        assert!(!d.pending);
        assert!(!d.apply(old, Some(4)), "the previous engine's answer");
        assert_eq!(d.known, None);
        assert!(d.apply(d.epoch, Some(4)));
        assert_eq!(d.known, Some(4));
        // "Cannot say" is an answer too: it clears a stale count.
        assert!(d.apply(d.epoch, None));
        assert_eq!(d.known, None);
    }
}
