//! The KV pool the app's streams share, when the app can know it
//! (docs/research/admission-by-budget.md §4.4; the one-session rule —
//! docs/research/silent-tasks-budget.md §4.3). Read where the session budget
//! is built (`background_runs.rs`); the loops never ask the server.
//!
//! | mode | pool | why |
//! |---|---|---|
//! | managed, unless the server reported **one** slot | `context_budget` (`-c`, or the explicit override) | above one session the launcher wrote `-np N --kv-unified`; at one it wrote nothing, and a `llama-server` without `-np` runs four slots over one unified pool (its default since December 2025) — the silent lane's requests land on them beside the turn's stream |
//! | managed, one slot reported | none | an explicit `-np 1` in the extra arguments: the server queues |
//! | external, more than one slot reported | `context_budget` (`n_ctx` on `/props`) | the shape is unknowable from outside — assuming *shared* is never wrong, only pessimistic on a split server (fork F4) |
//! | external otherwise | none | no `/props` (vLLM, Ollama, LM Studio, gateways), or one slot — the server queues |
//! | the clouds | none | no pool |
//!
//! `sessions` no longer decides: with one interactive permit nothing in that
//! lane overlaps, so the pool only ever matters *between* the lanes — which
//! is exactly where it was missing.

use super::Orchestrator;
use crate::shared::config::ServerMode;

/// The rule, pure: the active section's mode, the slot count the server
/// reported (`None` — not yet, or it cannot say), and the window the
/// compaction trigger measures against — asked only when the rule needs it,
/// since the question can start a background fetch.
pub(super) fn pool_for(
    mode: ServerMode,
    reported_slots: Option<u32>,
    context_budget: impl FnOnce() -> Option<u64>,
) -> Option<u64> {
    match mode {
        // Not reported yet reads as the launcher's default: four unified
        // slots. Only a report of exactly one slot says the server queues.
        ServerMode::Managed if reported_slots == Some(1) => None,
        ServerMode::Managed => context_budget(),
        ServerMode::External => reported_slots
            .filter(|&n| n > 1)
            .and_then(|_| context_budget()),
        ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude | ServerMode::Grok => None,
    }
}

impl Orchestrator {
    /// The pool the app's reservations are measured against, or `None` when
    /// nothing can (or need) guard: see the module's table.
    pub(super) fn session_pool(&mut self) -> Option<u64> {
        let mode = self.config.engine.mode;
        let slots = self.slots.known();
        pool_for(mode, slots, || self.context_budget())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Managed: the window the launcher wrote (or the override) — with four
    /// slots reported, with none reported yet (the launcher's default is
    /// four), and never when the server reported exactly one (an explicit
    /// `-np 1`), which is the only shape where nothing shares.
    #[test]
    fn managed_shares_the_context_budget_unless_one_slot_is_reported() {
        assert_eq!(
            pool_for(ServerMode::Managed, Some(4), || Some(16384)),
            Some(16384)
        );
        assert_eq!(
            pool_for(ServerMode::Managed, None, || Some(8192)),
            Some(8192)
        );
        assert_eq!(
            pool_for(ServerMode::Managed, Some(1), || unreachable!("not asked")),
            None
        );
        assert_eq!(pool_for(ServerMode::Managed, None, || None), None);
    }

    /// External: only a server that reported more than one slot is assumed
    /// to share a pool; no `/props` or one slot is no guard — and the budget
    /// is not even asked for then (the closure would start a fetch).
    #[test]
    fn external_needs_more_than_one_reported_slot() {
        assert_eq!(
            pool_for(ServerMode::External, Some(4), || Some(4096)),
            Some(4096)
        );
        assert_eq!(
            pool_for(ServerMode::External, Some(1), || unreachable!("not asked")),
            None
        );
        assert_eq!(
            pool_for(ServerMode::External, None, || unreachable!("not asked")),
            None
        );
    }

    /// The clouds have no pool, whatever the server would say.
    #[test]
    fn the_clouds_have_no_pool() {
        for mode in [
            ServerMode::OpenAi,
            ServerMode::Gemini,
            ServerMode::Claude,
            ServerMode::Grok,
        ] {
            assert_eq!(
                pool_for(mode, Some(4), || unreachable!("not asked")),
                None,
                "{mode:?}"
            );
        }
    }
}
