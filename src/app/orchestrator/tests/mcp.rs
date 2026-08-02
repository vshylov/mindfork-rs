//! Tests for MCP-host integration into the orchestrator: the `Ready` event puts wrappers
//! into the registry and a dynamic catalog into the `Settings` snapshot; a server crash removes
//! the tools. The protocol lives in `shared/mcp.rs`, the manager in `orchestrator/mcp.rs`.

use super::*;
// The test submodule `tests::mcp` shadows the source module `orchestrator::mcp` — go up
// to the orchestrator (the tests/self_model.rs pattern).
use super::super::mcp::McpEvent;
use crate::shared::config::{McpServerConfig, McpSettings};
use crate::shared::mcp::{McpConnection, McpToolInfo};

/// A connection over duplex (the transport isn't exercised in these tests).
fn dummy_conn() -> Arc<McpConnection> {
    let (client_io, _server_io) = tokio::io::duplex(1024);
    let (r, w) = tokio::io::split(client_io);
    Arc::new(McpConnection::over(r, w))
}

fn mcp_config() -> McpSettings {
    McpSettings {
        enabled: true,
        servers: vec![McpServerConfig {
            id: "fs".into(),
            // A nonexistent binary: the real background task would send Failed into
            // the manager's channel (it's dropped in tests) — events are fed in manually.
            command: "nonexistent-mcp-server-binary".into(),
            ..Default::default()
        }],
    }
}

fn ready_event(orch: &Orchestrator) -> McpEvent {
    McpEvent::Ready {
        epoch: orch.mcp.epoch(),
        server: "fs".into(),
        conn: dummy_conn(),
        tools: vec![McpToolInfo {
            name: "read_text_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({ "type": "object" }),
        }],
        server_info: "fake 0.1".into(),
    }
}

#[tokio::test]
async fn mcp_ready_registers_tools_and_rides_settings_snapshot() {
    let (_dir, mut orch, mut evt_rx) = bare_orch_rx();
    orch.config.mcp = mcp_config();
    orch.apply_mcp_settings();
    assert!(
        orch.registry.get("mcp__fs__read_text_file").is_none(),
        "no wrapper should be in the registry before Ready"
    );

    orch.handle_mcp_event(ready_event(&orch));

    // The wrapper is in the registry (a turn can call it), the standard tools are intact.
    assert!(orch.registry.get("mcp__fs__read_text_file").is_some());
    assert!(orch.registry.get("note_save").is_some());
    // TOFU: the first bring-up auto-pins the catalog, the pin persists to settings.json.
    let pin = orch.config.mcp.servers[0].pinned_catalog.clone();
    assert!(pin.is_some(), "the pin wasn't written to the config");
    let saved = orch.storage.json().load_config().unwrap();
    assert_eq!(
        saved.mcp.servers[0].pinned_catalog, pin,
        "the pin wasn't saved"
    );
    // The Settings snapshot carries the dynamic catalog and the server status.
    let evt = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    let AppEvent::Settings { mcp, .. } = evt else {
        unreachable!()
    };
    assert_eq!(mcp.tools.len(), 1);
    assert_eq!(mcp.tools[0].id, "mcp__fs__read_text_file");
    assert!(!mcp.tools[0].enabled_by_default, "double opt-in");
    assert_eq!(mcp.tools[0].description.as_deref(), Some("Read a file"));
    assert_eq!(mcp.servers.len(), 1);
    assert_eq!(mcp.servers[0].tool_count, 1);
}

#[tokio::test]
async fn changed_catalog_requires_confirmation_before_registering() {
    let (_dir, mut orch, mut evt_rx) = bare_orch_rx();
    let mut config = mcp_config();
    // Pin a "previous" catalog (a different description) — Ready with a changed one won't pass.
    config.servers[0].pinned_catalog =
        Some(crate::features::tools::mcp::catalog_hash(&[McpToolInfo {
            name: "read_text_file".into(),
            description: "старое описание".into(),
            input_schema: serde_json::json!({ "type": "object" }),
        }]));
    orch.config.mcp = config;
    orch.apply_mcp_settings();
    orch.handle_mcp_event(ready_event(&orch));

    // Tools are held back: they didn't make it into the registry, the snapshot marks pending.
    assert!(orch.registry.get("mcp__fs__read_text_file").is_none());
    let evt = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    let AppEvent::Settings { mcp, .. } = evt else {
        unreachable!()
    };
    assert!(mcp.servers[0].pending_catalog);
    assert!(mcp.tools.is_empty());

    // Confirmation (Enter in settings → a command): the tools are in the registry,
    // the new pin persists.
    let old_pin = orch.config.mcp.servers[0].pinned_catalog.clone();
    orch.handle_confirm_mcp_catalog("fs".into());
    assert!(orch.registry.get("mcp__fs__read_text_file").is_some());
    let new_pin = orch.config.mcp.servers[0].pinned_catalog.clone();
    assert_ne!(new_pin, old_pin, "the pin is updated to the new catalog");
    let saved = orch.storage.json().load_config().unwrap();
    assert_eq!(saved.mcp.servers[0].pinned_catalog, new_pin);
}

#[tokio::test]
async fn update_config_inherits_mcp_pins_from_stale_ui_snapshot() {
    // A config snapshot from the UI may not carry pins (a stale copy) — editing
    // settings must not reset trust or trigger a server restart.
    let (_dir, mut orch) = bare_orch();
    orch.config.mcp = mcp_config();
    orch.config.mcp.servers[0].pinned_catalog = Some("abc123".into());
    let mut ui_copy = orch.config.clone();
    ui_copy.mcp.servers[0].pinned_catalog = None; // a stale snapshot with no pin
    orch.handle_update_config(ui_copy);
    assert_eq!(
        orch.config.mcp.servers[0].pinned_catalog.as_deref(),
        Some("abc123"),
        "the pin is inherited by server id"
    );
}

/// Live e2e smoke for stage 3a (docs/research/plugin-system.md §7): the full path through the
/// orchestrator — a config with a real `npx @modelcontextprotocol/server-filesystem`,
/// the profile enables MCP tools, a live model reads a file with its tool and
/// uses the content in the reply. Requires `MINDFORK_ENGINE_URL` + `npx` on the PATH.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + npx (real filesystem MCP server)"]
async fn mcp_filesystem_e2e_live() {
    use crate::features::profiles::ProfileEdit;
    use crate::features::tools::default_tool_ids;

    // A secret file in a directory allowed for the filesystem MCP server.
    let files_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        files_dir.path().join("secret_number.txt"),
        "Секретное число: 7319",
    )
    .unwrap();
    let allowed = files_dir.path().to_string_lossy().replace('\\', "/");

    let (command, args) = filesystem_server_cmd(&allowed);
    let config = AppConfig {
        mcp: McpSettings {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "fs".into(),
                command,
                args,
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    let Some((_dir, cmd_tx, mut evt_rx, _handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };

    // ProfileList is emitted during bootstrap — collect it BEFORE waiting for the MCP catalog:
    // `wait_for` drains all events along the way, and the earlier ProfileList
    // would be swallowed by waiting for the later Settings (the test would hang).
    let pl = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v[0].id,
        _ => unreachable!(),
    };

    // Wait for server readiness: a Settings snapshot with a non-empty MCP catalog
    // (npx downloads the package on the first run — the timeout is generous).
    let settings = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::Settings { mcp, .. } if !mcp.tools.is_empty()),
        ),
    )
    .await
    .expect("the MCP server should come up within 120s")
    .unwrap();
    let AppEvent::Settings { mcp, .. } = settings else {
        unreachable!()
    };
    eprintln!("MCP tools in the catalog: {}", mcp.tools.len());
    assert!(mcp.tools.iter().all(|t| t.id.starts_with("mcp__fs__")));

    // The profile enables MCP tools (double opt-in: the master gate is already on).
    let mut enabled = default_tool_ids();
    enabled.extend(mcp.tools.iter().map(|t| t.id.clone()));
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(enabled),
                ..Default::default()
            }),
        })
        .unwrap();

    // Turn: the model should read the file with the MCP tool and name the number.
    let (out, tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Прочитай файл {allowed}/secret_number.txt с помощью инструмента и скажи, \
             какое секретное число в нём записано."
        ),
    )
    .await;
    eprintln!("calls: {tools:?}\nreply: {out}");
    assert!(
        tools.iter().any(|t| t.starts_with("mcp__fs__")),
        "the model didn't call the MCP tool: {tools:?}"
    );
    assert!(
        out.contains("7319"),
        "the reply doesn't use the MCP tool's result: {out:?}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
}

/// Storing an MCP environment secret schedules the re-apply that hands it to the
/// child process. Without this the `is_current` fix is never even consulted: the
/// settings are unchanged, so nothing would flag the servers, and the new value
/// would sit in the store while the running server keeps the old one.
#[tokio::test]
async fn storing_an_env_secret_schedules_the_reapply() {
    if !crate::shared::secrets::scheme_available() {
        return; // Linux without machine-id: storing secrets is unsupported
    }
    let (_dir, mut orch) = bare_orch();
    let mut settings = mcp_config();
    settings.servers[0]
        .env
        .insert("TOKEN".into(), String::new());
    orch.config.mcp = settings;
    orch.apply_mcp_settings();
    assert!(orch.mcp.is_current(&orch.config.mcp, &orch.config.api_keys));

    orch.handle_set_secret(
        crate::shared::secrets::SecretKey::McpEnv {
            server: "fs".into(),
            var: "TOKEN".into(),
        },
        "ghp-live".into(),
    );
    assert!(
        !orch.mcp.is_current(&orch.config.mcp, &orch.config.api_keys),
        "the value handed to the child changed"
    );
    orch.flush_restarts();
    assert!(
        orch.mcp.is_current(&orch.config.mcp, &orch.config.api_keys),
        "the debounce should have re-applied the servers with the new value"
    );
}

/// End to end for the JSON import (§9, S5–S7): the file's **literal** `env`
/// values become machine-bound secrets, the servers arrive disabled, ids are
/// sanitized to our slug, and — the claim that matters — no token reaches
/// `settings.json` in the clear.
#[tokio::test]
async fn import_stores_secrets_and_never_writes_them_in_the_clear() {
    if !crate::shared::secrets::scheme_available() {
        return; // Linux without machine-id: storing secrets is unsupported
    }
    let (dir, mut orch) = bare_orch();
    let path = dir.path().join("claude_desktop_config.json");
    std::fs::write(
        &path,
        r#"{ "mcpServers": {
              "My Server": { "command": "npx",
                             "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:/w"],
                             "env": { "GITHUB_TOKEN": "ghp_live_secret_42" } },
              "remote":    { "type": "sse", "url": "https://example/mcp" }
        } }"#,
    )
    .unwrap();

    orch.handle_import_mcp_servers(path.to_string_lossy().to_string());

    assert_eq!(orch.config.mcp.servers.len(), 1, "the sse entry is skipped");
    let srv = &orch.config.mcp.servers[0];
    assert_eq!(
        srv.id, "my-server",
        "the free-form key is sanitized to a slug"
    );
    assert!(!srv.enabled, "an imported server never spawns on arrival");
    assert_eq!(srv.command, "npx");
    // The variable is declared with no OS source; its value lives in the secret store.
    assert_eq!(srv.env.get("GITHUB_TOKEN").map(String::as_str), Some(""));
    let stored = crate::shared::secrets::stored_key(
        &orch.config.api_keys,
        &crate::shared::secrets::SecretKey::McpEnv {
            server: "my-server".into(),
            var: "GITHUB_TOKEN".into(),
        }
        .storage_name(),
    );
    assert_eq!(stored.as_deref(), Some("ghp_live_secret_42"));

    // On disk: ciphertext only. This is the whole reason the orchestrator parses
    // the file instead of the screen.
    let raw = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    assert!(
        !raw.contains("ghp_live_secret_42"),
        "the imported token leaked into settings.json"
    );
    assert!(raw.contains("my-server"), "the server wasn't persisted");

    // A re-import adds nothing (S7) — and does not duplicate the server.
    orch.handle_import_mcp_servers(path.to_string_lossy().to_string());
    assert_eq!(orch.config.mcp.servers.len(), 1);
}

/// A file that cannot be read or parsed reports itself on the import row rather
/// than failing silently — and changes nothing.
#[tokio::test]
async fn a_bad_import_reports_and_changes_nothing() {
    let (dir, mut orch, mut rx) = bare_orch_rx();
    orch.handle_import_mcp_servers(dir.path().join("nope.json").to_string_lossy().to_string());
    assert!(orch.config.mcp.servers.is_empty());
    let reported = std::iter::from_fn(|| rx.try_recv().ok())
        .any(|e| matches!(e, AppEvent::McpImportResult(_)));
    assert!(reported, "a failed import must say so");
}

#[tokio::test]
async fn mcp_exited_removes_tools_from_registry() {
    let (_dir, mut orch) = bare_orch();
    orch.config.mcp = mcp_config();
    orch.apply_mcp_settings();
    orch.handle_mcp_event(ready_event(&orch));
    assert!(orch.registry.get("mcp__fs__read_text_file").is_some());

    // Process crash: the connection is dead — the wrapper leaves the registry immediately.
    let epoch = orch.mcp.epoch();
    orch.handle_mcp_event(McpEvent::Exited {
        epoch,
        server: "fs".into(),
    });
    assert!(orch.registry.get("mcp__fs__read_text_file").is_none());
    assert!(orch.registry.get("note_save").is_some());
}

#[tokio::test]
async fn a_server_edited_in_settings_reaches_the_host() {
    // The seam the whole settings editor rests on: the screen only sends the
    // config, and everything else is the existing `UpdateConfig` → diff →
    // debounce → `apply_mcp_settings` path (docs/history/mcp-server-editor.md §1).
    // Without this, "the orchestrator needed no changes" is an untested claim.
    let (_dir, mut orch) = bare_orch();
    assert!(orch.mcp.snapshot().servers.is_empty());

    let mut edited = orch.config.clone();
    edited.mcp = mcp_config(); // the config a `Ctrl+N` + field edits would produce
    orch.handle_update_config(edited);
    orch.flush_restarts();
    assert_eq!(
        orch.mcp.snapshot().servers.len(),
        1,
        "the edited server should have reached the host"
    );

    // …and an edit that changes nothing effective costs no restart, so the
    // debounce cannot respawn `npx` for an edit plus its undo.
    let same = orch.config.clone();
    orch.handle_update_config(same);
    assert!(orch.mcp.is_current(&orch.config.mcp, &orch.config.api_keys));

    // Removing it takes the server back down.
    let mut cleared = orch.config.clone();
    cleared.mcp.servers.clear();
    orch.handle_update_config(cleared);
    orch.flush_restarts();
    assert!(orch.mcp.snapshot().servers.is_empty());
}

/// A minimal MCP server in Node that answers `tools/list` with a tool **named
/// after an environment variable it was given**. Enough protocol to reach
/// `Ready`; the point is what the child can see in its own environment.
#[cfg(test)]
fn env_echo_server_script() -> String {
    r#"
let buf = "";
process.stdin.on("data", (d) => {
  buf += d;
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1);
    if (!line.trim()) continue;
    const msg = JSON.parse(line);
    if (msg.method === "initialize") {
      send({ jsonrpc: "2.0", id: msg.id, result: {
        protocolVersion: "2025-11-25",
        capabilities: { tools: {} },
        serverInfo: { name: "env-echo", version: "1" } } });
    } else if (msg.method === "tools/list") {
      send({ jsonrpc: "2.0", id: msg.id, result: { tools: [
        { name: "echo_env",
          description: process.env.SECRET_TOKEN || "(no SECRET_TOKEN)",
          inputSchema: { type: "object" } } ] } });
    } else if (msg.id !== undefined) {
      send({ jsonrpc: "2.0", id: msg.id, result: {} });
    }
  }
});
function send(o) { process.stdout.write(JSON.stringify(o) + "\n"); }
"#
    .to_string()
}

/// **The one thing unit tests cannot show**: that a value stored as a
/// machine-bound secret actually reaches the child process's environment. The
/// storage, the resolution and the re-apply are each covered above; here a real
/// subprocess is spawned by the real manager and asked what it sees.
///
/// Needs only `node` — no model, so it runs without an engine.
#[tokio::test]
#[ignore = "requires node (spawns a real MCP server subprocess)"]
async fn stored_env_secret_reaches_the_child_process_live() {
    if !crate::shared::secrets::scheme_available() {
        eprintln!("skip: no secret encryption scheme on this machine");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("env-echo-server.mjs");
    std::fs::write(&script, env_echo_server_script()).unwrap();

    // The server declares SECRET_TOKEN with **no** OS source: the only way it can
    // get a value is the stored secret.
    let mut cfg = McpServerConfig {
        id: "envecho".into(),
        command: "node".into(),
        args: vec![script.to_string_lossy().to_string()],
        ..Default::default()
    };
    cfg.env.insert("SECRET_TOKEN".into(), String::new());
    let settings = McpSettings {
        enabled: true,
        servers: vec![cfg],
    };
    let mut keys = Vec::new();
    crate::shared::secrets::put_key(
        &mut keys,
        &crate::shared::secrets::SecretKey::McpEnv {
            server: "envecho".into(),
            var: "SECRET_TOKEN".into(),
        }
        .storage_name(),
        "ZARYA-7719",
        || "test".to_string(),
    )
    .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut mgr = super::super::mcp::McpManager::new(tx);
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    mgr.apply(&settings, &keys, loc);
    let evt = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("the node MCP server should come up within 30s")
        .unwrap();
    mgr.handle_event(evt, loc);

    let snap = mgr.snapshot();
    eprintln!("server: {:?}", snap.servers[0].status);
    let seen = snap.tools[0].description.clone().unwrap_or_default();
    eprintln!("the child reports SECRET_TOKEN as: {seen}");
    assert_eq!(
        seen, "ZARYA-7719",
        "the stored secret did not reach the child process's environment"
    );
    mgr.shutdown();
}

/// The launch command for a real `@modelcontextprotocol/server-filesystem` over
/// `npx` — **the same on every platform**, which is the point: on Windows `npx`
/// is a `.cmd` shim that `Command` cannot find without `PATHEXT` completion, and
/// `shared::mcp::resolve_command` closes exactly that gap. Before it, this test
/// needed a `cfg!(windows)` branch spelling out `cmd /c npx …`.
fn filesystem_server_cmd(allowed: &str) -> (String, Vec<String>) {
    (
        "npx".into(),
        ["-y", "@modelcontextprotocol/server-filesystem", allowed]
            .map(String::from)
            .to_vec(),
    )
}

/// Live smoke for the reconnect action (docs/history/mcp-server-editor.md F7):
/// a **real** server is brought up, then reconnected the way `Enter` on its
/// settings row does — and has to come back with its catalog.
///
/// This is the half unit tests cannot answer. They can prove the slot is reset
/// and a task is spawned with a fresh generation; whether a real subprocess is
/// actually torn down and a new one handshakes successfully in its place is a
/// property of the process handling, not of the manager's bookkeeping. Needs
/// only `npx` — no model, so it runs without an engine.
#[tokio::test]
#[ignore = "requires npx (real filesystem MCP server)"]
async fn mcp_reconnect_live() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().to_string_lossy().replace('\\', "/");
    let (command, args) = filesystem_server_cmd(&allowed);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut mgr = super::super::mcp::McpManager::new(tx);
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let settings = McpSettings {
        enabled: true,
        servers: vec![McpServerConfig {
            id: "fs".into(),
            command,
            args,
            ..Default::default()
        }],
    };

    // First bring-up (npx may download the package — a generous timeout).
    mgr.apply(&settings, &[], loc);
    let evt = tokio::time::timeout(std::time::Duration::from_secs(120), rx.recv())
        .await
        .expect("the MCP server should come up within 120s")
        .unwrap();
    mgr.handle_event(evt, loc);
    let first = mgr.snapshot();
    eprintln!(
        "first bring-up: {:?}, tools: {}",
        first.servers[0].status, first.servers[0].tool_count
    );
    assert_eq!(first.servers[0].status, ServerStatus::Ready);
    assert!(first.servers[0].tool_count > 0);

    // Reconnect: the old process is torn down, a new one takes its place.
    assert!(mgr.reconnect("fs", loc));
    assert_eq!(mgr.snapshot().servers[0].status, ServerStatus::Connecting);
    assert!(
        mgr.snapshot().tools.is_empty(),
        "the tools go with the connection being torn down"
    );
    let evt = tokio::time::timeout(std::time::Duration::from_secs(120), rx.recv())
        .await
        .expect("the reconnected server should come up within 120s")
        .unwrap();
    mgr.handle_event(evt, loc);
    let again = mgr.snapshot();
    eprintln!(
        "after reconnect: {:?}, tools: {}",
        again.servers[0].status, again.servers[0].tool_count
    );
    assert_eq!(again.servers[0].status, ServerStatus::Ready);
    assert_eq!(
        again.servers[0].tool_count, first.servers[0].tool_count,
        "the same server should come back with the same catalog"
    );
}
