//! A video through the OpenRouter gateway: what is sent, and what of the answer
//! is believed (docs/research/openrouter-mode.md §4.5, fork F10, §14).
//!
//! Over a real socket: what is asserted is what left the machine. The bodies
//! the stub answers with are the gateway's own, measured on 2026-09-29 and cut
//! to the keys this client reads.

use tokio_util::sync::CancellationToken;

use super::gateway::GatewayVideo;
use super::{VideoAnswer, VideoConfig, VideoRequest, VideoUnderstanding};
use crate::shared::config::{MediaResolution, VideoProvider};
use crate::shared::http_stub::{Step, Stub, json, refused};

const MODEL: &str = "google/gemini-3.5-flash-lite";
const VIDEO: &str = "https://www.youtube.com/watch?v=IwZVXmQdX1E";

/// The gateway's entry for the model: it must reason, and lists its efforts
/// highest first.
const ENTRY: &str = r#"{"data":{"id":"google/gemini-3.5-flash-lite","context_length":1048576,
    "architecture":{"input_modalities":["text","image","video","file","audio"]},
    "reasoning":{"mandatory":true,"default_enabled":true,
                 "supported_efforts":["high","medium","low","minimal"],"default_effort":"minimal"}}}"#;
/// An entry that lists no efforts at all.
const ENTRY_WITHOUT_EFFORTS: &str = r#"{"data":{"id":"vendor/plain","context_length":32768}}"#;

/// The video read: 4422 video tokens and 1672 of its sound among 6119.
const WATCHED: &str = r#"{"id":"gen-1","model":"google/gemini-3.5-flash-lite","provider":"Google",
    "choices":[{"index":0,"finish_reason":"stop","native_finish_reason":"STOP",
        "message":{"role":"assistant","content":"Now, bound for the moon.","reasoning":null}}],
    "usage":{"prompt_tokens":6119,"completion_tokens":19,"cost":0.0018832,
        "prompt_tokens_details":{"cached_tokens":0,"audio_tokens":1672,"video_tokens":4422}}}"#;
/// The same video with one more parameter in its address: a `200`, a
/// description, and no video in it — the page was read instead.
const A_PAGE_READ: &str = r#"{"id":"gen-2","provider":"Google",
    "choices":[{"index":0,"finish_reason":"stop",
        "message":{"role":"assistant","content":"The video shows a NASA video detailing the plans for a moon base."}}],
    "usage":{"prompt_tokens":551337,"completion_tokens":21,"cost":0.1654436,
        "prompt_tokens_details":{"cached_tokens":0,"audio_tokens":0,"video_tokens":0}}}"#;
/// An answer cut at the ceiling.
const CUT: &str = r#"{"choices":[{"finish_reason":"length",
        "message":{"content":"[0:00] This is the moment"}}],
    "usage":{"prompt_tokens":6119,"prompt_tokens_details":{"video_tokens":4422}}}"#;
/// The ceiling spent before a word was said.
const NOTHING_SAID: &str = r#"{"choices":[{"finish_reason":"length","message":{"content":""}}],
    "usage":{"prompt_tokens":6119,"prompt_tokens_details":{"video_tokens":6094}}}"#;
/// A refusal that came as a `200`: no `choices`, an `error` with its own code.
const REFUSED_AS_200: &str = r#"{"id":"gen-3","error":{"message":"Invalid image URL: content_type='text/html; charset=utf-8' for url='https://www.youtube.com/watch?v=IwZVXmQdX1E'","code":400,"metadata":{"error_type":"invalid_request"}}}"#;
/// A video that is not there: the gateway's `502` around the provider's `403`.
const NOT_THERE: &str = r#"{"error":{"message":"Provider returned error","code":502,"metadata":{"raw":"{\n  \"error\": {\n    \"code\": 403,\n    \"message\": \"The caller does not have permission\",\n    \"status\": \"PERMISSION_DENIED\"\n  }\n}\n","provider_name":"Google AI Studio"}}}"#;
const NO_VIDEO_INPUT: &str = r#"{"error":{"message":"No endpoints found that support input video","code":404,"metadata":{"failed_routing_step":"Filter by Input Video Support"}}}"#;
const WRONG_KEY: &str = r#"{"error":{"message":"User not found.","code":401}}"#;

fn config(url: &str, attribution: bool) -> VideoConfig {
    VideoConfig {
        provider: VideoProvider::OpenRouter,
        attribution,
        model: format!(" {MODEL} "),
        base_url: format!("{url}/"),
        api_key: "sk-or-stored".into(),
        media_resolution: MediaResolution::Medium,
        max_minutes: 30,
    }
}

fn request() -> VideoRequest {
    VideoRequest {
        url: VIDEO.into(),
        prompt: "describe it".into(),
        start_secs: Some(40),
        end_secs: Some(80),
        max_output_tokens: 2000,
    }
}

async fn watched(client: &GatewayVideo) -> anyhow::Result<VideoAnswer> {
    client.describe(request(), &CancellationToken::new()).await
}

/// What the client is asked for a video: the model's entry, then the video —
/// both with the key and the application's name. The request is the prompt and
/// the link and a ceiling; the lowest effort the entry lists; and nothing of
/// what the gateway does not carry — no bounds, though the request had them, no
/// resolution, no `processing`.
#[tokio::test]
async fn the_link_is_a_part_of_a_chat_request_at_the_lowest_effort_listed() {
    let stub = Stub::serving(vec![json(ENTRY), json(WATCHED)]).await;
    let client = GatewayVideo::new(config(&stub.url, true));
    let answer = watched(&client).await.expect("an answer");
    assert_eq!(
        answer,
        VideoAnswer {
            text: "Now, bound for the moon.".into(),
            truncated: false,
        }
    );
    assert!(!client.reads_segments(), "the whole video is what is read");

    assert_eq!(
        stub.asked(),
        [
            format!("get /v1/model/{MODEL} http/1.1"),
            "post /v1/chat/completions http/1.1".to_string()
        ],
        "a trailing slash in the address is not doubled, and the slug is trimmed"
    );
    for head in stub.requests().iter().map(|r| r.to_ascii_lowercase()) {
        assert!(
            head.contains("authorization: bearer sk-or-stored"),
            "{head}"
        );
        assert!(head.contains("x-openrouter-title: mindfork"), "{head}");
        assert!(head.contains("http-referer: https://mindfork.io"), "{head}");
    }
    assert_eq!(
        stub.bodies(),
        [serde_json::json!({
            "model": MODEL,
            "max_tokens": 2000,
            "reasoning": {"effort": "minimal"},
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "describe it"},
                {"type": "video_url", "video_url": {"url": VIDEO}},
            ]}],
        })]
    );
}

/// The entry is asked once for as long as the client lives — a video after the
/// first costs one request — and a model that lists no efforts, or that the
/// gateway does not know, is asked nothing about reasoning.
#[tokio::test]
async fn the_models_entry_is_asked_once_and_silence_about_efforts_is_silence() {
    let stub = Stub::serving(vec![json(ENTRY), json(WATCHED), json(WATCHED)]).await;
    let client = GatewayVideo::new(config(&stub.url, false));
    watched(&client).await.expect("the first video");
    watched(&client).await.expect("the second");
    let asked = stub.asked();
    assert_eq!(asked.len(), 3, "{asked:#?}");
    assert!(asked[2].starts_with("post "), "{asked:#?}");
    for head in stub.requests().iter().map(|r| r.to_ascii_lowercase()) {
        assert!(
            !head.contains("x-openrouter-title"),
            "the switch is off: {head}"
        );
        assert!(!head.contains("http-referer"), "{head}");
    }

    for entry in [
        json(ENTRY_WITHOUT_EFFORTS),
        refused("404 Not Found", r#"{"error":{"message":"no such model"}}"#),
        json("not an entry"),
    ] {
        let stub = Stub::serving(vec![entry, json(WATCHED), json(WATCHED)]).await;
        let client = GatewayVideo::new(config(&stub.url, true));
        watched(&client).await.expect("an answer");
        watched(&client).await.expect("and another");
        let bodies = stub.bodies();
        let sent = bodies.iter().filter(|b| b.get("messages").is_some());
        assert_eq!(sent.clone().count(), 2);
        assert!(
            sent.clone().all(|b| b.get("reasoning").is_none()),
            "{bodies:#?}"
        );
        assert_eq!(stub.asked().len(), 3, "what the gateway answered is kept");
    }
}

/// An outage of the catalogue — or a limit reached on it — is not an answer
/// about the model: the video is asked for without a word about reasoning,
/// and the entry is asked again with the next one.
#[tokio::test]
async fn an_outage_of_the_catalogue_is_asked_about_again() {
    for status in ["503 Service Unavailable", "429 Too Many Requests"] {
        let later = refused(status, r#"{"error":{"message":"try later"}}"#);
        let script = vec![later, json(WATCHED), json(ENTRY), json(WATCHED)];
        let stub = Stub::serving(script).await;
        let client = GatewayVideo::new(config(&stub.url, true));
        watched(&client).await.expect("watched during the outage");
        watched(&client).await.expect("and after it");
        let asked: Vec<String> = stub.asked().iter().map(|a| a[..4].to_string()).collect();
        assert_eq!(asked, ["get ", "post", "get ", "post"], "{status}");
        let efforts: Vec<Option<String>> = stub
            .bodies()
            .iter()
            .filter(|b| b.get("messages").is_some())
            .map(|b| b.pointer("/reasoning/effort")?.as_str().map(str::to_string))
            .collect();
        assert_eq!(efforts, [None, Some("minimal".to_string())], "{status}");
    }
}

/// **An answer with no video tokens is an error, whatever its text says.** The
/// body is the gateway's own answer about a link it did not read as a video: a
/// `200`, a sentence about NASA and the moon, 551 337 prompt tokens and not one
/// of them the video's. None of the sentence reaches the caller.
#[tokio::test]
async fn an_answer_with_no_video_tokens_is_an_error_whatever_it_says() {
    const NO_USAGE: &str = r#"{"choices":[{"finish_reason":"stop","message":{"content":"A plausible description."}}]}"#;
    const NO_DETAILS: &str = r#"{"choices":[{"finish_reason":"stop","message":{"content":"A plausible description."}}],
        "usage":{"prompt_tokens":77}}"#;
    for (body, counted) in [
        (A_PAGE_READ, "551337"),
        (NO_USAGE, "the 0 prompt"),
        (NO_DETAILS, "77"),
    ] {
        let stub = Stub::serving(vec![json(ENTRY), json(body)]).await;
        let err = watched(&GatewayVideo::new(config(&stub.url, true)))
            .await
            .expect_err("no video was read");
        let text = err.to_string();
        assert!(text.contains("without reading the video"), "{text}");
        assert!(text.contains(counted), "the count is said: {text}");
        assert!(
            !text.contains("NASA") && !text.contains("plausible"),
            "{text}"
        );
    }
}

/// What can be trusted is read first. A refusal is a refusal under any status
/// — one arrives as a `200` — and is said in the gateway's words, with the
/// provider's where the gateway passed them on. Columns: the status, the body,
/// what the error has to say.
#[tokio::test]
async fn a_refusal_is_read_from_the_body_before_the_status_is_believed() {
    let table: [(&'static str, &'static str, &[&str]); 6] = [
        ("200 OK", REFUSED_AS_200, &["200", "Invalid image URL"]),
        (
            "502 Bad Gateway",
            NOT_THERE,
            &[
                "502",
                "Provider returned error: The caller does not have permission",
            ],
        ),
        (
            "404 Not Found",
            NO_VIDEO_INPUT,
            &["404", "No endpoints found that support input video"],
        ),
        ("401 Unauthorized", WRONG_KEY, &["401", "User not found."]),
        (
            "502 Bad Gateway",
            "upstream is away",
            &["502", "upstream is away"],
        ),
        ("500 Internal Server Error", "{}", &["500"]),
    ];
    for (status, body, said) in table {
        let step = Step {
            status,
            ..json(body)
        };
        let stub = Stub::serving(vec![json(ENTRY), step]).await;
        let err = watched(&GatewayVideo::new(config(&stub.url, true)))
            .await
            .expect_err("a refusal");
        let text = err.to_string();
        for part in said {
            assert!(text.contains(part), "{status}: {text}");
        }
        assert!(
            !text.contains("metadata"),
            "the sentence, not the JSON: {text}"
        );
        assert_eq!(stub.asked().len(), 2, "{status}: not asked again");
    }
}

/// An answer cut at the ceiling is handed over and marked — a transcript that
/// stops looks whole — and a ceiling spent before a word was said is an error
/// that names what ended it.
#[tokio::test]
async fn a_cut_answer_is_marked_and_an_empty_one_is_an_error() {
    let stub = Stub::serving(vec![json(ENTRY), json(CUT), json(NOTHING_SAID)]).await;
    let client = GatewayVideo::new(config(&stub.url, true));
    let cut = watched(&client).await.expect("a prefix is an answer");
    assert!(cut.truncated);
    assert_eq!(cut.text, "[0:00] This is the moment");
    let err = watched(&client).await.expect_err("nothing was said");
    assert!(err.to_string().contains("finish reason: length"), "{err}");
}

/// A video that was stopped is not asked for.
#[tokio::test]
async fn a_cancelled_request_is_not_sent() {
    let stub = Stub::serving(vec![json(ENTRY), json(WATCHED)]).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = GatewayVideo::new(config(&stub.url, true))
        .describe(request(), &cancel)
        .await
        .expect_err("cancelled");
    assert!(err.to_string().contains("cancelled"), "{err}");
    assert!(stub.asked().is_empty(), "{:?}", stub.asked());
}
