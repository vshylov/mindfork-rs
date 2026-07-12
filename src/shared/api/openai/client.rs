//! HTTP-клиент к **OpenAI-совместимому** серверу (llama.cpp `llama-server`, vLLM,
//! LM Studio, …), реализующий [`EngineBackend`]. Стриминг через SSE
//! (`/v1/chat/completions`), эмбеддинги (`/v1/embeddings`). Протокол — OpenAI
//! (исходно сверялся с docs/xinfer-contract.md; llama.cpp говорит на том же).

use anyhow::{Context, Result, bail};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire;
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, Embedder, EngineBackend, FinishReason, TokenUsage,
    ToolCallDelta,
};
use crate::shared::api::thoughts::{Piece, ThoughtsParser};

/// Клиент к OpenAI-совместимому серверу инференса (локальный/external `llama-server`,
/// vLLM, LM Studio…; при желании — прокси с Bearer-ключом). Облака теперь используют
/// свои протоколы (OpenAI → Responses, Gemini → нативный, Claude → Anthropic), поэтому
/// диалекта тела больше нет — сэмплинг шлётся как есть (см. ADR 0004). Также источник
/// эмбеддингов (`/v1/embeddings`) для локального/облачного RAG.
pub struct OpenAiClient {
    http: reqwest::Client,
    /// Базовый URL с суффиксом `/v1`, например `http://127.0.0.1:8000/v1`.
    base_url: String,
    /// API-ключ для Bearer-аутентификации (прокси/облачные эмбеддинги). `None` — без
    /// заголовка.
    api_key: Option<String>,
    /// Имя модели; подставляется в тело запроса, если задано (облачные эмбеддинги/
    /// мульти-модельный прокси требуют, `llama-server` игнорирует). Доменный
    /// [`ChatRequest`] модель не несёт — это свойство бэкенда.
    model: Option<String>,
}

impl OpenAiClient {
    /// Клиент к локальному/external OpenAI-совместимому серверу: без ключа, без имени
    /// модели (расширения llama.cpp шлются как есть).
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            http: reqwest::Client::new(),
            base_url,
            api_key: None,
            model: None,
        }
    }

    /// Устанавливает API-ключ (Bearer). Билдер-стиль.
    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.is_empty());
        self
    }

    /// Устанавливает имя модели (для облачных эмбеддингов/мульти-модельного сервера).
    /// Билдер-стиль.
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model.filter(|m| !m.is_empty());
        self
    }

    /// Добавляет Bearer-заголовок, если задан ключ.
    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => rb.bearer_auth(key),
            None => rb,
        }
    }

    /// Проверка **готовности** сервера к инференсу (не просто «порт открыт»).
    ///
    /// llama.cpp биндит HTTP-порт сразу, но на время загрузки модели (~секунды для
    /// крупных GGUF) отвечает `503 Loading model` на эндпоинты инференса. Поэтому
    /// проверять только факт ответа нельзя — иначе статус «Ready» выставится раньше
    /// готовности, и первый же запрос упадёт с 503 (см. §7). Пробуем `/health`
    /// (в корне, вне `/v1`): `503` — ещё грузится (не готов), `200` — готов, `404`
    /// (сервер без `/health`) — считаем «жив и не грузится» (готов).
    pub async fn probe(&self) -> Result<()> {
        let url = health_url(&self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("probing {url}"))?;
        if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            bail!("сервер ещё загружает модель (503)");
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EngineBackend for OpenAiClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let body = wire::build_chat_request(&req, true, self.model.as_deref());
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .auth(self.http.post(&url))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        // Не глотаем тело ошибки: llama.cpp/OpenAI-серверы кладут причину в JSON
        // (`{"error":{"message":...}}`); без неё «error status» бесполезен. Логируем
        // в файл и пробрасываем в текст ошибки (обрезая длинные тела). См. §7.
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.trim().chars().take(500).collect();
            tracing::warn!(%status, body = %detail, "engine returned an error status");
            if detail.is_empty() {
                anyhow::bail!("движок вернул статус {status}");
            }
            anyhow::bail!("движок вернул статус {status}: {detail}");
        }

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
                                        // Счётчик токенов (include_usage) приходит отдельным
                                        // чанком (с пустым choices) — отдаём до разбора choice.
                                        if let Some(u) = chunk.usage {
                                            yield ChatChunk::Usage(TokenUsage {
                                                prompt_tokens: u.prompt_tokens,
                                                completion_tokens: u.completion_tokens,
                                                reasoning_tokens: u.completion_tokens_details.reasoning_tokens,
                                            });
                                        }
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
                                                yield ChatChunk::ToolCall(ToolCallDelta { thought_signature: None,
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
        let body = wire::EmbeddingRequest {
            model: self.model.clone(),
            input: texts,
        };
        let response = self
            .auth(self.http.post(&url))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        // Не глотаем тело ошибки (как и в chat_stream): llama-server кладёт причину в
        // JSON (напр. «input is too large to process. increase the physical batch
        // size» при слишком длинном чанке) — без неё «error status» бесполезен.
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.trim().chars().take(500).collect();
            tracing::warn!(%status, body = %detail, "embeddings request returned an error status");
            if detail.is_empty() {
                anyhow::bail!("эмбеддер вернул статус {status}");
            }
            anyhow::bail!("эмбеддер вернул статус {status}: {detail}");
        }
        let resp: wire::EmbeddingResponse = response
            .json()
            .await
            .context("decoding embeddings response")?;
        Ok(resp.data.into_iter().map(|d| d.embedding).collect())
    }
}

/// URL эндпоинта готовности `/health` из базового URL. `/health` живёт в корне
/// сервера (вне `/v1`), поэтому суффикс `/v1` отбрасывается.
fn health_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let root = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    format!("{root}/health")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_url_strips_v1_suffix() {
        assert_eq!(
            health_url("http://127.0.0.1:8000/v1"),
            "http://127.0.0.1:8000/health"
        );
        // Без /v1 — просто дописываем /health.
        assert_eq!(health_url("http://host:9"), "http://host:9/health");
        // Лишний слэш не задваивается.
        assert_eq!(health_url("http://host:9/v1/"), "http://host:9/health");
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
    use crate::shared::api::contract::{ApiMessage, ToolCallAccumulator, ToolSchema};
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
                ChatChunk::ThoughtsSignature(_) | ChatChunk::ToolCall(_) | ChatChunk::Usage(_) => {}
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

    /// Расширения «для разнообразия»: динамическая температура, adaptive-p,
    /// DRY-брейкеры и кастомный порядок семплеров — всё в теле одного запроса.
    /// Цель — убедиться, что `llama-server` **принимает** эти поля (не отвечает
    /// `400`/ошибкой) и генерирует. Ключи сверены по
    /// `tools/server/server-schema.cpp` (dynatemp_range/exponent, adaptive_target/
    /// decay, dry_sequence_breakers — непустой, samplers — массив имён). Если бы
    /// сервер отверг любое поле, `chat_stream` вернул бы ошибку статуса (клиент не
    /// глотает тело ошибки) и тест упал бы на `.unwrap()`.
    ///
    /// Проверяем **объединённый** поток (`text` + `thoughts`): у reasoning-модели
    /// (Gemma со «вшитым» thinking) ответ может целиком уйти в `reasoning_content`,
    /// а `content` остаться пустым с `finish_reason="length"` — это нормально и к
    /// принятию sampling-полей отношения не имеет (см. CLAUDE.md, ловушка
    /// reasoning-бюджета). `max_tokens` щедрый, чтобы было видно генерацию.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn accepts_creative_sampling_extensions() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            system: Some("You are a creative writing assistant.".into()),
            messages: vec![ApiMessage::user(
                "Write one whimsical sentence about a teapot.",
            )],
            sampling: SamplingConfig {
                temperature: Some(1.0),
                // Динамическая температура: ±0.5 вокруг temperature.
                dynatemp_range: Some(0.5),
                dynatemp_exponent: Some(1.0),
                // adaptive-p: положительная цель включает семплер (≤1.0).
                adaptive_target: Some(0.1),
                adaptive_decay: Some(0.9),
                // DRY с непустым списком брейкеров (пустой сервер отверг бы).
                dry_multiplier: Some(0.8),
                dry_sequence_breakers: Some(vec!["\n".into(), ":".into()]),
                // Кастомный порядок семплеров (валидные имена из sampling.cpp).
                samplers: Some(vec![
                    "penalties".into(),
                    "dry".into(),
                    "top_k".into(),
                    "top_p".into(),
                    "min_p".into(),
                    "temperature".into(),
                ]),
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        // Reasoning-модель кладёт ответ в «мысли» — проверяем оба потока.
        let combined = format!("{thoughts}{text}");
        assert!(
            !combined.trim().is_empty(),
            "server accepted extensions but generated nothing: finish={finish:?}"
        );
        assert!(
            matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
            "unexpected finish reason: {finish:?}"
        );
    }

    /// Управляющие инструменты беседы (followup/rewrite, spec §9.3.3): живая модель
    /// должна **вызвать** `send_followup_message` по инструкции — `finish_reason=
    /// "tool_calls"` и имя разобрано. Это ключевой неизвестный фичи (поймёт ли
    /// модель схему/описание). Схемы берём прямо из реализаций `Tool` (реальные
    /// описания). `max_tokens` щедрый — Gemma может «подумать» перед вызовом.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn control_tools_are_callable() {
        use crate::features::tools::Tool;
        use crate::features::tools::control::{RewriteCurrentMessage, SendFollowupMessage};
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            system: Some(
                "Ты — дружелюбный ассистент. Ответь на сообщение пользователя \
                 короткой первой репликой, а затем ОБЯЗАТЕЛЬНО вызови инструмент \
                 send_followup_message, чтобы добавить вторую реплику с подробностями."
                    .into(),
            ),
            messages: vec![ApiMessage::user("Расскажи интересный факт о космосе.")],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                ..Default::default()
            },
            tools: {
                let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
                vec![
                    SendFollowupMessage.schema(loc),
                    RewriteCurrentMessage.schema(loc),
                ]
            },
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
        assert!(
            calls.iter().any(|c| c.name == "send_followup_message"),
            "модель не вызвала send_followup_message: finish={finish:?} calls={calls:?}"
        );
    }
}
