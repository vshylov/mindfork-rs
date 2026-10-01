//! Live smokes of the effort ceiling against the real xAI API. `#[ignore]` —
//! not in CI; silently skipped without the key.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out (docs/journal/quality.md).
//!
//! Run: `MINDFORK_GROK_KEY=… cargo test xai_live_tests -- --ignored
//! --nocapture --test-threads=1` (`MINDFORK_GROK_MODEL` picks the model).

use futures_util::StreamExt;

use super::OpenAiClient;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason,
};
use crate::shared::config::CloudProvider;

/// The client `cloud_chat_setup` builds for Grok, with or without the ceiling.
fn client(capped: bool) -> Option<(String, OpenAiClient)> {
    let Ok(key) = std::env::var("MINDFORK_GROK_KEY") else {
        eprintln!("skip: MINDFORK_GROK_KEY not set");
        return None;
    };
    let model = std::env::var("MINDFORK_GROK_MODEL").unwrap_or_else(|_| "grok-4.7".into());
    let client = OpenAiClient::new(CloudProvider::Grok.chat_base_url())
        .with_api_key(Some(key))
        .with_model(Some(model.clone()))
        .with_effort_none_omitted(true);
    let client = if capped {
        client.with_effort_capped_at(ReasoningEffort::XHigh)
    } else {
        client
    };
    Some((model, client))
}

/// A turn at the scale's top tier.
fn at_max() -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user("Reply with exactly: pong")],
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..Default::default()
        },
        tools: vec![],
    }
}

/// The settings' `max` on Grok: xAI has no such value, and the client built
/// for it sends `xhigh` — so the turn completes.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn the_top_tier_completes_under_the_ceiling() {
    let Some((model, client)) = client(true) else {
        return;
    };
    let mut stream = client
        .chat_stream(at_max(), Default::default())
        .await
        .expect("a capped turn must not be refused");
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
    println!("{model}: finish={finish:?} reply={text}");
    assert!(
        finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
        "{model}: finish={finish:?} text={text:?}"
    );
}

/// The control arm: the same turn without the ceiling is the refusal the
/// ceiling exists for. When this starts passing a request through, xAI has
/// gained the tier and the ceiling can go.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn the_top_tier_is_refused_without_the_ceiling() {
    let Some((model, client)) = client(false) else {
        return;
    };
    let refused = client
        .chat_stream(at_max(), Default::default())
        .await
        .err()
        .map(|e| e.to_string());
    println!("{model}: {refused:?}");
    assert!(
        refused.is_some_and(|said| said.contains("400")),
        "{model} was measured to refuse \"max\": if it takes it now, drop the ceiling"
    );
}
