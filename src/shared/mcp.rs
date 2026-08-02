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
//! are `.cmd`-shims (spawn as `cmd /c npx …` on Windows); `.bat`/`.cmd` themselves
//! as a server command are **forbidden** (CVE-2024-24576 "BatBadBut"); on Windows
//! the process tree is killed by a Job Object kill-on-close (orphaned
//! `npx`→`node` children don't outlive app exit).
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
    /// Tool execution error (`isError:true`) — the text is handed to the model
    /// as an error result, this is NOT a protocol error (spec tools §error handling).
    pub is_error: bool,
}

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
                    text.push_str(&format!("[{other} content omitted]"));
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
            (Some(id_v), false) => {
                let Some(id) = id_v.as_i64() else { continue };
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
            // A server request to us: reply to ping, everything else —
            // method-not-found (staying silent would hang a well-behaved server).
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
            // A server notification — ignored in the tools-only subset
            // (list_changed — groundwork for stage 3: re-listing the catalog).
            (None, true) => {}
            _ => {}
        }
    }
    // The stream is closed: wake every waiter with an error (the oneshot closes on drop).
    pending.lock().await.clear();
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

/// Is the server command a `.bat`/`.cmd`? Such commands are **forbidden** as an
/// MCP server command: CVE-2024-24576 "BatBadBut" — batch-file arguments on
/// Windows can't be escaped (Rust ≥1.77.2 itself rejects spawning with risky
/// arguments; we reject it earlier, with a clear message). `npx` servers are
/// configured as `cmd /c npx …` (the command is `cmd`, which is allowed) or via
/// a direct exe path.
pub fn forbidden_batch_command(command: &str) -> bool {
    let base = Path::new(command.trim())
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    base.ends_with(".bat") || base.ends_with(".cmd")
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
        if forbidden_batch_command(program) {
            bail!(
                "an MCP server command can't be .bat/.cmd (BatBadBut, \
                 CVE-2024-24576); use `cmd /c …` or a direct exe path"
            );
        }
        let mut cmd = Command::new(program);
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
        let mut child = cmd
            .spawn()
            .with_context(|| format!("launching MCP server: {program}"))?;

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
    async fn call_tool_joins_text_and_placeholders_non_text() {
        // call_tool on a connection: text blocks are concatenated, image → a placeholder.
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
        assert!(res.text.starts_with("hello"), "{}", res.text);
        assert!(res.text.contains("[image content omitted]"), "{}", res.text);
        assert!(!res.is_error);
    }

    #[test]
    fn batch_commands_are_forbidden() {
        // BatBadBut (CVE-2024-24576): .bat/.cmd as a server command are
        // forbidden, including with a path and any letter case; `cmd` (the
        // shell for npx) and exe are allowed.
        assert!(forbidden_batch_command("evil.bat"));
        assert!(forbidden_batch_command("C:/tools/npx.CMD"));
        assert!(forbidden_batch_command(r"C:\tools\run.Bat"));
        assert!(!forbidden_batch_command("cmd"));
        assert!(!forbidden_batch_command("npx"));
        assert!(!forbidden_batch_command("C:/tools/server.exe"));
    }

    #[tokio::test]
    async fn spawn_rejects_batch_command() {
        let err = match McpClient::spawn("server.cmd", &[], &[]).await {
            Ok(_) => panic!(".cmd command should have been rejected"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("BatBadBut"), "{err}");
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
