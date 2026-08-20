//! Orchestrator tests (no UI, no real model): the generation state machine, races,
//! the agentic loop, sampling priorities, list/profile/RAG operations.

use super::engines::EngineManager;
use super::impersonation::{build_impersonation_request, swap_role_message};
use super::request::{PromptContext, build_request};
use super::restart_queue::RestartQueue;
use super::save_queue::SaveQueue;
use super::title::{TitleOrigin, TitleResult, salvage_title_source};
use super::*;

use crate::app::events::RagProgress;
use crate::entities::chat::FeedView;
use crate::entities::message::{Message, MessageRole};
use crate::features::profiles::ProfileEdit;
use crate::shared::api::{ChatChunk, Embedder, EngineBackend};

use crate::app::supervisor::MockSupervisor;
use crate::shared::api::mock::{MockBackend, MockEmbedder};
use crate::shared::paths::Paths;

fn test_embedder() -> Arc<dyn Embedder> {
    Arc::new(MockEmbedder::new(16))
}

/// The default config with the **automatic chat titling off** (spec §11.2).
///
/// For tests whose engine is a finite script or a last-request capture: the
/// title request the first exchange fires (on by default) would consume a
/// scripted entry out of turn or overwrite the captured request. The trigger
/// itself is covered in `tests/title.rs`, on the real default config.
fn no_auto_cfg() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    cfg
}

/// An engine that remembers the last request it was given and replies with a fixed
/// text — lets a test assert what actually *reached the model*, which is the only way to
/// check an injection (a file in the system prompt, an image on a message) rather than
/// the bookkeeping around it.
///
/// Shared by the attachment and image suites: they ask the same question of the same
/// object, and two copies would drift apart exactly where they must not.
pub(super) struct CapturingBackend {
    pub(super) last: std::sync::Mutex<Option<crate::shared::api::ChatRequest>>,
}

impl CapturingBackend {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            last: std::sync::Mutex::new(None),
        })
    }

    /// The last request the engine was handed. Panics if no turn has run — a test that
    /// asserts on "the request" when there was none is broken, not passing.
    pub(super) fn last_request(&self) -> crate::shared::api::ChatRequest {
        self.last.lock().unwrap().clone().expect("a request")
    }
}

#[async_trait::async_trait]
impl EngineBackend for CapturingBackend {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::shared::api::contract::ChatStream> {
        *self.last.lock().unwrap() = Some(req);
        let s = async_stream::stream! {
            yield ChatChunk::Text("ок".to_string());
            yield ChatChunk::Finished(crate::shared::api::FinishReason::Stop);
        };
        Ok(Box::pin(s))
    }
}

/// Like [`spawn_orch_cfg`], but hands the [`MockSupervisor`] back too — for tests that
/// inspect what the orchestrator *asked of* it: how often a server was raised, and with
/// which resolved key (`MockSupervisor::chat_keys`).
fn spawn_orch_sup(
    config: AppConfig,
) -> (
    tempfile::TempDir,
    Arc<MockSupervisor>,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let sup = Arc::new(MockSupervisor::with_backend(None));
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: sup.clone(),
        default_language: crate::shared::i18n::Lang::default(),
    }));
    (dir, sup, cmd_tx, evt_rx, handle)
}

/// Spins up the orchestrator on a temp storage. Returns the channels and a handle.
fn spawn_orch(
    backend: Option<Arc<dyn EngineBackend>>,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    spawn_orch_cfg(backend, AppConfig::default())
}

/// Like [`spawn_orch`], but with a given config (max_tool_rounds, etc.).
fn spawn_orch_cfg(
    backend: Option<Arc<dyn EngineBackend>>,
    config: AppConfig,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    // Deliberately **file-backed**, even though this fixture never restarts:
    // roughly thirty tests built on it reopen storage afterwards
    // (`Storage::open(Paths::with_root(&root))`) to assert what reached disk.
    // In-memory would not merely fail them — the ones asserting *absence*
    // (`removing_an_attachment_drops_its_index`, `an_inline_file_is_not_indexed`)
    // would keep passing against an always-empty store, i.e. pass vacuously.
    // Measured while trying: 6 of those tests failed outright and 2 more passed
    // for the wrong reason. `bare_orch_rx` and `orchestrator::rag::test_deps`
    // are in memory because no test on them reopens storage.
    let (cmd_tx, evt_rx, handle) = spawn_orch_at(dir.path(), backend, config);
    (dir, cmd_tx, evt_rx, handle)
}

/// Like [`spawn_orch_cfg`], but on an **existing** data root — for two-phase
/// tests that restart the app on the same data (what survived to disk, what a
/// fresh bootstrap makes of it).
///
/// Deliberately **file-backed**, unlike [`spawn_orch_cfg`]: an in-memory
/// database dies with its connection, so a second phase would silently start
/// from an empty `cache.db` and exercise the *rebuild* path instead of the
/// restore one — the test would still pass while covering something else.
fn spawn_orch_at(
    root: &std::path::Path,
    backend: Option<Arc<dyn EngineBackend>>,
    config: AppConfig,
) -> (
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let storage = Arc::new(Storage::open(Paths::with_root(root)).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend(backend)),
        default_language: crate::shared::i18n::Lang::default(),
    };
    let handle = tokio::spawn(run(deps));
    (cmd_tx, evt_rx, handle)
}

/// Drains events until the first one matching the predicate (or the channel closes).
async fn wait_for<F: Fn(&AppEvent) -> bool>(
    rx: &mut UnboundedReceiver<AppEvent>,
    pred: F,
) -> Option<AppEvent> {
    while let Some(ev) = rx.recv().await {
        if pred(&ev) {
            return Some(ev);
        }
    }
    None
}

/// Assembles a "bare" orchestrator for unit-testing pure methods (no loop),
/// discarding the event stream.
fn bare_orch() -> (tempfile::TempDir, Orchestrator) {
    let (dir, orch, _rx) = bare_orch_rx();
    (dir, orch)
}

/// Like [`bare_orch`], but also returns the event receiver (to check emission).
fn bare_orch_rx() -> (tempfile::TempDir, Orchestrator, UnboundedReceiver<AppEvent>) {
    let dir = tempfile::tempdir().unwrap();
    // In-memory: a bare orchestrator is built directly and never restarted.
    let storage = Arc::new(Storage::open_in_memory(Paths::with_root(dir.path())).unwrap());
    let (evt_tx, evt_rx) = unbounded_channel();
    let (done_tx, _done_rx) = unbounded_channel();
    let (status_tx, _status_rx) = unbounded_channel();
    let (title_tx, _title_rx) = unbounded_channel();
    let (imp_status_tx, _imp_status_rx) = unbounded_channel();
    let (embed_status_tx, _embed_status_rx) = unbounded_channel();
    let (imp_done_tx, _imp_done_rx) = unbounded_channel();
    let config = AppConfig {
        default_sampling: SamplingConfig {
            temperature: Some(0.1),
            ..Default::default()
        },
        ..Default::default()
    };
    let registry = Arc::new(build_registry(&config, storage.json().sandbox_dir()));
    // The server manager: immediately "ready" with a test embedder (as the bare
    // orchestrator used to be). Tests set the chat engine themselves when needed.
    let mut engines = EngineManager::new(
        Arc::new(MockSupervisor::with_backend(None)),
        status_tx,
        imp_status_tx,
        embed_status_tx,
    );
    engines.server_status = ServerStatus::Ready;
    engines.embedder = test_embedder();
    let orch = Orchestrator {
        evt_tx,
        engines,
        mcp: McpManager::new(unbounded_channel().0),
        confirm: None,
        imp_cancel: None,
        imp_gen: None,
        imp_done_tx,
        budget_tx: unbounded_channel().0,
        context: Default::default(),
        tts_cancel: None,
        tts_gen: None,
        tts_playback: None,
        tts_done_tx: unbounded_channel().0,
        storage,
        config,
        registry,
        title_tx,
        // No loop drains this either: compaction tests that need a roll's result
        // go through `spawn_orch_cfg` (the real `run` loop), the ones about
        // *applying* one call `handle_compact_result` directly.
        compact_tx: unbounded_channel().0,
        profiles: Vec::new(),
        chats: Vec::new(),
        active_id: None,
        gen_state: GenState::Idle,
        done_tx,
        rag_cancel: None,
        // The bare orchestrator has no loop draining this channel; attachment
        // tests go through `spawn_orch` (the real `run` loop) end to end.
        attach_tx: unbounded_channel().0,
        // Same for images: staging is exercised through the real loop.
        image_tx: unbounded_channel().0,
        staged_images: Default::default(),
        bg: std::collections::HashMap::new(),
        bg_done_tx: unbounded_channel().0,
        consolidate_counts: std::collections::HashMap::new(),
        self_consolidate_counts: std::collections::HashMap::new(),
        saves: SaveQueue::default(),
        restarts: RestartQueue::default(),
        default_language: crate::shared::i18n::Lang::default(),
    };
    (dir, orch, evt_rx)
}

/// Enables the whole tool catalog on the profile (including the control followup/
/// rewrite tools) — for control-tool tests. Returns the profile's id.
async fn enable_all_tools(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
) -> Uuid {
    use crate::features::tools::all_tool_ids;
    let pl = wait_for(evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v[0].id,
        _ => unreachable!(),
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(all_tool_ids()),
                ..Default::default()
            }),
        })
        .unwrap();
    pid
}

/// The real chat engine from `MINDFORK_ENGINE_URL` (for end-to-end smokes against a live
/// model). `None` — the variable isn't set (the test is skipped).
///
/// **Wrapped in `RetryBackend`, exactly as the supervisor wraps an external
/// server** (spec §6.8). Without this the whole live e2e set would bypass the
/// decorator that sits on every real cloud and external turn — it would be covered
/// by unit tests alone, and a mistake in how it hands a stream over (the head it
/// replays, the commit point, the delegated `context_budget` the compaction trigger
/// reads) would not show up against a real model. Since this backend does not fail
/// transiently in practice, the wrapper is invisible here apart from being
/// exercised.
fn live_backend() -> Option<Arc<dyn EngineBackend>> {
    let client = crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")?;
    Some(crate::shared::api::retry::RetryBackend::wrap(Arc::new(
        client,
    )))
}

/// The real embedder from `MINDFORK_EMBED_URL` (for live smokes — bge-m3 etc.);
/// `None` when unset → the live smoke falls back to the test `MockEmbedder`.
fn live_embedder() -> Option<Arc<dyn Embedder>> {
    let client = crate::shared::api::live_client("MINDFORK_EMBED_URL", "MINDFORK_EMBED_KEY")?;
    Some(Arc::new(client) as Arc<dyn Embedder>)
}

/// The tuple of a spun-up orchestrator (like [`spawn_orch`]): the data directory,
/// the command channel, the event receiver, the loop's handle.
type OrchHandle = (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
);

/// Spins up the orchestrator for a live smoke: chat from `MINDFORK_ENGINE_URL`, an
/// embedder from `MINDFORK_EMBED_URL` (a real server; otherwise a deterministic `MockEmbedder`).
/// `None` when `MINDFORK_ENGINE_URL` is unset (the smoke is skipped).
fn spawn_orch_live() -> Option<OrchHandle> {
    spawn_orch_live_cfg(AppConfig::default())
}

/// Like [`spawn_orch_live`], but with an explicit config (e.g. an MCP host enabled
/// with a real server for an e2e smoke).
fn spawn_orch_live_cfg(config: AppConfig) -> Option<OrchHandle> {
    spawn_orch_live_with(config, true)
}

/// Like [`spawn_orch_live_cfg`], but deliberately **without** an embedder, even
/// when `MINDFORK_EMBED_URL` is set.
///
/// For smokes that must exercise a path the model would otherwise be able to
/// shortcut. With an embedder present a by-reference attachment gets indexed,
/// and `attachment_search` becomes a legitimate — often better — route, so
/// "the model paged through the file" stops being a property of the code and
/// becomes a property of the model's mood that day. Removing the embedder
/// removes the alternative instead of hoping it is not taken.
///
/// Note `MockSupervisor::with_backend_no_embedder`: passing `None` as the
/// embedder is **not** enough, since the mock then falls back to a
/// `MockEmbedder` and the attachment still gets indexed.
fn spawn_orch_live_no_embed(config: AppConfig) -> Option<OrchHandle> {
    spawn_orch_live_with(config, false)
}

fn spawn_orch_live_with(config: AppConfig, with_embedder: bool) -> Option<OrchHandle> {
    let backend = live_backend()?;
    let supervisor: Arc<dyn crate::app::supervisor::ServerSupervisor> = if with_embedder {
        Arc::new(MockSupervisor::with_backend_and_embedder(
            Some(backend),
            live_embedder(),
        ))
    } else {
        Arc::new(MockSupervisor::with_backend_no_embedder(Some(backend)))
    };
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor,
        default_language: crate::shared::i18n::Lang::default(),
    };
    let handle = tokio::spawn(run(deps));
    Some((dir, cmd_tx, evt_rx, handle))
}

/// Drains events until `Finished` (or the channel closes), flagging whether a
/// `pred`-matching event occurred along the way. For control-tool end-to-end smokes.
async fn drain_until_finished<F: Fn(&AppEvent) -> bool>(
    rx: &mut UnboundedReceiver<AppEvent>,
    pred: F,
) -> bool {
    let mut saw = false;
    while let Some(ev) = rx.recv().await {
        if pred(&ev) {
            saw = true;
        }
        if matches!(ev, AppEvent::Finished { .. }) {
            break;
        }
    }
    saw
}

/// Runs one turn: sends a message, drains events until `Finished`, collecting
/// the reply text and the names of called tools. For live smokes.
#[cfg(test)]
async fn run_turn_live(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> (String, Vec<String>) {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    let mut tools = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => out.push_str(text),
            AppEvent::ToolCall { name, .. } => tools.push(name.clone()),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    (out, tools)
}

/// Like [`run_turn_live`], but collects pairs (tool name, result) — to check the
/// result text (e.g. the `add_insight` gate firing).
async fn run_turn_capture(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> (String, Vec<(String, String)>) {
    let (out, calls) = run_turn_capture_args(cmd_tx, evt_rx, text).await;
    (out, calls.into_iter().map(|(n, _, r)| (n, r)).collect())
}

/// Like [`run_turn_capture`], but keeps the call **arguments** too — triples
/// (tool name, arguments, result). Needed when the result's shape depends on
/// what the model passed, so the assertion can be made against the shape it
/// actually chose rather than the one it was expected to choose.
async fn run_turn_capture_args(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> (String, Vec<(String, String, String)>) {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    let mut calls = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => out.push_str(text),
            AppEvent::ToolCall {
                name,
                arguments,
                result,
                ..
            } => calls.push((name.clone(), arguments.clone(), result.clone())),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    (out, calls)
}

/// Prepares a bare orchestrator with a profile + an active chat (for F3 edits).
fn orch_with_active_profile() -> (tempfile::TempDir, Orchestrator, Uuid) {
    let (dir, mut orch) = bare_orch();
    let profile = Profile::new("P", "sys");
    let pid = profile.id;
    let chat = Chat::from_profile(&profile, "t");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, pid)
}

/// Prepares an orchestrator with a chat (user+assistant) and a profile that enabled the
/// self-model; `auto_reflect_every=1`. Returns `(dir, orch, chat_id)`.
fn orch_ready_for_reflection() -> (tempfile::TempDir, Orchestrator, Uuid) {
    use crate::features::tools::self_model::GET_SELF_MODEL_ID;
    let (dir, mut orch) = bare_orch();
    orch.config.self_model.auto_reflect_every = 1;
    let mut profile = Profile::new("P", "sys");
    profile.enabled_tools = vec![GET_SELF_MODEL_ID.into()];
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуй"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, chat_id)
}

/// Prepares an orchestrator with a chat (user+assistant), a profile with the self-model
/// enabled, and two `@self` observation-notes in the DB (the signal "there's something
/// to consolidate"); `auto_consolidate_every=1`. Returns `(dir, orch, chat_id)`.
/// See docs/history/self-model-consolidation.md (stage A1).
fn orch_ready_for_self_consolidation() -> (tempfile::TempDir, Orchestrator, Uuid) {
    use crate::entities::note::Note;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    use crate::features::tools::self_model::GET_SELF_MODEL_ID;
    let (dir, mut orch) = bare_orch();
    orch.config.self_model.auto_consolidate_every = 1;
    let mut profile = Profile::new("P", "sys");
    profile.enabled_tools = vec![
        GET_SELF_MODEL_ID.into(),
        "note_merge".into(),
        "update_self_model".into(),
    ];
    let pid = profile.id;
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуй"));
    let chat_id = chat.id;
    // Two observation-notes (@self) — the self-consolidation overview is non-empty (observations ≥ 2).
    for text in ["я ценю краткость", "пользователь любит лаконичность"]
    {
        let note = Note::new(pid, text, vec![SELF_NOTE_TAG.to_string()]);
        orch.storage.db().note_insert(&note).unwrap();
    }
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, chat_id)
}

// ---------- test submodules (god-object breakup: docs/history/refactoring-god-objects.md, stage 3) ----------

mod attachments;
mod chats;
mod compaction;
mod confirm;
mod demo;
mod generation;
mod images;
mod impersonation;
mod live;
mod mcp;
mod profiles;
mod project;
mod rag;
mod reflection;
mod request;
mod search;
mod self_consolidation;
mod self_model;
mod settings;
mod title;
mod tts;
