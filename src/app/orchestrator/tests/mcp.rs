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
    assert!(orch.mcp.is_current(&orch.config.mcp));

    // Removing it takes the server back down.
    let mut cleared = orch.config.clone();
    cleared.mcp.servers.clear();
    orch.handle_update_config(cleared);
    orch.flush_restarts();
    assert!(orch.mcp.snapshot().servers.is_empty());
}

/// The launch command for a real `@modelcontextprotocol/server-filesystem` over
/// `npx`, restricted to `allowed`. On Windows `npx` is a `.cmd` shim, so it is
/// launched through `cmd /c` (the §4.6 pitfall; a `.cmd` **as the command** is
/// refused outright — BatBadBut).
fn filesystem_server_cmd(allowed: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd".into(),
            [
                "/c",
                "npx",
                "-y",
                "@modelcontextprotocol/server-filesystem",
                allowed,
            ]
            .map(String::from)
            .to_vec(),
        )
    } else {
        (
            "npx".into(),
            ["-y", "@modelcontextprotocol/server-filesystem", allowed]
                .map(String::from)
                .to_vec(),
        )
    }
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
    mgr.apply(&settings, loc);
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
