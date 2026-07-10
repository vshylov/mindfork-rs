//! HTTP-клиент к нативному Google Gemini API (`POST …:streamGenerateContent?alt=sse`),
//! реализующий [`EngineBackend`]. Отдельный протокол от OpenAI-совместимого Chat
//! Completions (`OpenAiClient` + Gemini-диалект, теперь заменён этим клиентом): резюме
//! «мыслей» (`thinkingConfig.includeThoughts`), глубина (`thinkingLevel`/
//! `thinkingBudget`), `thoughtsTokenCount`. См. ADR 0004, docs/research/gemini-native-client.md.
//!
//! Эмбеддингов этот клиент не даёт — RAG в режиме Gemini берёт отдельный источник
//! (OpenAI-совместимый `…/v1beta/openai/embeddings`, см. супервайзер), как у Anthropic.

use anyhow::{Context, Result};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, GenResponse};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, TokenUsage, ToolCallDelta,
};

/// Клиент к нативному Gemini API.
pub struct GeminiClient {
    http: reqwest::Client,
    /// Базовый URL с суффиксом `/v1beta` (клиент добавляет
    /// `/models/{model}:streamGenerateContent`), напр.
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
        // Имя модели может прийти с префиксом `models/` — путь его уже несёт.
        let model = model
            .into()
            .trim_start_matches("models/")
            .trim()
            .to_string();
        Self {
            http: reqwest::Client::new(),
            base_url,
            api_key: api_key.into(),
            model,
        }
    }
}

/// Строковый `finishReason` Gemini → доменная причина. Наличие вызовов инструментов
/// (`saw_tool_call`) даёт `ToolCalls` даже при `STOP` (Gemini возвращает `STOP` с
/// `functionCall`-частями). `MAX_TOKENS` → `Length`; прочее (`SAFETY`/`RECITATION`/…)
/// сводим к `Stop`, чтобы не падать.
fn map_finish(reason: &str, saw_tool_call: bool) -> FinishReason {
    match reason {
        "MAX_TOKENS" => FinishReason::Length,
        _ if saw_tool_call => FinishReason::ToolCalls,
        _ => FinishReason::Stop,
    }
}

#[async_trait::async_trait]
impl EngineBackend for GeminiClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let body = wire::build_request(&req, &self.model);
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url, self.model
        );

        let response = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        // Не глотаем тело ошибки (как прочие клиенты): Gemini кладёт причину в JSON
        // (`{"error":{"message":...}}`) — логируем и пробрасываем в текст ошибки.
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.trim().chars().take(500).collect();
            tracing::warn!(%status, body = %detail, "gemini returned an error status");
            if detail.is_empty() {
                anyhow::bail!("движок (Gemini) вернул статус {status}");
            }
            anyhow::bail!("движок (Gemini) вернул статус {status}: {detail}");
        }

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // Порядковый индекс вызова инструмента (у Gemini нет call_id — синтезируем
            // стабильный id `"{name}-{index}"`, парность functionResponse — по нему).
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
                                tracing::warn!(error = %err, "SSE stream error (gemini)");
                                yield ChatChunk::Finished(FinishReason::Error);
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
}

/// Ручной смоук против реального Gemini API. Помечен `#[ignore]` — не в CI.
/// Запуск: `MINDFORK_GEMINI_KEY=… cargo test gemini -- --ignored --nocapture`.
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

    /// Резюме рассуждений: с `thinking=true` приходят «мысли» (Thoughts) и ответ.
    /// `max_tokens` щедрый — токены мыслей расходуют бюджет ответа.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn thinking_streams_thoughts() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
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
        assert!(
            !text.is_empty(),
            "expected final answer, thoughts={thoughts:?}"
        );
    }

    /// Один tool-раунд: модель вызывает инструмент (без переотправки подписи — Фаза B).
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
}
