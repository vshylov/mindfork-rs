//! What a client built with [`OpenAiClient::for_xai`] asks, over a real socket
//! (docs/research/effort-tiers.md §9): the model's own entry by name, and the
//! effort that entry's list turns a wish into.
//!
//! The entries below are xAI's, measured 2026-10-01 and cut to the keys this
//! client reads.

use super::OpenAiClient;
use super::gateway_tests::{JSON, ONE_CHUNK, Route, SSE, Seen, muted, run, stub, turn};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::EngineBackend;
use crate::shared::api::sse_stub::{self, BAD_REQUEST, Canned};

const GROK_4_3: &str = r#"{"id":"grok-4.3","aliases":["grok-4.3-latest"],"context_length":1000000,
    "capabilities":{"reasoning_effort":["none","low","medium","high","xhigh"],"default_reasoning_effort":"low"}}"#;
const GROK_4_7: &str = r#"{"id":"grok-4.7","aliases":[],"context_length":500000,
    "capabilities":{"reasoning_effort":["low","medium","high","xhigh"],"default_reasoning_effort":"high"}}"#;
/// One of the three chat models that publish no list — and refuse the
/// parameter, whatever its value.
const GROK_BUILD: &str =
    r#"{"id":"grok-build-0.1","aliases":["grok-code-fast-1"],"context_length":256000}"#;
const NO_PARAMETER: &str = r#"{"code":"invalid-argument","error":"Model grok-build-0.1 does not support parameter reasoningEffort."}"#;
const INVALID_EFFORT: &str = r#"{"code":"invalid-argument","error":"Invalid reasoning effort."}"#;

const CHAT: Route = ("/v1/chat/completions", "200 OK", SSE, ONE_CHUNK);

fn xai(url: String, model: &str) -> OpenAiClient {
    OpenAiClient::new(url)
        .with_api_key(Some("k".into()))
        .with_model(Some(model.into()))
        .for_xai()
}

fn at(effort: ReasoningEffort) -> SamplingConfig {
    SamplingConfig {
        reasoning_effort: Some(effort),
        ..Default::default()
    }
}

/// The `reasoning_effort` of every chat request, in the order they were made.
fn efforts(seen: &[Seen]) -> Vec<Option<String>> {
    seen.iter()
        .filter(|s| s.path() == "/v1/chat/completions")
        .map(|s| s.json()["reasoning_effort"].as_str().map(str::to_string))
        .collect()
}

/// Every path but the chat's: what the client asked about the model.
fn asked_about_the_model(seen: &[Seen]) -> Vec<&str> {
    seen.iter()
        .map(Seen::path)
        .filter(|path| *path != "/v1/chat/completions")
        .collect()
}

fn words(sent: &[Option<String>]) -> Vec<&str> {
    sent.iter()
        .map(|e| e.as_deref().unwrap_or("-"))
        .collect::<Vec<_>>()
}

/// `grok-4.3` lists `none`: the title's turn is sent it, where it used to go
/// out with no effort and reason at the model's default. A value outside the
/// list is the nearest in it, and the entry is asked once for all of them.
#[tokio::test]
async fn a_listed_effort_goes_as_it_is_and_any_other_as_the_nearest_listed() {
    const ROUTES: &[Route] = &[("/v1/models/grok-4.3", "200 OK", JSON, GROK_4_3), CHAT];
    let (url, server) = stub(6, ROUTES);
    let client = xai(url, "grok-4.3");
    run(&client, muted()).await;
    for effort in [
        ReasoningEffort::Max,
        ReasoningEffort::Minimal,
        ReasoningEffort::High,
    ] {
        run(&client, turn(at(effort))).await;
    }
    // Nothing asked: nothing sent, whatever the list holds.
    run(&client, turn(SamplingConfig::default())).await;

    let seen = server.join().unwrap();
    assert_eq!(asked_about_the_model(&seen), ["/v1/models/grok-4.3"]);
    assert_eq!(
        words(&efforts(&seen)),
        ["none", "xhigh", "low", "high", "-"]
    );
    assert_eq!(client.learned("none"), None, "a listed value is no lesson");
    assert_eq!(client.learned("max"), Some(Some("xhigh")));
    assert_eq!(client.learned("minimal"), Some(Some("low")));
}

/// `grok-4.7` lists no `none`, and its default is `high`: each of the three
/// ways a request says "do not reason" asks for the lowest depth it lists.
#[tokio::test]
async fn a_muted_turn_asks_for_the_lowest_listed_where_none_is_not() {
    const ROUTES: &[Route] = &[("/v1/models/grok-4.7", "200 OK", JSON, GROK_4_7), CHAT];
    let (url, server) = stub(5, ROUTES);
    let client = xai(url, "grok-4.7");
    run(&client, muted()).await;
    for sampling in [
        at(ReasoningEffort::None),
        SamplingConfig {
            thinking: Some(false),
            ..Default::default()
        },
        SamplingConfig {
            reasoning_budget: Some(0),
            ..Default::default()
        },
    ] {
        run(&client, turn(sampling)).await;
    }
    let seen = server.join().unwrap();
    assert_eq!(words(&efforts(&seen)), ["low", "low", "low", "low"]);
    assert_eq!(client.learned("none"), Some(Some("low")));
}

/// A name the endpoint does not know has no list: the answers that hold on
/// every model that publishes one stand in, the `404` is kept, and nothing is
/// learned from a list that was never read.
#[tokio::test]
async fn with_no_entry_the_measured_answers_stand() {
    const ROUTES: &[Route] = &[CHAT];
    let (url, server) = stub(5, ROUTES);
    let client = xai(url, "grok-x");
    run(&client, muted()).await;
    for effort in [
        ReasoningEffort::Max,
        ReasoningEffort::Minimal,
        ReasoningEffort::XHigh,
    ] {
        run(&client, turn(at(effort))).await;
    }
    let seen = server.join().unwrap();
    assert_eq!(asked_about_the_model(&seen), ["/v1/models/grok-x"]);
    assert_eq!(words(&efforts(&seen)), ["-", "xhigh", "minimal", "xhigh"]);
    assert_eq!(client.learned("none"), None);
    assert_eq!(client.learned("max"), None);
}

/// An entry that could not be had for now is not filed as "no list": the next
/// turn asks again, and meanwhile the turn goes out under the measured answers.
#[tokio::test]
async fn an_entry_that_is_unavailable_is_asked_again() {
    const ROUTES: &[Route] = &[
        (
            "/v1/models/grok-4.3",
            "503 Service Unavailable",
            JSON,
            r#"{"error":"try later"}"#,
        ),
        CHAT,
    ];
    let (url, server) = stub(4, ROUTES);
    let client = xai(url, "grok-4.3");
    run(&client, muted()).await;
    run(&client, muted()).await;
    let seen = server.join().unwrap();
    assert_eq!(
        asked_about_the_model(&seen),
        ["/v1/models/grok-4.3", "/v1/models/grok-4.3"]
    );
    assert_eq!(words(&efforts(&seen)), ["-", "-"]);
}

/// An alias is not an id in the list, so a scan of the list found nothing for
/// it — no window, no efforts. The model's own route answers for it.
#[tokio::test]
async fn an_alias_is_answered_by_the_models_own_route() {
    const ROUTES: &[Route] = &[("/v1/models/grok-latest", "200 OK", JSON, GROK_4_7)];
    let (url, server) = stub(1, ROUTES);
    let client = xai(url, "grok-latest");
    let caps = client.model_capabilities().await.expect("an entry");
    assert_eq!(caps.context_length, Some(500_000));
    // Asked again: the answer is the client's for its lifetime.
    assert!(client.model_capabilities().await.is_some());
    assert_eq!(
        asked_about_the_model(&server.join().unwrap()),
        ["/v1/models/grok-latest"]
    );
}

/// A client that was not told the server is xAI asks as chosen, and asks
/// nothing about the model first.
#[tokio::test]
async fn a_client_not_told_it_is_xai_asks_as_chosen() {
    const ROUTES: &[Route] = &[CHAT];
    let (url, server) = stub(1, ROUTES);
    let client = OpenAiClient::new(url)
        .with_api_key(Some("k".into()))
        .with_model(Some("grok-4.7".into()));
    run(&client, turn(at(ReasoningEffort::Max))).await;
    let seen = server.join().unwrap();
    assert!(asked_about_the_model(&seen).is_empty(), "{seen:?}");
    assert_eq!(words(&efforts(&seen)), ["max"]);
}

const SSE_OK: Canned = ("200 OK", SSE, ONE_CHUNK);

/// The `reasoning_effort` of a request body [`sse_stub::serve_in_turn`] saw.
fn effort_in(body: &str) -> Option<String> {
    let json: serde_json::Value =
        serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body:?}"));
    json["reasoning_effort"].as_str().map(str::to_string)
}

/// A model that publishes no list refuses the parameter itself. The turn is
/// asked once more without it, and from then on the client starts there —
/// where every turn with an effort chosen in the settings used to be the `400`.
#[tokio::test]
async fn a_model_that_refuses_the_parameter_is_asked_without_it() {
    const ANSWERS: &[Canned] = &[
        ("200 OK", JSON, GROK_BUILD),
        (BAD_REQUEST, JSON, NO_PARAMETER),
        SSE_OK,
        SSE_OK,
        SSE_OK,
    ];
    let (url, server) = sse_stub::serve_in_turn(ANSWERS);
    let client = xai(url, "grok-build-0.1");
    run(&client, turn(at(ReasoningEffort::High))).await;
    assert_eq!(client.learned("high"), Some(None));
    run(&client, turn(at(ReasoningEffort::High))).await;
    run(&client, muted()).await;

    let bodies = server.join().unwrap();
    let sent: Vec<Option<String>> = bodies[1..].iter().map(|b| effort_in(b)).collect();
    assert_eq!(
        words(&sent),
        ["high", "-", "-", "-"],
        "the refused turn, the same turn again, the next turn, a muted one"
    );
}

/// A refusal read wrongly costs one round trip and teaches nothing: the second
/// error is the turn's, and the next turn asks as chosen again.
#[tokio::test]
async fn a_parameter_refused_twice_is_the_turns_error_and_no_lesson() {
    const ANSWERS: &[Canned] = &[
        ("200 OK", JSON, GROK_BUILD),
        (BAD_REQUEST, JSON, NO_PARAMETER),
        (BAD_REQUEST, JSON, NO_PARAMETER),
    ];
    let (url, server) = sse_stub::serve_in_turn(ANSWERS);
    let client = xai(url, "grok-build-0.1");
    let refused = client
        .chat_stream(turn(at(ReasoningEffort::High)), Default::default())
        .await
        .err()
        .expect("the second refusal surfaces");
    assert!(refused.to_string().contains("reasoningEffort"), "{refused}");
    assert_eq!(client.learned("high"), None);
    assert_eq!(server.join().unwrap().len(), 3);
}

/// A refused **value** names nothing to send in its place — xAI's own words for
/// `max` — so it is reported as it came, after one request.
#[tokio::test]
async fn a_refused_value_is_reported_as_it_came() {
    const ANSWERS: &[Canned] = &[
        ("200 OK", JSON, GROK_BUILD),
        (BAD_REQUEST, JSON, INVALID_EFFORT),
    ];
    let (url, server) = sse_stub::serve_in_turn(ANSWERS);
    let client = xai(url, "grok-build-0.1");
    let refused = client
        .chat_stream(turn(at(ReasoningEffort::High)), Default::default())
        .await
        .err()
        .expect("a refusal");
    assert!(
        refused.to_string().contains("Invalid reasoning effort"),
        "a second request would have met a closed port instead: {refused}"
    );
    assert_eq!(server.join().unwrap().len(), 2);
}
