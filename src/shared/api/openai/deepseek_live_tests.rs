//! Live smokes of DeepSeek's own API in `external` mode, against the real API.
//! `#[ignore]` — not in CI; silently skipped without the key.
//!
//! DeepSeek refuses the thinking switch as llama.cpp's boolean with a `422`, so
//! before the client learned its spelling no request went through
//! (docs/research/deepseek-api.md §2.3). Each arm builds the client the way
//! `external` mode does — a URL, a key, a model, nothing told — and checks that
//! the turns the app sends complete, and that the switch still means what it
//! says.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out (docs/journal/quality.md).
//!
//! Run: `MINDFORK_DEEPSEEK_KEY=… [MINDFORK_LIVE_DEEPSEEK_MODELS=deepseek-flash]
//! cargo test -- deepseek_live_tests --ignored --nocapture --test-threads=1`.

use futures_util::StreamExt;

use super::OpenAiClient;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason,
};

/// DeepSeek's OpenAI-compatible base, as a user would type it in `external`.
const BASE: &str = "https://api.deepseek.com/v1";

/// Both models `GET /models` listed on 2026-10-18.
const MODELS: &str = "deepseek-flash,deepseek-v4-pro";

/// One client per model, as `external` mode builds it, or `None` without the key.
fn clients() -> Option<Vec<(String, OpenAiClient)>> {
    let Some(key) = std::env::var("MINDFORK_DEEPSEEK_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        eprintln!("skip: MINDFORK_DEEPSEEK_KEY not set");
        return None;
    };
    let models = std::env::var("MINDFORK_LIVE_DEEPSEEK_MODELS")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| MODELS.to_string());
    Some(
        models
            .split(',')
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|model| {
                let client = OpenAiClient::new(BASE)
                    .with_api_key(Some(key.clone()))
                    .with_model(Some(model.to_string()));
                (model.to_string(), client)
            })
            .collect(),
    )
}

fn turn(sampling: SamplingConfig) -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user("Reply with exactly: pong")],
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            ..sampling
        },
        tools: vec![],
    }
}

/// A turn that asks for reasoning to be off, the way `title.rs` does.
fn muted() -> ChatRequest {
    turn(SamplingConfig {
        thinking: Some(false),
        reasoning_effort: Some(ReasoningEffort::None),
        reasoning_budget: Some(0),
        ..Default::default()
    })
}

/// An ordinary turn under the settings' default: the switch on.
fn thinking() -> ChatRequest {
    turn(SamplingConfig {
        thinking: Some(true),
        ..Default::default()
    })
}

/// What a turn came to: its text, how it ended, and the reasoning tokens the
/// usage chunk counted.
struct Outcome {
    text: String,
    finish: Option<FinishReason>,
    reasoning: Option<u32>,
}

/// One turn to its end. A refusal fails the arm in the server's words.
async fn completed(client: &OpenAiClient, model: &str, what: &str, req: ChatRequest) -> Outcome {
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .unwrap_or_else(|e| panic!("{model}: {what} was refused: {e}"));
    let mut out = Outcome {
        text: String::new(),
        finish: None,
        reasoning: None,
    };
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => out.text.push_str(&t),
            ChatChunk::Usage(u) => out.reasoning = Some(u.reasoning_tokens),
            ChatChunk::Error { message, .. } => panic!("{model}: {what}: {message}"),
            ChatChunk::Finished(r) => {
                out.finish = Some(r);
                break;
            }
            _ => {}
        }
    }
    println!(
        "{model}: {what}: finish={:?} reasoning={:?} typed={} reply={}",
        out.finish,
        out.reasoning,
        client.thinking_learned(),
        out.text.trim()
    );
    assert!(
        out.finish == Some(FinishReason::Stop) && !out.text.trim().is_empty(),
        "{model}: {what}: finish={:?} text={:?}",
        out.finish,
        out.text
    );
    out
}

/// The muted turn — the title, the compaction roll, impersonation — completes
/// and does not reason; the client has learned the object from the refusal of
/// its first request. Then an ordinary turn with the switch on completes on the
/// same client, sent typed from its first request.
#[tokio::test]
#[ignore = "requires MINDFORK_DEEPSEEK_KEY (live DeepSeek API)"]
async fn a_muted_turn_and_a_thinking_turn_complete_on_deepseek() {
    let Some(clients) = clients() else {
        return;
    };
    for (model, client) in clients {
        assert!(!client.thinking_learned(), "a fresh client knows nothing");
        let off = completed(&client, &model, "muted turn", muted()).await;
        assert!(
            client.thinking_learned(),
            "{model}: the refusal of the boolean taught the object"
        );
        assert_eq!(
            off.reasoning.unwrap_or(0),
            0,
            "{model}: a muted turn must not reason"
        );
        completed(&client, &model, "thinking turn", thinking()).await;
    }
}

/// An ordinary turn as the **first** request of a client — the settings'
/// default, `thinking: true`, which was the `422` every new chat met.
#[tokio::test]
#[ignore = "requires MINDFORK_DEEPSEEK_KEY (live DeepSeek API)"]
async fn the_first_turn_of_a_new_engine_completes_on_deepseek() {
    let Some(clients) = clients() else {
        return;
    };
    for (model, client) in clients {
        completed(&client, &model, "first turn", thinking()).await;
        assert!(client.thinking_learned(), "{model}: learned on the way");
    }
}
