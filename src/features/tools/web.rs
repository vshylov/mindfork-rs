//! Инструмент `web_search` (spec §9.3.1): поиск в интернете собственным
//! `reqwest`-клиентом + парсинг HTML-выдачи (`scraper`). Под глобальным
//! выключателем `tools.web_enabled` (приватность, §9.4).
//!
//! **Несколько независимых провайдеров с фоллбэком** (см. [`PROVIDERS`]). По
//! порядку: DuckDuckGo lite (`POST q=`, простейшая разметка, ADR-решение M7) →
//! DuckDuckGo html (иная разметка) → **Mojeek** → **Ecosia** (`GET ?q=`, у каждого
//! своя инфраструктура и разметка). Первый, кто вернул непустую выдачу, выигрывает.
//!
//! **Анти-бот троттлинг.** При нескольких быстрых запросах подряд (что бывает в
//! agentic-loop на сложной/длинной задаче) поисковики режут трафик по IP:
//! DuckDuckGo отдаёт `HTTP 202` со страницей-вызовом («anomaly»), Mojeek/прочие —
//! `403`/`429`, а не результаты. Раньше `202` считался «успехом» (`error_for_status`
//! пропускает 2xx) → парсилась пустая страница → модель видела «Поиск не дал
//! результатов» (хотя запрос корректен), а `403` всплывал как фатальная ошибка
//! «провайдер недоступен». Теперь троттлинг распознаётся ([`is_throttled`]) и при
//! нём сразу пробуется следующий провайдер (троттл липкий per-IP — ретраи его лишь
//! углубляют; разные провайдеры режут независимо, поэтому почти всегда отвечает
//! кто-то один). Если **все** недоступны/троттлят — возвращается явная ошибка (а не
//! «нет результатов»), чтобы модель повторила запрос позже, а не сообщила, что
//! ничего не нашла.
//!
//! **Извлечение контента + реранкинг** (spec §9.3.1, по умолчанию включены,
//! отключаются аргументом `fetch_content`). После получения выдачи страницы
//! результатов загружаются и из них извлекается читаемый текст ([`extract_readable`]
//! на `scraper`: содержимое `<article>`/`<main>`/абзацев, без script/nav-мусора) —
//! «лучшее усилие»: ошибка загрузки одной страницы не валит поиск. Затем результаты
//! **переупорядочиваются эмбеддингами** (через `ctx.embedder`, ADR 0002): запрос и
//! контент каждого результата эмбеддятся, сортировка по убыванию косинусной близости
//! ([`rerank_order`]). Эмбеддер не настроен/недоступен (RAG выключен) → реранкинг
//! пропускается, остаётся порядок провайдера (мягкая деградация, как у RAG).

use std::cmp::Ordering;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use scraper::{Html, Selector};

use crate::entities::profile::ToolId;
use crate::shared::api::Embedder;

use super::{Tool, ToolContext, ToolOutcome};

/// UA, чтобы поисковики отдавали нормальную разметку (а не «лёгкую»/пустую).
/// `pub(crate)` — переиспользуется `fetch_url` (см. `tools/fetch.rs`).
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/124.0 Safari/537.36";
/// `Accept` для загрузки страниц контента (как у браузера).
pub(crate) const ACCEPT_HTML: &str =
    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
/// `Accept-Language` для загрузки страниц контента.
pub(crate) const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9,ru;q=0.8";
/// Результатов по умолчанию.
const DEFAULT_MAX_RESULTS: usize = 5;
/// Жёсткий потолок результатов.
const MAX_RESULTS_CAP: usize = 10;
/// Таймаут одного HTTP-запроса к поисковику.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Потолок извлекаемого читаемого текста одной страницы (символы). Ограничивает
/// раздувание контекста и размер эмбеддинг-запроса.
const MAX_CONTENT_CHARS: usize = 1500;
/// Минимальная длина фрагмента (абзаца) при извлечении: короче — вероятно
/// навигация/меню/кнопки, а не контент.
const MIN_FRAGMENT_CHARS: usize = 40;
/// Сколько символов контента результата идёт в эмбеддинг при реранкинге (хватает
/// репрезентативного начала; не раздувает запрос к эмбеддеру).
const RERANK_EMBED_CHARS: usize = 800;

/// HTTP-метод запроса к поисковику.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Method {
    /// `q` в теле формы (DuckDuckGo).
    PostForm,
    /// `q` в query-строке (Mojeek).
    GetQuery,
}

/// Описание поискового провайдера: эндпоинт, метод и CSS-селекторы выдачи.
struct Provider {
    /// Имя для логов/ошибок.
    name: &'static str,
    url: &'static str,
    method: Method,
    /// Селектор ссылки-результата (откуда берётся `href`).
    link_sel: &'static str,
    /// Селектор заголовка (его текст). У DDG/Mojeek совпадает с `link_sel` (заголовок
    /// и ссылка — один тег `<a>`); у Ecosia заголовок лежит отдельно от ссылки.
    title_sel: &'static str,
    /// Селектор сниппета. Списки ссылок/заголовков/сниппетов выравниваются по
    /// индексу (i-й результат = i-я ссылка + i-й заголовок + i-й сниппет).
    snippet_sel: &'static str,
}

/// Провайдеры в порядке предпочтения, каждый со своей разметкой и (важно)
/// инфраструктурой. DuckDuckGo (два варианта разметки) — основной; далее
/// независимые **Mojeek** и **Ecosia**. Анти-бот троттлинг у каждого свой и
/// кратковременный (per-IP); он липкий, ретраить один и тот же провайдер
/// бессмысленно — поэтому при троттлинге сразу уходим к следующему. Несколько
/// независимых провайдеров → при недоступности одного почти всегда отвечает другой.
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
        // Стабильные семантические `data-test-id` (а не хешированные css-классы).
        link_sel: r#"a[data-test-id="result-link"]"#,
        title_sel: r#"[data-test-id="result-title"]"#,
        snippet_sel: r#"[data-test-id="web-result-description"]"#,
    },
];

/// Один результат поиска.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Извлечённый читаемый текст страницы (пусто, если не загружали/не вышло).
    pub content: String,
}

/// `web_search` — поиск в интернете (DuckDuckGo → Mojeek → Ecosia, см. [`PROVIDERS`]).
pub struct WebSearch {
    http: reqwest::Client,
    /// Значение по умолчанию для аргумента `fetch_content` (из `config.tools`).
    fetch_content_default: bool,
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::new(true)
    }
}

impl WebSearch {
    pub fn new(fetch_content_default: bool) -> Self {
        // Таймаут на запрос: иначе зависший ответ DDG держал бы весь ход.
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            fetch_content_default,
        }
    }

    /// Один HTTP-запрос к провайдеру: `Ok(Some(html))` — нормальная страница;
    /// `Ok(None)` — анти-бот троттлинг (202/anomaly/403/429); `Err` — сеть/прочий HTTP.
    /// `loc` — язык каркаса для текстов ошибок (уходят модели при полном отказе).
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

    /// Загружает страницу результата и извлекает читаемый текст. `None` при любой
    /// ошибке/не-HTML — извлечение «лучшее усилие», поиск не должен падать из-за
    /// одной недоступной страницы.
    async fn fetch_content(&self, url: &str) -> Option<String> {
        let resp = match self
            .http
            .get(url)
            // Браузероподобные заголовки: часть сайтов отдаёт пустую/блок-страницу
            // на «голый» запрос без Accept/Accept-Language.
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, ACCEPT_HTML)
            .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
            .send()
            .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::debug!(url, error = %err, "web-поиск: страница не загрузилась");
                return None;
            }
        };
        if !resp.status().is_success() {
            tracing::debug!(url, status = %resp.status(), "web-поиск: страница вернула не-2xx");
            return None;
        }
        // Берём только HTML (PDF/изображения/прочее извлекать нечем). Заголовок
        // может отсутствовать — тогда пробуем как HTML.
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
            tracing::debug!(url, "web-поиск: из страницы не извлечён читаемый текст");
        }
        (!text.is_empty()).then_some(text)
    }

    /// Параллельно загружает страницы результатов и проставляет извлечённый текст в
    /// `content`. Каждая загрузка независима и отказоустойчива (см. [`Self::fetch_content`]).
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

/// Переупорядочивает результаты по убыванию близости их контента к запросу
/// (реранкинг эмбеддингами, spec §9.3.1). Эмбеддер недоступен/вернул нестыкующееся
/// число векторов → результаты не трогаем (мягкая деградация). `query` и контент
/// каждого результата эмбеддятся одним запросом.
async fn rerank_by_embeddings(
    embedder: &dyn Embedder,
    query: &str,
    results: &mut Vec<SearchResult>,
) {
    if results.len() < 2 {
        return; // нечего переупорядочивать
    }
    let mut texts: Vec<String> = Vec::with_capacity(results.len() + 1);
    texts.push(query.to_string());
    texts.extend(results.iter().map(rerank_text));
    let vecs = match embedder.embed(texts).await {
        Ok(v) if v.len() == results.len() + 1 => v,
        Ok(_) => return, // несоответствие — не рискуем перемешать
        Err(err) => {
            tracing::debug!(error = %err, "web-поиск: реранкинг недоступен, порядок провайдера");
            return;
        }
    };
    let query_vec = &vecs[0];
    let order = rerank_order(query_vec, &vecs[1..]);
    *results = order.into_iter().map(|i| results[i].clone()).collect();
}

/// Текст результата для эмбеддинга при реранкинге: контент (если извлечён) с
/// заголовком/сниппетом в качестве контекста; контент усечён до [`RERANK_EMBED_CHARS`].
fn rerank_text(r: &SearchResult) -> String {
    let body = if r.content.is_empty() {
        r.snippet.clone()
    } else {
        truncate_chars(&r.content, RERANK_EMBED_CHARS)
    };
    format!("{}\n{}", r.title, body).trim().to_string()
}

/// Порядок индексов `doc_vecs` по убыванию косинусной близости к `query_vec`.
fn rerank_order(query_vec: &[f32], doc_vecs: &[Vec<f32>]) -> Vec<usize> {
    let sims: Vec<f32> = doc_vecs.iter().map(|v| cosine(query_vec, v)).collect();
    let mut order: Vec<usize> = (0..doc_vecs.len()).collect();
    // Стабильная сортировка: при равной близости сохраняется порядок провайдера.
    order.sort_by(|&a, &b| sims[b].partial_cmp(&sims[a]).unwrap_or(Ordering::Equal));
    order
}

/// Косинусная близость двух векторов (0.0 при несовпадении длин/нулевой норме).
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

/// Извлекает читаемый текст HTML-страницы (упрощённый readability): берёт абзацы и
/// списки из `<article>`/`<main>` (если есть), иначе — из всего документа; короткие
/// фрагменты и всё внутри навигации/шапки/подвала/сайдбара ([`in_boilerplate`])
/// отбрасываются (иначе на сайтах без семантической разметки в контент попадает
/// мега-меню). script/style не попадают (их текст не внутри `<p>`/`<li>`). Результат
/// усечён до `max_chars` символов.
///
/// `pub(crate)` — переиспользуется `fetch_url` (см. `tools/fetch.rs`).
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

    // Предпочитаем основное содержимое (article/main) — меньше навигационного шума.
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

/// `true`, если элемент лежит внутри навигации/шапки/подвала/сайдбара — это
/// boilerplate (меню/ссылки), а не основной контент.
fn in_boilerplate(el: scraper::ElementRef) -> bool {
    el.ancestors().any(|n| {
        n.value()
            .as_element()
            .map(|e| matches!(e.name(), "nav" | "header" | "footer" | "aside"))
            .unwrap_or(false)
    })
}

/// Усекает строку до `max` символов (по границе символа, не байта).
/// `pub(crate)` — переиспользуется `fetch_url` (см. `tools/fetch.rs`).
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
        "поиск в интернете"
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

        // Перебираем провайдеров по порядку: первый, кто отдал непустую выдачу,
        // выигрывает. При троттлинге сразу уходим к следующему (ретраить липкий
        // per-IP троттл бессмысленно). `got_clean_page` — хоть один провайдер
        // вернул нормальную (не вызов-)страницу: тогда пустота — настоящее «нет
        // результатов», а не троттлинг.
        let mut got_clean_page = false;
        let mut last_err: Option<anyhow::Error> = None;
        let mut results = Vec::new();
        for provider in PROVIDERS {
            match self.fetch(provider, query, ctx.loc).await {
                Ok(Some(html)) => {
                    got_clean_page = true;
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
                }
                Ok(None) => {
                    tracing::debug!(
                        provider = provider.name,
                        "web-поиск: троттлинг, пробуем следующего провайдера"
                    );
                }
                Err(err) => {
                    tracing::warn!(provider = provider.name, error = %err, "web-поиск: ошибка провайдера");
                    last_err = Some(err);
                }
            }
        }

        if results.is_empty() {
            if got_clean_page {
                // Нормальная страница без результатов — это действительно пусто.
                return Ok(ToolOutcome::text(
                    ctx.loc.t("tool.web_search.result.no_results"),
                ));
            }
            // Ни один провайдер не отдал нормальную страницу: троттлинг и/или
            // сетевые ошибки. Возвращаем ошибку (а не «нет результатов»), чтобы
            // модель повторила запрос позже, а не сообщила, что ничего не нашла.
            if let Some(err) = last_err {
                return Err(
                    err.context(ctx.loc.t("tool.web_search.err.all_unavailable").to_string())
                );
            }
            anyhow::bail!(ctx.loc.t("tool.web_search.err.throttled"));
        }
        // Извлечение контента + реранкинг (если не отключено аргументом).
        // Загрузка страниц и эмбеддинги — «лучшее усилие»: при сбое остаётся
        // обычная выдача (заголовки/сниппеты, порядок провайдера).
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

/// Признак анти-бот троттлинга/блокировки провайдера: `HTTP 202` (DDG
/// страница-вызов), `403`/`429` (Mojeek/прочие при перегрузе по IP) либо маркер
/// `anomaly` в теле DDG. Нормальная выдача `anomaly` не содержит. Такие ответы
/// кратковременны — это не «нет результатов» и не фатальная ошибка.
fn is_throttled(status: StatusCode, body: &str) -> bool {
    matches!(
        status,
        StatusCode::ACCEPTED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
    ) || body.contains("anomaly")
}

/// Парсит выдачу провайдера: списки ссылок (`link_q` → `href`), заголовков
/// (`title_q` → текст) и сниппетов (`snippet_q` → текст) выравниваются по индексу.
/// У DDG/Mojeek `link_q == title_q` (один тег `<a>`); у Ecosia — разные теги.
/// Реальный URL извлекается из редиректа `uddg=...` (DDG) либо берётся как есть.
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

/// Достаёт настоящий URL из ссылки DDG: декодирует параметр `uddg`, либо
/// нормализует протокол-относительный `//host/...`.
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

/// Минимальное percent-декодирование значения query-параметра (`%XX`, `+`→пробел).
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

/// Схлопывает пробелы/переводы строк в один пробел и обрезает края.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_description_is_localized() {
        // Описание web_search различно на ru/en (ловит забытый `_loc`), en без
        // кириллицы. §3.5 docs/i18n.md.
        use crate::shared::i18n::{Lang, locale};
        let tool = WebSearch::new(true);
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        assert_ne!(tool.description(ru), tool.description(en));
        let e = tool.description(en);
        assert!(
            !e.chars()
                .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)),
            "кириллица в en-описании: {e}"
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

    /// Фикстура html-эндпоинта DDG (разметка `result__a` / `result__snippet`).
    const FIXTURE_HTML: &str = r#"
        <html><body>
        <div class="result">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.net%2Fx&amp;rut=y">Пример X</a>
            <a class="result__snippet" href="/snip">Сниппет   X</a>
        </div>
        </body></html>
    "#;

    /// Фикстура Mojeek (прямые ссылки `a.title`, сниппет `p.s`).
    const FIXTURE_MOJEEK: &str = r#"
        <html><body><ul class="results-standard">
        <li><h2><a class="title" title="https://example.io/m" href="https://example.io/m">Пример M</a></h2>
        <p class="s">Сниппет   M</p></li>
        </ul></body></html>
    "#;

    /// Фикстура Ecosia: заголовок и ссылка — РАЗНЫЕ теги (по `data-test-id`).
    const FIXTURE_ECOSIA: &str = r#"
        <html><body>
        <div class="result">
            <a data-test-id="result-link" href="https://example.dev/e" tabindex="-1">https://example.dev/e</a>
            <div data-test-id="result-title">Пример   E</div>
            <p data-test-id="web-result-description">Сниппет E</p>
        </div>
        </body></html>
    "#;

    /// Селекторы DDG-lite (link == title, как в [`PROVIDERS`]).
    const LITE: (&str, &str) = ("a.result-link", "td.result-snippet");

    #[test]
    fn parses_results_and_decodes_urls() {
        let results = parse_results(FIXTURE, LITE.0, LITE.0, LITE.1, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Пример A");
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].snippet, "Сниппет про A"); // схлопнуты пробелы
        assert_eq!(results[1].url, "https://example.org/b");
    }

    #[test]
    fn parses_html_endpoint_layout() {
        // Запасная разметка html-эндпоинта DDG тоже распознаётся.
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
        // Запасной провайдер Mojeek (прямые ссылки, иная разметка).
        let results = parse_results(FIXTURE_MOJEEK, "a.title", "a.title", "p.s", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Пример M");
        assert_eq!(results[0].url, "https://example.io/m");
        assert_eq!(results[0].snippet, "Сниппет M");
    }

    #[test]
    fn parses_ecosia_layout_separate_title_and_link() {
        // У Ecosia заголовок и ссылка — разные теги; парсер выравнивает по индексу.
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
        // Селекторы из PROVIDERS совпадают с тем, что парсят фикстуры.
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
        // HTTP 202 — анти-бот троттлинг DDG (страница-вызов), даже без маркера.
        assert!(is_throttled(
            StatusCode::ACCEPTED,
            "<html>что угодно</html>"
        ));
        // 403/429 — троттлинг/блокировка по IP (Mojeek и прочие).
        assert!(is_throttled(StatusCode::FORBIDDEN, ""));
        assert!(is_throttled(StatusCode::TOO_MANY_REQUESTS, ""));
        // Маркер anomaly в теле — тоже троттлинг.
        assert!(is_throttled(
            StatusCode::OK,
            "...If this error persists... anomaly ..."
        ));
        // Нормальная выдача (200, без маркера) — не троттлинг.
        assert!(!is_throttled(StatusCode::OK, FIXTURE));
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
        // Скрипты/стили и короткие фрагменты (nav/«Короткий.») отброшены.
        assert!(!text.contains("var a"));
        assert!(!text.contains("color:red"));
        assert!(!text.contains("Короткий."));
    }

    #[test]
    fn extract_readable_skips_boilerplate() {
        // Длинный абзац внутри <nav> (нет <main>) — это меню, не контент: отброшен,
        // а абзац вне навигации — взят.
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

    #[test]
    fn cosine_basic() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        // Несовпадение длин / нулевой вектор → 0.0.
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn rerank_order_sorts_by_similarity() {
        let query = vec![1.0, 0.0];
        let docs = vec![
            vec![0.0, 1.0], // ортогонален — наименее похож
            vec![1.0, 0.0], // совпадает — наиболее похож
            vec![0.7, 0.7], // средне
        ];
        let order = rerank_order(&query, &docs);
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn rerank_order_is_stable_on_ties() {
        // При равной близости сохраняется исходный порядок (стабильная сортировка).
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
        // Без контента — берётся сниппет.
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
        // Релевантный запросу результат поднялся наверх.
        assert_eq!(results[0].url, "https://e/rust");
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
        // Эмбеддер недоступен → порядок не изменился.
        assert_eq!(results[0].url, "https://e/a");
        assert_eq!(results[1].url, "https://e/b");
    }

    /// Реальный сетевой смоук (вручную: `cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "requires network access to search providers"]
    async fn live_search_returns_results() {
        let tool = WebSearch::new(true);
        let (_dir, _storage, ctx) = super::super::testkit::ctx_with_backends(
            uuid::Uuid::new_v4(),
            std::sync::Arc::new(crate::shared::api::mock::MockBackend::scripted(vec![])),
            std::sync::Arc::new(crate::shared::api::mock::MockEmbedder::new(16)),
        );
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"query": "rust language", "max_results": 3}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("http"), "got: {}", out.result);
    }
}
