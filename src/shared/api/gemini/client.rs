//! An HTTP client to the native Google Gemini API (`POST …:streamGenerateContent?alt=sse`),
//! implementing [`EngineBackend`]. A protocol separate from the OpenAI-compatible Chat
//! Completions path (`OpenAiClient` + the Gemini dialect, now replaced by this client): "thought"
//! summaries (`thinkingConfig.includeThoughts`), depth (`thinkingLevel`/
//! `thinkingBudget`), `thoughtsTokenCount`. See ADR 0004, docs/research/gemini-native-client.md.
//!
//! This client gives no embeddings — RAG in Gemini mode takes a separate source
//! (the OpenAI-compatible `…/v1beta/openai/embeddings`, see the supervisor), like Anthropic.

use anyhow::Result;
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, GenResponse};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, TokenUsage, ToolCallDelta,
    VisionSupport,
};
use crate::shared::api::error::{self, SUBJECT_GEMINI};
use crate::shared::api::http;

/// A client to the native Gemini API.
pub struct GeminiClient {
    http: reqwest::Client,
    /// The base URL with a `/v1beta` suffix (the client appends
    /// `/models/{model}:streamGenerateContent`), e.g.
    /// `https://generativelanguage.googleapis.com/v1beta`.
    base_url: String,
    api_key: String,
    model: String,
}

impl GeminiClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        // The model name may arrive with a `models/` prefix — the path already carries it.
        let model = model
            .into()
            .trim_start_matches("models/")
            .trim()
            .to_string();
        Self {
            http: http::engine_client(),
            base_url,
            api_key: api_key.into(),
            model,
        }
    }
}

/// Gemini's string `finishReason` → the domain reason. The presence of tool calls
/// (`saw_tool_call`) gives `ToolCalls` even on `STOP` (Gemini returns `STOP` with
/// `functionCall` parts). `MAX_TOKENS` → `Length`; anything else — `Stop`, so as not to fail
/// ([`is_block_reason`] recognizes blocking reasons separately and surfaces a note).
fn map_finish(reason: &str, saw_tool_call: bool) -> FinishReason {
    match reason {
        "MAX_TOKENS" => FinishReason::Length,
        _ if saw_tool_call => FinishReason::ToolCalls,
        _ => FinishReason::Stop,
    }
}

/// A "blocking" finish/rejection reason: a safety filter, recitation,
/// a malformed call, etc. Such a reply arrives empty — without an explanation the user would see
/// a silently empty turn, so it's surfaced as a note (see [`block_note`]).
fn is_block_reason(reason: &str) -> bool {
    matches!(
        reason,
        "SAFETY"
            | "RECITATION"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "MALFORMED_FUNCTION_CALL"
            | "OTHER"
    )
}

/// A note to the user about the block (goes into the feed as reply text, so an empty turn
/// is explainable).
fn block_note(reason: &str) -> String {
    format!("\n⚠ Gemini did not produce a response (reason: {reason}).")
}

#[async_trait::async_trait]
impl EngineBackend for GeminiClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let body = wire::build_request(&req, &self.model);
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url, self.model
        );

        let request = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .json(&body);
        let Some(response) = http::send_cancellable(request, &cancel).await? else {
            return Ok(http::cancelled_stream());
        };
        let response = error::check_status(SUBJECT_GEMINI, response).await?;

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // The ordinal index of the tool call (Gemini has no call_id — synthesize a
            // stable id `"{name}-{index}"`; functionResponse matching goes by it).
            let mut tool_index = 0usize;
            let mut saw_tool_call = false;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        yield ChatChunk::Finished(FinishReason::Cancelled);
                        break;
                    }
                    next = events.next() => {
                        match next {
                            None => {
                                yield ChatChunk::Finished(FinishReason::Stop);
                                break;
                            }
                            Some(Err(err)) => {
                                let message = error::chain_text(&err);
                                tracing::warn!(error = %message, "SSE stream error (gemini)");
                                for chunk in ChatChunk::failure(message, true) { yield chunk; }
                                break;
                            }
                            Some(Ok(event)) => {
                                if event.data == "[DONE]" {
                                    yield ChatChunk::Finished(FinishReason::Stop);
                                    break;
                                }
                                let resp: GenResponse = match serde_json::from_str(&event.data) {
                                    Ok(r) => r,
                                    Err(err) => {
                                        tracing::warn!(error = %err, data = %event.data, "failed to parse gemini SSE chunk");
                                        continue;
                                    }
                                };
                                // An error reported inside the stream. Checked before
                                // anything else: every other field of GenResponse
                                // defaults, so this payload is otherwise a valid
                                // empty chunk and the turn ended as a silent Stop.
                                if let Some(e) = resp.error {
                                    let transient = error::stream_error_transient(&e.status, e.code);
                                    tracing::warn!(
                                        status = %e.status,
                                        code = ?e.code,
                                        transient,
                                        message = %e.message,
                                        "gemini reported an error inside the stream"
                                    );
                                    let message = error::stream_error_text(&e.status, &e.message);
                                    for chunk in ChatChunk::failure(message, transient) { yield chunk; }
                                    break;
                                }
                                // The prompt was blocked by the filter (candidates is empty) —
                                // explain via a note, otherwise it would look like an empty STOP.
                                if let Some(reason) = resp
                                    .prompt_feedback
                                    .and_then(|f| f.block_reason)
                                    .filter(|r| !r.is_empty())
                                {
                                    tracing::warn!(reason = %reason, "gemini blocked the prompt");
                                    yield ChatChunk::Text(block_note(&reason));
                                    yield ChatChunk::Finished(FinishReason::Stop);
                                    break;
                                }
                                let candidate = resp.candidates.into_iter().next();
                                if let Some(cand) = &candidate
                                    && let Some(content) = &cand.content
                                {
                                    for part in &content.parts {
                                        if let Some(fc) = &part.function_call {
                                            saw_tool_call = true;
                                            yield ChatChunk::ToolCall(ToolCallDelta {
                                                index: tool_index,
                                                id: Some(format!("{}-{}", fc.name, tool_index)),
                                                name: Some(fc.name.clone()),
                                                arguments: fc.args.to_string(),
                                                // The thought signature (Gemini 3) — on the functionCall part.
                                                thought_signature: part.thought_signature.clone(),
                                            });
                                            tool_index += 1;
                                        } else if let Some(text) = &part.text {
                                            if text.is_empty() {
                                                continue;
                                            }
                                            if part.thought == Some(true) {
                                                yield ChatChunk::Thoughts(text.clone());
                                            } else {
                                                yield ChatChunk::Text(text.clone());
                                            }
                                        }
                                    }
                                }
                                if let Some(u) = resp.usage_metadata {
                                    yield ChatChunk::Usage(TokenUsage {
                                        prompt_tokens: u.prompt_token_count,
                                        completion_tokens: u.candidates_token_count,
                                        reasoning_tokens: u.thoughts_token_count,
                                    });
                                }
                                if let Some(reason) = candidate.and_then(|c| c.finish_reason) {
                                    // A blocking reason (SAFETY/RECITATION/…) — surfaced as
                                    // a note, otherwise an empty turn would go unexplained.
                                    if is_block_reason(&reason) && !saw_tool_call {
                                        tracing::warn!(reason = %reason, "gemini stopped with a block reason");
                                        yield ChatChunk::Text(block_note(&reason));
                                    }
                                    yield ChatChunk::Finished(map_finish(&reason, saw_tool_call));
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    /// Gemini takes images on every current model, so the answer is static.
    ///
    /// Deliberately **not** a model-name allowlist: a hardcoded list of vision
    /// models goes stale the week after it is written and then lies confidently —
    /// the trap docs/research/grok-xai-provider.md recorded for reasoning detection.
    /// A genuinely text-only model returns a clear provider error on send, which is
    /// a far better failure than refusing an attach on a guess.
    async fn vision(&self) -> VisionSupport {
        VisionSupport::Supported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_mapping() {
        assert_eq!(map_finish("STOP", false), FinishReason::Stop);
        assert_eq!(map_finish("STOP", true), FinishReason::ToolCalls);
        assert_eq!(map_finish("MAX_TOKENS", false), FinishReason::Length);
        assert_eq!(map_finish("SAFETY", false), FinishReason::Stop);
    }

    #[test]
    fn strips_models_prefix_from_model() {
        let c = GeminiClient::new("https://x/v1beta", "k", "models/gemini-2.5-flash");
        assert_eq!(c.model, "gemini-2.5-flash");
    }

    #[test]
    fn block_reasons_recognized_and_noted() {
        assert!(is_block_reason("SAFETY"));
        assert!(is_block_reason("RECITATION"));
        assert!(is_block_reason("MALFORMED_FUNCTION_CALL"));
        assert!(!is_block_reason("STOP"));
        assert!(!is_block_reason("MAX_TOKENS"));
        // The note carries the reason.
        assert!(block_note("SAFETY").contains("SAFETY"));
    }
}

/// A manual smoke against the real Gemini API. Marked `#[ignore]` — not in CI.
/// Run: `MINDFORK_GEMINI_KEY=… cargo test gemini -- --ignored --nocapture`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::ToolCallAccumulator;
    use crate::shared::api::contract::{ApiMessage, ToolSchema};

    fn client_from_env() -> Option<GeminiClient> {
        let key = std::env::var("MINDFORK_GEMINI_KEY").ok()?;
        let model =
            std::env::var("MINDFORK_GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
        Some(GeminiClient::new(
            "https://generativelanguage.googleapis.com/v1beta",
            key,
            model,
        ))
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: Some("You are a helpful assistant.".into()),
            messages: vec![ApiMessage::user("Reply with exactly: pong")],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut text = String::new();
        let mut finish = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Finished(r) => {
                    finish = Some(r);
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert!(!text.is_empty(), "expected non-empty response");
        assert!(matches!(
            finish,
            Some(FinishReason::Stop | FinishReason::Length)
        ));
    }

    /// The **fallback** an image in a tool result takes on Gemini, and the only place it
    /// is exercised end to end (fork F1-A of docs/research/mcp-tool-images.md).
    ///
    /// Gemini is the one provider that refuses a multimodal `functionResponse` — measured,
    /// a hard `400` "Multimodal function responses are not supported for this model" —
    /// so its builder emits the image as user parts *after* the response instead. This
    /// test is what proves that detour still reaches the model, and that the request is
    /// accepted at all: a regression putting the image back inside the `functionResponse`
    /// would fail here with that 400 rather than silently degrade.
    ///
    /// Control arm included, for the reason recorded in §2.1 of that document.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn tool_result_image_takes_the_user_part_fallback() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let turn = |with_image: bool| {
            let tool = ApiMessage::tool("take_screenshot-0", "Screenshot taken.");
            let tool = if with_image {
                tool.with_images(vec![crate::shared::api::ApiImage::new(
                    "image/png",
                    &crate::shared::api::green_circle_png_base64(),
                    None,
                )])
            } else {
                tool
            };
            ChatRequest {
                continue_final: false,
                system: None,
                // The shape a real turn has: the question is asked **up front**, and the
                // tool result is the last message — the model answers from it. A trailing
                // user message would be unrealistic *and* wrong here: Gemini puts a tool
                // result in a `user` content and adjacent same-role contents merge, so the
                // question would end up sharing one content with the `functionResponse` —
                // measured, Gemini then returns an empty candidate (`STOP`, zero
                // completion tokens) while still billing the image.
                messages: vec![
                    ApiMessage::user(crate::shared::api::TOOL_VISION_PROMPT),
                    ApiMessage::assistant_tool_calls(
                        "",
                        vec![crate::shared::api::ApiToolCall {
                            id: "take_screenshot-0".into(),
                            name: "take_screenshot".into(),
                            arguments: "{}".into(),
                            thought_signature: None,
                        }],
                    ),
                    tool,
                ],
                sampling: SamplingConfig {
                    max_tokens: Some(2048),
                    // Thinking muted, or there is no answer to assert on: 2.5-flash
                    // thinks by default and the budget is **shared** with the reply, so
                    // both arms come back empty and the failure reads as "the image did
                    // not arrive" (docs/lessons.md §3 — measured here first).
                    reasoning_budget: Some(0),
                    ..Default::default()
                },
                tools: vec![crate::shared::api::ToolSchema {
                    name: "take_screenshot".into(),
                    description: "Take a screenshot of the screen.".into(),
                    parameters: serde_json::json!({ "type": "object", "properties": {} }),
                }],
            }
        };
        let read = async |req| {
            let mut stream: ChatStream = client.chat_stream(req, Default::default()).await.unwrap();
            let mut text = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
                    ChatChunk::Finished(_) => break,
                    _ => {}
                }
            }
            text
        };

        let control = read(turn(false)).await;
        eprintln!("gemini control (no image): {control}");
        crate::shared::api::assert_sees_green_circle(&control, false, "control");

        let answer = read(turn(true)).await;
        eprintln!("gemini tool-result image: {answer}");
        crate::shared::api::assert_sees_green_circle(&answer, true, "with the image");
    }

    /// Image input (spec §9.10): an `inline_data` part reaches the model and is
    /// described. Verified live before the wire was written — see
    /// docs/research/multimodal-images.md §2.2.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn image_input_is_described() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::user(crate::shared::api::VISION_PROMPT).with_images(vec![
                    crate::shared::api::ApiImage::new(
                        "image/png",
                        &crate::shared::api::blue_square_png_base64(),
                        None,
                    ),
                ]),
            ],
            // 2.5-flash thinks by default and the budget is shared with the reply, so a
            // small cap here would return an empty answer rather than a wrong one.
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
                ChatChunk::Finished(_) => break,
                _ => {}
            }
        }
        eprintln!("gemini vision reply: {text}");
        crate::shared::api::assert_sees_blue_square(&text, "gemini");
    }

    /// Reasoning summary: with `thinking=true`, "thoughts" (Thoughts) and the reply arrive.
    /// `max_tokens` is generous — thought tokens eat into the reply budget.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn thinking_streams_thoughts() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step: what is 17 * 23? Show brief reasoning.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(4096),
                thinking: Some(true),
                reasoning_effort: Some(ReasoningEffort::Medium),
                ..Default::default()
            },
            tools: vec![],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut thoughts = String::new();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Finished(_) => break,
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert!(
            !text.is_empty(),
            "expected final answer, thoughts={thoughts:?}"
        );
    }

    /// One tool round: the model calls the tool (without resending the signature — Phase B).
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn single_tool_call() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let tool = ToolSchema {
            name: "get_weather".into(),
            description: "Get the current weather for a city.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            }),
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("Call get_weather for Paris.")],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![tool],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut acc = ToolCallAccumulator::default();
        let mut reason = FinishReason::Stop;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Finished(r) => {
                    reason = r;
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert_eq!(
            reason,
            FinishReason::ToolCalls,
            "model should call the tool"
        );
        let calls = acc.finish();
        assert!(!calls.is_empty(), "expected a tool call");
        assert_eq!(calls[0].name, "get_weather");
    }

    /// Phase B: a tool-use round-trip resending the thought signature. On **Gemini 3**
    /// (`MINDFORK_GEMINI_MODEL=gemini-3-*`) the first round gives a call + `thoughtSignature`;
    /// the second resends it on `functionCall` + the result — Gemini must not return
    /// `400` ("missing thought_signature"). On 2.5 the signature is optional (the round also
    /// goes through). Checks: the signature arrived and resending doesn't break the round.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API), Gemini 3 for signatures"]
    async fn tool_use_round_trips_signature() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let tool = ToolSchema {
            name: "get_weather".into(),
            description: "Get the current weather for a city.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            }),
        };
        let sampling = SamplingConfig {
            max_tokens: Some(4096),
            thinking: Some(true),
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        };
        let prompt = "Reason briefly which of Paris or Berlin is the capital of France, \
             then call get_weather for that city.";
        let round1 = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(prompt)],
            sampling: sampling.clone(),
            tools: vec![tool.clone()],
        };
        let mut stream = client
            .chat_stream(round1, Default::default())
            .await
            .unwrap();
        let mut acc = ToolCallAccumulator::default();
        let mut reason = FinishReason::Stop;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Finished(r) => {
                    reason = r;
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert_eq!(
            reason,
            FinishReason::ToolCalls,
            "model should call the tool"
        );
        let calls = acc.finish();
        assert!(!calls.is_empty(), "expected a tool call");
        let call = calls[0].clone();
        eprintln!(
            "thought_signature present: {}",
            call.thought_signature.is_some()
        );

        // Second round: assistant(functionCall with the signature) + the result.
        let round2 = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::user(prompt),
                ApiMessage::assistant_tool_calls("", vec![call.clone()]),
                ApiMessage::tool(&call.id, "18°C, sunny"),
            ],
            sampling,
            tools: vec![tool],
        };
        let mut stream = client
            .chat_stream(round2, Default::default())
            .await
            .unwrap();
        let mut text = String::new();
        let mut finish = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Finished(r) => {
                    finish = Some(r);
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert!(
            matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
            "second round must succeed (no 400), got {finish:?}"
        );
        assert!(
            !text.is_empty(),
            "expected a final answer after tool result"
        );
    }
}
