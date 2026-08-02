//! [`McpManager`] — lifecycle of MCP servers (tool plugins,
//! docs/research/plugin-system.md §4.4): spawns enabled servers from settings
//! (`config.mcp`), handshake + `tools/list` → a dynamic catalog of [`McpTool`]
//! wrappers, per-server status, a restart budget (N crashes within a window →
//! `Disconnected` until manual intervention — no endless restart loop, the
//! VS Code LSP pattern). Mirrors [`EngineManager`](super::engines::EngineManager):
//! the orchestrator remains the sole owner of `Chat`; here — only servers.
//!
//! Asynchrony: spawn/handshake take time, while command handlers are
//! synchronous — each server comes up as a background task, sending
//! [`McpEvent`] into the `run` loop's internal channel (like the engine
//! probe). Events carry `epoch` — a settings generation: late events from
//! already-shut-down servers (a "died before cancel" race) are dropped.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::app::events::{AppEvent, ServerStatus};
use crate::features::tools::Tool;
use crate::features::tools::mcp::{McpServerSnapshot, McpSnapshot, McpTool, catalog_hash};
use crate::features::tools::meta::ToolInfo;
use crate::shared::config::{McpServerConfig, McpSettings};
use crate::shared::i18n::Locale;
use crate::shared::mcp::{McpClient, McpConnection, McpToolInfo, valid_server_id};
use crate::shared::secrets::{ApiKeyEntry, SecretKey};

/// Max server restarts within the [`RESTART_WINDOW`] window; beyond that —
/// `Disconnected` until manual intervention (editing settings recreates the slot).
const RESTART_BUDGET: usize = 3;
/// The restart budget's window.
const RESTART_WINDOW: Duration = Duration::from_secs(300);

/// Every enabled server's resolved child environment, by server id.
type ResolvedEnvs = BTreeMap<String, Vec<(String, String)>>;

/// A server background task's event → the orchestrator's `run` loop.
pub(super) enum McpEvent {
    /// The server came up: handshake + the tool catalog were received.
    Ready {
        epoch: u64,
        server: String,
        conn: Arc<McpConnection>,
        tools: Vec<McpToolInfo>,
        server_info: String,
    },
    /// Spawn/handshake failed (a config/binary error) — no auto-restart.
    Failed {
        epoch: u64,
        server: String,
        reason: String,
    },
    /// The server process exited after being ready (a crash) — restart per the budget.
    Exited { epoch: u64, server: String },
}

/// One server's slot: a config snapshot, status, tools, restart budget.
struct McpSlot {
    cfg: McpServerConfig,
    /// The child's environment, already resolved (stored secrets decrypted, OS
    /// variables read). Held here because a respawn — after a crash or a
    /// reconnect — builds the task from the slot, and because it is what a
    /// re-apply is compared against. Plaintext lives here for as long as the
    /// server runs; unavoidable, it is what gets handed to the process.
    envs: Vec<(String, String)>,
    /// The generation this slot's task was spawned in. Events are matched
    /// against **the slot's** epoch, not the manager's: a reconnect bumps the
    /// generation for one server, and the others' in-flight events must still
    /// be accepted (otherwise reconnecting one server would leave another stuck
    /// on "connecting…" forever).
    epoch: u64,
    status: ServerStatus,
    /// Tool wrappers (after Ready; empty before readiness / after a crash).
    tools: Vec<Arc<dyn Tool>>,
    /// A snapshot of tool metadata (the profile toggle catalog).
    infos: Vec<ToolInfo>,
    /// Cancellation of the server's background task (spawn/monitor); also
    /// shuts down the process.
    cancel: CancellationToken,
    /// Restart timestamps within the budget window.
    restarts: Vec<Instant>,
    /// A catalog that failed TOFU pinning (changed vs. the approved one): held
    /// until the user confirms it ([`McpManager::confirm`]) — tools are not
    /// registered. See docs/research/plugin-system.md §4.5.
    pending: Option<PendingCatalog>,
}

/// A received but not-yet-approved server catalog (a TOFU mismatch).
struct PendingCatalog {
    conn: Arc<McpConnection>,
    tools: Vec<McpToolInfo>,
    hash: String,
}

/// Outcome of the manager processing an event — what the orchestrator should do.
#[derive(Default)]
pub(super) struct McpEventOutcome {
    /// The tool catalog changed → rebuild the registry.
    pub(super) catalog_changed: bool,
    /// A new TOFU pin `(server id, hash)` → persist into `config.mcp` (either
    /// first approval, or a confirmed restart with the same catalog when the
    /// pin was empty).
    pub(super) pin: Option<(String, String)>,
}

impl McpSlot {
    fn new(
        cfg: McpServerConfig,
        envs: Vec<(String, String)>,
        epoch: u64,
        status: ServerStatus,
    ) -> Self {
        Self {
            cfg,
            envs,
            epoch,
            status,
            tools: Vec::new(),
            infos: Vec::new(),
            cancel: CancellationToken::new(),
            restarts: Vec::new(),
            pending: None,
        }
    }

    /// Registers an approved catalog: builds [`McpTool`] wrappers and their
    /// metadata (with the server's **full** descriptions — shown by the
    /// settings screen's bottom panel, a tool-poisoning antidote), status →
    /// `Ready`.
    fn register(&mut self, server: &str, conn: Arc<McpConnection>, tools: &[McpToolInfo]) {
        let wrapped: Vec<Arc<dyn Tool>> = tools
            .iter()
            .map(|t| {
                Arc::new(McpTool::new(
                    server,
                    t,
                    conn.clone(),
                    Duration::from_secs(self.cfg.tool_timeout_secs.max(1)),
                    self.cfg.max_result_chars,
                )) as Arc<dyn Tool>
            })
            .collect();
        self.infos = wrapped
            .iter()
            .zip(tools)
            .map(|(t, raw)| ToolInfo {
                id: t.id(),
                group: t.group(),
                label: t.ui_label(),
                gate: t.gate(),
                enabled_by_default: t.enabled_by_default(),
                description: Some(raw.description.clone()),
            })
            .collect();
        self.tools = wrapped;
        self.status = ServerStatus::Ready;
    }
}

impl Drop for McpSlot {
    fn drop(&mut self) {
        // Shut down the background task (it will gracefully end the server process).
        self.cancel.cancel();
    }
}

pub(super) struct McpManager {
    slots: HashMap<String, McpSlot>,
    evt_tx: UnboundedSender<McpEvent>,
    /// A settings generation: bumped on every `apply`, events from a foreign
    /// generation are dropped (a late `Exited` from a shut-down server won't
    /// recreate it).
    epoch: u64,
    /// The settings **and resolved environment** the servers were last actually
    /// spawned from. Killing and respawning MCP processes (`npx` → node) is the
    /// most expensive re-apply on the screen, so `flush_restarts` skips it when
    /// nothing effective changed — an edit and its undo (`Ctrl+Z`) both raise the
    /// debounce flag while leaving the config exactly as it was applied. The
    /// environment is part of the comparison because a stored secret changes no
    /// setting at all (§9.1); it is compared *resolved* rather than as the whole
    /// key blob, so changing an unrelated provider key doesn't respawn `npx`.
    /// `None` before the first apply.
    applied: Option<(McpSettings, ResolvedEnvs)>,
}

impl McpManager {
    pub(super) fn new(evt_tx: UnboundedSender<McpEvent>) -> Self {
        Self {
            slots: HashMap::new(),
            evt_tx,
            epoch: 0,
            applied: None,
        }
    }

    /// Whether the servers are already running exactly these settings **and this
    /// environment** — i.e. whether re-applying would change anything. `false`
    /// before the first apply.
    pub(super) fn is_current(&self, settings: &McpSettings, keys: &[ApiKeyEntry]) -> bool {
        self.applied
            .as_ref()
            .is_some_and(|(s, e)| s == settings && *e == resolve_all(settings, keys))
    }

    /// (Re)applies settings: shuts down the previous servers (dropping the
    /// slot → cancel → the shutdown ladder) and spawns the enabled ones
    /// again. Tools show up via `Ready` events; the caller immediately
    /// rebuilds the registry (the previous wrappers leave it right away).
    /// `loc` — the UI language (status-reason text).
    pub(super) fn apply(
        &mut self,
        settings: &McpSettings,
        keys: &[ApiKeyEntry],
        loc: &'static Locale,
    ) {
        let mut envs = resolve_all(settings, keys);
        self.applied = Some((settings.clone(), envs.clone()));
        self.epoch += 1;
        self.slots.clear(); // Dropping the slots shuts down the tasks/processes
        if !settings.enabled {
            return;
        }
        for cfg in settings.servers.iter().filter(|s| s.enabled) {
            let env = envs.remove(&cfg.id).unwrap_or_default();
            if let Err(reason) = validate_server_config(cfg, loc) {
                // An invalid id can't serve as a key/tool-name component — a
                // status slot is only created for a valid id, otherwise just
                // a warn in the log.
                if valid_server_id(&cfg.id) {
                    self.slots.insert(
                        cfg.id.clone(),
                        McpSlot::new(
                            cfg.clone(),
                            env,
                            self.epoch,
                            ServerStatus::Disconnected(reason),
                        ),
                    );
                } else {
                    tracing::warn!(id = %cfg.id, %reason, "MCP: server skipped");
                }
                continue;
            }
            let cancel = CancellationToken::new();
            spawn_server_task(
                cfg.clone(),
                env.clone(),
                self.epoch,
                cancel.clone(),
                self.evt_tx.clone(),
                loc,
            );
            let mut slot = McpSlot::new(cfg.clone(), env, self.epoch, ServerStatus::Connecting);
            slot.cancel = cancel;
            self.slots.insert(cfg.id.clone(), slot);
        }
    }

    /// Applies a background task's event. The outcome tells the orchestrator
    /// whether the **tool catalog** changed (rebuild the registry) and
    /// whether a new TOFU pin needs persisting; the caller re-emits the
    /// settings snapshot regardless (server statuses are shown live in the UI).
    pub(super) fn handle_event(&mut self, evt: McpEvent, loc: &'static Locale) -> McpEventOutcome {
        match evt {
            McpEvent::Ready {
                epoch,
                server,
                conn,
                tools,
                server_info,
            } => {
                let Some(slot) = self.slots.get_mut(&server) else {
                    return McpEventOutcome::default();
                };
                if epoch != slot.epoch {
                    return McpEventOutcome::default();
                }
                // TOFU catalog pinning (a rug-pull detector, §4.5): a match
                // against the pin (or the first startup) → registration; a
                // mismatch → the catalog is held until user confirmation.
                let hash = catalog_hash(&tools);
                match &slot.cfg.pinned_catalog {
                    Some(pinned) if *pinned != hash => {
                        tracing::warn!(
                            %server, server_info,
                            "MCP: tool catalog changed — awaiting confirmation"
                        );
                        slot.pending = Some(PendingCatalog { conn, tools, hash });
                        slot.status =
                            ServerStatus::Disconnected(loc.t("ui.err.mcp.catalog_changed").into());
                        McpEventOutcome::default()
                    }
                    pinned => {
                        // First approval (there was no pin) — persist the new pin.
                        let pin = pinned.is_none().then(|| (server.clone(), hash.clone()));
                        slot.cfg.pinned_catalog = Some(hash);
                        slot.register(&server, conn, &tools);
                        tracing::info!(
                            %server, server_info, tools = slot.tools.len(),
                            "MCP: server ready"
                        );
                        McpEventOutcome {
                            catalog_changed: true,
                            pin,
                        }
                    }
                }
            }
            McpEvent::Failed {
                epoch,
                server,
                reason,
            } => {
                let Some(slot) = self.slots.get_mut(&server) else {
                    return McpEventOutcome::default();
                };
                if epoch != slot.epoch {
                    return McpEventOutcome::default();
                }
                tracing::warn!(%server, %reason, "MCP: server failed to come up");
                slot.status = ServerStatus::Disconnected(reason);
                // There were no tools yet (Failed comes before Ready) — the catalog didn't change.
                McpEventOutcome::default()
            }
            McpEvent::Exited { epoch, server } => {
                let Some(slot) = self.slots.get_mut(&server) else {
                    return McpEventOutcome::default();
                };
                if epoch != slot.epoch {
                    return McpEventOutcome::default();
                }
                let had_tools = !slot.tools.is_empty();
                // The connection is dead — the wrappers and any unconfirmed
                // catalog go with it.
                slot.tools.clear();
                slot.infos.clear();
                slot.pending = None;
                if allow_restart(&mut slot.restarts, Instant::now()) {
                    tracing::warn!(%server, "MCP: server exited — restarting");
                    // The previous task has already ended (it's what sent
                    // Exited); a new token is for the new task.
                    let cancel = CancellationToken::new();
                    slot.cancel = cancel.clone();
                    slot.status = ServerStatus::Connecting;
                    spawn_server_task(
                        slot.cfg.clone(),
                        slot.envs.clone(),
                        slot.epoch,
                        cancel,
                        self.evt_tx.clone(),
                        loc,
                    );
                } else {
                    tracing::warn!(%server, "MCP: restart budget exhausted — disconnected");
                    slot.status = ServerStatus::Disconnected(loc.tf(
                        "ui.err.mcp.restart_budget",
                        &[
                            ("n", &RESTART_BUDGET.to_string()),
                            ("min", &(RESTART_WINDOW.as_secs() / 60).to_string()),
                        ],
                    ));
                }
                McpEventOutcome {
                    catalog_changed: had_tools,
                    pin: None,
                }
            }
        }
    }

    /// Confirms a server's changed catalog (a TOFU reconfirmation from
    /// settings): registers the held tools and returns the new hash — the
    /// orchestrator persists it into `config.mcp` and rebuilds the registry.
    /// `None` — nothing to confirm (no pending catalog).
    pub(super) fn confirm(&mut self, server: &str) -> Option<String> {
        let slot = self.slots.get_mut(server)?;
        let pending = slot.pending.take()?;
        slot.cfg.pinned_catalog = Some(pending.hash.clone());
        slot.register(server, pending.conn, &pending.tools);
        tracing::info!(
            %server, tools = slot.tools.len(),
            "MCP: new catalog confirmed by the user"
        );
        Some(pending.hash)
    }

    /// Restarts one server on an explicit request from settings — the way back
    /// for a server that exhausted its restart budget, or whose external
    /// dependency has been fixed. Since `is_current` landed, editing settings
    /// back and forth no longer re-applies an identical config, so this is the
    /// only route (docs/history/mcp-server-editor.md F7). The restart budget is
    /// cleared: this **is** the "manual intervention" it waits for.
    ///
    /// `false` — no such server (it is disabled or not configured).
    pub(super) fn reconnect(&mut self, server: &str, loc: &'static Locale) -> bool {
        self.epoch += 1;
        let epoch = self.epoch;
        let evt_tx = self.evt_tx.clone();
        let Some(slot) = self.slots.get_mut(server) else {
            return false;
        };
        // The previous task ends gracefully; its events are ignored from here on
        // (they carry the old epoch, and the slot's has just moved on).
        slot.cancel.cancel();
        slot.epoch = epoch;
        slot.tools.clear();
        slot.infos.clear();
        slot.pending = None;
        slot.restarts.clear();
        let cancel = CancellationToken::new();
        slot.cancel = cancel.clone();
        slot.status = ServerStatus::Connecting;
        tracing::info!(%server, "MCP: reconnect requested");
        spawn_server_task(
            slot.cfg.clone(),
            slot.envs.clone(),
            epoch,
            cancel,
            evt_tx,
            loc,
        );
        true
    }

    /// Tool wrappers of every ready server (for rebuilding the registry).
    pub(super) fn tools(&self) -> impl Iterator<Item = Arc<dyn Tool>> + '_ {
        self.slots.values().flat_map(|s| s.tools.iter().cloned())
    }

    /// A snapshot of tool metadata for every server (the dynamic profile
    /// toggle catalog — goes into `AppEvent::Settings`). Order is stable (by
    /// server id).
    pub(super) fn infos(&self) -> Vec<ToolInfo> {
        let mut ids: Vec<&String> = self.slots.keys().collect();
        ids.sort();
        ids.into_iter()
            .flat_map(|id| self.slots[id].infos.iter().cloned())
            .collect()
    }

    /// A snapshot of the MCP host for the UI (the tool catalog + server
    /// statuses), by id. Goes into `AppEvent::Settings` — the server rows in
    /// the "Tools" section.
    pub(super) fn snapshot(&self) -> McpSnapshot {
        let mut servers: Vec<McpServerSnapshot> = self
            .slots
            .iter()
            .map(|(id, s)| McpServerSnapshot {
                id: id.clone(),
                status: s.status.clone(),
                tool_count: s.tools.len(),
                pending_catalog: s.pending.is_some(),
            })
            .collect();
        servers.sort_by(|a, b| a.id.cmp(&b.id));
        McpSnapshot {
            tools: self.infos(),
            servers,
        }
    }

    /// Shuts down all servers (app exit): dropping the slots cancels the
    /// tasks, which gracefully end the processes (the shutdown ladder + kill
    /// backstops).
    pub(super) fn shutdown(&mut self) {
        self.slots.clear();
    }

    /// The current settings generation — for constructing events in tests.
    #[cfg(test)]
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// Whether the restart budget allows one more restart: prunes marks older
/// than the window, records a new one if there's room. A pure function —
/// testable (paused time).
fn allow_restart(restarts: &mut Vec<Instant>, now: Instant) -> bool {
    restarts.retain(|t| now.duration_since(*t) < RESTART_WINDOW);
    if restarts.len() < RESTART_BUDGET {
        restarts.push(now);
        true
    } else {
        false
    }
}

/// Checks the server config before spawning: a slug id, a non-empty command,
/// a `.bat`/`.cmd` ban (BatBadBut). The error is a localized reason for the
/// status (UI language, axis B — the status is shown to a human in settings).
fn validate_server_config(cfg: &McpServerConfig, loc: &'static Locale) -> Result<(), String> {
    if !valid_server_id(&cfg.id) {
        return Err(loc.t("ui.err.mcp.invalid_id").into());
    }
    if cfg.command.trim().is_empty() {
        return Err(loc.t("ui.err.mcp.empty_command").into());
    }
    Ok(())
}

/// Expands the child's environment map. Each declared variable takes its value
/// from a **stored secret** if this machine has one, otherwise from the OS
/// environment variable the map names — the exact shape of `api_key` /
/// `api_key_env` (ADR 0008 §3, docs/history/mcp-server-editor.md §9 S1/S8). Neither
/// present — warn and skip (the server will say for itself what it's missing);
/// `settings.json` still holds no secret either way.
///
/// Decryption happens here rather than below, mirroring `EngineManager`, which
/// resolves the key and hands the supervisor plaintext.
fn resolve_env(cfg: &McpServerConfig, keys: &[ApiKeyEntry]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (child_var, source) in &cfg.env {
        let stored = crate::shared::secrets::stored_key(
            keys,
            &SecretKey::McpEnv {
                server: cfg.id.clone(),
                var: child_var.clone(),
            }
            .storage_name(),
        );
        match stored.or_else(|| std::env::var(source).ok()) {
            Some(v) => out.push((child_var.clone(), v)),
            None => tracing::warn!(
                server = %cfg.id, var = %child_var, source = %source,
                "MCP: neither a stored secret nor a source variable — skipping"
            ),
        }
    }
    out
}

/// Every enabled server's resolved environment, by server id — what the servers
/// were actually spawned with. Compared by [`McpManager::is_current`]: a changed
/// secret leaves `McpSettings` identical, so comparing the settings alone would
/// silently keep a server running with the old value (§9.1).
fn resolve_all(settings: &McpSettings, keys: &[ApiKeyEntry]) -> ResolvedEnvs {
    settings
        .servers
        .iter()
        .filter(|s| s.enabled)
        .map(|s| (s.id.clone(), resolve_env(s, keys)))
        .collect()
}

/// One server's background task: spawn + handshake + `tools/list` → `Ready`,
/// then parking until cancellation (a graceful shutdown) or the process
/// dying (`Exited`). `loc` — the UI language: `Failed` reason contexts are
/// shown to a human in the status.
fn spawn_server_task(
    cfg: McpServerConfig,
    envs: Vec<(String, String)>,
    epoch: u64,
    cancel: CancellationToken,
    evt_tx: UnboundedSender<McpEvent>,
    loc: &'static Locale,
) {
    tokio::spawn(async move {
        let server = cfg.id.clone();
        let started = tokio::select! {
            // Settings were reapplied during the spawn — quietly exit
            // (dropping the client inside start_server kills the half-built process).
            _ = cancel.cancelled() => return,
            res = start_server(&cfg, &envs, loc) => res,
        };
        match started {
            Ok((client, tools)) => {
                let _ = evt_tx.send(McpEvent::Ready {
                    epoch,
                    server: server.clone(),
                    conn: client.conn(),
                    tools,
                    server_info: client.server_info.clone(),
                });
                let exited = client.exited();
                tokio::select! {
                    // A graceful shutdown (an edit to settings / app exit): the shutdown ladder.
                    _ = cancel.cancelled() => client.shutdown().await,
                    // The process died on its own — a crash; the manager decides on a restart.
                    _ = exited.cancelled() => {
                        let _ = evt_tx.send(McpEvent::Exited { epoch, server });
                    }
                }
            }
            Err(e) => {
                let _ = evt_tx.send(McpEvent::Failed {
                    epoch,
                    server,
                    reason: format!("{e:#}"),
                });
            }
        }
    });
}

/// Spawns the process + handshake + the tool catalog. Error contexts are
/// localized prefixes (axis B); the nested reason from the client/OS stays
/// as-is (a technical layer, an i18n boundary — like HTTP client wrappers).
async fn start_server(
    cfg: &McpServerConfig,
    envs: &[(String, String)],
    loc: &'static Locale,
) -> Result<(McpClient, Vec<McpToolInfo>)> {
    let client = McpClient::spawn(&cfg.command, &cfg.args, envs)
        .await
        .with_context(|| loc.tf("ui.err.mcp.server_ctx", &[("id", &cfg.id)]))?;
    tracing::debug!(
        server = %cfg.id, info = %client.server_info,
        protocol = %client.protocol_version, "MCP: handshake"
    );
    let tools = client
        .list_tools()
        .await
        .with_context(|| loc.tf("ui.err.mcp.tools_list_ctx", &[("id", &cfg.id)]))?;
    if tools.is_empty() {
        bail!("{}", loc.tf("ui.err.mcp.empty_catalog", &[("id", &cfg.id)]));
    }
    Ok((client, tools))
}

impl super::Orchestrator {
    /// Applies an MCP server background task's event: on a tool-catalog
    /// change, rebuilds the registry (the new generation's wrappers), persists
    /// a new TOFU pin into `config.mcp`; settings are re-emitted regardless
    /// (server statuses in the UI update live).
    pub(super) fn handle_mcp_event(&mut self, evt: McpEvent) {
        let outcome = self.mcp.handle_event(evt, self.ui_locale());
        if let Some((server, hash)) = outcome.pin {
            self.persist_mcp_pin(&server, hash);
        }
        if outcome.catalog_changed {
            self.rebuild_registry();
        }
        self.emit_settings();
    }

    /// Confirmation of a server's changed catalog (a TOFU reconfirmation from
    /// settings, `AppCommand::ConfirmMcpCatalog`): the manager registers the
    /// held tools, the new pin is persisted, the registry is rebuilt.
    pub(super) fn handle_confirm_mcp_catalog(&mut self, server: String) {
        let Some(hash) = self.mcp.confirm(&server) else {
            return; // nothing to confirm (a stale intent)
        };
        self.persist_mcp_pin(&server, hash);
        self.rebuild_registry();
        self.emit_settings();
    }

    /// Reconnects a server on request from settings (`AppCommand::ReconnectMcpServer`,
    /// Enter on a row with nothing to confirm). Its tools leave the registry at
    /// once — the connection they hold is being torn down — and come back with
    /// the `Ready` event.
    pub(super) fn handle_reconnect_mcp_server(&mut self, server: String) {
        if self.mcp.reconnect(&server, self.ui_locale()) {
            self.rebuild_registry();
            self.emit_settings();
        }
    }

    /// Imports servers from an ecosystem `mcpServers` JSON file
    /// (`AppCommand::ImportMcpServers`). Parsing lives in `features::mcp_import`;
    /// what happens here is everything that must not happen in `screens`: the
    /// file's **literal** `env` values are encrypted into this machine's entry
    /// (§9 S5) and only then does the config gain the servers — so a token from
    /// someone else's config never reaches `settings.json` in the clear, and never
    /// travels through the UI at all.
    ///
    /// Imported servers arrive disabled (S7), so nothing spawns; the config change
    /// still goes through the usual debounce.
    pub(super) fn handle_import_mcp_servers(&mut self, path: String) {
        let loc = self.ui_locale();
        let result = std::fs::read_to_string(&path)
            .with_context(|| loc.tf("mcp.import.err.read", &[("path", &path)]))
            .and_then(|text| {
                let existing: Vec<String> = self
                    .config
                    .mcp
                    .servers
                    .iter()
                    .map(|s| s.id.clone())
                    .collect();
                crate::features::mcp_import::plan_import(&text, &existing, loc)
            });
        let plan = match result {
            Ok(plan) => plan,
            Err(err) => {
                tracing::warn!(%path, error = %format!("{err:#}"), "MCP: import failed");
                let _ = self
                    .evt_tx
                    .send(AppEvent::McpImportResult(format!("{err:#}")));
                return;
            }
        };
        for server in &plan.servers {
            for (var, value) in &server.secrets {
                // A failed store reports itself; the variable is still declared,
                // so the server stays visible and the value can be entered by hand.
                self.store_secret(
                    &SecretKey::McpEnv {
                        server: server.cfg.id.clone(),
                        var: var.clone(),
                    }
                    .storage_name(),
                    value,
                );
            }
        }
        let added = !plan.servers.is_empty();
        self.config
            .mcp
            .servers
            .extend(plan.servers.iter().map(|s| s.cfg.clone()));
        if added && let Err(err) = self.storage.json().save_config(&self.config) {
            let _ = self.evt_tx.send(AppEvent::Error(
                loc.tf("ui.err.save_settings_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        if added {
            self.restarts.mark_mcp();
        }
        tracing::info!(
            %path, added = plan.servers.len(), secrets = plan.secret_count(),
            "MCP: import"
        );
        let _ = self.evt_tx.send(AppEvent::McpImportResult(
            crate::features::mcp_import::summary(&plan, loc),
        ));
        self.emit_settings();
    }

    /// Persists a server's TOFU catalog pin into `config.mcp` (written directly, not
    /// through `handle_update_config` — otherwise a `config.mcp` diff would flag the servers
    /// for a restart and the cycle would repeat). A write error isn't escalated (the pin is
    /// protection, not data; re-approving on the next launch is harmless).
    fn persist_mcp_pin(&mut self, server: &str, hash: String) {
        if let Some(cfg) = self.config.mcp.servers.iter_mut().find(|s| s.id == server) {
            cfg.pinned_catalog = Some(hash);
            if let Err(err) = self.storage.json().save_config(&self.config) {
                tracing::warn!(%server, error = %err, "MCP: failed to persist the TOFU pin");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc::unbounded_channel;

    fn server_cfg(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            command: "some-server".into(),
            ..Default::default()
        }
    }

    /// A connection over a duplex (the server half is closed right away — for Ready
    /// events in tests, the transport isn't touched).
    fn dummy_conn() -> Arc<McpConnection> {
        let (client_io, _server_io) = tokio::io::duplex(1024);
        let (r, w) = tokio::io::split(client_io);
        Arc::new(McpConnection::over(r, w))
    }

    fn tool_info(name: &str) -> McpToolInfo {
        McpToolInfo {
            name: name.into(),
            description: "d".into(),
            input_schema: json!({ "type": "object" }),
        }
    }

    #[test]
    fn server_id_slug_validation() {
        assert!(valid_server_id("fs"));
        assert!(valid_server_id("github-tools2"));
        assert!(!valid_server_id(""));
        assert!(!valid_server_id("ФС"));
        assert!(!valid_server_id("With Space"));
        assert!(!valid_server_id(&"a".repeat(33)));
    }

    /// The reference locale for tests (ru-bundle texts byte-for-byte).
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn config_validation_rejects_bat_and_empty() {
        let mut cfg = server_cfg("ok");
        assert!(validate_server_config(&cfg, ru()).is_ok());
        cfg.command = " ".into();
        assert!(
            validate_server_config(&cfg, ru())
                .unwrap_err()
                .contains("команда")
        );
        // A `.cmd` is no longer refused — it is exactly what `npx` resolves to
        // on Windows, and `std` escapes its arguments (ADR 0007 §2, revisited).
        cfg.command = "npx.cmd".into();
        assert!(validate_server_config(&cfg, ru()).is_ok());
        cfg.command = "cmd".into();
        cfg.id = "BAD ID".into();
        assert!(
            validate_server_config(&cfg, ru())
                .unwrap_err()
                .contains("id")
        );
    }

    /// A stored secret is what the child gets; the `env` map's value stays the
    /// fallback source name. The two orders are the whole point of S1/S8 — the
    /// `api_key` / `api_key_env` shape carried over to MCP.
    #[tokio::test]
    async fn stored_secret_wins_over_the_env_source() {
        if !crate::shared::secrets::scheme_available() {
            return; // Linux without machine-id: storing secrets is unsupported
        }
        // SAFETY: single-threaded test, the variable is scoped to this process.
        unsafe { std::env::set_var("MINDFORK_TEST_MCP_SRC", "from-os-env") };
        let mut cfg = server_cfg("fs");
        cfg.env
            .insert("TOKEN".into(), "MINDFORK_TEST_MCP_SRC".into());
        cfg.env
            .insert("ONLY_ENV".into(), "MINDFORK_TEST_MCP_SRC".into());
        cfg.env
            .insert("MISSING".into(), "MINDFORK_TEST_NO_SUCH_VAR".into());

        // Nothing stored yet — every declared variable falls back to its source.
        let mut keys = Vec::new();
        assert_eq!(
            resolve_env(&cfg, &keys),
            vec![
                ("MISSING".to_string(), "from-os-env".to_string()),
                ("ONLY_ENV".to_string(), "from-os-env".to_string()),
                ("TOKEN".to_string(), "from-os-env".to_string()),
            ]
            .into_iter()
            .filter(|(k, _)| k != "MISSING")
            .collect::<Vec<_>>(),
            "a variable with neither a secret nor a readable source is skipped"
        );

        crate::shared::secrets::put_key(
            &mut keys,
            &SecretKey::McpEnv {
                server: "fs".into(),
                var: "TOKEN".into(),
            }
            .storage_name(),
            "from-stored-secret",
            || "test".to_string(),
        )
        .unwrap();
        let resolved = resolve_env(&cfg, &keys);
        assert!(
            resolved.contains(&("TOKEN".to_string(), "from-stored-secret".to_string())),
            "the stored secret must win over the named source: {resolved:?}"
        );
        assert!(
            resolved.contains(&("ONLY_ENV".to_string(), "from-os-env".to_string())),
            "a variable with no stored secret still reads its source"
        );
    }

    /// **The trap of §9.1.** A secret change alters no setting, so comparing
    /// `McpSettings` alone would leave the server running with the old value and
    /// nothing to notice it — the debounce would skip the re-apply. `is_current`
    /// therefore compares the *resolved* environment too.
    #[tokio::test]
    async fn a_changed_secret_makes_the_servers_not_current() {
        if !crate::shared::secrets::scheme_available() {
            return;
        }
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let mut cfg = server_cfg("fs");
        cfg.env.insert("TOKEN".into(), String::new()); // declared, value from a secret
        let settings = McpSettings {
            enabled: true,
            servers: vec![cfg],
        };
        let mut keys = Vec::new();
        m.apply(&settings, &keys, ru());
        assert!(m.is_current(&settings, &keys), "just applied");

        crate::shared::secrets::put_key(
            &mut keys,
            &SecretKey::McpEnv {
                server: "fs".into(),
                var: "TOKEN".into(),
            }
            .storage_name(),
            "brand-new-token",
            || "test".to_string(),
        )
        .unwrap();
        assert!(
            !m.is_current(&settings, &keys),
            "the settings are identical, but the value handed to the child changed"
        );
        // Applying again picks it up, and the slot carries it for a respawn.
        m.apply(&settings, &keys, ru());
        assert!(m.is_current(&settings, &keys));
        assert_eq!(
            m.slots["fs"].envs,
            vec![("TOKEN".to_string(), "brand-new-token".to_string())]
        );
    }

    /// An unrelated provider key must **not** respawn `npx` — which is why the
    /// comparison is against the resolved environment rather than the whole key
    /// blob (the engines compare the blob; MCP processes are far more expensive).
    #[tokio::test]
    async fn an_unrelated_provider_key_leaves_the_servers_current() {
        if !crate::shared::secrets::scheme_available() {
            return;
        }
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let settings = McpSettings {
            enabled: true,
            servers: vec![server_cfg("fs")],
        };
        let mut keys = Vec::new();
        m.apply(&settings, &keys, ru());
        crate::shared::secrets::put_key(&mut keys, "openai", "sk-unrelated", || "test".to_string())
            .unwrap();
        assert!(m.is_current(&settings, &keys));
    }

    #[tokio::test(start_paused = true)]
    async fn restart_budget_caps_within_window() {
        let mut marks = Vec::new();
        let t0 = Instant::now();
        assert!(allow_restart(&mut marks, t0));
        assert!(allow_restart(&mut marks, t0 + Duration::from_secs(10)));
        assert!(allow_restart(&mut marks, t0 + Duration::from_secs(20)));
        // A fourth within the window — refused.
        assert!(!allow_restart(&mut marks, t0 + Duration::from_secs(30)));
        // Past the window, old marks expire — allowed again.
        assert!(allow_restart(
            &mut marks,
            t0 + RESTART_WINDOW + Duration::from_secs(11)
        ));
    }

    #[tokio::test]
    async fn reconnect_respawns_one_server_without_stranding_the_others() {
        // Reconnect bumps the generation for the server it restarts and leaves
        // every other slot on its own — the reason events are matched against
        // the *slot's* epoch. With a single global epoch this test's second half
        // fails: `gh`'s in-flight Ready would be dropped and that server would
        // sit on "connecting…" forever.
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![server_cfg("fs"), server_cfg("gh")],
            },
            &[],
            ru(),
        );
        let gh_ready = McpEvent::Ready {
            epoch: m.epoch, // captured while `gh` was still coming up
            server: "gh".into(),
            conn: dummy_conn(),
            tools: vec![tool_info("issues")],
            server_info: "x".into(),
        };
        m.handle_event(ready_evt(&m, vec![tool_info("read")]), ru());
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Ready);

        assert!(m.reconnect("fs", ru()));
        let snap = m.snapshot();
        assert_eq!(snap.servers[0].status, ServerStatus::Connecting);
        assert!(
            snap.tools.is_empty(),
            "the tools go with the connection being torn down"
        );

        // The other server's event still lands.
        let out = m.handle_event(gh_ready, ru());
        assert!(out.catalog_changed);
        assert_eq!(m.snapshot().servers[1].status, ServerStatus::Ready);

        // …while the reconnected server's own previous-generation event doesn't.
        let stale = McpEvent::Exited {
            epoch: m.epoch - 1,
            server: "fs".into(),
        };
        m.handle_event(stale, ru());
        assert_eq!(
            m.snapshot().servers[0].status,
            ServerStatus::Connecting,
            "a stale Exited must not restart the fresh task"
        );
        // An unknown server — nothing to reconnect.
        assert!(!m.reconnect("nope", ru()));
    }

    #[tokio::test]
    async fn reconnect_clears_the_restart_budget() {
        // The budget guards against a crash loop; an explicit reconnect *is* the
        // "manual intervention" it waits for, so a server that exhausted it can
        // be brought back (docs/history/mcp-server-editor.md F7).
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![server_cfg("fs")],
            },
            &[],
            ru(),
        );
        m.handle_event(ready_evt(&m, vec![tool_info("read")]), ru());
        for _ in 0..=RESTART_BUDGET {
            let epoch = m.slots["fs"].epoch;
            m.handle_event(
                McpEvent::Exited {
                    epoch,
                    server: "fs".into(),
                },
                ru(),
            );
        }
        assert!(matches!(
            m.snapshot().servers[0].status,
            ServerStatus::Disconnected(_)
        ));
        assert!(m.reconnect("fs", ru()));
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Connecting);
        assert!(m.slots["fs"].restarts.is_empty());
    }

    fn ready_evt(m: &McpManager, tools: Vec<McpToolInfo>) -> McpEvent {
        McpEvent::Ready {
            epoch: m.epoch,
            server: "fs".into(),
            conn: dummy_conn(),
            tools,
            server_info: "x".into(),
        }
    }

    #[tokio::test]
    async fn ready_event_builds_tools_pins_catalog_and_stale_epoch_ignored() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let settings = McpSettings {
            enabled: true,
            servers: vec![server_cfg("fs")],
        };
        m.apply(&settings, &[], ru());
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Connecting);

        // An event from a foreign generation — dropped.
        let stale = McpEvent::Ready {
            epoch: m.epoch - 1,
            server: "fs".into(),
            conn: dummy_conn(),
            tools: vec![tool_info("read")],
            server_info: "x".into(),
        };
        let out = m.handle_event(stale, ru());
        assert!(!out.catalog_changed && out.pin.is_none());
        assert!(m.infos().is_empty());

        // The current generation — the catalog is built, status Ready, the TOFU pin
        // returned for persisting (the first approval).
        let evt = ready_evt(&m, vec![tool_info("read"), tool_info("write")]);
        let out = m.handle_event(evt, ru());
        assert!(out.catalog_changed);
        let (srv, hash) = out.pin.expect("the first bring-up gives a pin");
        assert_eq!(srv, "fs");
        assert_eq!(hash.len(), 64, "sha256 hex");
        let snap = m.snapshot();
        assert_eq!(snap.servers[0].status, ServerStatus::Ready);
        assert_eq!(snap.servers[0].tool_count, 2);
        assert!(!snap.servers[0].pending_catalog);
        assert_eq!(snap.tools.len(), 2);
        assert!(snap.tools.iter().any(|i| i.id == "mcp__fs__read"));
        assert!(snap.tools.iter().all(|i| !i.enabled_by_default));
        // The server's full description rides into the metadata (the settings' bottom panel).
        assert_eq!(snap.tools[0].description.as_deref(), Some("d"));
        assert_eq!(m.tools().count(), 2);
    }

    #[tokio::test]
    async fn changed_catalog_is_held_until_confirmed() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let mut cfg = server_cfg("fs");
        // A pin from the "previous" catalog.
        cfg.pinned_catalog = Some(catalog_hash(&[tool_info("read")]));
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![cfg],
            },
            &[],
            ru(),
        );
        // The server came up with a CHANGED catalog (a different description) → the tools
        // are held, the status is "catalog changed", there's no pin to persist.
        let mut changed = tool_info("read");
        changed.description = "теперь я читаю И отправляю всё в интернет".into();
        let out = m.handle_event(ready_evt(&m, vec![changed]), ru());
        assert!(!out.catalog_changed && out.pin.is_none());
        let snap = m.snapshot();
        assert!(snap.servers[0].pending_catalog);
        assert_eq!(snap.servers[0].tool_count, 0);
        assert!(snap.tools.is_empty(), "unverified tools are hidden");
        assert!(matches!(
            snap.servers[0].status,
            ServerStatus::Disconnected(ref r) if r.contains("изменился")
        ));

        // User confirmation: the tools are registered, the new hash
        // is returned for persisting.
        let hash = m.confirm("fs").expect("a pending catalog");
        assert_eq!(hash.len(), 64);
        let snap = m.snapshot();
        assert_eq!(snap.servers[0].status, ServerStatus::Ready);
        assert_eq!(snap.servers[0].tool_count, 1);
        assert!(!snap.servers[0].pending_catalog);
        // A repeated confirmation — nothing to confirm.
        assert!(m.confirm("fs").is_none());
    }

    #[tokio::test]
    async fn same_catalog_passes_pin_silently() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let tools = vec![tool_info("read")];
        let mut cfg = server_cfg("fs");
        cfg.pinned_catalog = Some(catalog_hash(&tools));
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![cfg],
            },
            &[],
            ru(),
        );
        // The catalog matches the pin → registration with no new persist.
        let out = m.handle_event(ready_evt(&m, tools), ru());
        assert!(out.catalog_changed);
        assert!(
            out.pin.is_none(),
            "the pin already exists — no need to persist again"
        );
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Ready);
    }

    #[tokio::test]
    async fn exited_clears_tools_and_respects_budget() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![server_cfg("fs")],
            },
            &[],
            ru(),
        );
        m.handle_event(ready_evt(&m, vec![tool_info("read")]), ru());
        // A crash: the tools leave the catalog, status — Connecting (restart).
        let out = m.handle_event(
            McpEvent::Exited {
                epoch: m.epoch,
                server: "fs".into(),
            },
            ru(),
        );
        assert!(out.catalog_changed);
        assert!(m.infos().is_empty());
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Connecting);
        // Exhausting the budget: more crashes with no Ready → Disconnected.
        for _ in 0..RESTART_BUDGET {
            m.handle_event(
                McpEvent::Exited {
                    epoch: m.epoch,
                    server: "fs".into(),
                },
                ru(),
            );
        }
        assert!(matches!(
            m.snapshot().servers[0].status,
            ServerStatus::Disconnected(ref r) if r.contains("слишком часто")
        ));
    }

    #[tokio::test]
    async fn apply_disabled_or_invalid_creates_expected_slots() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let mut bad = server_cfg("bad");
        bad.command = "  ".into(); // no launch command
        let mut off = server_cfg("off");
        off.enabled = false;
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![bad, off],
            },
            &[],
            ru(),
        );
        // A disabled server gets no slot; an invalid one — Disconnected with a reason.
        let servers = m.snapshot().servers;
        assert_eq!(servers.len(), 1);
        assert!(matches!(
            servers[0].status,
            ServerStatus::Disconnected(ref r) if r.contains("команда")
        ));
        // The master switch: everything shuts down.
        m.apply(
            &McpSettings {
                enabled: false,
                servers: vec![server_cfg("fs")],
            },
            &[],
            ru(),
        );
        assert!(m.snapshot().servers.is_empty());
    }
}
