//! Тесты оркестратора — авто-рефлексия: каденция/ватермарк, оповещение об ошибках. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/refactoring-god-objects.md, этап 3.

use super::*;

#[tokio::test]
async fn auto_reflect_advances_watermark_on_spawn() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    // Готовый движок — рефлексия реально спавнится (пустой скрипт → задача завершится).
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_reflect(chat_id);

    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    // Ватермарк сдвинут на всю длину истории (окно охвачено), рефлексия запущена.
    assert_eq!(chat.reflected_upto, Some(2));
    assert!(chat.reflected_at.is_some());
    assert!(orch.bg_running(BackgroundKind::Reflection));
}

#[tokio::test]
async fn auto_reflect_keeps_watermark_when_server_not_ready() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    // Движок не задан → backend_if_ready вернёт Err → пропуск БЕЗ сдвига ватермарка.
    orch.maybe_auto_reflect(chat_id);

    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    assert_eq!(chat.reflected_upto, None); // цикл не потерян — повторим позже
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
    // Две неудачи подряд — в UI ещё тихо (наблюдаемость без спама).
    orch.handle_bg_done(BackgroundKind::Reflection, Err("boom".into()));
    orch.handle_bg_done(BackgroundKind::Reflection, Err("boom".into()));
    assert!(!saw_error(&mut rx));
    // Третья подряд — одна ошибка.
    orch.handle_bg_done(BackgroundKind::Reflection, Err("boom".into()));
    assert!(saw_error(&mut rx));
    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 3);
    // Успех сбрасывает серию и шлёт SelfModelChanged.
    orch.handle_bg_done(BackgroundKind::Reflection, Ok(()));
    assert_eq!(orch.bg_failures(BackgroundKind::Reflection), 0);
    let mut changed = false;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, AppEvent::SelfModelChanged) {
            changed = true;
        }
    }
    assert!(changed);
}
