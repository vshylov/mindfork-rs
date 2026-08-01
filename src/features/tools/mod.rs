//! Contract for the tool system (`features/tools`): the [`Tool`] trait, the
//! [`ToolContext`] snapshot, the [`ToolOutcome`] result with [`ChatEffect`] effects,
//! and the [`ToolRegistry`] registry. See spec §9.2, §6.3.
//!
//! Tools **don't** mutate `Chat` directly: mutating ones return `effects`, which the
//! orchestrator applies (sole owner of `Chat`, spec §4.4.2). Memory/knowledge tools
//! must filter by `ctx.profile_id` (isolation, a repository invariant, spec §9.5).

#[cfg(test)]
mod embed_roles_tests;

pub mod attachment;
pub mod calc;
pub mod confirm;
pub mod control;
pub mod datetime;
pub mod fetch;
pub mod fs;
pub mod introspection;
pub mod mcp;
pub mod meta;
pub mod notes;
pub mod present;
pub mod python;
pub mod rag;
pub mod self_model;
pub mod subagent;
pub mod web;
pub mod youtube;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entities::profile::ToolId;
use crate::entities::sampling::{SamplingConfig, supported_sampling_fields};
use crate::entities::self_model::SelfModelParams;
use crate::shared::api::{Embedder, EngineBackend, ToolSchema};
use crate::shared::config::{AppConfig, CloudProvider, PythonMode};
use crate::shared::sandbox::WasmerSandbox;
use crate::shared::storage::Storage;

pub use introspection::{GET_SAMPLING_ID, SET_SAMPLING_ID};

/// Immutable snapshot of turn state (no shared locks). See spec §9.2.
#[derive(Clone)]
pub struct ToolContext {
    pub profile_id: Uuid,
    /// Id of the current chat (part of the turn snapshot). Scopes the attachment
    /// index — `attachment_search` never reaches another conversation's files.
    pub chat_id: Uuid,
    /// Snapshot of `Chat.system_message` at the start of the turn.
    pub system_message: String,
    /// Effective sampling (after the priority resolution, §8.3).
    pub effective_sampling: SamplingConfig,
    /// Timestamp of the last user message (if any).
    pub last_user_message_at: Option<DateTime<Utc>>,
    pub storage: Arc<Storage>,
    /// Chat engine (for `call_subagent`, M6).
    pub engine: Arc<dyn EngineBackend>,
    /// Embedding source (RAG); a dedicated server — see ADR 0002.
    pub embedder: Arc<dyn Embedder>,
    /// RAG chunking parameters from settings (`config.rag`, spec §9.3).
    pub chunk_params: rag::ChunkParams,
    /// Render/storage parameters for the "self-model" from settings
    /// (`config.self_model`). A snapshot of the model itself is **not** put into the
    /// context: SelfModel tools read/write fresh state directly through `storage`
    /// (to see edits made within the turn), while injecting the model into the
    /// prompt is a separate path in the orchestrator.
    pub self_model_params: SelfModelParams,
    /// Whether to show self-notes (`@self`) in the general `note_recall` (Tier 3,
    /// Path 2). From `config.notes.recall_includes_self`; `false` by default (self
    /// hidden).
    pub recall_includes_self: bool,
    /// Language of the **agent scaffold** for this turn (from `Profile.language`,
    /// axis A, docs/history/i18n.md). Text the model reads (the "self-model"
    /// scaffold, tool results) is localized through it. `&'static` — a built-in
    /// bundle.
    pub loc: &'static crate::shared::i18n::Locale,
    /// Files attached to the chat (`/file attach`) — a turn snapshot, like
    /// `system_message`. `Arc` because [`ToolContext`] is `Clone` and an
    /// attachment's text can be hundreds of KB. Read by `attachment_read`; empty
    /// for background tasks (they have no chat). See spec §9.7.
    pub attachments: std::sync::Arc<[crate::entities::attachment::Attachment]>,
    /// Attachment budget and page size (`config.attachments`): the page size for
    /// `attachment_read`, and — for a tool that produces an attachment of its own
    /// — the same thresholds the orchestrator decides the mode with.
    pub attachment_cfg: crate::shared::config::AttachmentSettings,
    /// Cancellation token for the turn (user Esc / background-task timeout): a
    /// long-running tool (MCP `tools/call`, network) must break on it rather than
    /// block cancellation. The agentic loop additionally wraps `invoke` in a
    /// `select!` with the same token — a safety net for tools that don't read the
    /// token. See docs/research/plugin-system.md §4.4 ("Cancellation").
    pub cancel: tokio_util::sync::CancellationToken,
}

/// Long-lived shared tool dependencies (an `Arc` bundle; changes on server
/// restart, not turn to turn). Gathered into one block so a new dependency doesn't
/// touch every [`ToolContext`] build site. See docs/history/refactoring-solid.md §3.
#[derive(Clone)]
pub struct ToolDeps {
    pub storage: Arc<Storage>,
    pub engine: Arc<dyn EngineBackend>,
    pub embedder: Arc<dyn Embedder>,
}

/// Tool parameters from config (a per-turn snapshot). The single place that maps
/// `AppConfig` → tool parameters — [`ToolParams::from_config`].
#[derive(Clone)]
pub struct ToolParams {
    pub chunk_params: rag::ChunkParams,
    pub self_model_params: SelfModelParams,
    pub recall_includes_self: bool,
    /// Attachment budget and page size (`config.attachments`). A tool that
    /// produces an attachment of its own needs the same numbers the orchestrator
    /// uses, or it would describe to the model something other than what gets
    /// stored.
    pub attachments: crate::shared::config::AttachmentSettings,
}

impl ToolParams {
    /// Snapshots tool parameters from the application configuration.
    pub fn from_config(cfg: &AppConfig) -> Self {
        Self {
            chunk_params: rag::ChunkParams::from_settings(&cfg.rag),
            self_model_params: SelfModelParams::from_settings(&cfg.self_model),
            recall_includes_self: cfg.notes.recall_includes_self,
            attachments: cfg.attachments,
        }
    }
}

/// Turn snapshot: what a tool sees about the current chat (identity + `Chat` snapshot).
pub struct TurnInfo {
    pub profile_id: Uuid,
    pub chat_id: Uuid,
    pub system_message: String,
    pub effective_sampling: SamplingConfig,
    pub last_user_message_at: Option<DateTime<Utc>>,
    /// Files attached to the chat (a `Chat` snapshot; empty for background tasks).
    pub attachments: std::sync::Arc<[crate::entities::attachment::Attachment]>,
    /// Language of the turn's agent scaffold (from `Profile.language`, axis A).
    pub lang: crate::shared::i18n::Lang,
    /// Cancellation token for the turn (a clone of the generation task's /
    /// background loop's token).
    pub cancel: tokio_util::sync::CancellationToken,
}

impl ToolContext {
    /// Unpacks the building blocks into the former flat fields. The flat shape is
    /// kept deliberately — tool code (`ctx.storage`, `ctx.chunk_params`, …) doesn't
    /// change. See docs/history/refactoring-solid.md §3.
    pub fn new(deps: ToolDeps, params: ToolParams, turn: TurnInfo) -> Self {
        Self {
            profile_id: turn.profile_id,
            chat_id: turn.chat_id,
            system_message: turn.system_message,
            effective_sampling: turn.effective_sampling,
            last_user_message_at: turn.last_user_message_at,
            attachments: turn.attachments,
            attachment_cfg: params.attachments,
            storage: deps.storage,
            engine: deps.engine,
            embedder: deps.embedder,
            chunk_params: params.chunk_params,
            self_model_params: params.self_model_params,
            recall_includes_self: params.recall_includes_self,
            loc: crate::shared::i18n::locale(turn.lang),
            cancel: turn.cancel,
        }
    }
}

/// An effect that mutates `Chat`; returned by a tool, applied by the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEffect {
    /// Replace the chat's system message (takes effect from the next request build).
    SetSystemMessage(String),
    /// Replace the chat's sampling override (takes effect from the next turn).
    /// `Box`, since `SamplingConfig` is larger than the other variants (clippy
    /// `large_enum_variant`).
    SetSamplingOverride(Box<SamplingConfig>),
    /// Attach text the tool produced to the chat (spec §9.7), replacing any
    /// attachment with the same `source`. A video transcript is the first user
    /// (spec §9.9), and the variant is deliberately generic — it is the natural
    /// home for any later "this tool produced too much text to hand back inline".
    ///
    /// The attachment arrives **already built**, mode included: the tool has
    /// told the model what it did, and the object described has to be the object
    /// stored — down to the `id`, which is the key the background index is
    /// written under. The agentic loop additionally mirrors it into the turn's
    /// `ToolContext` snapshot, so `attachment_read` finds it in the very next
    /// round rather than only in the next turn (docs/history/youtube-transcript.md §3 F1).
    AddAttachment(Box<crate::entities::attachment::Attachment>),
}

/// Result of a tool call: text for the model + effects for the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub result: String,
    pub effects: Vec<ChatEffect>,
}

impl ToolOutcome {
    /// A result with no effects (a pure tool).
    pub fn text(result: impl Into<String>) -> Self {
        Self {
            result: result.into(),
            effects: Vec::new(),
        }
    }

    /// A result with effects (a mutating tool).
    pub fn with_effects(result: impl Into<String>, effects: Vec<ChatEffect>) -> Self {
        Self {
            result: result.into(),
            effects,
        }
    }
}

/// A tool executed by the client-side agentic loop. See spec §9.2.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Unique name (matches the function name in the OpenAI schema).
    fn id(&self) -> ToolId;
    /// Human-readable description for the model **in the agent-scaffold language**
    /// `loc` (axis A, docs/history/i18n.md). Tools not yet translated (Tier 2 rolls
    /// out by group) return Russian text regardless of `loc`.
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String;
    /// JSON Schema of the parameter object (field descriptions — in the language `loc`).
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value;
    /// Executes the call. `args` — the model's parsed argument JSON.
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome>;

    /// Schema to hand to the server (built from `id`/`description`/`parameters` by
    /// default) in the language `loc`.
    fn schema(&self, loc: &crate::shared::i18n::Locale) -> ToolSchema {
        ToolSchema {
            name: self.id(),
            description: self.description(loc),
            parameters: self.parameters(loc),
        }
    }

    /// Semantic group for profile toggles (settings UI).
    fn group(&self) -> meta::ToolGroup;

    /// Short (2-4 word) label for the profile toggle (unlike the LLM-oriented
    /// [`Tool::description`]).
    fn ui_label(&self) -> &'static str;

    /// Global switch gating the tool (`None` — not gated). See
    /// [`effective_tool_ids`].
    fn gate(&self) -> Option<meta::ToolGate> {
        None
    }

    /// Whether a call to this tool changes something **outside the application**
    /// and should therefore be shown to the user first, when
    /// `tools.confirm_dangerous` is on (spec §9.8, fork F1 of
    /// docs/history/tool-confirmation.md).
    ///
    /// The default is `false` — safe — so a new tool is only asked about when its
    /// author says so. That is the right default here precisely because the
    /// switch is opt-in: a wrong `false` costs the user a confirmation they
    /// wanted, while a wrong `true` on, say, `current_time` would train them to
    /// press `Enter` without reading, which is worse than not asking.
    ///
    /// Writes to **our own** storage (notes, the self-model, RAG, attachments)
    /// are deliberately not dangerous: they are visible in the UI, scoped to the
    /// profile, and reversible by the same tools that wrote them.
    fn danger(&self) -> bool {
        false
    }

    /// Whether the tool is enabled in the profile by default (`false` — optional,
    /// enabled manually). See [`default_tool_ids`]/[`all_tool_ids`].
    fn enabled_by_default(&self) -> bool {
        true
    }
}

/// Name of the web tool (gated by the global switch `tools.web_enabled`).
pub const WEB_SEARCH_ID: &str = "web_search";
/// Name of the URL-fetch tool (gated by `tools.web_enabled` — network access).
pub const FETCH_URL_ID: &str = "fetch_url";
/// Name of the Python tool (gated by `tools.python_enabled`).
pub const PYTHON_EXEC_ID: &str = "python_exec";
/// Name of the video tool (gated by `tools.web_enabled` — network access).
pub const YOUTUBE_WATCH_ID: &str = "youtube_watch";

/// Metadata snapshot of all tools (a single source — the tools themselves via the
/// [`Tool`] trait). Metadata (group/label/gate/default) doesn't depend on
/// [`ToolConfig`], so the catalog is built once on the default config — this avoids
/// rebuilding the registry in the hot [`effective_tool_ids`] (called on every
/// agentic-loop round).
static CATALOG: LazyLock<Vec<meta::ToolInfo>> =
    LazyLock::new(|| standard_registry(&ToolConfig::default()).infos());

/// Catalog of metadata for all known tools (a snapshot of [`CATALOG`]). Order —
/// alphabetical by id (the registry is a `BTreeMap`). Consumers use the id by value
/// (membership/iteration), not by position.
pub fn tool_catalog() -> Vec<meta::ToolInfo> {
    CATALOG.clone()
}

/// Ids of tools enabled in a profile by default (M5-M7). External ones
/// (`web_search`/`python_exec`) are additionally gated by global switches — see
/// [`effective_tool_ids`]. Conversation-control tools and the "self-model" are
/// optional (off by default, `Tool::enabled_by_default`), see [`all_tool_ids`].
/// Derived from [`CATALOG`] (a single source — the tools themselves).
pub fn default_tool_ids() -> Vec<ToolId> {
    CATALOG
        .iter()
        .filter(|i| i.enabled_by_default)
        .map(|i| i.id.clone())
        .collect()
}

/// Full catalog of tool ids for profile toggles: default + optional (off by
/// default — conversation-control tools and the "self-model"). Unlike
/// [`default_tool_ids`], this includes the optional ones — so the user sees them in
/// profile settings and can enable them, but `reconcile_tools` does **not** enable
/// them automatically. Derived from [`CATALOG`]. See spec §9.3.
// The profile-toggle catalog pulls metadata via [`tool_catalog`]; this id helper is
// currently used by tests (fixtures/catalog) — kept as public API.
#[allow(dead_code)]
pub fn all_tool_ids() -> Vec<ToolId> {
    CATALOG.iter().map(|i| i.id.clone()).collect()
}

/// Effective tool set: `enabled` minus external ones disabled by global switches
/// (spec §9.4). `enabled`'s order is preserved. `web_enabled` gates both
/// `web_search` and `fetch_url` (both — network access); `fs_enabled` — the file
/// tools `fs_read`/`fs_write`/`fs_list` (the gate is taken from the tool's
/// metadata, [`Tool::gate`]). Sampling tools (`get_sampling`/`set_sampling`) are
/// disabled if the current engine mode has no available parameter at all
/// (`sampling_provider`, see [`supported_sampling_fields`]) — that's a dynamic gate
/// by provider, so it's handled separately from the static [`meta::ToolGate`].
/// MCP-server tools (id with the `mcp__` prefix) are gated by `mcp_enabled` **by
/// prefix**: they're dynamic and absent from the static [`CATALOG`].
pub fn effective_tool_ids(
    enabled: &[ToolId],
    web_enabled: bool,
    python_enabled: bool,
    fs_enabled: bool,
    mcp_enabled: bool,
    sampling_provider: Option<CloudProvider>,
) -> Vec<ToolId> {
    let sampling_available = !supported_sampling_fields(sampling_provider).is_empty();
    let gate_of = |id: &str| CATALOG.iter().find(|i| i.id == id).and_then(|i| i.gate);
    enabled
        .iter()
        .filter(|id| {
            if id.as_str() == GET_SAMPLING_ID || id.as_str() == SET_SAMPLING_ID {
                return sampling_available;
            }
            if id.starts_with(mcp::MCP_TOOL_PREFIX) {
                return mcp_enabled;
            }
            match gate_of(id) {
                Some(meta::ToolGate::Web) => web_enabled,
                Some(meta::ToolGate::Python) => python_enabled,
                Some(meta::ToolGate::Fs) => fs_enabled,
                Some(meta::ToolGate::Mcp) => mcp_enabled,
                None => true,
            }
        })
        .cloned()
        .collect()
}

/// Parameters for building the tool registry from configuration (`config.tools`,
/// spec §11.6). Let the registry be rebuilt on live settings edits.
#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Execution mode for `python_exec` (Wasmer sandbox / local interpreter).
    pub python_mode: PythonMode,
    /// Path to the Python interpreter for `python_exec` (`None` → system, Local mode).
    pub python_path: Option<String>,
    /// Allow network in the Wasmer sandbox (`--net`).
    pub python_net: bool,
    /// Execution timeout in the Wasmer sandbox.
    pub python_wasm_timeout: Duration,
    /// Hard sandbox memory limit (MB; `None` — no limit). Windows only.
    pub python_wasm_memory_mb: Option<u64>,
    /// Sandbox directory (`data/sandbox/`) with the `wasmer` binary and assets
    /// (`None` — no directory, sandbox only via the env override).
    pub sandbox_dir: Option<PathBuf>,
    /// Response token limit for `call_subagent`.
    pub subagent_max_tokens: usize,
    /// Time limit for a `call_subagent` call.
    pub subagent_timeout: Duration,
    /// Default for `web_search.fetch_content` (fetching/reranking pages,
    /// `config.tools.web_fetch_content`). The call argument overrides it.
    pub web_fetch_content: bool,
    /// "Sandbox" directory for file tools (`None` → no restriction).
    pub fs_root: Option<String>,
    /// Resolved video-understanding slot for `youtube_watch` (`None` — not
    /// configured; the tool then degrades to metadata). Independent of the chat
    /// engine — see `shared::video`.
    pub video: Option<crate::shared::video::VideoConfig>,
    /// Cloud provider of the chat engine (`None` — local/external). Determines
    /// which sampling parameters `get_sampling`/`set_sampling` see/change (schema
    /// and result filtering) — a mirror of the wire dialect. See ADR 0004.
    pub sampling_provider: Option<CloudProvider>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            python_mode: PythonMode::default(),
            python_path: None,
            python_net: true,
            python_wasm_timeout: Duration::from_secs(
                crate::shared::config::DEFAULT_PYTHON_WASM_TIMEOUT_SECS,
            ),
            python_wasm_memory_mb: None,
            sandbox_dir: None,
            subagent_max_tokens: crate::shared::config::DEFAULT_SUBAGENT_MAX_TOKENS,
            subagent_timeout: Duration::from_secs(
                crate::shared::config::DEFAULT_SUBAGENT_TIMEOUT_SECS,
            ),
            web_fetch_content: true,
            fs_root: None,
            video: None,
            sampling_provider: None,
        }
    }
}

/// Registry with all tools (M5-M7) per [`ToolConfig`] parameters. Global switches
/// aren't applied here, but when selecting the effective set (see
/// [`effective_tool_ids`]).
pub fn standard_registry(cfg: &ToolConfig) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(introspection::GetSampling::new(
        cfg.sampling_provider,
    )));
    reg.register(Arc::new(introspection::SetSampling::new(
        cfg.sampling_provider,
    )));
    reg.register(Arc::new(introspection::GetSystemMessage));
    reg.register(Arc::new(introspection::SetSystemMessage));
    reg.register(Arc::new(introspection::GetLastUserMessageTime));
    reg.register(Arc::new(notes::NoteSave));
    reg.register(Arc::new(notes::NoteRecall));
    reg.register(Arc::new(notes::NoteRevise));
    reg.register(Arc::new(notes::NoteLink));
    reg.register(Arc::new(notes::NoteNeighbors));
    reg.register(Arc::new(notes::NoteSupersede));
    reg.register(Arc::new(notes::NoteMerge));
    reg.register(Arc::new(notes::ConsolidateNotes));
    reg.register(Arc::new(notes::NoteCiteSource));
    reg.register(Arc::new(rag::RagAdd));
    reg.register(Arc::new(rag::RagSearch));
    reg.register(Arc::new(subagent::CallSubagent::new(
        cfg.subagent_max_tokens,
        cfg.subagent_timeout,
    )));
    reg.register(Arc::new(web::WebSearch::new(cfg.web_fetch_content)));
    reg.register(Arc::new(fetch::FetchUrl::new()));
    // Video understanding is a slot of its own (only Gemini takes video at all),
    // so the tool gets a client built from `cfg.video` rather than `ctx.engine`.
    // Unconfigured → registered anyway, degrading to metadata (fork R5a).
    reg.register(Arc::new(youtube::YoutubeWatch::new(
        cfg.video.clone().map(|c| {
            Arc::new(crate::shared::video::gemini::GeminiVideo::new(c))
                as Arc<dyn crate::shared::video::VideoUnderstanding>
        }),
        cfg.video
            .as_ref()
            .map_or(crate::shared::config::DEFAULT_VIDEO_MAX_MINUTES, |c| {
                c.max_minutes
            }),
    )));
    reg.register(Arc::new(python::PythonExec::new(
        cfg.python_mode,
        cfg.python_path.clone(),
        Arc::new(
            WasmerSandbox::new(cfg.sandbox_dir.clone())
                .with_memory_limit(cfg.python_wasm_memory_mb),
        ),
        cfg.python_net,
        cfg.python_wasm_timeout,
    )));
    reg.register(Arc::new(calc::Calculate));
    reg.register(Arc::new(datetime::CurrentTime));
    reg.register(Arc::new(fs::FsRead::new(cfg.fs_root.clone())));
    reg.register(Arc::new(fs::FsWrite::new(cfg.fs_root.clone())));
    reg.register(Arc::new(fs::FsList::new(cfg.fs_root.clone())));
    // Reading/searching files the user attached to the chat (`/file attach`). Not
    // gated: unlike fs_read they can only reach what the user explicitly attached.
    reg.register(Arc::new(attachment::AttachmentRead));
    reg.register(Arc::new(attachment::AttachmentSearch));
    // Conversation-control tools (optional, gated by the profile's set).
    reg.register(Arc::new(control::SendFollowupMessage));
    reg.register(Arc::new(control::RewriteCurrentMessage));
    // "Self-model" tools (optional, DB-only, gated by the profile's set).
    reg.register(Arc::new(self_model::GetSelfModel));
    reg.register(Arc::new(self_model::Reflect));
    reg.register(Arc::new(self_model::UpdateSelfModel));
    reg.register(Arc::new(self_model::UpdateUserModel));
    reg.register(Arc::new(self_model::AddInsight));
    reg
}

/// Tool registry: maps names to implementations, hands schemas to the engine.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<ToolId, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a tool (overwrites on a matching id).
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.id(), tool);
    }

    /// Tool by name (used by registry tests).
    #[allow(dead_code)]
    pub fn get(&self, id: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(id)
    }

    /// Metadata snapshot of all registered tools (for the UI catalog).
    pub fn infos(&self) -> Vec<meta::ToolInfo> {
        self.tools
            .values()
            .map(|t| meta::ToolInfo {
                id: t.id(),
                group: t.group(),
                label: t.ui_label(),
                gate: t.gate(),
                enabled_by_default: t.enabled_by_default(),
                description: None,
            })
            .collect()
    }

    /// Schemas for a subset of enabled tools (profile ∩ global), preserving
    /// `enabled`'s order. Unknown names are ignored.
    pub fn schemas_for(
        &self,
        enabled: &[ToolId],
        loc: &crate::shared::i18n::Locale,
    ) -> Vec<ToolSchema> {
        enabled
            .iter()
            .filter_map(|id| self.tools.get(id))
            .map(|t| t.schema(loc))
            .collect()
    }

    /// Executes a tool by name. Errors if the tool is unknown.
    pub async fn invoke(
        &self,
        id: &str,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolOutcome> {
        match self.tools.get(id) {
            Some(tool) => tool.invoke(ctx, args).await,
            None => anyhow::bail!("unknown tool: {id}"),
        }
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Utilities for tool tests: building a [`ToolContext`] over temp storage with a
    //! mock engine and a deterministic embedder.

    use super::*;
    use crate::shared::api::EmbedRole;
    use crate::shared::api::mock::{MockBackend, MockEmbedder};
    use crate::shared::paths::Paths;

    /// Default tool parameters for tests.
    fn test_params() -> ToolParams {
        ToolParams {
            chunk_params: rag::ChunkParams::default(),
            self_model_params: SelfModelParams::default(),
            recall_includes_self: false,
            attachments: crate::shared::config::AttachmentSettings::default(),
        }
    }

    /// Default turn snapshot for tests (profile given, chat is new).
    fn test_turn(profile_id: Uuid) -> TurnInfo {
        TurnInfo {
            profile_id,
            chat_id: Uuid::new_v4(),
            system_message: "системное сообщение".into(),
            effective_sampling: SamplingConfig::default(),
            last_user_message_at: None,
            // No attachments by default; tests that need them set `ctx.attachments`.
            attachments: std::sync::Arc::from(Vec::new()),
            lang: crate::shared::i18n::Lang::Ru,
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Context of a tool over temp storage. Also returns `TempDir` (keep alive)
    /// and `Arc<Storage>` (for checks in the test).
    pub fn ctx_with_storage(profile_id: Uuid) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        ctx_with_storage_lang(profile_id, crate::shared::i18n::Lang::Ru)
    }

    /// Like [`ctx_with_storage`], but with an explicit scaffold language (for
    /// checking tool-result localization — e.g. `python_exec` on an en profile).
    pub fn ctx_with_storage_lang(
        profile_id: Uuid,
        lang: crate::shared::i18n::Lang,
    ) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let deps = ToolDeps {
            storage: storage.clone(),
            engine: Arc::new(MockBackend::scripted(vec![])),
            embedder: Arc::new(MockEmbedder::new(16)),
        };
        let mut turn = test_turn(profile_id);
        turn.lang = lang;
        let ctx = ToolContext::new(deps, test_params(), turn);
        (dir, storage, ctx)
    }

    /// Context of a tool over a ready dependency bundle (tests where several
    /// contexts share one storage — e.g. profile isolation).
    pub fn ctx_with_deps(profile_id: Uuid, deps: ToolDeps) -> ToolContext {
        ToolContext::new(deps, test_params(), test_turn(profile_id))
    }

    /// An embedder that records the role every text was embedded under, on top of
    /// the usual deterministic mock.
    ///
    /// The role is invisible in the result — a wrong one changes no return value,
    /// only the quality of a comparison on a model that uses input prefixes — so
    /// recording it is the only way a test can see it at all (research
    /// docs/research/embedding-input-prefixes.md §2.3, §5).
    pub struct RoleRecorder {
        inner: MockEmbedder,
        pub calls: std::sync::Mutex<Vec<(Vec<String>, EmbedRole)>>,
    }

    impl RoleRecorder {
        pub fn new() -> Self {
            Self {
                inner: MockEmbedder::new(16),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        /// Roles recorded so far, in call order.
        pub fn roles(&self) -> Vec<EmbedRole> {
            self.calls.lock().unwrap().iter().map(|(_, r)| *r).collect()
        }

        /// Whether every recorded call used `role` (and at least one happened).
        pub fn all_were(&self, role: EmbedRole) -> bool {
            let roles = self.roles();
            !roles.is_empty() && roles.iter().all(|r| *r == role)
        }
    }

    #[async_trait::async_trait]
    impl Embedder for RoleRecorder {
        async fn embed(
            &self,
            texts: Vec<String>,
            role: EmbedRole,
        ) -> anyhow::Result<Vec<Vec<f32>>> {
            self.calls.lock().unwrap().push((texts.clone(), role));
            self.inner.embed(texts, role).await
        }
    }

    /// Context of a tool with a custom engine/embedder (web/subagent/rag/fetch
    /// tests), over temp storage.
    pub fn ctx_with_backends(
        profile_id: Uuid,
        engine: Arc<dyn EngineBackend>,
        embedder: Arc<dyn Embedder>,
    ) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let deps = ToolDeps {
            storage: storage.clone(),
            engine,
            embedder,
        };
        let ctx = ToolContext::new(deps, test_params(), test_turn(profile_id));
        (dir, storage, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait::async_trait]
    impl Tool for Echo {
        fn id(&self) -> ToolId {
            "echo".into()
        }
        fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
            "Возвращает аргумент text".into()
        }
        fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            })
        }
        async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
            let text = args["text"].as_str().unwrap_or_default();
            Ok(ToolOutcome::text(text))
        }
        fn group(&self) -> meta::ToolGroup {
            meta::ToolGroup::Utils
        }
        fn ui_label(&self) -> &'static str {
            "echo"
        }
    }

    #[tokio::test]
    async fn registry_invokes_registered_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Echo));
        let (_d, _s, ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        let out = reg
            .invoke("echo", &ctx, serde_json::json!({"text": "hi"}))
            .await
            .unwrap();
        assert_eq!(out.result, "hi");
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn registry_unknown_tool_errors() {
        let reg = ToolRegistry::new();
        let (_d, _s, ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        assert!(
            reg.invoke("nope", &ctx, serde_json::json!({}))
                .await
                .is_err()
        );
    }

    #[test]
    fn standard_registry_has_all_default_tools() {
        let reg = standard_registry(&ToolConfig::default());
        // The registry contains the whole catalog — both default and optional
        // control ones.
        for id in all_tool_ids() {
            assert!(reg.get(&id).is_some(), "tool {id} not registered");
        }
        // Schemas for the full catalog cover all ids.
        assert_eq!(
            reg.schemas_for(
                &all_tool_ids(),
                crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
            )
            .len(),
            all_tool_ids().len()
        );
    }

    #[test]
    fn all_tool_descriptions_localized_to_en() {
        // Strong gate (§3.5 docs/history/i18n.md): every tool's en description
        // contains no Cyrillic and differs from ru — catches a forgotten `_loc` in
        // any group.
        use crate::shared::i18n::{Lang, locale};
        let reg = standard_registry(&ToolConfig::default());
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        for id in all_tool_ids() {
            let t = reg.get(&id).expect("tool in registry");
            let d_en = t.description(en);
            assert!(
                !d_en
                    .chars()
                    .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)),
                "{id}: Cyrillic in en description: {d_en}"
            );
            assert_ne!(t.description(ru), d_en, "{id}: description not localized");
        }
    }

    #[test]
    fn note_revise_is_default_tool() {
        // Note revision is central to integration — enabled by default.
        assert!(
            default_tool_ids()
                .iter()
                .any(|t| t == notes::NOTE_REVISE_ID)
        );
    }

    #[test]
    fn control_tools_optional_not_in_defaults() {
        // Control tools — in the catalog, but not among the defaults (off by default).
        assert!(
            !default_tool_ids()
                .iter()
                .any(|t| t == control::SEND_FOLLOWUP_ID)
        );
        assert!(
            !default_tool_ids()
                .iter()
                .any(|t| t == control::REWRITE_CURRENT_ID)
        );
        assert!(
            all_tool_ids()
                .iter()
                .any(|t| t == control::SEND_FOLLOWUP_ID)
        );
        assert!(
            all_tool_ids()
                .iter()
                .any(|t| t == control::REWRITE_CURRENT_ID)
        );
    }

    #[test]
    fn self_model_tools_optional_not_in_defaults() {
        // "Self-model" tools — in the catalog, but not among the defaults.
        for id in [
            self_model::GET_SELF_MODEL_ID,
            self_model::REFLECT_ID,
            self_model::UPDATE_SELF_MODEL_ID,
            self_model::UPDATE_USER_MODEL_ID,
            self_model::ADD_INSIGHT_ID,
        ] {
            assert!(
                !default_tool_ids().iter().any(|t| t == id),
                "{id} in defaults"
            );
            assert!(
                all_tool_ids().iter().any(|t| t == id),
                "{id} not in catalog"
            );
        }
        // DB-only: pass the effective set with no global gates.
        let eff = effective_tool_ids(&all_tool_ids(), false, false, false, false, None);
        assert!(eff.iter().any(|t| t == self_model::GET_SELF_MODEL_ID));
        assert!(eff.iter().any(|t| t == self_model::UPDATE_SELF_MODEL_ID));
    }

    #[test]
    fn effective_tool_ids_gates_external_tools() {
        let enabled = default_tool_ids();
        // web on, python off, fs off → web_search/fetch_url present, no python/fs.
        let eff = effective_tool_ids(&enabled, true, false, false, false, None);
        assert!(eff.iter().any(|t| t == WEB_SEARCH_ID));
        assert!(eff.iter().any(|t| t == FETCH_URL_ID));
        assert!(!eff.iter().any(|t| t == PYTHON_EXEC_ID));
        assert!(!eff.iter().any(|t| t == fs::FS_READ_ID));
        // everything off → no external/file tools, but internal ones remain.
        let eff = effective_tool_ids(&enabled, false, false, false, false, None);
        assert!(!eff.iter().any(|t| t == WEB_SEARCH_ID || t == FETCH_URL_ID));
        assert!(
            !eff.iter()
                .any(|t| t == fs::FS_READ_ID || t == fs::FS_WRITE_ID || t == fs::FS_LIST_ID)
        );
        assert!(eff.iter().any(|t| t == "note_save"));
        // safe tools are always available.
        assert!(eff.iter().any(|t| t == "calculate"));
        assert!(eff.iter().any(|t| t == "current_time"));
        // fs on → file tools appear.
        let eff = effective_tool_ids(&enabled, false, false, true, false, None);
        assert!(eff.iter().any(|t| t == fs::FS_READ_ID));
        assert!(eff.iter().any(|t| t == fs::FS_WRITE_ID));
        assert!(eff.iter().any(|t| t == fs::FS_LIST_ID));
    }

    #[test]
    fn effective_tool_ids_keeps_sampling_tools_when_params_available() {
        let enabled = default_tool_ids();
        // Any current mode has at least one available parameter (max_tokens) —
        // sampling tools stay available (locally and in the cloud).
        for provider in [
            None,
            Some(CloudProvider::OpenAi),
            Some(CloudProvider::Gemini),
            Some(CloudProvider::Claude),
        ] {
            let eff = effective_tool_ids(&enabled, false, false, false, false, provider);
            assert!(
                eff.iter().any(|t| t == GET_SAMPLING_ID),
                "get_sampling must be available for {provider:?}"
            );
            assert!(eff.iter().any(|t| t == SET_SAMPLING_ID));
        }
    }

    #[test]
    fn effective_tool_ids_gates_mcp_tools_by_prefix() {
        // MCP tools (dynamic, outside CATALOG) are gated by the master switch by
        // the `mcp__` prefix; internal tools don't depend on it.
        let enabled: Vec<ToolId> = vec!["note_save".into(), "mcp__fs__read_text_file".into()];
        let eff = effective_tool_ids(&enabled, false, false, false, false, None);
        assert!(!eff.iter().any(|t| t.starts_with(mcp::MCP_TOOL_PREFIX)));
        assert!(eff.iter().any(|t| t == "note_save"));
        let eff = effective_tool_ids(&enabled, false, false, false, true, None);
        assert!(eff.iter().any(|t| t == "mcp__fs__read_text_file"));
    }

    #[test]
    fn schemas_for_filters_and_orders() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Echo));
        // Only enabled names make it into the schemas; unknown ones are ignored.
        let schemas = reg.schemas_for(
            &["echo".into(), "missing".into()],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
        );
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "echo");
        assert!(
            reg.schemas_for(
                &[],
                crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
            )
            .is_empty()
        );
    }
}
