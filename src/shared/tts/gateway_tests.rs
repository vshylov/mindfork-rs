//! Speech through the OpenRouter gateway: the format negotiated, what came
//! back read from its label, and what is sent with the request
//! (docs/research/openrouter-mode.md §4.4, fork F9, §13).
//!
//! Over a real socket, like the engine clients' own tests: what is asserted is
//! what left the machine. The bodies the stub answers with are the gateway's
//! own, measured on 2026-09-29.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::openai::{FormatMemo, GatewayFormat, OpenAiTts};
use super::{AudioClip, GatewaySpeech, TtsEngine, TtsSetupError, engines_from_config};
use crate::shared::config::{TtsCloudSettings, TtsMode, TtsSettings};
use crate::shared::http_stub::{Step, Stub, answered, body_of, refused};

const MODEL: &str = "minimax/speech-2.8-turbo";

/// What the two models that take one format each say to the other one.
const MP3_ONLY: &str = r#"{"error":{"message":"MiniMax TTS only supports response_format=\"mp3\" for streaming. Got \"pcm\".","code":400}}"#;
const PCM_ONLY: &str = r#"{"error":{"message":"Gemini TTS only supports response_format=\"pcm\". Got \"mp3\".","code":400}}"#;
const NO_VOICE: &str =
    r#"{"error":{"message":"An explicit voice is required for this TTS provider.","code":400}}"#;
const WRONG_KEY: &str = r#"{"error":{"message":"User not found.","code":401}}"#;

/// Four samples of raw PCM that begin with `0xFFFF` — a sample of −1, and an
/// MP3 frame sync to anything that sniffs.
const SAMPLES: &[u8] = &[0xFF, 0xFF, 0x00, 0x40, 0x00, 0x80, 0x34, 0x12];
/// What is served as an MP3. The client does not look inside.
const FRAMES: &[u8] = &[0xFF, 0xFB, 0x90, 0x64, 0x00, 0x0F, 0xF0, 0x00];

fn audio(label: &'static str, body: &[u8]) -> Step {
    answered(label, body)
}

/// The `response_format` of every request, in the order they came.
fn formats_asked(stub: &Stub) -> Vec<String> {
    let asked = |body: serde_json::Value| body["response_format"].as_str().map(str::to_string);
    stub.bodies().into_iter().filter_map(asked).collect()
}

fn named() -> GatewaySpeech {
    GatewaySpeech {
        attribution: true,
        formats: Arc::new(FormatMemo::default()),
    }
}

fn client(url: &str, gateway: &GatewaySpeech) -> OpenAiTts {
    OpenAiTts::gateway(
        url.to_string(),
        "sk-or-stored".into(),
        MODEL.into(),
        Some("English_expressive_narrator".into()),
        1.0,
        gateway,
    )
}

async fn said(engine: &dyn TtsEngine) -> anyhow::Result<AudioClip> {
    engine.synthesize("hello", &CancellationToken::new()).await
}

/// The common case — 19 of the 21 models: raw samples are asked for and
/// answered, in one request, and what the clip is played at is what the
/// label said. The request names the model and the voice, carries the key and
/// the application's name, and no `instructions`: that is not a field of this
/// route.
#[tokio::test]
async fn a_model_that_takes_raw_samples_is_asked_once_and_plays_at_its_own_rate() {
    let stub = Stub::serving(vec![audio("audio/pcm;rate=44100;channels=1", SAMPLES)]).await;
    let gateway = named();
    let clip = said(&client(&stub.url, &gateway)).await.expect("a clip");
    assert_eq!(
        clip,
        AudioClip::Pcm {
            sample_rate: 44_100,
            channels: 1,
            bytes: SAMPLES.to_vec(),
        }
    );
    let seen = stub.requests();
    assert_eq!(seen.len(), 1, "{seen:#?}");
    let head = seen[0].to_ascii_lowercase();
    assert!(head.starts_with("post /v1/audio/speech "), "{head}");
    assert!(
        head.contains("authorization: bearer sk-or-stored"),
        "{head}"
    );
    assert!(head.contains("x-openrouter-title: mindfork"), "{head}");
    assert!(head.contains("http-referer: https://mindfork.io"), "{head}");
    let body: serde_json::Value = serde_json::from_str(body_of(&seen[0])).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "model": MODEL,
            "input": "hello",
            "voice": "English_expressive_narrator",
            "response_format": "pcm",
        })
    );
    assert_eq!(gateway.formats.of(MODEL), GatewayFormat::Pcm);
}

/// A model that takes only the other format: the refusal names the field, the
/// other format is asked once, and the container goes to the decoder. The
/// session remembers — the next fragment, and the next command's client, ask
/// for what the model takes and are not refused again.
#[tokio::test]
async fn a_refusal_that_names_the_format_is_asked_again_in_the_other_one_and_remembered() {
    let stub = Stub::serving(vec![
        refused("400 Bad Request", MP3_ONLY),
        audio("audio/mpeg", FRAMES),
        audio("audio/mpeg", FRAMES),
        audio("audio/mpeg", FRAMES),
    ])
    .await;
    let gateway = named();
    let engine = client(&stub.url, &gateway);
    assert_eq!(
        said(&engine).await.expect("a clip"),
        AudioClip::Encoded(FRAMES.to_vec())
    );
    assert_eq!(formats_asked(&stub), ["pcm", "mp3"]);
    assert_eq!(gateway.formats.of(MODEL), GatewayFormat::Mp3);

    said(&engine).await.expect("the next fragment");
    // A client built anew, as every `/tts` command builds one, on the
    // session's memo.
    said(&client(&stub.url, &gateway))
        .await
        .expect("the next command");
    assert_eq!(formats_asked(&stub), ["pcm", "mp3", "mp3", "mp3"]);
}

/// What is remembered is what the gateway did, not a fact about the model: a
/// model that stops taking the remembered format costs one refused request and
/// is remembered the new way.
#[tokio::test]
async fn a_remembered_format_that_is_refused_now_is_learned_again() {
    let stub = Stub::serving(vec![
        refused("400 Bad Request", MP3_ONLY),
        audio("audio/mpeg", FRAMES),
        refused("400 Bad Request", PCM_ONLY),
        audio("audio/pcm;rate=24000;channels=1", SAMPLES),
    ])
    .await;
    let gateway = named();
    let engine = client(&stub.url, &gateway);
    said(&engine).await.expect("learned: mp3");
    let clip = said(&engine).await.expect("learned again: pcm");
    assert!(matches!(clip, AudioClip::Pcm { .. }), "{clip:?}");
    assert_eq!(formats_asked(&stub), ["pcm", "mp3", "mp3", "pcm"]);
    assert_eq!(gateway.formats.of(MODEL), GatewayFormat::Pcm);
}

/// Refused both ways, the second refusal is the error, there is no third
/// request, and what was known before stays known.
#[tokio::test]
async fn a_model_that_refuses_both_formats_is_an_error_and_teaches_nothing() {
    let stub = Stub::serving(vec![
        refused("400 Bad Request", MP3_ONLY),
        audio("audio/mpeg", FRAMES),
        refused("400 Bad Request", PCM_ONLY),
        refused("400 Bad Request", MP3_ONLY),
    ])
    .await;
    let gateway = named();
    let engine = client(&stub.url, &gateway);
    said(&engine).await.expect("learned: mp3");
    let err = said(&engine).await.expect_err("refused both ways");
    assert!(
        err.to_string().contains("MiniMax TTS only supports"),
        "{err}"
    );
    assert_eq!(stub.requests().len(), 4);
    assert_eq!(gateway.formats.of(MODEL), GatewayFormat::Mp3);
}

/// A refusal about anything else is the answer: one request, and the error is
/// the gateway's sentence rather than the JSON around it.
#[tokio::test]
async fn a_refusal_about_anything_else_is_said_and_not_asked_again() {
    for (status, body, sentence) in [
        (
            "400 Bad Request",
            NO_VOICE,
            "An explicit voice is required for this TTS provider.",
        ),
        ("401 Unauthorized", WRONG_KEY, "User not found."),
        // A refusal that is not JSON is said as it came.
        ("502 Bad Gateway", "upstream is away", "upstream is away"),
        // The field's name under another status is not the format's refusal.
        (
            "422 Unprocessable Entity",
            r#"{"error":{"message":"response_format is fine, the input is not"}}"#,
            "response_format is fine, the input is not",
        ),
    ] {
        let stub = Stub::serving(vec![refused(status, body), audio("audio/mpeg", FRAMES)]).await;
        let err = said(&client(&stub.url, &named()))
            .await
            .expect_err("a refusal");
        let text = err.to_string();
        assert!(text.ends_with(sentence), "{status}: {text}");
        assert!(text.contains(&status[..3]), "{status}: {text}");
        assert_eq!(stub.requests().len(), 1, "{status}: asked once");
    }
}

/// What a label says the body is. Columns: the label (`_` for a space, `-`
/// for none at all), the format that was asked for, what the clip is, `|`, why
/// the row is here.
const LABELS: &str = "
    audio/pcm;rate=24000;channels=1     pcm  24000x1    | 17 of the gateway's models
    audio/pcm;rate=44100;channels=1     pcm  44100x1    | Fish Audio's four
    audio/pcm;rate=24000;channels=2     pcm  24000x2    | ByteDance answers in stereo
    audio/pcm;_rate=32000;_channels=1   pcm  32000x1    | spaces after the semicolons
    Audio/PCM;Rate=48000;Channels=2     pcm  48000x2    | capitals
    audio/pcm;channels=2;rate=22050     pcm  22050x2    | the other order
    audio/L16;codec=pcm;rate=24000      pcm  24000x1    | the same thing under its registered name
    audio/pcm                           pcm  24000x1    | no rate named: the fallback, one channel
    audio/pcm;rate=fast;channels=many   pcm  24000x1    | not numbers: the fallback
    audio/pcm;rate=24000;channels=1     mp3  24000x1    | raw samples whatever was asked
    audio/mpeg                          mp3  container  | MiniMax
    audio/mpeg                          pcm  container  | a container whatever was asked
    application/octet-stream            pcm  container  | not a label of raw samples
    -                                   pcm  24000x1    | no label: what was asked for
    -                                   mp3  container  | ...either way
";

/// The label decides what the body is, the body does not: raw samples that
/// begin like an MP3 frame are samples, and a container is a container
/// whatever was asked.
#[tokio::test]
async fn the_label_decides_what_the_body_is() {
    for row in LABELS.lines().filter(|l| !l.trim().is_empty()) {
        let c: Vec<&str> = row.split_whitespace().collect();
        let label = (c[0] != "-").then(|| c[0].replace('_', " "));
        // The body is the same in every row, and begins as an MP3 frame does.
        let answer = Step {
            status: "200 OK",
            label: label.map(|l| &*l.leak()),
            body: SAMPLES.to_vec(),
        };
        // Asked for MP3 second, after the refusal of the first request.
        let refusal = (c[1] == "mp3").then(|| refused("400 Bad Request", MP3_ONLY));
        let stub = Stub::serving(refusal.into_iter().chain([answer]).collect()).await;
        let clip = said(&client(&stub.url, &named())).await.expect("a clip");
        let shown = match clip {
            AudioClip::Pcm {
                sample_rate,
                channels,
                bytes,
            } if bytes == SAMPLES => format!("{sample_rate}x{channels}"),
            AudioClip::Encoded(bytes) if bytes == SAMPLES => "container".to_string(),
            other => format!("{other:?}"),
        };
        assert_eq!(shown, c[2], "{}", row.trim());
        assert_eq!(formats_asked(&stub).last().map(String::as_str), Some(c[1]));
    }
}

/// The provider-wide switch is off: the request says nothing about who sent
/// it. And a rate other than normal is sent, as to every other provider — a
/// model may ignore it, which is the model's to decide.
#[tokio::test]
async fn the_switch_turns_the_applications_name_off_and_a_rate_is_sent_as_it_is_set() {
    let stub = Stub::serving(vec![audio("audio/pcm;rate=24000;channels=1", SAMPLES)]).await;
    let gateway = GatewaySpeech {
        attribution: false,
        ..named()
    };
    let engine = OpenAiTts::gateway(
        format!("{}/", stub.url),
        "sk-or-stored".into(),
        "fish-audio/s1".into(),
        None,
        1.25,
        &gateway,
    );
    said(&engine).await.expect("a clip");
    let seen = stub.requests();
    let head = seen[0].to_ascii_lowercase();
    assert!(
        head.starts_with("post /v1/audio/speech "),
        "a trailing slash in the address is not doubled: {head}"
    );
    assert!(!head.contains("x-openrouter-title"), "{head}");
    assert!(!head.contains("http-referer"), "{head}");
    let body: serde_json::Value = serde_json::from_str(body_of(&seen[0])).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "model": "fish-audio/s1",
            "input": "hello",
            "response_format": "pcm",
            "speed": 1.25,
        }),
        "no voice is set, and none is sent"
    );
}

/// Speech that was stopped is not asked for.
#[tokio::test]
async fn a_cancelled_request_is_not_sent() {
    let stub = Stub::serving(vec![audio("audio/pcm;rate=24000;channels=1", SAMPLES)]).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = client(&stub.url, &named())
        .synthesize("hello", &cancel)
        .await
        .expect_err("cancelled");
    assert!(err.to_string().contains("cancelled"), "{err}");
    assert!(stub.requests().is_empty());
}

// ---------- the slot's settings, as the engines are built from them ----------

fn slot(url: &str) -> TtsSettings {
    TtsSettings {
        mode: TtsMode::OpenRouter,
        openrouter: TtsCloudSettings {
            model_name: Some(MODEL.into()),
            voice: Some("English_expressive_narrator".into()),
            url: Some(url.to_string()),
            // The cloud sections' instructions are OpenAI's and Gemini's; one
            // left in this section by hand must not travel.
            instructions: Some("speak slowly".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A model and a key are what the mode cannot speak without — there is no
/// default model to fall back on — and a voice is not: four of the gateway's
/// models list none.
#[test]
fn the_gateway_needs_a_model_and_a_key_and_no_voice() {
    let mut tts = slot("http://127.0.0.1:9/v1");
    let built = |tts: &TtsSettings, key: Option<&str>| {
        engines_from_config(tts, key.map(str::to_string), &named())
            .map(|(engine, user)| (engine.max_input_chars(), user.is_some()))
    };
    assert_eq!(built(&tts, Some("sk-or-stored")), Ok((2000, false)));
    assert_eq!(built(&tts, None), Err(TtsSetupError::ApiKey));
    assert_eq!(built(&tts, Some("  ")), Err(TtsSetupError::ApiKey));

    tts.openrouter.voice = None;
    assert_eq!(built(&tts, Some("sk-or-stored")), Ok((2000, false)));
    tts.openrouter.user_voice = Some("Calm_Woman".into());
    assert_eq!(
        built(&tts, Some("sk-or-stored")),
        Ok((2000, true)),
        "a voice of the user's is a second engine"
    );

    for blank in [None, Some("  ".to_string())] {
        tts.openrouter.model_name = blank;
        assert_eq!(built(&tts, Some("sk-or-stored")), Err(TtsSetupError::Model));
    }
    // Another mode's model is not this mode's.
    assert!(tts.openai.model_name.is_some());
}

/// The two engines of one command — the assistant's voice and the user's —
/// speak through one model, so what the first learned the second knows; and
/// what the section holds under `instructions` stays in the settings.
#[tokio::test]
async fn both_voices_share_what_the_session_learned() {
    let stub = Stub::serving(vec![
        refused("400 Bad Request", MP3_ONLY),
        audio("audio/mpeg", FRAMES),
        audio("audio/mpeg", FRAMES),
    ])
    .await;
    let mut tts = slot(&stub.url);
    tts.openrouter.user_voice = Some("Calm_Woman".into());
    let (assistant, user) =
        engines_from_config(&tts, Some("sk-or-stored".into()), &named()).expect("two engines");
    said(assistant.as_ref())
        .await
        .expect("the assistant's line");
    said(user.expect("the user's engine").as_ref())
        .await
        .expect("the user's line");
    assert_eq!(formats_asked(&stub), ["pcm", "mp3", "mp3"]);
    let voices: Vec<String> = stub
        .requests()
        .iter()
        .map(|r| {
            let body: serde_json::Value = serde_json::from_str(body_of(r)).unwrap();
            assert!(body.get("instructions").is_none(), "{body}");
            body["voice"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(
        voices,
        [
            "English_expressive_narrator",
            "English_expressive_narrator",
            "Calm_Woman"
        ]
    );
}
