//! Orchestrator tests — auto-reflection: cadence/watermark, failure alerting. Part of the
//! [`super`] module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::super::background::{Acted, Acting, BgOutcome, Refund, Window};
use super::*;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn auto_reflect_advances_watermark_on_spawn() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    // A ready engine — reflection actually spawns (an empty script → the task finishes).
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_reflect(chat_id);

    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    // The watermark shifted to the full length of the history (the window is covered), reflection is running.
    assert_eq!(chat.reflected_upto, Some(2));
    assert!(chat.reflected_at.is_some());
    assert!(orch.bg_running(BackgroundKind::Reflection));
}

#[tokio::test]
async fn auto_reflect_keeps_watermark_when_server_not_ready() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    // No engine set → backend_if_ready returns Err → a skip WITHOUT shifting the watermark.
    orch.maybe_auto_reflect(chat_id);

    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    assert_eq!(chat.reflected_upto, None); // the cycle isn't lost — retry later
    assert!(!orch.bg_running(BackgroundKind::Reflection));
}

#[tokio::test]
async fn reflect_failures_alert_once_then_reset() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let saw_error = |rx: &mut UnboundedReceiver<AppEvent>| {
        let mut seen = false;
        while let Ok(e) = rx.try_recv() {
            if matches!(e, AppEvent::Error(_)) {
                seen = true;
            }
        }
        seen
    };
    // Two consecutive failures — the UI is still quiet (observability without spam).
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Failed("boom".into()),
        None,
    );
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Failed("boom".into()),
        None,
    );
    assert!(!saw_error(&mut rx));
    // The third in a row — one error.
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Failed("boom".into()),
        None,
    );
    assert!(saw_error(&mut rx));
    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 3);
    // Success resets the streak and sends SelfModelChanged.
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Done, None);
    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 0);
    let mut changed = false;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, AppEvent::SelfModelChanged) {
            changed = true;
        }
    }
    assert!(changed);
}

/// A stopped task (docs/research/stop-silent-task.md §3.3) clears its slot
/// and its indicator, re-sends the task list, still announces
/// `SelfModelChanged` (a partial run may have written), and touches the
/// failure streak not at all — neither reset nor counted, no alert.
#[test]
fn a_stopped_task_touches_neither_the_streak_nor_the_success_path() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Failed("boom".into()),
        None,
    );
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Failed("boom".into()),
        None,
    );
    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 2);
    let token = tokio_util::sync::CancellationToken::new();
    orch.begin_bg(BackgroundKind::Reflection, token.clone(), None);
    while rx.try_recv().is_ok() {}

    orch.handle_stop_background_task(BackgroundKind::Reflection);
    assert!(
        token.is_cancelled(),
        "the slot's token is what the stop cancels"
    );
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Cancelled { consumed: false },
        None,
    );

    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 2, "untouched");
    assert!(!orch.bg_running(BackgroundKind::Reflection));
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    assert!(events.iter().any(|e| matches!(
        e,
        AppEvent::BackgroundTask {
            kind: BackgroundKind::Reflection,
            active: false
        }
    )));
    assert!(events.iter().any(|e| matches!(e, AppEvent::TaskList(_))));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AppEvent::SelfModelChanged))
    );
    assert!(
        !events.iter().any(|e| matches!(e, AppEvent::Error(_))),
        "a stop is not a failure: {events:?}"
    );
}

/// Stopping a kind with no task running does nothing — the screen may be a
/// snapshot behind.
#[test]
fn stopping_an_idle_kind_does_nothing() {
    let (_d, orch, mut rx) = bare_orch_rx();
    orch.handle_stop_background_task(BackgroundKind::Consolidation);
    assert!(!orch.bg_running(BackgroundKind::Consolidation));
    assert!(rx.try_recv().is_err(), "nothing to announce");
}

// ---------- a stop gives the window back (docs/research/stop-refunds-window.md) ----------

fn watermark(orch: &Orchestrator, chat_id: Uuid) -> (Option<usize>, bool) {
    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    (chat.reflected_upto, chat.reflected_at.is_some())
}

/// A window on a slot whose task has not acted yet (the flag the loop would
/// set is fresh).
fn refund(window: Window) -> Refund {
    Refund {
        window,
        acted: Arc::new(Acted::default()),
    }
}

/// A reflection stopped before a round of its tools ran gives its window
/// back (§3.3): the watermark and the stamp are what they were before the
/// spawn, the chat is saved the way the advance was, and the next landing
/// spawns it again over the same window.
#[tokio::test]
async fn a_reflection_stopped_before_its_first_round_gives_the_window_back() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);
    orch.maybe_auto_reflect(chat_id);
    assert_eq!(
        watermark(&orch, chat_id),
        (Some(2), true),
        "advanced at spawn"
    );
    orch.saves.take();

    orch.handle_stop_background_task(BackgroundKind::Reflection);
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Cancelled { consumed: false },
        None,
    );

    assert_eq!(
        watermark(&orch, chat_id),
        (None, false),
        "the window is unread again"
    );
    assert!(
        orch.saves.is_dirty(chat_id),
        "the refund is saved like the advance"
    );
    assert!(!orch.bg_running(BackgroundKind::Reflection));

    // The ordinary cadence brings it back: the same window is due.
    orch.maybe_auto_reflect(chat_id);
    assert!(orch.bg_running(BackgroundKind::Reflection));
    assert_eq!(watermark(&orch, chat_id), (Some(2), true));
}

/// Stopped after a round of its tools ran, the reflection keeps its advance:
/// the window was acted on and must not be read twice (R2).
#[tokio::test]
async fn a_reflection_stopped_after_a_round_keeps_its_advance() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);
    orch.maybe_auto_reflect(chat_id);
    orch.saves.take();
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Cancelled { consumed: true },
        None,
    );
    assert_eq!(watermark(&orch, chat_id), (Some(2), true), "kept");
    assert!(!orch.saves.is_dirty(chat_id), "nothing to save");
}

/// Every other outcome drops the window: a task that finished or failed
/// keeps its advance exactly as before the refund existed.
#[tokio::test]
async fn a_finished_or_failed_reflection_keeps_its_advance() {
    for outcome in [BgOutcome::Done, BgOutcome::Failed("boom".into())] {
        let (_d, mut orch, chat_id) = orch_ready_for_reflection();
        orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
            FinishReason::Stop,
        )])) as Arc<dyn EngineBackend>);
        orch.maybe_auto_reflect(chat_id);
        orch.handle_bg_done(BackgroundKind::Reflection, outcome.clone(), None);
        assert_eq!(watermark(&orch, chat_id), (Some(2), true), "{outcome:?}");
    }
}

/// A consolidation's window is its counter: a stop before the first round
/// adds the reset's count back to whatever the landings during the run
/// left, and the sum is what the counter would read had the spawn never
/// happened; a stop after a round, or any other outcome, leaves it.
#[test]
fn a_consolidation_stopped_before_its_first_round_gets_its_count_back() {
    let (_d, mut orch, _rx) = bare_orch_rx();
    let chat = Uuid::new_v4();
    for (outcome, expected) in [
        (BgOutcome::Cancelled { consumed: false }, 3 + 5),
        (BgOutcome::Cancelled { consumed: true }, 3),
        (BgOutcome::Done, 3),
        (BgOutcome::Failed("boom".into()), 3),
    ] {
        orch.consolidate_counts.insert(chat, 3);
        orch.begin_bg(
            BackgroundKind::Consolidation,
            CancellationToken::new(),
            Some(refund(Window::Counter { chat, count: 5 })),
        );
        orch.handle_bg_done(BackgroundKind::Consolidation, outcome.clone(), None);
        assert_eq!(
            orch.consolidate_counts.get(&chat),
            Some(&expected),
            "{outcome:?}"
        );
    }
}

/// A reflection whose chat is gone by the landing is left alone: nothing to
/// restore, nothing saved.
#[test]
fn a_refund_for_a_chat_that_is_gone_does_nothing() {
    let (_d, mut orch, _rx) = bare_orch_rx();
    let chat = Uuid::new_v4();
    orch.begin_bg(
        BackgroundKind::Reflection,
        CancellationToken::new(),
        Some(refund(Window::Reflection {
            chat,
            upto: None,
            at: None,
        })),
    );
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Cancelled { consumed: false },
        None,
    );
    assert!(!orch.saves.is_dirty(chat));
}

// ---------- a quit gives the window back (docs/research/quit-refunds-window.md) ----------

/// `quit_bg` is the `Quit` arm's call (§3.2): every slot's token is
/// cancelled, and a task that had not yet acted on its window gets it back
/// before the exit flush — which writes the pre-spawn values to disk.
#[tokio::test]
async fn a_quit_before_the_first_round_gives_the_window_back_and_flushes_it() {
    let (dir, mut orch, chat_id) = orch_ready_for_reflection();
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);
    orch.maybe_auto_reflect(chat_id);
    assert_eq!(watermark(&orch, chat_id), (Some(2), true));
    orch.flush_saves();
    assert_eq!(
        super::subagent::load(dir.path(), chat_id).reflected_upto,
        Some(2),
        "the advance was flushed before the quit"
    );

    orch.cancel_bg_all();
    orch.refund_unlanded();

    assert_eq!(watermark(&orch, chat_id), (None, false));
    assert!(orch.saves.is_dirty(chat_id));
    orch.flush_saves();
    let on_disk = super::subagent::load(dir.path(), chat_id);
    assert_eq!(
        on_disk.reflected_upto, None,
        "the next launch reads the window"
    );
    assert_eq!(on_disk.reflected_at, None);
}

/// A task whose flag says a round of its tools ran keeps its advance at a
/// quit (R1), and a slot with nothing to refund (the roll) is only cancelled.
#[test]
fn a_quit_keeps_an_acted_window_and_ignores_the_roll() {
    let (_d, mut orch, _rx) = bare_orch_rx();
    let chat = Uuid::new_v4();
    orch.consolidate_counts.insert(chat, 3);
    let token = CancellationToken::new();
    orch.begin_bg(
        BackgroundKind::Consolidation,
        token.clone(),
        Some(Refund {
            window: Window::Counter { chat, count: 5 },
            acted: Arc::new(Acted::at(Acting::Wrote)),
        }),
    );
    let roll = CancellationToken::new();
    orch.begin_bg(BackgroundKind::Compaction, roll.clone(), None);

    orch.cancel_bg_all();
    orch.refund_unlanded();

    assert!(
        token.is_cancelled() && roll.is_cancelled(),
        "every task ended"
    );
    assert_eq!(orch.consolidate_counts.get(&chat), Some(&3), "kept: acted");
}

/// A quit during a round of the task's tools keeps the window as well
/// (docs/research/acted-by-effect.md §3.2): the round's write, if any, has
/// not reported yet, and the conservative side is the rule's.
#[test]
fn a_quit_during_a_round_of_tools_keeps_the_window() {
    let (_d, mut orch, _rx) = bare_orch_rx();
    let chat = Uuid::new_v4();
    orch.consolidate_counts.insert(chat, 3);
    orch.begin_bg(
        BackgroundKind::Consolidation,
        CancellationToken::new(),
        Some(Refund {
            window: Window::Counter { chat, count: 5 },
            acted: Arc::new(Acted::at(Acting::InTools)),
        }),
    );
    orch.cancel_bg_all();
    orch.refund_unlanded();
    assert_eq!(
        orch.consolidate_counts.get(&chat),
        Some(&3),
        "kept: mid-tools"
    );
}
