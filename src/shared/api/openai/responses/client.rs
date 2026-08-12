//! An HTTP client to the OpenAI Responses API (`POST /v1/responses`), implementing
//! [`EngineBackend`]. A protocol separate from Chat Completions
//! ([`OpenAiClient`](super::super::OpenAiClient)), same vendor: reasoning summaries,
//! `reasoning.effort`, `text.verbosity`, reasoning items for tool-use. See ADR 0004,
//! docs/research/openai-responses-client.md.
//!
//! Responses has no embeddings — [`Embedder`](crate::shared::api::contract::Embedder)
//! takes a separate source (ADR 0002), like Anthropic.

use anyhow::Result;
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, RespEvent, RespItem};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, ThinkingRef, TokenUsage,
    ToolCallDelta, VisionSupport,
};
use crate::shared::api::error::{self, SUBJECT_RESPONSES};
use crate::shared::api::http;

/// A client to the OpenAI Responses API.
pub struct ResponsesClient {
    http: reqwest::Client,
    /// The base URL with a `/v1` suffix (the client appends `/responses`), e.g.
    /// `https://api.openai.com/v1`.
    base_url: String,
    api_key: String,
    model: String,
}

impl ResponsesClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            http: http::engine_client(),
            base_url,
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

#[async_trait::async_trait]
impl EngineBackend for ResponsesClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let body = wire::build_request(&req, &self.model, true);
        let url = format!("{}/responses", self.base_url);

        let request = self.http.post(&url).bearer_auth(&self.api_key).json(&body);
        let Some(response) = http::send_cancellable(request, &cancel).await? else {
            return Ok(http::cancelled_stream());
        };
        let response = error::check_status(SUBJECT_RESPONSES, response).await?;

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // Whether there was at least one tool call — Responses doesn't send finish_reason,
            // the reason is inferred from a function_call item actually appearing in the stream.
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
                                tracing::warn!(error = %message, "SSE stream error (openai responses)");
                                for chunk in ChatChunk::failure(message, true) { yield chunk; }
                                break;
                            }
                            Some(Ok(event)) => {
                                if event.data == "[DONE]" {
                                    yield ChatChunk::Finished(FinishReason::Stop);
                                    break;
                                }
                                match serde_json::from_str::<RespEvent>(&event.data) {
                                    Ok(RespEvent::OutputTextDelta { delta }) if !delta.is_empty() => {
                                        yield ChatChunk::Text(delta);
                                    }
                                    Ok(
                                        RespEvent::ReasoningSummaryDelta { delta }
                                        | RespEvent::ReasoningTextDelta { delta },
                                    ) if !delta.is_empty() => {
                                        yield ChatChunk::Thoughts(delta);
                                    }
                                    Ok(RespEvent::OutputItemAdded {
                                        output_index,
                                        item: RespItem::FunctionCall { call_id, name },
                                    }) => {
                                        saw_tool_call = true;
                                        yield ChatChunk::ToolCall(ToolCallDelta { thought_signature: None,
                                            index: output_index,
                                            id: Some(call_id),
                                            name: Some(name),
                                            arguments: String::new(),
                                        });
                                    }
                                    Ok(RespEvent::FunctionArgsDelta { output_index, delta }) => {
                                        yield ChatChunk::ToolCall(ToolCallDelta { thought_signature: None,
                                            index: output_index,
                                            id: None,
                                            name: None,
                                            arguments: delta,
                                        });
                                    }
                                    // A reasoning item is done — carries encrypted_content
                                    // (requested via include). Accumulated for resending on tool-use.
                                    Ok(RespEvent::OutputItemDone {
                                        item: RespItem::Reasoning { id, encrypted_content: Some(enc) },
                                        ..
                                    }) => {
                                        yield ChatChunk::ThoughtsSignature(ThinkingRef {
                                            id: Some(id),
                                            signature: enc,
                                        });
                                    }
                                    Ok(RespEvent::Completed { response }) => {
                                        if let Some(u) = response.usage {
                                            yield ChatChunk::Usage(TokenUsage {
                                                prompt_tokens: u.input_tokens,
                                                completion_tokens: u.output_tokens,
                                                reasoning_tokens: u.output_tokens_details.reasoning_tokens,
                                            });
                                        }
                                        let reason = if saw_tool_call {
                                            FinishReason::ToolCalls
                                        } else {
                                            FinishReason::Stop
                                        };
                                        yield ChatChunk::Finished(reason);
                                        break;
                                    }
                                    Ok(RespEvent::Incomplete { response }) => {
                                        // Cut off by a limit (max_output_tokens etc.).
                                        if let Some(u) = response.and_then(|r| r.usage) {
                                            yield ChatChunk::Usage(TokenUsage {
                                                prompt_tokens: u.input_tokens,
                                                completion_tokens: u.output_tokens,
                                                reasoning_tokens: u.output_tokens_details.reasoning_tokens,
                                            });
                                        }
                                        yield ChatChunk::Finished(FinishReason::Length);
                                        break;
                                    }
                                    Ok(RespEvent::Failed { response }) => {
                                        let e = response.and_then(|r| r.error).unwrap_or_default();
                                        let code = e.code.unwrap_or_default();
                                        let transient = error::stream_error_transient(&code, None);
                                        tracing::warn!(
                                            %code,
                                            transient,
                                            message = %e.message,
                                            "openai responses failed mid-stream"
                                        );
                                        let message = error::stream_error_text(&code, &e.message);
                                        for chunk in ChatChunk::failure(message, transient) { yield chunk; }
                                        break;
                                    }
                                    Ok(RespEvent::Error { code, message }) => {
                                        let code = code.unwrap_or_default();
                                        let message = message.unwrap_or_default();
                                        let transient = error::stream_error_transient(&code, None);
                                        tracing::warn!(
                                            %code,
                                            transient,
                                            %message,
                                            "openai responses error event"
                                        );
                                        let message = error::stream_error_text(&code, &message);
                                        for chunk in ChatChunk::failure(message, transient) { yield chunk; }
                                        break;
                                    }
                                    // Other events (created/in_progress/part.added/…) and
                                    // added-reasoning (without encrypted) — ignored.
                                    Ok(_) => {}
                                    Err(err) => {
                                        tracing::warn!(error = %err, data = %event.data, "failed to parse responses SSE chunk");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    /// OpenAI takes images on every current model, so the answer is static.
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

/// A manual smoke against the real OpenAI Responses API. Marked `#[ignore]` — not in CI.
/// Run: `MINDFORK_OPENAI_KEY=sk-... cargo test responses -- --ignored --nocapture`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig, Verbosity};
    use crate::shared::api::ToolCallAccumulator;
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ThinkingBlock, ToolSchema};

    fn client_from_env() -> Option<ResponsesClient> {
        let key = std::env::var("MINDFORK_OPENAI_KEY").ok()?;
        let model = std::env::var("MINDFORK_OPENAI_MODEL").unwrap_or_else(|_| "gpt-5.5".into());
        Some(ResponsesClient::new(
            "https://api.openai.com/v1",
            key,
            model,
        ))
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_OPENAI_KEY (live OpenAI Responses API)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_OPENAI_KEY not set");
            return;
        };
        let req = ChatRequest {
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

    /// Reasoning summary: with `thinking=true`+`verbosity`, "thoughts" (Thoughts)
    /// and the final reply arrive. Requires a reasoning model (gpt-5.x). `max_tokens` is generous —
    /// reasoning tokens eat into the reply budget.
    #[tokio::test]
    #[ignore = "requires MINDFORK_OPENAI_KEY (live OpenAI Responses API)"]
    async fn reasoning_summary_streams_thoughts() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_OPENAI_KEY not set");
            return;
        };
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step: what is 17 * 23? Show brief reasoning.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(4096),
                thinking: Some(true),
                reasoning_effort: Some(ReasoningEffort::Medium),
                verbosity: Some(Verbosity::Low),
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
        // On a trivial task the summary might be absent — check at least the reply.
        assert!(
            !text.is_empty(),
            "expected final answer, thoughts={thoughts:?}"
        );
    }

    /// Tool-use round-trip: the first round gives a call + a reasoning item (id+encrypted);
    /// the second resends the reasoning item before its function_call and the result —
    /// OpenAI must not return an error.
    #[tokio::test]
    #[ignore = "requires MINDFORK_OPENAI_KEY (live OpenAI Responses API)"]
    async fn tool_use_round_trips_reasoning_item() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_OPENAI_KEY not set");
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
        let mut thinking_id = None;
        let mut enc = String::new();
        let mut reason = FinishReason::Stop;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::ThoughtsSignature(r) => {
                    thinking_id = r.id;
                    enc = r.signature;
                }
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
        let call = &calls[0];

        let round2 = ChatRequest {
            system: None,
            messages: vec![
                ApiMessage::user(prompt),
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        thought_signature: None,
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    }],
                )
                .with_thinking(Some(ThinkingBlock {
                    text: String::new(),
                    signature: enc,
                    id: thinking_id,
                })),
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
            "second round must succeed, got {finish:?}"
        );
        assert!(
            !text.is_empty(),
            "expected a final answer after tool result"
        );
    }
}
