//! Обёртка [`McpTool`] — инструмент MCP-сервера за трейтом [`Tool`]
//! (docs/research/plugin-system.md §4.4, этап 3). Описание и JSON-схема — снимок
//! из `tools/list` сервера (**не локализуются** — граница i18n, как probe-ошибки
//! движка); `invoke` → `tools/call` с per-call таймаутом и отменой (`ctx.cancel`);
//! результат клипуется (`max_result_chars`) — ограничение входа в промпт.
//!
//! Id инструмента — `mcp__<server>__<tool>` (конвенция Claude Code), нормализован
//! под лимиты провайдеров function-имён ([`mcp_tool_id`]): `[A-Za-z0-9_-]`,
//! ≤ 64 символов (усечение + hex-хвост от полного имени против коллизий).
//! `enabled_by_default = false` — двойной opt-in (мастер-гейт `config.mcp.enabled`
//! и тумблер в профиле); в статический `CATALOG` эти инструменты не входят
//! (динамический каталог едет снимком в `AppEvent::Settings`).

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use sha2::{Digest, Sha256};

use super::{Tool, ToolContext, ToolOutcome, meta};
use crate::entities::profile::ToolId;
use crate::shared::i18n::Locale;
use crate::shared::mcp::{McpConnection, McpToolInfo};
use crate::shared::server::ServerStatus;

/// Снимок MCP-хоста для UI (едет в `AppEvent::Settings`): динамический каталог
/// инструментов (тумблеры профиля) + статусы серверов (строки в секции
/// «Инструменты»). FSD: живёт в `features` — `screens` не импортирует `app`.
#[derive(Debug, Clone, Default)]
pub struct McpSnapshot {
    /// Метаданные инструментов всех готовых серверов (с полными описаниями).
    pub tools: Vec<meta::ToolInfo>,
    /// Статусы серверов (по id).
    pub servers: Vec<McpServerSnapshot>,
}

/// Снимок одного MCP-сервера для UI.
#[derive(Debug, Clone)]
pub struct McpServerSnapshot {
    pub id: String,
    pub status: ServerStatus,
    /// Число зарегистрированных инструментов (0 — сервер не готов).
    pub tool_count: usize,
    /// Каталог сервера изменился против TOFU-пина — инструменты не
    /// зарегистрированы, ждём подтверждения пользователя (Enter в настройках).
    pub pending_catalog: bool,
}

/// TOFU-хэш каталога инструментов сервера: sha256 по отсортированным
/// (имя, описание, JSON-схема) — любое изменение любого поля (rug-pull, tool
/// poisoning через описания/схемы) меняет хэш. Детерминизм: инструменты
/// сортируются по имени, ключи JSON-объектов у `serde_json` упорядочены (BTreeMap).
pub fn catalog_hash(tools: &[McpToolInfo]) -> String {
    let mut sorted: Vec<&McpToolInfo> = tools.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut hasher = Sha256::new();
    for t in sorted {
        hasher.update(t.name.as_bytes());
        hasher.update([0]);
        hasher.update(t.description.as_bytes());
        hasher.update([0]);
        hasher.update(t.input_schema.to_string().as_bytes());
        hasher.update([0xff]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Префикс id инструментов MCP-серверов. По нему [`super::effective_tool_ids`]
/// гейтит их мастер-выключателем `config.mcp.enabled` (инструменты динамические —
/// в статическом `CATALOG` их нет, lookup гейта невозможен).
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// Потолок длины имени function-инструмента у провайдеров (исторически
/// `^[a-zA-Z0-9_-]{1,64}$` у OpenAI/Anthropic).
const MAX_TOOL_ID: usize = 64;

/// Полный id инструмента MCP-сервера: `mcp__<server>__<tool>`, санитизированный
/// под лимиты провайдеров: символы вне `[A-Za-z0-9_-]` → `_`; длиннее 64 —
/// усечение + `_`-разделитель + 8 hex от хеша **полного** имени (коллизии
/// длинных имён не склеиваются). Чистая функция — тестируема.
pub fn mcp_tool_id(server: &str, tool: &str) -> ToolId {
    let raw = format!("{MCP_TOOL_PREFIX}{server}__{tool}");
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.len() <= MAX_TOOL_ID {
        return sanitized;
    }
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    let suffix = format!("_{:08x}", (hasher.finish() & 0xffff_ffff) as u32);
    let keep = MAX_TOOL_ID - suffix.len();
    format!("{}{}", &sanitized[..keep], suffix)
}

/// Интернирует строку в `&'static str` (дедуп через глобальный набор).
/// Нужен для `Tool::ui_label`/`ToolInfo.label` (`&'static str`): лейблы MCP —
/// динамические имена инструментов; их конечное число, утечка ограничена
/// (прецедент — интернирование кодов языков в `shared/i18n`).
fn intern(s: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = pool.lock().expect("intern pool poisoned");
    if let Some(existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// Инструмент MCP-сервера за трейтом [`Tool`]. Держит разделяемый транспорт
/// соединения ([`McpConnection`]) — жизненный цикл процесса сервера у
/// `McpManager` (`app/orchestrator/mcp.rs`).
pub struct McpTool {
    id: ToolId,
    /// Имя инструмента на сервере (оригинальное, без префикса/санитизации).
    remote_name: String,
    /// Снимок описания из `tools/list` (не локализуется — текст сервера).
    description: String,
    /// Снимок JSON-схемы аргументов (`inputSchema`).
    input_schema: serde_json::Value,
    /// Короткий лейбл для тумблера профиля (интернированное имя инструмента).
    label: &'static str,
    conn: Arc<McpConnection>,
    /// Per-call таймаут (`tool_timeout_secs` сервера).
    timeout: Duration,
    /// Клип результата в символах (`max_result_chars` сервера).
    max_result_chars: usize,
}

impl McpTool {
    /// Обёртка над инструментом `info` сервера `server_id` на соединении `conn`.
    pub fn new(
        server_id: &str,
        info: &McpToolInfo,
        conn: Arc<McpConnection>,
        timeout: Duration,
        max_result_chars: usize,
    ) -> Self {
        Self {
            id: mcp_tool_id(server_id, &info.name),
            remote_name: info.name.clone(),
            description: info.description.clone(),
            input_schema: info.input_schema.clone(),
            label: intern(&info.name),
            conn,
            timeout,
            max_result_chars: max_result_chars.max(1),
        }
    }
}

/// Клипует текст до `max` символов (по символам, не байтам — кириллица), с
/// пометкой об усечении на языке каркаса профиля (ось A).
fn clip_result(text: &str, max: usize, loc: &Locale) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let clipped: String = text.chars().take(max).collect();
    format!("{clipped}… {}", loc.t("tool.mcp.result_truncated"))
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn description(&self, _loc: &Locale) -> String {
        // Текст сервера — граница i18n (см. docs/history/i18n.md §2.3).
        self.description.clone()
    }

    fn parameters(&self, _loc: &Locale) -> serde_json::Value {
        self.input_schema.clone()
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let result = self
            .conn
            .call_tool(&self.remote_name, args, self.timeout, Some(&ctx.cancel))
            .await?;
        // `isError:true` — ошибка исполнения на сервере: текст отдаётся модели как
        // результат (не протокольная ошибка, spec tools §error handling). Пустой
        // текст ошибки подменяем пометкой — модель должна понять, что вызов не удался.
        let text = if result.is_error && result.text.is_empty() {
            ctx.loc.t("tool.mcp.error_empty").to_string()
        } else {
            result.text
        };
        Ok(ToolOutcome::text(clip_result(
            &text,
            self.max_result_chars,
            ctx.loc,
        )))
    }

    fn group(&self) -> meta::ToolGroup {
        meta::ToolGroup::Plugins
    }

    fn ui_label(&self) -> &'static str {
        self.label
    }

    fn gate(&self) -> Option<meta::ToolGate> {
        Some(meta::ToolGate::Mcp)
    }

    fn enabled_by_default(&self) -> bool {
        // Двойной opt-in (развилка Р7): включается вручную в профиле.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tools::testkit;
    use serde_json::json;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use uuid::Uuid;

    #[test]
    fn tool_id_sanitizes_and_keeps_short_names() {
        assert_eq!(
            mcp_tool_id("fs", "read_text_file"),
            "mcp__fs__read_text_file"
        );
        // Точки/прочие символы (спека допускает `.`) → `_`.
        assert_eq!(mcp_tool_id("srv", "a.b/c d"), "mcp__srv__a_b_c_d");
    }

    #[test]
    fn tool_id_truncates_long_names_with_stable_hash_tail() {
        let long = "x".repeat(100);
        let id1 = mcp_tool_id("server-with-long-id", &long);
        assert_eq!(id1.len(), 64);
        // Детерминированность и различимость: другое полное имя → другой хвост.
        let id2 = mcp_tool_id("server-with-long-id", &format!("{long}y"));
        assert_eq!(id1, mcp_tool_id("server-with-long-id", &long));
        assert_ne!(id1, id2);
        assert!(id1.starts_with("mcp__server-with-long-id__"));
    }

    #[test]
    fn clip_result_is_char_exact_and_localized() {
        use crate::shared::i18n::{Lang, locale};
        let ru = locale(Lang::Ru);
        assert_eq!(clip_result("привет", 10, ru), "привет");
        let clipped = clip_result(&"я".repeat(30), 5, ru);
        assert!(clipped.starts_with("яяяяя"));
        assert!(clipped.contains("усеч"), "{clipped}");
        let en = clip_result(&"a".repeat(30), 5, locale(Lang::En));
        assert!(en.contains("truncated"), "{en}");
    }

    #[test]
    fn intern_dedups() {
        let a = intern("read_file");
        let b = intern("read_file");
        assert!(std::ptr::eq(a, b));
    }

    /// Фейковый MCP-сервер поверх duplex, отвечающий на tools/call заданным
    /// содержимым; возвращает соединение клиента.
    fn conn_with_call_reply(reply_text: String) -> Arc<McpConnection> {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_r, client_w) = tokio::io::split(client_io);
        let (server_r, mut server_w) = tokio::io::split(server_io);
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let Some(id) = msg.get("id").cloned() else {
                    continue;
                };
                let reply = json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "content": [{ "type": "text", "text": reply_text }],
                    "isError": false
                }});
                if server_w
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        Arc::new(McpConnection::over(client_r, client_w))
    }

    #[tokio::test]
    async fn invoke_calls_server_and_clips_result() {
        let conn = conn_with_call_reply("0123456789".repeat(10)); // 100 симв.
        let info = McpToolInfo {
            name: "echo".into(),
            description: "Echo test tool".into(),
            input_schema: json!({ "type": "object" }),
        };
        let tool = McpTool::new("test", &info, conn, Duration::from_secs(5), 20);
        assert_eq!(tool.id(), "mcp__test__echo");
        assert_eq!(tool.group(), meta::ToolGroup::Plugins);
        assert_eq!(tool.gate(), Some(meta::ToolGate::Mcp));
        assert!(!tool.enabled_by_default());

        let (_d, _s, ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        let out = tool.invoke(&ctx, json!({ "text": "hi" })).await.unwrap();
        assert!(out.effects.is_empty());
        assert!(out.result.starts_with("01234567890123456789"));
        assert!(out.result.contains("усеч"), "{}", out.result);
    }

    #[tokio::test]
    async fn invoke_is_cancellable_via_ctx_cancel() {
        // Сервер молчит → отмена ctx.cancel прерывает вызов (Esc не блокируется).
        let (client_io, _server_io_keepalive) = tokio::io::duplex(64 * 1024);
        let (client_r, client_w) = tokio::io::split(client_io);
        let conn = Arc::new(McpConnection::over(client_r, client_w));
        let info = McpToolInfo {
            name: "slow".into(),
            description: String::new(),
            input_schema: json!({ "type": "object" }),
        };
        let tool = McpTool::new("test", &info, conn, Duration::from_secs(60), 100);
        let (_d, _s, ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        let tok = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tok.cancel();
        });
        let err = tool.invoke(&ctx, json!({})).await.unwrap_err().to_string();
        assert!(err.contains("отменён"), "{err}");
    }
}
