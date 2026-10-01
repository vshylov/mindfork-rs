//! Live smokes of the Anthropic client's effort levels against the real API.
//! `#[ignore]` — not in CI; silently skipped without their variables.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out, and smokes that never run in CI would otherwise count as
//! production code nothing covers (docs/journal/quality.md).
//!
//! Run both arms:
//! `MINDFORK_ANTHROPIC_KEY=… MINDFORK_LIVE_NO_EFFORT_XHIGH_MODEL=claude-sonnet-4-6
//! MINDFORK_LIVE_EFFORT_XHIGH_MODEL=claude-sonnet-5-5 cargo test
//! anthropic::live_tests -- --ignored --nocapture --test-threads=1`.

use futures_util::StreamExt;

use super::client::AnthropicClient;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason,
};

/// A client for the model `var` declares, or `None` when the key or the
/// declaration is missing.
fn client_for(var: &str) -> Option<(String, AnthropicClient)> {
    let Ok(key) = std::env::var("MINDFORK_ANTHROPIC_KEY") else {
        eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
        return None;
    };
    let Ok(model) = std::env::var(var) else {
        eprintln!("skip: {var} not set");
        return None;
    };
    let client = AnthropicClient::new("https://api.anthropic.com", key, model.clone());
    Some((model, client))
}

/// One thinking turn at `effort`: the reply and how the stream ended. A refused
/// request fails the smoke here, with the provider's words.
async fn turn_at(
    client: &AnthropicClient,
    effort: ReasoningEffort,
) -> (String, Option<FinishReason>) {
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user("Reply with exactly: pong")],
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            thinking: Some(true),
            reasoning_effort: Some(effort),
            ..Default::default()
        },
        tools: vec![],
    };
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .expect("the turn must not be refused");
    let (mut text, mut finish) = (String::new(), None);
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
            ChatChunk::Finished(r) => {
                finish = Some(r);
                break;
            }
            _ => {}
        }
    }
    (text, finish)
}

/// A level the API has and the model does not —
/// `MINDFORK_LIVE_NO_EFFORT_XHIGH_MODEL` declares a model without `xhigh`
/// (`claude-sonnet-4-6`, `claude-opus-4-6`; measured 2026-10-01). The settings'
/// `xhigh` is sent as `xhigh` now, so on such a model the turn meets a `400`
/// that lists what it takes and is asked again at the nearest.
///
/// Declared rather than guessed, so the smoke **fails** rather than passes on a
/// model that takes the level.
#[tokio::test]
#[ignore = "requires MINDFORK_ANTHROPIC_KEY + MINDFORK_LIVE_NO_EFFORT_XHIGH_MODEL (a Claude model with adaptive thinking and no \"xhigh\" level)"]
async fn a_level_the_model_lacks_becomes_the_nearest_it_lists() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_NO_EFFORT_XHIGH_MODEL") else {
        return;
    };
    for nth in 1..=2 {
        let (text, finish) = turn_at(&client, ReasoningEffort::XHigh).await;
        println!("{model} turn {nth}: finish={finish:?} reply={text}");
        assert!(
            finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
            "{model}: the turn must complete: finish={finish:?} text={text:?}"
        );
    }
    let learned = client.learned("xhigh");
    println!("{model}: asked instead of \"xhigh\": {learned:?}");
    assert!(
        matches!(learned, Some(Some(_))),
        "{model} was declared to refuse \"xhigh\" and to list what it takes: {learned:?}"
    );
}

/// The control arm, and the other half of the change: a model that has both
/// top levels — `MINDFORK_LIVE_EFFORT_XHIGH_MODEL` (`claude-sonnet-5-5`) — is
/// sent `xhigh` and `max` as they are, once each.
#[tokio::test]
#[ignore = "requires MINDFORK_ANTHROPIC_KEY + MINDFORK_LIVE_EFFORT_XHIGH_MODEL (a Claude model that takes \"xhigh\" and \"max\")"]
async fn the_top_levels_are_sent_as_they_are() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_EFFORT_XHIGH_MODEL") else {
        return;
    };
    for effort in [ReasoningEffort::XHigh, ReasoningEffort::Max] {
        let (text, finish) = turn_at(&client, effort).await;
        println!("{model} at {effort:?}: finish={finish:?} reply={text}");
        assert!(
            finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
            "{model} at {effort:?}: finish={finish:?} text={text:?}"
        );
        assert_eq!(
            client.learned(effort.as_wire()),
            None,
            "{model} was declared to take {effort:?}: nothing should have been learned"
        );
    }
}
