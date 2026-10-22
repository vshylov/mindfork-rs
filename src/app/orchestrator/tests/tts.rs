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

/// The gateway's mode has no default model — a default that names a model
/// ages — so a slot switched to it and left as it is says what is missing; and
/// with a model named, that the provider's key is.
#[test]
fn the_gateways_mode_says_what_it_cannot_speak_without() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    orch.bootstrap().unwrap();
    let active = orch.active_id.unwrap();
    orch.chat_mut(active)
        .unwrap()
        .push_message(Message::assistant("the assistant's answer"));
    orch.config.tts.mode = TtsMode::OpenRouter;
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
    let mut said = |orch: &mut Orchestrator| {
        while rx.try_recv().is_ok() {}
        orch.handle_tts(TtsScope::Last);
        std::iter::from_fn(|| rx.try_recv().ok()).find_map(|e| match e {
            AppEvent::Error(m) => Some(m),
            _ => None,
        })
    };
    assert_eq!(
        said(&mut orch).as_deref(),
        Some(loc.t("ui.err.tts_no_model"))
    );
    orch.config.tts.openrouter.model_name = Some("x-ai/grok-voice-tts-1.0".into());
    assert_eq!(
        said(&mut orch).as_deref(),
        Some(loc.t("ui.err.tts_no_api_key"))
    );
}

/// Every `/tts` builds its clients anew, and builds them on what the session
/// holds: the provider's switch as it is when the command is typed, and one
/// memo of formats — what a refused request taught one command, the next one
/// knows.
#[test]
fn every_command_speaks_on_the_sessions_memo_and_the_switch_as_it_stands() {
    use crate::shared::tts::GatewaySpeech;
    let (_dir, mut orch) = bare_orch();
    let GatewaySpeech {
        attribution,
        formats,
    } = orch.speech_gateway();
    assert!(attribution, "on until somebody turns it off");
    orch.config.openrouter.attribution = false;
    let next = orch.speech_gateway();
    assert!(!next.attribution);
    assert!(
        std::sync::Arc::ptr_eq(&formats, &next.formats),
        "one memo for the session"
    );
}

/// A bare orchestrator whose open chat ends with a reply, its profile speaking
/// replies or not, and speech in a mode with no URL — a setup error that is the
/// same on every machine, so the attempt shows without a sound device.
fn with_landed_reply(
    speak: bool,
) -> (
    tempfile::TempDir,
    Orchestrator,
    UnboundedReceiver<AppEvent>,
    Uuid,
    Uuid,
) {
    let (dir, mut orch, rx) = bare_orch_rx();
    orch.config.tts.mode = TtsMode::External;
    let mut profile = Profile::new("P", "s");
    profile.companion.speak_replies = speak;
    let mut chat = Chat::from_profile(&profile, "c");
    chat.push_message(Message::user("hi"));
    let reply = Message::assistant("hello there");
    let reply_id = reply.id;
    chat.push_message(reply);
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, rx, chat_id, reply_id)
}

fn errors(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let AppEvent::Error(m) = e {
            out.push(m);
        }
    }
    out
}

/// A profile that speaks its replies tries to speak one that lands; when speech
/// cannot be set up it says so once — every reply would repeat it — while a
/// `/tts` the person typed still answers.
#[test]
fn a_landed_reply_is_spoken_and_a_setup_failure_said_once() {
    let (_d, mut orch, mut rx, chat, reply) = with_landed_reply(true);
    let no_url = orch.ui_locale().t("ui.err.tts_no_url").to_string();
    orch.speak_landed_reply(chat, reply);
    assert_eq!(errors(&mut rx), vec![no_url.clone()]);
    orch.speak_landed_reply(chat, reply);
    assert!(errors(&mut rx).is_empty(), "said once");
    orch.handle_tts(TtsScope::Last);
    assert_eq!(
        errors(&mut rx),
        vec![no_url.clone()],
        "asked for, it answers"
    );
    // A settings change may have fixed it: the next failure is news again.
    orch.auto_speech_failed = false;
    orch.speak_landed_reply(chat, reply);
    assert_eq!(errors(&mut rx), vec![no_url]);
}

/// Off by default; and a reply in a conversation not in view is not spoken
/// over the one that is.
#[test]
fn a_landed_reply_is_not_spoken_when_off_or_out_of_view() {
    let (_d, mut orch, mut rx, chat, reply) = with_landed_reply(false);
    orch.speak_landed_reply(chat, reply);
    assert!(errors(&mut rx).is_empty());

    let (_d, mut orch, mut rx, chat, reply) = with_landed_reply(true);
    orch.active_id = None;
    orch.speak_landed_reply(chat, reply);
    assert!(errors(&mut rx).is_empty());
}

/// Through a real turn: the reply that lands is the one spoken.
#[tokio::test]
async fn a_turn_that_lands_speaks_its_reply() {
    let dir = tempfile::tempdir().unwrap();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(dir.path()));
    let mut profile = Profile::new("P", "s");
    profile.companion.speak_replies = true;
    json.upsert_profile(&profile).unwrap();
    let chat = Chat::from_profile(&profile, "c");
    let chat_id = chat.id;
    json.save_chat(&chat).unwrap();
    let mut cfg = no_auto_cfg();
    cfg.tts.mode = TtsMode::External;
    let backend: Arc<dyn EngineBackend> = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("a reply worth hearing".into()),
        ChatChunk::Finished(crate::shared::api::FinishReason::Stop),
    ]));
    let (tx, mut rx, handle) = spawn_orch_at(dir.path(), Some(backend), cfg);
    wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
    tx.send(AppCommand::SendMessage("hi".into())).unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. })).await;
    let err = wait_for(&mut rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .expect("the reply was put to speech");
    let AppEvent::Error(msg) = err else {
        unreachable!()
    };
    let expected =
        crate::shared::i18n::locale(crate::shared::i18n::Lang::default()).t("ui.err.tts_no_url");
    assert_eq!(msg, expected);
    tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
}
