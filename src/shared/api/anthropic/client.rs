//! An HTTP client to the Anthropic Messages API (`platform.claude.com`), implementing
//! [`EngineBackend`]. A protocol separate from OpenAI (`/v1/messages`, headers
//! `x-api-key` + `anthropic-version`, event-based SSE). See ADR 0004, Phase 2.
//!
//! Anthropic has no embeddings — [`Embedder`](super::super::contract::Embedder) isn't
//! implemented here (RAG uses a separate embedder, ADR 0002).

use anyhow::Result;
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, AntDelta, AntStartBlock, AntStreamEvent};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, ThinkingRef, TokenUsage,
    ToolCallDelta, VisionSupport,
};
use crate::shared::api::error::{self, SUBJECT_ANTHROPIC};
use crate::shared::api::http;

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
            http: http::engine_client(),
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

        let request = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body);
        let Some(response) = http::send_cancellable(request, &cancel).await? else {
            return Ok(http::cancelled_stream());
        };
        let response = error::check_status(SUBJECT_ANTHROPIC, response).await?;

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
                                let message = error::chain_text(&err);
                                tracing::warn!(error = %message, "SSE stream error (anthropic)");
                                for chunk in ChatChunk::failure(message, true) { yield chunk; }
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
                                    // The documented way a failure arrives once the
                                    // stream is open: `overloaded_error` here is the
                                    // `529` that would have been the status code a
                                    // moment earlier.
                                    Ok(AntStreamEvent::Error { error: e }) => {
                                        let transient = error::stream_error_transient(&e.name, None);
                                        tracing::warn!(
                                            name = %e.name,
                                            transient,
                                            message = %e.message,
                                            "anthropic reported an error inside the stream"
                                        );
                                        let message = error::stream_error_text(&e.name, &e.message);
                                        for chunk in ChatChunk::failure(message, transient) { yield chunk; }
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

    /// Claude takes images on every current model, so the answer is static.
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

    /// Serves one canned `text/event-stream` response on a local port and returns its
    /// base URL.
    ///
    /// A real socket and a real SSE body, so the whole client is exercised —
    /// `eventsource` framing, the wire enum, the chunks that come out — rather than
    /// serde alone. The alternative (asserting on `AntStreamEvent` only) is what let
    /// the swallowed `error` event survive: the type parsed fine, it was the client
    /// that dropped it.
    fn serve_sse(events: &'static [&'static str]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            // Read the request head so the client's write completes.
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let mut body = String::new();
            for e in events {
                body.push_str(&format!("data: {e}\n\n"));
            }
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = sock.flush();
        });
        format!("http://127.0.0.1:{port}")
    }

    async fn collect(stream: ChatStream) -> Vec<ChatChunk> {
        let mut out = Vec::new();
        let mut stream = stream;
        while let Some(c) = stream.next().await {
            let last = matches!(c, ChatChunk::Finished(_));
            out.push(c);
            if last {
                break;
            }
        }
        out
    }

    /// The headline defect: Anthropic sheds load *after* accepting the stream, and
    /// that event used to fall into `AntStreamEvent::Other` — so the connection then
    /// closed, the `None` arm reported `Finished(Stop)`, and an overloaded
    /// truncation was byte-for-byte a normal completion.
    #[tokio::test]
    async fn an_overloaded_error_mid_stream_ends_the_turn_as_an_error() {
        let base = serve_sse(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Half an ans"}}"#,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ]);
        let client = AnthropicClient::new(base, "k", "claude-x");
        let req = ChatRequest {
            system: None,
            messages: vec![crate::shared::api::contract::ApiMessage::user("hi")],
            sampling: Default::default(),
            tools: vec![],
        };
        let chunks = collect(client.chat_stream(req, Default::default()).await.unwrap()).await;

        assert!(
            chunks.contains(&ChatChunk::Text("Half an ans".into())),
            "the partial text still arrives: {chunks:?}"
        );
        let Some(ChatChunk::Error { message, transient }) = chunks
            .iter()
            .find(|c| matches!(c, ChatChunk::Error { .. }))
            .cloned()
        else {
            panic!("the error must reach the consumer, not just the log: {chunks:?}");
        };
        assert!(message.contains("overloaded_error"), "{message}");
        assert!(transient, "an overloaded provider is worth another attempt");
        assert_eq!(
            chunks.last(),
            Some(&ChatChunk::Finished(FinishReason::Error)),
            "and the turn must not end as a plain Stop: {chunks:?}"
        );
    }

    /// The other half of the same claim: a stream that ends normally must still end
    /// as `Stop`, with no error chunk invented.
    #[tokio::test]
    async fn a_clean_stream_still_finishes_without_an_error_chunk() {
        let base = serve_sse(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"pong"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
        ]);
        let client = AnthropicClient::new(base, "k", "claude-x");
        let req = ChatRequest {
            system: None,
            messages: vec![crate::shared::api::contract::ApiMessage::user("hi")],
            sampling: Default::default(),
            tools: vec![],
        };
        let chunks = collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        assert!(
            !chunks.iter().any(|c| matches!(c, ChatChunk::Error { .. })),
            "{chunks:?}"
        );
        assert_eq!(
            chunks.last(),
            Some(&ChatChunk::Finished(FinishReason::Stop))
        );
    }

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

    /// Image input (spec §9.10): a base64 `image` block reaches the model and is
    /// described. Verified live before the wire was written — see
    /// docs/research/multimodal-images.md §2.2.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ANTHROPIC_KEY (live Anthropic API)"]
    async fn image_input_is_described() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
            return;
        };
        let req = ChatRequest {
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
            sampling: SamplingConfig {
                max_tokens: Some(256),
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
        eprintln!("anthropic vision reply: {text}");
        crate::shared::api::assert_sees_blue_square(&text, "anthropic");
    }

    /// Phase A: with thinking enabled, extended thinking runs and its signature
    /// arrives; the visible **summary is best-effort**.
    ///
    /// The original assertion — `display:"summarized"` implies non-empty
    /// "thoughts" — turned out not to be an invariant, and it had been failing:
    /// **measured** on `claude-opus-4-8` (2026-08-12, five runs, clean streams with
    /// no error chunk) the turn comes back `thoughts=0 chars, signature=380 chars,
    /// text=50 chars`. A signature that long only exists for a real thinking block,
    /// so thinking *did* happen — Anthropic simply summarized a short one to
    /// nothing. The deterministic claims are therefore the signature (which is also
    /// what the tool-use round-trip depends on, and which is empty if `thinking` is
    /// ever dropped from the request or `signature_delta` stops being parsed) and
    /// the answer itself; the summary is logged, not asserted. Same shape as the
    /// OpenAI Responses sibling, whose comment already said "on a trivial task the
    /// summary might be absent".
    ///
    /// `reasoning_effort: High` is kept for the reason its tool-use sibling states:
    /// `thinking:{type:"adaptive"}` lets the model decide *whether* to think, and
    /// without the nudge a trivial prompt can skip reasoning altogether — which
    /// would empty the signature too, and that assertion is the point of this smoke.
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
                reasoning_effort: Some(crate::entities::sampling::ReasoningEffort::High),
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
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        eprintln!(
            "thoughts={} chars, signature={} chars, text={} chars",
            thoughts.len(),
            signature.len(),
            text.len()
        );
        if thoughts.is_empty() {
            eprintln!("note: the provider delivered no thinking summary for this turn");
        }
        assert!(
            !signature.is_empty(),
            "expected a thinking signature — extended thinking did not run \
             (thoughts={} chars, text={} chars)",
            thoughts.len(),
            text.len()
        );
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
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
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
