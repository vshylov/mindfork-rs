//! Speech tests (`/tts`, spec §11.9): clear refusals with an unconfigured
//! engine, and playback stop points (settings-driven and unconditional).

use super::*;
use crate::features::tts_command::TtsScope;
use crate::shared::config::TtsMode;

/// Sets "speech is in progress" without a real task: a token + a generation id — exactly the
/// state the stop points check.
fn mark_speaking(orch: &mut Orchestrator) -> tokio_util::sync::CancellationToken {
    let token = tokio_util::sync::CancellationToken::new();
    orch.tts_cancel = Some(token.clone());
    orch.tts_gen = Some(Uuid::new_v4());
    token
}

#[tokio::test]
async fn tts_without_api_key_reports_setup_error() {
    // The default mode is the OpenAI cloud with a sensible model but no key: the command
    // should respond with a clear hint, not stay silent.
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
        .expect("expected an error message");
    let AppEvent::Error(msg) = err else {
        unreachable!()
    };
    let expected = crate::shared::i18n::locale(crate::shared::i18n::Lang::default())
        .t("ui.err.tts_no_api_key");
    assert_eq!(msg, expected, "no key → a hint to open settings");
    // The speech chip never lit up (the task never started).
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
        .expect("expected an error message");
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
    // The "stop on chat switch" setting is on by default.
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    orch.bootstrap().unwrap();
    let first = orch.chats[0].id;
    orch.handle_new_chat(None);
    let second = orch.active_id.unwrap();
    assert_ne!(first, second);
    while rx.try_recv().is_ok() {}

    let token = mark_speaking(&mut orch);
    orch.handle_switch(first);
    assert!(token.is_cancelled(), "switching chats stops speech");
    assert!(orch.tts_gen.is_none());
    assert!(
        std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, AppEvent::TtsActive(false))),
        "the speech chip goes dark right away"
    );

    // With the setting disabled — speech survives the switch.
    orch.config.tts.stop_on_chat_switch = false;
    let token = mark_speaking(&mut orch);
    orch.handle_switch(second);
    assert!(
        !token.is_cancelled(),
        "with the setting disabled we don't stop it"
    );
    assert!(orch.tts_gen.is_some());
}

#[test]
fn deleting_active_chat_stops_speaking_unconditionally() {
    // Deleting a chat is an unconditional stop (the spoken text no longer exists),
    // settings don't affect it.
    let (_dir, mut orch, _rx) = bare_orch_rx();
    orch.bootstrap().unwrap();
    orch.handle_new_chat(None);
    let active = orch.active_id.unwrap();
    orch.config.tts.stop_on_chat_switch = false;

    let token = mark_speaking(&mut orch);
    orch.handle_delete(active);
    assert!(token.is_cancelled(), "deleting the chat stops speech");
    assert!(orch.tts_gen.is_none());
}

#[test]
fn stop_command_is_idempotent() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    // Nothing is playing — the command emits no extra events.
    orch.stop_tts();
    assert!(rx.try_recv().is_err(), "a no-op sends no events");

    let token = mark_speaking(&mut orch);
    orch.stop_tts();
    assert!(token.is_cancelled());
    assert!(matches!(rx.try_recv(), Ok(AppEvent::TtsActive(false))));
    orch.stop_tts();
    assert!(rx.try_recv().is_err(), "a repeat stop is also a no-op");
}

#[test]
fn pause_resume_without_active_playback_are_safe_noops() {
    // There's no audio in CI, so `tts_playback` never comes up — check that the
    // pause/resume commands don't panic and don't send events when nothing is speaking.
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    orch.handle_tts_pause();
    orch.handle_tts_resume();
    assert!(orch.tts_playback.is_none());
    assert!(rx.try_recv().is_err(), "a no-op sends no events");

    // Mark "in progress" with no real device: pause/resume are still safe
    // (no handle), and `stop_tts` clears the pause and douses the chip.
    let token = mark_speaking(&mut orch);
    orch.handle_tts_pause();
    orch.handle_tts_resume();
    assert!(!token.is_cancelled(), "pause/resume don't cancel the task");
    orch.stop_tts();
    assert!(token.is_cancelled());
    assert!(matches!(rx.try_recv(), Ok(AppEvent::TtsActive(false))));
}

#[test]
fn late_done_of_cancelled_task_does_not_hide_new_indicator() {
    // The "stopped the old one → started a new one" race: a late `done` from the stale task
    // must not douse the current one's chip.
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let _ = mark_speaking(&mut orch);
    let stale = orch.tts_gen.unwrap();
    orch.stop_tts();
    let _ = mark_speaking(&mut orch);
    while rx.try_recv().is_ok() {}

    orch.handle_tts_done(stale);
    assert!(orch.tts_gen.is_some(), "the current speech is untouched");
    assert!(rx.try_recv().is_err(), "no chip-douse event");

    let current = orch.tts_gen.unwrap();
    orch.handle_tts_done(current);
    assert!(orch.tts_gen.is_none());
    assert!(matches!(rx.try_recv(), Ok(AppEvent::TtsActive(false))));
}

#[test]
fn external_mode_without_url_reports_setup_error() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    orch.bootstrap().unwrap();
    // Put the message directly into the chat: the server-readiness gate is irrelevant here —
    // what's checked is specifically the speech-configuration refusal.
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
        "external without a URL — a clear hint"
    );
}
