//! Тесты оркестратора — авто-консолидация «модели себя» («сон» модели себя, этап A1):
//! каденция/гейты, оповещение об исходе. Часть модуля [`super`] (фикстуры в mod.rs).
//! См. docs/history/self-model-consolidation.md (этап A1).

use super::*;

#[tokio::test]
async fn self_consolidation_spawns_when_threshold_reached() {
    let (_d, mut orch, chat_id) = orch_ready_for_self_consolidation();
    // Готовый движок — «сон» реально спавнится (пустой скрипт → задача завершится).
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_self_consolidate(chat_id);

    assert!(orch.bg_running(BackgroundKind::SelfConsolidation));
    // Счётчик каденции сброшен при фактическом спавне.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&0));
}

#[tokio::test]
async fn self_consolidation_keeps_counter_when_server_not_ready() {
    let (_d, mut orch, chat_id) = orch_ready_for_self_consolidation();
    // Движок не задан → backend_if_ready вернёт Err → пропуск БЕЗ сброса счётчика.
    orch.maybe_auto_self_consolidate(chat_id);
    assert!(!orch.bg_running(BackgroundKind::SelfConsolidation));
    // Цикл не потерян: счётчик инкрементирован, но не сброшен.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&1));
}

#[tokio::test]
async fn self_consolidation_gated_when_feature_off() {
    let (_d, mut orch, chat_id) = orch_ready_for_self_consolidation();
    orch.config.self_model.auto_consolidate_every = 0; // выключено
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_self_consolidate(chat_id);

    assert!(!orch.bg_running(BackgroundKind::SelfConsolidation));
    // Вышли до инкремента — счётчика нет.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), None);
}

#[tokio::test]
async fn self_consolidation_gated_when_nothing_to_consolidate() {
    use crate::features::tools::self_model::GET_SELF_MODEL_ID;
    // Профиль включил модель себя, но наблюдений < 2 и summary не раздут → нет спавна.
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
    // Счётчик инкрементирован (порог достигнут), но не сброшен — цикл не потерян.
    assert_eq!(orch.self_consolidate_counts.get(&chat_id), Some(&1));
}

#[tokio::test]
async fn self_consolidation_success_emits_self_model_changed() {
    // Успех «сна» модели себя обновляет открытый экран F3 (как рефлексия): SelfModelChanged.
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.handle_bg_done(BackgroundKind::SelfConsolidation, Ok(()));
    let mut changed = false;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, AppEvent::SelfModelChanged) {
            changed = true;
        }
    }
    assert!(changed);
}
