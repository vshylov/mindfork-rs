//! The [`McpTool`] wrapper — an MCP server's tool behind the [`Tool`] trait
//! (docs/research/plugin-system.md §4.4, part 3). The description and JSON schema are a
//! snapshot from the server's `tools/list` (**not localized** — an i18n boundary, like
//! engine probe errors); `invoke` → `tools/call` with a per-call timeout and cancellation
//! (`ctx.cancel`); the result is clipped (`max_result_chars`) — a limit on prompt input.
//!
//! The tool's id is `mcp__<server>__<tool>` (the Claude Code convention), normalized
//! against providers' function-name limits ([`mcp_tool_id`]): `[A-Za-z0-9_-]`,
//! ≤ 64 characters (truncation + a hex tail from the full name against collisions).
//! `enabled_by_default = false` — double opt-in (the master gate `config.mcp.enabled`
//! and a profile toggle); these tools aren't in the static `CATALOG`
//! (the dynamic catalog rides as a snapshot in `AppEvent::Settings`).

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use sha2::{Digest, Sha256};

use super::{Tool, ToolContext, ToolImage, ToolOutcome, meta};
use crate::entities::profile::ToolId;
use crate::shared::i18n::Locale;
use crate::shared::mcp::{McpConnection, McpToolInfo};
use crate::shared::server::ServerStatus;

/// The MCP host's snapshot for the UI (rides in `AppEvent::Settings`): the dynamic
/// catalog of tools (profile toggles) + server statuses (rows in the "Tools"
/// section). FSD: lives in `features` — `screens` doesn't import `app`.
#[derive(Debug, Clone, Default)]
pub struct McpSnapshot {
    /// Tool metadata for all ready servers (with full descriptions).
    pub tools: Vec<meta::ToolInfo>,
    /// Server statuses (by id).
    pub servers: Vec<McpServerSnapshot>,
}

/// A snapshot of one MCP server for the UI.
#[derive(Debug, Clone)]
pub struct McpServerSnapshot {
    pub id: String,
    pub status: ServerStatus,
    /// The number of registered tools (0 — the server isn't ready).
    pub tool_count: usize,
    /// The server's catalog has changed against the TOFU pin — the tools aren't
    /// registered, waiting for the user's confirmation (Enter in settings).
    pub pending_catalog: bool,
}

/// The TOFU hash of a server's tool catalog: sha256 over sorted
/// (name, description, JSON schema) — any change to any field (a rug-pull, tool
/// poisoning via descriptions/schemas) changes the hash. Determinism: tools
/// are sorted by name, `serde_json`'s JSON-object keys are ordered (BTreeMap).
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

/// The id prefix of MCP-server tools. [`super::effective_tool_ids`] uses it to
/// gate them via the master switch `config.mcp.enabled` (the tools are dynamic —
/// they're absent from the static `CATALOG`, so a gate lookup there is impossible).
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// The ceiling on providers' function-tool name length (historically
/// `^[a-zA-Z0-9_-]{1,64}$` for OpenAI/Anthropic).
const MAX_TOOL_ID: usize = 64;

/// The full id of an MCP-server tool: `mcp__<server>__<tool>`, sanitized
/// against provider limits: characters outside `[A-Za-z0-9_-]` → `_`; longer than 64 —
/// truncation + a `_` separator + 8 hex characters from a hash of the **full** name (long
/// names' collisions don't merge). A pure function — testable.
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

/// Interns a string into `&'static str` (dedup via a global set).
/// Needed for `Tool::ui_label`/`ToolInfo.label` (`&'static str`): MCP labels are
/// dynamic tool names; there's a finite number of them, so the leak is bounded
/// (a precedent — interning language codes in `shared/i18n`).
fn intern(s: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = pool.lock().expect("intern pool poisoned");
    if let Some(existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// An MCP-server tool behind the [`Tool`] trait. Holds the shared connection
/// transport ([`McpConnection`]) — the server process's lifecycle belongs to
/// `McpManager` (`app/orchestrator/mcp.rs`).
pub struct McpTool {
    id: ToolId,
    /// The tool's name on the server (the original, without the prefix/sanitization).
    remote_name: String,
    /// A description snapshot from `tools/list` (not localized — the server's text).
    description: String,
    /// A snapshot of the arguments' JSON schema (`inputSchema`).
    input_schema: serde_json::Value,
    /// A short label for the profile toggle (the tool's interned name).
    label: &'static str,
    conn: Arc<McpConnection>,
    /// The per-call timeout (the server's `tool_timeout_secs`).
    timeout: Duration,
    /// The result clip in characters (the server's `max_result_chars`).
    max_result_chars: usize,
}

impl McpTool {
    /// A wrapper over tool `info` of server `server_id` on connection `conn`.
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

/// Clips text to `max` characters (by character, not byte — Cyrillic), with
/// a truncation note in the profile's scaffold language (axis A).
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
        // The server's text — an i18n boundary (see docs/history/i18n.md §2.3).
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
        // `isError:true` — an execution error on the server: the text is given to the model
        // as the result (not a protocol error, spec tools §error handling). An empty
        // error text is replaced with a marker — the model must understand the call failed.
        let text = if result.is_error && result.text.is_empty() {
            ctx.loc.t("tool.mcp.error_empty").to_string()
        } else {
            result.text
        };
        // Images ride alongside the text rather than inside it (spec §9.10). The switch
        // is consulted here, at the boundary where third-party pixels would enter the
        // conversation: a user who turned it off keeps the server and its text results.
        let (images, withheld): (Vec<ToolImage>, usize) = if ctx.mcp_images {
            (
                result
                    .images
                    .into_iter()
                    .map(|i| ToolImage {
                        mime: i.mime,
                        data: i.data,
                    })
                    .collect(),
                0,
            )
        } else {
            (Vec::new(), result.images.len())
        };
        let mut text = clip_result(&text, self.max_result_chars, ctx.loc);
        // Nothing withheld goes unsaid (spec §9.10, docs/history/sandbox-file-exchange.md §11 S8).
        // The parser's `[image content omitted]` covers only a *malformed* block, so a
        // well-formed image under the cap left no trace at all and the model answered
        // about pictures it never received — the failure §10 measured, where both families
        // described a chart they had not seen. Said **after** the clip: the one line that
        // says what is missing must not be the one the truncation eats.
        if withheld > 0 {
            text.push('\n');
            text.push_str(
                &ctx.loc
                    .tf("tool.mcp.images_off", &[("n", &withheld.to_string())]),
            );
        }
        Ok(ToolOutcome::text(text).with_images(images))
    }

    fn group(&self) -> meta::ToolGroup {
        meta::ToolGroup::Plugins
    }

    fn ui_label(&self) -> &'static str {
        self.label
    }

    /// Every MCP tool is third-party code whose effects we cannot know (fork
    /// F3). The server's own `destructiveHint`/`readOnlyHint` annotations are
    /// untrusted input — a server can claim anything — so they may never *relax*
    /// this, which is why they are not consulted at all.
    fn danger(&self) -> bool {
        true
    }

    fn gate(&self) -> Option<meta::ToolGate> {
        Some(meta::ToolGate::Mcp)
    }

    fn enabled_by_default(&self) -> bool {
        // Double opt-in (decision point R7): enabled manually in the profile.
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
        // Dots/other characters (the spec allows `.`) → `_`.
        assert_eq!(mcp_tool_id("srv", "a.b/c d"), "mcp__srv__a_b_c_d");
    }

    #[test]
    fn tool_id_truncates_long_names_with_stable_hash_tail() {
        let long = "x".repeat(100);
        let id1 = mcp_tool_id("server-with-long-id", &long);
        assert_eq!(id1.len(), 64);
        // Determinism and distinguishability: a different full name → a different tail.
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

    /// A fake MCP server over a duplex, replying to tools/call with the given
    /// content; returns the client's connection.
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
        let conn = conn_with_call_reply("0123456789".repeat(10)); // 100 chars
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

    /// A server that answers every call with one text block and one image block.
    fn conn_with_image_reply() -> Arc<McpConnection> {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_r, client_w) = tokio::io::split(client_io);
        let (server_r, mut server_w) = tokio::io::split(server_io);
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let Some(id) = msg.get("id").cloned() else {
                    continue;
                };
                let reply = json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "content": [
                        { "type": "text", "text": "Screenshot taken." },
                        { "type": "image", "data": "QUJD", "mimeType": "image/png" },
                    ],
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

    /// The `tools.mcp_images` switch decides at the boundary where third-party pixels
    /// would enter the conversation (fork F3 of docs/research/mcp-tool-images.md). Both
    /// directions are asserted, because a switch that only ever reads one way is
    /// indistinguishable from no switch at all.
    #[tokio::test]
    async fn the_switch_decides_whether_a_server_image_reaches_the_model() {
        let info = McpToolInfo {
            name: "screenshot".into(),
            description: "Take a screenshot".into(),
            input_schema: json!({ "type": "object" }),
        };

        let tool = McpTool::new(
            "test",
            &info,
            conn_with_image_reply(),
            Duration::from_secs(5),
            1000,
        );
        let (_d, _s, mut ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        ctx.mcp_images = true;
        let out = tool.invoke(&ctx, json!({})).await.unwrap();
        assert_eq!(
            out.images,
            vec![ToolImage {
                mime: "image/png".into(),
                data: "QUJD".into(),
            }]
        );
        assert_eq!(out.result, "Screenshot taken.");

        // Off: the server and its text keep working, only the pixels stay behind — and
        // the result *says* they did. Silence here is what §10 measured: the model reads
        // a complete-looking result and describes a picture it never received.
        let tool = McpTool::new(
            "test",
            &info,
            conn_with_image_reply(),
            Duration::from_secs(5),
            1000,
        );
        ctx.mcp_images = false;
        let out = tool.invoke(&ctx, json!({})).await.unwrap();
        assert!(out.images.is_empty());
        assert!(
            out.result.starts_with("Screenshot taken."),
            "the server's own text survives: {}",
            out.result
        );
        let said = ctx.loc.tf("tool.mcp.images_off", &[("n", "1")]);
        assert!(
            out.result.ends_with(&said),
            "the withheld image has to be stated: {}",
            out.result
        );
    }

    /// The statement is appended **after** the clip, so the one line that says what the
    /// model is missing cannot be the line the truncation eats.
    #[tokio::test]
    async fn a_withheld_image_is_still_stated_when_the_text_is_truncated() {
        let info = McpToolInfo {
            name: "screenshot".into(),
            description: "Take a screenshot".into(),
            input_schema: json!({ "type": "object" }),
        };
        // `max_result_chars` well under the server's text, so `clip_result` fires.
        let tool = McpTool::new(
            "test",
            &info,
            conn_with_image_reply(),
            Duration::from_secs(5),
            4,
        );
        let (_d, _s, mut ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        ctx.mcp_images = false;
        let out = tool.invoke(&ctx, json!({})).await.unwrap();
        assert!(
            out.result
                .contains(&ctx.loc.t("tool.mcp.result_truncated").to_string()),
            "the fixture must actually be truncated: {}",
            out.result
        );
        let said = ctx.loc.tf("tool.mcp.images_off", &[("n", "1")]);
        assert!(out.result.ends_with(&said), "{}", out.result);
    }

    #[tokio::test]
    async fn invoke_is_cancellable_via_ctx_cancel() {
        // The server stays silent → cancelling ctx.cancel interrupts the call (Esc isn't blocked).
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
        assert!(err.contains("cancelled"), "{err}");
    }
}
