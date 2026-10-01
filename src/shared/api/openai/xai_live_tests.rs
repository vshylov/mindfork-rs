//! Live smokes of the reasoning efforts xAI publishes per model, against the
//! real API. `#[ignore]` — not in CI; silently skipped without the key, and each
//! arm without the variable that declares its model.
//!
//! Declared rather than guessed: which model lists `none`, which does not, and
//! which lists nothing at all is the very thing these pin
//! (docs/research/effort-tiers.md §9).
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out (docs/journal/quality.md).
//!
//! Run: `MINDFORK_GROK_KEY=… MINDFORK_LIVE_XAI_NONE_MODEL=grok-4.3
//! MINDFORK_LIVE_XAI_NO_NONE_MODEL=grok-4.7
//! MINDFORK_LIVE_XAI_NO_EFFORT_MODEL=grok-build-0.1
//! MINDFORK_LIVE_XAI_ALIAS=grok-4.5-latest cargo test -- xai_live_tests
//! --ignored --nocapture --test-threads=1`.

use futures_util::StreamExt;

use super::OpenAiClient;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason,
};
use crate::shared::config::CloudProvider;

fn key() -> Option<String> {
    let key = std::env::var("MINDFORK_GROK_KEY").ok();
    if key.is_none() {
        eprintln!("skip: MINDFORK_GROK_KEY not set");
    }
    key
}

/// The model `var` declares, and the client `cloud_chat_setup` builds for it.
fn declared(var: &str) -> Option<(String, OpenAiClient)> {
    let key = key()?;
    let Ok(model) = std::env::var(var) else {
        eprintln!("skip: {var} not set");
        return None;
    };
    let client = OpenAiClient::new(CloudProvider::Grok.chat_base_url())
        .with_api_key(Some(key))
        .with_model(Some(model.clone()))
        .for_xai();
    Some((model, client))
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

fn at(effort: ReasoningEffort) -> ChatRequest {
    turn(SamplingConfig {
        reasoning_effort: Some(effort),
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

async fn run(client: &OpenAiClient, req: ChatRequest) -> Result<Outcome, String> {
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Outcome {
        text: String::new(),
        finish: None,
        reasoning: None,
    };
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => out.text.push_str(&t),
            ChatChunk::Usage(u) => out.reasoning = Some(u.reasoning_tokens),
            ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
            ChatChunk::Finished(r) => {
                out.finish = Some(r);
                break;
            }
            _ => {}
        }
    }
    Ok(out)
}

/// A turn that completed with an answer.
fn completed(model: &str, what: &str, outcome: Result<Outcome, String>) -> Outcome {
    let outcome = outcome.unwrap_or_else(|said| panic!("{model}: {what} was refused: {said}"));
    println!(
        "{model}: {what}: finish={:?} reasoning={:?} reply={}",
        outcome.finish, outcome.reasoning, outcome.text
    );
    assert!(
        outcome.finish == Some(FinishReason::Stop) && !outcome.text.trim().is_empty(),
        "{model}: {what}: finish={:?} text={:?}",
        outcome.finish,
        outcome.text
    );
    outcome
}

/// A model that lists `none` is sent it: the title, the roll and impersonation
/// do not reason there. Left out, as it was before the list was read, the same
/// turn reasoned at the model's default — some three hundred tokens, measured.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn a_muted_turn_does_not_reason_where_the_model_lists_none() {
    let Some((model, client)) = declared("MINDFORK_LIVE_XAI_NONE_MODEL") else {
        return;
    };
    for nth in 1..=2 {
        let outcome = completed(
            &model,
            &format!("muted turn {nth}"),
            run(&client, muted()).await,
        );
        assert_eq!(
            outcome.reasoning,
            Some(0),
            "{model} lists `none`, and a muted turn must be sent it"
        );
    }
    assert_eq!(client.learned("none"), None, "a listed value goes as it is");
}

/// A model that lists no `none` is asked for the lowest depth it lists, in
/// place of a request with no effort — which is its default, `high` on the
/// models measured.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn a_muted_turn_asks_for_the_lowest_listed_where_none_is_not() {
    let Some((model, client)) = declared("MINDFORK_LIVE_XAI_NO_NONE_MODEL") else {
        return;
    };
    completed(&model, "muted turn", run(&client, muted()).await);
    assert_eq!(client.learned("none"), Some(Some("low")));
}

/// The settings' `max` on a model whose list ends at `xhigh`.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn the_top_tier_goes_out_as_the_highest_listed() {
    let Some((model, client)) = declared("MINDFORK_LIVE_XAI_NO_NONE_MODEL") else {
        return;
    };
    completed(
        &model,
        "a turn at max",
        run(&client, at(ReasoningEffort::Max)).await,
    );
    assert_eq!(client.learned("max"), Some(Some("xhigh")));
}

/// A model with no list refuses the parameter itself, whatever the value: the
/// turn is asked once more without it, and the next one starts there.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn a_model_without_the_parameter_is_asked_without_it() {
    let Some((model, client)) = declared("MINDFORK_LIVE_XAI_NO_EFFORT_MODEL") else {
        return;
    };
    for nth in 1..=2 {
        completed(
            &model,
            &format!("turn {nth} at high"),
            run(&client, at(ReasoningEffort::High)).await,
        );
    }
    assert_eq!(client.learned("high"), Some(None));
    completed(&model, "muted turn", run(&client, muted()).await);
}

/// An alias is a name the list does not carry as an id: the single-model route
/// resolves it, and the window comes back with the efforts.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn an_alias_is_read_as_the_model_it_names() {
    let Some((alias, client)) = declared("MINDFORK_LIVE_XAI_ALIAS") else {
        return;
    };
    let caps = client.model_capabilities().await;
    println!("{alias}: {caps:?}");
    assert!(
        caps.is_some_and(|c| c.context_length.is_some()),
        "{alias}: the model's entry must answer for an alias"
    );
}

/// The control arm: the top tier sent as it is, by a client that was not told
/// the server is xAI, is the refusal the list is read for. When this starts
/// passing a request through, xAI has gained the tier.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn the_top_tier_is_refused_when_sent_as_it_is() {
    let Some((model, _)) = declared("MINDFORK_LIVE_XAI_NO_NONE_MODEL") else {
        return;
    };
    let plain = OpenAiClient::new(CloudProvider::Grok.chat_base_url())
        .with_api_key(key())
        .with_model(Some(model.clone()));
    let refused = run(&plain, at(ReasoningEffort::Max)).await.err();
    println!("{model}: {refused:?}");
    assert!(
        refused.is_some_and(|said| said.contains("400")),
        "{model} was measured to refuse \"max\""
    );
}
