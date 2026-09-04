//! The KV pool a turn's streams share, when the app can know it
//! (docs/research/admission-by-budget.md §4.4). Read once per turn where the
//! session budget is built (`generation.rs`); the loop never asks the server.
//!
//! | mode | pool | why |
//! |---|---|---|
//! | managed, `sessions > 1` | `context_budget` (`-c`, or the explicit override) | the launcher wrote `-np N --kv-unified`: unified by construction |
//! | managed, `sessions = 1` | none | one permit; no two streams ever overlap |
//! | external, more than one slot reported | `context_budget` (`n_ctx` on `/props`) | the shape is unknowable from outside — assuming *shared* is never wrong, only pessimistic on a split server (fork F4) |
//! | external otherwise | none | no `/props` (vLLM, Ollama, LM Studio, gateways), or one slot — the server queues |
//! | the clouds | none | no pool |

use super::Orchestrator;
use crate::shared::config::ServerMode;

/// The rule, pure: `sessions` of the active section, its mode, the slot
/// count the server reported (external), and the window the compaction
/// trigger measures against — asked only when the rule needs it, since the
/// question can start a background fetch.
pub(super) fn pool_for(
    mode: ServerMode,
    sessions: u32,
    reported_slots: Option<u32>,
    context_budget: impl FnOnce() -> Option<u64>,
) -> Option<u64> {
    if sessions <= 1 {
        return None;
    }
    match mode {
        ServerMode::Managed => context_budget(),
        ServerMode::External => reported_slots
            .filter(|&n| n > 1)
            .and_then(|_| context_budget()),
        ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude | ServerMode::Grok => None,
    }
}

impl Orchestrator {
    /// The pool this turn's reservations are measured against, or `None` when
    /// nothing can (or need) guard: see the module's table.
    pub(super) fn session_pool(&mut self) -> Option<u64> {
        let mode = self.config.engine.mode;
        let sessions = self.config.engine.active_sessions();
        let slots = self.slots.known();
        pool_for(mode, sessions, slots, || self.context_budget())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One session: no pool in any mode — nothing can overlap.
    #[test]
    fn one_session_has_no_pool() {
        for mode in [
            ServerMode::Managed,
            ServerMode::External,
            ServerMode::Claude,
        ] {
            assert_eq!(pool_for(mode, 1, Some(4), || Some(8192)), None, "{mode:?}");
        }
    }

    /// Managed above one: the window the launcher wrote (or the override),
    /// whatever the server has or has not reported about its slots.
    #[test]
    fn managed_above_one_uses_the_context_budget() {
        assert_eq!(
            pool_for(ServerMode::Managed, 2, None, || Some(8192)),
            Some(8192)
        );
        assert_eq!(
            pool_for(ServerMode::Managed, 4, Some(4), || Some(16384)),
            Some(16384)
        );
        assert_eq!(pool_for(ServerMode::Managed, 2, None, || None), None);
    }

    /// External: only a server that reported more than one slot is assumed
    /// to share a pool; no `/props` or one slot is no guard — and the budget
    /// is not even asked for then (the closure would start a fetch).
    #[test]
    fn external_needs_more_than_one_reported_slot() {
        assert_eq!(
            pool_for(ServerMode::External, 2, Some(4), || Some(4096)),
            Some(4096)
        );
        assert_eq!(
            pool_for(ServerMode::External, 2, Some(1), || unreachable!(
                "not asked"
            )),
            None
        );
        assert_eq!(
            pool_for(ServerMode::External, 2, None, || unreachable!("not asked")),
            None
        );
    }

    /// The clouds have no pool, whatever the count.
    #[test]
    fn the_clouds_have_no_pool() {
        for mode in [
            ServerMode::OpenAi,
            ServerMode::Gemini,
            ServerMode::Claude,
            ServerMode::Grok,
        ] {
            assert_eq!(
                pool_for(mode, 4, Some(4), || unreachable!("not asked")),
                None,
                "{mode:?}"
            );
        }
    }
}
