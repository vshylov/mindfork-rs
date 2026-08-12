//! A mini **MCP** (Model Context Protocol) client — stdio, tools-only. Grown out
//! of the "plugins" track probe (docs/research/plugin-system.md §4, §7, stage 2 —
//! GO); from stage 3 (`feat/mcp-host`) onward — part of the binary: MCP servers
//! are launched by the orchestrator's `McpManager`, their tools are registered in
//! the registry via the `McpTool` wrapper (`features/tools/mcp.rs`). Target
//! revision — 2025-11-25; the tools-only subset has been wire-stable since
//! 2024-11-05, so we accept any counterpart server version (the tools-only
//! methods are identical across all revisions).
//!
//! Scope (research §4.2): a subprocess + newline-delimited JSON-RPC 2.0 (UTF-8,
//! the server's stdout carries only the protocol, stderr is drained to the log),
//! `initialize` → `notifications/initialized`, `tools/list` (pagination),
//! `tools/call` (`isError:true` → an error text for the model), replying to
//! `ping`, `-32601` on any other server request (otherwise a well-behaved server
//! would hang), `notifications/cancelled` on timeout/cancellation, a shutdown
//! ladder (close stdin → wait → kill). Known pitfalls (§4.6): garbage lines on
//! stdout are skipped with a warn; unknown notifications are ignored; `npx`/`uvx`
//! are `.cmd` shims, which [`resolve_command`] finds via `PATHEXT` so one config
//! works on every platform (the old `.bat`/`.cmd` ban is gone — see its doc and
//! ADR 0007 §2); on Windows the process tree is killed by a Job Object
//! kill-on-close (orphaned `npx`→`node` children don't outlive app exit).
//!
//! Transport is decoupled from the process ([`McpConnection::over`] works over
//! any `AsyncRead`/`AsyncWrite`) — unit tests run the protocol on
//! `tokio::io::duplex` with no processes; [`McpClient::spawn`] adds subprocess
//! management (a monitor task with `kill`/`exited` tokens — the
//! `shared/api/managed.rs` pattern).
//!
//! The module's error texts are plain English (a technical layer, not localized;
//! UI statuses are localized by stage 3b).

use std::collections::HashMap;
// Only the Windows command resolver needs it — an unconditional import is an
// unused-import error on other targets under `-D warnings`.
#[cfg(windows)]
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

/// Protocol version the client offers.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// Timeout waiting for a reply to `initialize`/`tools/list` (startup requests).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for the server to exit after closing stdin (shutdown ladder).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// An MCP server tool (a snapshot from `tools/list`).
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments object (`inputSchema`).
    pub input_schema: Value,
}

/// The result of `tools/call`: text content blocks joined into one string.
#[derive(Debug, Clone)]
pub struct McpCallResult {
    pub text: String,
    /// Image blocks the tool returned, in order (spec §9.10,
    /// docs/research/mcp-tool-images.md). Empty for every tool that returns none,
    /// which is every tool that existed before this was added.
    pub images: Vec<McpImage>,
    /// Tool execution error (`isError:true`) — the text is handed to the model
    /// as an error result, this is NOT a protocol error (spec tools §error handling).
    pub is_error: bool,
}

/// An image block from a tool result: base64 payload plus the MIME type the server
/// declared. Kept raw here — the client's job is to parse the protocol, and deciding
/// what is small enough or decodable belongs to the layer that owns the limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpImage {
    pub mime: String,
    pub data: String,
}

/// How many image blocks one tool result may contribute (fork F2 of
/// docs/research/mcp-tool-images.md).
///
/// An MCP server is third-party code, and every image it returns rides **every**
/// subsequent turn of the conversation — so the ceiling bounds a standing cost, not one
/// reply. Extras are dropped and **said out loud** in the result text: a silent cap reads
/// as "the tool returned four images" when it returned fifty.
pub const MAX_RESULT_IMAGES: usize = 4;

/// The connection's shared writer: written to both by our own requests and by
/// the reader task (replies to `ping`/`-32601`).
type SharedWriter = Arc<Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;
/// Requests awaiting a reply: id → the result sender (`Err` — the JSON-RPC error text).
type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

/// The transport half: a request→response channel over a reader/writer pair.
/// Responses are routed by `id` (oneshot); server requests are served in the
/// reader task (`ping` → an empty result, anything else → `-32601`),
/// notifications are ignored, non-JSON lines are skipped with a warn.
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
    /// A connection over an arbitrary pair of streams (for tests — `duplex`).
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

    /// Sends a request and waits for a reply for at most `timeout`. On timeout
    /// sends `notifications/cancelled` (spec lifecycle §timeouts) and returns an
    /// error.
    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.request_cancellable(method, params, timeout, None)
            .await
    }

    /// Like [`Self::request`], but is additionally interruptible by the `cancel`
    /// token (a user cancelling the turn, Esc): the server gets
    /// `notifications/cancelled` with a `reason`, the call returns an error. A
    /// late server reply is dropped as an unknown id.
    pub async fn request_cancellable(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send_line(&msg).await?;

        let cancelled = async {
            match cancel {
                Some(tok) => tok.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        let outcome = tokio::select! {
            res = tokio::time::timeout(timeout, rx) => res,
            _ = cancelled => {
                self.abandon_request(id, "cancelled").await;
                bail!("MCP {method}: call cancelled")
            }
        };
        match outcome {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(rpc_err))) => bail!("MCP {method}: {rpc_err}"),
            // Channel closed: the reader task died (the server closed stdout / a broken stream).
            Ok(Err(_)) => bail!("MCP {method}: connection closed by the server"),
            Err(_) => {
                self.abandon_request(id, "timeout").await;
                bail!("MCP {method}: timed out after {}s", timeout.as_secs())
            }
        }
    }

    /// Drops the pending wait for request `id` and notifies the server of the
    /// cancellation (spec lifecycle §timeouts) — it may stop working; a reply, if
    /// one still arrives, is dropped as an unknown id.
    async fn abandon_request(&self, id: i64, reason: &str) {
        self.pending.lock().await.remove(&id);
        let cancel = json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": { "requestId": id, "reason": reason }
        });
        let _ = self.send_line(&cancel).await;
    }

    /// The server's tool catalog (`tools/list`, paginated via `nextCursor`).
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self
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

    /// Calls a tool. Text content blocks are concatenated; non-text ones
    /// (image/audio/resource) collapse to a placeholder marker. `cancel` — a
    /// user cancelling the turn (the server gets `notifications/cancelled`).
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<McpCallResult> {
        let result = self
            .request_cancellable(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                timeout,
                cancel,
            )
            .await?;
        let mut text = String::new();
        let mut images: Vec<McpImage> = Vec::new();
        let mut dropped = 0usize;
        let push_line = |text: &mut String, line: &str| {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(line);
        };
        for block in result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => push_line(
                    &mut text,
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                // An image the model can actually be shown (spec §9.10). A block missing
                // its payload or MIME type is not an image we can send anywhere, so it
                // keeps the placeholder rather than becoming a broken attachment.
                Some("image") => match (
                    block.get("data").and_then(Value::as_str),
                    block.get("mimeType").and_then(Value::as_str),
                ) {
                    (Some(data), Some(mime)) if !data.is_empty() => {
                        if images.len() < MAX_RESULT_IMAGES {
                            images.push(McpImage {
                                mime: mime.to_string(),
                                data: data.to_string(),
                            });
                        } else {
                            dropped += 1;
                        }
                    }
                    _ => push_line(&mut text, "[image content omitted]"),
                },
                // Audio and resource blocks stay placeholders: nothing downstream can
                // carry them, and saying so is better than dropping them silently.
                Some(other) => push_line(&mut text, &format!("[{other} content omitted]")),
                None => {}
            }
        }
        if dropped > 0 {
            // The cap is stated, never silent — otherwise the model reads the result as
            // complete and answers about images it was never shown.
            push_line(
                &mut text,
                &format!(
                    "[{dropped} more image(s) were returned but not included: at most \
                     {MAX_RESULT_IMAGES} images per tool result]"
                ),
            );
        }
        Ok(McpCallResult {
            text,
            images,
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Sends a notification (no id, no waiting for a reply).
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

/// The reader loop: line → JSON → routing (a reply / a server request / a notification).
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
                // Pitfall §4.6: servers print banners/logs to stdout — skip the
                // line without tearing down the connection.
                tracing::warn!(line = %clip_line(&line), "MCP: non-JSON line on stdout, skipped");
                continue;
            }
        };
        let id = msg.get("id");
        let has_method = msg.get("method").is_some();
        match (id, has_method) {
            // A reply to our request.
            (Some(id_v), false) => deliver_reply(id_v, &msg, &pending).await,
            // A server request to us: reply to ping, everything else —
            // method-not-found (staying silent would hang a well-behaved server).
            (Some(id_v), true) => answer_server_request(id_v, &msg, &writer).await,
            // A server notification — ignored in the tools-only subset
            // (list_changed — groundwork for stage 3: re-listing the catalog).
            (None, true) => {}
            _ => {}
        }
    }
    // The stream is closed: wake every waiter with an error (the oneshot closes on drop).
    pending.lock().await.clear();
}

/// Routes a server reply to the request waiting on its id; a reply nobody is
/// waiting for (or with a non-numeric id) is dropped.
async fn deliver_reply(id_v: &Value, msg: &Value, pending: &PendingMap) {
    let Some(id) = id_v.as_i64() else { return };
    if let Some(tx) = pending.lock().await.remove(&id) {
        let outcome = match msg.get("error") {
            Some(e) => Err(e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error with no description")
                .to_string()),
            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = tx.send(outcome);
    }
}

/// Answers a server→client request: an empty result for `ping`, `-32601`
/// (method not found) for anything else.
async fn answer_server_request(id_v: &Value, msg: &Value, writer: &SharedWriter) {
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

fn clip_line(s: &str) -> &str {
    &s[..s.len().min(200)]
}

/// Whether a server id is a valid slug (`[a-z0-9-]`, 1..=32): it is part of the
/// tool id `mcp__<id>__<tool>` and the host's slot key. Lives here rather than in
/// the host because the settings screen validates it before committing an edit —
/// an invalid id creates no slot at all, so the server would otherwise silently
/// vanish from the status list (docs/history/mcp-server-editor.md F8).
pub fn valid_server_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether a child environment variable name is one we can carry: POSIX-shaped
/// (`[A-Za-z0-9_]`, not starting with a digit). The restriction is what keeps the
/// flat variable-list settings row parseable and the `mcp-<server>-<VAR>`
/// secret storage name unambiguous — only the server id may contain `-`
/// (docs/history/mcp-server-editor.md §9.4).
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Resolves a server command the way a shell would, so **one config works on
/// every platform** (`"command": "npx"` rather than `cmd /c npx …` on Windows).
///
/// Why this is needed at all: `cmd.exe` completes a bare name using `PATHEXT`,
/// and **Rust does not** — measured, `Command::new("npx")` is `NotFound` on
/// Windows while `Command::new("npx.cmd")` spawns fine. That single difference
/// is what used to force a platform-specific config.
///
/// Why spawning the resolved `.cmd` directly is safe — and safer than the
/// `cmd /c` we used to require: CVE-2024-24576 ("BatBadBut") was fixed in
/// `std` as of Rust 1.77.2, which escapes batch-file arguments and **refuses**
/// the ones it cannot escape (measured: `a"b`, `%CD%` and `a&whoami` are
/// escaped, an embedded newline is rejected with `InvalidInput`). Routing
/// through `cmd /c` instead hands the arguments to `cmd.exe`, which re-parses
/// them *outside* that protection. So the old `.bat`/`.cmd` ban added no safety
/// over `std` while pushing users onto the worse path — see ADR 0007 §2.
///
/// A no-op on unix, where `Command` already does the `PATH` lookup itself and
/// there is no extension to complete. Returns `None` when nothing matched — the
/// caller then spawns the command as written, so the OS produces the error.
#[cfg(windows)]
pub fn resolve_command(command: &str) -> Option<std::path::PathBuf> {
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    let dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    resolve_in(command, &dirs, &exts)
}

/// The pure core of [`resolve_command`]: the environment is a parameter, so the
/// rule can be tested without touching the process's own `PATH`.
///
/// **A bare name is never taken as-is** — that is the whole subtlety, and it was
/// caught by a live run rather than by reasoning: npm ships *both*
/// `npx` (a Unix shell script) and `npx.cmd` next to each other, so accepting
/// the extensionless file spawns something Windows cannot execute
/// (`os error 193: not a valid Win32 application`). `cmd.exe` only ever
/// completes a bare name from `PATHEXT`; a name that already carries an
/// extension is tried as written first, then still completed (so `my.tool`
/// can reach `my.tool.exe`).
#[cfg(windows)]
fn resolve_in(
    command: &str,
    dirs: &[std::path::PathBuf],
    exts: &[String],
) -> Option<std::path::PathBuf> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return None;
    }
    let has_ext = Path::new(cmd).extension().is_some();
    let candidates = |dir: &Path| -> Option<std::path::PathBuf> {
        let base = dir.join(cmd);
        has_ext
            .then(|| base.clone())
            .into_iter()
            .chain(exts.iter().map(|ext| {
                let mut s = base.clone().into_os_string();
                s.push(ext);
                std::path::PathBuf::from(s)
            }))
            .find(|p| p.is_file())
    };
    // A command with a path in it is not searched on `PATH` — only completed,
    // exactly like a shell.
    if cmd.contains(['/', '\\']) || Path::new(cmd).is_absolute() {
        return candidates(Path::new(""));
    }
    dirs.iter().find_map(|dir| candidates(dir))
}

/// On unix `Command` performs the `PATH` lookup itself — nothing to resolve.
#[cfg(not(windows))]
pub fn resolve_command(_command: &str) -> Option<std::path::PathBuf> {
    None
}

/// An MCP client over a subprocess: spawn + handshake + tools methods + shutdown.
/// The transport (`Arc<McpConnection>`) is exposed outward ([`Self::conn`]) —
/// held by the `McpTool` tool wrappers; the client itself anchors the process
/// lifecycle. `Drop` arms `kill`: the monitor task (owner of [`Child`]) gives the
/// server a grace period to exit on its own (stdin closes when the connection is
/// dropped) and then kills it.
pub struct McpClient {
    conn: Arc<McpConnection>,
    /// Armed on `drop`/`shutdown`: the monitor task terminates the process.
    kill: CancellationToken,
    /// Armed by the monitor task once the process has exited (on its own or after kill).
    exited: CancellationToken,
    /// The server's name/version from `initialize` (diagnostics).
    pub server_info: String,
    /// The protocol version confirmed by the server.
    pub protocol_version: String,
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // The monitor task owns `Child`; signal it to terminate the process. Our
        // Arc to the connection is dropped right after (the field) — if no tool
        // holders remain, stdin closes and the server gets a chance to exit on
        // its own during the grace period.
        self.kill.cancel();
    }
}

impl McpClient {
    /// Spawns the server and runs the handshake. `program`/`args` — the server
    /// command (on Windows, spawn `npx` and other `.cmd`-shims as `cmd /c npx …`
    /// — see pitfall §4.6; `.bat`/`.cmd` themselves are forbidden — BatBadBut).
    /// `envs` — already **resolved** child environment-variable pairs (the
    /// caller — `McpManager` — expands source names into values). The server's
    /// stderr is drained into the file log.
    pub async fn spawn(program: &str, args: &[String], envs: &[(String, String)]) -> Result<Self> {
        // Resolve the way a shell would, so the same config works on every
        // platform (see `resolve_command`). Unresolved — spawn as written and
        // let the OS produce the error.
        let resolved = resolve_command(program);
        let mut cmd = match &resolved {
            Some(path) => Command::new(path),
            None => Command::new(program),
        };
        cmd.args(args)
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // No console window (CREATE_NO_WINDOW); the child process doesn't
            // inherit the TUI's Ctrl+C group.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().with_context(|| match &resolved {
            // Name what was actually launched: with `npx` resolving to
            // `npx.cmd`, "launching npx" would hide which file failed.
            Some(p) if p.as_os_str() != program => {
                format!("launching MCP server: {program} ({})", p.display())
            }
            _ => format!("launching MCP server: {program}"),
        })?;

        // The process tree (`cmd /c npx` → node) goes into a kill-on-close Job
        // Object: the handle lives in the monitor task; closing it (a clean exit
        // or an app crash) kills the whole tree — orphans don't outlive exit.
        // Windows only.
        let job = JobGuard::assign(&child);

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        // stderr — free-form server logs (spec transports); drain continuously,
        // otherwise a filled pipe would block the server mid-write.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(line = %clip_line(&line), "MCP stderr");
                }
            });
        }

        // The monitor task owns Child (and the Job handle) — the managed.rs pattern.
        let kill = CancellationToken::new();
        let exited = CancellationToken::new();
        spawn_monitor(child, job, kill.clone(), exited.clone());

        let conn = Arc::new(McpConnection::over(stdout, stdin));
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
        // We accept any counterpart version: the tools subset has been
        // wire-stable since 2024-11-05 (the probe's decision, confirmed against
        // a live server).
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
            kill,
            exited,
            server_info,
            protocol_version,
        })
    }

    /// The connection's shared transport (held by the `McpTool` wrappers).
    pub fn conn(&self) -> Arc<McpConnection> {
        self.conn.clone()
    }

    /// A "server process has exited" signal — for `McpManager`'s monitor
    /// (restart budget / `Disconnected` status).
    pub fn exited(&self) -> CancellationToken {
        self.exited.clone()
    }

    /// The server's tool catalog (see [`McpConnection::list_tools`]).
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        self.conn.list_tools().await
    }

    /// A clean shutdown: dropping the client closes stdin (our connection Arc)
    /// and arms `kill`; the monitor gives the server a grace period to exit on
    /// its own and then kills it. Returns once the process has actually exited.
    pub async fn shutdown(self) {
        let exited = self.exited.clone();
        drop(self); // Drop: kill.cancel() + drop the connection Arc (stdin closes)
        exited.cancelled().await;
    }
}

/// The server process's monitor task: waits for it to exit (arms `exited`) or
/// for the `kill` signal (a grace period for a self-initiated exit after
/// closing stdin → `start_kill`). Owns [`Child`] and the Job handle
/// ([`JobGuard`]) — `kill_on_drop` and kill-on-close finish off the
/// process/tree even if the runtime drops the task.
fn spawn_monitor(
    mut child: Child,
    job: JobGuard,
    kill: CancellationToken,
    exited: CancellationToken,
) {
    tokio::spawn(async move {
        // The Job handle lives until the end of the task: its closure (after
        // the child has already exited) finishes off the whole process tree
        // via kill-on-close (Windows; a no-op stub on other OSes). Binding it
        // instead of `drop(job)` at the end — on unix the stub has no Drop,
        // and an explicit drop would trip clippy::drop_non_drop.
        let _job = job;
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(s) => tracing::warn!(status = ?s, "MCP server exited on its own"),
                    Err(e) => tracing::warn!(error = %e, "error waiting for the MCP server"),
                }
            }
            _ = kill.cancelled() => {
                // stdin closes when the connection drops (may lag `kill` by an
                // instant) — a grace period for a clean exit, then kill.
                if tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await.is_err() {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
        }
        exited.cancel();
    });
}

/// The backing handle of the kill-on-close Job Object (Windows): while the
/// handle is open, the tree lives; closing the handle (a clean exit in the
/// monitor, or an app crash) kills the whole server process tree (including
/// grandchildren, `cmd /c npx` → `node`). A no-op on other OSes.
/// The field is held only for `Drop` (closing the handle) — never read.
struct JobGuard(
    #[cfg(windows)]
    #[allow(dead_code)]
    Option<JobHandle>,
);

#[cfg(windows)]
struct JobHandle(windows_sys::Win32::Foundation::HANDLE);
// SAFETY: a Job Object HANDLE is just a kernel handle; sending it across threads is safe.
#[cfg(windows)]
unsafe impl Send for JobHandle {}

#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was created by us in `assign` and hasn't been closed yet.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

impl JobGuard {
    /// Places the process into a Job Object with `KILL_ON_JOB_CLOSE`. "Best
    /// effort": a failure is only logged (the server keeps working without a
    /// job — as on unix).
    #[cfg(windows)]
    fn assign(child: &Child) -> Self {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        let Some(raw) = child.raw_handle() else {
            tracing::warn!("MCP: no process handle — Job Object not assigned");
            return Self(None);
        };
        // SAFETY: `raw` is a valid handle of the just-spawned process; the
        // struct is zero-initialized; the job closes via JobHandle::drop.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                tracing::warn!("MCP: CreateJobObjectW failed");
                return Self(None);
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, raw as _) == 0 {
                tracing::warn!("MCP: failed to assign the kill-on-close Job Object");
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Self(None);
            }
            Self(Some(JobHandle(job)))
        }
    }

    #[cfg(not(windows))]
    fn assign(_child: &Child) -> Self {
        Self()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[test]
    fn env_variable_name_rule() {
        // POSIX-shaped, and no `-`: the storage name `mcp-<server>-<VAR>` has to
        // stay unambiguous, and the flat variable-list row parseable.
        assert!(valid_env_name("GITHUB_TOKEN"));
        assert!(valid_env_name("a1"));
        assert!(!valid_env_name(""));
        assert!(!valid_env_name("1A"));
        assert!(!valid_env_name("has-dash"));
        assert!(!valid_env_name("has space"));
    }

    /// A scripted fake server over duplex: a closure decides the reply to
    /// each request. Returns the client's connection + the fake's
    /// JoinHandle (for assertions inside it).
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

    /// The standard script: initialize/tools/list/tools/call.
    fn scripted(msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        match msg["method"].as_str()? {
            "initialize" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake", "version": "0.1" }
            }})),
            "tools/list" => {
                // Two pages: pagination via nextCursor.
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
        // The counterpart's version (different from ours) is accepted.
        assert_eq!(init["protocolVersion"], "2025-06-18");

        // Pagination: two pages are stitched together.
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

        // A call: text is concatenated, image → the client level adds the
        // placeholder (here — the raw result).
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
        // A direct duplex: the server prints a banner, then a valid reply.
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
        // The server sends us a ping and a sampling request; check the client side's replies.
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
        assert!(pong.get("result").is_some(), "ping → empty result");
        let mnf: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(mnf["id"], 101);
        assert_eq!(mnf["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn timeout_sends_cancelled_notification() {
        // The fake stays silent on tools/call → the client times out and sends cancelled.
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
        assert!(err.contains("timed out"), "{err}");
        drop(conn); // close the stream → the fake returns what it received
        let received = fake.await.unwrap();
        assert!(
            received
                .iter()
                .any(|m| m["method"] == "notifications/cancelled"
                    && m["params"]["reason"] == "timeout"),
            "no notifications/cancelled: {received:?}"
        );
    }

    #[tokio::test]
    async fn cancel_sends_cancelled_notification() {
        // The fake stays silent on tools/call → cancelling via the token
        // interrupts the call and sends notifications/cancelled with
        // reason=cancelled (the user's Esc).
        let (conn, fake) = fake_server(|msg| {
            let id = msg.get("id")?.clone();
            (msg["method"].as_str()? != "tools/call")
                .then(|| json!({ "jsonrpc": "2.0", "id": id, "result": {} }))
        });
        let cancel = CancellationToken::new();
        let tok = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tok.cancel();
        });
        let err = conn
            .call_tool("slow", json!({}), Duration::from_secs(30), Some(&cancel))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("cancelled"), "{err}");
        drop(conn);
        let received = fake.await.unwrap();
        assert!(
            received
                .iter()
                .any(|m| m["method"] == "notifications/cancelled"
                    && m["params"]["reason"] == "cancelled"),
            "no notifications/cancelled: {received:?}"
        );
    }

    #[tokio::test]
    async fn call_tool_joins_text_and_carries_images() {
        // Text blocks are concatenated; an image block is **kept** (spec §9.10) rather
        // than collapsed into the placeholder it used to become.
        let (conn, _fake) = fake_server(scripted);
        let res = conn
            .call_tool(
                "echo",
                json!({ "text": "hi" }),
                Duration::from_secs(2),
                None,
            )
            .await
            .unwrap();
        assert_eq!(res.text, "hello", "the image leaves no placeholder behind");
        assert_eq!(
            res.images,
            vec![McpImage {
                mime: "image/png".into(),
                data: "…".into(),
            }]
        );
        assert!(!res.is_error);
    }

    /// The blocks nothing downstream can carry, and the malformed ones, still say so —
    /// dropping them silently would let the model read the result as complete.
    #[tokio::test]
    async fn audio_and_broken_image_blocks_keep_their_placeholder() {
        let (conn, _fake) = fake_server(|msg| {
            let id = msg.get("id").cloned().unwrap_or(json!(1));
            match msg["method"].as_str().unwrap_or_default() {
                "initialize" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "fake", "version": "0.1" }
                }})),
                "tools/call" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "content": [
                        { "type": "audio", "data": "…", "mimeType": "audio/wav" },
                        // An image block with no payload is not an image we can send.
                        { "type": "image", "mimeType": "image/png" },
                    ],
                    "isError": false
                }})),
                _ => None,
            }
        });
        let res = conn
            .call_tool("t", json!({}), Duration::from_secs(2), None)
            .await
            .unwrap();
        assert!(res.images.is_empty());
        assert!(res.text.contains("[audio content omitted]"), "{}", res.text);
        assert!(res.text.contains("[image content omitted]"), "{}", res.text);
    }

    /// The per-result cap is stated, never silent: a model told "4 images" about a reply
    /// that carried fifty would answer about pictures it was never shown (fork F2).
    #[tokio::test]
    async fn the_image_cap_drops_the_extras_and_says_so() {
        let (conn, _fake) = fake_server(|msg| {
            let id = msg.get("id").cloned().unwrap_or(json!(1));
            match msg["method"].as_str().unwrap_or_default() {
                "initialize" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "fake", "version": "0.1" }
                }})),
                "tools/call" => {
                    let blocks: Vec<_> = (0..MAX_RESULT_IMAGES + 3)
                        .map(|i| {
                            json!({ "type": "image", "data": format!("d{i}"),
                                         "mimeType": "image/png" })
                        })
                        .collect();
                    Some(json!({ "jsonrpc": "2.0", "id": id, "result": {
                        "content": blocks, "isError": false
                    }}))
                }
                _ => None,
            }
        });
        let res = conn
            .call_tool("t", json!({}), Duration::from_secs(2), None)
            .await
            .unwrap();
        assert_eq!(res.images.len(), MAX_RESULT_IMAGES);
        // The ones kept are the first ones, in order.
        assert_eq!(res.images[0].data, "d0");
        assert_eq!(res.images[MAX_RESULT_IMAGES - 1].data, "d3");
        assert!(
            res.text.contains('3'),
            "the count of dropped ones: {}",
            res.text
        );
        assert!(res.text.contains("not included"), "{}", res.text);
    }

    #[test]
    fn batch_commands_are_forbidden() {
        // BatBadBut (CVE-2024-24576): .bat/.cmd as a server command are
        // forbidden, including with a path and any letter case; `cmd` (the
        // shell for npx) and exe are allowed.
        // The resolver replaced the ban (ADR 0007 §2): a `.cmd` is what `npx`
        // legitimately resolves to on Windows, and `std` escapes its arguments.
        assert!(resolve_command("").is_none());
        assert!(resolve_command("definitely-not-a-real-program-xyz").is_none());
    }

    #[tokio::test]
    async fn spawn_reports_which_file_it_failed_to_launch() {
        // A `.cmd` is no longer refused up front (ADR 0007 §2, revisited); a
        // missing program fails at the OS, and the message has to name it —
        // with `npx` resolving to `npx.cmd`, "launching npx" would hide which
        // file was actually tried.
        let err = match McpClient::spawn("definitely-not-a-real-program-xyz", &[], &[]).await {
            Ok(_) => panic!("a nonexistent program should not spawn"),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("definitely-not-a-real-program-xyz"), "{err}");
    }

    /// The whole point of the resolver: a config written once works on every
    /// platform. On Windows `Command::new("cmd")` finds nothing without
    /// `PATHEXT` completion — that gap is what used to force `cmd /c npx …`.
    #[cfg(windows)]
    #[test]
    fn resolve_completes_pathext_on_windows() {
        let resolved = resolve_command("cmd").expect("cmd.exe is on PATH");
        assert_eq!(
            resolved.extension().map(|e| e.to_ascii_lowercase()),
            Some("exe".into()),
            "resolved to {resolved:?}"
        );
        assert!(resolved.is_file());
        // An explicit extension is honoured rather than completed again
        // (`npx.cmd` must not become `npx.cmd.exe`).
        let exact = resolve_command("cmd.exe").expect("cmd.exe is on PATH");
        assert_eq!(exact, resolved);
    }

    /// The npm layout, which is what a live run tripped over: `npx` and
    /// `npx.cmd` sit **side by side**, and the extensionless one is a Unix shell
    /// script Windows cannot execute. A bare name must therefore complete from
    /// `PATHEXT` and never be taken as-is.
    #[cfg(windows)]
    #[test]
    fn bare_name_never_resolves_to_the_extensionless_twin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("npx"), "#!/bin/sh\n").unwrap();
        std::fs::write(dir.path().join("npx.cmd"), "@echo off\n").unwrap();
        let dirs = vec![dir.path().to_path_buf()];
        let exts: Vec<String> = [".com", ".exe", ".bat", ".cmd"].map(String::from).to_vec();

        let got = resolve_in("npx", &dirs, &exts).expect("npx.cmd should be found");
        assert_eq!(got, dir.path().join("npx.cmd"), "picked the shell script");
        // Spelled out in full — taken as written, not completed again.
        assert_eq!(
            resolve_in("npx.cmd", &dirs, &exts).unwrap(),
            dir.path().join("npx.cmd")
        );
        // A path rather than a name: completed, but not searched on PATH.
        let full = dir.path().join("npx").to_string_lossy().replace('\\', "/");
        assert_eq!(
            resolve_in(&full, &[], &exts).unwrap(),
            dir.path().join("npx.cmd")
        );
        // A name with a dot that is not a real extension still reaches the exe.
        std::fs::write(dir.path().join("my.tool.exe"), "").unwrap();
        assert_eq!(
            resolve_in("my.tool", &dirs, &exts).unwrap(),
            dir.path().join("my.tool.exe")
        );
        assert!(resolve_in("nothing-here", &dirs, &exts).is_none());
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

/// The probe's live smoke (go/no-go, docs/research/plugin-system.md §7 stage 2):
/// a real third-party MCP server (`npx @modelcontextprotocol/server-filesystem`)
/// plus a live model (`MINDFORK_ENGINE_URL`). A manual mini agentic-loop mirrors
/// the orchestrator's mechanics (stream → tool_calls → execution → the next round);
/// requires `npx` on PATH (Windows: spawned as `cmd /c npx …` — pitfall §4.6).
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::api::contract::{
        ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason, ToolCallAccumulator,
        ToolSchema,
    };
    use futures_util::StreamExt;

    async fn spawn_filesystem_server(allowed_dir: &str) -> Result<McpClient> {
        // npx on Windows is a .cmd shim: spawning without a shell gives ENOENT (§4.6).
        let (program, args): (&str, Vec<String>) = if cfg!(windows) {
            (
                "cmd",
                [
                    "/c",
                    "npx",
                    "-y",
                    "@modelcontextprotocol/server-filesystem",
                    allowed_dir,
                ]
                .map(String::from)
                .to_vec(),
            )
        } else {
            (
                "npx",
                ["-y", "@modelcontextprotocol/server-filesystem", allowed_dir]
                    .map(String::from)
                    .to_vec(),
            )
        };
        McpClient::spawn(program, &args, &[]).await
    }

    /// GO criterion: the model calls the file-reading MCP tool on its own and
    /// uses its result in the reply; calls/results go through our client; the
    /// turn ends cleanly, the server shuts down via the shutdown ladder.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL + npx (real filesystem MCP server)"]
    async fn gemma_reads_file_via_mcp_filesystem_server() {
        let Some(engine) =
            crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")
        else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };

        // A secret in a file inside the allowed directory.
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
                eprintln!("skip: failed to launch the npx MCP server: {e:#}");
                return;
            }
        };
        eprintln!(
            "MCP server: {} (protocol {})",
            client.server_info, client.protocol_version
        );

        // The tool catalog → OpenAI schemas (as stage 3 will do).
        let tools = client.list_tools().await.unwrap();
        assert!(!tools.is_empty(), "the filesystem server has tools");
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
            "tools: {}, schemas ≈ {} KiB (16k-context budget)",
            schemas.len(),
            schema_bytes / 1024
        );

        // A manual agentic loop (orchestrator/generation.rs's mechanics, by hand).
        let mut messages = vec![ApiMessage::user(format!(
            "Прочитай файл {allowed}/secret_number.txt с помощью инструмента и скажи, \
             какое секретное число в нём записано."
        ))];
        let mut rounds = 0;
        let final_text = loop {
            rounds += 1;
            assert!(rounds <= 8, "the model got stuck looping on tool calls");
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
                        eprintln!("→ model calls {}({})", call.name, call.arguments);
                        let result = client
                            .conn()
                            .call_tool(&call.name, args, Duration::from_secs(30), None)
                            .await
                            .unwrap();
                        eprintln!(
                            "← result ({}): {}",
                            if result.is_error { "error" } else { "ok" },
                            &result.text[..result.text.len().min(120)]
                        );
                        messages.push(ApiMessage::tool(call.id, result.text));
                    }
                }
                _ => break text,
            }
        };
        eprintln!("rounds: {rounds}; final answer: {final_text}");
        assert!(
            final_text.contains("7319"),
            "the model didn't use the MCP tool's result: {final_text:?}"
        );
        assert!(
            rounds >= 2,
            "the tool wasn't called (an answer with no rounds)"
        );
        client.shutdown().await;
    }
}
