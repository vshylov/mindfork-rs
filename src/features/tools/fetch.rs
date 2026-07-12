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
        // Content-Type читаем ДО поглощения тела (`resp.text()` забирает resp).
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp.text().await.with_context(|| format!("чтение {url}"))?;
        body_to_text(&content_type, &body)
            .ok_or_else(|| anyhow::anyhow!("со страницы не удалось извлечь читаемый текст"))
    }
}

/// Выбирает текст для возврата из тела ответа. Не-HTML **текстовые** ответы
/// (JSON/text/csv/JS — определяем по `Content-Type`, а при его отсутствии по
/// JSON-форме тела) отдаём как есть (усечённо): это данные API, readability к ним
/// неприменима (нет `<p>`/`<li>`) — именно из-за этого JSON Steam-API прежде давал
/// «не удалось извлечь читаемый текст». HTML → извлечение читаемого текста.
/// `None` — извлекать нечего (пусто). Чистая функция — тестируема без сети.
fn body_to_text(content_type: &str, body: &str) -> Option<String> {
    let ct = content_type.to_ascii_lowercase();
    let is_html = ct.contains("html") || ct.contains("xml");
    let is_texty = ct.contains("json")
        || ct.contains("text/plain")
        || ct.contains("javascript")
        || ct.contains("csv");
    let looks_json = {
        let t = body.trim_start();
        t.starts_with('{') || t.starts_with('[')
    };
    if is_texty || (looks_json && !is_html) {
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            return Some(truncate_chars(trimmed, MAX_CONTENT_CHARS));
        }
    }
    let text = extract_readable(body, MAX_CONTENT_CHARS);
    (!text.is_empty()).then_some(text)
}

#[async_trait::async_trait]
impl Tool for FetchUrl {
    fn id(&self) -> ToolId {
        super::FETCH_URL_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "загрузить страницу"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Web)
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Загрузить веб-страницу по URL и вернуть её краткое содержание. Передай focus, \
         чтобы сосредоточиться на конкретном вопросе. summarize=false вернёт извлечённый \
         текст без саммаризации."
            .into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
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
                ChatChunk::Thoughts(_)
                | ChatChunk::ThoughtsSignature(_)
                | ChatChunk::ToolCall(_)
                | ChatChunk::Usage(_) => {}
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
    use crate::shared::api::contract::{ChatStream, EngineBackend, FinishReason};
    use crate::shared::api::mock::{MockBackend, MockEmbedder};
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
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(16));
        let (dir, _storage, ctx) =
            super::super::testkit::ctx_with_backends(Uuid::new_v4(), engine, embedder);
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

    #[test]
    fn json_body_returned_as_is_not_extracted() {
        // JSON-ответ API (по Content-Type) отдаётся как есть — раньше readability
        // возвращала пусто → «не удалось извлечь читаемый текст» (Steam appreviews).
        let body = r#"{"success":1,"query_summary":{"total_positive":200,"total_negative":30}}"#;
        let out = body_to_text("application/json; charset=utf-8", body).unwrap();
        assert!(out.contains("total_positive"), "got: {out}");
    }

    #[test]
    fn json_shaped_body_returned_when_content_type_missing() {
        // Нет Content-Type, но тело — JSON-форма (начинается с `{`) → отдаём как есть.
        let out = body_to_text("", r#"  {"a":1}"#).unwrap();
        assert!(out.contains("\"a\":1"), "got: {out}");
    }

    #[test]
    fn html_without_readable_text_yields_none() {
        // HTML без читаемого текста (только скрипты) → извлекать нечего.
        assert!(body_to_text("text/html", "<html><script>var x=1;</script></html>").is_none());
    }

    #[test]
    fn html_with_paragraph_is_extracted() {
        let out = body_to_text(
            "text/html; charset=utf-8",
            "<html><body><p>Реальный читаемый абзац страницы, достаточно длинный, \
             чтобы пройти порог отсева коротких фрагментов.</p></body></html>",
        )
        .unwrap();
        assert!(out.contains("читаемый абзац"), "got: {out}");
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
