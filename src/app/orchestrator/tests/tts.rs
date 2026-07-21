//! Тесты озвучивания (`/tts`, spec §11.9): понятные отказы при ненастроенном
//! движке и точки остановки воспроизведения (по настройке и безусловные).

use super::*;
use crate::features::tts_command::TtsScope;
use crate::shared::config::TtsMode;

/// Ставит «идёт озвучивание» без реальной задачи: токен + поколение — ровно то
/// состояние, которое проверяют точки остановки.
fn mark_speaking(orch: &mut Orchestrator) -> tokio_util::sync::CancellationToken {
    let token = tokio_util::sync::CancellationToken::new();
    orch.tts_cancel = Some(token.clone());
    orch.tts_gen = Some(Uuid::new_v4());
    token
}

#[tokio::test]
async fn tts_without_api_key_reports_setup_error() {
    // Дефолтный режим — облако OpenAI с осмысленной моделью, но без ключа: команда
    // должна ответить понятной подсказкой, а не молчать.
    let backend: Arc<dyn EngineBackend> = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("ответ ассистента".into()),
        ChatChunk::Finished(crate::shared::api::FinishReason::Stop),
    ]));
    let (_dir, tx, mut rx, handle) = spawn_orch(Some(backend));
    wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;

    tx.send(AppCommand::SendMessage("привет".into())).unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. })).await;
    tx.send(AppCommand::Tts(TtsScope::Last)).unwrap();

    let err = wait_for(&mut rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .expect("ожидали сообщение об ошибке");
    let AppEvent::Error(msg) = err else {
        unreachable!()
    };
    let expected = crate::shared::i18n::locale(crate::shared::i18n::Lang::default())
        .t("ui.err.tts_no_api_key");
    assert_eq!(msg, expected, "ключа нет → подсказка открыть настройки");
    // Чип озвучивания не зажигался (задача не стартовала).
    tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
}

#[tokio::test]
async fn tts_in_empty_chat_reports_nothing_to_speak() {
    let (_dir, tx, mut rx, handle) = spawn_orch(None);
    wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    tx.send(AppCommand::Tts(TtsScope::All)).unwrap();

    let err = wait_for(&mut rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .expect("ожидали сообщение об ошибке");
    let AppEvent::Error(msg) = err else {
        unreachable!()
    };
    assert_eq!(
        msg,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::default())
            .t("ui.err.tts_nothing_to_speak")
    );
    tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
}

#[test]
fn chat_switch_stops_speaking_only_when_enabled() {
    // Настройка «прерывать при переключении чата» включена по умолчанию.
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    orch.bootstrap().unwrap();
    let first = orch.chats[0].id;
    orch.handle_new_chat(None);
    let second = orch.active_id.unwrap();
    assert_ne!(first, second);
    while rx.try_recv().is_ok() {}

    let token = mark_speaking(&mut orch);
    orch.handle_switch(first);
    assert!(token.is_cancelled(), "переключение чата прерывает озвучку");
    assert!(orch.tts_gen.is_none());
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, AppEvent::TtsActive(false))),
        "чип озвучки гаснет сразу"
    );

    // Выключенная настройка — озвучка переживает переключение.
    orch.config.tts.stop_on_chat_switch = false;
    let token = mark_speaking(&mut orch);
    orch.handle_switch(second);
    assert!(
        !token.is_cancelled(),
        "с выключенной настройкой не прерываем"
    );
    assert!(orch.tts_gen.is_some());
}

#[test]
fn deleting_active_chat_stops_speaking_unconditionally() {
    // Удаление чата — безусловная остановка (озвучиваемого текста больше нет),
    // настройки на неё не влияют.
    let (_dir, mut orch, _rx) = bare_orch_rx();
    orch.bootstrap().unwrap();
    orch.handle_new_chat(None);
    let active = orch.active_id.unwrap();
    orch.config.tts.stop_on_chat_switch = false;

    let token = mark_speaking(&mut orch);
    orch.handle_delete(active);
    assert!(token.is_cancelled(), "удаление чата прерывает озвучку");
    assert!(orch.tts_gen.is_none());
}

#[test]
fn stop_command_is_idempotent() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    // Ничего не играет — команда не эмитит лишних событий.
    orch.stop_tts();
    assert!(rx.try_recv().is_err(), "no-op не шлёт событий");

    let token = mark_speaking(&mut orch);
    orch.stop_tts();
    assert!(token.is_cancelled());
    assert!(matches!(rx.try_recv(), Ok(AppEvent::TtsActive(false))));
    orch.stop_tts();
    assert!(rx.try_recv().is_err(), "повторная остановка — тоже no-op");
}

#[test]
fn late_done_of_cancelled_task_does_not_hide_new_indicator() {
    // Гонка «остановили старую → запустили новую»: поздний `done` устаревшей задачи
    // не должен гасить чип текущей.
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let _ = mark_speaking(&mut orch);
    let stale = orch.tts_gen.unwrap();
    orch.stop_tts();
    let _ = mark_speaking(&mut orch);
    while rx.try_recv().is_ok() {}

    orch.handle_tts_done(stale);
    assert!(orch.tts_gen.is_some(), "текущая озвучка не тронута");
    assert!(rx.try_recv().is_err(), "события гашения чипа нет");

    let current = orch.tts_gen.unwrap();
    orch.handle_tts_done(current);
    assert!(orch.tts_gen.is_none());
    assert!(matches!(rx.try_recv(), Ok(AppEvent::TtsActive(false))));
}

#[test]
fn external_mode_without_url_reports_setup_error() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    orch.bootstrap().unwrap();
    // Кладём сообщение прямо в чат: гейт готовности сервера здесь не при чём —
    // проверяем именно отказ настройки озвучивания.
    let active = orch.active_id.unwrap();
    orch.chat_mut(active)
        .unwrap()
        .push_message(Message::assistant("ответ ассистента"));
    while rx.try_recv().is_ok() {}
    orch.config.tts.mode = TtsMode::External;

    orch.handle_tts(TtsScope::Last);
    let msg = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|e| match e {
        AppEvent::Error(m) => Some(m),
        _ => None,
    });
    assert_eq!(
        msg.as_deref(),
        Some(
            crate::shared::i18n::locale(crate::shared::i18n::Lang::default())
                .t("ui.err.tts_no_url")
        ),
        "external без URL — понятная подсказка"
    );
}
