//! HTTP-клиент к OpenAI Responses API (`POST /v1/responses`), реализующий
//! [`EngineBackend`]. Отдельный протокол от Chat Completions
//! ([`OpenAiClient`](super::super::OpenAiClient)) того же вендора: резюме рассуждений,
//! `reasoning.effort`, `text.verbosity`, reasoning-элементы для tool-use. См. ADR 0004,
//! docs/research/openai-responses-client.md.
//!
//! Эмбеддингов Responses не имеет — [`Embedder`](crate::shared::api::contract::Embedder)
//! берёт отдельный источник (ADR 0002), как и у Anthropic.

use anyhow::{Context, Result};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, RespEvent, RespItem};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, ThinkingRef, TokenUsage,
    ToolCallDelta,
};

/// Клиент к OpenAI Responses API.
pub struct ResponsesClient {
    http: reqwest::Client,
    /// Базовый URL с суффиксом `/v1` (клиент добавляет `/responses`), напр.
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
            http: reqwest::Client::new(),
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

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        // Не глотаем тело ошибки (как прочие клиенты): OpenAI кладёт причину в JSON
        // (`{"error":{"message":...}}`) — логируем и пробрасываем в текст ошибки.
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.trim().chars().take(500).collect();
            tracing::warn!(%status, body = %detail, "openai responses returned an error status");
            if detail.is_empty() {
                anyhow::bail!("движок (OpenAI Responses) вернул статус {status}");
            }
            anyhow::bail!("движок (OpenAI Responses) вернул статус {status}: {detail}");
        }

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // Был ли хоть один вызов инструмента — Responses не шлёт finish_reason,
            // причину выводим по факту появления function_call-элемента в потоке.
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
                                tracing::warn!(error = %err, "SSE stream error (openai responses)");
                                yield ChatChunk::Finished(FinishReason::Error);
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
                                        yield ChatChunk::ToolCall(ToolCallDelta {
                                            index: output_index,
                                            id: Some(call_id),
                                            name: Some(name),
                                            arguments: String::new(),
                                        });
                                    }
                                    Ok(RespEvent::FunctionArgsDelta { output_index, delta }) => {
                                        yield ChatChunk::ToolCall(ToolCallDelta {
                                            index: output_index,
                                            id: None,
                                            name: None,
                                            arguments: delta,
                                        });
                                    }
                                    // Reasoning-элемент завершён — несёт encrypted_content
                                    // (запросили через include). Копим для переотправки при tool-use.
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
                                        // Обрыв по лимиту (max_output_tokens и т.п.).
                                        if let Some(u) = response.and_then(|r| r.usage) {
                                            yield ChatChunk::Usage(TokenUsage {
                                                prompt_tokens: u.input_tokens,
                                                completion_tokens: u.output_tokens,
                                            });
                                        }
                                        yield ChatChunk::Finished(FinishReason::Length);
                                        break;
                                    }
                                    Ok(RespEvent::Failed) => {
                                        yield ChatChunk::Finished(FinishReason::Error);
                                        break;
                                    }
                                    Ok(RespEvent::Error { message }) => {
                                        if let Some(m) = message {
                                            tracing::warn!(message = %m, "openai responses error event");
                                        }
                                        yield ChatChunk::Finished(FinishReason::Error);
                                        break;
                                    }
                                    // Прочие события (created/in_progress/part.added/…) и
                                    // added-reasoning (без encrypted) — игнорируем.
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
}

/// Ручной смоук против реального OpenAI Responses API. Помечен `#[ignore]` — не в CI.
/// Запуск: `MINDFORK_OPENAI_KEY=sk-... cargo test responses -- --ignored --nocapture`.
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
                _ => {}
            }
        }
        assert!(!text.is_empty(), "expected non-empty response");
        assert!(matches!(
            finish,
            Some(FinishReason::Stop | FinishReason::Length)
        ));
    }

    /// Резюме рассуждений: с `thinking=true`+`verbosity` приходят «мысли» (Thoughts)
    /// и финальный ответ. Требует reasoning-модель (gpt-5.x). `max_tokens` щедрый —
    /// reasoning-токены расходуют бюджет ответа.
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
                _ => {}
            }
        }
        // На тривиальной задаче резюме может отсутствовать — проверяем хотя бы ответ.
        assert!(
            !text.is_empty(),
            "expected final answer, thoughts={thoughts:?}"
        );
    }

    /// Tool-use round-trip: первый раунд даёт вызов + reasoning-элемент (id+encrypted);
    /// второй переотправляет reasoning-элемент перед его function_call и результат —
    /// OpenAI не должен вернуть ошибку.
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
