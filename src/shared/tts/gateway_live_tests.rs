//! Live smokes of speech through the OpenRouter gateway — `#[ignore]`, run by
//! hand against the gateway before a change to this mode is merged
//! (AGENTS.md §3). Part of the mode's live gate: `cargo test gateway_live --
//! --ignored --nocapture --test-threads=1`.
//!
//! Declared by `MINDFORK_OPENROUTER_KEY`; without it every smoke skips, saying
//! so. Each names the model it was measured on and takes another from a
//! variable of its own: a model's name ages, and its **kind** — takes raw
//! samples only, takes MP3 only, answers at 44.1 kHz — is what the smoke is
//! about. A smoke whose model turns out not to be of that kind fails rather
//! than skips (docs/lessons.md §9).
//!
//! **The assertion is the text.** A `200` with 180 KB in it is not speech: what
//! the model answered is transcribed back by a speech-to-text model through the
//! gateway's own `/audio/transcriptions`, and the sentence is looked for in
//! what was heard (docs/research/openrouter-mode.md §10).
//!
//! The file is named `…_tests` like every test file of the crate: the coverage
//! report leaves files so named out, and one it does not recognise is
//! production code to it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use tokio_util::sync::CancellationToken;

use super::openai::{FormatMemo, GatewayFormat, OpenAiTts};
use super::playback::Playback;
use super::{AudioClip, GatewaySpeech, TtsEngine};
use crate::shared::api::catalogue::{self, CatalogueRequest, CatalogueShape, ModelRole, ModelSlot};
use crate::shared::config::CloudProvider;

const ENGLISH: &str = "Testing, one two three. The weather is fine today.";
const RUSSIAN: &str = "Проверка связи, раз два три. Сегодня хорошая погода.";

/// Measured on 2026-09-29: answers `pcm` and refuses `mp3`.
const RAW_ONLY: &str = "google/gemini-3.8-flash-lite-tts";
/// …answers `mp3` and refuses `pcm`.
const MP3_ONLY: &str = "minimax/speech-2.8-turbo";
/// …answers raw samples at 44.1 kHz, and lists no voice.
const AT_44_KHZ: &str = "fish-audio/s1";
/// What hears the speech back.
const LISTENER: &str = "openai/whisper-large-v3-turbo";

fn key() -> Option<String> {
    let key = std::env::var("MINDFORK_OPENROUTER_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    if key.is_none() {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY not set");
    }
    key
}

fn base() -> &'static str {
    CloudProvider::OpenRouter.chat_base_url()
}

/// The model a smoke runs on: the one the run names, or the one it was
/// measured on.
fn model(var: &str, measured: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| measured.to_string())
}

fn session() -> GatewaySpeech {
    GatewaySpeech {
        attribution: true,
        formats: Arc::new(FormatMemo::default()),
    }
}

/// The gateway's speech list, as the picker asks for it.
async fn speech_list(key: Option<&str>) -> Vec<catalogue::CatalogModel> {
    let request = CatalogueRequest {
        shape: CatalogueShape::OpenRouterSpeech,
        base: base().to_string(),
        key: key.map(str::to_string),
        attribution: true,
    };
    let listed = catalogue::fetch(&request).await.expect("the speech list");
    catalogue::for_slot(listed, ModelSlot::Speech)
}

/// The first voice the gateway lists for a model — what a user who opened the
/// voice row and pressed `Enter` would have.
async fn first_voice(key: &str, model: &str) -> Option<String> {
    let listed = speech_list(Some(key)).await;
    let entry = listed.iter().find(|m| m.id == model);
    let entry = entry.unwrap_or_else(|| panic!("{model} is not in the gateway's speech list"));
    entry.voices.first().cloned()
}

fn client(key: &str, model: &str, voice: Option<String>, session: &GatewaySpeech) -> OpenAiTts {
    OpenAiTts::gateway(
        base().to_string(),
        key.to_string(),
        model.to_string(),
        voice,
        1.0,
        session,
    )
}

async fn spoken(engine: &OpenAiTts, text: &str) -> AudioClip {
    let started = Instant::now();
    let clip = engine
        .synthesize(text, &CancellationToken::new())
        .await
        .expect("the gateway speaks");
    match &clip {
        AudioClip::Pcm {
            sample_rate,
            channels,
            bytes,
        } => eprintln!(
            "   raw samples: {} bytes at {sample_rate} Hz x{channels} — {:.2} s of audio, in {:.2?}",
            bytes.len(),
            seconds(*sample_rate, *channels, bytes.len()),
            started.elapsed()
        ),
        AudioClip::Encoded(bytes) => eprintln!(
            "   a container: {} bytes, first four {:02x?}, in {:.2?}",
            bytes.len(),
            &bytes[..bytes.len().min(4)],
            started.elapsed()
        ),
    }
    clip
}

fn seconds(sample_rate: u32, channels: u16, bytes: usize) -> f64 {
    bytes as f64 / 2.0 / f64::from(channels.max(1)) / f64::from(sample_rate.max(1))
}

/// Raw samples under a WAV header, for the listener: it takes containers.
fn wav(sample_rate: u32, channels: u16, pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    let block = channels * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// What a speech-to-text model hears in the clip, in lower case and without
/// punctuation.
async fn heard(key: &str, clip: &AudioClip) -> String {
    let (data, format) = match clip {
        AudioClip::Pcm {
            sample_rate,
            channels,
            bytes,
        } => (wav(*sample_rate, *channels, bytes), "wav"),
        AudioClip::Encoded(bytes) => (bytes.clone(), "mp3"),
    };
    let body = serde_json::json!({
        "model": model("MINDFORK_OPENROUTER_LISTENER", LISTENER),
        "input_audio": {
            "data": base64::engine::general_purpose::STANDARD.encode(data),
            "format": format,
        },
    });
    let answer = reqwest::Client::new()
        .post(format!("{}/audio/transcriptions", base()))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .expect("the listener answers");
    let status = answer.status();
    let answer: serde_json::Value = answer.json().await.expect("the listener's JSON");
    let text = answer["text"].as_str().unwrap_or_else(|| {
        panic!("the listener heard nothing it could write down: {status} {answer}")
    });
    eprintln!("   heard: {text:?}");
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect()
}

/// The gateway's speech list is what the picker says it is: public, the same
/// with a key, every entry a speech model by the gateway's own word, most of
/// them with voices — and the three models the smokes below run on are in it,
/// of the kind each is there for.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn the_gateways_speech_list_answers_live() {
    let Some(key) = key() else { return };
    let public = speech_list(None).await;
    let mine = speech_list(Some(&key)).await;
    eprintln!(
        "   {} speech models without a key, {} with; with voices: {}",
        public.len(),
        mine.len(),
        mine.iter().filter(|m| !m.voices.is_empty()).count()
    );
    assert!(public.len() >= 10, "{}", public.len());
    assert!(mine.len() >= 10 && mine.len() <= public.len());
    assert!(public.iter().all(|m| m.role == ModelRole::Speech));
    assert!(
        public.iter().all(|m| !m.id.contains("whisper")),
        "a listener is not a speaker"
    );
    let voices = |id: &str| {
        let entry = mine.iter().find(|m| m.id == id);
        entry.map(|m| m.voices.len())
    };
    assert!(voices(RAW_ONLY).is_some_and(|n| n > 0), "{RAW_ONLY}");
    assert!(voices(MP3_ONLY).is_some_and(|n| n > 0), "{MP3_ONLY}");
    assert_eq!(voices(AT_44_KHZ), Some(0), "{AT_44_KHZ} lists no voice");
}

/// The go/no-go, first half: a model that takes raw samples only. Asked once,
/// answered at the rate its label names, and what it said is heard back — in
/// English and in Russian.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_model_that_takes_raw_samples_only_is_heard_back_live() {
    let Some(key) = key() else { return };
    let model = model("MINDFORK_OPENROUTER_TTS_RAW_MODEL", RAW_ONLY);
    let voice = first_voice(&key, &model).await;
    eprintln!("== {model}, voice {voice:?}");
    let session = session();
    let engine = client(&key, &model, voice.clone(), &session);

    let clip = spoken(&engine, ENGLISH).await;
    assert!(matches!(clip, AudioClip::Pcm { .. }), "raw samples");
    assert_eq!(session.formats.of(&model), GatewayFormat::Pcm);
    let text = heard(&key, &clip).await;
    assert!(text.contains("weather") && text.contains("fine"), "{text}");

    let clip = spoken(&engine, RUSSIAN).await;
    let text = heard(&key, &clip).await;
    assert!(text.contains("погода"), "{text}");

    // The control: that this model is of the kind the smoke is about — asked
    // for the other format alone, it refuses.
    let refused = reqwest::Client::new()
        .post(format!("{}/audio/speech", base()))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": model, "input": ENGLISH, "voice": voice, "response_format": "mp3",
        }))
        .send()
        .await
        .expect("an answer");
    assert_eq!(refused.status(), 400, "{model} takes mp3 now");
}

/// The go/no-go, second half: a model that takes MP3 only. The client asks
/// for raw samples, is refused, asks for MP3 and hands a container to the
/// decoder; the session remembers, and what was said is heard back.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_model_that_takes_mp3_only_is_heard_back_live() {
    let Some(key) = key() else { return };
    let model = model("MINDFORK_OPENROUTER_TTS_MP3_MODEL", MP3_ONLY);
    let voice = first_voice(&key, &model).await;
    eprintln!("== {model}, voice {voice:?}");
    let session = session();
    assert_eq!(
        session.formats.of(&model),
        GatewayFormat::Pcm,
        "unknown yet"
    );
    let engine = client(&key, &model, voice, &session);

    let clip = spoken(&engine, ENGLISH).await;
    let AudioClip::Encoded(bytes) = &clip else {
        panic!("{model} answered raw samples: it is not of the kind this smoke is about")
    };
    assert!(bytes.len() > 5_000, "{} bytes", bytes.len());
    assert_eq!(session.formats.of(&model), GatewayFormat::Mp3);
    // Not the container as the gateway sent it: what the application's own
    // decoder makes of it, which is what is played. The sentence has to be
    // there to its last word — a decoder that trusts the first frame's tag
    // stops a streamed clip short, or does not start.
    let played = through_the_decoder(bytes.clone());
    let text = heard(&key, &played).await;
    assert!(text.contains("weather") && text.contains("today"), "{text}");

    // The second fragment is asked for what the model takes: no refused
    // request stands before it. A refused request takes 0.1 s, so the time
    // says nothing — the memo does, and the unit test counts the requests.
    let AudioClip::Encoded(bytes) = spoken(&engine, RUSSIAN).await else {
        panic!("the second fragment came as raw samples")
    };
    let text = heard(&key, &through_the_decoder(bytes)).await;
    assert!(
        text.contains("сегодня") && text.contains("погода"),
        "{text}"
    );
}

/// A container as the application plays it: through the decoder the playback
/// queue uses, to the samples the sound card would be given.
fn through_the_decoder(container: Vec<u8>) -> AudioClip {
    use rodio::Source;
    let source = super::playback::decoder(container).expect("the decoder reads it");
    let (sample_rate, channels) = (source.sample_rate().get(), source.channels().get());
    let bytes: Vec<u8> = source
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16).to_le_bytes())
        .collect();
    eprintln!(
        "   decoded: {sample_rate} Hz x{channels}, {:.2} s",
        seconds(sample_rate, channels, bytes.len())
    );
    AudioClip::Pcm {
        sample_rate,
        channels,
        bytes,
    }
}

/// A model that answers at 44.1 kHz plays at its own rate: the clip carries
/// the rate the label named, what it holds is the sentence, and through the
/// sound card it lasts what it lasts — at the 24 kHz the other clouds answer
/// in it would last 1.84 times as long, and say it an octave lower.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY, and a sound card for the second half"]
async fn a_model_at_44_khz_plays_at_its_own_rate_live() {
    let Some(key) = key() else { return };
    let model = model("MINDFORK_OPENROUTER_TTS_44K_MODEL", AT_44_KHZ);
    let voice = first_voice(&key, &model).await;
    eprintln!("== {model}, voice {voice:?}");
    let clip = spoken(&client(&key, &model, voice, &session()), ENGLISH).await;
    let AudioClip::Pcm {
        sample_rate,
        channels,
        bytes,
    } = &clip
    else {
        panic!("{model} answered a container")
    };
    assert_eq!(*sample_rate, 44_100, "{model} is not at 44.1 kHz now");
    let lasts = seconds(*sample_rate, *channels, bytes.len());
    assert!((1.5..15.0).contains(&lasts), "a sentence, not {lasts:.2} s");
    let text = heard(&key, &clip).await;
    assert!(text.contains("weather") && text.contains("fine"), "{text}");

    let Ok(playback) = Playback::open() else {
        eprintln!("skip: no audio device — the rate was not heard, only read");
        return;
    };
    let started = Instant::now();
    playback.enqueue(clip.clone()).expect("queued");
    while !playback.is_drained() && started.elapsed() < Duration::from_secs(40) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let played = started.elapsed().as_secs_f64();
    eprintln!(
        "   played in {played:.2} s a clip of {lasts:.2} s ({:.2} of it)",
        played / lasts
    );
    assert!(
        (0.85..1.25).contains(&(played / lasts)),
        "{played:.2} s for {lasts:.2} s of audio: at 24 kHz it would be {:.2} s",
        lasts * 44_100.0 / 24_000.0
    );
}

/// A key the gateway refuses is said in the gateway's words, by the first
/// request that needs it. Speech has no status to say it in earlier: its
/// client is built when `/tts` is typed.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_refused_key_is_said_in_the_gateways_words_live() {
    if key().is_none() {
        return;
    }
    // A key of the right shape that is nobody's: nothing of this machine's is
    // sent in its place.
    let nobody = format!("sk-or-v1-{}", "0".repeat(64));
    let engine = client(&nobody, RAW_ONLY, Some("Kore".into()), &session());
    let err = engine
        .synthesize(ENGLISH, &CancellationToken::new())
        .await
        .expect_err("refused");
    eprintln!("   {err}");
    let text = err.to_string();
    assert!(
        text.contains("401") && text.contains("User not found"),
        "{text}"
    );
    assert!(!text.contains('{'), "the sentence, not the JSON: {text}");
}

/// A clip lasts what its rate and its channels say it does (needs a sound
/// card and no network; two tones are audible). The clouds this player was
/// written for answer 24 kHz mono; the gateway's models answer at 44.1 kHz
/// too, and one of them in stereo. Columns: the rate, the channels. The clip
/// is a second long in each row: read at another rate, or as mono where it is
/// stereo, it would last 1.8 or 2 times that. It stands in for the one model
/// that answers in stereo, which takes 11 to 23 s a sentence and is not spoken
/// here.
#[test]
#[ignore = "requires a sound card (two short tones are audible)"]
fn a_clip_lasts_what_its_rate_and_channels_say_live() {
    let Ok(playback) = Playback::open() else {
        eprintln!("skip: no audio device available");
        return;
    };
    for (rate, channels) in [(44_100u32, 1u16), (24_000, 2)] {
        let frames = rate as usize;
        let mut bytes = Vec::with_capacity(frames * 2 * usize::from(channels));
        for i in 0..frames {
            let t = i as f32 / rate as f32;
            let amp = (t * 440.0 * std::f32::consts::TAU).sin() * 0.2;
            for _ in 0..channels {
                bytes.extend_from_slice(&((amp * i16::MAX as f32) as i16).to_le_bytes());
            }
        }
        let started = std::time::Instant::now();
        playback
            .enqueue(AudioClip::Pcm {
                sample_rate: rate,
                channels,
                bytes,
            })
            .expect("queued");
        while !playback.is_drained() && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let lasted = started.elapsed().as_secs_f32();
        eprintln!("{rate} Hz x{channels}: a second of audio played in {lasted:.2} s");
        assert!(
            (0.85..1.3).contains(&lasted),
            "{rate} Hz x{channels}: {lasted:.2} s"
        );
    }
}
