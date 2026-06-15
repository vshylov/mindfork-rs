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

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use scraper::{Html, Selector};

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// UA, чтобы поисковики отдавали нормальную разметку (а не «лёгкую»/пустую).
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/124.0 Safari/537.36";
/// Результатов по умолчанию.
const DEFAULT_MAX_RESULTS: usize = 5;
/// Жёсткий потолок результатов.
const MAX_RESULTS_CAP: usize = 10;
/// Таймаут одного HTTP-запроса к поисковику.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

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
}

/// `web_search` — поиск в интернете (DuckDuckGo → Mojeek → Ecosia, см. [`PROVIDERS`]).
pub struct WebSearch {
    http: reqwest::Client,
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearch {
    pub fn new() -> Self {
        // Таймаут на запрос: иначе зависший ответ DDG держал бы весь ход.
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self { http }
    }

    /// Один HTTP-запрос к провайдеру: `Ok(Some(html))` — нормальная страница;
    /// `Ok(None)` — анти-бот троттлинг (202/anomaly/403/429); `Err` — сеть/прочий HTTP.
    async fn fetch(&self, provider: &Provider, query: &str) -> Result<Option<String>> {
        let req = match provider.method {
            Method::PostForm => self.http.post(provider.url).form(&[("q", query)]),
            Method::GetQuery => {
                let mut url = reqwest::Url::parse(provider.url)
                    .with_context(|| format!("разбор URL {}", provider.name))?;
                url.query_pairs_mut().append_pair("q", query);
                self.http.get(url)
            }
        };
        let resp = req
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .with_context(|| format!("запрос к {}", provider.name))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .with_context(|| format!("чтение ответа {}", provider.name))?;
        if is_throttled(status, &body) {
            return Ok(None);
        }
        if !status.is_success() {
            anyhow::bail!("{} вернул статус {status}", provider.name);
        }
        Ok(Some(body))
    }
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn id(&self) -> ToolId {
        "web_search".into()
    }
    fn description(&self) -> String {
        "Искать в интернете. Возвращает заголовки, ссылки и сниппеты.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": MAX_RESULTS_CAP}
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле query"))?;
        let max = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_RESULTS_CAP))
            .unwrap_or(DEFAULT_MAX_RESULTS);

        // Перебираем провайдеров по порядку: первый, кто отдал непустую выдачу,
        // выигрывает. При троттлинге сразу уходим к следующему (ретраить липкий
        // per-IP троттл бессмысленно). `got_clean_page` — хоть один провайдер
        // вернул нормальную (не вызов-)страницу: тогда пустота — настоящее «нет
        // результатов», а не троттлинг.
        let mut got_clean_page = false;
        let mut last_err: Option<anyhow::Error> = None;
        let mut results = Vec::new();
        for provider in PROVIDERS {
            match self.fetch(provider, query).await {
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
                return Ok(ToolOutcome::text("Поиск не дал результатов."));
            }
            // Ни один провайдер не отдал нормальную страницу: троттлинг и/или
            // сетевые ошибки. Возвращаем ошибку (а не «нет результатов»), чтобы
            // модель повторила запрос позже, а не сообщила, что ничего не нашла.
            if let Some(err) = last_err {
                return Err(err.context("все поисковые провайдеры недоступны"));
            }
            anyhow::bail!(
                "Поиск временно недоступен: все поисковики включили анти-бот троттлинг. \
                 Повтори запрос через несколько секунд."
            );
        }
        let mut out = format!("Результаты поиска ({}):\n", results.len());
        for (i, r) in results.iter().enumerate() {
            out.push_str(&format!("{}. {} — {}\n", i + 1, r.title, r.url));
            if !r.snippet.is_empty() {
                out.push_str(&format!("   {}\n", r.snippet));
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

    /// Реальный сетевой смоук (вручную: `cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "requires network access to search providers"]
    async fn live_search_returns_results() {
        let tool = WebSearch::new();
        let dir = tempfile::tempdir().unwrap();
        let storage = std::sync::Arc::new(
            crate::shared::storage::Storage::open(crate::shared::paths::Paths::with_root(
                dir.path(),
            ))
            .unwrap(),
        );
        let ctx = ToolContext {
            profile_id: uuid::Uuid::new_v4(),
            chat_id: uuid::Uuid::new_v4(),
            system_message: String::new(),
            effective_sampling: Default::default(),
            last_user_message_at: None,
            storage,
            engine: std::sync::Arc::new(crate::shared::api::mock::MockBackend::scripted(vec![])),
            embedder: std::sync::Arc::new(crate::shared::api::mock::MockEmbedder::new(16)),
        };
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
