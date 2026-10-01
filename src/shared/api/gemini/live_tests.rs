//! Live smokes of the Gemini client's muted turns against the real API.
//! `#[ignore]` — not in CI; silently skipped without their variables.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out, and smokes that never run in CI would otherwise count as
//! production code nothing covers (docs/journal/quality.md).
//!
//! Run both arms:
//! `MINDFORK_GEMINI_KEY=… MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL=gemini-3.8-flash
//! MINDFORK_LIVE_MINIMAL_LEVEL_MODEL=gemini-3.5-flash cargo test
//! gemini::live_tests -- --ignored --nocapture --test-threads=1`.

use futures_util::StreamExt;

use super::client::GeminiClient;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason,
};

/// A client for the model `var` declares, or `None` when the key or the
/// declaration is missing.
fn client_for(var: &str) -> Option<(String, GeminiClient)> {
    let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
        eprintln!("skip: MINDFORK_GEMINI_KEY not set");
        return None;
    };
    let Ok(model) = std::env::var(var) else {
        eprintln!("skip: {var} not set");
        return None;
    };
    let client = GeminiClient::new(
        "https://generativelanguage.googleapis.com/v1beta",
        key,
        model.clone(),
    );
    Some((model, client))
}

/// The title turn's own shape (`title.rs`): muted reasoning, a short cap, a
/// moderate temperature, no tools.
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

/// One muted turn: the reply and how the stream ended. A refused request fails
/// the smoke here, with the provider's words.
async fn muted_turn(client: &GeminiClient) -> (String, Option<FinishReason>) {
    let mut stream = client
        .chat_stream(title_turn(), Default::default())
        .await
        .expect("a muted turn must not be refused");
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

/// The defect, live: a Gemini 3.x model that answers `thinkingLevel:
/// "minimal"` with a `400` and is not a Pro —
/// `MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL` declares one (`gemini-3.7-flash` and
/// `gemini-3.8-flash`, measured 2026-10-01). A muted turn asks for that level,
/// so before the recovery this request was the `400` itself and no chat on
/// such a model got its title.
///
/// Declared rather than guessed, so the smoke **fails** rather than passes when
/// the recovery never ran: a model that takes `minimal` would be green here
/// having proved nothing.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY + MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL (a Gemini 3.x model, not a Pro, that refuses thinkingLevel \"minimal\")"]
async fn a_muted_turn_survives_a_model_without_the_minimal_level() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL") else {
        return;
    };
    // Twice through one client: the first turn meets the refusal, the second
    // must ask for the learned level at once.
    for turn in 1..=2 {
        let (text, finish) = muted_turn(&client).await;
        println!("{model} turn {turn}: finish={finish:?} title={text}");
        assert!(
            finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
            "{model}: the muted turn must complete: finish={finish:?} text={text:?}"
        );
    }
    let learned = client.learned("minimal");
    println!("{model}: asked instead of \"minimal\": {learned:?}");
    assert!(
        learned.is_some(),
        "{model} was declared to refuse \"minimal\", yet nothing was refused — the recovery did not run"
    );
}

/// The control arm: a model that does take `minimal` —
/// `MINDFORK_LIVE_MINIMAL_LEVEL_MODEL` (`gemini-3.5-flash`) — is asked once, at
/// the level it was always asked at.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY + MINDFORK_LIVE_MINIMAL_LEVEL_MODEL (a Gemini 3.x model that takes thinkingLevel \"minimal\")"]
async fn a_muted_turn_on_a_model_with_the_minimal_level_is_sent_as_before() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_MINIMAL_LEVEL_MODEL") else {
        return;
    };
    let (text, finish) = muted_turn(&client).await;
    println!("{model}: finish={finish:?} title={text}");
    assert!(
        finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
        "{model}: finish={finish:?} text={text:?}"
    );
    assert_eq!(
        client.learned("minimal"),
        None,
        "{model} was declared to take \"minimal\": nothing should have been learned"
    );
}
