//! HTTP-клиент к Anthropic Messages API (`platform.claude.com`), реализующий
//! [`EngineBackend`]. Отдельный протокол от OpenAI (`/v1/messages`, заголовки
//! `x-api-key` + `anthropic-version`, событийный SSE). См. ADR 0004, Фаза 2.
//!
//! Эмбеддингов у Anthropic нет — [`Embedder`](super::super::contract::Embedder) тут
//! не реализуется (RAG берёт отдельный эмбеддер, ADR 0002).

use anyhow::{Context, Result};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, AntDelta, AntStartBlock, AntStreamEvent};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, TokenUsage, ToolCallDelta,
};

/// Версия Anthropic API (обязательный заголовок `anthropic-version`).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Клиент к Anthropic Messages API.
pub struct AnthropicClient {
    http: reqwest::Client,
    /// Базовый URL без суффикса (клиент добавляет `/v1/messages`), напр.
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

/// Маппинг `stop_reason` Anthropic в доменный [`FinishReason`].
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
        // Не глотаем тело ошибки (как OpenAiClient): Anthropic кладёт причину в JSON
        // (`{"error":{"message":...}}`) — логируем и пробрасываем в текст ошибки.
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
            // input_tokens приходят в message_start, output_tokens — в message_delta.
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
                                        yield ChatChunk::ToolCall(ToolCallDelta {
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
                                        AntDelta::InputJsonDelta { partial_json } => {
                                            yield ChatChunk::ToolCall(ToolCallDelta {
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

/// Ручной смоук против реального Anthropic API. Помечен `#[ignore]` — не идёт в CI.
/// Запуск: `MINDFORK_ANTHROPIC_KEY=sk-ant-... cargo test anthropic -- --ignored`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::api::contract::ApiMessage;

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
}
