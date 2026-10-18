//! What a client built with [`OpenAiClient::for_deepseek`] sends and reads, over
//! a real socket (docs/research/deepseek-mode.md §4.1): the thinking switch in
//! DeepSeek's spelling from the first request, a muted turn as the switch alone,
//! and the window and the images from the list in DeepSeek's words — which a
//! client not told it is DeepSeek leaves unread.
//!
//! The list below is DeepSeek's, measured 2026-10-19 and cut to the keys this
//! client reads.

use super::OpenAiClient;
use super::gateway_tests::{JSON, ONE_CHUNK, Route, SSE, Seen, muted, run, stub, turn};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{EngineBackend, VisionSupport};

const LIST: &str = r#"{"object":"list","data":[
    {"id":"deepseek-flash","name":"DeepSeek-V4.1-Flash","context_window":1048576,
     "max_output_tokens":393216,"input_modalities":["text","image"],"output_modalities":["text"],
     "effort":{"supported_levels":["low","high","max"],"default_level":"high"}},
    {"id":"deepseek-v4-pro","name":"DeepSeek-V4-Pro","context_window":1048576,
     "max_output_tokens":393216,"input_modalities":["text"],"output_modalities":["text"],
     "effort":{"supported_levels":["low","high","max"],"default_level":"high"}}
]}"#;

const CHAT: Route = ("/v1/chat/completions", "200 OK", SSE, ONE_CHUNK);
const MODELS: Route = ("/v1/models", "200 OK", JSON, LIST);

fn deepseek(url: String, model: &str) -> OpenAiClient {
    OpenAiClient::new(url)
        .with_api_key(Some("k".into()))
        .with_model(Some(model.into()))
        .for_deepseek()
}

/// The chat requests' bodies, in the order they were made.
fn bodies(seen: &[Seen]) -> Vec<serde_json::Value> {
    seen.iter()
        .filter(|s| s.path() == "/v1/chat/completions")
        .map(Seen::json)
        .collect()
}

/// Each way a turn says "do not reason" is the switch off and no effort: the
/// title's, the compaction roll's and impersonation's `muted`, an effort of
/// `none` alone — which wins over a switch left on — and a switch off alone.
/// Never a refused first request: the switch is typed from the start.
#[tokio::test]
async fn a_muted_turn_is_the_typed_switch_off_and_no_effort() {
    const ROUTES: &[Route] = &[MODELS, CHAT];
    let (url, server) = stub(5, ROUTES);
    let client = deepseek(url, "deepseek-flash");
    run(&client, muted()).await;
    for sampling in [
        SamplingConfig {
            reasoning_effort: Some(ReasoningEffort::None),
            ..Default::default()
        },
        SamplingConfig {
            reasoning_effort: Some(ReasoningEffort::None),
            thinking: Some(true),
            ..Default::default()
        },
        SamplingConfig {
            thinking: Some(false),
            ..Default::default()
        },
    ] {
        run(&client, turn(sampling)).await;
    }
    let seen = server.join().unwrap();
    let sent = bodies(&seen);
    assert_eq!(sent.len(), 4, "one request a turn: nothing was refused");
    for body in &sent {
        assert_eq!(body["thinking"], serde_json::json!({"type": "disabled"}));
        assert!(body.get("reasoning_effort").is_none(), "{body}");
    }
    assert!(
        seen.iter()
            .all(|s| matches!(s.path(), "/v1/chat/completions" | "/v1/models")),
        "no llama.cpp, Ollama or LM Studio route is asked: {:?}",
        seen.iter().map(Seen::path).collect::<Vec<_>>()
    );
}

/// A turn that reasons goes as it came: the switch typed on, and the effort
/// the settings chose — `medium` too, which DeepSeek itself reads as `high`
/// (docs/research/deepseek-mode.md §2.3, D2), where a nearest-tier rule here
/// would have sent `low`.
#[tokio::test]
async fn a_reasoning_turn_keeps_its_effort_and_types_the_switch_on() {
    const ROUTES: &[Route] = &[MODELS, CHAT];
    // Four turns, and the list once: a turn with the switch and no effort asks
    // the catalogue whether it lists a gateway's `reasoning` (it does not).
    let (url, server) = stub(5, ROUTES);
    let client = deepseek(url, "deepseek-flash");
    for effort in [
        ReasoningEffort::Medium,
        ReasoningEffort::XHigh,
        ReasoningEffort::Max,
    ] {
        run(
            &client,
            turn(SamplingConfig {
                reasoning_effort: Some(effort),
                thinking: Some(true),
                ..Default::default()
            }),
        )
        .await;
    }
    // No effort chosen: none is sent, and the model reasons at its default.
    run(
        &client,
        turn(SamplingConfig {
            thinking: Some(true),
            ..Default::default()
        }),
    )
    .await;
    let seen = server.join().unwrap();
    let sent = bodies(&seen);
    let efforts: Vec<_> = sent
        .iter()
        .map(|b| b["reasoning_effort"].as_str().unwrap_or("-"))
        .collect();
    assert_eq!(efforts, ["medium", "xhigh", "max", "-"]);
    for body in &sent {
        assert_eq!(body["thinking"], serde_json::json!({"type": "enabled"}));
    }
}

/// The window and the images, read from the list in DeepSeek's words: flash
/// takes images, pro does not, and both windows are 1 048 576. Nothing but the
/// list is asked.
#[tokio::test]
async fn the_window_and_the_images_are_read_from_deepseeks_list() {
    const ROUTES: &[Route] = &[MODELS];
    for (model, vision) in [
        ("deepseek-flash", VisionSupport::Supported),
        ("deepseek-v4-pro", VisionSupport::Unsupported),
    ] {
        let (url, server) = stub(1, ROUTES);
        let client = deepseek(url, model);
        let caps = client
            .model_capabilities()
            .await
            .expect("a catalogue answer");
        assert_eq!(caps.context_length, Some(1_048_576), "{model}");
        assert_eq!(caps.sampling_fields, None, "{model}: DeepSeek lists none");
        assert_eq!(client.vision().await, vision, "{model}");
        assert_eq!(
            client.context_budget().await,
            None,
            "{model}: the window is the catalogue's, and no server route is asked"
        );
        let seen = server.join().unwrap();
        assert_eq!(
            seen.iter().map(Seen::path).collect::<Vec<_>>(),
            ["/v1/models"],
            "{model}: the list once, nothing else"
        );
    }
}

/// The same list to a client **not** told it is DeepSeek — `external` at the
/// same URL — reads as it did: no window, so the endpoint does not count as
/// catalogued, and images unknown (deepseek-api.md, fork F3).
#[tokio::test]
async fn external_at_the_same_list_reads_nothing_new() {
    const ROUTES: &[Route] = &[MODELS];
    let (url, server) = stub(2, ROUTES);
    let client = OpenAiClient::new(url)
        .with_api_key(Some("k".into()))
        .with_model(Some("deepseek-v4-pro".into()));
    assert_eq!(client.model_capabilities().await, None);
    assert_eq!(client.vision().await, VisionSupport::Unknown);
    let seen = server.join().unwrap();
    assert_eq!(
        seen.iter().map(Seen::path).collect::<Vec<_>>(),
        ["/v1/models", "/props"],
        "external still asks llama.cpp's route"
    );
}

/// A window of another type, or one that does not fit, is silence; so is an
/// input list of anything but strings.
#[test]
fn deepseeks_keys_of_another_shape_are_silence() {
    let entry = |json: &str| -> super::wire::ModelEntry { serde_json::from_str(json).unwrap() };
    assert_eq!(
        entry(r#"{"id":"a","context_window":"big"}"#).deepseek_window(),
        None
    );
    assert_eq!(
        entry(r#"{"id":"a","context_window":0}"#).deepseek_window(),
        None
    );
    assert_eq!(
        entry(r#"{"id":"a","context_window":99999999999}"#).deepseek_window(),
        None
    );
    assert_eq!(
        entry(r#"{"id":"a","context_window":65536}"#).deepseek_window(),
        Some(65536)
    );
    assert_eq!(
        entry(r#"{"id":"a","input_modalities":[]}"#).takes_images_top_level(),
        None
    );
    assert_eq!(
        entry(r#"{"id":"a","input_modalities":["text",1]}"#).takes_images_top_level(),
        None
    );
    assert_eq!(
        entry(r#"{"id":"a","input_modalities":"text"}"#).takes_images_top_level(),
        None
    );
    assert_eq!(
        entry(r#"{"id":"a","input_modalities":["text"]}"#).takes_images_top_level(),
        Some(false)
    );
}
