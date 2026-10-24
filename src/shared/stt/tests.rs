use super::*;
use crate::shared::config::{SttCloudSettings, SttExternalSettings};
use crate::shared::http_stub::{Stub, json, refused};

/// A short real note: half a second of a tone, written as the bridge would.
fn note() -> Vec<u8> {
    let samples: Vec<i16> = (0..12_000)
        .map(|i| ((i as f64 / 24_000.0 * 440.0 * std::f64::consts::TAU).sin() * 8_000.0) as i16)
        .collect();
    voice_note::encode(&samples, 24_000).unwrap()
}

fn hearing(mode: SttMode, url: &str) -> Hearing {
    let section = SttCloudSettings {
        model_name: Some("listener-1".into()),
        api_key_env: None,
        url: Some(url.into()),
    };
    Hearing {
        stt: SttSettings {
            mode,
            openai: section.clone(),
            openrouter: section,
            external: SttExternalSettings {
                url: Some(url.into()),
                model_name: Some("whisper".into()),
                api_key_env: None,
            },
            language: None,
        },
        key: Some("sk-test".into()),
        attribution: true,
    }
}

fn client(h: &Hearing) -> Transcriber {
    transcriber(h, Arc::new(WavMemo::default())).unwrap()
}

#[tokio::test]
async fn openai_gets_the_note_as_it_came_in_a_form() {
    let stub = Stub::serving(vec![json(r#"{"text":" Remind me at nine. "}"#)]).await;
    let got = client(&hearing(SttMode::OpenAi, &stub.url))
        .transcribe(&note())
        .await;
    assert_eq!(got.as_deref(), Ok("Remind me at nine."));
    let req = &stub.requests()[0];
    assert!(
        req.to_ascii_lowercase()
            .starts_with("post /v1/audio/transcriptions")
    );
    assert!(
        req.contains("Bearer sk-test") || req.contains("bearer sk-test"),
        "{req:.300}"
    );
    assert!(req.contains("filename=\"voice.ogg\""), "the file is named");
    assert!(req.contains("audio/ogg"));
    assert!(req.contains("listener-1"), "the model");
    assert!(req.contains("OggS"), "the note itself, untouched");
    assert!(
        !req.contains("name=\"language\""),
        "no hint unless one is set"
    );
    assert!(
        !req.to_ascii_lowercase().contains("x-openrouter-title"),
        "attribution is the gateway's"
    );
}

#[tokio::test]
async fn the_gateway_gets_the_note_in_json_with_its_attribution_and_language() {
    let stub = Stub::serving(vec![json(r#"{"text":"привет","usage":{"seconds":1}}"#)]).await;
    let mut h = hearing(SttMode::OpenRouter, &stub.url);
    h.stt.language = Some(" ru ".into());
    let got = client(&h).transcribe(&note()).await;
    assert_eq!(got.as_deref(), Ok("привет"));
    let body = &stub.bodies()[0];
    assert_eq!(body["model"], "listener-1");
    assert_eq!(body["input_audio"]["format"], "ogg");
    assert_eq!(body["language"], "ru");
    let data = body["input_audio"]["data"].as_str().unwrap();
    let sent = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    assert_eq!(sent, note(), "the note as it came");
    assert!(
        stub.requests()[0]
            .to_ascii_lowercase()
            .contains("x-openrouter-title: mindfork")
    );
}

#[tokio::test]
async fn a_server_that_refuses_the_note_hears_it_as_wav_once_and_is_remembered() {
    let stub = Stub::serving(vec![
        refused(
            "400 Bad Request",
            r#"{"error":{"message":"Invalid file format"}}"#,
        ),
        json(r#"{"text":"first"}"#),
        json(r#"{"text":"second"}"#),
    ])
    .await;
    let h = hearing(SttMode::External, &stub.url);
    let memo = Arc::new(WavMemo::default());
    let first = transcriber(&h, memo.clone())
        .unwrap()
        .transcribe(&note())
        .await;
    assert_eq!(first.as_deref(), Ok("first"));
    // A client of the next note, sharing the session's memo, goes straight to WAV.
    let second = transcriber(&h, memo).unwrap().transcribe(&note()).await;
    assert_eq!(second.as_deref(), Ok("second"));
    let reqs = stub.requests();
    assert_eq!(reqs.len(), 3, "the refused shape is asked once a session");
    assert!(reqs[0].contains("voice.ogg"));
    assert!(reqs[1].contains("voice.wav") && reqs[1].contains("RIFF"));
    assert!(reqs[2].contains("voice.wav"));
}

#[tokio::test]
async fn a_refusal_that_is_not_the_format_is_told_in_the_servers_words() {
    let stub = Stub::serving(vec![
        refused(
            "401 Unauthorized",
            r#"{"error":{"message":"Incorrect API key provided"}}"#,
        ),
        refused(
            "400 Bad Request",
            r#"{"error":{"message":"model not found"}}"#,
        ),
        refused(
            "400 Bad Request",
            r#"{"error":{"message":"model not found"}}"#,
        ),
    ])
    .await;
    let c = client(&hearing(SttMode::OpenAi, &stub.url));
    assert_eq!(
        c.transcribe(&note()).await,
        Err(SttError::Refused {
            status: 401,
            said: "Incorrect API key provided".into()
        }),
        "a key is not asked again as WAV"
    );
    // A 400 that WAV does not cure is the first refusal, as it was said.
    assert_eq!(
        c.transcribe(&note()).await,
        Err(SttError::Refused {
            status: 400,
            said: "model not found".into()
        })
    );
    assert_eq!(stub.requests().len(), 3);
}

#[tokio::test]
async fn an_answer_with_no_words_is_nothing_heard() {
    let stub = Stub::serving(vec![json(r#"{"text":"  "}"#)]).await;
    let got = client(&hearing(SttMode::OpenAi, &stub.url))
        .transcribe(&note())
        .await;
    assert_eq!(got, Err(SttError::Nothing));
}

#[test]
fn what_is_missing_is_named_before_anything_is_sent() {
    let memo = || Arc::new(WavMemo::default());
    let mut h = Hearing {
        key: None,
        ..Hearing::default()
    };
    // The default mode is OpenAI with its default model: only the key lacks —
    // unless an environment variable it names has one.
    h.stt.openai.api_key_env = Some("MINDFORK_STT_TEST_UNSET_VARIABLE".into());
    assert!(matches!(
        transcriber(&h, memo()),
        Err(SttSetupError::ApiKey)
    ));
    h.stt.mode = SttMode::OpenRouter;
    assert!(
        matches!(transcriber(&h, memo()), Err(SttSetupError::Model)),
        "the gateway has no default model"
    );
    h.stt.mode = SttMode::External;
    assert!(matches!(transcriber(&h, memo()), Err(SttSetupError::Url)));
    h.stt.external.url = Some("http://127.0.0.1:1/v1".into());
    assert!(
        transcriber(&h, memo()).is_ok(),
        "a server of one's own needs no key and may serve one model"
    );
}

#[test]
fn a_plain_text_answer_is_the_words() {
    assert_eq!(
        words_of("text/plain; charset=utf-8", "hello\n").as_deref(),
        Ok("hello\n")
    );
    assert!(matches!(
        words_of("text/html", "<html>"),
        Err(SttError::Network(_))
    ));
}
