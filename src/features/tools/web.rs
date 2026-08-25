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
//! "provider unavailable" error. Now throttling is detected ([`is_throttled`], and
//! behind a `200` by [`is_challenge_page`]) and on it the next provider is tried right
//! away (throttling is sticky per-IP — retries only deepen it; different providers
//! throttle independently, so almost always someone answers). If **all** are
//! unavailable/throttled — an explicit error is returned (not "no results"), so the
//! model retries the request later instead of reporting that it found nothing.
//!
//! A block is also **remembered** ([`WebSearch::mark_blocked`]) and moves that
//! provider *family* to the back of the order for [`PROVIDER_COOLDOWN`]. Without
//! it every call restarted at DuckDuckGo and paid two dead round trips before
//! reaching a provider that could answer — measured across an agentic turn,
//! where a model issues several searches in a row
//! (docs/research/web-search-keyed-providers.md §1.1). A cooling provider is
//! reordered, never skipped.
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
/// The DuckDuckGo family name — two [`PROVIDERS`] entries share it (see
/// [`Provider::family`]).
const DDG: &str = "ddg";
/// How long a family that answered with a block is moved to the back of the
/// order (see [`WebSearch::provider_order`]).
///
/// A judgement call, not a measurement: what *was* measured (2026-08-25, from a
/// residential IP, docs/research/web-search-keyed-providers.md §1.1) is that a
/// block outlives fifteen minutes of complete silence, so no honest value here
/// "waits out" anything. Five minutes is chosen to span a whole agentic turn —
/// which is what the reordering is for — while being short enough that a family
/// which did recover is not stranded for the session. Nothing is ever *skipped*
/// on account of a cooldown, so an over-long value cannot make the tool blind.
const PROVIDER_COOLDOWN: Duration = Duration::from_secs(300);

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
    /// Which infrastructure this entry sits on. Throttling is **per-IP and
    /// per-operator**, not per-URL: `lite.duckduckgo.com` and
    /// `html.duckduckgo.com` are two markup variants of one search engine and
    /// share a single throttle (measured — with lite already blocked, html
    /// answered `202` on its very first request). So the cooldown is keyed by
    /// family, and a chain of four entries is a chain of **three** independent
    /// providers.
    family: &'static str,
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
        family: DDG,
        url: "https://lite.duckduckgo.com/lite/",
        method: Method::PostForm,
        link_sel: "a.result-link",
        title_sel: "a.result-link",
        snippet_sel: "td.result-snippet",
    },
    Provider {
        name: "DuckDuckGo html",
        family: DDG,
        url: "https://html.duckduckgo.com/html/",
        method: Method::PostForm,
        link_sel: "a.result__a",
        title_sel: "a.result__a",
        snippet_sel: "a.result__snippet",
    },
    Provider {
        name: "Mojeek",
        family: "mojeek",
        url: "https://www.mojeek.com/search",
        method: Method::GetQuery,
        link_sel: "a.title",
        title_sel: "a.title",
        snippet_sel: "p.s",
    },
    Provider {
        name: "Ecosia",
        family: "ecosia",
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

/// One keyed provider the tool may use, with its credential already resolved
/// (spec §9.3.1). Constructed by [`keyed_backends`]; the tool never reads a
/// secret store or the environment itself.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiBackend {
    slot: crate::shared::secrets::SearchSlot,
    key: String,
}

impl ApiBackend {
    /// The name for logs, errors and the "which backend answered" line.
    fn name(&self) -> &'static str {
        use crate::shared::secrets::SearchSlot::*;
        match self.slot {
            Tavily => "Tavily",
            Brave => "Brave",
        }
    }

    /// The cooldown key. A keyed provider is its own family: its rate limit is
    /// per-account, unrelated to any other's.
    fn family(&self) -> &'static str {
        use crate::shared::secrets::SearchSlot::*;
        match self.slot {
            Tavily => "tavily",
            Brave => "brave",
        }
    }
}

/// Which keyed backends a configuration yields, in the order to try them.
///
/// `keys` arrives in preference order with the stored-beats-environment rule
/// already applied. [`WebProvider::FreeOnly`] yields none — someone who keeps a
/// key for another purpose can stop `web_search` spending it — and a named
/// provider yields only itself, so choosing Tavily does not quietly fall back
/// to a Brave key that happens to be present.
///
/// [`WebProvider::FreeOnly`]: crate::shared::config::WebProvider::FreeOnly
pub fn keyed_backends(
    provider: crate::shared::config::WebProvider,
    keys: &[(crate::shared::secrets::SearchSlot, String)],
) -> Vec<ApiBackend> {
    use crate::shared::config::WebProvider as P;
    use crate::shared::secrets::SearchSlot as S;
    let wanted = |slot: S| match provider {
        P::Auto => true,
        P::Tavily => slot == S::Tavily,
        P::Brave => slot == S::Brave,
        P::FreeOnly => false,
    };
    keys.iter()
        .filter(|(slot, key)| wanted(*slot) && !key.trim().is_empty())
        .map(|(slot, key)| ApiBackend {
            slot: *slot,
            key: key.clone(),
        })
        .collect()
}

/// One entry in the order [`WebSearch`] tries: a keyed API, or one of the
/// keyless scraped [`PROVIDERS`].
#[derive(Clone, Copy)]
enum Backend<'a> {
    Api(&'a ApiBackend),
    Scraped(&'static Provider),
}

impl Backend<'_> {
    fn name(&self) -> &'static str {
        match self {
            Self::Api(a) => a.name(),
            Self::Scraped(p) => p.name,
        }
    }
    fn family(&self) -> &'static str {
        match self {
            Self::Api(a) => a.family(),
            Self::Scraped(p) => p.family,
        }
    }
}

/// What one backend's attempt came to. Kept separate from `Result` because
/// "blocked" is neither success nor a failure to report: it is the one outcome
/// that must never reach the model as "the web has nothing".
enum Attempt {
    /// Results, always non-empty.
    Results(Vec<SearchResult>),
    /// A genuine, believable empty result set.
    Empty,
    /// Anti-bot throttling, a challenge page, or a vendor rate limit.
    Blocked,
}

/// `web_search` — internet search: the keyed providers a key is configured for
/// (see [`keyed_backends`]), then the keyless chain (DuckDuckGo → Mojeek →
/// Ecosia, see [`PROVIDERS`]).
pub struct WebSearch {
    http: crate::shared::net::GuardedClient,
    /// The default value for the `fetch_content` argument (from `config.tools`).
    fetch_content_default: bool,
    /// Keyed backends, in preference order — empty when no key is configured,
    /// which is what makes that case byte-identical to the keyless tool.
    api: Vec<ApiBackend>,
    /// When each provider family was last seen blocking us, for
    /// [`WebSearch::provider_order`]. The tool outlives the turn — the registry
    /// is built once and rebuilt only on a settings/MCP change
    /// (`orchestrator::build_registry`) — so this memory spans the whole
    /// agentic loop, which is exactly the span that wastes round trips today.
    /// A plain `Mutex`: it guards a four-entry array, is never held across an
    /// `await`, and a poisoned lock is not worth failing a search over.
    cooldown: std::sync::Mutex<Vec<(&'static str, std::time::Instant)>>,
}

impl Default for WebSearch {
    fn default() -> Self {
        // The guarded policy is the default here too: a caller that does not pass one must
        // get the safe tool, not the permissive one.
        Self::new(
            true,
            crate::shared::net::AddressPolicy::PublicOnly,
            Vec::new(),
        )
    }
}

impl WebSearch {
    pub fn new(
        fetch_content_default: bool,
        policy: crate::shared::net::AddressPolicy,
        api: Vec<ApiBackend>,
    ) -> Self {
        // A request timeout: otherwise a hung DDG response would hold up the whole turn.
        // The address policy rides on the same client: the result pages this fetches are
        // chosen by whatever the search provider ranked, which is one step further from the
        // model than `fetch_url` and the same threat (fork F1,
        // docs/research/fetch-url-address-policy.md).
        let http = crate::shared::net::GuardedClient::new(policy, REQUEST_TIMEOUT);
        Self {
            http,
            fetch_content_default,
            api,
            cooldown: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Records that `family` just answered with a block, so the next search
    /// starts elsewhere. Keyed by family, not by provider: the two DuckDuckGo
    /// entries share one throttle (see [`Provider::family`]).
    fn mark_blocked(&self, family: &'static str) {
        let Ok(mut seen) = self.cooldown.lock() else {
            return; // a poisoned lock only costs us the reordering
        };
        let now = std::time::Instant::now();
        match seen.iter_mut().find(|(f, _)| *f == family) {
            Some((_, at)) => *at = now,
            None => seen.push((family, now)),
        }
    }

    /// When `family` was last seen blocking us, if that is still within
    /// [`PROVIDER_COOLDOWN`].
    fn blocked_at(&self, family: &str) -> Option<std::time::Instant> {
        let seen = self.cooldown.lock().ok()?;
        let now = std::time::Instant::now();
        seen.iter()
            .find(|(f, _)| *f == family)
            .map(|(_, at)| *at)
            .filter(|at| now.duration_since(*at) < PROVIDER_COOLDOWN)
    }

    /// The order to try backends in: **keyed providers first** (user's
    /// decision, 2026-08-25 — a dead round trip through a blocked scraper costs
    /// more seconds than a credit costs cents, and a block outlives fifteen
    /// minutes), then the keyless chain. Within that, everything not known to
    /// be blocking comes first in its declared order, then the cooling
    /// families, oldest block first.
    ///
    /// A cooling backend is **reordered, never skipped**. Skipping would let a
    /// stale cooldown return "everything is throttled" without a single request
    /// having gone out — the tool would be lying about the same thing it exists
    /// to report honestly, and a family that recovered early would never be
    /// found to have recovered.
    fn backend_order(&self) -> Vec<Backend<'_>> {
        let all = self
            .api
            .iter()
            .map(Backend::Api)
            .chain(PROVIDERS.iter().map(Backend::Scraped));
        let mut order: Vec<(Option<std::time::Instant>, usize, Backend<'_>)> = all
            .enumerate()
            .map(|(i, b)| (self.blocked_at(b.family()), i, b))
            .collect();
        // `None` sorts before `Some`, and among the cooling ones the oldest
        // block (the smallest `Instant`) first; ties keep the declared order —
        // which is what keeps every keyed backend ahead of every scraped one.
        order.sort_by_key(|(at, i, _)| (*at, *i));
        order.into_iter().map(|(_, _, b)| b).collect()
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
            Method::PostForm => self
                .http
                .post(provider.url)
                .with_context(|| {
                    loc.tf("tool.web_search.err.url_parse", &[("name", provider.name)])
                })?
                .form(&[("q", query)]),
            Method::GetQuery => {
                let mut url = reqwest::Url::parse(provider.url).with_context(|| {
                    loc.tf("tool.web_search.err.url_parse", &[("name", provider.name)])
                })?;
                url.query_pairs_mut().append_pair("q", query);
                self.http.get(url.as_str()).with_context(|| {
                    loc.tf("tool.web_search.err.url_parse", &[("name", provider.name)])
                })?
            }
        };
        let resp = req
            // The same browser-like header set `fetch_content` already sends. A
            // Chrome `User-Agent` with no `Accept`/`Accept-Language` beside it is
            // a bot signature in its own right, and this request had exactly
            // that shape. Hygiene, not a fix: measured, once an IP is blocked a
            // real Chrome from that IP is blocked too, so headers can only
            // affect *whether* the block is provoked, never recovery from it
            // (docs/research/web-search-keyed-providers.md §5, F-3).
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, ACCEPT_HTML)
            .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
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
        let request = match self.http.get(url) {
            Ok(r) => r,
            Err(err) => {
                tracing::debug!(url, %err, "web search: the result page's address is refused");
                return None;
            }
        };
        let resp = match request
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
    /// A result that **already carries content** is left alone: a keyed provider
    /// can return the page text itself (Tavily's `include_raw_content`), and
    /// re-fetching it would spend the latency this backend was chosen to avoid
    /// and put one more automated request in front of the site's own anti-bot.
    async fn enrich_with_content(&self, results: &mut [SearchResult]) {
        let pending = needs_content(results);
        let contents = futures_util::future::join_all(
            pending.iter().map(|&i| self.fetch_content(&results[i].url)),
        )
        .await;
        for (i, c) in pending.into_iter().zip(contents) {
            if let Some(c) = c {
                results[i].content = c;
            }
        }
    }

    /// Goes through providers in order: the first one to return a non-empty
    /// result set wins. On throttling, moves to the next one right away
    /// (retrying a sticky per-IP throttle is pointless). Returns the results,
    /// `got_clean_page` — at least one backend returned a normal (non-challenge)
    /// answer: then emptiness is a genuine "no results", not throttling — the
    /// last backend error, if any, and the name of the backend that answered.
    async fn run_providers(
        &self,
        query: &str,
        max: usize,
        want_content: bool,
        loc: &crate::shared::i18n::Locale,
    ) -> (
        Vec<SearchResult>,
        bool,
        Option<anyhow::Error>,
        Option<&'static str>,
    ) {
        let mut got_clean_page = false;
        let mut last_err: Option<anyhow::Error> = None;
        let mut results = Vec::new();
        let mut answered_by = None;
        for backend in self.backend_order() {
            match self
                .run_backend(backend, query, max, want_content, loc)
                .await
            {
                Ok(Attempt::Results(r)) => {
                    results = r;
                    answered_by = Some(backend.name());
                    break;
                }
                Ok(Attempt::Empty) => got_clean_page = true,
                Ok(Attempt::Blocked) => {
                    tracing::debug!(
                        provider = backend.name(),
                        "web search: blocked, trying the next backend"
                    );
                    self.mark_blocked(backend.family());
                }
                Err(err) => {
                    tracing::warn!(provider = backend.name(), error = %err, "web search: backend error");
                    last_err = Some(err);
                }
            }
        }
        (results, got_clean_page, last_err, answered_by)
    }

    /// One backend's attempt.
    async fn run_backend(
        &self,
        backend: Backend<'_>,
        query: &str,
        max: usize,
        want_content: bool,
        loc: &crate::shared::i18n::Locale,
    ) -> Result<Attempt> {
        match backend {
            Backend::Scraped(p) => self.run_scraped(p, query, max, loc).await,
            Backend::Api(a) => self.run_api(a, query, max, want_content, loc).await,
        }
    }

    /// One keyless provider: fetch the results page and parse it.
    async fn run_scraped(
        &self,
        provider: &'static Provider,
        query: &str,
        max: usize,
        loc: &crate::shared::i18n::Locale,
    ) -> Result<Attempt> {
        let Some(html) = self.fetch(provider, query, loc).await? else {
            return Ok(Attempt::Blocked);
        };
        let r = parse_results(
            &html,
            provider.link_sel,
            provider.title_sel,
            provider.snippet_sel,
            max,
        );
        if !r.is_empty() {
            return Ok(Attempt::Results(r));
        }
        // Empty parse: either the query genuinely has no matches, or this is an
        // anti-bot challenge served with HTTP 200 (Mojeek does exactly that —
        // measured). The check runs only here, on an empty parse, so a results
        // page can never be mistaken for a challenge (a search for "captcha"
        // keeps working).
        Ok(if is_challenge_page(&html, query) {
            Attempt::Blocked
        } else {
            Attempt::Empty
        })
    }

    /// One keyed provider. The endpoints answer JSON, so there is no markup to
    /// parse and no challenge to recognise — the failure modes are HTTP status
    /// codes, and they mean different things:
    ///
    /// - **429** is a rate limit: the same shape as an anti-bot throttle, so it
    ///   takes the same cooldown and falls through.
    /// - **401/403/402** mean the key is wrong, revoked or out of credit. That
    ///   is a *user* problem and must not be reported as throttling — but it
    ///   must not fail the search either, or a stale key would take web search
    ///   down while a working keyless chain sits behind it. So it becomes
    ///   `last_err`: the run continues, and if nothing else answers, the message
    ///   the model gets names the key rather than blaming the network.
    async fn run_api(
        &self,
        api: &ApiBackend,
        query: &str,
        max: usize,
        want_content: bool,
        loc: &crate::shared::i18n::Locale,
    ) -> Result<Attempt> {
        use crate::shared::secrets::SearchSlot::*;
        let name = api.name();
        // A fixed vendor endpoint the code itself writes, not an address the
        // model or a page chose — the same category as the YouTube metadata
        // calls, so it does not go through the address guard (which would
        // resolve and re-check a hostname that is not in question).
        let http = self.http.unchecked_inner();
        let req = match api.slot {
            Tavily => http
                .post("https://api.tavily.com/search")
                .bearer_auth(&api.key)
                .json(&serde_json::json!({
                    "query": query,
                    "max_results": max,
                    // Ask for the page text only when the caller wants content:
                    // it is the expensive half of the response, and with
                    // `fetch_content=false` the tool would throw it away.
                    "include_raw_content": want_content,
                })),
            Brave => {
                let mut url = reqwest::Url::parse("https://api.search.brave.com/res/v1/web/search")
                    .with_context(|| loc.tf("tool.web_search.err.url_parse", &[("name", name)]))?;
                url.query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("count", &max.to_string());
                http.get(url)
                    .header("X-Subscription-Token", &api.key)
                    .header(reqwest::header::ACCEPT, "application/json")
            }
        };
        let resp = req
            .send()
            .await
            .with_context(|| loc.tf("tool.web_search.err.request", &[("name", name)]))?;
        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Ok(Attempt::Blocked);
        }
        if matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::PAYMENT_REQUIRED
        ) {
            anyhow::bail!(loc.tf("tool.web_search.err.key_rejected", &[("name", name)]));
        }
        if !status.is_success() {
            anyhow::bail!(loc.tf(
                "tool.web_search.err.status",
                &[("name", name), ("status", &status.to_string())]
            ));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .with_context(|| loc.tf("tool.web_search.err.read", &[("name", name)]))?;
        let results = match api.slot {
            Tavily => parse_tavily(&body, max),
            Brave => parse_brave(&body, max),
        };
        Ok(if results.is_empty() {
            // A keyed provider answering 200 with an empty list is believable
            // emptiness — there is no interstitial to mistake it for.
            Attempt::Empty
        } else {
            Attempt::Results(results)
        })
    }
}

/// Which results still need their page fetched — those whose `content` is
/// empty. Its own function so the decision can be asserted on directly: the
/// behaviour is a *negative* one (a populated result is never fetched), and a
/// test that only checks the field afterwards passes even when the skip is
/// removed, because a refused fetch leaves the field alone anyway.
fn needs_content(results: &[SearchResult]) -> Vec<usize> {
    results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.content.is_empty())
        .map(|(i, _)| i)
        .collect()
}

/// Tavily's `{"results": [{title, url, content, raw_content?}]}`.
/// `content` is the snippet; `raw_content` (present only when the request asked
/// for it) is the cleaned page text, which spares the tool its own fetch.
fn parse_tavily(body: &serde_json::Value, max: usize) -> Vec<SearchResult> {
    let Some(items) = body.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|it| {
            let url = it.get("url")?.as_str()?.trim();
            let title = it.get("title").and_then(|v| v.as_str()).unwrap_or_default();
            (!url.is_empty() && !title.is_empty()).then(|| SearchResult {
                title: collapse_ws(title),
                url: url.to_string(),
                snippet: collapse_ws(it.get("content").and_then(|v| v.as_str()).unwrap_or("")),
                content: it
                    .get("raw_content")
                    .and_then(|v| v.as_str())
                    .map(|c| truncate_chars(c.trim(), MAX_CONTENT_CHARS))
                    .unwrap_or_default(),
            })
        })
        .take(max)
        .collect()
}

/// Brave's `{"web": {"results": [{title, url, description}]}}`. Snippets only —
/// the tool fetches the pages itself, as it does for the keyless chain.
fn parse_brave(body: &serde_json::Value, max: usize) -> Vec<SearchResult> {
    let Some(items) = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|it| {
            let url = it.get("url")?.as_str()?.trim();
            let title = it.get("title").and_then(|v| v.as_str()).unwrap_or_default();
            (!url.is_empty() && !title.is_empty()).then(|| SearchResult {
                title: collapse_ws(title),
                url: url.to_string(),
                // Brave marks the query terms with `<strong>` — this is a
                // snippet for a model to read, not markup to render.
                snippet: collapse_ws(&strip_tags(
                    it.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                )),
                content: String::new(),
            })
        })
        .take(max)
        .collect()
}

/// Drops HTML tags from a snippet, keeping their text.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
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

    let mut blocks: Vec<Block> = doc
        .select(&scope_sel)
        .flat_map(|root| collect_blocks(root, &block_sel))
        .collect();
    if blocks.is_empty() {
        blocks = collect_blocks(doc.root_element(), &block_sel);
    }

    join_blocks(blocks, max_chars)
}

/// Collects rich-extraction blocks under `root`, in document order.
fn collect_blocks(root: scraper::ElementRef, block_sel: &Selector) -> Vec<Block> {
    // A container that already emitted its content must not emit it again
    // (`div.language-v` wrapping a `pre`). Selection is in document order, so
    // an ancestor is always seen first — hence "skip if an ancestor emitted".
    let mut emitted = std::collections::HashSet::new();
    let mut out = Vec::new();
    for el in root.select(block_sel) {
        if in_boilerplate(el) || el.ancestors().any(|a| emitted.contains(&a.id())) {
            continue;
        }
        if let Some(b) = block_from(el) {
            emitted.insert(el.id());
            out.push(b);
        }
    }
    out
}

/// Joins rich-extraction blocks into the final text, truncated to `max_chars`.
fn join_blocks(blocks: Vec<Block>, max_chars: usize) -> String {
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

        let (mut results, got_clean_page, last_err, answered_by) =
            self.run_providers(query, max, fetch_content, ctx.loc).await;

        if results.is_empty() {
            return no_results_outcome(got_clean_page, last_err, ctx.loc);
        }
        // Content extraction + reranking (unless disabled by the argument).
        // Fetching pages and embedding are "best effort": on failure the
        // regular result set remains (titles/snippets, provider order).
        if fetch_content {
            self.enrich_with_content(&mut results).await;
            rerank_by_embeddings(ctx.embedder.as_ref(), query, &mut results).await;
        }

        Ok(ToolOutcome::text(format_results(
            &results,
            answered_by,
            ctx.loc,
        )))
    }
}

/// The outcome when every provider came back empty: a genuine "no results" when
/// at least one normal page was seen, otherwise an explicit error.
fn no_results_outcome(
    got_clean_page: bool,
    last_err: Option<anyhow::Error>,
    // `&'static` (like `ToolContext.loc`) — `bail!` embeds the borrowed message.
    loc: &'static crate::shared::i18n::Locale,
) -> Result<ToolOutcome> {
    if got_clean_page {
        // A normal page with no results — that's genuinely empty.
        return Ok(ToolOutcome::text(
            loc.t("tool.web_search.result.no_results"),
        ));
    }
    // No provider returned a normal page: throttling and/or
    // network errors. Return an error (not "no results"), so the
    // model retries the request later instead of reporting it found nothing.
    if let Some(err) = last_err {
        return Err(err.context(loc.t("tool.web_search.err.all_unavailable").to_string()));
    }
    anyhow::bail!(loc.t("tool.web_search.err.throttled"));
}

/// Formats the result list into the tool's text result.
///
/// The header names **which backend answered**. The chain degrades silently by
/// design — a keyed provider, then three scrapers, then nothing — and without
/// this the model cannot tell a thin answer from a degraded one; the transcript
/// that started this work has it reasoning aloud about the tool's health with no
/// evidence to reason from.
fn format_results(
    results: &[SearchResult],
    answered_by: Option<&str>,
    loc: &crate::shared::i18n::Locale,
) -> String {
    let mut out = format!(
        "{}\n",
        match answered_by {
            Some(name) => loc.tf(
                "tool.web_search.result.header_via",
                &[("n", &results.len().to_string()), ("name", name)]
            ),
            None => loc.tf(
                "tool.web_search.result.header",
                &[("n", &results.len().to_string())]
            ),
        }
    );
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("{}. {} — {}\n", i + 1, r.title, r.url));
        if !r.snippet.is_empty() {
            out.push_str(&format!("   {}\n", r.snippet));
        }
        if !r.content.is_empty() {
            out.push_str("   ");
            out.push_str(loc.t("tool.web_search.result.content_label"));
            out.push('\n');
            out.push_str(&r.content);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
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

/// What an anti-bot interstitial calls itself in its `<title>`. Matched as a
/// **whole title or its leading phrase** (see [`is_challenge_page`]), never as a
/// substring of the body — which is what makes this list safe to keep short and
/// generic where the phrase list below could not be.
const CHALLENGE_TITLES: &[&str] = &[
    "captcha",
    "verification required",
    "attention required",
    "just a moment",
    "access denied",
    "robot check",
    "security check",
];

/// Phrases an anti-bot interstitial uses in its **body**. The second signal, for
/// pages whose title is generic. Deliberately whole phrases, not the word
/// "captcha": this is checked **only on a page that parsed to zero results**
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
/// page. Mojeek serves its captcha with **HTTP 200** and no `anomaly` marker, so
/// [`is_throttled`] cannot see it, and without this the blocked provider counts
/// as "answered, found nothing" — which suppresses the honest "all providers are
/// throttled" error and tells the model the web has nothing to say. That is the
/// worst thing this tool can say, and it has now been said twice: the first fix
/// (docs/history/fetch-url-fidelity.md §2, P4) matched *body phrases*, and
/// Mojeek's block page has since been rewritten to contain none of them —
/// measured 2026-08-25, `HTTP 200`, `<title>Captcha</title>`, zero of the five
/// phrases present (docs/research/web-search-keyed-providers.md §2).
///
/// So the primary anchor is now the **title element**, which names what the page
/// *is* and survives a rewording of its prose. Two guards keep this from firing
/// on a real result set, on top of the caller only asking on an empty parse:
///
/// - a title that **contains the query** is a results page, whatever else it
///   says — that is how a search engine titles a result set (`captcha - Mojeek
///   Search`), and it is precisely the fruitless-search-for-anti-bot-topics case
///   the phrase list was contorted to avoid;
/// - otherwise the title must **be** a marker or **begin** with one, so a marker
///   buried in a page's prose cannot reach the classifier at all.
fn is_challenge_page(body: &str, query: &str) -> bool {
    let title = page_title(body);
    let query = collapse_ws(&query.to_lowercase());
    if !title.is_empty() && !query.is_empty() && title.contains(&query) {
        return false; // a results page names what was searched for
    }
    // `starts_with` rather than equality: Cloudflare's is "Attention Required! |
    // Cloudflare", and the operator's name is appended to several of these.
    if CHALLENGE_TITLES.iter().any(|m| title.starts_with(m)) {
        return true;
    }
    let lower = body.to_ascii_lowercase();
    CHALLENGE_MARKERS.iter().any(|m| lower.contains(m))
}

/// The document's `<title>`, lowercased and whitespace-collapsed (empty when
/// there is none).
fn page_title(body: &str) -> String {
    let doc = Html::parse_document(body);
    let sel = Selector::parse("title").unwrap();
    doc.select(&sel)
        .next()
        .map(|el| collapse_ws(&el.text().collect::<String>()).to_lowercase())
        .unwrap_or_default()
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
    use crate::shared::config::WebProvider;
    use crate::shared::secrets::SearchSlot;

    #[test]
    fn web_search_description_is_localized() {
        // The web_search description differs between ru/en (catches a forgotten
        // `_loc`), en has no Cyrillic. §3.5 docs/history/i18n.md.
        use crate::shared::i18n::{Lang, locale};
        let tool = WebSearch::default();
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
        assert!(is_challenge_page(mojeek, "rust language"));
        assert!(is_challenge_page(
            "<p>We detected UNUSUAL TRAFFIC from your network</p>",
            "rust language"
        ));
        // A real result set is never a challenge — the check runs only on an
        // empty parse, but it must not be trigger-happy even so.
        assert!(!is_challenge_page(FIXTURE, "rust language"));
        assert!(!is_challenge_page(FIXTURE_MOJEEK, "rust language"));
    }

    /// The regression this branch exists for. Mojeek's block page was rewritten
    /// and now carries **none** of `CHALLENGE_MARKERS` — measured 2026-08-25,
    /// `HTTP 200`, `<title>Captcha</title>`, and the body says only "Please
    /// prove you are human". Under the old body-phrase check this parsed to zero
    /// results, was declared a clean page, and the model was told the web knew
    /// nothing. See docs/research/web-search-keyed-providers.md §2.
    #[test]
    fn a_block_page_is_recognized_by_its_title_alone() {
        let reworded = "<html><head><title>Captcha</title></head>\
             <body><h1>Please prove you are human</h1></body></html>";
        // The premise: the old signal really is absent from this page.
        let lower = reworded.to_ascii_lowercase();
        assert!(
            !CHALLENGE_MARKERS.iter().any(|m| lower.contains(m)),
            "fixture must not carry a body phrase, or it proves nothing"
        );
        assert!(is_challenge_page(reworded, "rust language"));
        // Cloudflare appends its own name after the phrase.
        assert!(is_challenge_page(
            "<title>Attention Required! | Cloudflare</title>",
            "rust language"
        ));
    }

    /// The guard that lets someone search *for* anti-bot topics. A search engine
    /// titles a result set after the query, so a title containing the query is a
    /// results page whatever else it says — which is what makes the short,
    /// generic `CHALLENGE_TITLES` list safe.
    #[test]
    fn a_fruitless_search_for_captcha_is_not_a_challenge() {
        let empty_results = "<html><head><title>captcha bypass - Mojeek Search</title></head>\
             <body><p>No results found.</p></body></html>";
        assert!(!is_challenge_page(empty_results, "captcha bypass"));
        // ...and the same page for an unrelated query *would* read as a block:
        // that is the trade this check makes, and it errs toward "retry later"
        // rather than toward "the web knows nothing".
        assert!(is_challenge_page(empty_results, "rust language"));
    }

    #[test]
    fn page_title_is_normalized() {
        assert_eq!(
            page_title("<html><head><title>  Just\n a   Moment  </title></head></html>"),
            "just a moment"
        );
        assert_eq!(page_title("<html><body>no title</body></html>"), "");
    }

    /// With nothing known to be blocking, the order is the declared preference
    /// order — so a fresh process behaves exactly as before this change.
    #[test]
    fn provider_order_is_the_declared_one_when_nothing_is_cooling() {
        let tool = WebSearch::default();
        let names: Vec<_> = tool.backend_order().iter().map(|b| b.name()).collect();
        assert_eq!(
            names,
            PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>(),
            "no cooldown recorded must mean no reordering"
        );
    }

    fn keys() -> Vec<(SearchSlot, String)> {
        vec![
            (SearchSlot::Tavily, "tvly-x".into()),
            (SearchSlot::Brave, "brave-x".into()),
        ]
    }

    /// The invariant the whole keyed track rests on: **no key configured means
    /// the tool is what it was**. If this ever fails, a fresh install has
    /// quietly changed how it searches.
    #[test]
    fn without_a_key_the_order_is_the_keyless_chain_alone() {
        for provider in WebProvider::ALL {
            let tool = WebSearch::new(
                true,
                crate::shared::net::AddressPolicy::PublicOnly,
                keyed_backends(provider, &[]),
            );
            let names: Vec<_> = tool.backend_order().iter().map(|b| b.name()).collect();
            assert_eq!(
                names,
                PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>(),
                "{provider:?} with no key must be the keyless chain"
            );
        }
    }

    /// `auto` uses every key present; a named provider uses only its own (so
    /// choosing Tavily never quietly spends a Brave key); `free_only` uses none.
    #[test]
    fn the_provider_choice_decides_which_keys_are_used() {
        let names = |p| -> Vec<&'static str> {
            keyed_backends(p, &keys())
                .iter()
                .map(|b| b.name())
                .collect()
        };
        assert_eq!(names(WebProvider::Auto), vec!["Tavily", "Brave"]);
        assert_eq!(names(WebProvider::Tavily), vec!["Tavily"]);
        assert_eq!(names(WebProvider::Brave), vec!["Brave"]);
        assert!(names(WebProvider::FreeOnly).is_empty());
    }

    /// A blank key is not a key: a cleared settings field must not put a
    /// backend into the order that can only answer 401.
    #[test]
    fn a_blank_key_yields_no_backend() {
        let blank = vec![
            (SearchSlot::Tavily, "   ".to_string()),
            (SearchSlot::Brave, String::new()),
        ];
        assert!(keyed_backends(WebProvider::Auto, &blank).is_empty());
    }

    /// Keyed backends go **before** the keyless chain (user's decision: a dead
    /// round trip through a blocked scraper costs more than a credit does).
    #[test]
    fn keyed_backends_are_tried_before_the_free_chain() {
        let tool = WebSearch::new(
            true,
            crate::shared::net::AddressPolicy::PublicOnly,
            keyed_backends(WebProvider::Auto, &keys()),
        );
        let names: Vec<_> = tool.backend_order().iter().map(|b| b.name()).collect();
        assert_eq!(&names[..2], &["Tavily", "Brave"]);
        assert_eq!(names.len(), 2 + PROVIDERS.len());
    }

    /// ...and a keyed backend that rate-limited us is reordered by the same
    /// cooldown as a scraper, not treated as a special case.
    #[test]
    fn a_rate_limited_keyed_backend_moves_back_too() {
        let tool = WebSearch::new(
            true,
            crate::shared::net::AddressPolicy::PublicOnly,
            keyed_backends(WebProvider::Auto, &keys()),
        );
        tool.mark_blocked("tavily");
        let names: Vec<_> = tool.backend_order().iter().map(|b| b.name()).collect();
        assert_eq!(names[0], "Brave", "the un-blocked keyed backend leads");
        assert_eq!(
            names.last(),
            Some(&"Tavily"),
            "the rate-limited one goes last, but is still there"
        );
    }

    #[test]
    fn tavily_results_are_parsed_with_their_page_text() {
        let body = serde_json::json!({"results": [
            {"title": "Rust", "url": "https://rust-lang.org",
             "content": "A language empowering everyone", "raw_content": "  Full page text.  "},
            {"title": "No URL"},
            {"title": "Ratatui", "url": "https://ratatui.rs", "content": "TUI library"}
        ]});
        let r = parse_tavily(&body, 5);
        assert_eq!(r.len(), 2, "an item without a url is dropped");
        assert_eq!(r[0].url, "https://rust-lang.org");
        assert_eq!(r[0].snippet, "A language empowering everyone");
        assert_eq!(
            r[0].content, "Full page text.",
            "raw_content lands as content, trimmed — that is what spares the fetch"
        );
        assert!(
            r[1].content.is_empty(),
            "no raw_content means the page still needs fetching"
        );
        assert_eq!(parse_tavily(&body, 1).len(), 1, "max_results is honoured");
    }

    #[test]
    fn brave_results_are_parsed_and_their_snippets_de_marked_up() {
        let body = serde_json::json!({"web": {"results": [
            {"title": "Rust", "url": "https://rust-lang.org",
             "description": "A <strong>language</strong> empowering everyone"},
            {"url": "https://no-title.example"}
        ]}});
        let r = parse_brave(&body, 5);
        assert_eq!(r.len(), 1, "an item without a title is dropped");
        assert_eq!(
            r[0].snippet, "A language empowering everyone",
            "Brave marks query terms with <strong>; the model reads text, not markup"
        );
        assert!(
            r[0].content.is_empty(),
            "Brave returns snippets only — the pages are fetched as for the free chain"
        );
    }

    /// A shape neither vendor documents but both could send on an off day.
    #[test]
    fn a_malformed_api_body_parses_to_nothing_rather_than_panicking() {
        for body in [
            serde_json::json!({}),
            serde_json::json!({"results": "not a list"}),
            serde_json::json!({"web": {}}),
            serde_json::json!(null),
        ] {
            assert!(parse_tavily(&body, 5).is_empty());
            assert!(parse_brave(&body, 5).is_empty());
        }
    }

    /// A block moves the whole **family** back, not just the entry that saw it:
    /// the two DuckDuckGo hosts share one throttle, so hitting the second after
    /// the first is the wasted round trip this exists to stop.
    #[test]
    fn a_block_moves_the_whole_family_to_the_back() {
        let tool = WebSearch::default();
        tool.mark_blocked(DDG);
        let names: Vec<_> = tool.backend_order().iter().map(|b| b.name()).collect();
        assert_eq!(
            names,
            vec!["Mojeek", "Ecosia", "DuckDuckGo lite", "DuckDuckGo html"]
        );
    }

    /// Every family blocked must still yield every provider: a cooling one is
    /// reordered, never skipped. Otherwise a stale cooldown would let the tool
    /// report "everything is throttled" without a request going out.
    #[test]
    fn a_cooling_provider_is_reordered_not_skipped() {
        let tool = WebSearch::default();
        for family in [DDG, "mojeek", "ecosia"] {
            tool.mark_blocked(family);
        }
        assert_eq!(
            tool.backend_order().len(),
            PROVIDERS.len(),
            "no provider may be dropped from the order"
        );
    }

    /// Among cooling families the one blocked longest ago is tried first — it is
    /// the likeliest to have recovered.
    #[test]
    fn the_oldest_block_is_retried_first() {
        let tool = WebSearch::default();
        tool.mark_blocked("ecosia");
        std::thread::sleep(Duration::from_millis(20));
        tool.mark_blocked(DDG);
        std::thread::sleep(Duration::from_millis(20));
        tool.mark_blocked("mojeek");
        let names: Vec<_> = tool.backend_order().iter().map(|b| b.name()).collect();
        assert_eq!(
            names,
            vec!["Ecosia", "DuckDuckGo lite", "DuckDuckGo html", "Mojeek"]
        );
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

    /// A result that already carries content (Tavily's `raw_content`) must not
    /// be fetched again — that is the latency the keyed backend was chosen to
    /// avoid, and one fewer automated request in front of the site's own
    /// anti-bot. Asserted on the *selection*, not on the field afterwards: an
    /// unfetchable URL leaves the field intact either way, so the weaker
    /// assertion passes even with the skip removed (measured — it did).
    #[test]
    fn only_results_without_content_are_fetched() {
        let r = |content: &str| SearchResult {
            title: "t".into(),
            url: "https://example.test/p".into(),
            snippet: "s".into(),
            content: content.into(),
        };
        let results = vec![r("already have this"), r(""), r("and this")];
        assert_eq!(
            needs_content(&results),
            vec![1],
            "only the result with no content may be fetched"
        );
        assert!(
            needs_content(&[r("x"), r("y")]).is_empty(),
            "a fully populated set must issue no fetches at all"
        );
        assert_eq!(needs_content(&[r(""), r("")]), vec![0, 1]);
    }

    /// A real network smoke (manual: `cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "requires network access to search providers"]
    async fn live_search_returns_results() {
        let tool = WebSearch::default();
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

    /// The keyed track's live criterion (docs/research/web-search-keyed-providers.md
    /// §8): **ten searches in one run, all ten returning results** — the load
    /// pattern that degrades the keyless chain to two. Silently skipped without
    /// a key, like every other gated smoke.
    ///
    /// Set `MINDFORK_TAVILY_KEY` or `MINDFORK_BRAVE_KEY` to run it. Unlike the
    /// keyless smoke above there is **no skip-on-throttle escape**: a keyed
    /// provider answering 429 within ten searches is a real finding about the
    /// free tier, not an infrastructure excuse, and this test exists to catch it.
    #[tokio::test]
    #[ignore = "requires a keyed search provider (MINDFORK_TAVILY_KEY / MINDFORK_BRAVE_KEY)"]
    async fn live_keyed_search_survives_ten_searches_in_a_row() {
        let keyed: Vec<_> = [
            (SearchSlot::Tavily, "MINDFORK_TAVILY_KEY"),
            (SearchSlot::Brave, "MINDFORK_BRAVE_KEY"),
        ]
        .into_iter()
        .filter_map(|(slot, var)| Some((slot, std::env::var(var).ok()?)))
        .collect();
        if keyed.is_empty() {
            eprintln!("skip: no keyed search provider configured");
            return;
        }
        let backends = keyed_backends(WebProvider::Auto, &keyed);
        let names: Vec<_> = backends.iter().map(|b| b.name()).collect();
        eprintln!("keyed backends under test: {names:?}");
        let tool = WebSearch::new(
            false, // titles and links only: this measures the search, not the fetching
            crate::shared::net::AddressPolicy::PublicOnly,
            backends,
        );
        let (_dir, _storage, ctx) = super::super::testkit::ctx_with_backends(
            uuid::Uuid::new_v4(),
            std::sync::Arc::new(crate::shared::api::mock::MockBackend::scripted(vec![])),
            std::sync::Arc::new(crate::shared::api::mock::MockEmbedder::new(16)),
        );
        // Ten *different* queries: repeating one would let a vendor-side cache
        // answer nine of them and prove nothing about the rate limit.
        let queries = [
            "rust ratatui widget",
            "llama.cpp jinja template",
            "sqlite-vec vector search",
            "feature sliced design",
            "tokio cancellation token",
            "wasmer wasix python",
            "duckduckgo lite anti-bot",
            "tavily search api",
            "brave search api pricing",
            "rust edition 2024 changes",
        ];
        for (i, q) in queries.iter().enumerate() {
            let out = tool
                .invoke(&ctx, serde_json::json!({"query": q, "max_results": 3}))
                .await
                .unwrap_or_else(|e| panic!("search {} of 10 ({q:?}) failed: {e:#}", i + 1));
            assert!(
                out.result.contains("http"),
                "search {} of 10 ({q:?}) returned no links: {}",
                i + 1,
                out.result
            );
            // The header names the backend, which is how a silent fall-through
            // to the keyless chain would show up here rather than passing as a
            // success (see `format_results`).
            assert!(
                names.iter().any(|n| out.result.contains(n)),
                "search {} of 10 fell through to the keyless chain: {}",
                i + 1,
                out.result
            );
        }
    }
}
