//! Live smokes of the Responses client against the real OpenAI API: an effort
//! value the model does not have — a muted turn's `"none"`, a depth from the
//! settings. `#[ignore]` — not in CI; silently skipped without their variables.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out, and smokes that never run in CI would otherwise count as
//! production code nothing covers (docs/journal/quality.md).
//!
//! Run every arm:
//! `MINDFORK_OPENAI_KEY=… MINDFORK_LIVE_NO_EFFORT_NONE_MODEL=gpt-6.1-sol
//! MINDFORK_LIVE_EFFORT_NONE_MODEL=gpt-6-sol
//! MINDFORK_LIVE_NO_EFFORT_MINIMAL_MODEL=gpt-6.1-sol
//! MINDFORK_LIVE_NO_EFFORT_MAX_MODEL=gpt-5.5 cargo test
//! responses::live_tests -- --ignored --nocapture --test-threads=1`.

use futures_util::StreamExt;

use super::client::ResponsesClient;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason,
};

/// A client for the model `var` declares, or `None` when the key or the
/// declaration is missing.
fn client_for(var: &str) -> Option<(String, ResponsesClient)> {
    let Ok(key) = std::env::var("MINDFORK_OPENAI_KEY") else {
        eprintln!("skip: MINDFORK_OPENAI_KEY not set");
        return None;
    };
    let Ok(model) = std::env::var(var) else {
        eprintln!("skip: {var} not set");
        return None;
    };
    let client = ResponsesClient::new("https://api.openai.com/v1", key, model.clone());
    Some((model, client))
}

/// The title turn's own shape (`title.rs`): muted reasoning, a short cap, no
/// tools. The temperature it also sets never reaches this wire.
fn title_turn() -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: Some("Give this conversation a short title. Answer with the title only.".into()),
        messages: vec![ApiMessage::user("How do database indexes work?")],
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            temperature: Some(0.3),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..Default::default()
        },
        tools: vec![],
    }
}

/// An ordinary turn at a depth chosen in the settings, thinking untouched.
fn turn_at(effort: ReasoningEffort) -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user("Reply with exactly: pong")],
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            reasoning_effort: Some(effort),
            ..Default::default()
        },
        tools: vec![],
    }
}

/// One turn: the reply, the reasoning tokens the API reported, and how the
/// stream ended. A refused request fails the smoke here, with the provider's
/// words.
async fn turn(client: &ResponsesClient, req: ChatRequest) -> (String, u32, Option<FinishReason>) {
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .expect("the turn must not be refused");
    let (mut text, mut reasoning, mut finish) = (String::new(), 0, None);
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Usage(u) => reasoning = u.reasoning_tokens,
            ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
            ChatChunk::Finished(r) => {
                finish = Some(r);
                break;
            }
            _ => {}
        }
    }
    (text, reasoning, finish)
}

/// The defect, live: a model that answers `reasoning.effort: "none"` with a
/// `400` — `MINDFORK_LIVE_NO_EFFORT_NONE_MODEL` declares one (`gpt-6.1-sol`,
/// measured 2026-10-01). Before the recovery this request was the `400` itself
/// and no chat on such a model got its title.
///
/// Declared rather than guessed, so the smoke **fails** rather than passes when
/// the recovery never ran: a model that takes `"none"` would be green here
/// having proved nothing.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY + MINDFORK_LIVE_NO_EFFORT_NONE_MODEL (an OpenAI model that refuses reasoning.effort \"none\")"]
async fn a_muted_turn_survives_a_model_that_refuses_effort_none() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_NO_EFFORT_NONE_MODEL") else {
        return;
    };
    // Twice through one client: the first turn meets the refusal, the second
    // must ask in the learned words at once.
    for nth in 1..=2 {
        let (text, reasoning, finish) = turn(&client, title_turn()).await;
        println!("{model} turn {nth}: finish={finish:?} reasoning_tokens={reasoning} title={text}");
        assert!(
            finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
            "{model}: the muted turn must complete: finish={finish:?} text={text:?}"
        );
    }
    let learned = client.learned("none");
    println!("{model}: asked instead of \"none\": {learned:?}");
    assert!(
        learned.is_some(),
        "{model} was declared to refuse \"none\", yet nothing was refused — the recovery did not run"
    );
}

/// The control arm: a model that does take `"none"` —
/// `MINDFORK_LIVE_EFFORT_NONE_MODEL` (`gpt-6-sol`) — is asked once, in the
/// words it was always asked in, and does not reason.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY + MINDFORK_LIVE_EFFORT_NONE_MODEL (an OpenAI model that takes reasoning.effort \"none\")"]
async fn a_muted_turn_on_a_model_that_takes_none_is_sent_as_before() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_EFFORT_NONE_MODEL") else {
        return;
    };
    let (text, reasoning, finish) = turn(&client, title_turn()).await;
    println!("{model}: finish={finish:?} reasoning_tokens={reasoning} title={text}");
    assert!(
        finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
        "{model}: finish={finish:?} text={text:?}"
    );
    assert_eq!(reasoning, 0, "{model} was told not to reason");
    assert_eq!(
        client.learned("none"),
        None,
        "{model} was declared to take \"none\": nothing should have been learned"
    );
}

/// A depth from the settings that the model does not have —
/// `MINDFORK_LIVE_NO_EFFORT_MINIMAL_MODEL` declares one that refuses `minimal`
/// (nine OpenAI models of the ten measured on 2026-10-01; `gpt-6.1-sol` among
/// them). One setting serves every model, so before the recovery a value
/// chosen for one model was a `400` on every turn of the next.
///
/// Declared rather than guessed, so the smoke **fails** rather than passes on a
/// model that takes the value.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY + MINDFORK_LIVE_NO_EFFORT_MINIMAL_MODEL (an OpenAI model that refuses reasoning.effort \"minimal\")"]
async fn a_chosen_depth_the_model_lacks_becomes_the_nearest_it_has() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_NO_EFFORT_MINIMAL_MODEL") else {
        return;
    };
    for nth in 1..=2 {
        let (text, reasoning, finish) = turn(&client, turn_at(ReasoningEffort::Minimal)).await;
        println!("{model} turn {nth}: finish={finish:?} reasoning_tokens={reasoning} reply={text}");
        assert!(
            finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
            "{model}: the turn must complete: finish={finish:?} text={text:?}"
        );
    }
    let learned = client.learned("minimal");
    println!("{model}: asked instead of \"minimal\": {learned:?}");
    assert!(
        matches!(learned, Some(Some(_))),
        "{model} was declared to refuse \"minimal\" and to list what it takes: {learned:?}"
    );
}

/// The scale's top tier on a model that stops below it —
/// `MINDFORK_LIVE_NO_EFFORT_MAX_MODEL` declares one (`gpt-5.2`, `gpt-5.4`,
/// `gpt-5.5` end at `xhigh`; measured 2026-10-01). The nearest it lists is
/// below, never above: there is nothing above.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY + MINDFORK_LIVE_NO_EFFORT_MAX_MODEL (an OpenAI model that refuses reasoning.effort \"max\")"]
async fn the_top_tier_on_a_model_without_it_becomes_the_one_below() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_NO_EFFORT_MAX_MODEL") else {
        return;
    };
    let (text, reasoning, finish) = turn(&client, turn_at(ReasoningEffort::Max)).await;
    println!("{model}: finish={finish:?} reasoning_tokens={reasoning} reply={text}");
    assert!(
        finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
        "{model}: the turn must complete: finish={finish:?} text={text:?}"
    );
    let learned = client.learned("max");
    println!("{model}: asked instead of \"max\": {learned:?}");
    assert!(
        matches!(learned, Some(Some(_))),
        "{model} was declared to refuse \"max\" and to list what it takes: {learned:?}"
    );
}
