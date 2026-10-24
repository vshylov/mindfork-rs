//! Live `#[ignore]` smoke for voice notes through the messenger bridge (ADR
//! 0013, stage C): a voice note from the phone — real speech, synthesised and
//! written as a phone writes one, OGG/Opus — heard by a live recogniser,
//! answered by a live model, and sent back as a voice note that a recogniser
//! hears again. Against a Bot API of the test's own ([`TelegramFake`]).
//!
//! Each run, two notes in one conversation, English then Russian, each
//! telling the companion a cat's name and asking for it back:
//! 1. the transcript is the person's message, marked as spoken, and names the
//!    cat;
//! 2. the text reply names the cat;
//! 3. a voice note follows it, and heard back it names the cat too.
//!
//! `MINDFORK_ENGINE_URL` (+ `MINDFORK_ENGINE_KEY`) is the model;
//! `MINDFORK_OPENROUTER_KEY` the gateway that hears and speaks —
//! `MINDFORK_VOICE_STT` its recogniser (default `openai/gpt-transcribe`),
//! `MINDFORK_VOICE_TTS` its speech model (default
//! `google/gemini-3.8-flash-lite-tts`, which speaks both languages; Ann in its
//! voice `Kore`, Mara in `Charon`).
//! `MINDFORK_BRIDGE_RUNS` repeats it (default 1).
//! `cargo test voice_live -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::shared::config::{SttMode, TtsMode};
use crate::shared::messenger::telegram_fake::TelegramFake;
use crate::shared::secrets::SecretKey;

const TOKEN: &str = "123:voice";
const ANN_VOICE: &str = "Kore";
const MARA_VOICE: &str = "Charon";
const ANN: i64 = 42;

const PERSONA: &str = "You are Mara, the keeper of a lighthouse on a cold northern coast. You \
talk with Ann, a friend who writes to you from the mainland. Speak as Mara, warmly and plainly, \
in one to three sentences, in the language Ann writes in.";

/// What Ann says, and the word the answers have to hold (lower case, a stem
/// where the word declines).
const NOTES: [(&str, &str); 2] = [
    (
        "Hi Mara! My cat is called Pumpkin. Tell me, what is my cat called?",
        "pumpkin",
    ),
    (
        "Мара, а теперь по-русски: мою кошку зовут Тыковка. Скажи, как зовут мою кошку?",
        "тыковк",
    ),
];

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The recognition and speech settings: both through the gateway.
fn voice_config(cfg: &mut AppConfig, key_env: &str) {
    cfg.stt.mode = SttMode::OpenRouter;
    cfg.stt.openrouter.model_name = Some(env_or("MINDFORK_VOICE_STT", "openai/gpt-transcribe"));
    cfg.stt.openrouter.api_key_env = Some(key_env.into());
    cfg.tts.mode = TtsMode::OpenRouter;
    cfg.tts.openrouter.model_name = Some(env_or(
        "MINDFORK_VOICE_TTS",
        "google/gemini-3.8-flash-lite-tts",
    ));
    // Ann's voice; Mara speaks in a voice of her own (`Profile::voices`).
    cfg.tts.openrouter.voice = Some(ANN_VOICE.into());
    cfg.tts.openrouter.api_key_env = Some(key_env.into());
}

/// `text` spoken by the gateway and written as a phone writes a voice note:
/// the note and its length in seconds.
async fn phone_note(cfg: &AppConfig, text: &str) -> (Vec<u8>, u32) {
    let gateway = crate::shared::tts::GatewaySpeech::default();
    let (engine, _) =
        crate::shared::tts::engines_from_config(&cfg.tts, None, &gateway).expect("speech set up");
    let clip = engine
        .synthesize(text, &tokio_util::sync::CancellationToken::new())
        .await
        .expect("the gateway speaks");
    let (samples, rate) = clip.into_mono().expect("sound");
    let seconds = (samples.len() as u32).div_ceil(rate);
    let note = crate::shared::voice_note::encode(&samples, rate).expect("a note");
    (note, seconds)
}

/// What a recogniser hears in `note`, in lower case.
async fn heard(cfg: &AppConfig, note: &[u8]) -> String {
    let hearing = crate::shared::stt::Hearing {
        stt: cfg.stt.clone(),
        key: None,
        attribution: false,
    };
    crate::shared::stt::transcriber(&hearing, Default::default())
        .expect("recognition set up")
        .transcribe(note)
        .await
        .expect("heard")
        .to_lowercase()
}

#[tokio::test]
#[ignore = "requires a live model (MINDFORK_ENGINE_URL) and MINDFORK_OPENROUTER_KEY"]
async fn a_voice_note_is_heard_answered_and_spoken_back_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL is not set");
        return;
    };
    if std::env::var("MINDFORK_OPENROUTER_KEY").is_err() {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY is not set");
        return;
    }
    if !crate::shared::secrets::scheme_available() {
        eprintln!("skip: this machine cannot store a secret");
        return;
    }
    let runs: usize = std::env::var("MINDFORK_BRIDGE_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    for run in 1..=runs {
        eprintln!("=== run {run}/{runs}");
        one_run(backend.clone()).await;
    }
}

async fn one_run(backend: Arc<dyn EngineBackend>) {
    let fake = TelegramFake::start("mara_bot", TOKEN).await;
    let dir = tempfile::tempdir().unwrap();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(dir.path()));
    let mut p = Profile::new("Mara", PERSONA);
    p.language = crate::shared::i18n::Lang::En;
    p.enabled_tools = crate::features::tools::default_tool_ids();
    p.known_tools = p.enabled_tools.clone();
    p.companion.bridge = true;
    p.companion.speak_replies = true;
    p.voices.insert(TtsMode::OpenRouter, MARA_VOICE.into());
    p.bridge.account = Some(ANN.to_string());
    json.upsert_profile(&p).unwrap();
    let desk = Chat::from_profile(&p, "Desk");
    json.save_chat(&desk).unwrap();
    let mut cfg = no_auto_cfg();
    cfg.default_sampling.max_tokens = Some(1500);
    cfg.interface.language = crate::shared::i18n::Lang::En;
    cfg.bridge.telegram_api = Some(fake.api.clone());
    voice_config(&mut cfg, "MINDFORK_OPENROUTER_KEY");
    crate::shared::secrets::put_key(
        &mut cfg.api_keys,
        &SecretKey::BridgeToken(p.id).storage_name(),
        TOKEN,
        || "test".into(),
    )
    .unwrap();
    let speech = cfg.clone();
    let (tx, mut rx, handle) = spawn_orch_at(dir.path(), Some(backend), cfg);
    wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;

    for (n, (said, word)) in NOTES.iter().enumerate() {
        let (note, seconds) = phone_note(&speech, said).await;
        eprintln!("--- note {}: {seconds} s, {} bytes", n + 1, note.len());
        fake.voice(ANN, "ann", &note, seconds);
        let texts = fake.wait_texts(ANN, n + 1, 180).await;
        assert!(texts.len() > n, "no reply to note {}: {texts:?}", n + 1);
        let reply = texts[n].to_lowercase();
        eprintln!("    reply: {}", texts[n]);
        assert!(reply.contains(word), "the reply names the cat: {reply}");
        let voices = fake.wait_voices(ANN, n + 1, 180).await;
        assert!(voices.len() > n, "no voice note after reply {}", n + 1);
        let back = heard(&speech, &voices[n]).await;
        eprintln!("    voice ({} bytes) heard as: {back}", voices[n].len());
        assert!(back.contains(word), "the voice note names the cat: {back}");
    }

    tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat = json
        .load_profiles()
        .unwrap()
        .into_iter()
        .find(|x| x.id == p.id)
        .and_then(|x| x.bridge.chat)
        .expect("a bridged conversation");
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    let kept = storage.json().load_chat(chat).unwrap().unwrap();
    let spoken: Vec<_> = kept
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .collect();
    assert_eq!(spoken.len(), 2, "two notes, two messages");
    for (m, (_, word)) in spoken.iter().zip(NOTES.iter()) {
        eprintln!("    transcript: {} ({:?})", m.text, m.voice);
        assert!(m.voice.is_some(), "marked as spoken");
        assert!(m.text.to_lowercase().contains(word), "{}", m.text);
    }
    // Typing was shown, then recording.
    let actions: Vec<String> = fake
        .sent()
        .into_iter()
        .filter(|s| s.method == "sendChatAction")
        .filter_map(|s| s.body["action"].as_str().map(str::to_string))
        .collect();
    assert!(actions.iter().any(|a| a == "record_voice"), "{actions:?}");
}
