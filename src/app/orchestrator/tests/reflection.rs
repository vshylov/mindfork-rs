//! Orchestrator tests — auto-reflection: cadence/watermark, failure alerting. Part of the
//! [`super`] module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::super::background::BgOutcome;
use super::*;

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
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Failed("boom".into()));
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Failed("boom".into()));
    assert!(!saw_error(&mut rx));
    // The third in a row — one error.
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Failed("boom".into()));
    assert!(saw_error(&mut rx));
    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 3);
    // Success resets the streak and sends SelfModelChanged.
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Done);
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
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Failed("boom".into()));
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Failed("boom".into()));
    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 2);
    let token = tokio_util::sync::CancellationToken::new();
    orch.begin_bg(BackgroundKind::Reflection, token.clone());
    while rx.try_recv().is_ok() {}

    orch.handle_stop_background_task(BackgroundKind::Reflection);
    assert!(
        token.is_cancelled(),
        "the slot's token is what the stop cancels"
    );
    orch.handle_bg_done(BackgroundKind::Reflection, BgOutcome::Cancelled);

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
