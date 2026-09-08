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
                                                prefill: None,
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
                                                prefill: None,
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
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ThinkingBlock, ToolSchema};
    use crate::shared::api::{ThinkingAccumulator, ToolCallAccumulator};

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

    /// Image input (spec §9.10): an `input_image` part reaches the model and is
    /// described. Verified live before the wire was written — see
    /// docs/research/multimodal-images.md §2.2.
    #[tokio::test]
    #[ignore = "requires MINDFORK_OPENAI_KEY (live OpenAI Responses API)"]
    async fn image_input_is_described() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_OPENAI_KEY not set");
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
            // Generous: a reasoning model spends this budget before any text, and a cap
            // that starves it reads as "the image was not seen" (docs/lessons.md §3).
            sampling: SamplingConfig {
                max_tokens: Some(4096),
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
        eprintln!("openai vision reply: {text}");
        crate::shared::api::assert_sees_blue_square(&text, "openai");
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
            continue_final: false,
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
            continue_final: false,
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
                .with_thinking_blocks(vec![ThinkingBlock {
                    text: String::new(),
                    signature: enc,
                    id: thinking_id,
                }]),
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

    /// The next round of a request, as the app would send it: `Ok((text, calls))`
    /// — the reply's text and how many tool calls it made — or the engine's
    /// refusal (a status error before the stream, or an error chunk).
    async fn next_round(
        client: &ResponsesClient,
        req: ChatRequest,
    ) -> Result<(String, usize), String> {
        let mut stream = client
            .chat_stream(req, Default::default())
            .await
            .map_err(|e| format!("{e:#}"))?;
        let mut text = String::new();
        let mut acc = ToolCallAccumulator::default();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Error { message, .. } => return Err(message),
                ChatChunk::Finished(_) => break,
                _ => {}
            }
        }
        Ok((text, acc.finish().len()))
    }

    /// The multi-item echo (docs/journal/engine.md): gpt-5.6 answers a
    /// reasoning-heavy brief with two to five reasoning items in about half its
    /// replies, and every one must go back as its own item, in order. Streams the
    /// brief that produced the shape in the field — verbatim, with the search tool
    /// the sub-agent had — until such a reply arrives (eight tries at most: one item
    /// eight times in a row is a 0.4% event at the measured rate), then sends the
    /// next request through the app's own wire with the blocks the loop's
    /// accumulator produced, and — the control arm — the fused shape the loop
    /// produced before the fix (one item, the last id, every ciphertext
    /// concatenated), which the API rejected with `invalid_encrypted_content`
    /// when this was written.
    /// Run: `MINDFORK_OPENAI_KEY=... MINDFORK_OPENAI_MODEL=gpt-5.6 cargo test
    /// several_reasoning_items -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "requires MINDFORK_OPENAI_KEY (live OpenAI Responses API)"]
    async fn several_reasoning_items_round_trip_each_as_its_own_item() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_OPENAI_KEY not set");
            return;
        };
        let tool = ToolSchema {
            name: "web_search".into(),
            description: "Search the web: a list of results with title, url and snippet. \
                          Several independent queries go in one reply as several calls."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string", "description": "The search query."}},
                "required": ["query"],
            }),
        };
        let sampling = SamplingConfig {
            max_tokens: Some(8192),
            thinking: Some(true),
            reasoning_effort: Some(ReasoningEffort::Medium),
            ..Default::default()
        };
        // The sub-agent brief that produced the shape in the field, verbatim: a
        // Russian brief with a search tool at hand (the tool-less English probes of
        // the same day gave one item per reply).
        let system = "Ты — исследователь феноменологии Гуссерля. Анализируй строго, различая  \
            собственные тексты Гуссерля, обоснованную реконструкцию и спекуляцию. Не  \
            приписывай философу знакомства с квантовой механикой или тезисом  \
            квантового бессмертия. Пиши по-русски, содержательно и без театральной  \
            имитации его голоса.";
        let prompt = "Разбери идею квантового бессмертия с позиций философии Эдмунда Гуссерля.  \
            Сначала кратко и точно определи сам мысленный эксперимент и его спорные  \
            физические предпосылки (многомировая интерпретация, квантовое  \
            самоубийство, антропный/селекционный эффект). Затем исследуй через  \
            эпохе, трансцендентальную субъективность, внутреннее сознание времени,  \
            конституирование собственного тела и смерти, интерсубъективность.  \
            Ответь: может ли невозможность пережить собственное небытие служить  \
            аргументом за субъективное бессмертие? Где происходит подмена между  \
            феноменологической неданностью смерти и онтологическим продолжением  \
            жизни? Дай структурированный вывод и обозначь пределы реконструкции.";
        let mut refs = Vec::new();
        let mut calls = Vec::new();
        let mut text = String::new();
        for attempt in 1..=8 {
            let round1 = ChatRequest {
                continue_final: false,
                system: Some(system.into()),
                messages: vec![ApiMessage::user(prompt)],
                sampling: sampling.clone(),
                tools: vec![tool.clone()],
            };
            let mut stream = client
                .chat_stream(round1, Default::default())
                .await
                .unwrap();
            let mut thinking = ThinkingAccumulator::default();
            let mut acc = ToolCallAccumulator::default();
            text.clear();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    ChatChunk::ToolCall(d) => acc.push(d),
                    ChatChunk::ThoughtsSignature(r) => thinking.push(r),
                    ChatChunk::Error { message, .. } => panic!("round 1 failed: {message}"),
                    ChatChunk::Finished(_) => break,
                    _ => {}
                }
            }
            refs = thinking.finish();
            calls = acc.finish();
            eprintln!(
                "attempt {attempt}: {} reasoning item(s), {} call(s)",
                refs.len(),
                calls.len()
            );
            if refs.len() >= 2 {
                break;
            }
        }
        assert!(
            refs.len() >= 2,
            "eight replies with a single reasoning item — rerun (a 0.4% event at the measured rate)"
        );
        assert!(
            refs.iter().all(|r| r.id.is_some()),
            "every Responses reasoning item carries an id"
        );

        // The next request as the loop builds it: the assistant turn with its
        // blocks, then the tool outputs (stubs) — or, for a text reply, a follow-up.
        let history = |thinking: Vec<ThinkingBlock>| {
            let mut messages = vec![ApiMessage::user(prompt)];
            if calls.is_empty() {
                messages.push(ApiMessage::assistant(text.clone()).with_thinking_blocks(thinking));
                messages.push(ApiMessage::user(
                    "Спасибо. Теперь одним абзацем: в чём главный разрыв?",
                ));
            } else {
                messages.push(
                    ApiMessage::assistant_tool_calls(text.clone(), calls.clone())
                        .with_thinking_blocks(thinking),
                );
                for c in &calls {
                    messages.push(ApiMessage::tool(
                        &c.id,
                        "Результаты поиска (1): 1. заглушка — https://example.org — фрагмент.",
                    ));
                }
            }
            ChatRequest {
                continue_final: false,
                system: Some(system.into()),
                messages,
                sampling: sampling.clone(),
                tools: vec![tool.clone()],
            }
        };

        // The fixed path: every item, in order, each under its own id.
        let fixed: Vec<ThinkingBlock> = refs
            .iter()
            .map(|r| ThinkingBlock {
                text: String::new(),
                signature: r.signature.clone(),
                id: r.id.clone(),
            })
            .collect();
        let (answer, more_calls) = next_round(&client, history(fixed))
            .await
            .unwrap_or_else(|e| panic!("the multi-item echo was rejected: {e}"));
        assert!(
            !answer.is_empty() || more_calls > 0,
            "expected an answer or another round of calls after the echo"
        );
        eprintln!(
            "fixed path: {} items echoed after {} call(s); next reply: {} chars, {} call(s)",
            refs.len(),
            calls.len(),
            answer.len(),
            more_calls
        );

        // The control arm: the shape the loop produced before the fix. The API
        // rejected it when this was written; should it ever stop, the list stays the
        // documented contract ("pass back all reasoning items, untouched"), so the
        // arm reports rather than fails.
        let fused = vec![ThinkingBlock {
            text: String::new(),
            signature: refs.iter().map(|r| r.signature.as_str()).collect(),
            id: refs.last().and_then(|r| r.id.clone()),
        }];
        match next_round(&client, history(fused)).await {
            Err(e) => {
                assert!(
                    e.contains("invalid_encrypted_content"),
                    "the fused item was refused, but not as invalid_encrypted_content: {e}"
                );
                eprintln!(
                    "control arm: the fused item is rejected (invalid_encrypted_content), as before the fix"
                );
            }
            Ok(_) => eprintln!(
                "control arm: the API now accepts a fused item — the list stays the contract"
            ),
        }
    }
}
