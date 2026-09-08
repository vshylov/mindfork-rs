//! Orchestrator tests — self-model auto-consolidation (self-model "sleep", stage A1):
//! cadence/gates, outcome alerting. Part of the [`super`] module (fixtures in mod.rs).
//! See docs/history/self-model-consolidation.md (stage A1).

use super::*;

#[tokio::test]
async fn self_consolidation_spawns_when_threshold_reached() {
    let (_d, mut orch, chat_id) = orch_ready_for_self_consolidation();
    // A ready engine — "sleep" actually spawns (an empty script → the task finishes).
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_self_consolidate(chat_id);

    assert!(orch.bg_running(BackgroundKind::SelfConsolidation));
    // The cadence counter is reset on an actual spawn.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&0));
}

#[tokio::test]
async fn self_consolidation_keeps_counter_when_server_not_ready() {
    let (_d, mut orch, chat_id) = orch_ready_for_self_consolidation();
    // No engine set → backend_if_ready returns Err → a skip WITHOUT resetting the counter.
    orch.maybe_auto_self_consolidate(chat_id);
    assert!(!orch.bg_running(BackgroundKind::SelfConsolidation));
    // The cycle isn't lost: the counter is incremented but not reset.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&1));
}

#[tokio::test]
async fn self_consolidation_gated_when_feature_off() {
    let (_d, mut orch, chat_id) = orch_ready_for_self_consolidation();
    orch.config.self_model.auto_consolidate_every = 0; // disabled
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_self_consolidate(chat_id);

    assert!(!orch.bg_running(BackgroundKind::SelfConsolidation));
    // Exited before the increment — no counter.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), None);
}

#[tokio::test]
async fn self_consolidation_gated_when_nothing_to_consolidate() {
    use crate::features::tools::self_model::GET_SELF_MODEL_ID;
    // The profile enabled the self-model, but observations < 2 and summary isn't bloated → no spawn.
    let (_dir, mut orch) = bare_orch();
    orch.config.self_model.auto_consolidate_every = 1;
    let mut profile = Profile::new("P", "sys");
    profile.enabled_tools = vec![GET_SELF_MODEL_ID.into()];
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуй"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_self_consolidate(chat_id);

    assert!(!orch.bg_running(BackgroundKind::SelfConsolidation));
    // The counter is incremented (the threshold is reached), but not reset — the cycle isn't lost.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&1));
}

#[tokio::test]
async fn self_consolidation_success_emits_self_model_changed() {
    // A successful self-model "sleep" updates the open F3 screen (like reflection): SelfModelChanged.
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.handle_bg_done(
        BackgroundKind::SelfConsolidation,
        super::super::background::BgOutcome::Done,
    );
    let mut changed = false;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, AppEvent::SelfModelChanged) {
            changed = true;
        }
    }
    assert!(changed);
}

/// A "sleep" stopped before a round of its tools ran gets its count back
/// (docs/research/stop-refunds-window.md §3.3): the reset's count is added
/// to the landing that happened during the run, and the next landing spawns
/// it again.
#[tokio::test]
async fn a_sleep_stopped_before_its_first_round_gets_its_count_back() {
    let (_d, mut orch, chat_id) = orch_ready_for_self_consolidation();
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);
    orch.maybe_auto_self_consolidate(chat_id);
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&0));
    // A landing during the run counts, and does not spawn a second one.
    orch.maybe_auto_self_consolidate(chat_id);
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&1));

    orch.handle_stop_background_task(BackgroundKind::SelfConsolidation);
    orch.handle_bg_done(
        BackgroundKind::SelfConsolidation,
        super::super::background::BgOutcome::Cancelled { consumed: false },
    );
    assert_eq!(
        orch.self_consolidate_counts.get(&chat_id),
        Some(&2),
        "the reset's one added back to the landing's one"
    );

    // The ordinary cadence brings it back.
    orch.maybe_auto_self_consolidate(chat_id);
    assert!(orch.bg_running(BackgroundKind::SelfConsolidation));
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&0));
}
