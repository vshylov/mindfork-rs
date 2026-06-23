//! Инструмент `fetch_url` (spec §9.3): загрузить веб-страницу и саммаризировать её.
//!
//! Под глобальным выключателем `tools.web_enabled` (сетевой доступ/приватность, как
//! `web_search`). Шаги: загрузка страницы собственным `reqwest`-клиентом →
//! извлечение читаемого текста (`web::extract_readable`, переиспользуем readability)
//! → саммаризация через `ctx.engine` (независимый одно-ходовый запрос, как
//! `call_subagent`). С `summarize=false` возвращается извлечённый текст без вызова
//! модели (быстрый путь).

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest};

use super::web::{ACCEPT_HTML, ACCEPT_LANGUAGE, USER_AGENT, extract_readable, truncate_chars};
use super::{Tool, ToolContext, ToolOutcome};

/// Таймаут загрузки страницы.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Потолок извлекаемого текста (символов): что уходит в саммаризацию/возврат.
const MAX_CONTENT_CHARS: usize = 12_000;
/// Лимит токенов ответа-саммари.
const SUMMARY_MAX_TOKENS: usize = 768;
/// Таймаут саммаризации (вызов модели).
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

/// `fetch_url` — загрузить страницу и (по умолчанию) саммаризировать её.
pub struct FetchUrl {
    http: reqwest::Client,
}

impl Default for FetchUrl {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchUrl {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self { http }
    }

    /// Загружает страницу и извлекает читаемый текст. Ошибка → понятное сообщение.
    async fn fetch_text(&self, url: &str) -> Result<String> {
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, ACCEPT_HTML)
            .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
            .send()
            .await
            .with_context(|| format!("запрос к {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("страница вернула статус {status}");
        }
        let body = resp.text().await.with_context(|| format!("чтение {url}"))?;
        let text = extract_readable(&body, MAX_CONTENT_CHARS);
        if text.is_empty() {
            anyhow::bail!("со страницы не удалось извлечь читаемый текст");
        }
        Ok(text)
    }
}

#[async_trait::async_trait]
impl Tool for FetchUrl {
    fn id(&self) -> ToolId {
        "fetch_url".into()
    }
    fn description(&self) -> String {
        "Загрузить веб-страницу по URL и вернуть её краткое содержание. Передай focus, \
         чтобы сосредоточиться на конкретном вопросе. summarize=false вернёт извлечённый \
         текст без саммаризации."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Адрес страницы (http/https)"},
                "focus": {
                    "type": "string",
                    "description": "На чём сосредоточиться при саммаризации (необязательно)"
                },
                "summarize": {
                    "type": "boolean",
                    "description": "Саммаризировать моделью (по умолчанию true); false — вернуть извлечённый текст"
                }
            },
            "required": ["url"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле url"))?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            anyhow::bail!("url должен начинаться с http:// или https://");
        }
        let focus = args
            .get("focus")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let summarize = args
            .get("summarize")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let text = match self.fetch_text(url).await {
            Ok(t) => t,
            Err(err) => {
                return Ok(ToolOutcome::text(format!(
                    "Не удалось загрузить {url}: {err}"
                )));
            }
        };

        if !summarize {
            return Ok(ToolOutcome::text(format!(
                "Содержимое {url}:\n{}",
                truncate_chars(&text, MAX_CONTENT_CHARS)
            )));
        }

        match summarize_text(ctx, url, focus, &text).await {
            Ok(summary) if !summary.trim().is_empty() => Ok(ToolOutcome::text(summary)),
            // Саммаризация не удалась/пуста → отдаём извлечённый текст (мягкая
            // деградация: модель всё равно получит контент страницы).
            _ => Ok(ToolOutcome::text(format!(
                "Содержимое {url} (саммаризация недоступна):\n{}",
                truncate_chars(&text, MAX_CONTENT_CHARS)
            ))),
        }
    }
}

/// Саммаризирует текст страницы независимым одно-ходовым запросом к модели
/// (как `call_subagent`: без истории и инструментов, с лимитом токенов/времени).
async fn summarize_text(
    ctx: &ToolContext,
    url: &str,
    focus: Option<&str>,
    text: &str,
) -> Result<String> {
    let system = "Ты кратко и точно пересказываешь содержимое веб-страниц. Выдели \
         главное по существу, без воды и домыслов. Если в тексте нет ответа — скажи об этом."
        .to_string();
    let task = match focus {
        Some(f) => {
            format!("Страница: {url}\n\nСосредоточься на вопросе: {f}\n\nТекст страницы:\n{text}")
        }
        None => {
            format!("Страница: {url}\n\nКратко перескажи содержимое.\n\nТекст страницы:\n{text}")
        }
    };

    let sampling = SamplingConfig {
        max_tokens: Some(
            ctx.effective_sampling
                .max_tokens
                .map_or(SUMMARY_MAX_TOKENS, |m| m.min(SUMMARY_MAX_TOKENS)),
        ),
        ..ctx.effective_sampling.clone()
    };
    let request = ChatRequest {
        system: Some(system),
        messages: vec![ApiMessage::user(task)],
        sampling,
        tools: Vec::new(), // без инструментов (запрет вложенности)
    };

    let cancel = CancellationToken::new();
    let engine = ctx.engine.clone();
    let collect = async {
        let mut stream = engine.chat_stream(request, cancel.clone()).await?;
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => out.push_str(&t),
                ChatChunk::Finished(_) => break,
                ChatChunk::Thoughts(_) | ChatChunk::ToolCall(_) | ChatChunk::Usage(_) => {}
            }
        }
        Ok::<String, anyhow::Error>(out)
    };

    match tokio::time::timeout(SUMMARY_TIMEOUT, collect).await {
        Ok(res) => res,
        Err(_) => {
            cancel.cancel();
            anyhow::bail!("саммаризация превысила лимит времени");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::Embedder;
    use crate::shared::api::backend::{ChatStream, EngineBackend, FinishReason};
    use crate::shared::api::mock::{MockBackend, MockEmbedder};
    use crate::shared::paths::Paths;
    use crate::shared::storage::Storage;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    /// Движок, запоминающий запрос и отдающий фиксированный ответ.
    struct CapturingBackend {
        last: Mutex<Option<ChatRequest>>,
        reply: String,
    }

    #[async_trait::async_trait]
    impl EngineBackend for CapturingBackend {
        async fn chat_stream(
            &self,
            req: ChatRequest,
            _cancel: CancellationToken,
        ) -> Result<ChatStream> {
            *self.last.lock().unwrap() = Some(req);
            let reply = self.reply.clone();
            let s = async_stream::stream! {
                yield ChatChunk::Text(reply);
                yield ChatChunk::Finished(FinishReason::Stop);
            };
            Ok(Box::pin(s))
        }
    }

    fn ctx_with_engine(engine: Arc<dyn EngineBackend>) -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(16));
        let ctx = ToolContext {
            profile_id: Uuid::new_v4(),
            chat_id: Uuid::new_v4(),
            system_message: String::new(),
            effective_sampling: SamplingConfig::default(),
            last_user_message_at: None,
            storage,
            engine,
            embedder,
            chunk_params: crate::features::tools::rag::ChunkParams::default(),
        };
        (dir, ctx)
    }

    #[tokio::test]
    async fn rejects_non_http_url() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        assert!(
            FetchUrl::new()
                .invoke(&ctx, serde_json::json!({"url": "ftp://x/y"}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_empty_url() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        assert!(
            FetchUrl::new()
                .invoke(&ctx, serde_json::json!({"url": "  "}))
                .await
                .is_err()
        );
    }

    /// Саммаризация строит запрос без инструментов, с лимитом токенов, и включает
    /// focus в задачу. Проверяет чистую функцию `summarize_text` напрямую.
    #[tokio::test]
    async fn summarize_builds_single_turn_request_with_focus() {
        let backend = Arc::new(CapturingBackend {
            last: Mutex::new(None),
            reply: "краткое содержание".into(),
        });
        let (_d, ctx) = ctx_with_engine(backend.clone());
        let summary = summarize_text(
            &ctx,
            "https://example.com",
            Some("какова цена?"),
            "Длинный текст страницы про цены и условия.",
        )
        .await
        .unwrap();
        assert_eq!(summary, "краткое содержание");

        let req = backend.last.lock().unwrap().take().unwrap();
        assert!(req.system.is_some());
        assert_eq!(req.messages.len(), 1);
        assert!(
            req.tools.is_empty(),
            "без инструментов (запрет вложенности)"
        );
        assert!(req.sampling.max_tokens.unwrap() <= SUMMARY_MAX_TOKENS);
        // focus попал в задачу.
        let msg = format!("{:?}", req.messages[0]);
        assert!(msg.contains("какова цена"), "focus в задаче: {msg}");
    }

    /// Реальный сетевой смоук (вручную: `cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn live_fetch_without_summarize() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        let out = FetchUrl::new()
            .invoke(
                &ctx,
                serde_json::json!({"url": "https://example.com", "summarize": false}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Содержимое"), "got: {}", out.result);
    }
}
