//! Инструмент `web_search` (spec §9.3.1): поиск в DuckDuckGo (lite-эндпоинт)
//! собственным `reqwest`-клиентом + парсинг результатов (`scraper`). Под
//! глобальным выключателем `tools.web_enabled` (приватность, §9.4).
//!
//! Используется `https://lite.duckduckgo.com/lite/` (POST `q=...`) — простейшая
//! и стабильная для скрейпинга HTML-разметка (ADR-решение M7). Извлечение
//! читаемого контента страниц и реранкинг эмбеддингами — на будущее.

use anyhow::{Context, Result};
use scraper::{Html, Selector};

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Endpoint lite-версии DuckDuckGo.
const DDG_LITE_URL: &str = "https://lite.duckduckgo.com/lite/";
/// UA, чтобы DDG отдавал нормальную разметку.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/124.0 Safari/537.36";
/// Результатов по умолчанию.
const DEFAULT_MAX_RESULTS: usize = 5;
/// Жёсткий потолок результатов.
const MAX_RESULTS_CAP: usize = 10;

/// Один результат поиска.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// `web_search` — поиск в интернете (DuckDuckGo lite).
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
        Self {
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn id(&self) -> ToolId {
        "web_search".into()
    }
    fn description(&self) -> String {
        "Искать в интернете (DuckDuckGo). Возвращает заголовки, ссылки и сниппеты.".into()
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

        let html = self
            .http
            .post(DDG_LITE_URL)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .form(&[("q", query)])
            .send()
            .await
            .context("запрос к DuckDuckGo")?
            .error_for_status()
            .context("DuckDuckGo вернул ошибку")?
            .text()
            .await
            .context("чтение ответа DuckDuckGo")?;

        let results = parse_lite_results(&html, max);
        if results.is_empty() {
            return Ok(ToolOutcome::text("Поиск не дал результатов."));
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

/// Парсит HTML lite-выдачи DuckDuckGo: ссылки `a.result-link` + сниппеты
/// `td.result-snippet`. Реальный URL извлекается из редиректа `uddg=...`.
fn parse_lite_results(html: &str, max: usize) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let link_sel = Selector::parse("a.result-link").unwrap();
    let snippet_sel = Selector::parse("td.result-snippet").unwrap();

    let snippets: Vec<String> = doc
        .select(&snippet_sel)
        .map(|el| collapse_ws(&el.text().collect::<String>()))
        .collect();

    let mut results = Vec::new();
    for (i, link) in doc.select(&link_sel).enumerate() {
        if results.len() >= max {
            break;
        }
        let title = collapse_ws(&link.text().collect::<String>());
        let href = link.value().attr("href").unwrap_or_default();
        let url = extract_real_url(href);
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

    #[test]
    fn parses_results_and_decodes_urls() {
        let results = parse_lite_results(FIXTURE, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Пример A");
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].snippet, "Сниппет про A"); // схлопнуты пробелы
        assert_eq!(results[1].url, "https://example.org/b");
    }

    #[test]
    fn respects_max_results() {
        assert_eq!(parse_lite_results(FIXTURE, 1).len(), 1);
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
    #[ignore = "requires network access to DuckDuckGo"]
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
