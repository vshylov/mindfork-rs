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
//! - [`llm_history`] — seeding a profile's language-model history from what its
//!   chats already record (spec §9.14);
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
mod background_runs;
mod chats;
mod compaction;
mod consolidation;
mod embed_guard;
mod engines;
mod generation;
mod images;
mod impersonation;
mod llm_history;
mod mcp;
mod model_name;
mod pool;
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
mod slots;
mod title;
mod tool_loop;
mod tts;
mod workspace;

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
use crate::shared::secrets::{SearchSlot, SecretKey};
use crate::shared::storage::Storage;

use self::attachments::AttachResult;
use self::background::BgSlot;
use self::compaction::{CompactResult, ContextDiscovery};
use self::engines::EngineManager;
use self::generation::{GenMessage, TurnProgress};
use self::images::{ImageAttachResult, StagedImages};
use self::mcp::{McpEvent, McpManager};
use self::restart_queue::RestartQueue;
use self::save_queue::SaveQueue;
use self::title::TitleResult;
use crate::entities::subagent::RunOutcome;

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
    /// Tools registered on top of the standard set, and re-registered on
    /// every rebuild. Empty in production: the hook exists for tests that
    /// need an instrumented tool inside a real turn (a counting, delaying
    /// read for the concurrent segment, docs/research/concurrent-tools.md
    /// §5) — the same door the MCP tools come through, without a server.
    pub extra_tools: Vec<Arc<dyn crate::features::tools::Tool>>,
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
        extra_tools,
    } = deps;

    let (done_tx, mut done_rx) = unbounded_channel::<GenMessage>();
    // Internal server-status channel: the supervisor's background probe posts
    // readiness (Ready/Disconnected) here, the loop translates it into AppEvent::ServerStatus.
    let (status_tx, mut status_rx) = unbounded_channel::<ServerStatus>();
    // Internal auto-title channel: a background task sends the generated
    // title (or an error), the loop applies it to the chat.
    let (title_tx, mut title_rx) = unbounded_channel::<TitleResult>();
    // Internal history-compression channel: a background roll sends the summary
    // (or an error), the loop stores it on the chat.
    let (compact_tx, mut compact_rx) = unbounded_channel::<CompactResult>();
    // Internal channel for what the engine says its context window is (spec §6.7,
    // sub-decision S1): a background task asks `EngineBackend::context_budget`
    // and answers `(epoch, budget)`; the epoch is what lets the loop drop an
    // answer that belongs to an engine which has since been replaced.
    let (budget_tx, mut budget_rx) = unbounded_channel::<(u64, Option<u32>)>();
    // The same shape for what the engine calls the model it is running
    // (docs/research/external-model-name.md §4): a background task asks
    // `EngineBackend::model_id` and answers `(epoch, name)`, and the epoch drops
    // an answer belonging to an engine that has since been replaced.
    let (model_tx, mut model_rx) = unbounded_channel::<(u64, Option<String>)>();
    // And the same shape for how many requests the engine serves at once (the
    // hint next to the `sessions` field, spec §11.6): `(epoch, slots)`.
    let (slots_tx, mut slots_rx) = unbounded_channel::<(u64, Option<u32>)>();
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
    // The background sub-agent runs' channel (progress and ends): their own,
    // since a run outlives the turn whose channel a child normally shares.
    let (bg_run_tx, mut bg_run_rx) = unbounded_channel::<generation::BackgroundMessage>();
    // Internal "speech finished" channel (background task → loop): the loop
    // distinguishes its own outcome from a stale one by the task's generation.
    let (tts_done_tx, mut tts_done_rx) = unbounded_channel::<Uuid>();
    // Internal channel for MCP-server events (spawn/monitor background tasks).
    let (mcp_evt_tx, mut mcp_evt_rx) = unbounded_channel::<McpEvent>();
    // Internal channel for reading/extracting an attached file (`/file attach`):
    // a blocking task sends back the extracted text, the loop inserts it into the chat.
    let (attach_tx, mut attach_rx) = unbounded_channel::<AttachResult>();
    // The same shape for `/image attach`: decoding and downscaling a photo is far too
    // slow to run on the command loop.
    let (image_tx, mut image_rx) = unbounded_channel::<ImageAttachResult>();
    let registry = {
        let mut reg = build_registry(&config, storage.json().sandbox_dir());
        for tool in &extra_tools {
            reg.register(tool.clone());
        }
        Arc::new(reg)
    };
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
        compact_tx,
        budget_tx,
        context: ContextDiscovery::default(),
        model_tx,
        model: model_name::ModelDiscovery::default(),
        slots_tx,
        slots: slots::SlotsDiscovery::default(),
        profiles: Vec::new(),
        chats: Vec::new(),
        confirm: None,
        inflight: None,
        background_runs: Vec::new(),
        bg_run_tx,
        background_slots: Arc::new(generation::BackgroundSlots::new(
            crate::shared::config::DEFAULT_SUBAGENT_BACKGROUND_MAX,
        )),
        pending_landings: Vec::new(),
        session_budget_memo: None,
        active_id: None,
        gen_state: GenState::Idle,
        done_tx,
        rag_cancel: None,
        attach_tx,
        image_tx,
        staged_images: StagedImages::default(),
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
        extra_tools,
    };

    // Bring up the servers from config and emit the startup events/settings.
    orch.background_slots
        .set_max(orch.config.tools.subagent_background_max);
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
    // A database that was not there is otherwise indistinguishable from a first
    // launch — the assistant simply has no memory of conversations that are on
    // screen, and nothing says why. Sent **after** `bootstrap`, whose
    // `activate` rebuilds the feed from the chat's messages: a note pushed
    // before that rebuild would be wiped by it. The text names what is empty,
    // what survived and the one route back, per docs/lessons.md §4.
    if orch.storage.chats_without_db() {
        let _ = orch.evt_tx.send(AppEvent::Notice(
            orch.ui_locale().t("ui.startup.db_missing").into(),
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
                match done {
                    Some(GenMessage::Done(res)) => orch.handle_done(res),
                    Some(GenMessage::Progress { id, progress }) => orch.handle_progress(id, progress),
                    None => {}
                }
            }
            status = status_rx.recv() => {
                if let Some(s) = status {
                    orch.engines.set_chat_status(s);
                    // Readiness flipped, so the engine may answer differently now:
                    // a server that was down could not report its context window,
                    // and one that just came up can. The channel only carries
                    // flips, so this is not a per-probe cost. See `ContextDiscovery`.
                    orch.context.invalidate();
                    // And a server that just came up can now say what it loaded,
                    // where a moment ago it could not (`ModelDiscovery`).
                    orch.refresh_model_name();
                    // Likewise its slot count (the `sessions` hint).
                    orch.refresh_engine_slots();
                    orch.emit_server_status();
                    orch.relaunch_dead_managed_servers();
                }
            }
            title = title_rx.recv() => {
                if let Some(res) = title {
                    orch.handle_title_result(res);
                }
            }
            compact = compact_rx.recv() => {
                if let Some(res) = compact {
                    orch.handle_compact_result(res);
                }
            }
            budget = budget_rx.recv() => {
                if let Some((epoch, value)) = budget {
                    orch.handle_budget_result(epoch, value);
                }
            }
            model = model_rx.recv() => {
                if let Some((epoch, name)) = model {
                    orch.handle_model_result(epoch, name);
                }
            }
            slots = slots_rx.recv() => {
                if let Some((epoch, count)) = slots {
                    orch.handle_slots_result(epoch, count);
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
            message = bg_run_rx.recv() => {
                if let Some(message) = message {
                    orch.handle_background_message(message);
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
            res = image_rx.recv() => {
                if let Some(res) = res {
                    orch.handle_image_result(res);
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
        web_fetch_content: config.tools.web_fetch_content,
        web_allow_private: config.tools.web_allow_private,
        web_provider: config.tools.web_provider,
        web_search_keys: web_search_keys(config),
        fs_root: config.tools.fs_root.clone(),
        subagent_parallel: config.tools.subagent_parallel,
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

/// Resolves the keyed search providers' credentials, in preference order.
///
/// A key stored in settings (machine-encrypted, ADR 0008) **wins** over the
/// environment variable the settings name — the same precedence `api_key_env`
/// already has everywhere else, so a key typed into the app is never shadowed by
/// a stale shell. Resolving it here rather than inside the tool keeps the secret
/// store in one layer: `web_search` receives strings and never learns where they
/// came from.
fn web_search_keys(config: &AppConfig) -> Vec<(SearchSlot, String)> {
    let from_env = |name: &Option<String>| -> Option<String> {
        let name = name.as_deref()?.trim();
        (!name.is_empty())
            .then(|| std::env::var(name).ok())
            .flatten()
    };
    [(SearchSlot::Tavily, &config.tools.web_tavily_key_env)]
        .into_iter()
        .filter_map(|(slot, env_name)| {
            let key = crate::shared::secrets::stored_key(
                &config.api_keys,
                &crate::shared::secrets::SecretKey::Search(slot).storage_name(),
            )
            .or_else(|| from_env(env_name))?;
            (!key.trim().is_empty()).then_some((slot, key))
        })
        .collect()
}

/// What an id the UI hands over resolves to (see [`Orchestrator::view`]).
enum ChatView<'a> {
    Top(&'a Chat),
    Child {
        parent: &'a Chat,
        run: &'a crate::entities::subagent::SubagentRun,
    },
}

/// The running turn as the orchestrator mirrors it (docs/subagent-live.md
/// §3.2). See [`Orchestrator::inflight`].
struct InflightTurn {
    generation: Uuid,
    chat: Uuid,
    /// The parent's rounds filed so far — not yet in `Chat.messages`.
    rounds: Vec<Message>,
    /// The parent's round in progress — text, thoughts and the calls it has
    /// opened — so its feed is rebuilt whole on a return mid-turn
    /// (docs/history/subagent-live.md §8, last bullet).
    partial: crate::app::events::LivePartial,
    /// The sub-agent runs in progress, or just ended and not yet landed, in
    /// start order — several at once when the model delegated several tasks
    /// in one reply (spec §9.3.2, docs/research/parallel-subagents.md §4.5).
    /// Every progress step names its run; nothing here is "the" child.
    children: Vec<InflightChild>,
    /// The turn continues the chat's trailing assistant message in place
    /// (`/continue`, spec §6.4): a mid-turn rebuild folds the filed first
    /// round into that message's view and appends the live partial there.
    continuation: bool,
}

/// Applies one mirrored step to a round in progress (see
/// [`TurnProgress::OwnStep`] / [`TurnProgress::ChildStep`]).
fn apply_step(partial: &mut crate::app::events::LivePartial, step: &generation::StreamStep) {
    use generation::StreamStep;
    match step {
        StreamStep::Chunk(text) => partial.text.push_str(text),
        StreamStep::Thoughts(text) => partial.thoughts.push_str(text),
        StreamStep::ToolStarted {
            call_id,
            name,
            arguments,
        } => partial.tools.push(crate::app::events::LiveTool {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            result: None,
        }),
        StreamStep::ToolCall {
            call_id,
            name,
            arguments,
            result,
            images,
        } => match partial.tools.iter_mut().find(|t| t.call_id == *call_id) {
            Some(tool) => tool.result = Some((result.clone(), *images)),
            None => partial.tools.push(crate::app::events::LiveTool {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                result: Some((result.clone(), *images)),
            }),
        },
        // A follow-up starts a new bubble: the mirror keeps only the round
        // in progress, and the filed round carries the earlier bubble.
        StreamStep::Continue => {}
        StreamStep::Rewrite => *partial = Default::default(),
    }
}

/// One running sub-agent as the orchestrator mirrors it (see
/// [`InflightTurn::children`]).
struct InflightChild {
    /// The run as it will land: persona, title, the rounds filed so far.
    run: crate::entities::subagent::SubagentRun,
    /// The run's own stream id — what its transcript's feed accepts
    /// (docs/history/subagent-live.md §8). Minted with the run.
    stream: Uuid,
    /// The run's round in progress: text and thoughts streamed since the
    /// last filed round, so a transcript opened mid-round starts whole.
    partial: crate::app::events::LivePartial,
    /// Which side of the transcript the round in progress belongs to:
    /// `Assistant` for a sub-agent's rounds; a dialogue's line carries its
    /// speaker's side (spec §9.13), set by [`TurnProgress::ChildLineStarted`].
    line_role: crate::entities::message::MessageRole,
}

impl InflightTurn {
    fn child(&self, id: Uuid) -> Option<&InflightChild> {
        self.children.iter().find(|c| c.run.id == id)
    }

    fn child_mut(&mut self, id: Uuid) -> Option<&mut InflightChild> {
        self.children.iter_mut().find(|c| c.run.id == id)
    }

    /// Whether `id` names the turn's chat or one of its running transcripts.
    fn covers(&self, id: Uuid) -> bool {
        self.chat == id || self.child(id).is_some()
    }
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
    /// Channel for results of background history compression (spec §6.7).
    compact_tx: UnboundedSender<CompactResult>,
    /// Channel for the engine's answer about its context window: `(epoch, budget)`.
    budget_tx: UnboundedSender<(u64, Option<u32>)>,
    /// What is known about the engine's context window — the budget the automatic
    /// compaction measures itself against.
    context: ContextDiscovery,
    /// Channel for the engine's answer about the model it is running: `(epoch, name)`.
    model_tx: UnboundedSender<(u64, Option<String>)>,
    /// What the engine said it is running, when the configuration does not say
    /// (`external` with a blank "Model (opt.)" — see [`model_name`]).
    model: model_name::ModelDiscovery,
    /// Channel for the engine's answer about its slot count: `(epoch, slots)`.
    slots_tx: UnboundedSender<(u64, Option<u32>)>,
    /// What the engine said about how many requests it serves at once — the
    /// hint next to the `sessions` field (see [`slots`]).
    slots: slots::SlotsDiscovery,
    profiles: Vec<Profile>,
    /// The in-flight turn's dangerous-tool confirmation channel: its id and the
    /// sender the generation task is listening on (spec §9.8, fork F8 of
    /// docs/history/tool-confirmation.md). `None` between turns.
    confirm: Option<(Uuid, UnboundedSender<(String, ToolDecision)>)>,
    /// The running turn's in-flight mirror (docs/subagent-live.md §3.2):
    /// the parent's rounds filed so far and the sub-agent run in progress,
    /// so a transcript is a row of the list and openable before the turn
    /// lands. Created by `start_generation`, fed by [`Self::handle_progress`],
    /// dropped by `handle_done`. Never a source of truth: `GenResult` is.
    inflight: Option<InflightTurn>,
    /// The sub-agent runs out in the **background** (spec §9.3.2,
    /// docs/research/background-subagents.md §4.9): one seat per run, its
    /// mirror outliving any turn. See [`background_runs`].
    background_runs: Vec<background_runs::BackgroundRun>,
    /// The background runs' channel — their progress and their ends
    /// ([`generation::BackgroundMessage`]).
    bg_run_tx: UnboundedSender<generation::BackgroundMessage>,
    /// How many background runs are out, against the cap; shared with
    /// every turn ([`generation::BackgroundSlots`]).
    background_slots: Arc<generation::BackgroundSlots>,
    /// Runs that ended while a turn was running in their chat: their
    /// records land with that turn, and so do they (research §4.4, §5).
    pending_landings: Vec<background_runs::PendingLanding>,
    /// The session budget the turns and the background runs share, with
    /// the key it was built for (research §4.7; [`background_runs::BudgetKey`]).
    session_budget_memo: Option<(
        background_runs::BudgetKey,
        Arc<crate::shared::session_budget::SessionBudget>,
    )>,
    /// Visible chats, entirely in memory (the orchestrator is the sole writer).
    chats: Vec<Chat>,
    active_id: Option<Uuid>,
    /// The lifecycle state machine for assistant-reply generation (see `gen_state`).
    gen_state: GenState,
    done_tx: UnboundedSender<GenMessage>,
    /// Cancellation token for the current background RAG indexing (`/rag add`);
    /// `None` — not running. Replaced/cancelled on new indexing and on shutdown.
    rag_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Channel for results of reading/extracting an attached file (`/file attach`,
    /// a blocking task → the loop). See [`attachments`].
    attach_tx: UnboundedSender<AttachResult>,
    /// Channel for results of reading/decoding an attached image (`/image attach`,
    /// a background task → the loop). See [`images`].
    image_tx: UnboundedSender<ImageAttachResult>,
    /// Images staged for each chat's **next** message (`/image attach`, spec §9.10).
    /// Session-only and deliberately not persisted: what was staged but never sent is a
    /// half-finished thought, not conversation state. See [`images`].
    staged_images: StagedImages,
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
    /// See [`OrchestratorDeps::extra_tools`]; kept so a rebuild re-registers them.
    extra_tools: Vec<Arc<dyn crate::features::tools::Tool>>,
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

        // Profiles and chats are both loaded now, and nothing has recorded an
        // exchange yet: the one moment a language-model history that is still
        // empty can be seeded from the metadata the stored replies already
        // carry (spec §9.14, [`llm_history`]).
        self.seed_llm_history();

        // Restore the last-open chat — or transcript — if it's still visible;
        // otherwise the most recently modified chat (the previous behavior).
        let active = self
            .config
            .last_active_chat
            .filter(|id| self.view(*id).is_some())
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
                // Every background run lands `cancelled` from its mirror
                // before the exit flush (research fork F7).
                self.stop_all_background_runs();
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
            AppCommand::SetChildrenExpanded { id, expanded } => {
                self.handle_set_children_expanded(id, expanded)
            }
            AppCommand::StopSubagentRun { id } => self.handle_stop_subagent_run(id),
            AppCommand::RegenerateLast => self.handle_regenerate(),
            AppCommand::ContinueLast => self.handle_continue(),
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
            AppCommand::Compact => self.handle_compact(),
            AppCommand::CloneChat(id) => self.handle_clone(id),
            AppCommand::CopyChat(id) => self.handle_copy_chat(id),
            AppCommand::DeleteChat(id) => self.handle_delete(id),
            AppCommand::SearchChats(query) => self.handle_search_chats(query),
            AppCommand::SearchMessages { query, sort } => self.handle_search_messages(query, sort),
            AppCommand::CreateProfile {
                name,
                system_message,
            } => self.handle_create_profile(name, system_message),
            AppCommand::ExportChat { id, format, path } => {
                self.handle_export_chat(id, format, path.as_deref())
            }
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
            AppCommand::ProjectAttach { path } => self.handle_project_attach(path),
            AppCommand::ProjectDetach => self.handle_project_detach(),
            AppCommand::ProjectStatus => self.handle_project_status(),
            AppCommand::ProjectSlot { slot, action } => self.handle_project_slot(slot, action),
            AppCommand::OpenChanges => self.handle_open_changes(),
            AppCommand::RevertWorkspaceFile { path } => self.handle_revert_workspace_file(path),
            AppCommand::ImageAttach { path } => self.handle_image_attach(path),
            AppCommand::ImageRemove { target } => self.handle_image_remove(target),
            AppCommand::ImageList => self.handle_image_list(),
            AppCommand::ImagePaste(image) => self.handle_image_paste(*image),
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

    /// Sends the `F3` screen a fresh snapshot of `pid`'s self-model together with
    /// that profile's role names — the screen heads its two halves with them
    /// (spec §17.7). The single place emitting `SelfModelView`: every path (the
    /// request, and each re-emit after an edit) goes through it, so the names can
    /// never be forgotten on one of them.
    fn emit_self_model_view(&self, pid: Option<Uuid>) {
        let model = pid.and_then(|pid| self.self_model_view_snapshot(pid));
        // The **profile's** own names, not `names_of`: that one re-labels a
        // subagent transcript's sides for the run, and the self-model is the
        // profile's.
        let names = pid
            .and_then(|pid| self.profiles.iter().find(|p| p.id == pid))
            .map(|p| p.character_names.clone())
            .unwrap_or_default();
        let _ = self.evt_tx.send(AppEvent::SelfModelView {
            model: Box::new(model),
            names,
        });
    }

    fn handle_request_self_model(&self) {
        self.emit_self_model_view(self.active_profile_id());
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
                self.emit_self_model_view(Some(pid));
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
        self.emit_self_model_view(Some(pid));
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
        let secrets_present: Vec<SecretKey> = CloudProvider::ALL
            .into_iter()
            .map(SecretKey::Provider)
            .chain(
                crate::shared::secrets::ExternalSlot::ALL
                    .into_iter()
                    .map(SecretKey::External),
            )
            .chain(SearchSlot::ALL.into_iter().map(SecretKey::Search))
            .chain(std::iter::once(SecretKey::BackupPassword))
            .chain(self.config.mcp.servers.iter().flat_map(|s| {
                s.env.keys().map(|var| SecretKey::McpEnv {
                    server: s.id.clone(),
                    var: var.clone(),
                })
            }))
            .filter(|k| {
                crate::shared::secrets::stored_key(&self.config.api_keys, &k.storage_name())
                    .is_some()
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
    /// language (axis A, docs/history/i18n.md): visible chats **with
    /// conversation content** ([`Chat::is_pristine`]), a non-empty
    /// "self-model", or notes (including `@self` observations). A pristine
    /// chat — the first-launch default one included — binds nothing: its
    /// localized artifacts are re-derived on a language switch
    /// ([`Self::rederive_pristine_chats`]), so a fresh install's language
    /// stays editable. RAG documents are deliberately not counted — the
    /// knowledge base's file language is set by the user, not the agent.
    /// While `false`, the profile's language is editable.
    pub(super) fn profile_has_data(&self, profile_id: Uuid) -> bool {
        if self
            .chats
            .iter()
            .any(|c| !c.is_hidden && c.profile_id == profile_id && !c.is_pristine())
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

    /// What an id names: a chat of the list, or a sub-agent transcript inside
    /// one (spec §9.3.2). The single resolver for the paths that work on
    /// either — opening, renaming, titling, copying, exporting, speaking.
    /// Everything that looks the id up in `self.chats` directly fails closed
    /// on a transcript, which is the read-only behaviour by construction
    /// (docs/research/subagent-chats.md §3.8).
    fn view(&self, id: Uuid) -> Option<ChatView<'_>> {
        if let Some(chat) = self.chats.iter().find(|c| c.id == id) {
            return Some(ChatView::Top(chat));
        }
        // A run out in the background: its mirror, ahead of the placeholder
        // its record landed with (docs/research/background-subagents.md
        // §4.9) — the transcript grows here until the run ends.
        if let Some(seat) = self.background_run(id)
            && let Some(parent) = self.chats.iter().find(|c| c.id == seat.chat)
        {
            return Some(ChatView::Child {
                parent,
                run: &seat.child.run,
            });
        }
        if let Some(found) = self
            .chats
            .iter()
            .find_map(|parent| parent.child(id).map(|run| ChatView::Child { parent, run }))
        {
            return Some(found);
        }
        // The sub-agent running right now (docs/subagent-live.md §3.3): not in
        // any chat yet, reachable through the same resolver so opening,
        // naming, copying and the `chat://` book need no second path.
        let turn = self.inflight.as_ref()?;
        let run = &turn.child(id)?.run;
        let parent = self.chats.iter().find(|c| c.id == turn.chat)?;
        Some(ChatView::Child { parent, run })
    }

    /// The chat that holds the transcript `id`, if `id` is one.
    fn parent_of(&self, id: Uuid) -> Option<Uuid> {
        match self.view(id) {
            Some(ChatView::Child { parent, .. }) => Some(parent.id),
            _ => None,
        }
    }

    /// Edits the transcript `id` in place and marks its parent dirty. `false`
    /// when there is no such transcript.
    fn with_child_mut(
        &mut self,
        id: Uuid,
        edit: impl FnOnce(&mut crate::entities::subagent::SubagentRun),
    ) -> bool {
        let Some(parent_id) = self.parent_of(id) else {
            return false;
        };
        // A run out in the background: the mirror, which the landing
        // carries onto the record (a title given while it ran).
        if let Some(child) = self
            .background_runs
            .iter_mut()
            .find(|b| b.run_id == id)
            .map(|b| &mut b.child)
        {
            edit(&mut child.run);
            return true;
        }
        if let Some(run) = self.chat_mut(parent_id).and_then(|c| c.child_mut(id)) {
            edit(run);
            self.mark_dirty(parent_id);
            return true;
        }
        // A running transcript: the edit lands on the in-flight mirror, and
        // `handle_done` carries the title onto the landed run (§3.3). Nothing
        // is on disk yet, so nothing is dirty.
        if let Some(child) = self.inflight.as_mut().and_then(|t| t.child_mut(id)) {
            edit(&mut child.run);
        }
        true
    }

    /// Applies one step of the running turn to the in-flight mirror
    /// (docs/subagent-live.md §3.1–§3.4): the parent's rounds accumulate; a
    /// sub-agent's start and end redraw the list (the *running* row); its
    /// rounds grow the transcript on screen when it is the open one. A step
    /// from a turn that is not the one in flight is dropped — a cancelled
    /// turn's task can still be filing while the next turn has begun.
    pub(super) fn handle_progress(&mut self, id: Uuid, progress: TurnProgress) {
        let Some(turn) = self.inflight.as_mut().filter(|t| t.generation == id) else {
            return;
        };
        match progress {
            TurnProgress::RoundFiled(messages) => {
                turn.rounds.extend(messages);
                turn.partial = Default::default();
            }
            TurnProgress::OwnStep(step) => apply_step(&mut turn.partial, &step),
            TurnProgress::ChildStarted(run) => {
                turn.children.push(InflightChild {
                    run: *run,
                    stream: Uuid::new_v4(),
                    partial: Default::default(),
                    line_role: crate::entities::message::MessageRole::Assistant,
                });
                self.emit_chat_list();
            }
            // A `start_subagent` call: the run leaves the turn for a seat of
            // its own (docs/research/background-subagents.md §4.2).
            TurnProgress::BackgroundStart(start) => {
                let chat = turn.chat;
                self.spawn_background_run(start, chat);
            }
            other => self.handle_child_progress(other),
        }
    }

    /// One step of a sub-agent run's life, applied to whichever mirror
    /// holds the run — the turn's children or a background seat
    /// (docs/research/background-subagents.md §4.9): every child step names
    /// its run, and that id is the key. A step that is not a child's is
    /// nothing here.
    fn handle_child_progress(&mut self, progress: TurnProgress) {
        match progress {
            // A dialogue's next line (spec §9.13): the partial starts over on
            // the given side; the open transcript is told which bubble the
            // coming stream belongs to.
            TurnProgress::ChildLineStarted { run, role } => {
                let Some(child) = self.child_mut_any(run) else {
                    return;
                };
                child.partial = Default::default();
                child.line_role = role;
                self.forward_child(run, |stream| AppEvent::TranscriptLine {
                    generation_id: stream,
                    role,
                });
            }
            // A dialogue edited its transcript (a discarded or rewritten
            // line): the mirror takes the full replacement, and so does the
            // open transcript — appending cannot express an edit.
            TurnProgress::ChildTranscript { run, messages } => {
                let Some(child) = self.child_mut_any(run) else {
                    return;
                };
                child.run.messages = messages.clone();
                child.partial = Default::default();
                if self.active_id == Some(run) {
                    let _ = self
                        .evt_tx
                        .send(AppEvent::TranscriptReset { id: run, messages });
                }
                // The count on the row.
                self.emit_chat_list();
            }
            TurnProgress::ChildRoundFiled { run, messages } => {
                let Some(child) = self.child_mut_any(run) else {
                    return;
                };
                child.run.messages.extend(messages.iter().cloned());
                child.partial = Default::default();
                if self.active_id == Some(run) {
                    let _ = self
                        .evt_tx
                        .send(AppEvent::TranscriptGrew { id: run, messages });
                }
                // The count on the row.
                self.emit_chat_list();
            }
            TurnProgress::ChildEnded {
                run,
                outcome,
                finished_at,
                tokens,
            } => {
                let Some(child) = self.child_mut_any(run) else {
                    return;
                };
                child.run.outcome = Some(outcome);
                child.run.finished_at = Some(finished_at);
                child.run.tokens = tokens;
                let stream = child.stream;
                if self.active_id == Some(run) {
                    let reason = match outcome {
                        RunOutcome::Completed | RunOutcome::RoundLimit => FinishReason::Stop,
                        RunOutcome::Cancelled | RunOutcome::TimedOut => FinishReason::Cancelled,
                        RunOutcome::Failed => FinishReason::Error,
                    };
                    let _ = self.evt_tx.send(AppEvent::Finished {
                        generation_id: stream,
                        reason,
                        // A transcript is read-only — nothing on it continues.
                        continuable: false,
                    });
                }
                self.emit_chat_list();
            }
            // The sub-agent's stream (docs/history/subagent-live.md §8): the
            // round's partial is kept for a late opening; the step goes to
            // the screen, under the run's own stream id, only while the
            // transcript is the open conversation.
            TurnProgress::ChildStep { run, step } => {
                let Some(child) = self.child_mut_any(run) else {
                    return;
                };
                apply_step(&mut child.partial, &step);
                self.forward_child(run, |stream| {
                    use generation::StreamStep;
                    match step {
                        StreamStep::Chunk(text) => AppEvent::Chunk {
                            generation_id: stream,
                            text,
                        },
                        StreamStep::Thoughts(text) => AppEvent::Thoughts {
                            generation_id: stream,
                            text,
                        },
                        StreamStep::ToolStarted {
                            call_id,
                            name,
                            arguments,
                        } => AppEvent::ToolCallStarted {
                            generation_id: stream,
                            call_id,
                            name,
                            arguments,
                        },
                        StreamStep::ToolCall {
                            call_id,
                            name,
                            arguments,
                            result,
                            images,
                        } => AppEvent::ToolCall {
                            generation_id: stream,
                            call_id,
                            name,
                            arguments,
                            result,
                            images,
                        },
                        StreamStep::Continue => AppEvent::AssistantContinue {
                            generation_id: stream,
                        },
                        StreamStep::Rewrite => AppEvent::AssistantRewrite {
                            generation_id: stream,
                        },
                    }
                });
            }
            TurnProgress::ChildTokens {
                run,
                completion,
                reasoning,
            } => self.forward_child(run, |stream| AppEvent::TokenUsage {
                generation_id: stream,
                completion,
                context: None,
                context_exact: false,
                reasoning,
            }),
            TurnProgress::RoundFiled(_)
            | TurnProgress::OwnStep(_)
            | TurnProgress::ChildStarted(_)
            | TurnProgress::BackgroundStart(_) => {}
        }
    }

    /// Sends one step of the running sub-agent's stream to the screen, under
    /// the run's stream id, when its transcript is the open conversation.
    fn forward_child(&self, run: Uuid, event: impl FnOnce(Uuid) -> AppEvent) {
        if self.active_id != Some(run) {
            return;
        }
        if let Some(child) = self.child_any(run) {
            let _ = self.evt_tx.send(event(child.stream));
        }
    }

    /// Whether a switch from the active chat to `id` stays inside the running
    /// turn — its chat and any of its in-flight sub-agents, either way round —
    /// and so must not cancel it (docs/subagent-live.md §3.5, fork F6).
    fn switch_within_turn(&self, id: Uuid) -> bool {
        let Some(turn) = self.inflight.as_ref() else {
            return false;
        };
        // The turn's chat, its in-flight children, and the background runs
        // of that chat (docs/research/background-subagents.md §4.9):
        // looking at any of them is not leaving the turn.
        let within = |x: Uuid| {
            turn.covers(x)
                || self
                    .background_runs
                    .iter()
                    .any(|b| b.run_id == x && b.chat == turn.chat)
        };
        within(id) && self.active_id.is_some_and(within)
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
        for tool in &self.extra_tools {
            reg.register(tool.clone());
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

    /// Tool context for a **background task** (reflection, notes and
    /// self-model consolidation): it runs outside any chat turn, so the turn
    /// snapshot is empty — no attachments, no folded range, no other-chat
    /// list. One seam instead of three build sites that drifted a field at a
    /// time whenever `TurnInfo` grew.
    // One argument per field a background task actually varies; a parameter
    // struct here would just be `TurnInfo` under another name.
    #[allow(clippy::too_many_arguments)]
    fn background_tool_ctx(
        &self,
        backend: Arc<dyn crate::shared::api::EngineBackend>,
        profile_id: Uuid,
        chat_id: Uuid,
        system_message: String,
        last_user: Option<chrono::DateTime<chrono::Utc>>,
        lang: crate::shared::i18n::Lang,
        cancel: tokio_util::sync::CancellationToken,
    ) -> crate::features::tools::ToolContext {
        crate::features::tools::ToolContext::new(
            self.tool_deps(backend),
            crate::features::tools::ToolParams::from_config(&self.config),
            crate::features::tools::TurnInfo {
                profile_id,
                chat_id,
                system_message,
                effective_sampling: crate::entities::sampling::SamplingConfig::default(),
                last_user_message_at: last_user,
                attachments: std::sync::Arc::from(Vec::new()),
                history: None,
                other_chats: std::sync::Arc::from(Vec::new()),
                // A background turn (reflection, consolidation) runs without a
                // chat in front of the user, so it has no attached project and
                // the code tools stay unreachable from it.
                workspace: None,
                workspace_journal: None,
                lang,
                cancel,
                // The same resolver the turn path reads (config first, then
                // the discovered name) — a background turn answers about the
                // same engine a foreground one would.
                model_name: self.effective_model_name(),
                engine_mode: self.config.engine.mode,
                // Outside the session budget, as every background task is
                // (docs/research/parallel-subagents.md fork F9).
                sessions: None,
            },
        )
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
        // Opening a chat is reading it: the unread mark a background run's
        // result left on it goes (spec §11.2), on disk and on the list.
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == id && c.unread) {
            chat.unread = false;
            self.mark_dirty(id);
            self.emit_chat_list();
        }
        // The running turn's chat and sub-agent (docs/subagent-live.md §3.4,
        // §3.5): the parent's feed gets the rounds filed so far and keeps its
        // generation; the child keeps the status chip.
        let turn = self.inflight.as_ref();
        let live_turn = turn.and_then(|t| {
            if t.chat == id {
                Some(Box::new(crate::app::events::LiveTurn {
                    turn: t.generation,
                    stream: t.generation,
                    role: crate::entities::message::MessageRole::Assistant,
                    partial: Some(t.partial.clone()),
                    // Only until the first round files: what files after a
                    // tool round is a message of its own, and the filed
                    // continuation round was folded into the seed's view below.
                    continues: t.continuation && t.rounds.is_empty(),
                    background: false,
                }))
            } else {
                t.child(id).map(|child| {
                    Box::new(crate::app::events::LiveTurn {
                        turn: t.generation,
                        stream: child.stream,
                        role: child.line_role,
                        partial: Some(child.partial.clone()),
                        continues: false,
                        background: false,
                    })
                })
            }
        });
        // A background run's transcript streams under its own id the same
        // way, keyed by the run's own generation
        // (docs/research/background-subagents.md §4.9). Only while the run is
        // **still out**: a seat whose run ended and is waiting for its chat's
        // turn to land (`pending_landings`) has nothing streaming, and saying
        // otherwise would put a stop key on the screen that stops nothing
        // (spec §11.2).
        let live_turn = live_turn.or_else(|| {
            self.background_run(id)
                .filter(|seat| seat.child.run.outcome.is_none())
                .map(|seat| {
                    Box::new(crate::app::events::LiveTurn {
                        turn: seat.generation,
                        stream: seat.child.stream,
                        role: seat.child.line_role,
                        partial: Some(seat.child.partial.clone()),
                        continues: false,
                        background: true,
                    })
                })
        });
        let event = match self.view(id) {
            Some(ChatView::Top(chat)) => {
                let mut messages = chat.messages.clone();
                if let Some(t) = turn.filter(|t| t.chat == id) {
                    let mut rounds = t.rounds.clone();
                    // A continuation turn's first filed round belongs **inside**
                    // the trailing partial (`/continue`, fork F8): fold it into
                    // the view the same way `handle_done` will fold it into the
                    // chat — otherwise the rebuild would show the seam the
                    // append path exists to avoid.
                    if t.continuation
                        && let Some(pos) = rounds.iter().position(|m| {
                            m.role == crate::entities::message::MessageRole::Assistant
                        })
                        && let Some(seed) = messages
                            .last_mut()
                            .filter(|m| m.role == crate::entities::message::MessageRole::Assistant)
                    {
                        generation::merge_continuation(seed, rounds.remove(pos));
                    }
                    messages.extend(rounds);
                }
                AppEvent::ChatActivated {
                    id,
                    title: chat.title.clone(),
                    messages,
                    draft: chat.draft.clone(),
                    feed_view: chat.feed_view,
                    focus,
                    compaction: chat
                        .compaction_view(self.config.compaction.enabled)
                        .map(|(summary, upto)| (chat.messages[upto].id, summary.to_string())),
                    child: None,
                    live_turn: live_turn.clone(),
                }
            }
            // A sub-agent transcript opens like a chat (spec §11.2): its own
            // messages and title, the **parent's** collapse state (it is part
            // of the parent), no draft, no summary, and the persona for the
            // screen to draw first.
            Some(ChatView::Child { parent, run }) => AppEvent::ChatActivated {
                id,
                title: run.title.clone(),
                messages: run.messages.clone(),
                draft: String::new(),
                feed_view: parent.feed_view,
                focus,
                compaction: None,
                child: Some(crate::app::events::ChildView {
                    parent: parent.id,
                    parent_title: parent.title.clone(),
                    system_message: self.child_system_text(run),
                }),
                live_turn: live_turn.clone(),
            },
            None => return,
        };
        self.active_id = Some(id);
        let _ = self.evt_tx.send(event);
        self.emit_character_names();
        self.emit_attachments();
        self.emit_staged_images();
        self.remember_active_chat(id);
    }

    /// Sends the feed the role names of the active chat's profile (spec §5.1).
    /// Called on activation and after a profile edit — so renaming in settings
    /// applies to the open chat right away (the names are resolved from the profile,
    /// not from the chat's creation-time copy).
    pub(super) fn emit_character_names(&self) {
        let names = self
            .active_id
            .map(|id| self.names_of(id))
            .unwrap_or_default();
        let _ = self.evt_tx.send(AppEvent::CharacterNames(names));
    }

    /// The role names to draw for `id`: a chat's are its profile's. A sub-agent
    /// transcript's are resolved **at activation** so they never go stale
    /// (docs/research/subagent-chats.md §3.1): its `User` header is the parent
    /// persona — the profile's assistant name, else the localized assistant
    /// label — because that is who wrote the instruction; its `Assistant`
    /// header is the run's `name`, else the localized "Sub-agent"; the system
    /// name is the profile's.
    pub(super) fn names_of(&self, id: Uuid) -> CharacterNames {
        let Some(view) = self.view(id) else {
            return CharacterNames::default();
        };
        let (chat, run) = match view {
            ChatView::Top(chat) => (chat, None),
            ChatView::Child { parent, run } => (parent, Some(run)),
        };
        let profile_names = self
            .profiles
            .iter()
            .find(|p| p.id == chat.profile_id)
            .map(|p| p.character_names.clone())
            .unwrap_or_default();
        let Some(run) = run else {
            return profile_names;
        };
        let loc = self.ui_locale();
        // A dialogue transcript's sides are its participants (spec §9.13):
        // `Assistant` is participant a, `User` is participant b — the
        // role-encoded transcript of research §3.5.
        if run.kind == crate::entities::subagent::RunKind::Dialogue {
            let label = |i: usize, key: &str| {
                run.participants
                    .get(i)
                    .and_then(|p| p.name.clone())
                    .unwrap_or_else(|| loc.t(key).to_string())
            };
            return CharacterNames {
                assistant: label(0, "ui.feed.role.participant_a"),
                user: label(1, "ui.feed.role.participant_b"),
                system: profile_names.system,
            };
        }
        CharacterNames {
            user: profile_names
                .assistant_name()
                .map(str::to_string)
                .unwrap_or_else(|| loc.t("ui.feed.role.assistant").to_string()),
            assistant: run
                .name
                .clone()
                .unwrap_or_else(|| loc.t("ui.feed.role.subagent").to_string()),
            system: profile_names.system,
        }
    }

    /// The text of a transcript's opening system bubble (spec §11.3). A
    /// sub-agent's is its persona as the parent composed it; a dialogue's
    /// composes both participants' personas and the director's brief
    /// (spec §9.13) — the three texts a reader of the scene wants first.
    fn child_system_text(&self, run: &crate::entities::subagent::SubagentRun) -> String {
        if run.kind != crate::entities::subagent::RunKind::Dialogue {
            return run.system_message.clone();
        }
        let loc = self.ui_locale();
        let mut parts: Vec<String> = Vec::new();
        for (i, p) in run.participants.iter().enumerate() {
            let name = p.name.clone().unwrap_or_else(|| {
                loc.t(if i == 0 {
                    "ui.feed.role.participant_a"
                } else {
                    "ui.feed.role.participant_b"
                })
                .to_string()
            });
            parts.push(format!(
                "{}\n{}",
                loc.tf("ui.feed.dialogue.persona", &[("name", &name)]),
                p.system_message
            ));
        }
        parts.push(format!(
            "{}\n{}",
            loc.t("ui.feed.dialogue.direction"),
            run.system_message
        ));
        parts.join("\n\n")
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
        // The sub-agents running right now, under their parent, marked running
        // (docs/subagent-live.md §3.3); once landed they come from the chat.
        if let Some(turn) = &self.inflight
            && let Some(parent) = summaries.iter_mut().find(|s| s.id == turn.chat)
        {
            for child in &turn.children {
                let mut card = crate::entities::chat::ChildSummary::of(&child.run);
                card.running = child.run.outcome.is_none();
                parent.children.push(card);
            }
        }
        // The runs out in the background: their placeholder card came from
        // the chat; the mirror says *running* and how far it is
        // (docs/research/background-subagents.md §4.9).
        for seat in &self.background_runs {
            let Some(parent) = summaries.iter_mut().find(|s| s.id == seat.chat) else {
                continue;
            };
            let mut card = crate::entities::chat::ChildSummary::of(&seat.child.run);
            card.running = seat.child.run.outcome.is_none();
            card.background = true;
            match parent.children.iter_mut().find(|c| c.id == seat.run_id) {
                Some(existing) => *existing = card,
                None => parent.children.push(card),
            }
        }
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
