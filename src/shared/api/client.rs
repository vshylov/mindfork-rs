//! HTTP-клиент к **OpenAI-совместимому** серверу (llama.cpp `llama-server`, vLLM,
//! LM Studio, …), реализующий [`EngineBackend`]. Стриминг через SSE
//! (`/v1/chat/completions`), эмбеддинги (`/v1/embeddings`). Протокол — OpenAI
//! (исходно сверялся с docs/xinfer-contract.md; llama.cpp говорит на том же).

use anyhow::{Context, Result};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::backend::{
    ChatChunk, ChatRequest, ChatStream, Embedder, EngineBackend, FinishReason, ToolCallDelta,
};
use super::thoughts::{Piece, ThoughtsParser};
use super::wire;

/// Клиент к OpenAI-совместимому серверу инференса.
pub struct OpenAiClient {
    http: reqwest::Client,
    /// Базовый URL с суффиксом `/v1`, например `http://127.0.0.1:8000/v1`.
    base_url: String,
}

impl OpenAiClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    /// Лёгкая проверка доступности сервера (нет `/health` — пробуем `/usage`).
    /// См. docs/xinfer-contract.md §2.
    pub async fn probe(&self) -> Result<()> {
        let url = format!("{}/usage", self.base_url);
        self.http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("probing {url}"))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl EngineBackend for OpenAiClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let body = wire::build_chat_request(&req, true);
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?
            .error_for_status()
            .context("engine returned an error status")?;

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // Разделитель «мыслей» на случай инлайнового <think> в content.
            let mut parser = ThoughtsParser::new();
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
                                for piece in parser.finish() { yield piece_to_chunk(piece); }
                                yield ChatChunk::Finished(FinishReason::Stop);
                                break;
                            }
                            Some(Err(err)) => {
                                tracing::warn!(error = %err, "SSE stream error");
                                yield ChatChunk::Finished(FinishReason::Error);
                                break;
                            }
                            Some(Ok(event)) => {
                                if event.data == "[DONE]" {
                                    for piece in parser.finish() { yield piece_to_chunk(piece); }
                                    yield ChatChunk::Finished(FinishReason::Stop);
                                    break;
                                }
                                match serde_json::from_str::<wire::ChatCompletionChunk>(&event.data) {
                                    Ok(chunk) => {
                                        let Some(choice) = chunk.choices.into_iter().next() else { continue };
                                        if let Some(r) = choice.delta.reasoning_content
                                            && !r.is_empty()
                                        {
                                            yield ChatChunk::Thoughts(r);
                                        }
                                        if let Some(c) = choice.delta.content
                                            && !c.is_empty()
                                        {
                                            for piece in parser.push(&c) { yield piece_to_chunk(piece); }
                                        }
                                        if let Some(tool_calls) = choice.delta.tool_calls {
                                            for tc in tool_calls {
                                                let (name, arguments) = match tc.function {
                                                    Some(f) => (f.name, f.arguments.unwrap_or_default()),
                                                    None => (None, String::new()),
                                                };
                                                yield ChatChunk::ToolCall(ToolCallDelta {
                                                    index: tc.index,
                                                    id: tc.id,
                                                    name,
                                                    arguments,
                                                });
                                            }
                                        }
                                        if let Some(reason) = choice.finish_reason {
                                            for piece in parser.finish() { yield piece_to_chunk(piece); }
                                            yield ChatChunk::Finished(FinishReason::from_wire(&reason));
                                            break;
                                        }
                                    }
                                    Err(err) => {
                                        tracing::warn!(error = %err, data = %event.data, "failed to parse SSE chunk");
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

#[async_trait::async_trait]
impl Embedder for OpenAiClient {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = wire::EmbeddingRequest { input: texts };
        let resp: wire::EmbeddingResponse = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?
            .error_for_status()
            .context("embeddings request returned an error status")?
            .json()
            .await
            .context("decoding embeddings response")?;
        Ok(resp.data.into_iter().map(|d| d.embedding).collect())
    }
}

fn piece_to_chunk(piece: Piece) -> ChatChunk {
    match piece {
        Piece::Text(t) => ChatChunk::Text(t),
        Piece::Thoughts(t) => ChatChunk::Thoughts(t),
    }
}

/// Ручной смоук-набор против реального OpenAI-совместимого сервера (llama.cpp
/// `llama-server` и т.п.). Помечен `#[ignore]` — не идёт в CI.
/// Запуск: задать `MINDFORK_ENGINE_URL=http://127.0.0.1:8000/v1` и
/// `cargo test -- --ignored`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::api::backend::{ApiMessage, ToolCallAccumulator, ToolSchema};
    use futures_util::StreamExt;

    fn client_from_env() -> Option<OpenAiClient> {
        std::env::var("MINDFORK_ENGINE_URL")
            .ok()
            .map(OpenAiClient::new)
    }

    async fn collect(stream: ChatStream) -> (String, String, Option<FinishReason>) {
        let mut text = String::new();
        let mut thoughts = String::new();
        let mut finish = None;
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::ToolCall(_) => {}
                ChatChunk::Finished(r) => {
                    finish = Some(r);
                    break;
                }
            }
        }
        (text, thoughts, finish)
    }

    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
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
        let (text, _thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        assert!(!text.is_empty(), "expected non-empty response");
        assert!(matches!(
            finish,
            Some(FinishReason::Stop | FinishReason::Length)
        ));
    }

    /// Просит модель напечатать литеральный EOS-текст и затем сказать `DONE` —
    /// генерация не должна оборваться (остановка по token-id на сервере, поле `stop`
    /// не шлём; docs/xinfer-contract.md §5). `DONE` может прийти в тексте или в
    /// «мыслях» (reasoning-модель), поэтому проверяем оба потока.
    async fn assert_no_self_terminate(client: &OpenAiClient, eos_text: &str) {
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user(format!(
                "Print this token literally and then say DONE: {eos_text}"
            ))],
            sampling: SamplingConfig {
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        let combined = format!("{thoughts}{text}");
        assert!(
            combined.contains("DONE"),
            "generation cut off early for {eos_text:?}: text={text:?} thoughts={thoughts:?}"
        );
        assert!(finish.is_some());
    }

    /// Анти-самообрыв на тексте EOS — для обоих семейств: Qwen (`<|im_end|>`) и
    /// Gemma (`<end_of_turn>`). См. spec §7, docs/xinfer-contract.md §5, §9.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn does_not_self_terminate_on_eos_text() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        for eos in ["<|im_end|>", "<end_of_turn>"] {
            assert_no_self_terminate(&client, eos).await;
        }
    }

    /// Tool-calling: сервер получает схему инструмента, модель вызывает его —
    /// `finish_reason="tool_calls"` и `delta.tool_calls` корректно собираются.
    /// `max_tokens` щедрый: reasoning-модель «думает» перед вызовом.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn tool_call_is_emitted_and_parsed() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let tool = ToolSchema {
            name: "get_weather".into(),
            description: "Get current weather for a city".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }),
        };
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user(
                "Call get_weather for Paris. Respond only with the tool call.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                ..Default::default()
            },
            tools: vec![tool],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut acc = ToolCallAccumulator::default();
        let mut finish = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(delta) => acc.push(delta),
                ChatChunk::Finished(reason) => {
                    finish = Some(reason);
                    break;
                }
                _ => {}
            }
        }
        let calls = acc.finish();
        assert_eq!(
            finish,
            Some(FinishReason::ToolCalls),
            "expected tool_calls finish, got {finish:?} (calls={calls:?})"
        );
        assert!(
            calls.iter().any(|c| c.name == "get_weather"),
            "get_weather call not parsed: {calls:?}"
        );
    }

    /// «Мысли» (CoT): reasoning-модель (или сервер с `--reasoning-format`) отдаёт
    /// `reasoning_content` отдельным потоком — mindfork собирает их в `Thoughts`.
    /// Требует thinking-модель; иначе `thoughts` будет пуст (мысли инлайнятся).
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server with a reasoning model"]
    async fn emits_thoughts_for_reasoning_model() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step, then answer: what is 17*23?",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                thinking: Some(true),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        assert!(finish.is_some());
        assert!(
            !thoughts.is_empty(),
            "expected non-empty thoughts from a reasoning model: text={text:?}"
        );
    }
}
