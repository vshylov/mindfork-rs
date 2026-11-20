//! Live smokes of the thinking knobs and a text reply's signature against the
//! real Gemini API (docs/journal/engine.md, "Gemini 4 takes thinking levels").
//! Run per model: `MINDFORK_GEMINI_KEY=… MINDFORK_GEMINI_MODEL=gemini-4-neon cargo
//! test gemini::client::thinking_live -- --ignored --nocapture`. Kept under
//! `tests/` beside the client, like the other live suites: CI never runs an
//! `#[ignore]` smoke, and the coverage report leaves files here out.

use super::ignored_smoke::client_from_env;
use super::*;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::ApiMessage;

/// One streamed turn: the reply's text, thoughts, text signature and end.
async fn turn(
    client: &GeminiClient,
    messages: Vec<ApiMessage>,
    sampling: SamplingConfig,
) -> (String, String, Option<String>, Option<FinishReason>) {
    let req = ChatRequest {
        continue_final: false,
        output_schema: None,
        system: None,
        messages,
        sampling: SamplingConfig {
            max_tokens: Some(4096),
            ..sampling
        },
        tools: vec![],
    };
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .unwrap_or_else(|e| panic!("the request was refused: {e:#}"));
    let (mut text, mut thoughts, mut signature, mut finish) =
        (String::new(), String::new(), None, None);
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Thoughts(t) => thoughts.push_str(&t),
            ChatChunk::TextSignature(s) => signature = Some(s),
            ChatChunk::Finished(r) => {
                finish = Some(r);
                break;
            }
            ChatChunk::Error { message, .. } => panic!("engine error: {message}"),
            _ => {}
        }
    }
    (text, thoughts, signature, finish)
}

/// Every thinking setting the app sends reaches the model and is answered:
/// thoughts on at the model's own depth, a chosen effort high and low, and a
/// muted turn. The defect it guards: Gemini 4 and Gemma 4 were sent 2.5's
/// `thinkingBudget` — taken by Gemini 4 as a legacy knob, refused by Gemma 4
/// with a `400` on every turn with thoughts on. Run per model with
/// `MINDFORK_GEMINI_MODEL`; prints what each setting was answered with.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
async fn every_thinking_setting_is_answered() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_GEMINI_KEY not set");
        return;
    };
    let ask = || vec![ApiMessage::user("What is 17 * 23? Answer with the number.")];
    let settings = [
        ("thoughts on", Some(true), None, None),
        ("effort high", Some(true), Some(ReasoningEffort::High), None),
        ("effort low", Some(true), Some(ReasoningEffort::Low), None),
        ("muted", None, None, Some(0)),
    ];
    for (label, thinking, effort, budget) in settings {
        let sampling = SamplingConfig {
            thinking,
            reasoning_effort: effort,
            reasoning_budget: budget,
            ..Default::default()
        };
        let (text, thoughts, _, finish) = turn(&client, ask(), sampling).await;
        eprintln!(
            "{} {label}: {finish:?}, text {:?}, thoughts {} chars",
            client.model,
            text.trim(),
            thoughts.len()
        );
        assert!(text.contains("391"), "{label}: {text:?}");
    }
    for wish in ["minimal", "low", "medium"] {
        if let Some(instead) = client.learned(wish) {
            eprintln!("{} learned: {wish} -> {instead:?}", client.model);
        }
    }
}

/// A text reply's signature arrives and is taken back: a model of levels
/// (3.x and later) signs a reply that thought, and the next turn, carrying
/// the signature on the reply's text, is answered — no `400` "Corrupted
/// thought signature", which a forged one gets. 2.5 signs no text reply.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
async fn a_text_replys_signature_round_trips() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_GEMINI_KEY not set");
        return;
    };
    let thinking = || SamplingConfig {
        thinking: Some(true),
        ..Default::default()
    };
    let first = ApiMessage::user("Pick a random fruit and name it in one word.");
    let (text, _, signature, finish) = turn(&client, vec![first.clone()], thinking()).await;
    eprintln!(
        "{}: {finish:?}, reply {:?}, signature {}",
        client.model,
        text.trim(),
        signature.as_ref().map_or(0, String::len)
    );
    if !wire::takes_thinking_level(&client.model) {
        assert!(signature.is_none(), "2.5 signs no text reply");
        return;
    }
    // Google documents the signature for Gemini 3 and 4; Gemma 4 signs its
    // calls and not its text (measured 2026-11-20), so it is not required there.
    let Some(signature) = signature else {
        assert!(
            !client.model.starts_with("gemini-"),
            "a Gemini of levels signs a reply that thought"
        );
        eprintln!("{}: no text signature to send back", client.model);
        return;
    };
    let history = vec![
        first,
        ApiMessage::assistant(text.clone()).with_thought_signature(Some(signature)),
        ApiMessage::user("Which fruit did you pick? Repeat the word."),
    ];
    let (again, _, _, finish) = turn(&client, history, thinking()).await;
    eprintln!(
        "{} second turn: {finish:?}, {:?}",
        client.model,
        again.trim()
    );
    assert!(
        matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
        "{finish:?}"
    );
    let picked = text.trim().trim_end_matches('.').to_lowercase();
    assert!(
        again.to_lowercase().contains(&picked),
        "{again:?} vs {text:?}"
    );
}
