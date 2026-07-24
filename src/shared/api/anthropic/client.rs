//! An HTTP client to the Anthropic Messages API (`platform.claude.com`), implementing
//! [`EngineBackend`]. A protocol separate from OpenAI (`/v1/messages`, headers
//! `x-api-key` + `anthropic-version`, event-based SSE). See ADR 0004, Phase 2.
//!
//! Anthropic has no embeddings — [`Embedder`](super::super::contract::Embedder) isn't
//! implemented here (RAG uses a separate embedder, ADR 0002).

use anyhow::{Context, Result};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, AntDelta, AntStartBlock, AntStreamEvent};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, ThinkingRef, TokenUsage,
    ToolCallDelta,
};

/// The Anthropic API version (the mandatory `anthropic-version` header).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// A client to the Anthropic Messages API.
pub struct AnthropicClient {
    http: reqwest::Client,
    /// The base URL with no suffix (the client appends `/v1/messages`), e.g.
    /// `https://api.anthropic.com`.
    base_url: String,
    api_key: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            http: reqwest::Client::new(),
            base_url,
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

/// Maps Anthropic's `stop_reason` to the domain [`FinishReason`].
fn map_stop_reason(s: &str) -> FinishReason {
    match s {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "max_tokens" => FinishReason::Length,
        "tool_use" => FinishReason::ToolCalls,
        _ => FinishReason::Stop,
    }
}

#[async_trait::async_trait]
impl EngineBackend for AnthropicClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let body = wire::build_request(&req, &self.model, true);
        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        // Don't swallow the error body (like OpenAiClient): Anthropic puts the reason in JSON
        // (`{"error":{"message":...}}`) — log it and surface it in the error text.
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.trim().chars().take(500).collect();
            tracing::warn!(%status, body = %detail, "anthropic returned an error status");
            if detail.is_empty() {
                anyhow::bail!("движок (Anthropic) вернул статус {status}");
            }
            anyhow::bail!("движок (Anthropic) вернул статус {status}: {detail}");
        }

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // input_tokens arrive in message_start, output_tokens — in message_delta.
            let mut input_tokens = 0u32;
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
                                tracing::warn!(error = %err, "SSE stream error (anthropic)");
                                yield ChatChunk::Finished(FinishReason::Error);
                                break;
                            }
                            Some(Ok(event)) => {
                                match serde_json::from_str::<AntStreamEvent>(&event.data) {
                                    Ok(AntStreamEvent::MessageStart { message }) => {
                                        if let Some(u) = message.usage {
                                            input_tokens = u.input_tokens;
                                        }
                                    }
                                    Ok(AntStreamEvent::ContentBlockStart {
                                        index,
                                        content_block: AntStartBlock::ToolUse { id, name },
                                    }) => {
                                        yield ChatChunk::ToolCall(ToolCallDelta { thought_signature: None,
                                            index,
                                            id: Some(id),
                                            name: Some(name),
                                            arguments: String::new(),
                                        });
                                    }
                                    Ok(AntStreamEvent::ContentBlockStart { .. }) => {}
                                    Ok(AntStreamEvent::ContentBlockDelta { index, delta }) => match delta {
                                        AntDelta::TextDelta { text } if !text.is_empty() => {
                                            yield ChatChunk::Text(text);
                                        }
                                        AntDelta::ThinkingDelta { thinking } if !thinking.is_empty() => {
                                            yield ChatChunk::Thoughts(thinking);
                                        }
                                        AntDelta::SignatureDelta { signature } if !signature.is_empty() => {
                                            yield ChatChunk::ThoughtsSignature(ThinkingRef {
                                                id: None,
                                                signature,
                                            });
                                        }
                                        AntDelta::InputJsonDelta { partial_json } => {
                                            yield ChatChunk::ToolCall(ToolCallDelta { thought_signature: None,
                                                index,
                                                id: None,
                                                name: None,
                                                arguments: partial_json,
                                            });
                                        }
                                        _ => {}
                                    },
                                    Ok(AntStreamEvent::MessageDelta { delta, usage }) => {
                                        let completion = usage.map(|u| u.output_tokens).unwrap_or(0);
                                        yield ChatChunk::Usage(TokenUsage {
                                            prompt_tokens: input_tokens,
                                            completion_tokens: completion,
                                            // Anthropic doesn't separate reasoning tokens — "thoughts"
                                            // are already counted in output_tokens.
                                            reasoning_tokens: 0,
                                        });
                                        let reason = delta
                                            .stop_reason
                                            .as_deref()
                                            .map(map_stop_reason)
                                            .unwrap_or(FinishReason::Stop);
                                        yield ChatChunk::Finished(reason);
                                        break;
                                    }
                                    Ok(AntStreamEvent::Other) => {}
                                    Err(err) => {
                                        tracing::warn!(error = %err, data = %event.data, "failed to parse anthropic SSE chunk");
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reason_mapping() {
        assert_eq!(map_stop_reason("end_turn"), FinishReason::Stop);
        assert_eq!(map_stop_reason("stop_sequence"), FinishReason::Stop);
        assert_eq!(map_stop_reason("max_tokens"), FinishReason::Length);
        assert_eq!(map_stop_reason("tool_use"), FinishReason::ToolCalls);
        assert_eq!(map_stop_reason("weird"), FinishReason::Stop);
    }
}

/// A manual smoke against the real Anthropic API. Marked `#[ignore]` — doesn't run in CI.
/// Run: `MINDFORK_ANTHROPIC_KEY=sk-ant-... cargo test anthropic -- --ignored`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ThinkingBlock, ToolSchema};

    fn client_from_env() -> Option<AnthropicClient> {
        let key = std::env::var("MINDFORK_ANTHROPIC_KEY").ok()?;
        let model =
            std::env::var("MINDFORK_ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-opus-4-8".into());
        Some(AnthropicClient::new(
            "https://api.anthropic.com",
            key,
            model,
        ))
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_ANTHROPIC_KEY (live Anthropic API)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
            return;
        };
        let req = ChatRequest {
            system: Some("You are a helpful assistant.".into()),
            messages: vec![ApiMessage::user("Reply with exactly: pong")],
            sampling: SamplingConfig {
                max_tokens: Some(64),
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
                _ => {}
            }
        }
        assert!(!text.is_empty(), "expected non-empty response");
        assert!(matches!(
            finish,
            Some(FinishReason::Stop | FinishReason::Length)
        ));
    }

    /// Phase A: with thinking enabled, both "thoughts" and their signature arrive.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ANTHROPIC_KEY (live Anthropic API)"]
    async fn extended_thinking_streams_thoughts_and_signature() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
            return;
        };
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step: what is 17 * 23? Show brief reasoning.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                thinking: Some(true),
                ..Default::default()
            },
            tools: vec![],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut thoughts = String::new();
        let mut signature = String::new();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::ThoughtsSignature(r) => signature.push_str(&r.signature),
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Finished(_) => break,
                _ => {}
            }
        }
        // display:summarized → non-empty "thoughts"; the signature is present.
        assert!(!thoughts.is_empty(), "expected summarized thoughts");
        assert!(!signature.is_empty(), "expected thinking signature");
        assert!(!text.is_empty(), "expected final answer");
    }

    /// Phase B: thinking + tool-use. The first round gives "thoughts"+a signature+a call; the second
    /// request resends the thinking block (with the signature) on an assistant turn with tool_use and
    /// the tool result — Anthropic must not return 400 (which is exactly what checks the signature).
    #[tokio::test]
    #[ignore = "requires MINDFORK_ANTHROPIC_KEY (live Anthropic API)"]
    async fn thinking_with_tool_use_round_trips_signature() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
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
            max_tokens: Some(2048),
            thinking: Some(true),
            // High effort: nudge the model to actually think before the
            // call (otherwise on a trivial request adaptive thinking might
            // skip the reasoning — and there'd be no signature).
            reasoning_effort: Some(crate::entities::sampling::ReasoningEffort::High),
            ..Default::default()
        };
        // A prompt with an explicit reasoning step (picking a city) — so the model
        // generates a thinking block with a signature before calling the tool.
        let prompt = "Two candidate cities: Paris and Berlin. Reason briefly about \
             which one is the capital of France, then call the get_weather tool for \
             that city.";
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
        let mut thoughts = String::new();
        let mut signature = String::new();
        let mut acc = crate::shared::api::ToolCallAccumulator::default();
        let mut reason = FinishReason::Stop;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::ThoughtsSignature(r) => signature.push_str(&r.signature),
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Finished(r) => {
                    reason = r;
                    break;
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
        assert!(!signature.is_empty(), "expected a thinking signature");
        let call = &calls[0];

        // Second round: assistant(thinking+signature, tool_use) → tool_result.
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
                    text: thoughts.clone(),
                    signature: signature.clone(),
                    id: None,
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
                _ => {}
            }
        }
        // If the signature hadn't been resent/were invalid, Anthropic would return 400
        // and chat_stream would yield Finished(Error) with empty text.
        assert!(
            matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
            "second round must succeed (signature round-trip), got {finish:?}"
        );
        assert!(
            !text.is_empty(),
            "expected a final answer after tool result"
        );
    }
}
