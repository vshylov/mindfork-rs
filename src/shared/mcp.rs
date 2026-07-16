//! Мини-клиент **MCP** (Model Context Protocol) — stdio, tools-only. **Зонд**
//! направления «плагины» (docs/research/plugin-system.md §4, §7, этап 2): проверяем,
//! что живая локальная модель корректно вызывает инструменты реального MCP-сервера
//! через наш agentic-loop. Целевая ревизия — 2025-11-25; подмножество wire-стабильно
//! с 2024-11-05, поэтому встречная версия сервера принимается любая (для tools-only
//! методы идентичны во всех ревизиях).
//!
//! Объём (§4.2 исследования): подпроцесс + newline-delimited JSON-RPC 2.0 (UTF-8,
//! stdout сервера — только протокол, stderr дренируется в лог), `initialize` →
//! `notifications/initialized`, `tools/list` (пагинация), `tools/call`
//! (`isError:true` → текст ошибки модели), ответ на `ping`, `-32601` на прочие
//! запросы сервера (иначе корректный сервер повиснет), `notifications/cancelled`
//! при таймауте, shutdown-лестница (закрыть stdin → подождать → kill). Известные
//! питфоллы (§4.6): мусорные строки в stdout пропускаются с warn; неизвестные
//! нотификации игнорируются.
//!
//! Транспорт отделён от процесса ([`McpConnection::over`] поверх любых
//! `AsyncRead`/`AsyncWrite`) — юнит-тесты гоняют протокол на `tokio::io::duplex`
//! без процессов; [`McpClient::spawn`] добавляет управление подпроцессом.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

/// Версия протокола, которую предлагает клиент.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// Таймаут ожидания ответа на `initialize`/`tools/list` (стартовые запросы).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Сколько ждать выхода сервера после закрытия stdin (shutdown-лестница).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Инструмент MCP-сервера (снимок из `tools/list`).
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    /// JSON Schema объекта аргументов (`inputSchema`).
    pub input_schema: Value,
}

/// Результат `tools/call`: текстовые блоки содержимого одной строкой.
#[derive(Debug, Clone)]
pub struct McpCallResult {
    pub text: String,
    /// Ошибка исполнения инструмента (`isError:true`) — текст отдаётся модели
    /// как результат-ошибка, это НЕ протокольная ошибка (spec tools §error handling).
    pub is_error: bool,
}

/// Разделяемый writer соединения: пишут и наши запросы, и reader-задача
/// (ответы на `ping`/`-32601`).
type SharedWriter = Arc<Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;
/// Ожидающие ответа запросы: id → отправитель результата (`Err` — текст JSON-RPC-ошибки).
type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

/// Транспортная половина: канал запрос→ответ поверх пары reader/writer.
/// Ответы маршрутизируются по `id` (oneshot); запросы сервера обслуживаются
/// в reader-задаче (`ping` → пустой результат, прочее → `-32601`), нотификации
/// игнорируются, не-JSON строки пропускаются с warn.
pub struct McpConnection {
    writer: SharedWriter,
    pending: PendingMap,
    next_id: AtomicI64,
    reader_task: tokio::task::JoinHandle<()>,
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

impl McpConnection {
    /// Подключение поверх произвольной пары потоков (для тестов — `duplex`).
    pub fn over(
        reader: impl AsyncRead + Send + Unpin + 'static,
        writer: impl AsyncWrite + Send + Unpin + 'static,
    ) -> Self {
        let writer: SharedWriter = Arc::new(Mutex::new(Box::new(writer)));
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let reader_task = tokio::spawn(read_loop(reader, writer.clone(), pending.clone()));
        Self {
            writer,
            pending,
            next_id: AtomicI64::new(1),
            reader_task,
        }
    }

    /// Отправляет запрос и ждёт ответ не дольше `timeout`. По таймауту шлёт
    /// `notifications/cancelled` (spec lifecycle §timeouts) и возвращает ошибку.
    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send_line(&msg).await?;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(rpc_err))) => bail!("MCP {method}: {rpc_err}"),
            // Канал закрыт: reader-задача умерла (сервер закрыл stdout/битый поток).
            Ok(Err(_)) => bail!("MCP {method}: соединение закрыто сервером"),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                // Уведомляем сервер об отмене (он может прекратить работу); ответ,
                // если всё же придёт, отбросится как неизвестный id.
                let cancel = json!({
                    "jsonrpc": "2.0", "method": "notifications/cancelled",
                    "params": { "requestId": id, "reason": "timeout" }
                });
                let _ = self.send_line(&cancel).await;
                bail!("MCP {method}: таймаут {}с", timeout.as_secs())
            }
        }
    }

    /// Отправляет нотификацию (без id и без ожидания ответа).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send_line(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn send_line(&self, msg: &Value) -> Result<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        let mut w = self.writer.lock().await;
        w.write_all(line.as_bytes()).await?;
        w.flush().await?;
        Ok(())
    }
}

/// Reader-петля: строка → JSON → маршрутизация (ответ / запрос сервера / нотификация).
async fn read_loop(
    reader: impl AsyncRead + Send + Unpin + 'static,
    writer: SharedWriter,
    pending: PendingMap,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                // Питфолл §4.6: серверы печатают баннеры/логи в stdout — пропускаем
                // строку, не рвя соединение.
                tracing::warn!(line = %clip_line(&line), "MCP: не-JSON строка в stdout, пропущена");
                continue;
            }
        };
        let id = msg.get("id");
        let has_method = msg.get("method").is_some();
        match (id, has_method) {
            // Ответ на наш запрос.
            (Some(id_v), false) => {
                let Some(id) = id_v.as_i64() else { continue };
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let outcome = match msg.get("error") {
                        Some(e) => Err(e
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("ошибка без описания")
                            .to_string()),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(outcome);
                }
            }
            // Запрос сервера к нам: ping отвечаем, прочее — method-not-found
            // (молчание подвесило бы корректный сервер).
            (Some(id_v), true) => {
                let method = msg["method"].as_str().unwrap_or_default();
                let reply = if method == "ping" {
                    json!({ "jsonrpc": "2.0", "id": id_v, "result": {} })
                } else {
                    json!({ "jsonrpc": "2.0", "id": id_v,
                            "error": { "code": -32601, "message": "method not found" } })
                };
                let mut line = reply.to_string();
                line.push('\n');
                let mut w = writer.lock().await;
                let _ = w.write_all(line.as_bytes()).await;
                let _ = w.flush().await;
            }
            // Нотификация сервера — в tools-only подмножестве игнорируем
            // (list_changed — задел этапа 3: пересписок каталога).
            (None, true) => {}
            _ => {}
        }
    }
    // Поток закрыт: будим всех ожидающих ошибкой (oneshot закроется дропом).
    pending.lock().await.clear();
}

fn clip_line(s: &str) -> &str {
    &s[..s.len().min(200)]
}

/// MCP-клиент поверх подпроцесса: спавн + handshake + tools-методы + shutdown.
pub struct McpClient {
    conn: McpConnection,
    child: Child,
    /// Имя/версия сервера из `initialize` (диагностика).
    pub server_info: String,
    /// Версия протокола, которую подтвердил сервер.
    pub protocol_version: String,
}

impl McpClient {
    /// Запускает сервер и проводит handshake. `program`/`args` — команда сервера
    /// (на Windows `npx` и прочие `.cmd`-шимы запускать как `cmd /c npx …` — см.
    /// питфолл §4.6). stderr сервера дренируется в файловый лог.
    pub async fn spawn(program: &str, args: &[&str]) -> Result<Self> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // Без окна консоли (CREATE_NO_WINDOW); дочерний процесс не наследует
            // Ctrl+C-группу TUI.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("запуск MCP-сервера: {program}"))?;

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        // stderr — свободные логи сервера (spec transports); дренируем непрерывно,
        // иначе заполненный pipe заблокирует сервер посреди записи.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(line = %clip_line(&line), "MCP stderr");
                }
            });
        }

        let conn = McpConnection::over(stdout, stdin);
        let init = conn
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "mindfork-rs",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await?;
        // Встречную версию принимаем любую: tools-подмножество wire-стабильно
        // с 2024-11-05 (решение зонда; строгий отказ — вопрос этапа 3).
        let protocol_version = init
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let server_info = init
            .get("serverInfo")
            .map(|si| {
                format!(
                    "{} {}",
                    si.get("name").and_then(Value::as_str).unwrap_or("?"),
                    si.get("version").and_then(Value::as_str).unwrap_or("?")
                )
            })
            .unwrap_or_else(|| "?".into());
        conn.notify("notifications/initialized", json!({})).await?;

        Ok(Self {
            conn,
            child,
            server_info,
            protocol_version,
        })
    }

    /// Каталог инструментов сервера (`tools/list`, с пагинацией по `nextCursor`).
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self
                .conn
                .request("tools/list", params, HANDSHAKE_TIMEOUT)
                .await?;
            for t in page
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                tools.push(McpToolInfo {
                    name: t
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    description: t
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({ "type": "object" })),
                });
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    /// Вызов инструмента. Текстовые блоки содержимого склеиваются; не-текстовые
    /// (image/audio/resource) в зонде сводятся к пометке-плейсхолдеру.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<McpCallResult> {
        let result = self
            .conn
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                timeout,
            )
            .await?;
        let mut text = String::new();
        for block in result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                }
                Some(other) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[{other}-содержимое опущено]"));
                }
                None => {}
            }
        }
        Ok(McpCallResult {
            text,
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Штатное завершение: закрыть stdin (дропом writer) → подождать выход →
    /// kill (spec lifecycle §shutdown; на Windows SIGTERM нет — сразу kill).
    pub async fn shutdown(mut self) -> Result<()> {
        // Дроп соединения закрывает наш конец stdin-пайпа (writer владеет им).
        drop(self.conn);
        match tokio::time::timeout(SHUTDOWN_GRACE, self.child.wait()).await {
            Ok(status) => {
                status.context("ожидание выхода MCP-сервера")?;
                Ok(())
            }
            Err(_) => {
                self.child.kill().await.context("kill MCP-сервера")?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    /// Скриптованный фейк-сервер поверх duplex: на каждый запрос — замыкание.
    /// Возвращает соединение клиента + JoinHandle фейка (для ассертов внутри).
    fn fake_server<F>(script: F) -> (McpConnection, tokio::task::JoinHandle<Vec<Value>>)
    where
        F: Fn(&Value) -> Option<Value> + Send + 'static,
    {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_r, client_w) = tokio::io::split(client_io);
        let (server_r, mut server_w) = tokio::io::split(server_io);
        let handle = tokio::spawn(async move {
            let mut received = Vec::new();
            let mut lines = BufReader::new(server_r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                received.push(msg.clone());
                if let Some(reply) = script(&msg) {
                    let mut out = reply.to_string();
                    out.push('\n');
                    if server_w.write_all(out.as_bytes()).await.is_err() {
                        break;
                    }
                    let _ = server_w.flush().await;
                }
            }
            received
        });
        (McpConnection::over(client_r, client_w), handle)
    }

    /// Стандартный скрипт: initialize/tools/list/tools/call.
    fn scripted(msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        match msg["method"].as_str()? {
            "initialize" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake", "version": "0.1" }
            }})),
            "tools/list" => {
                // Две страницы: пагинация по nextCursor.
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                if params.get("cursor").is_none() {
                    Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                        "tools": [{ "name": "echo", "description": "Echo text",
                                    "inputSchema": { "type": "object" } }],
                        "nextCursor": "p2"
                    }}))
                } else {
                    Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                        "tools": [{ "name": "add", "description": "Add numbers",
                                    "inputSchema": { "type": "object" } }]
                    }}))
                }
            }
            "tools/call" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                "content": [ { "type": "text", "text": "hello" },
                             { "type": "image", "data": "…", "mimeType": "image/png" } ],
                "isError": false
            }})),
            _ => None,
        }
    }

    async fn handshake(conn: &McpConnection) -> Value {
        let init = conn
            .request(
                "initialize",
                json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                        "clientInfo": { "name": "t", "version": "0" } }),
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        conn.notify("notifications/initialized", json!({}))
            .await
            .unwrap();
        init
    }

    #[tokio::test]
    async fn handshake_lists_and_calls_over_duplex() {
        let (conn, _fake) = fake_server(scripted);
        let init = handshake(&conn).await;
        // Встречная версия сервера (иная, чем наша) принимается.
        assert_eq!(init["protocolVersion"], "2025-06-18");

        // Пагинация: две страницы склеены.
        let p1 = conn
            .request("tools/list", json!({}), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(p1["nextCursor"], "p2");
        let p2 = conn
            .request(
                "tools/list",
                json!({ "cursor": "p2" }),
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert!(p2.get("nextCursor").is_none());

        // Вызов: текст склеен, image → плейсхолдер добавит уровень клиента
        // (здесь — сырой результат).
        let call = conn
            .request(
                "tools/call",
                json!({ "name": "echo", "arguments": { "text": "hi" } }),
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(call["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn garbage_line_then_valid_response() {
        // Прямой duplex: сервер печатает баннер, затем валидный ответ.
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_r, client_w) = tokio::io::split(client_io);
        let (server_r, mut server_w) = tokio::io::split(server_io);
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_r).lines();
            let Ok(Some(line)) = lines.next_line().await else {
                return;
            };
            let msg: Value = serde_json::from_str(&line).unwrap();
            let id = msg["id"].clone();
            server_w
                .write_all(b"Starting fake MCP server v1.0!\n")
                .await
                .unwrap();
            let reply = json!({ "jsonrpc": "2.0", "id": id, "result": { "ok": true } });
            server_w
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        let conn = McpConnection::over(client_r, client_w);
        let r = conn
            .request("x", json!({}), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(r["ok"], true);
    }

    #[tokio::test]
    async fn answers_ping_and_rejects_unknown_server_requests() {
        // Сервер шлёт нам ping и sampling-запрос; проверяем ответы клиентской стороны.
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_r, client_w) = tokio::io::split(client_io);
        let (server_r, mut server_w) = tokio::io::split(server_io);
        let _conn = McpConnection::over(client_r, client_w);

        let ping = json!({ "jsonrpc": "2.0", "id": 100, "method": "ping" });
        let sampling = json!({ "jsonrpc": "2.0", "id": 101, "method": "sampling/createMessage", "params": {} });
        server_w
            .write_all(format!("{ping}\n{sampling}\n").as_bytes())
            .await
            .unwrap();

        let mut lines = BufReader::new(server_r).lines();
        let pong: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(pong["id"], 100);
        assert!(pong.get("result").is_some(), "ping → пустой результат");
        let mnf: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(mnf["id"], 101);
        assert_eq!(mnf["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn timeout_sends_cancelled_notification() {
        // Фейк молчит на tools/call → клиент таймаутится и шлёт cancelled.
        let (conn, fake) = fake_server(|msg| {
            let id = msg.get("id")?.clone();
            (msg["method"].as_str()? != "tools/call")
                .then(|| json!({ "jsonrpc": "2.0", "id": id, "result": {} }))
        });
        let err = conn
            .request(
                "tools/call",
                json!({ "name": "slow" }),
                Duration::from_millis(100),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("таймаут"), "{err}");
        drop(conn); // закрыть поток → фейк вернёт полученное
        let received = fake.await.unwrap();
        assert!(
            received
                .iter()
                .any(|m| m["method"] == "notifications/cancelled"
                    && m["params"]["reason"] == "timeout"),
            "нет notifications/cancelled: {received:?}"
        );
    }

    #[tokio::test]
    async fn rpc_error_surfaces_as_error() {
        let (conn, _fake) = fake_server(|msg| {
            let id = msg.get("id")?.clone();
            Some(json!({ "jsonrpc": "2.0", "id": id,
                         "error": { "code": -32602, "message": "Unknown tool" } }))
        });
        let err = conn
            .request(
                "tools/call",
                json!({ "name": "nope" }),
                Duration::from_secs(2),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown tool"), "{err}");
    }
}

/// Живой смоук зонда (go/no-go, docs/research/plugin-system.md §7 этап 2):
/// реальный сторонний MCP-сервер (`npx @modelcontextprotocol/server-filesystem`)
/// плюс живая модель (`MINDFORK_ENGINE_URL`). Ручной мини-agentic-loop повторяет
/// механику оркестратора (стрим → tool_calls → исполнение → следующий раунд);
/// требует `npx` в PATH (Windows: запускается как `cmd /c npx …` — питфолл §4.6).
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::api::OpenAiClient;
    use crate::shared::api::contract::{
        ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason, ToolCallAccumulator,
        ToolSchema,
    };
    use futures_util::StreamExt;

    async fn spawn_filesystem_server(allowed_dir: &str) -> Result<McpClient> {
        // npx на Windows — .cmd-шим: без шелла спавн даёт ENOENT (§4.6).
        if cfg!(windows) {
            McpClient::spawn(
                "cmd",
                &[
                    "/c",
                    "npx",
                    "-y",
                    "@modelcontextprotocol/server-filesystem",
                    allowed_dir,
                ],
            )
            .await
        } else {
            McpClient::spawn(
                "npx",
                &["-y", "@modelcontextprotocol/server-filesystem", allowed_dir],
            )
            .await
        }
    }

    /// Критерий GO: модель сама вызывает MCP-инструмент чтения файла и использует
    /// его результат в ответе; вызовы/результаты ходят через наш клиент; ход
    /// завершается штатно, сервер гасится shutdown-лестницей.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL + npx (real filesystem MCP server)"]
    async fn gemma_reads_file_via_mcp_filesystem_server() {
        let Some(url) = std::env::var("MINDFORK_ENGINE_URL").ok() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let engine = OpenAiClient::new(url);

        // Секрет в файле внутри разрешённого каталога.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("secret_number.txt"),
            "Секретное число: 7319",
        )
        .unwrap();
        let allowed = dir.path().to_string_lossy().replace('\\', "/");

        let client = match spawn_filesystem_server(&allowed).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: не удалось запустить npx MCP-сервер: {e:#}");
                return;
            }
        };
        eprintln!(
            "MCP server: {} (protocol {})",
            client.server_info, client.protocol_version
        );

        // Каталог инструментов → OpenAI-схемы (как сделает этап 3).
        let tools = client.list_tools().await.unwrap();
        assert!(!tools.is_empty(), "у filesystem-сервера есть инструменты");
        let schemas: Vec<ToolSchema> = tools
            .iter()
            .map(|t| ToolSchema {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            })
            .collect();
        let schema_bytes: usize = schemas
            .iter()
            .map(|s| s.name.len() + s.description.len() + s.parameters.to_string().len())
            .sum();
        eprintln!(
            "инструментов: {}, схемы ≈ {} КиБ (бюджет 16k-контекста)",
            schemas.len(),
            schema_bytes / 1024
        );

        // Ручной agentic-loop (механика orchestrator/generation.rs, вручную).
        let mut messages = vec![ApiMessage::user(format!(
            "Прочитай файл {allowed}/secret_number.txt с помощью инструмента и скажи, \
             какое секретное число в нём записано."
        ))];
        let mut rounds = 0;
        let final_text = loop {
            rounds += 1;
            assert!(rounds <= 8, "модель зациклилась на вызовах инструментов");
            let req = ChatRequest {
                system: Some(
                    "Ты — ассистент с инструментами файловой системы. Пользуйся ими.".into(),
                ),
                messages: messages.clone(),
                sampling: SamplingConfig {
                    max_tokens: Some(1024),
                    ..Default::default()
                },
                tools: schemas.clone(),
            };
            let mut stream = engine.chat_stream(req, Default::default()).await.unwrap();
            let mut text = String::new();
            let mut acc = ToolCallAccumulator::default();
            let mut finish = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    ChatChunk::ToolCall(d) => acc.push(d),
                    ChatChunk::Finished(r) => {
                        finish = Some(r);
                        break;
                    }
                    _ => {}
                }
            }
            let calls = acc.finish();
            match finish {
                Some(FinishReason::ToolCalls) if !calls.is_empty() => {
                    messages.push(ApiMessage::assistant_tool_calls(text, calls.clone()));
                    for call in calls {
                        let args: Value =
                            serde_json::from_str(&call.arguments).unwrap_or(json!({}));
                        eprintln!("→ модель вызывает {}({})", call.name, call.arguments);
                        let result = client
                            .call_tool(&call.name, args, Duration::from_secs(30))
                            .await
                            .unwrap();
                        eprintln!(
                            "← результат ({}): {}",
                            if result.is_error {
                                "ошибка"
                            } else {
                                "ок"
                            },
                            &result.text[..result.text.len().min(120)]
                        );
                        messages.push(ApiMessage::tool(call.id, result.text));
                    }
                }
                _ => break text,
            }
        };
        eprintln!("раундов: {rounds}; финальный ответ: {final_text}");
        assert!(
            final_text.contains("7319"),
            "модель не использовала результат MCP-инструмента: {final_text:?}"
        );
        assert!(rounds >= 2, "инструмент не вызывался (ответ без раундов)");
        client.shutdown().await.unwrap();
    }
}
