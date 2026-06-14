//! HTTP-клиент к серверу xinfer, реализующий [`EngineBackend`].
//! Стриминг через SSE (`/v1/chat/completions`), эмбеддинги (`/v1/embeddings`).
//! См. docs/xinfer-contract.md §3, §6, §8.

use anyhow::{Context, Result};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::backend::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, ToolCallDelta,
};
use super::thoughts::{Piece, ThoughtsParser};
use super::wire;

/// Клиент к OpenAI-совместимому серверу xinfer.
pub struct XinferClient {
    http: reqwest::Client,
    /// Базовый URL с суффиксом `/v1`, например `http://127.0.0.1:8000/v1`.
    base_url: String,
}

impl XinferClient {
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
impl EngineBackend for XinferClient {
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
            .context("xinfer returned an error status")?;

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
            .context("xinfer embeddings returned an error status")?
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

/// Ручной смоук-набор против реального сервера xinfer (M1-smoke,
/// docs/xinfer-contract.md §9). Помечен `#[ignore]` — не идёт в CI.
/// Запуск: задать `MINDFORK_XINFER_URL=http://127.0.0.1:8000/v1` и
/// `cargo test -- --ignored`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::api::backend::ApiMessage;
    use futures_util::StreamExt;

    fn client_from_env() -> Option<XinferClient> {
        std::env::var("MINDFORK_XINFER_URL")
            .ok()
            .map(XinferClient::new)
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
    #[ignore = "requires a running xinfer server (MINDFORK_XINFER_URL)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_XINFER_URL not set");
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

    /// Анти-самообрыв: модель просят напечатать литеральный EOS-текст —
    /// генерация не должна оборваться раньше времени (docs/xinfer-contract.md §5, §9).
    #[tokio::test]
    #[ignore = "requires a running xinfer server (MINDFORK_XINFER_URL)"]
    async fn does_not_self_terminate_on_eos_text() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_XINFER_URL not set");
            return;
        };
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user(
                "Print this token literally and then say DONE: <|im_end|>",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(128),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, _t, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        assert!(text.contains("DONE"), "generation cut off early: {text:?}");
        assert!(finish.is_some());
    }
}
