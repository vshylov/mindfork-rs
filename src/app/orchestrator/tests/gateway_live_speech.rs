//! Live smokes of `/tts` through the OpenRouter mode — stage 3 of
//! docs/research/openrouter-mode.md (§7). Part of the [`super`] module.
//!
//! The whole road, as the application takes it: the command, the engines built
//! from a snapshot of the settings, the format negotiated, the playback queue
//! and the sound card. What the client's own smokes cannot say is said here —
//! that a clip of each kind the gateway answers with is **played**, in real
//! time and to its end, and that nothing on the way is an error in the feed.
//!
//! Declared by `MINDFORK_OPENROUTER_KEY`, and by a sound card: on a machine
//! without one each smoke skips, saying so. What is played is audible.

use std::time::{Duration, Instant};

use super::*;
use crate::features::tts_command::TtsScope;
use crate::shared::api::catalogue::{self, CatalogueRequest, CatalogueShape};
use crate::shared::config::{CloudProvider, TtsCloudSettings, TtsMode};
use crate::shared::i18n::{Lang, locale};

const ASKED: &str = "What is the weather like?";
const ANSWERED: &str = "Testing, one two three. The weather is fine today.";

fn key() -> Option<String> {
    let key = std::env::var("MINDFORK_OPENROUTER_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    if key.is_none() {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY not set");
    }
    key
}

/// The model a smoke runs on: the one the run names, or the one it was
/// measured on.
fn model(var: &str, measured: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| measured.to_string())
}

/// The voices the gateway lists for a model, as the voice rows offer them.
async fn voices(key: &str, model: &str) -> Vec<String> {
    let request = CatalogueRequest {
        shape: CatalogueShape::OpenRouterSpeech,
        base: CloudProvider::OpenRouter.chat_base_url().to_string(),
        key: Some(key.to_string()),
        attribution: true,
    };
    let listed = catalogue::fetch(&request).await.expect("the speech list");
    let entry = listed.into_iter().find(|m| m.id == model);
    entry
        .unwrap_or_else(|| panic!("{model} is not in the gateway's speech list"))
        .voices
}

/// What one `/tts` came to.
#[derive(Debug)]
enum Spoken {
    /// Played to its end, in this long.
    For(Duration),
    /// This machine has no sound card to play it on.
    NoSoundCard,
}

/// A chat of one exchange, the speech slot on the gateway with this section,
/// and `/tts` over `scope`. Any error in the feed fails the smoke, but for the
/// one that says there is nothing to play on.
async fn tts(section: TtsCloudSettings, scope: TtsScope) -> Spoken {
    let mut config = AppConfig::default();
    config.tts.mode = TtsMode::OpenRouter;
    config.tts.openrouter = TtsCloudSettings {
        api_key_env: Some("MINDFORK_OPENROUTER_KEY".into()),
        ..section
    };
    let backend: Arc<dyn EngineBackend> = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text(ANSWERED.into()),
        ChatChunk::Finished(crate::shared::api::FinishReason::Stop),
    ]));
    let (_dir, tx, mut rx, handle) = spawn_orch_cfg(Some(backend), config);
    wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    tx.send(AppCommand::SendMessage(ASKED.into())).unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. })).await;

    let no_sound_card = locale(Lang::default()).tf("ui.err.tts_no_audio", &[("err", "")]);
    tx.send(AppCommand::Tts(scope)).unwrap();
    let mut started = None;
    let spoken = loop {
        let event = tokio::time::timeout(Duration::from_secs(120), rx.recv())
            .await
            .expect("the speech neither ended nor failed in two minutes")
            .expect("the orchestrator is running");
        match event {
            AppEvent::Error(said) if said.starts_with(no_sound_card.trim_end()) => {
                eprintln!("skip: {said}");
                break Spoken::NoSoundCard;
            }
            AppEvent::Error(said) => panic!("the feed was told of an error: {said}"),
            AppEvent::TtsActive(true) => started = Some(Instant::now()),
            AppEvent::TtsActive(false) => {
                let started = started.expect("speech ended that never began");
                break Spoken::For(started.elapsed());
            }
            _ => {}
        }
    };
    tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
    spoken
}

/// A sentence of four seconds, asked for and played: longer than the request
/// alone takes, and not for ever.
fn played_in_real_time(spoken: Spoken, sentences: u32, what: &str) {
    let Spoken::For(lasted) = spoken else { return };
    eprintln!("   {what}: spoken in {lasted:.2?}");
    assert!(
        lasted >= Duration::from_secs(2) * sentences,
        "{what}: {lasted:.2?} is not the time a sentence takes to say"
    );
    assert!(
        lasted < Duration::from_secs(40) * sentences,
        "{what}: {lasted:.2?}"
    );
}

/// A model that takes MP3 only, through the application: the refusal, the
/// second request, the container through the decoder to the sound card — and
/// with a voice of the user's, two engines on one model, the second of which
/// knows what the first was refused.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and a sound card (speech is audible)"]
async fn a_model_that_takes_mp3_only_is_spoken_in_two_voices_live() {
    let Some(key) = key() else { return };
    let model = model(
        "MINDFORK_OPENROUTER_TTS_MP3_MODEL",
        "minimax/speech-2.8-turbo",
    );
    let listed = voices(&key, &model).await;
    assert!(listed.len() >= 2, "{model} lists {listed:?}");
    eprintln!("== {model}, voices {:?}", &listed[..2]);
    let section = TtsCloudSettings {
        model_name: Some(model.clone()),
        voice: Some(listed[0].clone()),
        user_voice: Some(listed[1].clone()),
        ..Default::default()
    };
    played_in_real_time(tts(section, TtsScope::All).await, 2, &model);
}

/// A model that answers raw samples at 44.1 kHz and lists no voice: spoken
/// with none set.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and a sound card (speech is audible)"]
async fn a_model_at_44_khz_without_a_voice_is_spoken_live() {
    if key().is_none() {
        return;
    }
    let model = model("MINDFORK_OPENROUTER_TTS_44K_MODEL", "fish-audio/s1");
    eprintln!("== {model}, no voice");
    let section = TtsCloudSettings {
        model_name: Some(model.clone()),
        ..Default::default()
    };
    played_in_real_time(tts(section, TtsScope::Last).await, 1, &model);
}

/// A model that takes raw samples only, in the voice the gateway lists first.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and a sound card (speech is audible)"]
async fn a_model_that_takes_raw_samples_only_is_spoken_live() {
    let Some(key) = key() else { return };
    let model = model(
        "MINDFORK_OPENROUTER_TTS_RAW_MODEL",
        "google/gemini-3.8-flash-lite-tts",
    );
    let voice = voices(&key, &model).await.into_iter().next();
    eprintln!("== {model}, voice {voice:?}");
    let section = TtsCloudSettings {
        model_name: Some(model.clone()),
        voice,
        ..Default::default()
    };
    played_in_real_time(tts(section, TtsScope::Last).await, 1, &model);
}

/// What the gateway refuses is an error in the feed, in its own words: a
/// model that needs a voice and is given none.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and a sound card"]
async fn a_missing_voice_is_said_in_the_feed_in_the_gateways_words_live() {
    if key().is_none() {
        return;
    }
    let mut config = AppConfig::default();
    config.tts.mode = TtsMode::OpenRouter;
    config.tts.openrouter = TtsCloudSettings {
        model_name: Some("x-ai/grok-voice-tts-1.0".into()),
        api_key_env: Some("MINDFORK_OPENROUTER_KEY".into()),
        ..Default::default()
    };
    let backend: Arc<dyn EngineBackend> = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text(ANSWERED.into()),
        ChatChunk::Finished(crate::shared::api::FinishReason::Stop),
    ]));
    let (_dir, tx, mut rx, handle) = spawn_orch_cfg(Some(backend), config);
    wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    tx.send(AppCommand::SendMessage(ASKED.into())).unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. })).await;
    tx.send(AppCommand::Tts(TtsScope::Last)).unwrap();
    let said = wait_for(&mut rx, |e| matches!(e, AppEvent::Error(_))).await;
    let Some(AppEvent::Error(said)) = said else {
        panic!("no error was said")
    };
    eprintln!("   {said}");
    if said.contains("explicit voice is required") {
        assert!(!said.contains('{'), "the sentence, not the JSON: {said}");
        // The chip the command lit goes out when the speech has ended.
        let ended = wait_for(&mut rx, |e| matches!(e, AppEvent::TtsActive(false))).await;
        assert!(ended.is_some(), "the speech never ended");
    } else {
        eprintln!("skip: nothing to play on, so nothing was asked");
    }
    tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
}
