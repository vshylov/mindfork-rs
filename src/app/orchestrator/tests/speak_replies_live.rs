//! Live `#[ignore]` smokes for a companion's voice (spec §11.9,
//! docs/research/companion-platform.md C7 (a)): two profiles that speak their
//! replies as they land, one in a voice of its own and one in the settings',
//! each reply put to speech with no `/tts` typed and played on the sound card.
//!
//! - `…_in_their_own_voices_live` — a speech server of the test's own (it
//!   records what it was asked and answers with a tone): which voice each
//!   companion's reply was asked in. Needs a sound card only.
//! - `…_through_the_gateway_live` — the same through OpenRouter, a real
//!   provider, in two of the voices it lists: audible.
//!   `MINDFORK_OPENROUTER_KEY` declares it.
//!
//! `cargo test speak_replies_live -- --ignored --nocapture --test-threads=1`

use std::time::{Duration, Instant};

use super::*;
use crate::shared::config::{TtsCloudSettings, TtsMode};
use crate::shared::i18n::{Lang, locale};

/// A second of a 440 Hz tone, as a 16-bit mono WAV at 24 kHz.
fn tone() -> Vec<u8> {
    let rate = 24_000u32;
    let samples: Vec<i16> = (0..rate)
        .map(|i| {
            let t = i as f32 / rate as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 6000.0) as i16
        })
        .collect();
    let data = samples.len() as u32 * 2;
    let mut w = Vec::new();
    w.extend(b"RIFF");
    w.extend((36 + data).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(rate.to_le_bytes());
    w.extend((rate * 2).to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend(data.to_le_bytes());
    for s in samples {
        w.extend(s.to_le_bytes());
    }
    w
}

/// What a reply's speech came to.
enum Spoken {
    For(Duration),
    NoSoundCard,
}

/// Two companions on disk — Sol with a voice of its own under `mode`, Ada
/// with none — each with a chat; the app started with `config`. Each chat is
/// opened and sent a message; each reply is waited for until its speech ends.
async fn two_companions(config: AppConfig, mode: TtsMode, sol_voice: &str) -> Vec<Spoken> {
    let dir = tempfile::tempdir().unwrap();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(dir.path()));
    let mut chats = Vec::new();
    for (name, voice) in [("Sol", Some(sol_voice)), ("Ada", None)] {
        let mut p = Profile::new(name, "You are a companion. Answer in one short sentence.");
        p.language = Lang::En;
        p.companion.speak_replies = true;
        if let Some(v) = voice {
            p.voices.insert(mode, v.to_string());
        }
        json.upsert_profile(&p).unwrap();
        let chat = Chat::from_profile(&p, name);
        chats.push(chat.id);
        json.save_chat(&chat).unwrap();
    }
    let backend: Arc<dyn EngineBackend> = CapturingBackend::new();
    let (tx, mut rx, handle) = spawn_orch_at(dir.path(), Some(backend), config);
    wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    let no_sound_card = locale(Lang::default()).tf("ui.err.tts_no_audio", &[("err", "")]);
    let mut out = Vec::new();
    for chat in chats {
        tx.send(AppCommand::SwitchChat(chat)).unwrap();
        tx.send(AppCommand::SendMessage("Good evening!".into()))
            .unwrap();
        let mut started = None;
        let spoken = loop {
            let event = tokio::time::timeout(Duration::from_secs(120), rx.recv())
                .await
                .expect("the reply was neither spoken nor refused in two minutes")
                .expect("the orchestrator is running");
            match event {
                AppEvent::Error(said) if said.starts_with(no_sound_card.trim_end()) => {
                    eprintln!("skip: {said}");
                    break Spoken::NoSoundCard;
                }
                AppEvent::Error(said) => panic!("the feed was told of an error: {said}"),
                AppEvent::TtsActive(true) => started = Some(Instant::now()),
                AppEvent::TtsActive(false) => {
                    break Spoken::For(started.expect("speech ended that never began").elapsed());
                }
                _ => {}
            }
        };
        let stop = matches!(spoken, Spoken::NoSoundCard);
        out.push(spoken);
        if stop {
            break;
        }
    }
    tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
    out
}

#[tokio::test]
#[ignore = "requires a sound card (a tone is played)"]
async fn two_companions_speak_their_replies_in_their_own_voices_live() {
    let stub = crate::shared::http_stub::Stub::serving(vec![
        crate::shared::http_stub::answered("audio/wav", &tone()),
        crate::shared::http_stub::answered("audio/wav", &tone()),
    ])
    .await;
    let mut config = no_auto_cfg();
    config.tts.mode = TtsMode::External;
    config.tts.external.url = Some(stub.url.clone());
    config.tts.external.voice = Some("the-settings-voice".into());
    let spoken = two_companions(config, TtsMode::External, "sols-own-voice").await;
    if spoken.iter().any(|s| matches!(s, Spoken::NoSoundCard)) {
        return;
    }
    let voices: Vec<String> = stub
        .bodies()
        .iter()
        .map(|b| b["voice"].as_str().unwrap_or_default().to_string())
        .collect();
    eprintln!("asked in: {voices:?}");
    assert_eq!(voices, ["sols-own-voice", "the-settings-voice"]);
    for s in spoken {
        let Spoken::For(lasted) = s else {
            unreachable!()
        };
        eprintln!("   played for {lasted:.2?}");
        assert!(
            lasted >= Duration::from_millis(900),
            "a second's tone in {lasted:.2?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and a sound card (speech is audible)"]
async fn two_companions_speak_their_replies_through_the_gateway_live() {
    let Some(_key) = std::env::var("MINDFORK_OPENROUTER_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY not set");
        return;
    };
    let model = std::env::var("MINDFORK_OPENROUTER_TTS_MP3_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "minimax/speech-2.8-turbo".into());
    let request = crate::shared::api::catalogue::CatalogueRequest {
        shape: crate::shared::api::catalogue::CatalogueShape::OpenRouterSpeech,
        base: crate::shared::config::CloudProvider::OpenRouter
            .chat_base_url()
            .to_string(),
        key: Some(_key),
        attribution: true,
    };
    let listed = crate::shared::api::catalogue::fetch(&request)
        .await
        .expect("the speech list");
    let voices = listed
        .into_iter()
        .find(|m| m.id == model)
        .unwrap_or_else(|| panic!("{model} is not in the gateway's speech list"))
        .voices;
    assert!(voices.len() >= 2, "{model} lists {voices:?}");
    eprintln!(
        "== {model}: Sol in {}, Ada in the settings' {}",
        voices[1], voices[0]
    );
    let mut config = no_auto_cfg();
    config.tts.mode = TtsMode::OpenRouter;
    config.tts.openrouter = TtsCloudSettings {
        model_name: Some(model),
        voice: Some(voices[0].clone()),
        api_key_env: Some("MINDFORK_OPENROUTER_KEY".into()),
        ..Default::default()
    };
    let spoken = two_companions(config, TtsMode::OpenRouter, &voices[1]).await;
    for s in spoken {
        if let Spoken::For(lasted) = s {
            eprintln!("   spoken for {lasted:.2?}");
            assert!(lasted >= Duration::from_millis(300), "{lasted:.2?}");
        }
    }
}
