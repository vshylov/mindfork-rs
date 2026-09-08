//! A slot registry for "silent" background tasks (self-model auto-reflection and
//! notes auto-consolidation). Both tasks are a mini agentic loop with no UI (the shared runner
//! [`tool_loop::spawn_silent_loop`](super::tool_loop)); their lifecycle (the "running"
//! flag, a failure streak, clearing the indicator, an error alert) used to be
//! duplicated as fields and handlers per task. Here it's one — the registry key
//! is the existing [`BackgroundKind`]. Adding task #3 (self-model auto-consolidation,
//! roadmap — architecture.md §9.9) doesn't touch the `run()`/`Quit` scaffolding.
//! See docs/history/refactoring-solid.md §4.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppEvent, BackgroundKind};
use crate::shared::i18n::Locale;

use super::Orchestrator;

/// How a silent task ended (`bg_done_tx`): its work done, stopped by its
/// own token — the tasks screen's `F6`, or `Quit` — or failed with a
/// reason worded for the user. A stop is neither a success nor a failure
/// to the streak (docs/research/stop-silent-task.md §3.3); `consumed` says
/// whether a round of the task's tools had run by then, which decides
/// whether the window it advanced at spawn is given back
/// (docs/research/stop-refunds-window.md §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BgOutcome {
    Done,
    Cancelled { consumed: bool },
    Failed(String),
}

/// What a spawn advanced, and how to put it back: reflection's watermark
/// and stamp before the spawn, or the reply count a consolidation's reset
/// took. Kept on the task's slot from the spawn to the landing; given back
/// only when the task was stopped before a round of its tools ran, so the
/// ordinary cadence makes it due again at the next landing — a window the
/// task acted on must not be read twice
/// (docs/research/stop-refunds-window.md §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Window {
    /// Reflection: the chat's `reflected_upto` and `reflected_at` before the spawn.
    Reflection {
        chat: Uuid,
        upto: Option<usize>,
        at: Option<DateTime<Utc>>,
    },
    /// A consolidation: the chat's reply count the spawn reset to zero.
    Counter { chat: Uuid, count: u32 },
}

/// What a stop or a quit can give back, and whether it still may: the
/// [`Window`] the spawn advanced, and the flag the loop sets at the very
/// line it counts a round — "a round of the task's tools is about to run"
/// — so the fact is readable outside the loop at any moment, a quit's
/// included, and not only from the outcome the loop sends last
/// (docs/research/quit-refunds-window.md §3.1).
pub(super) struct Refund {
    pub window: Window,
    pub acted: Arc<AtomicBool>,
}

/// A silent background task's slot: the active-run token + a failure streak. The streak
/// outlives a single run (it survives completions) — hence a slot, not a separate task.
#[derive(Default)]
pub(super) struct BgSlot {
    /// `Some` — the task is running (one at a time); the cancellation token (for the `Quit` branch).
    cancel: Option<CancellationToken>,
    /// The count of consecutive failures; at the [`BACKGROUND_FAILURE_ALERT`](super::BACKGROUND_FAILURE_ALERT)
    /// threshold we show a UI error once, then stay quiet until the first success.
    failures: u32,
    /// What the running task's spawn advanced, with the loop's flag
    /// ([`Refund`]); taken at the landing, given back on a stop — or a quit
    /// — before the first round of tools.
    refund: Option<Refund>,
}

impl Orchestrator {
    /// Is a background task of this kind running (the "one at a time" gate).
    pub(super) fn bg_running(&self, kind: BackgroundKind) -> bool {
        self.bg.get(&kind).is_some_and(|s| s.cancel.is_some())
    }

    /// Records a run: the slot is marked active (`cancel = Some`), what the
    /// spawn advanced is kept for a stop or a quit to give back (`refund`;
    /// `None` for the roll, which has nothing to refund), and a quiet
    /// "running …" indicator goes into the status bar. Called by the spawn
    /// tails of `maybe_auto_reflect`/`maybe_auto_consolidate`/`spawn_compact`.
    pub(super) fn begin_bg(
        &mut self,
        kind: BackgroundKind,
        cancel: CancellationToken,
        refund: Option<Refund>,
    ) {
        let slot = self.bg.entry(kind).or_default();
        slot.cancel = Some(cancel);
        slot.refund = refund;
        let _ = self
            .evt_tx
            .send(AppEvent::BackgroundTask { kind, active: true });
        // The tasks screen's "the app's own work" rows (spec §11.10).
        self.emit_task_list();
    }

    /// The shared background-task outcome handler (formerly `handle_reflect_done`/
    /// `handle_consolidate_done`): clears the "running" flag, clears the indicator, tracks the
    /// failure streak (at the threshold — one UI error, observability without spam). On success of
    /// **reflection** or **self-model consolidation**, additionally sends
    /// `SelfModelChanged` (an open `F3` screen re-requests a fresh snapshot);
    /// *notes* consolidation doesn't (it changes notes, not the "self-model"). The task's
    /// tools have already written the changes into `Storage`; this doesn't touch the chat/feed.
    /// A task **stopped** by its own token (docs/research/stop-silent-task.md
    /// §3.3) clears the slot like the others and touches the streak not at
    /// all — neither reset nor counted — and still announces
    /// `SelfModelChanged` for the two self-model kinds, since a partial run
    /// may have written before the stop. Stopped **before a round of its
    /// tools ran**, it also gets the window its spawn advanced back
    /// ([`Window`]; docs/research/stop-refunds-window.md §3.3); on every
    /// other outcome the window is dropped and the advance stands.
    pub(super) fn handle_bg_done(&mut self, kind: BackgroundKind, outcome: BgOutcome) {
        // Mutate the slot and compute whether an error alert is needed BEFORE sending events
        // (the borrow of `self.bg` doesn't overlap `self.evt_tx` in the send below).
        let (alert, window) = {
            let slot = self.bg.entry(kind).or_default();
            slot.cancel = None;
            let window = slot.refund.take().map(|r| r.window);
            let alert = match &outcome {
                BgOutcome::Done => {
                    slot.failures = 0;
                    None
                }
                BgOutcome::Cancelled { .. } => None,
                BgOutcome::Failed(reason) => {
                    slot.failures += 1;
                    (slot.failures == super::BACKGROUND_FAILURE_ALERT).then(|| reason.clone())
                }
            };
            (alert, window)
        };
        if let (Some(window), BgOutcome::Cancelled { consumed: false }) = (window, &outcome) {
            self.give_back(kind, window);
        }
        let _ = self.evt_tx.send(AppEvent::BackgroundTask {
            kind,
            active: false,
        });
        self.emit_task_list();
        if !matches!(outcome, BgOutcome::Failed(_))
            && matches!(
                kind,
                BackgroundKind::Reflection | BackgroundKind::SelfConsolidation
            )
        {
            let _ = self.evt_tx.send(AppEvent::SelfModelChanged);
        }
        if let Some(reason) = alert {
            let loc = self.ui_locale();
            let _ = self.evt_tx.send(AppEvent::Error(loc.tf(
                "ui.err.bg_failed",
                &[("label", kind_label(loc, kind)), ("reason", &reason)],
            )));
        }
    }

    /// Puts back what a spawn advanced (docs/research/stop-refunds-window.md
    /// §3.3): the task was stopped before a round of its tools ran, so the
    /// window it was about to read is unread, and the ordinary cadence makes
    /// it due again at the next landing. Reflection's watermark and stamp
    /// are restored and the chat saved the way the advance was; a chat gone
    /// meanwhile is left alone. A counter is **added back**, since the
    /// landings during the run incremented it legitimately and the sum is
    /// what it would read had the spawn never happened.
    fn give_back(&mut self, kind: BackgroundKind, window: Window) {
        match window {
            Window::Reflection { chat, upto, at } => {
                let found = self.chats.iter_mut().find(|c| c.id == chat).map(|c| {
                    c.reflected_upto = upto;
                    c.reflected_at = at;
                });
                if found.is_some() {
                    self.mark_dirty(chat);
                }
            }
            Window::Counter { chat, count } => {
                let counts = match kind {
                    BackgroundKind::Consolidation => &mut self.consolidate_counts,
                    BackgroundKind::SelfConsolidation => &mut self.self_consolidate_counts,
                    // Neither keeps a counter; a spawn never records one for them.
                    BackgroundKind::Reflection | BackgroundKind::Compaction => return,
                };
                *counts.entry(chat).or_insert(0) += count;
            }
        }
    }

    /// Stops one running task of `kind` (`AppCommand::StopBackgroundTask`:
    /// the tasks screen's `F6` on its row, docs/research/stop-silent-task.md
    /// §3.2): its token is cancelled and it lands as `Cancelled` on its own
    /// path, at once — a wait returns, a stream ends on its next chunk. A
    /// kind with no task running is ignored: the screen may be a snapshot
    /// behind.
    pub(super) fn handle_stop_background_task(&self, kind: BackgroundKind) {
        if let Some(token) = self.bg.get(&kind).and_then(|s| s.cancel.as_ref()) {
            token.cancel();
        }
    }

    /// The `Quit` branch: cancels every running task in the family and gives
    /// back the window of each one that had not yet acted on it — the same
    /// rule a stop applies at the landing, read off the slot's flag since a
    /// quit has no landing (docs/research/quit-refunds-window.md §3.2). The
    /// token is cancelled **before** the flag is read: a loop that stores
    /// its flag after this read checks the token before its tools and
    /// starts none, so a refunded window is never written into (§3.3). The
    /// exit flush after the loop writes what `give_back` marked dirty.
    pub(super) fn quit_bg(&mut self) {
        let refunds: Vec<(BackgroundKind, Window)> = self
            .bg
            .iter_mut()
            .filter_map(|(kind, slot)| {
                if let Some(token) = &slot.cancel {
                    token.cancel();
                }
                let refund = slot.refund.take()?;
                (!refund.acted.load(Ordering::SeqCst)).then_some((*kind, refund.window))
            })
            .collect();
        for (kind, window) in refunds {
            self.give_back(kind, window);
        }
    }

    /// The task's consecutive-failure count (for error-alert tests).
    #[cfg(test)]
    pub(super) fn bg_failures(&self, kind: BackgroundKind) -> u32 {
        self.bg.get(&kind).map_or(0, |s| s.failures)
    }
}

/// The silent lane's label for a task kind (`SessionBudget::acquire_silent`,
/// docs/research/silent-tasks-budget.md §4.6): what the budget reports as
/// streaming, and what the tasks screen's snapshot compares against to say
/// which running task is *waiting*. Stable identifiers, never shown.
pub(super) fn lane_label(kind: BackgroundKind) -> &'static str {
    match kind {
        BackgroundKind::Reflection => "reflection",
        BackgroundKind::Consolidation => "consolidation",
        BackgroundKind::SelfConsolidation => "self_consolidation",
        BackgroundKind::Compaction => "compaction",
    }
}

/// A human-readable label for the task kind (the interface language, axis B) — error
/// texts are assembled from it (**byte-for-byte** with the previous Russian wording).
fn kind_label(loc: &'static Locale, kind: BackgroundKind) -> &'static str {
    loc.t(match kind {
        BackgroundKind::Reflection => "ui.err.bg_reflection",
        BackgroundKind::Consolidation => "ui.err.bg_consolidation",
        BackgroundKind::SelfConsolidation => "ui.err.bg_self_consolidation",
        BackgroundKind::Compaction => "ui.err.bg_compaction",
    })
}
