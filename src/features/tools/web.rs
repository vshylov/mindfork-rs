//! `web_search` tool (spec §9.3.1): internet search via its own
//! `reqwest` client + parsing the HTML results page (`scraper`). Under the global
//! switch `tools.web_enabled` (privacy, §9.4).
//!
//! **Several independent providers with fallback** (see [`PROVIDERS`]). In
//! order: DuckDuckGo lite (`POST q=`, the simplest markup, an ADR decision from M7) →
//! DuckDuckGo html (different markup) → **Mojeek** → **Ecosia** (`GET ?q=`, each with
//! its own infrastructure and markup). The first one to return a non-empty result set wins.
//!
//! **Anti-bot throttling.** With several quick requests in a row (which happens in the
//! agentic loop on a complex/long task), search engines cut traffic by IP:
//! DuckDuckGo returns `HTTP 202` with a challenge page ("anomaly"), Mojeek/others —
//! `403`/`429`, not results. Previously `202` was treated as "success" (`error_for_status`
//! lets 2xx through) → an empty page got parsed → the model saw "the search returned no
//! results" (even though the query was valid), and a `403` surfaced as a fatal
//! "provider unavailable" error. Now throttling is detected ([`is_throttled`]) and on it
//! the next provider is tried right away (throttling is sticky per-IP — retries only
//! deepen it; different providers throttle independently, so almost always someone
//! answers). If **all** are unavailable/throttled — an explicit error is returned (not
//! "no results"), so the model retries the request later instead of reporting that it
//! found nothing.
//!
//! **Content extraction + reranking** (spec §9.3.1, on by default,
//! disabled via the `fetch_content` argument). After the results page arrives, results
//! are fetched and readable text is extracted from them ([`extract_readable`] via
//! `scraper`: the content of `<article>`/`<main>`/paragraphs, without script/nav clutter) —
//! "best effort": one page's fetch failure doesn't fail the whole search. Then results
//! are **reordered via embeddings** (through `ctx.embedder`, ADR 0002): the query and
//! each result's content are embedded, sorted by decreasing cosine similarity
//! ([`rerank_order`]). The embedder isn't configured/is unavailable (RAG is off) → reranking
//! is skipped, the provider order remains (graceful degradation, like RAG's).

use std::cmp::Ordering;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use scraper::{Html, Selector};

use crate::entities::profile::ToolId;
use crate::shared::api::{EmbedRole, Embedder};

use super::{Tool, ToolContext, ToolOutcome};

/// A UA so search engines return regular markup (not "lite"/empty).
/// `pub(crate)` — reused by `fetch_url` (see `tools/fetch.rs`).
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/124.0 Safari/537.36";
/// `Accept` for fetching content pages (like a browser's).
pub(crate) const ACCEPT_HTML: &str =
    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
/// `Accept-Language` for fetching content pages.
pub(crate) const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9,ru;q=0.8";
/// Default number of results.
const DEFAULT_MAX_RESULTS: usize = 5;
/// Hard ceiling on the number of results.
const MAX_RESULTS_CAP: usize = 10;
/// Timeout for one HTTP request to a search provider.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Ceiling on the extracted readable text of one page (characters). Limits
/// context bloat and the size of the embedding request.
const MAX_CONTENT_CHARS: usize = 1500;
/// Minimum fragment (paragraph) length on extraction: shorter is likely
/// navigation/menu/buttons, not content.
const MIN_FRAGMENT_CHARS: usize = 40;
/// How many characters of a result's content go into the embedding during reranking
/// (enough for a representative start; doesn't bloat the embedder request).
const RERANK_EMBED_CHARS: usize = 800;

/// HTTP method for the request to the search provider.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Method {
    /// `q` in the form body (DuckDuckGo).
    PostForm,
    /// `q` in the query string (Mojeek).
    GetQuery,
}

/// A search provider's description: endpoint, method, and the results' CSS selectors.
struct Provider {
    /// The name for logs/errors.
    name: &'static str,
    url: &'static str,
    method: Method,
    /// The result-link selector (where `href` comes from).
    link_sel: &'static str,
    /// The title selector (its text). For DDG/Mojeek it matches `link_sel` (the title
    /// and the link are one `<a>` tag); for Ecosia the title sits apart from the link.
    title_sel: &'static str,
    /// The snippet selector. Lists of links/titles/snippets are aligned by
    /// index (result i = link i + title i + snippet i).
    snippet_sel: &'static str,
}

/// Providers in order of preference, each with its own markup and (importantly)
/// infrastructure. DuckDuckGo (two markup variants) is primary; then the
/// independent **Mojeek** and **Ecosia**. Each has its own anti-bot throttling, and
/// it's short-lived (per-IP); it's sticky, retrying the same provider is
/// pointless — hence a throttle moves straight to the next one. Several
/// independent providers → when one is unavailable, another almost always answers.
const PROVIDERS: &[Provider] = &[
    Provider {
        name: "DuckDuckGo lite",
        url: "https://lite.duckduckgo.com/lite/",
        method: Method::PostForm,
        link_sel: "a.result-link",
        title_sel: "a.result-link",
        snippet_sel: "td.result-snippet",
    },
    Provider {
        name: "DuckDuckGo html",
        url: "https://html.duckduckgo.com/html/",
        method: Method::PostForm,
        link_sel: "a.result__a",
        title_sel: "a.result__a",
        snippet_sel: "a.result__snippet",
    },
    Provider {
        name: "Mojeek",
        url: "https://www.mojeek.com/search",
        method: Method::GetQuery,
        link_sel: "a.title",
        title_sel: "a.title",
        snippet_sel: "p.s",
    },
    Provider {
        name: "Ecosia",
        url: "https://www.ecosia.org/search",
        method: Method::GetQuery,
        // Stable semantic `data-test-id`s (not hashed css classes).
        link_sel: r#"a[data-test-id="result-link"]"#,
        title_sel: r#"[data-test-id="result-title"]"#,
        snippet_sel: r#"[data-test-id="web-result-description"]"#,
    },
];

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// The page's extracted readable text (empty if not fetched/it didn't work out).
    pub content: String,
}

/// `web_search` — internet search (DuckDuckGo → Mojeek → Ecosia, see [`PROVIDERS`]).
pub struct WebSearch {
    http: reqwest::Client,
    /// The default value for the `fetch_content` argument (from `config.tools`).
    fetch_content_default: bool,
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::new(true)
    }
}

impl WebSearch {
    pub fn new(fetch_content_default: bool) -> Self {
        // A request timeout: otherwise a hung DDG response would hold up the whole turn.
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            fetch_content_default,
        }
    }

    /// One HTTP request to a provider: `Ok(Some(html))` — a normal page;
    /// `Ok(None)` — anti-bot throttling (202/anomaly/403/429); `Err` — network/other HTTP.
    /// `loc` — the scaffold language for error texts (goes to the model on total failure).
    async fn fetch(
        &self,
        provider: &Provider,
        query: &str,
        loc: &crate::shared::i18n::Locale,
    ) -> Result<Option<String>> {
        let req = match provider.method {
            Method::PostForm => self.http.post(provider.url).form(&[("q", query)]),
            Method::GetQuery => {
                let mut url = reqwest::Url::parse(provider.url).with_context(|| {
                    loc.tf("tool.web_search.err.url_parse", &[("name", provider.name)])
                })?;
                url.query_pairs_mut().append_pair("q", query);
                self.http.get(url)
            }
        };
        let resp = req
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .with_context(|| loc.tf("tool.web_search.err.request", &[("name", provider.name)]))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .with_context(|| loc.tf("tool.web_search.err.read", &[("name", provider.name)]))?;
        if is_throttled(status, &body) {
            return Ok(None);
        }
        if !status.is_success() {
            anyhow::bail!(loc.tf(
                "tool.web_search.err.status",
                &[("name", provider.name), ("status", &status.to_string())]
            ));
        }
        Ok(Some(body))
    }

    /// Fetches a result page and extracts readable text. `None` on any
    /// error/non-HTML — extraction is "best effort", the search shouldn't fail because of
    /// one unavailable page.
    async fn fetch_content(&self, url: &str) -> Option<String> {
        let resp = match self
            .http
            .get(url)
            // Browser-like headers: some sites return an empty/block page
            // on a "bare" request with no Accept/Accept-Language.
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, ACCEPT_HTML)
            .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
            .send()
            .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::debug!(url, error = %err, "web search: the page failed to load");
                return None;
            }
        };
        if !resp.status().is_success() {
            tracing::debug!(url, status = %resp.status(), "web search: the page returned a non-2xx status");
            return None;
        }
        // Only take HTML (there's nothing to extract PDF/images/other with). The header
        // may be absent — then try as HTML.
        let is_html = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|t| t.contains("html"))
            .unwrap_or(true);
        if !is_html {
            return None;
        }
        let body = resp.text().await.ok()?;
        let text = extract_readable(&body, MAX_CONTENT_CHARS);
        if text.is_empty() {
            tracing::debug!(url, "web search: no readable text extracted from the page");
        }
        (!text.is_empty()).then_some(text)
    }

    /// Fetches result pages in parallel and sets the extracted text into
    /// `content`. Each fetch is independent and fault-tolerant (see [`Self::fetch_content`]).
    async fn enrich_with_content(&self, results: &mut [SearchResult]) {
        let contents =
            futures_util::future::join_all(results.iter().map(|r| self.fetch_content(&r.url)))
                .await;
        for (r, c) in results.iter_mut().zip(contents) {
            if let Some(c) = c {
                r.content = c;
            }
        }
    }
}

/// Reorders results by decreasing similarity of their content to the query
/// (reranking via embeddings, spec §9.3.1). The embedder is unavailable/returned a
/// mismatched vector count → results are left untouched (graceful degradation). `query` and
/// each result's content are embedded in one request.
async fn rerank_by_embeddings(
    embedder: &dyn Embedder,
    query: &str,
    results: &mut Vec<SearchResult>,
) {
    if results.len() < 2 {
        return; // nothing to reorder
    }
    // Two requests, not one: this is genuine asymmetric retrieval, so the query
    // and the page texts carry different roles (research §5.1/§5.2). One extra
    // round trip on a path that already issues N parallel page fetches.
    let query_vec = match embedder
        .embed(vec![query.to_string()], EmbedRole::Query)
        .await
    {
        Ok(mut v) if !v.is_empty() => v.remove(0),
        Ok(_) => {
            tracing::debug!("reranking skipped: the embedder returned no query vector");
            return;
        }
        Err(e) => {
            tracing::debug!(error = %e, "reranking skipped: the embedder is unavailable");
            return;
        }
    };
    let texts: Vec<String> = results.iter().map(rerank_text).collect();
    let vecs = match embedder.embed(texts, EmbedRole::Passage).await {
        Ok(v) if v.len() == results.len() => v,
        Ok(_) => return, // a mismatch — don't risk shuffling
        Err(err) => {
            tracing::debug!(error = %err, "web search: reranking unavailable, keeping the provider order");
            return;
        }
    };
    let order = rerank_order(&query_vec, &vecs);
    *results = order.into_iter().map(|i| results[i].clone()).collect();
}

/// A result's text for embedding during reranking: content (if extracted) with the
/// title/snippet as context; content is truncated to [`RERANK_EMBED_CHARS`].
fn rerank_text(r: &SearchResult) -> String {
    let body = if r.content.is_empty() {
        r.snippet.clone()
    } else {
        truncate_chars(&r.content, RERANK_EMBED_CHARS)
    };
    format!("{}\n{}", r.title, body).trim().to_string()
}

/// The order of `doc_vecs` indices by decreasing cosine similarity to `query_vec`.
fn rerank_order(query_vec: &[f32], doc_vecs: &[Vec<f32>]) -> Vec<usize> {
    let sims: Vec<f32> = doc_vecs.iter().map(|v| cosine(query_vec, v)).collect();
    let mut order: Vec<usize> = (0..doc_vecs.len()).collect();
    // A stable sort: with equal similarity the provider order is preserved.
    order.sort_by(|&a, &b| sims[b].partial_cmp(&sims[a]).unwrap_or(Ordering::Equal));
    order
}

/// Cosine similarity of two vectors (0.0 on a length mismatch/zero norm).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Extracts an HTML page's readable text (a simplified readability): takes paragraphs and
/// lists from `<article>`/`<main>` (if present), otherwise — from the whole document; short
/// fragments and everything inside nav/header/footer/sidebar ([`in_boilerplate`])
/// are dropped (otherwise on sites without semantic markup a mega-menu ends up in
/// the content). script/style don't get in (their text isn't inside `<p>`/`<li>`). The result
/// is truncated to `max_chars` characters.
///
/// `pub(crate)` — reused by `fetch_url` (see `tools/fetch.rs`).
pub(crate) fn extract_readable(html: &str, max_chars: usize) -> String {
    let doc = Html::parse_document(html);
    let scope_sel = Selector::parse("article, main").unwrap();
    let para_sel = Selector::parse("p, li").unwrap();

    let take = |el: scraper::ElementRef| -> Option<String> {
        if in_boilerplate(el) {
            return None;
        }
        let t = collapse_ws(&el.text().collect::<String>());
        (t.chars().count() >= MIN_FRAGMENT_CHARS).then_some(t)
    };

    // Prefer the main content (article/main) — less navigational noise.
    let mut parts: Vec<String> = doc
        .select(&scope_sel)
        .flat_map(|root| root.select(&para_sel).filter_map(take).collect::<Vec<_>>())
        .collect();
    if parts.is_empty() {
        parts = doc.select(&para_sel).filter_map(take).collect();
    }

    let mut out = String::new();
    for p in parts {
        if out.chars().count() >= max_chars {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&p);
    }
    truncate_chars(&out, max_chars)
}

/// Extracts a page keeping what a *reader* needs and prose extraction throws
/// away: **headings** (the document's structure) and **code blocks**, in document
/// order, rendered Markdown-ish (`## Heading`, fenced code with its language).
///
/// Only `fetch_url` uses this (fork F1a, docs/history/fetch-url-fidelity.md):
/// `web_search` budgets 1500 characters per result page for ranking, where
/// headings and code would spend the budget without helping. On a documentation
/// page the difference is not cosmetic — with prose-only extraction every "here
/// is an example:" leads nowhere, which is exactly how one `fetch_url` call
/// turned into six `python_exec` rounds in the transcript that prompted this.
///
/// The result is truncated to `max_chars` characters.
pub(crate) fn extract_rich(html: &str, max_chars: usize) -> String {
    let doc = Html::parse_document(html);
    let scope_sel = Selector::parse("article, main").unwrap();
    // `pre` covers the standard case (including Prism's `pre.language-x`);
    // `div[class*="language-"]` covers VitePress/VuePress, which wrap code in a
    // bare `div` with no `pre` at all — the shape of the page in the transcript.
    let block_sel = Selector::parse(
        r#"h1, h2, h3, h4, h5, h6, p, li, blockquote, pre, div[class*="language-"]"#,
    )
    .unwrap();

    let collect = |root: scraper::ElementRef| -> Vec<Block> {
        // A container that already emitted its content must not emit it again
        // (`div.language-v` wrapping a `pre`). Selection is in document order, so
        // an ancestor is always seen first — hence "skip if an ancestor emitted".
        let mut emitted = std::collections::HashSet::new();
        let mut out = Vec::new();
        for el in root.select(&block_sel) {
            if in_boilerplate(el) || el.ancestors().any(|a| emitted.contains(&a.id())) {
                continue;
            }
            if let Some(b) = block_from(el) {
                emitted.insert(el.id());
                out.push(b);
            }
        }
        out
    };

    let mut blocks: Vec<Block> = doc.select(&scope_sel).flat_map(collect).collect();
    if blocks.is_empty() {
        blocks = collect(doc.root_element());
    }

    let mut out = String::new();
    let mut prev: Option<Kind> = None;
    for b in blocks {
        if out.chars().count() >= max_chars {
            break;
        }
        if let Some(p) = prev {
            // Consecutive list items read as a list; everything else gets a blank
            // line, so headings and fences land as valid Markdown.
            out.push_str(if p == Kind::Item && b.kind == Kind::Item {
                "\n"
            } else {
                "\n\n"
            });
        }
        out.push_str(&b.text);
        prev = Some(b.kind);
    }
    truncate_chars(&out, max_chars)
}

/// What a rich-extraction block is — drives only the spacing between blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Heading,
    Item,
    Other,
}

/// One rendered block of rich extraction.
struct Block {
    kind: Kind,
    text: String,
}

/// Renders one element into a rich-extraction block, or `None` if there's
/// nothing worth keeping. Headings and code are **exempt** from
/// [`MIN_FRAGMENT_CHARS`]: that floor exists to drop navigation chrome from
/// search snippets, and a two-word heading or a one-line example is content.
fn block_from(el: scraper::ElementRef) -> Option<Block> {
    let name = el.value().name();
    if let Some(level) = heading_level(name) {
        let t = collapse_ws(&el.text().collect::<String>());
        return (!t.is_empty()).then(|| Block {
            kind: Kind::Heading,
            text: format!("{} {t}", "#".repeat(level)),
        });
    }
    if name == "pre" || has_language_class(el) {
        // Whitespace is the code's meaning — `collapse_ws` would destroy it.
        let raw = el.text().collect::<String>();
        let code = raw.trim_matches('\n').trim_end();
        return (!code.trim().is_empty()).then(|| Block {
            kind: Kind::Other,
            text: format!("```{}\n{code}\n```", code_language(el).unwrap_or_default()),
        });
    }
    let t = collapse_ws(&el.text().collect::<String>());
    if t.chars().count() < MIN_FRAGMENT_CHARS {
        return None;
    }
    Some(if name == "li" {
        Block {
            kind: Kind::Item,
            text: format!("- {t}"),
        }
    } else {
        Block {
            kind: Kind::Other,
            text: t,
        }
    })
}

/// `1..=6` for `h1`..`h6`.
fn heading_level(name: &str) -> Option<usize> {
    name.strip_prefix('h')
        .and_then(|n| n.parse().ok())
        .filter(|l| (1..=6).contains(l))
}

/// `true` if the element itself carries a `language-*`/`lang-*` class (a code
/// container in the VitePress/Prism conventions).
fn has_language_class(el: scraper::ElementRef) -> bool {
    el.value().classes().any(is_language_class)
}

fn is_language_class(c: &str) -> bool {
    c.starts_with("language-") || c.starts_with("lang-")
}

/// The code block's language: from a `language-*`/`lang-*` class on the element
/// itself or on an inner `<code>` (`<pre><code class="language-rust">` — the
/// common highlighter output).
fn code_language(el: scraper::ElementRef) -> Option<String> {
    let from = |e: scraper::ElementRef| -> Option<String> {
        e.value()
            .classes()
            .find(|c| is_language_class(c))
            .and_then(|c| c.split_once('-'))
            .map(|(_, lang)| lang.to_string())
            .filter(|l| !l.is_empty())
    };
    from(el).or_else(|| {
        let code_sel = Selector::parse("code").unwrap();
        el.select(&code_sel).next().and_then(from)
    })
}

/// `true` if the element sits inside nav/header/footer/sidebar — this is
/// boilerplate (a menu/links), not the main content.
fn in_boilerplate(el: scraper::ElementRef) -> bool {
    el.ancestors().any(|n| {
        n.value()
            .as_element()
            .map(|e| matches!(e.name(), "nav" | "header" | "footer" | "aside"))
            .unwrap_or(false)
    })
}

/// Truncates a string to `max` characters (on a character boundary, not a byte one).
/// `pub(crate)` — reused by `fetch_url` (see `tools/fetch.rs`).
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn id(&self) -> ToolId {
        super::WEB_SEARCH_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "web search"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Web)
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.web_search.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": MAX_RESULTS_CAP},
                "fetch_content": {
                    "type": "boolean",
                    "description": loc.t("tool.web_search.param.fetch_content")
                }
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.web_search.err.query_empty")))?;
        let max = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_RESULTS_CAP))
            .unwrap_or(DEFAULT_MAX_RESULTS);
        let fetch_content = args
            .get("fetch_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(self.fetch_content_default);

        // Go through providers in order: the first one to return a non-empty result set
        // wins. On throttling, move to the next one right away (retrying a sticky
        // per-IP throttle is pointless). `got_clean_page` — at least one provider
        // returned a normal (non-challenge) page: then emptiness is a genuine "no
        // results", not throttling.
        let mut got_clean_page = false;
        let mut last_err: Option<anyhow::Error> = None;
        let mut results = Vec::new();
        for provider in PROVIDERS {
            match self.fetch(provider, query, ctx.loc).await {
                Ok(Some(html)) => {
                    let r = parse_results(
                        &html,
                        provider.link_sel,
                        provider.title_sel,
                        provider.snippet_sel,
                        max,
                    );
                    if !r.is_empty() {
                        results = r;
                        break;
                    }
                    // Empty parse: either the query genuinely has no matches, or
                    // this is an anti-bot challenge served with HTTP 200 (Mojeek
                    // does exactly that — measured). The check runs only here, on
                    // an empty parse, so a results page can never be mistaken for
                    // a challenge (a search for "captcha" keeps working).
                    if is_challenge_page(&html) {
                        tracing::debug!(
                            provider = provider.name,
                            "web search: anti-bot challenge behind a 200, trying the next provider"
                        );
                    } else {
                        got_clean_page = true;
                    }
                }
                Ok(None) => {
                    tracing::debug!(
                        provider = provider.name,
                        "web search: throttled, trying the next provider"
                    );
                }
                Err(err) => {
                    tracing::warn!(provider = provider.name, error = %err, "web search: provider error");
                    last_err = Some(err);
                }
            }
        }

        if results.is_empty() {
            if got_clean_page {
                // A normal page with no results — that's genuinely empty.
                return Ok(ToolOutcome::text(
                    ctx.loc.t("tool.web_search.result.no_results"),
                ));
            }
            // No provider returned a normal page: throttling and/or
            // network errors. Return an error (not "no results"), so the
            // model retries the request later instead of reporting it found nothing.
            if let Some(err) = last_err {
                return Err(
                    err.context(ctx.loc.t("tool.web_search.err.all_unavailable").to_string())
                );
            }
            anyhow::bail!(ctx.loc.t("tool.web_search.err.throttled"));
        }
        // Content extraction + reranking (unless disabled by the argument).
        // Fetching pages and embedding are "best effort": on failure the
        // regular result set remains (titles/snippets, provider order).
        if fetch_content {
            self.enrich_with_content(&mut results).await;
            rerank_by_embeddings(ctx.embedder.as_ref(), query, &mut results).await;
        }

        let mut out = format!(
            "{}\n",
            ctx.loc.tf(
                "tool.web_search.result.header",
                &[("n", &results.len().to_string())]
            )
        );
        for (i, r) in results.iter().enumerate() {
            out.push_str(&format!("{}. {} — {}\n", i + 1, r.title, r.url));
            if !r.snippet.is_empty() {
                out.push_str(&format!("   {}\n", r.snippet));
            }
            if !r.content.is_empty() {
                out.push_str("   ");
                out.push_str(ctx.loc.t("tool.web_search.result.content_label"));
                out.push('\n');
                out.push_str(&r.content);
                out.push('\n');
            }
        }
        Ok(ToolOutcome::text(out.trim_end().to_string()))
    }
}

/// A sign of anti-bot throttling/blocking by the provider: `HTTP 202` (a DDG
/// challenge page), `403`/`429` (Mojeek/others under IP overload), or an
/// `anomaly` marker in DDG's body. A normal result set doesn't contain `anomaly`. Such responses
/// are short-lived — this is neither "no results" nor a fatal error.
fn is_throttled(status: StatusCode, body: &str) -> bool {
    matches!(
        status,
        StatusCode::ACCEPTED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
    ) || body.contains("anomaly")
}

/// Phrases an anti-bot interstitial uses. Deliberately whole phrases, not the
/// word "captcha": this is checked **only on a page that parsed to zero results**
/// (see the provider loop), so a genuine result set is never at risk — but the
/// snippets of a *fruitless* search for anti-bot topics could still be, and a
/// phrase is far less likely to appear there than a single word.
const CHALLENGE_MARKERS: &[&str] = &[
    "verification required",
    "complete the challenge",
    "unusual traffic",
    "are you a robot",
    "enable javascript and cookies",
];

/// `true` if the body looks like an anti-bot interstitial rather than a results
/// page. Mojeek serves its captcha with **HTTP 200** and no `anomaly` marker
/// (measured live), so [`is_throttled`] cannot see it, and without this the
/// blocked provider counted as "answered, found nothing" — which suppressed the
/// honest "all providers are throttled" error and told the model the web had
/// nothing to say. See docs/history/fetch-url-fidelity.md §2 (P4).
fn is_challenge_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    CHALLENGE_MARKERS.iter().any(|m| lower.contains(m))
}

/// Parses a provider's result set: lists of links (`link_q` → `href`), titles
/// (`title_q` → text), and snippets (`snippet_q` → text) are aligned by index.
/// For DDG/Mojeek `link_q == title_q` (one `<a>` tag); for Ecosia — different tags.
/// The real URL is extracted from the `uddg=...` redirect (DDG) or taken as-is.
fn parse_results(
    html: &str,
    link_q: &str,
    title_q: &str,
    snippet_q: &str,
    max: usize,
) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let link_sel = Selector::parse(link_q).unwrap();
    let title_sel = Selector::parse(title_q).unwrap();
    let snippet_sel = Selector::parse(snippet_q).unwrap();

    let urls: Vec<String> = doc
        .select(&link_sel)
        .map(|el| extract_real_url(el.value().attr("href").unwrap_or_default()))
        .collect();
    let titles: Vec<String> = doc
        .select(&title_sel)
        .map(|el| collapse_ws(&el.text().collect::<String>()))
        .collect();
    let snippets: Vec<String> = doc
        .select(&snippet_sel)
        .map(|el| collapse_ws(&el.text().collect::<String>()))
        .collect();

    let mut results = Vec::new();
    for i in 0..urls.len().min(titles.len()) {
        if results.len() >= max {
            break;
        }
        let url = urls[i].clone();
        let title = titles[i].clone();
        if title.is_empty() || url.is_empty() {
            continue;
        }
        results.push(SearchResult {
            title,
            url,
            snippet: snippets.get(i).cloned().unwrap_or_default(),
            content: String::new(),
        });
    }
    results
}

/// Extracts the real URL from a DDG link: decodes the `uddg` parameter, or
/// normalizes a protocol-relative `//host/...`.
fn extract_real_url(href: &str) -> String {
    if let Some(pos) = href.find("uddg=") {
        let rest = &href[pos + 5..];
        let encoded = rest.split('&').next().unwrap_or(rest);
        return percent_decode(encoded);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

/// Minimal percent-decoding of a query-parameter value (`%XX`, `+`→space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Collapses spaces/line breaks into a single space and trims the edges.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_description_is_localized() {
        // The web_search description differs between ru/en (catches a forgotten
        // `_loc`), en has no Cyrillic. §3.5 docs/history/i18n.md.
        use crate::shared::i18n::{Lang, locale};
        let tool = WebSearch::new(true);
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        assert_ne!(tool.description(ru), tool.description(en));
        let e = tool.description(en);
        assert!(
            !e.chars()
                .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)),
            "Cyrillic in the en description: {e}"
        );
    }

    const FIXTURE: &str = r#"
        <html><body><table>
        <tr><td>1.&nbsp;</td><td>
            <a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=x" class="result-link">Пример A</a>
        </td></tr>
        <tr><td class="result-snippet">Сниппет про   A</td></tr>
        <tr><td>2.&nbsp;</td><td>
            <a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fb" class="result-link">Пример B</a>
        </td></tr>
        <tr><td class="result-snippet">Сниппет B</td></tr>
        </table></body></html>
    "#;

    /// A fixture of DDG's html endpoint (`result__a` / `result__snippet` markup).
    const FIXTURE_HTML: &str = r#"
        <html><body>
        <div class="result">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.net%2Fx&amp;rut=y">Пример X</a>
            <a class="result__snippet" href="/snip">Сниппет   X</a>
        </div>
        </body></html>
    "#;

    /// A Mojeek fixture (direct links `a.title`, snippet `p.s`).
    const FIXTURE_MOJEEK: &str = r#"
        <html><body><ul class="results-standard">
        <li><h2><a class="title" title="https://example.io/m" href="https://example.io/m">Пример M</a></h2>
        <p class="s">Сниппет   M</p></li>
        </ul></body></html>
    "#;

    /// An Ecosia fixture: the title and the link are DIFFERENT tags (by `data-test-id`).
    const FIXTURE_ECOSIA: &str = r#"
        <html><body>
        <div class="result">
            <a data-test-id="result-link" href="https://example.dev/e" tabindex="-1">https://example.dev/e</a>
            <div data-test-id="result-title">Пример   E</div>
            <p data-test-id="web-result-description">Сниппет E</p>
        </div>
        </body></html>
    "#;

    /// DDG-lite selectors (link == title, as in [`PROVIDERS`]).
    const LITE: (&str, &str) = ("a.result-link", "td.result-snippet");

    #[test]
    fn parses_results_and_decodes_urls() {
        let results = parse_results(FIXTURE, LITE.0, LITE.0, LITE.1, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Пример A");
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].snippet, "Сниппет про A"); // whitespace collapsed
        assert_eq!(results[1].url, "https://example.org/b");
    }

    #[test]
    fn parses_html_endpoint_layout() {
        // The fallback markup of DDG's html endpoint is recognized too.
        let results = parse_results(
            FIXTURE_HTML,
            "a.result__a",
            "a.result__a",
            "a.result__snippet",
            5,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Пример X");
        assert_eq!(results[0].url, "https://example.net/x");
        assert_eq!(results[0].snippet, "Сниппет X");
    }

    #[test]
    fn parses_mojeek_layout() {
        // The fallback provider Mojeek (direct links, different markup).
        let results = parse_results(FIXTURE_MOJEEK, "a.title", "a.title", "p.s", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Пример M");
        assert_eq!(results[0].url, "https://example.io/m");
        assert_eq!(results[0].snippet, "Сниппет M");
    }

    #[test]
    fn parses_ecosia_layout_separate_title_and_link() {
        // For Ecosia the title and the link are different tags; the parser aligns by index.
        let eco = PROVIDERS.iter().find(|p| p.name == "Ecosia").unwrap();
        let results = parse_results(
            FIXTURE_ECOSIA,
            eco.link_sel,
            eco.title_sel,
            eco.snippet_sel,
            5,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Пример E");
        assert_eq!(results[0].url, "https://example.dev/e");
        assert_eq!(results[0].snippet, "Сниппет E");
    }

    #[test]
    fn provider_selectors_match_fixtures() {
        // Selectors from PROVIDERS match what the fixtures are parsed with.
        let lite = &PROVIDERS[0];
        assert_eq!((lite.link_sel, lite.snippet_sel), LITE);
        assert!(
            !parse_results(FIXTURE, lite.link_sel, lite.title_sel, lite.snippet_sel, 5).is_empty()
        );
        let mojeek = PROVIDERS.iter().find(|p| p.name == "Mojeek").unwrap();
        assert!(
            !parse_results(
                FIXTURE_MOJEEK,
                mojeek.link_sel,
                mojeek.title_sel,
                mojeek.snippet_sel,
                5
            )
            .is_empty()
        );
    }

    #[test]
    fn respects_max_results() {
        assert_eq!(parse_results(FIXTURE, LITE.0, LITE.0, LITE.1, 1).len(), 1);
    }

    #[test]
    fn detects_throttling() {
        // HTTP 202 — DDG anti-bot throttling (a challenge page), even with no marker.
        assert!(is_throttled(
            StatusCode::ACCEPTED,
            "<html>что угодно</html>"
        ));
        // 403/429 — throttling/blocking by IP (Mojeek and others).
        assert!(is_throttled(StatusCode::FORBIDDEN, ""));
        assert!(is_throttled(StatusCode::TOO_MANY_REQUESTS, ""));
        // An anomaly marker in the body — also throttling.
        assert!(is_throttled(
            StatusCode::OK,
            "...If this error persists... anomaly ..."
        ));
        // A normal result set (200, no marker) — not throttling.
        assert!(!is_throttled(StatusCode::OK, FIXTURE));
    }

    /// Mojeek serves its captcha with **HTTP 200** and no `anomaly` marker
    /// (measured live), so `is_throttled` cannot see it — and without this the
    /// blocked provider counted as "answered, found nothing", which suppressed
    /// the honest "all providers are throttled" error. The body below is the
    /// text of the real interstitial. See docs/history/fetch-url-fidelity.md P4.
    #[test]
    fn a_captcha_behind_a_200_is_recognized_as_a_challenge() {
        let mojeek = "<html><body>Captcha Search Web Images News Verification required \
             Please complete the challenge to continue. Waiting for verification.</body></html>";
        assert!(is_challenge_page(mojeek));
        assert!(is_challenge_page(
            "<p>We detected UNUSUAL TRAFFIC from your network</p>"
        ));
        // A real result set is never a challenge — the check runs only on an
        // empty parse, but it must not be trigger-happy even so.
        assert!(!is_challenge_page(FIXTURE));
        assert!(!is_challenge_page(FIXTURE_MOJEEK));
    }

    #[test]
    fn percent_decode_handles_encoded_url() {
        assert_eq!(
            percent_decode("https%3A%2F%2Fa.b%2Fc%20d"),
            "https://a.b/c d"
        );
    }

    #[test]
    fn extract_real_url_protocol_relative() {
        assert_eq!(extract_real_url("//host.tld/path"), "https://host.tld/path");
    }

    const PAGE_HTML: &str = r#"
        <html><head><style>.x{color:red}</style><script>var a=1;</script></head>
        <body>
        <nav><a href="/">Главная</a> <a href="/about">О нас</a></nav>
        <header>Шапка сайта</header>
        <main>
            <h1>Заголовок</h1>
            <p>Это первый содержательный абзац статьи, достаточно длинный, чтобы пройти фильтр минимальной длины.</p>
            <p>Короткий.</p>
            <p>Второй содержательный абзац статьи с дополнительными подробностями по теме запроса пользователя.</p>
        </main>
        <footer>Подвал сайта со ссылками</footer>
        </body></html>
    "#;

    #[test]
    fn extract_readable_picks_main_paragraphs() {
        let text = extract_readable(PAGE_HTML, 1000);
        assert!(text.contains("первый содержательный абзац"));
        assert!(text.contains("Второй содержательный абзац"));
        // Scripts/styles and short fragments (nav/the one-word paragraph) are dropped.
        assert!(!text.contains("var a"));
        assert!(!text.contains("color:red"));
        assert!(!text.contains("Короткий."));
    }

    #[test]
    fn extract_readable_skips_boilerplate() {
        // A long paragraph inside <nav> (no <main>) is a menu, not content: dropped,
        // while a paragraph outside navigation is taken.
        let html = r#"<html><body>
            <nav><p>Перейти к разделам сайта, услуги, цены, контакты, поддержка и помощь.</p></nav>
            <div><p>Это настоящий содержательный абзац статьи достаточной длины.</p></div>
            <footer><p>Все права защищены, политика конфиденциальности, условия использования сервиса.</p></footer>
        </body></html>"#;
        let text = extract_readable(html, 1000);
        assert!(text.contains("настоящий содержательный абзац"));
        assert!(!text.contains("Перейти к разделам"));
        assert!(!text.contains("Все права защищены"));
    }

    #[test]
    fn extract_readable_falls_back_without_main() {
        let html = r#"<html><body>
            <p>Абзац без обёртки main, но содержательный и достаточно длинный для фильтра.</p>
        </body></html>"#;
        let text = extract_readable(html, 1000);
        assert!(text.contains("Абзац без обёртки main"));
    }

    #[test]
    fn extract_readable_truncates_to_max() {
        let text = extract_readable(PAGE_HTML, 20);
        assert!(text.chars().count() <= 20);
    }

    /// The shape of the page from the transcript that prompted this work
    /// (docs.vlang.io): **no `<pre>` at all** — VitePress wraps code in a bare
    /// `div.language-v` — and section titles in `<h2>`. Prose extraction drops
    /// both, so every "here is an example:" led nowhere and the model went
    /// hunting for the source elsewhere.
    const DOC_HTML: &str = r#"
        <html><head><title>Memory management</title></head><body>
        <nav><a href="/">Home</a></nav>
        <main>
            <h2 id="control">Control <a class="header-anchor">#</a></h2>
            <p>You can take advantage of V's autofree engine and define a free() method
               on custom data types, which is what the example below shows:</p>
            <div class="language-v">struct MyType {}

@[unsafe]
fn (data &amp;MyType) free() {
    // ...
}
</div>
            <p>Just as the compiler frees C data types with C's free(), it will statically
               insert free() calls for your data type at the end of each lifetime.</p>
        </main>
        </body></html>
    "#;

    #[test]
    fn rich_extraction_keeps_headings_and_code() {
        let text = extract_rich(DOC_HTML, 10_000);
        assert!(text.contains("## Control"), "heading missing: {text}");
        assert!(
            text.contains("```v"),
            "code fence with language missing: {text}"
        );
        assert!(
            text.contains("fn (data &MyType) free()"),
            "the code example itself is missing: {text}"
        );
        // Whitespace is the code's meaning — it must survive `collapse_ws`.
        assert!(
            text.contains("struct MyType {}\n"),
            "code was collapsed: {text}"
        );
        assert!(text.contains("autofree engine"), "prose missing: {text}");
        assert!(!text.contains("Home"), "nav leaked in: {text}");

        // Fork F1a: the prose path is what `web_search` ranks on and stays
        // exactly as it was — neither the heading nor the code appears there.
        let prose = extract_readable(DOC_HTML, 10_000);
        assert!(!prose.contains("Control"), "prose path changed: {prose}");
        assert!(
            !prose.contains("struct MyType"),
            "prose path changed: {prose}"
        );
    }

    #[test]
    fn a_wrapped_code_block_is_not_emitted_twice() {
        // The highlighter output `div.language-* > pre > code` matches two of
        // our selectors; ancestry dedup keeps the outer one only.
        let html = r#"<html><body><main>
            <div class="language-rust"><pre><code class="language-rust">let x = 1;</code></pre></div>
        </main></body></html>"#;
        let text = extract_rich(html, 10_000);
        assert_eq!(text.matches("let x = 1;").count(), 1, "duplicated: {text}");
        assert_eq!(text.matches("```").count(), 2, "not one fence: {text}");
    }

    #[test]
    fn code_language_comes_from_the_inner_code_tag() {
        // Prism/Rouge put the class on `<code>`, not on `<pre>`.
        let html = r#"<html><body><main>
            <pre><code class="language-python">print(1)</code></pre>
        </main></body></html>"#;
        assert!(extract_rich(html, 10_000).contains("```python"));
    }

    #[test]
    fn short_code_and_headings_survive_the_fragment_floor() {
        // MIN_FRAGMENT_CHARS exists to drop navigation chrome from search
        // snippets; a two-word heading and a one-line example are content.
        let html = r#"<html><body><main>
            <h3>Control</h3><pre>v -autofree</pre>
        </main></body></html>"#;
        let text = extract_rich(html, 10_000);
        assert!(text.contains("### Control"), "got: {text}");
        assert!(text.contains("v -autofree"), "got: {text}");
    }

    #[test]
    fn rich_extraction_skips_boilerplate_and_truncates() {
        let html = r#"<html><body>
            <nav><pre>menu code that is not content</pre></nav>
            <p>Настоящий содержательный абзац страницы, достаточно длинный для порога.</p>
        </body></html>"#;
        let text = extract_rich(html, 10_000);
        assert!(!text.contains("menu code"), "nav leaked in: {text}");
        assert!(text.contains("содержательный абзац"));
        assert!(extract_rich(DOC_HTML, 20).chars().count() <= 20);
    }

    #[test]
    fn cosine_basic() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        // A length mismatch / a zero vector → 0.0.
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn rerank_order_sorts_by_similarity() {
        let query = vec![1.0, 0.0];
        let docs = vec![
            vec![0.0, 1.0], // orthogonal — least similar
            vec![1.0, 0.0], // identical — most similar
            vec![0.7, 0.7], // in between
        ];
        let order = rerank_order(&query, &docs);
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn rerank_order_is_stable_on_ties() {
        // With equal similarity the original order is preserved (a stable sort).
        let query = vec![1.0, 0.0];
        let docs = vec![vec![1.0, 0.0], vec![1.0, 0.0], vec![1.0, 0.0]];
        assert_eq!(rerank_order(&query, &docs), vec![0, 1, 2]);
    }

    #[test]
    fn rerank_text_prefers_content_over_snippet() {
        let r = SearchResult {
            title: "Заголовок".into(),
            url: "https://e/x".into(),
            snippet: "сниппет".into(),
            content: "извлечённый контент".into(),
        };
        let t = rerank_text(&r);
        assert!(t.contains("Заголовок"));
        assert!(t.contains("извлечённый контент"));
        assert!(!t.contains("сниппет"));
        // With no content — the snippet is taken.
        let r2 = SearchResult {
            content: String::new(),
            ..r
        };
        assert!(rerank_text(&r2).contains("сниппет"));
    }

    #[tokio::test]
    async fn rerank_reorders_results_by_query() {
        use crate::shared::api::mock::MockEmbedder;
        let embedder = MockEmbedder::new(64);
        let mut results = vec![
            SearchResult {
                title: "Про погоду".into(),
                url: "https://e/weather".into(),
                snippet: String::new(),
                content: "сегодня дождь и ветер, прогноз погоды на завтра".into(),
            },
            SearchResult {
                title: "Язык Rust".into(),
                url: "https://e/rust".into(),
                snippet: String::new(),
                content: "rust системный язык программирования с безопасной памятью".into(),
            },
        ];
        rerank_by_embeddings(&embedder, "rust язык программирования", &mut results).await;
        // The result relevant to the query rose to the top.
        assert_eq!(results[0].url, "https://e/rust");
    }

    #[tokio::test]
    async fn rerank_splits_the_query_from_the_page_texts() {
        // The only site in the codebase where one comparison needs both roles, so
        // it is the only one that issues two requests. Pinned here because a
        // regression would silently embed the query as a passage — invisible on
        // bge-m3, quietly wrong on any model that uses input prefixes
        // (docs/research/embedding-input-prefixes.md §5.1/§5.2).
        use crate::features::tools::testkit::RoleRecorder;
        let rec = RoleRecorder::new();
        let mut results = vec![
            SearchResult {
                title: "Про погоду".into(),
                url: "https://e/weather".into(),
                snippet: "дождь".into(),
                content: String::new(),
            },
            SearchResult {
                title: "Язык Rust".into(),
                url: "https://e/rust".into(),
                snippet: "системный язык".into(),
                content: String::new(),
            },
        ];
        rerank_by_embeddings(&rec, "что такое rust?", &mut results).await;

        assert_eq!(rec.roles(), vec![EmbedRole::Query, EmbedRole::Passage]);
        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls[0].0, vec!["что такое rust?".to_string()]);
        assert_eq!(calls[1].0.len(), 2, "one text per result, query excluded");
    }

    #[tokio::test]
    async fn rerank_noop_when_embedder_unavailable() {
        use crate::shared::api::UnavailableEmbedder;
        let mut results = vec![
            SearchResult {
                title: "A".into(),
                url: "https://e/a".into(),
                snippet: "s".into(),
                content: "контент a".into(),
            },
            SearchResult {
                title: "B".into(),
                url: "https://e/b".into(),
                snippet: "s".into(),
                content: "контент b".into(),
            },
        ];
        rerank_by_embeddings(&UnavailableEmbedder, "запрос", &mut results).await;
        // The embedder is unavailable → the order didn't change.
        assert_eq!(results[0].url, "https://e/a");
        assert_eq!(results[1].url, "https://e/b");
    }

    /// A real network smoke (manual: `cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "requires network access to search providers"]
    async fn live_search_returns_results() {
        let tool = WebSearch::new(true);
        let (_dir, _storage, ctx) = super::super::testkit::ctx_with_backends(
            uuid::Uuid::new_v4(),
            std::sync::Arc::new(crate::shared::api::mock::MockBackend::scripted(vec![])),
            std::sync::Arc::new(crate::shared::api::mock::MockEmbedder::new(16)),
        );
        let out = match tool
            .invoke(
                &ctx,
                serde_json::json!({"query": "rust language", "max_results": 3}),
            )
            .await
        {
            Ok(out) => out,
            Err(e) => {
                // Every provider throttling at once is an infrastructure
                // condition, not a regression -- and a datacenter IP (a CI
                // runner) is throttled far harder than a home one, which is how
                // this smoke first went red remotely. The tool already draws
                // that distinction, so skip on it and fail on anything else.
                // Matched by the bundle key rather than by prose, so it holds
                // whatever locale the profile is in.
                if e.to_string()
                    .contains(ctx.loc.t("tool.web_search.err.throttled"))
                {
                    eprintln!("skip: every search provider is throttling this IP");
                    return;
                }
                panic!("web search failed: {e:#}");
            }
        };
        assert!(out.result.contains("http"), "got: {}", out.result);
    }
}
