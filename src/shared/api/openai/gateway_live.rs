//! Live smokes of the OpenRouter mode's client — `#[ignore]`, run by hand against
//! the gateway before a change to this mode is merged (AGENTS.md §3).
//!
//! Declared by `MINDFORK_OPENROUTER_KEY`; without it every smoke skips, saying
//! so. Each names the model it was measured on and takes another from a
//! variable of its own, because a model's name ages and its **kind** — reasons
//! by default, must reason, takes images — is what the smoke is about. A smoke
//! whose model turns out not to be of that kind fails rather than skips
//! (docs/lessons.md §9).
//!
//! The client is built the way the supervisor builds it, not by
//! [`live_client`](crate::shared::api::live_client): that one is the `external`
//! client, which is the control arm here, not the subject.

use futures_util::StreamExt;

use super::OpenAiClient;
use super::client::KeyVerdict;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::catalogue::{self, CatalogueRequest, CatalogueShape, ModelRole};
use crate::shared::api::contract::{
    ApiImage, ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EmbedRole, Embedder, EngineBackend,
    FinishReason, Served, TokenUsage, ToolSchema, VisionSupport,
};
use crate::shared::config::CloudProvider;

fn key() -> Option<String> {
    let key = std::env::var("MINDFORK_OPENROUTER_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    if key.is_none() {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY not set");
    }
    key
}

fn base() -> &'static str {
    CloudProvider::OpenRouter.chat_base_url()
}

/// The mode's own client, as `cloud_chat_setup` builds it.
fn gateway(key: &str, model: &str) -> OpenAiClient {
    OpenAiClient::new(base())
        .with_api_key(Some(key.to_string()))
        .with_model(Some(model.to_string()))
        .for_openrouter(true)
}

/// The same address and key through the client `external` builds — the control.
fn external(key: &str, model: &str) -> OpenAiClient {
    OpenAiClient::new(base())
        .with_api_key(Some(key.to_string()))
        .with_model(Some(model.to_string()))
}

/// The model a smoke runs on: the one the run names, or the one it was
/// measured on.
fn model(var: &str, measured: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| measured.to_string())
}

/// Everything a turn produced.
#[derive(Debug, Default)]
struct Turn {
    text: String,
    thoughts: String,
    calls: Vec<(Option<String>, String)>,
    usage: Option<TokenUsage>,
    served: Option<Served>,
    finish: Option<FinishReason>,
    failure: Option<String>,
}

async fn run(client: &OpenAiClient, req: ChatRequest) -> Turn {
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .expect("the request was taken");
    let mut turn = Turn::default();
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => turn.text.push_str(&t),
            ChatChunk::Thoughts(t) => turn.thoughts.push_str(&t),
            ChatChunk::ToolCall(d) => match turn.calls.get_mut(d.index) {
                Some((name, arguments)) => {
                    *name = name.take().or(d.name);
                    arguments.push_str(&d.arguments);
                }
                None => turn.calls.push((d.name, d.arguments)),
            },
            ChatChunk::Usage(u) => turn.usage = Some(u),
            ChatChunk::Served(s) => turn.served = Some(s),
            ChatChunk::Error { message, .. } => turn.failure = Some(message),
            ChatChunk::Finished(reason) => turn.finish = Some(reason),
            ChatChunk::ThoughtsSignature(_) | ChatChunk::Retry { .. } => {}
        }
    }
    turn
}

fn ask(text: &str, sampling: SamplingConfig) -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user(text)],
        sampling,
        tools: Vec::new(),
    }
}

/// The mode's first claim: a reasoning model's thoughts, its answer, the exact
/// usage — and the two facts only a gateway has, who served and for how much.
///
/// Measured on an open-weight model whose providers split the thoughts from
/// the answer every time (4 of 4, four providers). `deepseek/deepseek-r1` is
/// not one: on the provider the gateway routes it to, 2 runs of 4 came back
/// with the whole reply — the answer included — in `reasoning` and no
/// `content` at all. That is the stream as the gateway sends it, read chunk by
/// chunk outside the app, so this smoke would fail on it for a reason that is
/// not the client's.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_REASONING_MODEL names a model that reasons"]
async fn the_mode_streams_thoughts_an_answer_and_who_served_live() {
    let Some(key) = key() else { return };
    let model = model("MINDFORK_OPENROUTER_REASONING_MODEL", "qwen/qwen3.6-27b");
    let turn = run(
        &gateway(&key, &model),
        ask(
            "What is 17 times 23? Work it out, then answer with the number.",
            SamplingConfig {
                thinking: Some(true),
                max_tokens: Some(4096),
                ..Default::default()
            },
        ),
    )
    .await;
    eprintln!(
        "[{model}] finish={:?} thoughts={} chars text={:?} usage={:?} served={:?}",
        turn.finish,
        turn.thoughts.len(),
        turn.text,
        turn.usage,
        turn.served
    );
    assert_eq!(turn.failure, None);
    assert_eq!(turn.finish, Some(FinishReason::Stop));
    assert!(
        !turn.thoughts.trim().is_empty(),
        "{model} was declared to reason"
    );
    assert!(turn.text.contains("391"), "{:?}", turn.text);
    assert!(turn.usage.is_some_and(|u| u.prompt_tokens > 0));
    let served = turn.served.expect("a gateway says who served");
    assert!(served.provider.is_some_and(|p| !p.is_empty()));
    assert!(
        served.cost_nanos.is_some_and(|c| c > 0),
        "a paid model costs"
    );
}

/// A tool call comes back through the gateway as a call, and its result is
/// answered from — the round trip the agentic loop is made of.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_MODEL names a model that takes tools"]
async fn a_tool_call_round_trips_live() {
    let Some(key) = key() else { return };
    let model = model("MINDFORK_OPENROUTER_MODEL", "anthropic/claude-haiku-4.5");
    let client = gateway(&key, &model);
    let tools = vec![ToolSchema {
        name: "get_access_code".into(),
        description: "Returns today's access code.".into(),
        parameters: serde_json::json!({ "type": "object", "properties": {} }),
    }];
    let question = "Call get_access_code, then tell me the code it returned.";
    let first = run(
        &client,
        ChatRequest {
            tools: tools.clone(),
            ..ask(question, Default::default())
        },
    )
    .await;
    eprintln!("[{model}] first round: {first:?}");
    assert_eq!(first.finish, Some(FinishReason::ToolCalls), "{first:?}");
    assert_eq!(
        first.calls.first().and_then(|(name, _)| name.as_deref()),
        Some("get_access_code")
    );

    let second = run(
        &client,
        ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::user(question),
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        id: "call-1".into(),
                        name: "get_access_code".into(),
                        arguments: "{}".into(),
                        thought_signature: None,
                    }],
                ),
                // Invented, so that no model can answer it from anything but the result.
                ApiMessage::tool("call-1", "48213"),
            ],
            sampling: Default::default(),
            tools,
        },
    )
    .await;
    eprintln!("[{model}] second round: {second:?}");
    assert_eq!(second.failure, None);
    assert!(second.text.contains("48213"), "{:?}", second.text);
}

/// A turn that asks for reasoning to be off, the way the title, the compaction
/// roll and impersonation do.
fn muted(text: &str) -> ChatRequest {
    ask(
        text,
        SamplingConfig {
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            thinking: Some(false),
            max_tokens: Some(1024),
            ..Default::default()
        },
    )
}

/// What the dialect buys on a muted turn, against the control it replaces
/// (docs/research/openrouter-mode.md §4.1). Through the mode a model that must
/// reason is asked for its lowest effort and answers at once. Through
/// `external` the same turn is refused, re-sent without the request, and
/// answered at the model's **default** depth — so it works, and pays for
/// reasoning nobody asked for. Both arms must answer; the mode's must reason
/// less.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_MUST_REASON_MODEL names a model whose entry says reasoning is mandatory and lists efforts"]
async fn a_muted_turn_is_answered_at_the_lowest_effort_live() {
    let Some(key) = key() else { return };
    let model = model(
        "MINDFORK_OPENROUTER_MUST_REASON_MODEL",
        "google/gemini-3.5-flash",
    );
    let question = "Name a title of at most five words for a chat about database indexing. \
                    Answer with the title only.";
    let reasoned = |turn: &Turn| turn.usage.map_or(0, |u| u.reasoning_tokens);

    let through_the_mode = run(&gateway(&key, &model), muted(question)).await;
    eprintln!("[{model}] the mode: {through_the_mode:?}");
    assert_eq!(through_the_mode.failure, None);
    assert!(!through_the_mode.text.trim().is_empty());

    let through_external = run(&external(&key, &model), muted(question)).await;
    eprintln!("[{model}] external: {through_external:?}");
    assert_eq!(through_external.failure, None, "the recovery still works");
    assert!(!through_external.text.trim().is_empty());

    assert!(
        reasoned(&through_external) > 0,
        "{model} was declared to reason by default: the control measured nothing"
    );
    assert!(
        reasoned(&through_the_mode) < reasoned(&through_external),
        "the mode reasoned {} tokens, external {}",
        reasoned(&through_the_mode),
        reasoned(&through_external)
    );
}

/// The other mandatory kind: a model that must reason and lists **no** efforts.
/// There is nothing lower to ask for, so nothing is asked — and the turn is
/// answered rather than refused.
///
/// The answer is looked for in the reply **and** in the thoughts: the claim is
/// that the turn was taken, and the model this was measured on sometimes has
/// its whole reply delivered as reasoning (see
/// [`the_mode_streams_thoughts_an_answer_and_who_served_live`]).
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_ALWAYS_REASONS_MODEL names a model whose entry says mandatory and lists no efforts"]
async fn a_muted_turn_on_a_model_with_no_efforts_is_not_refused_live() {
    let Some(key) = key() else { return };
    let model = model(
        "MINDFORK_OPENROUTER_ALWAYS_REASONS_MODEL",
        "deepseek/deepseek-r1",
    );
    let turn = run(
        &gateway(&key, &model),
        muted("Answer with one word: the capital of France?"),
    )
    .await;
    eprintln!("[{model}] {turn:?}");
    assert_eq!(turn.failure, None, "the turn was refused");
    assert_eq!(turn.finish, Some(FinishReason::Stop));
    let said = format!("{} {}", turn.thoughts, turn.text).to_lowercase();
    assert!(said.contains("paris"), "{turn:?}");
}

/// A round whose tool result carries the fixture image — or, for the control,
/// does not.
fn screenshot_round(with_image: bool) -> ChatRequest {
    let tool = ApiMessage::tool("call-1", "Screenshot taken.");
    let tool = if with_image {
        // Labelled, as every image the app sends is (`prompt.images.label`). It
        // is not decoration: the image is re-homed into a user message, and a
        // user message that is an image and nothing else — measured on
        // `google/gemini-3.5-flash`, 8 of 8 — comes back with the model's
        // scratch text in the reply (`0The background…`, `_thought…`) or with
        // no reply at all; with the label before it, 8 of 8 are clean.
        tool.with_images(vec![ApiImage::new(
            "image/png",
            &crate::shared::api::green_circle_png_base64(),
            Some("Image #1 (screenshot.png):".into()),
        )])
    } else {
        tool
    };
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![
            ApiMessage::user(crate::shared::api::TOOL_VISION_PROMPT),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    id: "call-1".into(),
                    name: "take_screenshot".into(),
                    arguments: "{}".into(),
                    thought_signature: None,
                }],
            ),
            tool,
        ],
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            ..Default::default()
        },
        tools: vec![ToolSchema {
            name: "take_screenshot".into(),
            description: "Take a screenshot of the screen.".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }],
    }
}

/// A tool's image reaches the model — with the control that makes the claim
/// worth something: the same round without the image must **not** be answered
/// as if it had one.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_VISION_MODEL names a model that takes images"]
async fn a_tools_image_is_seen_live() {
    let Some(key) = key() else { return };
    let model = model(
        "MINDFORK_OPENROUTER_VISION_MODEL",
        "anthropic/claude-haiku-4.5",
    );
    let client = gateway(&key, &model);
    assert_eq!(
        client.vision().await,
        VisionSupport::Supported,
        "{model} was declared to take images"
    );
    let control = run(&client, screenshot_round(false)).await;
    eprintln!("[{model}] control (no image): {control:?}");
    crate::shared::api::assert_sees_green_circle(&control.text, false, "control");
    let seen = run(&client, screenshot_round(true)).await;
    eprintln!("[{model}] with the image: {seen:?}");
    crate::shared::api::assert_sees_green_circle(&seen.text, true, "with the image");
}

/// A `:variant` slug is not in the catalogue's list, and the gateway resolves
/// it all the same — so the mode knows the model's facts where `external`,
/// matching the list by id, knows nothing.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_variant_slug_has_its_models_facts_live() {
    let Some(key) = key() else { return };
    let slug = format!(
        "{}:nitro",
        model("MINDFORK_OPENROUTER_MODEL", "anthropic/claude-haiku-4.5")
    );
    let mode = gateway(&key, &slug);
    let caps = mode
        .model_capabilities()
        .await
        .expect("the gateway's entry");
    eprintln!("[{slug}] the mode: {caps:?}");
    assert!(caps.context_length.is_some_and(|n| n > 0));
    assert!(caps.sampling_fields.is_some_and(|f| !f.is_empty()));
    assert!(
        external(&key, &slug).model_capabilities().await.is_none(),
        "the control: the list has no such id"
    );
    let turn = run(
        &mode,
        ask(
            "Answer with one word: the capital of France?",
            Default::default(),
        ),
    )
    .await;
    assert!(turn.text.to_lowercase().contains("paris"), "{turn:?}");
}

/// Fork F6 against the real gateway: a key is accepted, a well-formed wrong one
/// is refused in the gateway's words.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn the_gateway_judges_a_key_live() {
    let Some(key) = key() else { return };
    assert_eq!(gateway(&key, "m").check_key().await, KeyVerdict::Accepted);
    let wrong = format!("sk-or-v1-{}", "0".repeat(64));
    match gateway(&wrong, "m").check_key().await {
        KeyVerdict::Refused(said) => eprintln!("refused: {said}"),
        other => panic!("a wrong key is refused: {other:?}"),
    }
}

/// Fork F7 against the real catalogues: the account's list, the public one and
/// the embedding models — each non-empty, none offering a `:batch` slug, and
/// the facts a row shows present on most entries.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn the_gateways_catalogues_answer_live() {
    let Some(key) = key() else { return };
    let ask = |shape, key: Option<String>| CatalogueRequest {
        shape,
        base: base().to_string(),
        key,
    };
    for (label, request, role) in [
        (
            "the account's list",
            ask(CatalogueShape::OpenRouter, Some(key.clone())),
            ModelRole::Chat,
        ),
        (
            "the public list",
            ask(CatalogueShape::OpenRouter, None),
            ModelRole::Chat,
        ),
        (
            "the embedding models",
            ask(CatalogueShape::OpenRouterEmbeddings, Some(key.clone())),
            ModelRole::Embedding,
        ),
    ] {
        let models = catalogue::fetch(&request).await.expect(label);
        let with_window = models
            .iter()
            .filter(|m| m.facts.context_length.is_some())
            .count();
        let with_price = models
            .iter()
            .filter(|m| m.facts.prompt_price.is_some())
            .count();
        eprintln!(
            "{label}: {} entries, {with_window} with a window, {with_price} with a price; \
             first {:?}",
            models.len(),
            models.first().map(|m| &m.id)
        );
        assert!(!models.is_empty(), "{label}");
        assert!(models.iter().all(|m| !m.id.ends_with(":batch")), "{label}");
        assert!(models.iter().all(|m| m.role == role), "{label}");
        assert!(with_window * 2 > models.len(), "{label}");
        assert!(with_price * 2 > models.len(), "{label}");
    }
}

/// The embeddings arm, as it ships in this stage: the request the embedder has
/// always sent, through the gateway's client — vectors in input order, one
/// width, unit length, and the same text embedding to the same place twice.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_EMBED_MODEL names an embedding model"]
async fn embeddings_through_the_gateway_live() {
    let Some(key) = key() else { return };
    let model = model("MINDFORK_OPENROUTER_EMBED_MODEL", "baai/bge-m3");
    let client = gateway(&key, &model);
    let texts = vec![
        "The quick brown fox jumps over the lazy dog.".to_string(),
        "fn main() { println!(\"hello\"); }".to_string(),
        "The quick brown fox jumps over the lazy dog.".to_string(),
    ];
    let vectors = client
        .embed(texts, EmbedRole::Passage)
        .await
        .expect("the embeddings");
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let cosine = |a: &[f32], b: &[f32]| {
        a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / (norm(a) * norm(b))
    };
    eprintln!(
        "[{model}] {} vectors of {}, norms {:?}, same text {:.6}, other text {:.6}",
        vectors.len(),
        vectors[0].len(),
        vectors.iter().map(|v| norm(v)).collect::<Vec<_>>(),
        cosine(&vectors[0], &vectors[2]),
        cosine(&vectors[0], &vectors[1]),
    );
    assert_eq!(vectors.len(), 3);
    assert!(vectors.iter().all(|v| v.len() == vectors[0].len()));
    assert!(vectors.iter().all(|v| (norm(v) - 1.0).abs() < 0.01));
    assert!(cosine(&vectors[0], &vectors[2]) > 0.999, "input order");
    assert!(
        cosine(&vectors[0], &vectors[1]) < 0.9,
        "and not everything alike"
    );
}
