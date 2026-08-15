//! UI ↔ orchestrator exchange contract: commands and events.
//! Unidirectional data flow, see spec §4.4.

use uuid::Uuid;

use crate::entities::attachment::AttachmentInfo;
use crate::entities::chat::{ChatSummary, FeedView};
use crate::entities::message::Message;
use crate::entities::message_image::ImageInfo;
use crate::entities::profile::{CharacterNames, Profile, ProfileSummary};
pub use crate::features::chat_search::FeedFocus;
use crate::features::chat_search_sort::SortMode;
pub use crate::features::file_command::FileProgress;
pub use crate::features::image_command::ImageProgress;
use crate::features::profiles::ProfileEdit;
pub use crate::features::rag_ingest::RagProgress;
pub use crate::features::tools::confirm::ToolDecision;
use crate::shared::api::FinishReason;
use crate::shared::config::AppConfig;
pub use crate::shared::server::{ServerStatus, ServerStatuses};

/// Raw pixels taken off the system clipboard (`arboard::ImageData`): RGBA8, row-major.
///
/// Deliberately un-encoded at this point. Turning a screenshot into a png costs tens of
/// milliseconds, and the clipboard is read on the **input thread** — so the encode is left
/// to the orchestrator's blocking pool, where every other image already goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Command from UI to orchestrator.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Send the user's message to the active chat and start generation.
    SendMessage(String),
    /// The user's answer to an [`AppEvent::ToolConfirmRequest`]. Routed into the
    /// running generation task; a reply whose `generation_id` is not the turn in
    /// flight is dropped (spec §9.8).
    ConfirmTool {
        generation_id: Uuid,
        call_id: String,
        decision: ToolDecision,
    },
    /// Save the input box draft in the active chat (unsaved text). UI sends this
    /// on every input change; the orchestrator writes it to the chat file with a debounce.
    /// See spec §11.7.
    SetDraft(String),
    /// Save the feed's collapse state on the active chat (`Ctrl+T` — "thoughts",
    /// `Ctrl+O` — tool calls). Like [`AppCommand::SetDraft`]: written with the
    /// save debounce, `modified_at` untouched — a view toggle must not bump the
    /// chat up the list. See spec §11.3, docs/feed-collapse.md.
    SetFeedView(FeedView),
    /// Regenerate the last assistant reply: delete everything after the last
    /// user message and restart generation from the same request.
    RegenerateLast,
    /// Delete the last exchange (the assistant's reply together with the user
    /// message that triggered it); the user's text is restored into the input box
    /// (`RestoreInput`).
    DeleteLastExchange,
    /// Cancel the current generation.
    Cancel,
    /// Write a message on the user's behalf (impersonation, `Ctrl+U`). `seed` —
    /// text already typed into the field (the model continues it; empty — writes from scratch).
    /// The result streams via `Impersonation*` events. See spec §11.8.
    Impersonate { seed: String },
    /// Cancel the current impersonation.
    CancelImpersonation,
    /// Create a new chat from a profile (by `id`; `None` — the default profile).
    NewChat { profile_id: Option<Uuid> },
    /// Make a chat active (load it into the feed).
    SwitchChat(Uuid),
    /// Make a chat active **and put the feed on one of its messages** (a jump
    /// from a search hit). Same activation as `SwitchChat`, plus a focus carried
    /// through `ChatActivated`; a message the feed doesn't show (a `Tool`/
    /// `System` one) simply lands at the tail. See docs/history/chat-search-stage2.md §3.
    OpenChatAt {
        chat: Uuid,
        message: Uuid,
        /// The query the hit came from — its matches are highlighted inside the
        /// focused message (fork S3(b)). Empty when there is nothing to
        /// highlight.
        query: String,
    },
    /// Make a chat active and put the feed on its **first message matching
    /// `query`** (`Enter` in the chat list's content mode). The orchestrator
    /// resolves the message: it owns both the index and the chat, so only it
    /// can order the matches by real chat position. A chat whose match cannot
    /// be resolved simply opens at the tail, exactly like a plain switch.
    OpenChatAtFirstMatch { chat: Uuid, query: String },
    /// Rename a chat.
    RenameChat { id: Uuid, title: String },
    /// Auto-title a chat: the model reads the conversation (or part of it) and comes up
    /// with a title. The request runs as a background task; the result is a `ChatRenamed`
    /// event.
    AutoRenameChat(Uuid),
    /// Clone a chat (a copy of the messages and settings).
    CloneChat(Uuid),
    /// Copy the whole chat conversation to the clipboard. The orchestrator builds the
    /// text (it owns `Chat`) and emits `CopyToClipboard`; writing to the clipboard is a
    /// UI-layer concern.
    CopyChat(Uuid),
    /// Write the chat's conversation to a file (`/export`). Unlike
    /// [`AppCommand::CopyChat`] the orchestrator finishes the job itself: it owns
    /// `Chat` *and* the disk, and the answer is a path, not content. Reports
    /// through `Notice`/`Error`. See docs/history/chat-export-file.md.
    ExportChat {
        id: Uuid,
        format: crate::features::export_command::ExportFormat,
        path: Option<String>,
    },
    /// Soft-delete a chat.
    DeleteChat(Uuid),
    /// Full-text search over chat **content** (the chat list's content mode,
    /// `Ctrl+F`). The argument is the **raw** user query: escaping it into a
    /// valid FTS5 query is the orchestrator's job, so that rule lives in one
    /// place (`features::chat_search::to_fts_query`; FSD — see
    /// docs/research/chat-content-search.md §4, §7a). The result is a
    /// [`AppEvent::ChatSearchResults`] event.
    SearchChats(String),
    /// Full-text search over chat content answered at **message** level (the
    /// chat list's `Ctrl+G`) — the query is raw, escaped by the orchestrator
    /// exactly like [`AppCommand::SearchChats`]. The result is a
    /// [`AppEvent::MessageSearchResults`] event. See docs/history/chat-search-stage2.md.
    SearchMessages { query: String, sort: SortMode },
    /// Create a new profile (the UI section is M8; the command is needed for
    /// operations/tests).
    CreateProfile {
        name: String,
        system_message: String,
    },
    /// Soft-delete a profile with a cascade onto its chats (notes/RAG are excluded).
    DeleteProfile(Uuid),
    /// Replace the whole config (settings screen). The orchestrator saves it,
    /// restarts the server/registry if needed, and re-emits it. See spec §11.6.
    /// `Box` — `AppConfig` is large, don't bloat the enum.
    UpdateConfig(Box<AppConfig>),
    /// Apply profile edits (settings screen). `Box` — `ProfileEdit` carries
    /// large fields (the system message).
    UpdateProfile { id: Uuid, edit: Box<ProfileEdit> },
    /// Index a file or directory into the active profile's knowledge base (RAG)
    /// (the `/rag add <path> [-r]` command). Runs as a background task; progress
    /// arrives via `RagProgress` events. See spec §9.3.
    RagAdd { path: String, recursive: bool },
    /// Remove a file or directory (and everything under it) from the active
    /// profile's knowledge base (the `/rag remove <path>` command). The result is a
    /// `RagProgress` event.
    RagDelete { path: String },
    /// Show the active profile's knowledge-base sources (the `/rag list` command).
    /// The result is a `RagProgress::Listed` event.
    RagList,
    /// Reindex the active profile's knowledge base (the `/rag rebuild` command):
    /// re-chunk and re-embed the stored sources. Runs as a background task;
    /// progress — via `RagProgress` events. See spec §9.3.
    RagRebuild,
    /// Re-embed every stored vector with the current embedding model
    /// (`/reindex`). Unlike `RagRebuild` this is **DB-global** and re-embeds
    /// **in place**: no re-chunking, no source text needed, so it also repairs
    /// legacy rows whose file is gone and attachment indexes that would
    /// otherwise only come back on re-attach. See
    /// docs/research/embedding-model-change-reindex.md §8.1.
    Reindex,
    /// Compress the older part of the active chat into a rolling summary
    /// (`/compact`, spec §6.7). Refused when the master switch is off.
    Compact,
    /// Attach a file to the active chat (the `/file attach <path>` command).
    /// Reading/extracting the text runs as a background task; the result arrives
    /// as a `FileProgress` event. See docs/file-attachments.md, spec §9.7.
    FileAttach { path: String },
    /// Remove an attachment from the active chat by display name, path, or `#N`
    /// (the `/file remove <target>` command).
    FileRemove { target: String },
    /// Show the active chat's attachments (the `/file list` command). The result
    /// is a `FileProgress::Listed` event.
    FileList,
    /// Stage an image for the next message (the `/image attach <path>` command).
    /// Reading, decoding and downscaling run as a background task; the result arrives as
    /// an `ImageProgress` event. See spec §9.10.
    ImageAttach { path: String },
    /// Unstage an image by display name, path, or `#N` (`/image remove <target>`).
    ImageRemove { target: String },
    /// Show what is staged for the next message (`/image list`). The result is an
    /// `ImageProgress::Listed` event.
    ImageList,
    /// Stage an image read off the system clipboard (`Ctrl+V`, `/image paste`).
    ///
    /// Carries the pixels rather than a request to read them: the `arboard` client lives
    /// in `runtime` (a UI-layer side effect, like `CopyToClipboard`), so by the time the
    /// orchestrator is involved the clipboard has already been consulted. `Box` — an
    /// uncompressed screenshot is megabytes, and every other variant would pay for it.
    ImagePaste(Box<ClipboardImage>),
    /// Speak the active chat's messages (the `/tts [N|all]` command). The orchestrator
    /// (the owner of `Chat`) takes a **snapshot** of the conversation at command time and
    /// starts a background synthesis/playback task. A new command interrupts the current
    /// playback. See spec §11.9, docs/research/tts.md §7.
    Tts(crate::features::tts_command::TtsScope),
    /// Stop speech playback and clear the queue (the `/tts stop` command).
    TtsStop,
    /// Pause playback, keeping the queue (the `/tts pause` command).
    TtsPause,
    /// Resume paused playback (the `/tts resume` command).
    TtsResume,
    /// Request a snapshot of the active profile's "self-model" (for the viewer screen,
    /// `F3`). The orchestrator (the owner of `Storage`) responds with a `SelfModelView`
    /// event.
    RequestSelfModel,
    /// Apply a manual edit to the active profile's "self-model" (the `F3` UI editor).
    /// The orchestrator applies it, saves, and re-emits the updated `SelfModelView`.
    UpdateSelfModel(crate::entities::self_model::SelfModelEdit),
    /// Confirm a changed tool catalog for an MCP server (TOFU
    /// reconfirmation from settings): register the held-back tools
    /// and persist the new pin. The argument is the server id. See spec §9.6.
    ConfirmMcpCatalog(String),
    /// Store a secret entered in settings — a cloud provider's API key, the
    /// backup password, or an MCP server's environment value. The orchestrator
    /// encrypts it with the machine key and puts it into `config.api_keys`:
    /// plaintext lives only along this path and in the consumer (the HTTP client,
    /// the archive, the child process), never on disk. An empty `value` deletes
    /// it. Separate from `UpdateConfig` precisely so the secret never travels in
    /// a config snapshot. See `shared::secrets`, docs/research/api-key-storage.md.
    SetSecret {
        key: crate::shared::secrets::SecretKey,
        value: String,
    },
    /// Restart one MCP server (Enter on its row in settings when there is no
    /// catalog to confirm). An action, not a config edit: an identical config is
    /// not re-applied (`McpManager::is_current`), so a server that exhausted its
    /// restart budget has no other way back. See spec §9.6.
    ReconnectMcpServer(String),
    /// Import MCP servers from an ecosystem `mcpServers` JSON file (the argument
    /// is the path). Parsed by the orchestrator, not the screen: it is the sole
    /// writer of `settings.json` and the only layer allowed to touch the literal
    /// secrets such a file carries. See docs/history/mcp-server-editor.md §9.
    ImportMcpServers(String),
    /// Shut down (the orchestrator stops).
    Quit,
}

impl AppCommand {
    /// Does this command **work on the open conversation** — change what it
    /// stores, or start a turn in it?
    ///
    /// The one consumer is the `Esc` back-stack (`app::runtime::Back`): a way
    /// back exists for someone who is *looking* at the chat they drilled into,
    /// and stops making sense the moment they start working in it. See the
    /// clearing funnel in `app::runtime::dispatch`.
    ///
    /// Deliberately an **exhaustive match** rather than a list of the few
    /// interesting variants: a new command then cannot join the enum without
    /// someone deciding which side it falls on — the compiler asks. The rule is
    /// narrow on purpose. Reading (`FileList`, `CopyChat`, `Tts*`), looking
    /// (`SetFeedView`), typing without sending (`SetDraft`) and staging for the
    /// *next* message (`Image*` — turn-scoped, never stored) all leave the way
    /// back alone; a chat switch is not here at all, because activation already
    /// has its own funnel.
    pub fn works_on_the_open_chat(&self) -> bool {
        match self {
            // Starts a turn in this conversation.
            AppCommand::SendMessage(_)
            | AppCommand::RegenerateLast
            | AppCommand::Impersonate { .. }
            // Changes what the conversation stores.
            | AppCommand::DeleteLastExchange
            | AppCommand::Compact
            | AppCommand::FileAttach { .. }
            | AppCommand::FileRemove { .. } => true,

            // Reading the conversation out to a file changes nothing in it —
            // the same side as `CopyChat`.
            AppCommand::ExportChat { .. }
            | AppCommand::ConfirmTool { .. }
            | AppCommand::SetDraft(_)
            | AppCommand::SetFeedView(_)
            | AppCommand::Cancel
            | AppCommand::CancelImpersonation
            | AppCommand::NewChat { .. }
            | AppCommand::SwitchChat(_)
            | AppCommand::OpenChatAt { .. }
            | AppCommand::OpenChatAtFirstMatch { .. }
            | AppCommand::RenameChat { .. }
            | AppCommand::AutoRenameChat(_)
            | AppCommand::CloneChat(_)
            | AppCommand::CopyChat(_)
            | AppCommand::DeleteChat(_)
            | AppCommand::SearchChats(_)
            | AppCommand::SearchMessages { .. }
            | AppCommand::CreateProfile { .. }
            | AppCommand::DeleteProfile(_)
            | AppCommand::UpdateConfig(_)
            | AppCommand::UpdateProfile { .. }
            | AppCommand::RagAdd { .. }
            | AppCommand::RagDelete { .. }
            | AppCommand::RagList
            | AppCommand::RagRebuild
            | AppCommand::Reindex
            | AppCommand::FileList
            | AppCommand::ImageAttach { .. }
            | AppCommand::ImageRemove { .. }
            | AppCommand::ImageList
            | AppCommand::ImagePaste(_)
            | AppCommand::Tts(_)
            | AppCommand::TtsStop
            | AppCommand::TtsPause
            | AppCommand::TtsResume
            | AppCommand::RequestSelfModel
            | AppCommand::UpdateSelfModel(_)
            | AppCommand::ConfirmMcpCatalog(_)
            | AppCommand::SetSecret { .. }
            | AppCommand::ReconnectMcpServer(_)
            | AppCommand::ImportMcpServers(_)
            | AppCommand::Quit => false,
        }
    }
}

/// Event from the orchestrator to UI. This is the only way UI updates its read-only
/// projection.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// The status of one of the servers changed — a snapshot of all of them (chat/
    /// embeddings/impersonation).
    ServerStatus(ServerStatuses),
    /// The full list of visible chats (for the overlay). Sent whenever the set changes.
    ChatList(Vec<ChatSummary>),
    /// A chat's title changed (manual/auto rename). UI updates the active chat's
    /// feed header without a rebuild (`ChatList` updates the list).
    ChatRenamed { id: Uuid, title: String },
    /// The result of a content search (a reply to [`AppCommand::SearchChats`]):
    /// the chats having at least one matching message. `query` is echoed back so
    /// a late reply can be told from the current one.
    ///
    /// `chat_ids: None` means **"not a searchable query — do not filter"**
    /// (nothing survived trigram's 3-character floor, or the query failed).
    /// Deliberately an `Option` rather than "all the ids": the event must never
    /// claim that every chat matched.
    ChatSearchResults {
        query: String,
        chat_ids: Option<Vec<Uuid>>,
    },
    /// The result of a message-level content search (a reply to
    /// [`AppCommand::SearchMessages`]): matching messages **grouped by chat**
    /// (fork S2), chats in the chat list's order, messages in chat order.
    /// `query` is echoed back so a late reply can be told from the current one.
    ///
    /// `total` is the true number of matching messages, which may exceed the
    /// hits carried here ([`crate::features::chat_search::HIT_CAP`]) — the screen shows
    /// "showing N of M" rather than truncating silently.
    MessageSearchResults {
        query: String,
        groups: Vec<crate::features::chat_search::SearchGroup>,
        total: usize,
    },
    /// An error from a chat-list operation (auto-title/delete/clone). Shown in
    /// the list overlay's dedicated status area (not the chat feed), if it's open.
    ChatListError(String),
    /// Write text to the clipboard (a UI-layer side effect). The orchestrator built
    /// the chat conversation text; runtime writes it to the clipboard and sends a
    /// confirmation/error to the overlay.
    CopyToClipboard(String),
    /// The full list of visible profiles (for the selection overlay when creating a
    /// chat).
    ProfileList(Vec<ProfileSummary>),
    /// A full settings snapshot for the settings screen (config + full profiles).
    /// Sent on startup and after any config/profile edit. See spec §11.6.
    /// `language_locked` — ids of profiles whose scaffold language (axis A,
    /// docs/history/i18n.md) can no longer be changed: data has appeared (chats /
    /// "self-model" / notes). Computed by the orchestrator (the screen has no access to
    /// chats/the DB).
    /// `mcp` — a snapshot of the MCP host (a dynamic tool catalog for profile
    /// toggles + server statuses for rows in the "Tools" section); MCP tools are not
    /// part of the static `tool_catalog()`. Empty while the servers haven't
    /// come up / are disabled.
    /// `secrets_present` — which secrets are stored **on this machine** (the
    /// "configured" status shown by every secret field: provider keys, the backup
    /// password, MCP environment values). The secrets themselves aren't included
    /// in the snapshot: `config.api_keys` is cleared on emit — UI doesn't carry
    /// them even as ciphertext, and the orchestrator restores them on the way back
    /// (`handle_update_config`). See `shared::secrets`,
    /// docs/research/api-key-storage.md.
    Settings {
        config: Box<AppConfig>,
        profiles: Vec<Profile>,
        language_locked: Vec<uuid::Uuid>,
        mcp: crate::features::tools::mcp::McpSnapshot,
        secrets_present: Vec<crate::shared::secrets::SecretKey>,
    },
    /// The outcome of an MCP import (`AppCommand::ImportMcpServers`) — a
    /// localized one-line summary, or the reason it failed. Shown as the import
    /// row's value in settings, where the user is standing.
    McpImportResult(String),
    /// The role names to show in the feed — the active chat's profile
    /// `character_names` (spec §5.1). Sent on chat activation and whenever the
    /// profile is edited, so a name change applies to the open chat immediately.
    /// An empty field means "not set": the feed keeps its localized header.
    CharacterNames(CharacterNames),
    /// The active chat changed — UI rebuilds the feed from its messages and loads
    /// the saved draft into the input box (`draft`; empty for a new chat).
    ChatActivated {
        id: Uuid,
        title: String,
        messages: Vec<Message>,
        draft: String,
        /// The chat's stored collapse state for the feed's foldable blocks
        /// ("thoughts"/tool calls) — the counterpart of `draft` for the view
        /// (spec §11.3).
        feed_view: FeedView,
        /// Put the feed on this message instead of the tail, and highlight the
        /// query inside it (a jump from a search hit, `AppCommand::OpenChatAt`).
        /// `None` — every other activation, which must not highlight anything.
        /// See docs/history/chat-search-stage2.md §3 and §4a S3(b).
        focus: Option<FeedFocus>,
        /// The rolling summary and the id of the first message still sent
        /// verbatim, when this chat has been compacted **and** the feature is on
        /// — what the feed needs to draw the boundary (spec §6.7). `None` when
        /// there is no summary or the master switch is off, so turning the
        /// switch off also removes the divider.
        compaction: Option<(Uuid, String)>,
    },
    /// The user's message was accepted (an echo for the feed).
    UserMessage(String),
    /// Restore text into the input box (after deleting the last exchange). A non-empty
    /// existing input isn't overwritten — the text is prepended to it (UI).
    RestoreInput(String),
    /// The assistant's reply generation has started.
    GenerationStarted { generation_id: Uuid },
    /// A delta of the reply's main text.
    Chunk { generation_id: Uuid, text: String },
    /// A delta of "thoughts" (CoT).
    Thoughts { generation_id: Uuid, text: String },
    /// The current generation's live token counter. `completion` — reply tokens
    /// generated so far (accumulated across agentic-loop rounds); `context` — tokens
    /// in the prompt (the whole conversation); `None` leaves the previous value
    /// untouched. `context_exact` — whether this number is exact, from the server's
    /// `usage` (otherwise a client-side estimate, UI marks it with `~`). `reasoning` —
    /// "thoughts" tokens (included in `completion`); `None` leaves the previous value
    /// untouched (known only from `usage`, not from the stream).
    /// UI shows "conversation + reply" in the status bar, see spec §11.1.
    TokenUsage {
        generation_id: Uuid,
        completion: u64,
        context: Option<u64>,
        context_exact: bool,
        reasoning: Option<u32>,
    },
    /// The agentic loop is holding a **dangerous** tool call and asking the user
    /// whether to run it (`tools.confirm_dangerous`, spec §9.8). The turn is
    /// parked until an [`AppCommand::ConfirmTool`] carrying the same
    /// `generation_id` comes back, or until the turn is cancelled.
    ToolConfirmRequest {
        generation_id: Uuid,
        /// The call's id — echoed back so a reply cannot answer the wrong call
        /// when the model made several in one round.
        call_id: String,
        name: String,
        arguments: String,
    },
    /// A tool was called and executed (for the tool block in the feed). See spec §6.3,
    /// §11.3.
    ToolCall {
        generation_id: Uuid,
        name: String,
        arguments: String,
        result: String,
        /// How many images the call returned (spec §9.10). A count, not the pixels: the
        /// feed shows a chip, and megabytes of base64 have no business travelling to a
        /// widget that renders one line.
        images: usize,
    },
    /// The assistant decided to write **another** message (the tool
    /// `send_followup_message`): UI finishes the current bubble and starts a new one,
    /// which will receive the next round's text. See spec §9.3.
    AssistantContinue { generation_id: Uuid },
    /// The assistant decided to **rewrite** the current message (the tool
    /// `rewrite_current_message`): UI discards the already-accumulated text of the
    /// current bubble; the rewritten reply will go into it. See spec §9.3.
    AssistantRewrite { generation_id: Uuid },
    /// Generation finished.
    Finished {
        generation_id: Uuid,
        reason: FinishReason,
    },
    /// Impersonation started: UI hides the input box and shows a streaming preview
    /// of the reply (pre-filled with the already-typed text). See spec §11.8.
    ImpersonationStarted { generation_id: Uuid },
    /// A text delta of the impersonated reply (into the preview).
    ImpersonationChunk { generation_id: Uuid, text: String },
    /// Impersonation finished. On `Stop`/`Length` UI inserts the accumulated text into
    /// the input box; on `Cancelled`/`Error` — discards it (the input box is unchanged).
    ImpersonationFinished {
        generation_id: Uuid,
        reason: FinishReason,
    },
    /// Progress of background file indexing into RAG (the `/rag add` command). See
    /// spec §9.3.
    RagProgress(RagProgress),
    /// Outcome of a `/file` command (attached/removed/list/error) — a note in the
    /// feed. See docs/file-attachments.md.
    FileProgress(FileProgress),
    /// The active chat's attachment cards — for the status-bar chip (attachments
    /// cost tokens on every turn, so their presence has to be visible). Sent on
    /// chat activation and after every attach/remove, like `CharacterNames`.
    /// Cards only: the text itself never travels through the event channel.
    Attachments(Vec<AttachmentInfo>),
    /// Outcome of an `/image` command (staged/unstaged/list/error) — a note in the feed.
    /// See spec §9.10.
    ImageProgress(ImageProgress),
    /// The images staged for the next message — for the status-bar chip. Sent on chat
    /// activation, after every attach/remove, and when a turn consumes the staged set
    /// (then empty). Cards only: the payload never travels through the event channel.
    StagedImages(Vec<ImageInfo>),
    /// A snapshot of the active profile's "self-model" (a reply to `RequestSelfModel`)
    /// for the viewer screen (`F3`). `None` — the model hasn't been created yet. `Box` —
    /// a large type, don't bloat the enum. See docs/history/self-model-mvp.md.
    SelfModelView(Box<Option<crate::entities::self_model::SelfModel>>),
    /// The "self-model" changed (via background reflection or turn tools) — a signal
    /// **with no snapshot**. If the `F3` screen is open, UI re-requests a fresh snapshot
    /// (`RequestSelfModel`); ignored when the screen is closed. See stage 5 of the
    /// refinements.
    SelfModelChanged,
    /// Whether speech (synthesis/playback) is active — for the quiet "♪ speaking" chip
    /// in the status bar. `true` when the task starts, `false` on its completion/
    /// cancellation. See spec §11.9.
    TtsActive(bool),
    /// Background task activity (auto-reflection/consolidation) for the quiet indicator
    /// in the status bar: `active=true` at the start, `false` on completion. See stage 5.
    BackgroundTask { kind: BackgroundKind, active: bool },
    /// A transient provider failure is being retried; the next attempt starts in
    /// `delay_secs`.
    ///
    /// Shown as a quiet, transient status-bar chip rather than a feed note: a note
    /// per attempt would be noise, while saying nothing for up to half a minute is
    /// the door left open (docs/lessons.md §4). Cleared by the next chunk or by
    /// `Finished`. Carries `generation_id` so a chip from a turn the user has since
    /// cancelled cannot linger (spec §4.4).
    Retrying {
        generation_id: Uuid,
        attempt: u32,
        max: u32,
        delay_secs: u64,
    },
    /// An error (for showing in UI).
    Error(String),
    /// A plain informational note in the feed — the counterpart of [`AppEvent::Error`]
    /// for an outcome that is not a failure ("nothing to compress yet"). Rendered
    /// with `push_note`, not `push_error`.
    Notice(String),
    /// A compaction finished: the chat's older messages are now represented in the
    /// request by a rolling summary (spec §6.7). Carries what the feed needs to
    /// draw the boundary — `boundary` is the id of the first message still sent
    /// verbatim — plus `folded`, how many messages the summary now covers, for the
    /// confirmation note. `chat.messages` is unchanged, so the feed keeps showing
    /// everything; only a divider appears.
    Compacted {
        chat_id: Uuid,
        boundary: Uuid,
        summary: String,
        folded: usize,
    },
}

/// The kind of background task for the status-bar indicator
/// (`AppEvent::BackgroundTask`). `Hash` — used as the key of the background-task slot
/// registry (`orchestrator::background`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackgroundKind {
    /// Auto-reflection of the "self-model".
    Reflection,
    /// Auto-consolidation of notes ("sleep").
    Consolidation,
    /// Auto-consolidation of the "self-model" (self-model "sleep"): merge duplicate
    /// observations, compress a bloated description, link contradictions. See
    /// docs/history/self-model-consolidation.md.
    SelfConsolidation,
    /// Compressing the older part of a conversation into a rolling summary
    /// (`/compact`, spec §6.7).
    Compaction,
}
