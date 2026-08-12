//! `fetch_url` tool (spec §9.3): fetch a web page and summarize it.
//!
//! Under the global switch `tools.web_enabled` (network access/privacy, like
//! `web_search`). Steps: fetch the page via its own `reqwest` client →
//! extract readable text (`web::extract_rich` — **headings and code blocks
//! included**, unlike the prose-only extraction `web_search` uses for ranking)
//! → summarize via `ctx.engine` (an independent single-turn request, like
//! `call_subagent`). With `summarize=false`, the extracted text is returned
//! without calling the model (the fast path).
//!
//! A page too big for one result **is attached to the chat** (spec §9.7) instead
//! of being silently cut: an attachment is already paged (`attachment_read`) and
//! searchable (`attachment_search`), so nothing is lost and the model can reach
//! all of it. The threshold is the attachment budget itself, exactly as
//! `youtube_watch(transcript:)` uses it. See
//! docs/history/fetch-url-fidelity.md (forks F1a/F2b).

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::entities::attachment::{Attachment, decide_mode, inline_tokens_excluding};
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest};

use super::web::{ACCEPT_HTML, ACCEPT_LANGUAGE, USER_AGENT, extract_rich, truncate_chars};
use super::{ChatEffect, Tool, ToolContext, ToolOutcome};

/// Page-fetch timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Hard ceiling on the extracted text of one page (characters) — a bound on what
/// a single page may put into a chat. Far above the attachment budget: beyond
/// that budget the text is attached rather than cut, so this only fires on a
/// genuinely enormous page, and when it does the result **says so** (unlike the
/// silent 12 000-character cut this replaced — see the plan doc, P2).
const MAX_EXTRACT_CHARS: usize = 400_000;
/// How much of the text goes into the summarizer: it is a single-turn subagent
/// with its own context, so a 250 000-character page cannot be handed over
/// whole. When the page exceeds this the summary covers its head, and the full
/// text travels as the attachment.
const SUMMARY_INPUT_CHARS: usize = 12_000;
/// Ceiling on the page title used as the attachment's display name.
const NAME_TITLE_CHARS: usize = 60;
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
    async fn fetch_text(&self, url: &str, loc: &crate::shared::i18n::Locale) -> Result<PageText> {
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

/// The page's extracted text plus what the caller has to be honest about.
pub(crate) struct PageText {
    pub text: String,
    /// The text hit [`MAX_EXTRACT_CHARS`] — this is not the whole page.
    pub truncated: bool,
    /// The page's `<title>`, when it had one (the attachment's display name).
    pub title: Option<String>,
}

/// Picks the text to return from the response body. Non-HTML **text** responses
/// (JSON/text/csv/JS — detected by `Content-Type`, or by the body's JSON shape
/// when it's absent) are returned as-is: this is API data, readability
/// doesn't apply to it (no `<p>`/`<li>`) — this is exactly why the JSON Steam API used to
/// give "failed to extract readable text". HTML → rich extraction
/// (`web::extract_rich`: prose **plus headings and code blocks**).
/// `None` — nothing to extract (empty). A pure function — testable without a network.
fn body_to_text(content_type: &str, body: &str) -> Option<PageText> {
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
            return Some(PageText {
                text: truncate_chars(trimmed, MAX_EXTRACT_CHARS),
                truncated: trimmed.chars().count() > MAX_EXTRACT_CHARS,
                title: None,
            });
        }
    }
    let text = extract_rich(body, MAX_EXTRACT_CHARS);
    (!text.is_empty()).then(|| PageText {
        truncated: text.chars().count() >= MAX_EXTRACT_CHARS,
        text,
        title: page_name(body),
    })
}

/// The page's name for the attachment: **`<h1>` first**, `<title>` second.
/// Measured on the site from the transcript — `docs.vlang.io` gives every page
/// the same `<title>` ("V Documentation") while `<h1>` is the actual page
/// ("Memory management" / "Concurrency"), so taking the title would name two
/// different pages of one site identically, and `attachment_read` resolves a
/// name to the **first** match — a silently wrong page.
fn page_name(body: &str) -> Option<String> {
    let doc = scraper::Html::parse_document(body);
    let pick = |q: &str| -> Option<String> {
        let sel = scraper::Selector::parse(q).ok()?;
        let raw = doc.select(&sel).next()?.text().collect::<String>();
        let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        let clipped: String = collapsed.chars().take(NAME_TITLE_CHARS).collect();
        let t = clipped.trim().to_string();
        (!t.is_empty()).then_some(t)
    };
    pick("h1").or_else(|| pick("title"))
}

/// Keeps the display name unique within the chat: a site whose `<h1>` is as
/// constant as its `<title>` would still collide, so a name already taken by a
/// **different** page gets the URL's last segment appended. Deterministic (no
/// counters), so re-fetching the same page produces the same name and replaces
/// its own attachment rather than piling up copies.
fn unique_name(base: &str, url: &str, existing: &[Attachment]) -> String {
    let taken = existing
        .iter()
        .any(|a| a.name.eq_ignore_ascii_case(base) && !a.source.eq_ignore_ascii_case(url));
    match taken.then(|| url_segment(url)).flatten() {
        Some(seg) => format!("{base} — {seg}"),
        None => base.to_string(),
    }
}

/// The URL's last non-empty path segment, else its host — the shortest thing
/// that still tells two pages of one site apart.
fn url_segment(url: &str) -> Option<String> {
    let without_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let path = without_scheme.split(['?', '#']).next().unwrap_or("");
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let host = parts.next()?;
    Some(parts.next_back().unwrap_or(host).to_string())
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
        "fetch page"
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
        // A YouTube watch page is a JavaScript shell: measured, it has zero
        // paragraphs and zero list items, so readability extracts nothing and
        // this used to answer "failed to extract readable text" — a dead end the
        // model cannot reason its way out of. Hand back what the free paths know
        // and point at the tool that can actually watch it (fork R6,
        // docs/research/youtube-integration.md §1).
        if super::youtube::is_youtube_url(url)
            && let Some(id) = super::youtube::video_id(url)
        {
            let meta = super::youtube::fetch_meta(&self.http, &id)
                .await
                .unwrap_or_default();
            let mut out = super::youtube::YoutubeWatch::meta_block(
                &meta,
                &super::youtube::watch_url(&id),
                ctx.loc,
            );
            out.push('\n');
            out.push_str(ctx.loc.t("tool.fetch_url.result.youtube"));
            return Ok(ToolOutcome::text(out));
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

        let page = match self.fetch_text(url, ctx.loc).await {
            Ok(t) => t,
            Err(err) => {
                return Ok(ToolOutcome::text(ctx.loc.tf(
                    "tool.fetch_url.result.fetch_failed",
                    &[("url", url), ("err", &err.to_string())],
                )));
            }
        };

        // Small enough to read at once → straight into the result: the model
        // needs no second call, and attaching would put the same text in the
        // pinned block *and* in the history. The threshold is the attachment
        // budget itself (sub-decision S1), which is also why an attached page is
        // always by reference: inline requires `est <= max_file_tokens`, and this
        // branch is exactly the other side of it.
        let est = crate::shared::tokens::estimate_text(&page.text) as usize;
        if est <= ctx.attachment_cfg.max_file_tokens {
            return Ok(ToolOutcome::text(
                self.inline_result(ctx, url, focus, summarize, &page).await,
            ));
        }
        Ok(self.attached_result(ctx, url, focus, summarize, page).await)
    }
}

impl FetchUrl {
    /// The page fits the budget: previous behaviour — a summary, or the text
    /// itself when `summarize=false` (or when summarization failed).
    async fn inline_result(
        &self,
        ctx: &ToolContext,
        url: &str,
        focus: Option<&str>,
        summarize: bool,
        page: &PageText,
    ) -> String {
        let mut out = if summarize {
            match summarize_text(ctx, url, focus, &page.text).await {
                Ok(s) if !s.trim().is_empty() => s,
                // Summarization failed/came back empty → return the extracted text
                // (graceful degradation: the model still gets the page's content).
                _ => format!(
                    "{}\n{}",
                    ctx.loc
                        .tf("tool.fetch_url.result.content_no_summary", &[("url", url)]),
                    page.text
                ),
            }
        } else {
            format!(
                "{}\n{}",
                ctx.loc.tf("tool.fetch_url.result.content", &[("url", url)]),
                page.text
            )
        };
        if page.truncated {
            out.push('\n');
            out.push_str(ctx.loc.t("tool.fetch_url.result.truncated"));
        }
        out
    }

    /// The page is over the attachment budget: attach it whole (spec §9.7) and
    /// return a summary of its head plus how to reach the rest. The alternative —
    /// cutting the text into the result — is what made a long documentation page
    /// indistinguishable from a complete one (the plan doc, P2).
    async fn attached_result(
        &self,
        ctx: &ToolContext,
        url: &str,
        focus: Option<&str>,
        summarize: bool,
        page: PageText,
    ) -> ToolOutcome {
        let base = page.title.clone().unwrap_or_else(|| url.to_string());
        let name = unique_name(&base, url, &ctx.attachments);
        let header = ctx.loc.tf(
            "tool.fetch_url.attachment.header",
            &[("name", &name), ("url", url)],
        );
        let text = format!("{header}\n\n{}", page.text);
        // Through the shared rule rather than hardcoding `ByReference`: that is a
        // *consequence* of the threshold above, and the orchestrator decides
        // `/file attach` the same way, so the two cannot drift.
        let used = inline_tokens_excluding(&ctx.attachments, url);
        let est = crate::shared::tokens::estimate_text(&text) as usize;
        let mode = decide_mode(est, used, &ctx.attachment_cfg);
        let bytes = text.len();
        let attachment = Attachment::new(name.clone(), url.to_string(), text, bytes, mode);
        let pages = attachment.page_count(ctx.attachment_cfg.page_tokens);

        let mut out = String::new();
        if summarize {
            // The head only: the summarizer is a single-turn subagent with its
            // own context. The result says the whole page is attached, so a
            // partial summary is a starting point rather than the only access.
            let head = truncate_chars(&page.text, SUMMARY_INPUT_CHARS);
            if let Ok(s) = summarize_text(ctx, url, focus, &head).await
                && !s.trim().is_empty()
            {
                out.push_str(s.trim());
                out.push('\n');
            }
        }
        out.push_str(&ctx.loc.tf(
            "tool.fetch_url.result.attached",
            &[("name", &name), ("pages", &pages.to_string())],
        ));
        if page.truncated {
            out.push('\n');
            out.push_str(ctx.loc.t("tool.fetch_url.result.truncated"));
        }
        ToolOutcome::with_effects(out, vec![ChatEffect::AddAttachment(Box::new(attachment))])
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
                // A background turn: the retry is worth a log line (a flaky provider is
                // otherwise invisible here) but has nothing to show — these turns have no
                // chip of their own.
                ChatChunk::Retry {
                    attempt,
                    max,
                    delay,
                } => {
                    tracing::info!(attempt, max, ?delay, "retrying a a page-summary turn");
                }
                ChatChunk::Error { message, .. } => {
                    tracing::warn!(error = %message, "engine error while summarizing a page");
                }
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

    /// A page big enough to be over the attachment budget.
    fn big_page(title: Option<&str>) -> PageText {
        // Deliberately long enough that `estimate_text` clears
        // `max_file_tokens` (4000 by default) several times over.
        let body = "Абзац с содержательным текстом страницы. ".repeat(2000);
        PageText {
            text: format!("НАЧАЛО\n{body}\nКОНЕЦ"),
            truncated: false,
            title: title.map(str::to_string),
        }
    }

    /// The defect this closes (plan doc, P2): a long page used to be cut at
    /// 12 000 characters, mid-word, with nothing saying so and no way to reach
    /// the rest. Now it arrives whole as an attachment.
    #[tokio::test]
    async fn a_page_over_the_budget_is_attached_whole() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        let page = big_page(Some("Управление памятью в V"));
        let full = page.text.clone();
        let out = FetchUrl::new()
            .attached_result(&ctx, "https://docs.vlang.io/x.html", None, false, page)
            .await;

        let [ChatEffect::AddAttachment(att)] = out.effects.as_slice() else {
            panic!("no attachment effect: {:?}", out.effects);
        };
        assert!(
            att.text.contains("НАЧАЛО") && att.text.contains("КОНЕЦ"),
            "the text is not whole"
        );
        assert!(att.text.contains(&full), "the extracted text was altered");
        assert_eq!(
            att.name, "Управление памятью в V",
            "the page title names it"
        );
        assert_eq!(att.source, "https://docs.vlang.io/x.html");
        // Over `max_file_tokens` by construction — the other side of the same
        // threshold, so an attachment made this way is never inline.
        assert_eq!(
            att.mode,
            crate::entities::attachment::AttachMode::ByReference
        );
        // The attachment carries its own header, so the file still says what it
        // is when it is read page by page much later.
        assert!(
            att.text.contains("https://docs.vlang.io/x.html"),
            "no source in the header"
        );

        // And the result must say where the text went and how to reach it —
        // otherwise this repeats P2 in a politer form.
        let pages = att.page_count(ctx.attachment_cfg.page_tokens).to_string();
        assert!(
            out.result.contains("attachment_read"),
            "got: {}",
            out.result
        );
        assert!(
            out.result.contains("attachment_search"),
            "got: {}",
            out.result
        );
        assert!(out.result.contains(&pages), "no page count: {}", out.result);
    }

    #[tokio::test]
    async fn a_titleless_page_is_named_by_its_url() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        let out = FetchUrl::new()
            .attached_result(&ctx, "https://example.com/a", None, false, big_page(None))
            .await;
        let [ChatEffect::AddAttachment(att)] = out.effects.as_slice() else {
            panic!("no attachment effect");
        };
        assert_eq!(att.name, "https://example.com/a");
    }

    /// `summarize=true` still summarizes — from the head, since the summarizer is
    /// a single-turn subagent — and still attaches, so a partial summary is a
    /// starting point rather than the only access to the page.
    #[tokio::test]
    async fn an_attached_page_is_still_summarized_from_its_head() {
        let backend = Arc::new(CapturingBackend {
            last: Mutex::new(None),
            reply: "краткое содержание".into(),
        });
        let (_d, ctx) = ctx_with_engine(backend.clone());
        let out = FetchUrl::new()
            .attached_result(&ctx, "https://example.com", None, true, big_page(None))
            .await;
        assert!(
            out.result.contains("краткое содержание"),
            "got: {}",
            out.result
        );
        assert!(
            out.result.contains("attachment_read"),
            "got: {}",
            out.result
        );
        assert_eq!(
            out.effects.len(),
            1,
            "the page is attached as well as summarized"
        );

        let req = backend.last.lock().unwrap().take().unwrap();
        let sent = format!("{:?}", req.messages[0]);
        assert!(
            sent.chars().count() < SUMMARY_INPUT_CHARS * 2,
            "the whole page went to the summarizer ({} chars)",
            sent.chars().count()
        );
    }

    /// The size ceiling stays, but it is announced — the other half of P2.
    #[tokio::test]
    async fn hitting_the_size_ceiling_is_announced() {
        use crate::shared::i18n::{Lang, locale};
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        let mut page = big_page(None);
        page.truncated = true;
        let out = FetchUrl::new()
            .attached_result(&ctx, "https://example.com", None, false, page)
            .await;
        let marker = locale(Lang::Ru).t("tool.fetch_url.result.truncated");
        assert!(out.result.contains(marker), "got: {}", out.result);

        // The same on the path where the page fits and is returned inline.
        let small = PageText {
            text: "короткий текст".into(),
            truncated: true,
            title: None,
        };
        let inline = FetchUrl::new()
            .inline_result(&ctx, "https://example.com", None, false, &small)
            .await;
        assert!(inline.contains(marker), "got: {inline}");
    }

    /// A page within the budget keeps the previous behaviour: the text itself,
    /// in the result, with no attachment.
    #[tokio::test]
    async fn a_small_page_comes_back_in_the_result() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        let page = PageText {
            text: "Небольшая страница целиком.".into(),
            truncated: false,
            title: Some("t".into()),
        };
        let out = FetchUrl::new()
            .inline_result(&ctx, "https://example.com", None, false, &page)
            .await;
        assert!(out.contains("Небольшая страница целиком."), "got: {out}");
    }

    #[test]
    fn the_attachment_is_named_by_h1_then_title() {
        // Measured on the site from the transcript: `docs.vlang.io` gives
        // **every** page the same `<title>` ("V Documentation") while `<h1>`
        // names the page. Taking the title would give two pages of one site one
        // name, and `attachment_read` resolves a name to the first match — a
        // silently wrong page.
        let page = |head: &str, body: &str| {
            body_to_text(
                "text/html",
                &format!(
                    "<html><head>{head}</head><body>{body}\
                     <p>Достаточно длинный абзац, чтобы пройти порог отсева фрагментов.</p>\
                     </body></html>"
                ),
            )
            .unwrap()
        };
        let out = page(
            "<title>V Documentation</title>",
            "<h1>Memory management</h1>",
        );
        assert_eq!(out.title.as_deref(), Some("Memory management"));
        assert!(!out.truncated);
        // No h1 → the title, collapsed and trimmed as before.
        let out = page("<title>  Memory\n management  </title>", "");
        assert_eq!(out.title.as_deref(), Some("Memory management"));
    }

    #[test]
    fn a_name_already_taken_by_another_page_gets_the_url_segment() {
        use crate::entities::attachment::AttachMode;
        let mine = "https://docs.example.io/memory-management.html";
        let other = Attachment::new(
            "V Documentation",
            "https://docs.example.io/concurrency.html",
            "x".into(),
            1,
            AttachMode::ByReference,
        );
        assert_eq!(
            unique_name("V Documentation", mine, std::slice::from_ref(&other)),
            "V Documentation — memory-management.html"
        );
        // Re-fetching the *same* page keeps the plain name: the attachment it
        // replaces is its own, so nothing collides and the name stays stable.
        let same = Attachment::new(
            "V Documentation",
            mine,
            "x".into(),
            1,
            AttachMode::ByReference,
        );
        assert_eq!(
            unique_name("V Documentation", mine, &[same]),
            "V Documentation"
        );
        assert_eq!(unique_name("V Documentation", mine, &[]), "V Documentation");
    }

    #[test]
    fn url_segment_falls_back_to_the_host() {
        assert_eq!(
            url_segment("https://a.io/x/y.html?q=1").as_deref(),
            Some("y.html")
        );
        assert_eq!(url_segment("https://a.io/").as_deref(), Some("a.io"));
        assert_eq!(url_segment("https://a.io").as_deref(), Some("a.io"));
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
        let out = body_to_text("application/json; charset=utf-8", body)
            .unwrap()
            .text;
        assert!(out.contains("total_positive"), "got: {out}");
    }

    #[test]
    fn json_shaped_body_returned_when_content_type_missing() {
        // No Content-Type, but the body has a JSON shape (starts with `{`) → return as-is.
        let out = body_to_text("", r#"  {"a":1}"#).unwrap().text;
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
        .unwrap()
        .text;
        assert!(out.contains("читаемый абзац"), "got: {out}");
    }

    /// A YouTube link used to be a dead end here — the watch page is a
    /// JavaScript shell, so readability extracted nothing and the answer was
    /// "failed to extract readable text". Now it hands back what the free paths
    /// know and points at `youtube_watch` (fork R6). Needs no key — only network.
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn live_youtube_link_returns_metadata_not_a_dead_end() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        let out = FetchUrl::new()
            .invoke(
                &ctx,
                serde_json::json!({"url": "https://youtu.be/dQw4w9WgXcQ"}),
            )
            .await
            .unwrap();
        eprintln!("--- fetch_url on a YouTube link ---\n{}", out.result);
        assert!(
            out.result.contains("Never Gonna Give You Up"),
            "the live watch page's title is missing: {}",
            out.result
        );
        assert!(
            out.result.contains(super::super::YOUTUBE_WATCH_ID),
            "the answer must point at the tool that can watch it: {}",
            out.result
        );
    }

    /// The page from the transcript that prompted this work
    /// (docs/history/fetch-url-fidelity.md). Two claims that only a live fetch
    /// can settle, because both depend on the real markup: the code examples
    /// come back (P1 — VitePress wraps them in a bare `div.language-v`, which
    /// prose extraction dropped), and a documentation page of this size arrives
    /// as an attachment rather than cut mid-word (P2). Needs no key — only network.
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn live_documentation_page_keeps_its_code() {
        let (_d, ctx) = ctx_with_engine(Arc::new(MockBackend::scripted(vec![])));
        let out = FetchUrl::new()
            .invoke(
                &ctx,
                serde_json::json!({
                    "url": "https://docs.vlang.io/memory-management.html",
                    "summarize": false
                }),
            )
            .await
            .unwrap();

        // The text is wherever the size rule put it — that is the point of the
        // rule, so the test asks for the content, not for a particular branch.
        let attached = match out.effects.as_slice() {
            [ChatEffect::AddAttachment(a)] => Some(a.clone()),
            [] => None,
            other => panic!("unexpected effects: {other:?}"),
        };
        let text = attached
            .as_ref()
            .map(|a| a.text.clone())
            .unwrap_or_else(|| out.result.clone());
        eprintln!(
            "--- fetch_url on the V docs page: {} chars, attached={} ---",
            text.chars().count(),
            attached.is_some()
        );

        assert!(
            text.contains("fn (data &MyType) free()"),
            "the code example is missing — prose-only extraction is back: {text}"
        );
        assert!(text.contains("```"), "code is not fenced: {text}");
        assert!(
            text.contains("## Control") || text.contains("# Control"),
            "section headings are missing: {text}"
        );
        assert!(
            text.contains("Arena allocation is available"),
            "the prose is missing: {text}"
        );
        if let Some(att) = attached {
            let pages = att.page_count(ctx.attachment_cfg.page_tokens);
            eprintln!(
                "attached as {:?}, {pages} page(s), mode {:?}",
                att.name, att.mode
            );
            assert!(
                out.result.contains("attachment_read"),
                "the result must say how to reach the attached page: {}",
                out.result
            );
        }
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
