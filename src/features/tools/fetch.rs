//! `fetch_url` tool (spec §9.3): fetch a web page and summarize it.
//!
//! Under the global switch `tools.web_enabled` (network access/privacy, like
//! `web_search`). Steps: fetch the page via its own `reqwest` client →
//! extract readable text (`web::extract_readable`, reusing readability)
//! → summarize via `ctx.engine` (an independent single-turn request, like
//! `call_subagent`). With `summarize=false`, the extracted text is returned
//! without calling the model (the fast path).

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest};

use super::web::{ACCEPT_HTML, ACCEPT_LANGUAGE, USER_AGENT, extract_readable, truncate_chars};
use super::{Tool, ToolContext, ToolOutcome};

/// Page-fetch timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Ceiling on extracted text (characters): what goes into summarization/the return value.
const MAX_CONTENT_CHARS: usize = 12_000;
/// Token limit for the summary reply.
const SUMMARY_MAX_TOKENS: usize = 768;
/// Summarization timeout (a model call).
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

/// `fetch_url` — fetch a page and (by default) summarize it.
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

    /// Fetches the page and extracts readable text. An error → a clear message
    /// (in the scaffold language `loc` — goes to the model in the `fetch_url` result).
    async fn fetch_text(&self, url: &str, loc: &crate::shared::i18n::Locale) -> Result<String> {
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, ACCEPT_HTML)
            .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
            .send()
            .await
            .with_context(|| loc.tf("tool.fetch_url.err.request", &[("url", url)]))?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!(loc.tf(
                "tool.fetch_url.err.status",
                &[("status", &status.to_string())]
            ));
        }
        // Read Content-Type BEFORE consuming the body (`resp.text()` takes resp).
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp
            .text()
            .await
            .with_context(|| loc.tf("tool.fetch_url.err.read", &[("url", url)]))?;
        body_to_text(&content_type, &body)
            .ok_or_else(|| anyhow::anyhow!(loc.t("tool.fetch_url.err.no_text").to_string()))
    }
}

/// Picks the text to return from the response body. Non-HTML **text** responses
/// (JSON/text/csv/JS — detected by `Content-Type`, or by the body's JSON shape
/// when it's absent) are returned as-is (truncated): this is API data, readability
/// doesn't apply to it (no `<p>`/`<li>`) — this is exactly why the JSON Steam API used to
/// give "failed to extract readable text". HTML → extract readable text.
/// `None` — nothing to extract (empty). A pure function — testable without a network.
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
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.fetch_url.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": loc.t("tool.fetch_url.param.url")},
                "focus": {
                    "type": "string",
                    "description": loc.t("tool.fetch_url.param.focus")
                },
                "summarize": {
                    "type": "boolean",
                    "description": loc.t("tool.fetch_url.param.summarize")
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
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.fetch_url.err.url_empty")))?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            anyhow::bail!(ctx.loc.t("tool.fetch_url.err.url_scheme"));
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

        let text = match self.fetch_text(url, ctx.loc).await {
            Ok(t) => t,
            Err(err) => {
                return Ok(ToolOutcome::text(ctx.loc.tf(
                    "tool.fetch_url.result.fetch_failed",
                    &[("url", url), ("err", &err.to_string())],
                )));
            }
        };

        if !summarize {
            return Ok(ToolOutcome::text(format!(
                "{}\n{}",
                ctx.loc.tf("tool.fetch_url.result.content", &[("url", url)]),
                truncate_chars(&text, MAX_CONTENT_CHARS)
            )));
        }

        match summarize_text(ctx, url, focus, &text).await {
            Ok(summary) if !summary.trim().is_empty() => Ok(ToolOutcome::text(summary)),
            // Summarization failed/came back empty → return the extracted text (graceful
            // degradation: the model still gets the page's content).
            _ => Ok(ToolOutcome::text(format!(
                "{}\n{}",
                ctx.loc
                    .tf("tool.fetch_url.result.content_no_summary", &[("url", url)]),
                truncate_chars(&text, MAX_CONTENT_CHARS)
            ))),
        }
    }
}

/// Summarizes the page text via an independent single-turn request to the model
/// (like `call_subagent`: no history/tools, with a token/time limit).
async fn summarize_text(
    ctx: &ToolContext,
    url: &str,
    focus: Option<&str>,
    text: &str,
) -> Result<String> {
    let system = ctx.loc.t("tool.fetch_url.summarize.system").to_string();
    let task = match focus {
        Some(f) => ctx.loc.tf(
            "tool.fetch_url.summarize.task_focus",
            &[("url", url), ("f", f), ("text", text)],
        ),
        None => ctx.loc.tf(
            "tool.fetch_url.summarize.task",
            &[("url", url), ("text", text)],
        ),
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
        tools: Vec::new(), // no tools (a nesting ban)
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
            anyhow::bail!(ctx.loc.t("tool.fetch_url.err.summary_timeout"));
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

    /// An engine that remembers the last request and returns a fixed reply.
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

    #[test]
    fn fetch_url_description_and_summary_system_localized() {
        // The description and the summarization system prompt are localized (en≠ru,
        // no Cyrillic). §3.5 docs/history/i18n.md.
        use crate::shared::i18n::{Lang, locale};
        let tool = FetchUrl::new();
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        let no_cyr = |s: &str| {
            !s.chars()
                .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
        };
        assert_ne!(tool.description(ru), tool.description(en));
        assert!(no_cyr(&tool.description(en)));
        assert_ne!(
            ru.t("tool.fetch_url.summarize.system"),
            en.t("tool.fetch_url.summarize.system")
        );
        assert!(no_cyr(en.t("tool.fetch_url.summarize.system")));
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

    /// Summarization builds a request with no tools, with a token limit, and includes
    /// focus in the task. Checks the pure function `summarize_text` directly.
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
        assert!(req.tools.is_empty(), "no tools (a nesting ban)");
        assert!(req.sampling.max_tokens.unwrap() <= SUMMARY_MAX_TOKENS);
        // focus made it into the task.
        let msg = format!("{:?}", req.messages[0]);
        assert!(msg.contains("какова цена"), "focus in the task: {msg}");
    }

    #[test]
    fn json_body_returned_as_is_not_extracted() {
        // A JSON API response (by Content-Type) is returned as-is — readability used to
        // return empty → "failed to extract readable text" (Steam appreviews).
        let body = r#"{"success":1,"query_summary":{"total_positive":200,"total_negative":30}}"#;
        let out = body_to_text("application/json; charset=utf-8", body).unwrap();
        assert!(out.contains("total_positive"), "got: {out}");
    }

    #[test]
    fn json_shaped_body_returned_when_content_type_missing() {
        // No Content-Type, but the body has a JSON shape (starts with `{`) → return as-is.
        let out = body_to_text("", r#"  {"a":1}"#).unwrap();
        assert!(out.contains("\"a\":1"), "got: {out}");
    }

    #[test]
    fn html_without_readable_text_yields_none() {
        // HTML with no readable text (only scripts) → nothing to extract.
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

    /// A real network smoke (manual: `cargo test -- --ignored`).
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
