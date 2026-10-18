//! Live smokes of DeepSeek's own API — in `external` mode and in the `deepseek`
//! mode of its own — against the real API. `#[ignore]` — not in CI; silently
//! skipped without the key.
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

use super::OpenAiClient;
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, EngineBackend, FinishReason, VisionSupport,
};
use crate::shared::api::sse_stub::{collect, hello, muted};

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

/// An ordinary turn under the settings' default: the switch on. The cap leaves
/// room for the reasoning, which it counts.
fn switched_on() -> ChatRequest {
    let mut req = hello();
    req.sampling.thinking = Some(true);
    req.sampling.max_tokens = Some(2048);
    req
}

/// One turn to its end, and the reasoning tokens its usage counted. A refusal
/// or an error in the stream fails the arm in the server's words.
async fn completed(client: &OpenAiClient, model: &str, what: &str, req: ChatRequest) -> u32 {
    let stream = client
        .chat_stream(req, Default::default())
        .await
        .unwrap_or_else(|e| panic!("{model}: {what} was refused: {e}"));
    let chunks = collect(stream).await;
    let mut text = String::new();
    let (mut finish, mut reasoning) = (None, 0);
    for chunk in chunks {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Usage(u) => reasoning = u.reasoning_tokens,
            ChatChunk::Error { message, .. } => panic!("{model}: {what}: {message}"),
            ChatChunk::Finished(r) => finish = Some(r),
            _ => {}
        }
    }
    println!(
        "{model}: {what}: finish={finish:?} reasoning={reasoning} typed={} reply={}",
        client.thinking_learned(),
        text.trim()
    );
    assert!(
        finish == Some(FinishReason::Stop) && !text.trim().is_empty(),
        "{model}: {what}: finish={finish:?} text={text:?}"
    );
    reasoning
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
        let reasoned = completed(&client, &model, "muted turn", muted()).await;
        assert!(
            client.thinking_learned(),
            "{model}: the refusal of the boolean taught the object"
        );
        assert_eq!(reasoned, 0, "{model}: a muted turn must not reason");
        completed(&client, &model, "thinking turn", switched_on()).await;
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
        completed(&client, &model, "first turn", switched_on()).await;
        assert!(client.thinking_learned(), "{model}: learned on the way");
    }
}

/// The clients of [`clients`] as the `deepseek` mode builds them
/// ([`OpenAiClient::for_deepseek`], docs/research/deepseek-mode.md §4.1).
fn in_the_mode() -> Option<Vec<(String, OpenAiClient)>> {
    Some(
        clients()?
            .into_iter()
            .map(|(model, client)| (model, client.for_deepseek()))
            .collect(),
    )
}

/// In the mode the switch is typed from the first request: a muted turn —
/// the switch off and no effort — completes without reasoning, and a turn with
/// the switch on reasons. Neither had a refusal to learn from.
#[tokio::test]
#[ignore = "requires MINDFORK_DEEPSEEK_KEY (live DeepSeek API)"]
async fn the_mode_mutes_and_reasons_on_deepseek() {
    let Some(clients) = in_the_mode() else {
        return;
    };
    for (model, client) in clients {
        assert!(client.thinking_learned(), "{model}: told, not learned");
        let muted = completed(&client, &model, "muted turn", muted()).await;
        assert_eq!(muted, 0, "{model}: the switch off must mean no reasoning");
        let on = completed(&client, &model, "thinking turn", switched_on()).await;
        assert!(on > 0, "{model}: the switch on must reason");
    }
}

/// The window and the images, read from DeepSeek's list in its words: every
/// model's window, and images on the models whose inputs name them —
/// `deepseek-flash` takes them, `deepseek-v4-pro` does not (measured §1).
#[tokio::test]
#[ignore = "requires MINDFORK_DEEPSEEK_KEY (live DeepSeek API)"]
async fn the_mode_reads_the_window_and_the_images_on_deepseek() {
    let Some(clients) = in_the_mode() else {
        return;
    };
    for (model, client) in clients {
        let caps = client.model_capabilities().await;
        let vision = client.vision().await;
        println!("{model}: caps={caps:?} vision={vision:?}");
        let window = caps.and_then(|c| c.context_length);
        assert!(window.is_some_and(|w| w >= 65_536), "{model}: {window:?}");
        let expected = match model.as_str() {
            "deepseek-flash" => VisionSupport::Supported,
            "deepseek-v4-pro" => VisionSupport::Unsupported,
            _ => continue,
        };
        assert_eq!(vision, expected, "{model}");
    }
}
