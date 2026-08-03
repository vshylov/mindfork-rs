//! Orchestrator: the sole owner of domain state (profiles/chats, [`Storage`])
//! and of the generation state machine. Accepts [`AppCommand`], runs
//! generation (in a separate task), and dispatches [`AppEvent`].
//! See spec §4.4 (unidirectional flow, `generation_id`, the
//! `Idle/Generating/Cancelling` state machine) and §4.4.2 (the orchestrator is
//! the sole writer).
//!
//! The module is split by feature (the god object was broken up, `Chat`
//! still has a single owner):
//! - [`mod.rs`](self) — the skeleton: [`Orchestrator`], the [`run`] loop, the
//!   [`Orchestrator::handle_command`] dispatcher, shared helpers (emitters, `chat_mut`);
//! - [`engines`] — [`EngineManager`]: server lifecycle and readiness;
//! - [`save_queue`] — [`SaveQueue`]: debounce for deferred chat saves;
//! - [`restart_queue`] — [`RestartQueue`]: debounce for (re)launching servers
//!   on engine-settings edits;
//! - [`generation`] — send/regenerate/delete exchange + the agentic-loop task;
//! - [`chats`] — managing the chat list and the draft;
//! - [`profiles`] — creating/editing/deleting profiles;
//! - [`settings`] — config and (re)launching servers via the supervisor;
//! - [`title`] — auto-titling a chat (a background task);
//! - [`impersonation`] — writing a message "on the user's behalf" (a background task);
//! - [`rag`] — indexing/removing files in the knowledge base;
//! - [`search`] — the chat-content search index (`cache.db`): startup
//!   reconciliation, the post-save hook, content queries;
//! - [`tts`] — speaking chat messages (the `/tts` command);
//! - [`request`] — mapping domain messages to the engine's format.

mod attachments;
mod background;
mod chats;
mod consolidation;
mod embed_guard;
mod engines;
mod generation;
mod impersonation;
mod mcp;
mod profiles;
mod rag;
mod reembed;
mod reflection;
mod request;
mod restart_queue;
mod save_queue;
mod search;
mod self_consolidation;
mod settings;
mod title;
mod tool_loop;
mod tts;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;
use uuid::Uuid;

use crate::app::events::{
    AppCommand, AppEvent, BackgroundKind, FeedFocus, ServerStatus, ToolDecision,
};
use crate::app::gen_state::GenState;
use crate::app::supervisor::ServerSupervisor;
use crate::entities::chat::{Chat, ChatSummary};
use crate::entities::message::Message;
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::FinishReason;
use crate::shared::config::{AppConfig, CloudProvider};
use crate::shared::secrets::SecretKey;
use crate::shared::storage::Storage;

use self::attachments::AttachResult;
use self::background::BgSlot;
use self::engines::EngineManager;
use self::generation::GenResult;
use self::mcp::{McpEvent, McpManager};
use self::restart_queue::RestartQueue;
use self::save_queue::SaveQueue;
use self::title::TitleResult;

/// Default profile in language `lang` (bootstrap on empty storage / a
/// protective fallback). Name and system message — from the agent-scaffold
/// bundle (`defaults.*`, axis A, docs/history/i18n.md); the language is set
/// on the profile.
fn default_profile(lang: crate::shared::i18n::Lang) -> Profile {
    let loc = crate::shared::i18n::locale(lang);
    let mut profile = Profile::new(
        loc.t("defaults.profile_name"),
        loc.t("defaults.system_message"),
    );
    profile.language = lang;
    profile
}

/// Orchestrator launch parameters. The orchestrator configures the servers
/// (chat/embedding) and the tool registry itself from [`AppConfig`] via
/// [`ServerSupervisor`] — this lets it restart them on settings edits (spec
/// §11.6).
pub struct OrchestratorDeps {
    pub cmd_rx: UnboundedReceiver<AppCommand>,
    pub evt_tx: UnboundedSender<AppEvent>,
    pub storage: Arc<Storage>,
    /// The full app configuration (the orchestrator is its sole writer).
    pub config: AppConfig,
    /// Supervisor for the inference/embedding servers (real or a mock in tests).
    pub supervisor: Arc<dyn ServerSupervisor>,
    /// Agent-scaffold language for new profiles (from `defaults.json`, axis A).
    /// In tests — `Lang::default()` (`ru`). See docs/history/i18n.md,
    /// `shared::paths::Defaults`.
    pub default_language: crate::shared::i18n::Lang,
}

/// The orchestrator's main loop. Ends when the command channel closes or
/// [`AppCommand::Quit`] is received.
pub async fn run(deps: OrchestratorDeps) {
    let OrchestratorDeps {
        mut cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor,
        default_language,
    } = deps;

    let (done_tx, mut done_rx) = unbounded_channel::<GenResult>();
    // Internal server-status channel: the supervisor's background probe posts
    // readiness (Ready/Disconnected) here, the loop translates it into AppEvent::ServerStatus.
    let (status_tx, mut status_rx) = unbounded_channel::<ServerStatus>();
    // Internal auto-title channel: a background task sends the generated
    // title (or an error), the loop applies it to the chat.
    let (title_tx, mut title_rx) = unbounded_channel::<TitleResult>();
    // Internal status channel for the impersonation server (a background probe).
    let (imp_status_tx, mut imp_status_rx) = unbounded_channel::<ServerStatus>();
    // Internal status channel for the embedding server (a background probe).
    let (embed_status_tx, mut embed_status_rx) = unbounded_channel::<ServerStatus>();
    // Internal "impersonation finished" channel (background task → loop).
    let (imp_done_tx, mut imp_done_rx) = unbounded_channel::<(Uuid, FinishReason)>();
    // A single outcome channel for "silent" background tasks (auto-reflection/
    // consolidation): the task sends `(kind, Ok/Err(reason))`, the loop handles
    // it in one branch via `handle_bg_done`.
    let (bg_done_tx, mut bg_done_rx) = unbounded_channel::<(BackgroundKind, Result<(), String>)>();
    // Internal "speech finished" channel (background task → loop): the loop
    // distinguishes its own outcome from a stale one by the task's generation.
    let (tts_done_tx, mut tts_done_rx) = unbounded_channel::<Uuid>();
    // Internal channel for MCP-server events (spawn/monitor background tasks).
    let (mcp_evt_tx, mut mcp_evt_rx) = unbounded_channel::<McpEvent>();
    // Internal channel for reading/extracting an attached file (`/file attach`):
    // a blocking task sends back the extracted text, the loop inserts it into the chat.
    let (attach_tx, mut attach_rx) = unbounded_channel::<AttachResult>();
    let registry = Arc::new(build_registry(&config, storage.json().sandbox_dir()));
    let mut orch = Orchestrator {
        evt_tx,
        engines: EngineManager::new(supervisor, status_tx, imp_status_tx, embed_status_tx),
        mcp: McpManager::new(mcp_evt_tx),
        imp_cancel: None,
        imp_gen: None,
        imp_done_tx,
        storage,
        config,
        registry,
        title_tx,
        profiles: Vec::new(),
        chats: Vec::new(),
        confirm: None,
        active_id: None,
        gen_state: GenState::Idle,
        done_tx,
        rag_cancel: None,
        attach_tx,
        tts_cancel: None,
        tts_gen: None,
        tts_playback: None,
        tts_done_tx,
        bg: HashMap::new(),
        bg_done_tx,
        consolidate_counts: HashMap::new(),
        self_consolidate_counts: HashMap::new(),
        saves: SaveQueue::default(),
        restarts: RestartQueue::default(),
        default_language,
    };

    // Bring up the servers from config and emit the startup events/settings.
    orch.apply_chat_settings();
    orch.apply_impersonation_settings();
    orch.apply_embed_settings();
    orch.apply_mcp_settings();
    if let Err(err) = orch.bootstrap() {
        let _ = orch.evt_tx.send(AppEvent::Error(
            orch.ui_locale()
                .tf("ui.err.load_data_failed", &[("err", &err.to_string())]),
        ));
    }
    orch.emit_settings();
    // Bring the search index in line with what is on disk — changes made
    // outside the app (import/restore/a hand-edited file/a deleted `cache.db`).
    // A background task, so it never delays the UI; a stat walk of an
    // up-to-date index costs a fraction of a millisecond. Spawned **here**
    // rather than inside `bootstrap`: bootstrap is plain data loading that unit
    // tests call directly, off any runtime, and spawning background work is the
    // loop's business. See [`search`].
    search::spawn_reconcile(orch.storage.clone());

    loop {
        let deadline = orch.saves.deadline();
        let restart_deadline = orch.restarts.deadline();
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break,
                    Some(cmd) => if orch.handle_command(cmd) { break },
                }
            }
            done = done_rx.recv() => {
                if let Some(res) = done {
                    orch.handle_done(res);
                }
            }
            status = status_rx.recv() => {
                if let Some(s) = status {
                    orch.engines.set_chat_status(s);
                    orch.emit_server_status();
                    orch.relaunch_dead_managed_servers();
                }
            }
            title = title_rx.recv() => {
                if let Some(res) = title {
                    orch.handle_title_result(res);
                }
            }
            status = imp_status_rx.recv() => {
                if let Some(s) = status {
                    orch.engines.set_imp_status(s);
                    orch.emit_server_status();
                    orch.relaunch_dead_managed_servers();
                }
            }
            status = embed_status_rx.recv() => {
                if let Some(s) = status {
                    orch.engines.set_embed_status(s);
                    orch.emit_server_status();
                    orch.relaunch_dead_managed_servers();
                }
            }
            done = imp_done_rx.recv() => {
                if let Some((id, reason)) = done {
                    orch.handle_imp_done(id, reason);
                }
            }
            done = bg_done_rx.recv() => {
                if let Some((kind, res)) = done {
                    orch.handle_bg_done(kind, res);
                }
            }
            done = tts_done_rx.recv() => {
                if let Some(task_id) = done {
                    orch.handle_tts_done(task_id);
                }
            }
            evt = mcp_evt_rx.recv() => {
                if let Some(evt) = evt {
                    orch.handle_mcp_event(evt);
                }
            }
            res = attach_rx.recv() => {
                if let Some(res) = res {
                    orch.handle_attach_result(res);
                }
            }
            _ = sleep_until_opt(deadline) => orch.flush_saves(),
            _ = sleep_until_opt(restart_deadline) => orch.flush_restarts(),
        }
    }
    // Deferred restarts on exit are deliberately NOT applied: servers get torn
    // down via Drop/kill_on_drop anyway — no point bringing up a process right
    // before it's dropped.
    orch.flush_saves();
}

/// How many consecutive failures of a background task (reflection/
/// consolidation) must accumulate before showing an error in the UI once.
/// After that — stays quiet until the first success (counter reset).
/// Observability without spam. See the "refinements" stage 5.
pub(super) const BACKGROUND_FAILURE_ALERT: u32 = 3;

/// Builds the tool registry from the configuration (`config.tools`).
/// `sandbox_dir` — the Python sandbox directory (`data/sandbox/`, from
/// [`Paths`]) for Wasmer mode.
fn build_registry(
    config: &AppConfig,
    sandbox_dir: std::path::PathBuf,
) -> crate::features::tools::ToolRegistry {
    crate::features::tools::standard_registry(&crate::features::tools::ToolConfig {
        python_mode: config.tools.python_mode,
        python_path: config.tools.python_path.clone(),
        python_net: config.tools.python_net_enabled,
        python_wasm_timeout: Duration::from_secs(config.tools.python_wasm_timeout_secs),
        python_wasm_memory_mb: config.tools.python_wasm_memory_mb,
        sandbox_dir: Some(sandbox_dir),
        subagent_max_tokens: config.tools.subagent_max_tokens,
        subagent_timeout: Duration::from_secs(config.tools.subagent_timeout_secs),
        web_fetch_content: config.tools.web_fetch_content,
        fs_root: config.tools.fs_root.clone(),
        // The video slot for `youtube_watch`: settings + the shared Gemini key
        // (ADR 0008). Independent of the chat engine — see `shared::video`.
        video: crate::shared::video::resolve_config(
            &config.video,
            crate::shared::secrets::stored_key(
                &config.api_keys,
                crate::shared::config::CloudProvider::Gemini.key(),
            ),
        ),
        // The chat-engine mode determines the sampling parameters available in
        // get_sampling/set_sampling (schema + filtering). See ADR 0004.
        sampling_provider: config.engine.mode.cloud_provider(),
    })
}

/// Sleeps until `deadline`, or "hangs forever" if there's no deadline (an empty queue).
async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending::<()>().await,
    }
}

struct Orchestrator {
    evt_tx: UnboundedSender<AppEvent>,
    /// Inference/embedding servers and their readiness (extracted in Phase 3).
    engines: EngineManager,
    /// MCP servers (plugin tools) and their catalog/statuses (see [`mcp`]).
    mcp: McpManager,
    /// Cancellation token for the current impersonation and its generation_id
    /// (`None` — not running).
    imp_cancel: Option<tokio_util::sync::CancellationToken>,
    imp_gen: Option<Uuid>,
    /// "Impersonation finished" channel (background task → loop).
    imp_done_tx: UnboundedSender<(Uuid, FinishReason)>,
    storage: Arc<Storage>,
    /// The full configuration (the orchestrator is the sole writer of `settings.json`).
    config: AppConfig,
    /// The tool registry (rebuilt on `config.tools` edits).
    registry: Arc<crate::features::tools::ToolRegistry>,
    /// Channel for results of background chat-auto-title generation.
    title_tx: UnboundedSender<TitleResult>,
    profiles: Vec<Profile>,
    /// The in-flight turn's dangerous-tool confirmation channel: its id and the
    /// sender the generation task is listening on (spec §9.8, fork F8 of
    /// docs/history/tool-confirmation.md). `None` between turns.
    confirm: Option<(Uuid, UnboundedSender<(String, ToolDecision)>)>,
    /// Visible chats, entirely in memory (the orchestrator is the sole writer).
    chats: Vec<Chat>,
    active_id: Option<Uuid>,
    /// The lifecycle state machine for assistant-reply generation (see `gen_state`).
    gen_state: GenState,
    done_tx: UnboundedSender<GenResult>,
    /// Cancellation token for the current background RAG indexing (`/rag add`);
    /// `None` — not running. Replaced/cancelled on new indexing and on shutdown.
    rag_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Channel for results of reading/extracting an attached file (`/file attach`,
    /// a blocking task → the loop). See [`attachments`].
    attach_tx: UnboundedSender<AttachResult>,
    /// Cancellation token for the current speech (`/tts`) and its generation
    /// (`None` — not running). Stop points are collected in
    /// [`Orchestrator::stop_tts`] (see [`tts`]).
    tts_cancel: Option<tokio_util::sync::CancellationToken>,
    tts_gen: Option<Uuid>,
    /// Handle to the current speech audio device (shared with the background
    /// task via `Arc`; `Playback` is `Send+Sync`). The orchestrator holds it
    /// for `/tts pause`/`resume`, applied instantly; `None` — speech isn't
    /// running. Dropping it closes the device.
    tts_playback: Option<std::sync::Arc<crate::shared::tts::playback::Playback>>,
    /// "Speech finished" channel (background task → loop).
    tts_done_tx: UnboundedSender<Uuid>,
    /// A registry of "silent" background-task slots (auto-reflection/
    /// consolidation): one slot per [`BackgroundKind`] — a "running" flag
    /// (cancellation token) + a failure streak. Lifecycle — in
    /// [`background`](self::background). Reflection cadence is tracked by the
    /// `Chat.reflected_upto` watermark (survives a restart), not by a field here.
    bg: HashMap<BackgroundKind, BgSlot>,
    /// A single outcome channel for "silent" background tasks (`(kind, Ok/Err(reason))` → loop).
    bg_done_tx: UnboundedSender<(BackgroundKind, Result<(), String>)>,
    /// Assistant-reply counters since the last auto-consolidation of notes (per chat).
    /// Consolidation-cadence data (not task lifecycle — that's in `bg`).
    consolidate_counts: HashMap<Uuid, u32>,
    /// Assistant-reply counters since the last self-model auto-consolidation
    /// ("sleep", per chat). Self-model-sleep cadence data (not lifecycle —
    /// that's in `bg`). See docs/history/self-model-consolidation.md (stage A1).
    self_consolidate_counts: HashMap<Uuid, u32>,
    /// Queue for deferred chat saving (debounce; extracted in Phase 3).
    saves: SaveQueue,
    /// Queue for deferred (re)launch of servers on engine-settings edits
    /// (debounce: a series of quick field edits coalesces into one restart).
    restarts: RestartQueue,
    /// Agent-scaffold language for new profiles (from `defaults.json`, axis A —
    /// docs/history/i18n.md): the first profile's bootstrap and `CreateProfile`
    /// are created in it.
    default_language: crate::shared::i18n::Lang,
}

impl Orchestrator {
    /// Loads profiles/chats, guarantees at least one of each exists, picks the
    /// active chat, and sends the startup events.
    fn bootstrap(&mut self) -> anyhow::Result<()> {
        self.profiles = self
            .storage
            .json()
            .load_profiles()?
            .into_iter()
            .filter(|p| !p.is_hidden)
            .collect();
        // Reconcile profile tools against the current default set: tools added
        // to the application are enabled in existing profiles (ones the user
        // disabled are not). See spec §9.4 and `features::profiles::reconcile_tools`.
        for profile in &mut self.profiles {
            let mut changed = crate::features::profiles::reconcile_tools(profile);
            // Role names became user-visible (feed headers, F5 export): drop the
            // legacy seed values so the labels follow the interface language until
            // the user sets their own. See spec §5.1.
            changed |= crate::features::profiles::clear_seed_character_names(profile);
            if changed {
                let _ = self.storage.json().upsert_profile(profile);
            }
        }
        self.migrate_impersonation_profiles();
        if self.profiles.is_empty() {
            // The first profile — in the scaffold language from defaults.json
            // (filled in by the installer per the user's choice). See
            // docs/history/i18n.md.
            let mut profile = default_profile(self.default_language);
            // Enable all base tools in the default profile.
            crate::features::profiles::reconcile_tools(&mut profile);
            self.storage.json().upsert_profile(&profile)?;
            self.profiles.push(profile);
        }

        self.chats = self
            .storage
            .json()
            .load_chats()?
            .into_iter()
            .filter(|c| !c.is_hidden)
            .collect();
        if self.chats.is_empty() {
            let chat = self.new_chat_value(None);
            self.storage.json().save_chat(&chat)?;
            self.chats.push(chat);
        }
        self.chats.sort_by_key(|c| std::cmp::Reverse(c.modified_at));

        // Restore the last-open chat if it's still visible; otherwise — the
        // most recently modified one (the previous behavior).
        let active = self
            .config
            .last_active_chat
            .filter(|id| self.chats.iter().any(|c| c.id == *id))
            .or_else(|| self.chats.first().map(|c| c.id));
        self.emit_profile_list();
        self.emit_chat_list();
        if let Some(id) = active {
            self.activate(id);
        }
        Ok(())
    }

    /// Handles a command. Returns `true` if the loop should end.
    fn handle_command(&mut self, cmd: AppCommand) -> bool {
        match cmd {
            AppCommand::Quit => {
                if let Some(token) = self.gen_state.active_cancel() {
                    token.cancel();
                }
                if let Some(token) = &self.rag_cancel {
                    token.cancel();
                }
                if let Some(token) = &self.imp_cancel {
                    token.cancel();
                }
                if let Some(token) = &self.tts_cancel {
                    token.cancel();
                }
                self.cancel_all_bg();
                self.mcp.shutdown();
                return true;
            }
            AppCommand::Cancel => {
                if let Some(token) = self.gen_state.request_cancel() {
                    token.cancel();
                }
            }
            AppCommand::SendMessage(text) => self.handle_send(text),
            AppCommand::ConfirmTool {
                generation_id,
                call_id,
                decision,
            } => self.handle_confirm_tool(generation_id, call_id, decision),
            AppCommand::Impersonate { seed } => self.handle_impersonate(seed),
            AppCommand::CancelImpersonation => self.handle_cancel_impersonation(),
            AppCommand::SetDraft(text) => self.handle_set_draft(text),
            AppCommand::SetFeedView(view) => self.handle_set_feed_view(view),
            AppCommand::RegenerateLast => self.handle_regenerate(),
            AppCommand::DeleteLastExchange => self.handle_delete_last(),
            AppCommand::NewChat { profile_id } => self.handle_new_chat(profile_id),
            AppCommand::SwitchChat(id) => self.handle_switch(id),
            AppCommand::OpenChatAt {
                chat,
                message,
                query,
            } => self.handle_open_chat_at(chat, message, query),
            AppCommand::OpenChatAtFirstMatch { chat, query } => {
                self.handle_open_chat_at_first_match(chat, &query)
            }
            AppCommand::RenameChat { id, title } => self.handle_rename(id, title),
            AppCommand::AutoRenameChat(id) => self.handle_auto_rename(id),
            AppCommand::CloneChat(id) => self.handle_clone(id),
            AppCommand::CopyChat(id) => self.handle_copy_chat(id),
            AppCommand::DeleteChat(id) => self.handle_delete(id),
            AppCommand::SearchChats(query) => self.handle_search_chats(query),
            AppCommand::SearchMessages { query, sort } => self.handle_search_messages(query, sort),
            AppCommand::CreateProfile {
                name,
                system_message,
            } => self.handle_create_profile(name, system_message),
            AppCommand::DeleteProfile(id) => self.handle_delete_profile(id),
            AppCommand::UpdateConfig(config) => self.handle_update_config(*config),
            AppCommand::UpdateProfile { id, edit } => self.handle_update_profile(id, *edit),
            AppCommand::RagAdd { path, recursive } => self.handle_rag_add(path, recursive),
            AppCommand::RagDelete { path } => self.handle_rag_delete(path),
            AppCommand::RagList => self.handle_rag_list(),
            AppCommand::RagRebuild => self.handle_rag_rebuild(),
            AppCommand::Reindex => self.handle_reindex(),
            AppCommand::FileAttach { path } => self.handle_file_attach(path),
            AppCommand::FileRemove { target } => self.handle_file_remove(target),
            AppCommand::FileList => self.handle_file_list(),
            AppCommand::Tts(scope) => self.handle_tts(scope),
            AppCommand::TtsStop => self.stop_tts(),
            AppCommand::TtsPause => self.handle_tts_pause(),
            AppCommand::TtsResume => self.handle_tts_resume(),
            AppCommand::RequestSelfModel => self.handle_request_self_model(),
            AppCommand::UpdateSelfModel(edit) => self.handle_update_self_model(edit),
            AppCommand::ConfirmMcpCatalog(server) => self.handle_confirm_mcp_catalog(server),
            AppCommand::ReconnectMcpServer(server) => self.handle_reconnect_mcp_server(server),
            AppCommand::SetSecret { key, value } => self.handle_set_secret(key, value),
            AppCommand::ImportMcpServers(path) => self.handle_import_mcp_servers(path),
        }
        false
    }

    /// Returns a snapshot of the active profile's "self-model" for the viewer
    /// screen (`F3`). Reads the DB in place (fast); `None` — no active chat or
    /// the model hasn't been created yet. A read error is treated as "no
    /// model" (the view shows empty).
    /// A profile's "self-model" snapshot for the `F3` screen: the model blob +
    /// observations reconstructed from self-notes (the narrative moved into
    /// notes, Tier 1). Observations go into the snapshot's `narrative` field
    /// **for display only** — the snapshot itself is never persisted (writes
    /// go through `self_model_update` against the real model, empty of
    /// narrative). `None` if there's neither a model nor observations. See
    /// docs/history/narrative-as-notes.md.
    fn self_model_view_snapshot(
        &self,
        pid: uuid::Uuid,
    ) -> Option<crate::entities::self_model::SelfModel> {
        use crate::entities::self_model::{NarrativeSegment, SelfModel, SelfModelParams};
        let cap = SelfModelParams::from_settings(&self.config.self_model).max_narrative;
        let notes = crate::features::tools::notes::self_notes_recent(&self.storage, pid, cap);
        let model = self.storage.db().self_model_get(pid).ok().flatten();
        if model.is_none() && notes.is_empty() {
            return None;
        }
        let mut m = model.unwrap_or_else(|| SelfModel::new(pid));
        m.narrative = notes
            .into_iter()
            .map(|n| NarrativeSegment {
                id: n.id,
                text: n.content,
                created_at: n.created_at,
            })
            .collect();
        Some(m)
    }

    fn handle_request_self_model(&self) {
        let snapshot = self
            .active_profile_id()
            .and_then(|pid| self.self_model_view_snapshot(pid));
        let _ = self
            .evt_tx
            .send(AppEvent::SelfModelView(Box::new(snapshot)));
    }

    /// Applies a manual edit to the active profile's "self-model" (the `F3`
    /// UI editor): loads it (or creates an empty one), applies the edit,
    /// saves if it changed, then re-emits the updated snapshot (an open
    /// screen updates in place).
    fn handle_update_self_model(&self, edit: crate::entities::self_model::SelfModelEdit) {
        use crate::entities::self_model::SelfModelEdit;
        let Some(pid) = self.active_profile_id() else {
            return;
        };
        // Observations are self-notes, so deleting them / a full clear goes
        // through notes, not through the model blob (the narrative moved into
        // notes, Tier 1).
        match &edit {
            SelfModelEdit::DeleteInsight(id) => {
                let _ = self.storage.db().note_delete(pid, *id);
                let snapshot = self.self_model_view_snapshot(pid);
                let _ = self
                    .evt_tx
                    .send(AppEvent::SelfModelView(Box::new(snapshot)));
                return;
            }
            SelfModelEdit::Clear => {
                // A full clear wipes both the observation notes (@self) and the blob (below).
                for n in
                    crate::features::tools::notes::self_notes_recent(&self.storage, pid, usize::MAX)
                {
                    let _ = self.storage.db().note_delete(pid, n.id);
                }
            }
            _ => {}
        }
        // An atomic edit (under a single DB-mutex acquisition) — prevents a
        // concurrent auto-reflection from clobbering the manual edit via a
        // load-modify-save race. Along the way, fold old closed goals
        // (uniformly with the tools): fold returns scars, which we write as
        // self-notes after the edit.
        let params =
            crate::entities::self_model::SelfModelParams::from_settings(&self.config.self_model);
        let loc = self.profile_locale(pid);
        let mut scars: Vec<String> = Vec::new();
        let _ = self.storage.db().self_model_update(pid, |m| {
            let mut changed = m.apply_edit(edit);
            scars = m.fold_closed_goals(params.max_closed_goals, loc);
            if !scars.is_empty() {
                changed = true;
            }
            changed
        });
        // Scars from folded closed goals → self-notes. A sync insert (the vector, lazily).
        for scar in scars {
            let note = crate::entities::note::Note::new(
                pid,
                scar,
                vec![crate::features::tools::notes::SELF_NOTE_TAG.to_string()],
            );
            let _ = self.storage.db().note_insert(&note);
        }
        // Re-emit the authoritative snapshot (the model + observations from self-notes).
        let snapshot = self.self_model_view_snapshot(pid);
        let _ = self
            .evt_tx
            .send(AppEvent::SelfModelView(Box::new(snapshot)));
    }

    /// Emits the full settings snapshot (config + full profiles) for the settings screen.
    fn emit_settings(&self) {
        let visible: Vec<Profile> = self
            .profiles
            .iter()
            .filter(|p| !p.is_hidden)
            .cloned()
            .collect();
        let language_locked: Vec<Uuid> = visible
            .iter()
            .filter(|p| self.profile_has_data(p.id))
            .map(|p| p.id)
            .collect();
        // Which secrets are stored on **this** machine (the status shown by every
        // secret field). Candidates are enumerated rather than listed from the
        // entry: the entry may also hold this machine's secrets for servers that
        // have since been renamed or deleted (they are deliberately not collected,
        // §9 S4), and a status row must only speak for a field that exists.
        let secrets_present: Vec<SecretKey> = [
            CloudProvider::OpenAi,
            CloudProvider::Gemini,
            CloudProvider::Claude,
        ]
        .into_iter()
        .map(SecretKey::Provider)
        .chain(std::iter::once(SecretKey::BackupPassword))
        .chain(self.config.mcp.servers.iter().flat_map(|s| {
            s.env.keys().map(|var| SecretKey::McpEnv {
                server: s.id.clone(),
                var: var.clone(),
            })
        }))
        .filter(|k| {
            crate::shared::secrets::stored_key(&self.config.api_keys, &k.storage_name()).is_some()
        })
        .collect();
        // Secrets never leave the backend for the UI, not even as ciphertext:
        // the config snapshot goes out without them (`handle_update_config`
        // holds onto them separately). See docs/research/api-key-storage.md.
        let mut config = self.config.clone();
        config.api_keys.clear();
        let _ = self.evt_tx.send(AppEvent::Settings {
            config: Box::new(config),
            profiles: visible,
            language_locked,
            mcp: self.mcp.snapshot(),
            secrets_present,
        });
    }

    /// Whether the profile has data "binding" it to the current scaffold
    /// language (axis A, docs/history/i18n.md): visible chats, a non-empty
    /// "self-model", or notes (including `@self` observations). RAG documents
    /// are deliberately not counted — the knowledge base's file language is
    /// set by the user, not the agent. While `false`, the profile's language
    /// is editable.
    pub(super) fn profile_has_data(&self, profile_id: Uuid) -> bool {
        if self
            .chats
            .iter()
            .any(|c| !c.is_hidden && c.profile_id == profile_id)
        {
            return true;
        }
        let db = self.storage.db();
        if let Ok(Some(m)) = db.self_model_get(profile_id)
            && !m.is_empty()
        {
            return true;
        }
        db.note_list(profile_id, None, &[], None)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    // ---------- helpers (shared across submodules) ----------

    /// The **interface** locale (axis B, docs/i18n-ui.md) — for the
    /// orchestrator's error/notice text shown to the human. Independent of
    /// the agents' language (axis A).
    pub(super) fn ui_locale(&self) -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(self.config.interface.language)
    }

    /// A profile's agent-scaffold locale (axis A, docs/history/i18n.md) by
    /// `profile_id`. An unknown profile → the reference language (`Lang::default`).
    pub(super) fn profile_locale(&self, profile_id: Uuid) -> &'static crate::shared::i18n::Locale {
        let lang = self
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .map(|p| p.language)
            .unwrap_or_default();
        crate::shared::i18n::locale(lang)
    }

    /// Creates a new chat from a profile (by `id`, or the first one) with a
    /// greeting. The "new chat" title — in the profile's agent-scaffold
    /// language (axis A).
    fn new_chat_value(&self, profile_id: Option<Uuid>) -> Chat {
        let profile = profile_id
            .and_then(|id| self.profiles.iter().find(|p| p.id == id))
            .or_else(|| self.profiles.first())
            .cloned()
            .unwrap_or_else(|| default_profile(crate::shared::i18n::Lang::default()));
        let title = crate::shared::i18n::locale(profile.language).t("defaults.chat_title");
        let mut chat = Chat::from_profile(&profile, title);
        if let Some(greeting) = &profile.greeting
            && !greeting.is_empty()
        {
            chat.push_message(Message::assistant(greeting.clone()));
        }
        chat
    }

    fn chat_mut(&mut self, id: Uuid) -> Option<&mut Chat> {
        self.chats.iter_mut().find(|c| c.id == id)
    }

    /// Rebuilds the tool registry: the standard set from config + live
    /// wrappers for MCP-server tools (dynamic — taken from [`McpManager`]).
    /// The single rebuild path: any call site (a settings edit, an MCP event)
    /// must go through this, otherwise MCP tools would fall out of the registry.
    pub(super) fn rebuild_registry(&mut self) {
        let mut reg = build_registry(&self.config, self.storage.json().sandbox_dir());
        for tool in self.mcp.tools() {
            reg.register(tool);
        }
        self.registry = Arc::new(reg);
    }

    /// Assembles the bundle of shared tool dependencies for the given
    /// chat-engine (the embedder and storage are shared). See
    /// docs/history/refactoring-solid.md §3.
    fn tool_deps(
        &self,
        backend: Arc<dyn crate::shared::api::EngineBackend>,
    ) -> crate::features::tools::ToolDeps {
        crate::features::tools::ToolDeps {
            storage: self.storage.clone(),
            engine: backend,
            embedder: self.engines.embedder(),
        }
    }

    /// Resolves the actual sampling for a chat: `Chat.sampling_override` →
    /// `Profile.default_sampling` → global (spec §8.3).
    fn effective_sampling(&self, chat_id: Uuid) -> SamplingConfig {
        let chat = self.chats.iter().find(|c| c.id == chat_id);
        let chat_override = chat.and_then(|c| c.sampling_override.as_ref());
        let profile_default = chat
            .and_then(|c| self.profiles.iter().find(|p| p.id == c.profile_id))
            .and_then(|p| p.default_sampling.as_ref());
        crate::entities::sampling::resolve(
            chat_override,
            profile_default,
            &self.config.default_sampling,
        )
    }

    /// Makes a chat active and sends its messages to the UI.
    fn activate(&mut self, id: Uuid) {
        self.activate_focused(id, None);
    }

    /// [`Self::activate`] with an optional message to put the feed on and query
    /// to highlight in it (a jump from a search hit,
    /// [`AppCommand::OpenChatAt`]). The single activation funnel — everything
    /// else goes through `activate` and passes `None`.
    fn activate_focused(&mut self, id: Uuid, focus: Option<FeedFocus>) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        self.active_id = Some(id);
        let _ = self.evt_tx.send(AppEvent::ChatActivated {
            id,
            title: chat.title.clone(),
            messages: chat.messages.clone(),
            draft: chat.draft.clone(),
            feed_view: chat.feed_view,
            focus,
        });
        self.emit_character_names();
        self.emit_attachments();
        self.remember_active_chat(id);
    }

    /// Sends the feed the role names of the active chat's profile (spec §5.1).
    /// Called on activation and after a profile edit — so renaming in settings
    /// applies to the open chat right away (the names are resolved from the profile,
    /// not from the chat's creation-time copy).
    pub(super) fn emit_character_names(&self) {
        let names = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .and_then(|chat| self.profiles.iter().find(|p| p.id == chat.profile_id))
            .map(|p| p.character_names.clone())
            .unwrap_or_default();
        let _ = self.evt_tx.send(AppEvent::CharacterNames(names));
    }

    /// The active chat's profile role names (for the `F5` export). Default (empty
    /// = the localized labels) when there's no active chat or the profile is gone.
    pub(super) fn active_character_names(&self, chat: &Chat) -> CharacterNames {
        self.profiles
            .iter()
            .find(|p| p.id == chat.profile_id)
            .map(|p| p.character_names.clone())
            .unwrap_or_default()
    }

    /// Remembers the last-open chat in `settings.json` so it can be restored
    /// on the next launch. Writes only on an actual switch of the active chat
    /// — `activate` is also called to rebuild the same chat's feed
    /// (regeneration, deleting an exchange), where saving settings isn't
    /// needed. A write error isn't escalated (memory is a convenience, not
    /// critical).
    fn remember_active_chat(&mut self, id: Uuid) {
        if self.config.last_active_chat == Some(id) {
            return;
        }
        self.config.last_active_chat = Some(id);
        if let Err(err) = self.storage.json().save_config(&self.config) {
            tracing::warn!(error = %err, "failed to remember the last-open chat");
        }
    }

    fn emit_chat_list(&self) {
        let mut summaries: Vec<ChatSummary> = self.chats.iter().map(|c| c.summary()).collect();
        summaries.sort_by_key(|s| std::cmp::Reverse(s.modified_at));
        let _ = self.evt_tx.send(AppEvent::ChatList(summaries));
    }

    fn emit_profile_list(&self) {
        let _ = self.evt_tx.send(AppEvent::ProfileList(
            self.profiles.iter().map(|p| p.summary()).collect(),
        ));
    }

    /// Emits a snapshot of all server statuses (chat/embeddings/impersonation)
    /// into the status bar. Called on any change to any status (a probe/settings change).
    fn emit_server_status(&self) {
        let _ = self
            .evt_tx
            .send(AppEvent::ServerStatus(self.engines.statuses()));
    }

    /// Marks a chat for deferred saving (debounce).
    fn mark_dirty(&mut self, id: Uuid) {
        self.saves.mark(id);
    }

    /// Saves all dirty chats to disk, then brings each one's search index in
    /// line (see [`search`]). Indexing runs **after** a successful save so it
    /// records the file state it actually indexed; a failure there is logged
    /// and ignored — the index is disposable and must never break saving.
    fn flush_saves(&mut self) {
        for id in self.saves.take() {
            let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
                continue;
            };
            match self.storage.json().save_chat(chat) {
                Ok(()) => self.index_saved_chat(chat),
                Err(err) => tracing::error!(chat = %id, error = %err, "failed to save the chat"),
            }
        }
    }
}
