//! The chat screen (FSD "page"): widget layout, focus routing, hotkeys, and a
//! read-only projection of state. See spec §11.1, §11.7.
//!
//! The screen does NOT know about `app`/channels (FSD: dependencies only flow
//! down). On a keypress it returns a [`ChatIntent`] — an intent that `app`
//! translates into an `AppCommand`. `app` applies orchestrator events by
//! calling the screen's mutators (`set_*`, `push_*`, …) — the screen doesn't
//! need the `AppEvent` type.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};
use uuid::Uuid;

use crate::app::events::{BackgroundKind, ChildView};
use crate::entities::attachment::AttachmentInfo;
use crate::entities::chat::{ChatSummary, FeedView};
use crate::entities::message::Message;
use crate::entities::message_image::ImageInfo;
use crate::entities::profile::{CharacterNames, Profile, ProfileSummary};
use crate::features::rag_ingest::RagProgress;
use crate::features::spellcheck::SpellChecker;
use crate::features::tools::confirm::ToolDecision;
use crate::shared::api::FinishReason;
use crate::shared::config::AppConfig;
use crate::shared::i18n::{Locale, locale};
use crate::shared::keys;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::theme::Palette;
use crate::shared::ui::{ListScroll, dim_background};
use crate::widgets::chat_link_picker::{ChatLinkAction, ChatLinkPickerState};
use crate::widgets::emoji_picker::{EmojiPickerAction, EmojiPickerState};
use crate::widgets::help_dialog::{HelpContext, HelpSection};
use crate::widgets::impersonation_preview;
use crate::widgets::input_box::InputBox;
use crate::widgets::message_feed::{FeedMessage, FeedRole, MessageFeed};
use crate::widgets::profile_list::{ProfileListAction, ProfileListState};
use crate::widgets::status_bar::{self, EscTarget};

/// The chat's "Shortcuts" section (`F1`): one row per key this screen's
/// handlers match (`input.rs` — the chords, the plain keys, the input box) —
/// a new arm gets a row here, next door (AGENTS.md §3). The app layer
/// composes the dialog's tab from the screens' sections
/// (docs/history/help-hotkeys-context.md §6).
pub(crate) static HELP_SECTION: HelpSection = HelpSection {
    title: "ui.help.sec.chat",
    context: Some(HelpContext::Chat),
    // The groups: composing · selection/clipboard · the conversation ·
    // editing · panels and toggles.
    rows: &[
        ("Enter", "ui.help.send"),
        ("Shift+Enter / Alt+Enter", "ui.help.newline"),
        ("Shift+←/→/↑/↓", "ui.help.select"),
        ("Ctrl+A", "ui.help.select_all"),
        ("Ctrl+C", "ui.help.copy"),
        ("Ctrl+X", "ui.help.cut"),
        ("Ctrl+V", "ui.help.paste"),
        ("Esc", "ui.help.esc"),
        ("Ctrl+N", "ui.help.new_chat"),
        ("F2", "ui.help.rename_chat"),
        ("F5", "ui.help.copy_chat"),
        ("F6", "ui.help.stop_run"),
        ("Ctrl+R", "ui.help.regenerate"),
        ("Ctrl+E", "ui.help.delete_exchange"),
        ("Ctrl+U", "ui.help.impersonate"),
        ("Ctrl+K", "ui.help.clear_input"),
        ("Ctrl+Z / Ctrl+Y", "ui.help.undo_redo"),
        ("Ctrl+←/→", "ui.help.word_move"),
        ("Ctrl+Backspace/Delete", "ui.help.word_delete"),
        ("Home", "ui.help.line_home"),
        ("End", "ui.help.line_end"),
        ("Ctrl+Home/End", "ui.help.doc_move"),
        ("Ctrl+G", "ui.help.spell"),
        ("Ctrl+P", "ui.help.settings"),
        ("F3", "ui.help.self_model"),
        ("F4", "ui.help.changes"),
        ("F7", "ui.help.tasks"),
        ("Ctrl+F", "ui.help.find_in_chat"),
        ("Ctrl+T", "ui.help.thoughts"),
        ("Ctrl+O", "ui.help.tool_calls"),
        ("Ctrl+B", "ui.help.emoji"),
        ("Ctrl+L", "ui.help.chat_links"),
        ("Ctrl+W", "ui.help.mouse_toggle"),
        ("ui.help.k.mouse", "ui.help.mouse_action"),
        ("PageUp/PageDown", "ui.help.scroll"),
    ],
    openers: &["Shift+←/→/↑/↓", "Esc", "Ctrl+K", "Ctrl+P"],
};

/// Feed scroll height per PageUp/PageDown press (rows).
const PAGE_SCROLL: usize = 8;

/// Feed scroll height per mouse-wheel "notch" (rows).
const WHEEL_SCROLL: usize = 3;

/// Spellcheck debounce delay: a word isn't flagged while the user is typing.
const SPELL_DEBOUNCE: Duration = Duration::from_millis(300);

/// A settings snapshot from the `Settings` event: config + full profiles + ids of
/// profiles with a locked scaffold language + the MCP host snapshot (catalog +
/// server statuses) + which secrets are stored on this machine (presence only —
/// never the secrets themselves).
pub type SettingsSnapshot = (
    AppConfig,
    Vec<Profile>,
    Vec<uuid::Uuid>,
    crate::features::tools::mcp::McpSnapshot,
    Vec<crate::shared::secrets::SecretKey>,
);

/// A user intent that `app` executes (translates into an `AppCommand`).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatIntent {
    Quit,
    Send(String),
    /// Regenerate the assistant's last reply (`Ctrl+R`).
    RegenerateLast,
    /// Resume the last interrupted reply in place (`/continue`, spec §6.4).
    ContinueLast,
    /// Delete the last exchange; the user's text returns to the input box (`Ctrl+E`).
    DeleteLastExchange,
    Cancel,
    /// Write a message on the user's behalf (impersonation, `Ctrl+U`). `seed` —
    /// text already typed (the model continues it). See spec §11.8.
    Impersonate {
        seed: String,
    },
    /// Cancel the current impersonation (`Esc` in the preview).
    CancelImpersonation,
    /// Create a chat from a profile (`None` — the default profile). `Ctrl+N` in
    /// the chat (operations on specific chats — switch/clone/delete/rename —
    /// go through the chat-list screen,
    /// [`ChatListIntent`](crate::screens::chat_list::ChatListIntent)).
    NewChat {
        profile_id: Option<Uuid>,
    },
    /// Copy the active chat's whole conversation to the clipboard (`F5`).
    CopyChat(Uuid),
    /// Rename the open chat (command `/rename <title>`; the chat list's own
    /// `F2` renames the *selected* chat through
    /// [`ChatListIntent`](crate::screens::chat_list::ChatListIntent)).
    RenameChat {
        id: Uuid,
        title: String,
    },
    /// Ask the model to title the open chat (command `/autotitle`; `Ctrl+R` in
    /// the chat list — browser-taken, which is what earned it a typed route).
    /// The result arrives as `ChatRenamed`; failures fall back to a feed note
    /// (`ChatListError` routing). See spec §11.2, docs/history/commands-stage3.md.
    AutoTitleChat(Uuid),
    /// Clone the open chat (command `/clone`; `Ctrl+D` in the chat list).
    CloneChat(Uuid),
    /// Fold or unfold the open chat's sub-agent transcripts in the chat list
    /// (command `/subagents`; `Ctrl+O` in the list toggles the *selected*
    /// chat's through [`ChatListIntent`](crate::screens::chat_list::ChatListIntent)).
    /// Stored on the chat; see spec §11.2.
    SetChildrenExpanded {
        id: Uuid,
        expanded: bool,
    },
    /// Stop a background sub-agent run (command `/subagents stop [n]`,
    /// spec §9.3.2): `id` is the run's own id, resolved here from the list's
    /// cards — the open transcript's, or the n-th running one under the
    /// open chat.
    StopSubagentRun {
        id: Uuid,
    },
    /// Stop one of the app's own silent tasks (command `/tasks stop <kind>`,
    /// spec §11.10) — the typed twin of `F6` on the task's row of the tasks
    /// screen, ending in the very command that key sends
    /// (docs/research/tasks-stop-command.md).
    StopBackgroundTask {
        kind: BackgroundKind,
    },
    /// Write the open chat to a file (command `/export [md|json] [path]`).
    /// The orchestrator owns the conversation and the disk, so it formats and
    /// writes; a relative path (or a generated name) resolves against the
    /// **current working directory**. See docs/history/chat-export-file.md.
    ExportChat {
        id: Uuid,
        format: crate::features::export_command::ExportFormat,
        path: Option<String>,
    },
    /// Search every chat's messages and open the results screen (command
    /// `/search <text>`; `Ctrl+G` in the chat list's content mode). The screen
    /// opens on the reply `AppEvent::MessageSearchResults` — the orchestrator
    /// owns the index — and the sort order is the chat list's default, since the
    /// chat screen has no list to inherit one from.
    SearchMessages {
        query: String,
    },
    /// Index a file/directory into RAG (command `/rag add <path> [-r]`).
    RagAdd {
        path: String,
        recursive: bool,
    },
    /// Remove a file/directory from RAG (command `/rag remove <path>`).
    RagDelete {
        path: String,
    },
    /// Show the knowledge base's sources (command `/rag list`).
    RagList,
    /// Reindex the knowledge base (command `/rag rebuild`).
    RagRebuild,
    /// Re-embed every stored vector with the current embedding model (command
    /// `/reindex`). Top-level, not a `/rag` subcommand: it spans notes, chat
    /// attachments and every profile's knowledge base. See
    /// docs/research/embedding-model-change-reindex.md §8.1.
    Reindex,
    /// Fold the earlier part of this conversation into a rolling summary
    /// (command `/compact`). Top-level rather than a subcommand: it is the
    /// whole operation, and there is nothing else to name. Messages are never
    /// deleted — only what a request carries changes. See spec §6.7,
    /// docs/research/history-compression.md §6.5.
    Compact,
    /// Attach a file to the chat (command `/file attach <path>`). See
    /// docs/file-attachments.md.
    FileAttach {
        path: String,
    },
    /// Remove an attachment (command `/file remove <name|#N>`).
    FileRemove {
        target: String,
    },
    /// Show the chat's attachments (command `/file list`).
    FileList,
    /// Attach a code project to the chat (command `/project attach <dir>`).
    /// See docs/history/code-workspace.md, spec §9.12.
    ProjectAttach {
        path: String,
    },
    /// Detach the chat's code project (command `/project detach`).
    ProjectDetach,
    /// Report the chat's code project (command `/project status`).
    ProjectStatus,
    /// Open the changes screen (`F4`, `/changes`): what the assistant changed
    /// in the attached project. See spec §9.12.
    OpenChanges,
    /// Open the tasks screen (`F7`, `/tasks`): everything the app is doing in
    /// the background. See spec §11.10.
    OpenTasks,
    /// Open the help dialog (`/help`). `F1` never reaches the screen — the
    /// runtime routes it above every screen and owns the overlay (spec §11.7,
    /// docs/history/help-hotkeys-context.md stage 2).
    OpenHelp,
    /// Set, show or clear one of the project's command slots
    /// (`/project build-cmd|run-cmd|test-cmd [line]`, `/project clear <slot>`).
    ProjectSlot {
        slot: crate::entities::workspace::CommandSlot,
        action: crate::features::project_command::SlotAction,
    },
    /// Stage an image for the next message (command `/image attach <path>`). See
    /// spec §9.10.
    ImageAttach {
        path: String,
    },
    /// Unstage an image (command `/image remove <name|#N>`).
    ImageRemove {
        target: String,
    },
    /// Show the images staged for the next message (command `/image list`).
    ImageList,
    /// Stage the image on the system clipboard (`Ctrl+V`, command `/image paste`).
    ///
    /// The clipboard is read by `runtime`, not here and not by the orchestrator — it is a
    /// UI-layer side effect, and only `runtime` holds the `arboard` client (the same
    /// arrangement [`ChatIntent::CopyToClipboard`] uses).
    ///
    /// `text_fallback` says what to do when the clipboard holds no image: `Ctrl+V` must
    /// still paste text, because that is what the help overlay has always promised the key
    /// does; a typed `/image paste` must not, and says instead that there is no image.
    PasteImage {
        text_fallback: bool,
    },
    /// Speak the chat's messages (command `/tts`, `/tts N`, `/tts all`). See spec §11.9.
    Tts(crate::features::tts_command::TtsScope),
    /// Stop speech (command `/tts stop`).
    TtsStop,
    /// Pause speech (command `/tts pause`).
    TtsPause,
    /// Resume speech (command `/tts resume`).
    TtsResume,
    /// Open the settings screen (`Ctrl+P`). `app` builds it from the settings snapshot.
    OpenSettings,
    /// Open the chat-list screen (`Esc`). `app` builds it from the list snapshot.
    OpenChatList,
    /// Follow a `chat://` reference drawn in the feed (`Ctrl+L`, then `Enter`
    /// on the picked conversation). A plain activation — an address names a
    /// conversation, not a message — so `app` answers it with
    /// `AppCommand::SwitchChat`. See spec §11.3.
    OpenChatLink(Uuid),
    /// Open the "self-model" viewer screen (`F3`). `app` requests a snapshot from
    /// the orchestrator (`RequestSelfModel`) and builds the screen from the
    /// `SelfModelView` event.
    OpenSelfModel,
    /// Wipe the active profile's self-model (command `/self clear`, confirmed) —
    /// the same edit the self-model screen sends on `Ctrl+K` twice. See spec §17.7.
    ClearSelfModel,
    /// Create a companion profile (command `/profile new [name]`; `Ctrl+N` in the
    /// settings screen's "Profiles" section). The persona is empty — it is
    /// written in the settings screen afterwards. See spec §11.6.
    CreateProfile {
        name: String,
    },
    /// Delete a companion profile, hiding its chats with it (command
    /// `/profile delete <name>`, confirmed; `Ctrl+D` in the settings screen).
    /// The orchestrator refuses to delete the last one.
    DeleteProfile(Uuid),
    /// Apply a profile edit (commands `/profile system|greeting`,
    /// `/impersonation use`) — the same `AppCommand::UpdateProfile` the
    /// settings editors commit through, so validation and persistence are
    /// shared. `Box` — `ProfileEdit` carries large fields.
    UpdateProfile {
        id: Uuid,
        edit: Box<crate::features::profiles::ProfileEdit>,
    },
    /// Replace the whole config (commands `/impersonation new|delete|system` —
    /// the personas live in `AppConfig.impersonation_profiles` and are plain
    /// config edits, exactly as the settings screen commits them). Built from
    /// the screen's settings snapshot, which every config change re-emits.
    /// `Box` — `AppConfig` is large.
    UpdateConfig(Box<AppConfig>),
    /// Toggle terminal mouse capture for wheel scrolling (`Ctrl+W`). `true` —
    /// the wheel scrolls the feed (text selection — with Shift); `false` —
    /// native mouse selection. See spec §11.3.
    SetMouseCapture(bool),
    /// The feed's collapse state changed (`Ctrl+T` — "thoughts", `Ctrl+O` —
    /// tool calls). Stored **per chat**, like the draft: the orchestrator writes
    /// it to the active chat and hands it back on activation. See spec §11.3,
    /// docs/feed-collapse.md.
    SetFeedView(FeedView),
    /// Write text to the system clipboard (`Ctrl+C` copy / `Ctrl+X` cut the
    /// input box's selection). A UI-layer side effect — `runtime` writes via
    /// `arboard` (doesn't go through the orchestrator: the text is already at
    /// the UI). See docs/history/input-selection-undo-mouse.md §B.
    CopyToClipboard(String),
    /// The user's answer to a dangerous-tool confirmation (spec §9.8). Carries
    /// the ids back so a reply cannot land on the wrong turn or the wrong call.
    ConfirmTool {
        generation_id: Uuid,
        call_id: String,
        decision: ToolDecision,
    },
}

/// An irreversible operation that requires confirmation in a modal popup.
///
/// Two of these are the destructive **keys** (`Ctrl+R`/`Ctrl+E`), raised through
/// [`ChatScreen::trigger_destructive`] and asked about only when
/// `interface.confirm_destructive_keys` is on — and never while a turn runs.
/// The other two are typed routes into another screen's territory (stage 2 of
/// docs/history/command-only-control.md); their commands set this state
/// **directly**, because they differ from the keys in both respects: they always
/// ask (a command names its target by word where the screen would have shown it
/// selected, so the popup is what puts the target back in front of the user),
/// and generation does not gate them, the keys they mirror not being gated
/// either. See spec §11.7.
#[derive(Debug, Clone, PartialEq)]
enum ConfirmAction {
    /// Regenerate the last reply (`Ctrl+R`).
    Regenerate,
    /// Delete the last exchange (`Ctrl+E`).
    DeleteExchange,
    /// Delete a companion profile and hide its conversations with it
    /// (`/profile delete <name>`; `Ctrl+D` in the settings screen's "Profiles").
    /// Carries the resolved name and chat count so the question can state what
    /// goes: a prefix may have matched a profile the user did not picture.
    DeleteProfile {
        id: Uuid,
        name: String,
        chats: usize,
    },
    /// Wipe the active profile's self-model (`/self clear`; `Ctrl+K` twice in
    /// the self-model screen, which is itself a confirmation).
    ClearSelfModel,
    /// Delete an impersonation profile (`/impersonation delete <name>`;
    /// `Ctrl+D` in the settings screen's "Impersonation" subsection). Always
    /// confirmed, the `/profile delete` rule: a typed prefix can resolve to a
    /// persona the user did not picture, and its system message is
    /// unrecoverable. The intent is built **at confirm time** from the current
    /// settings snapshot ([`ChatScreen::confirmed_intent`]) — a config edit,
    /// not a fixed intent.
    DeleteImpersonation { id: Uuid, name: String },
}

impl ConfirmAction {
    /// The intent this operation confirms. [`ConfirmAction::DeleteImpersonation`]
    /// has none of its own — the screen builds a config edit at confirm time
    /// ([`ChatScreen::confirmed_intent`]).
    fn intent(&self) -> Option<ChatIntent> {
        match self {
            ConfirmAction::Regenerate => Some(ChatIntent::RegenerateLast),
            ConfirmAction::DeleteExchange => Some(ChatIntent::DeleteLastExchange),
            ConfirmAction::DeleteProfile { id, .. } => Some(ChatIntent::DeleteProfile(*id)),
            ConfirmAction::ClearSelfModel => Some(ChatIntent::ClearSelfModel),
            ConfirmAction::DeleteImpersonation { .. } => None,
        }
    }

    /// The confirmation popup's question text (localized).
    fn prompt(&self, loc: &'static Locale) -> String {
        match self {
            ConfirmAction::Regenerate => loc.t("ui.confirm.regenerate").to_string(),
            ConfirmAction::DeleteExchange => loc.t("ui.confirm.delete_exchange").to_string(),
            ConfirmAction::DeleteProfile { name, chats, .. } => loc.tf(
                "ui.confirm.delete_profile",
                &[("name", name), ("chats", &chats.to_string())],
            ),
            ConfirmAction::ClearSelfModel => loc.t("ui.confirm.clear_self_model").to_string(),
            ConfirmAction::DeleteImpersonation { name, .. } => {
                loc.tf("ui.confirm.delete_impersonation", &[("name", name)])
            }
        }
    }
}

/// A dangerous tool call the agentic loop is holding until the user answers
/// (spec §9.8).
///
/// A separate state rather than another [`ConfirmAction`] variant for two
/// reasons: this popup appears **during** generation — that is the whole point —
/// and it offers three answers instead of yes/no.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ToolConfirm {
    pub(super) generation_id: Uuid,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: String,
}

/// An item of the spellcheck suggestions popup.
#[derive(Debug, Clone, PartialEq)]
enum SuggestItem {
    /// Replace the word with a suggestion.
    Replace(String),
    /// Add the word to the personal dictionary.
    AddToDictionary,
}

/// The spellcheck suggestions popup for the word under the cursor. See spec §11.5.
struct SuggestPopup {
    word: String,
    row: usize,
    start: usize,
    end: usize,
    items: Vec<SuggestItem>,
    selected: usize,
    /// The list's scroll position. The popup is sized to its (at most eight)
    /// suggestions, so it only scrolls on a terminal too short to hold them —
    /// but the rule is the same one every list here follows ([`ListScroll`]).
    scroll: ListScroll,
}

/// Impersonation state (`Ctrl+U`): while a reply "on behalf of the user" is
/// being written, the input box is hidden and a streaming preview is shown
/// instead. See spec §11.8.
struct ImpersonationState {
    generation_id: Uuid,
    /// The reply's accumulated text (starts with whatever seed text was already
    /// typed).
    text: String,
    /// A repaint-tick counter for the spinner animation.
    tick: usize,
    /// Generation has finished (the spinner stops until applied/reset).
    done: bool,
}

/// The progress banner for background indexing — RAG (`/rag add`) and the chat
/// attachment index (`/file attach`) share this one slot: both are "an index is
/// being built in the background", and in practice they don't overlap. Lives
/// while indexing is in progress; completion/error clear the banner and leave a
/// note in the feed.
///
/// The text is kept in **parts** rather than as one string, because the row is
/// one line high and a long file name or page title would otherwise push the
/// counters — the only thing that changes while the banner is up — off the
/// right edge. The renderer fits the parts into the width it has
/// ([`RagBanner::fit`]): the location goes first, then the name is shortened in
/// the middle, and the counters stay.
struct RagBanner {
    /// The fixed text before the name (without the spinner).
    before: String,
    /// What is being indexed — the file or attachment name. The one part the
    /// renderer may shorten; empty for a banner that names nothing (`Started`).
    name: String,
    /// Where it is (` from <dir>`; `/rag add` only) — a detail, dropped whole
    /// when the row is short of columns.
    location: String,
    /// The fixed text after the name — the file and chunk counters.
    after: String,
    /// A repaint-tick counter for the spinner animation.
    tick: usize,
}

/// The chat screen: all UI state and its rendering.
pub struct ChatScreen {
    feed: Vec<FeedMessage>,
    feed_view: MessageFeed,
    active_chat: Option<Uuid>,
    /// `Some` while the open "chat" is a sub-agent transcript (spec §11.2):
    /// the screen is read-only — sending and every chat-changing chord and
    /// command refuse with a note — and the feed opens with the persona as a
    /// system bubble. Set by `app` right before `activate_chat`.
    child: Option<ChildView>,
    title: String,
    chats: Vec<ChatSummary>,
    /// A profile snapshot (for the picker overlay when creating a chat).
    profiles: Vec<ProfileSummary>,
    /// The open profile-picker overlay.
    profile_overlay: Option<ProfileListState>,
    input: InputBox,
    /// A snapshot of every server's status (chat/embeddings/impersonation) for
    /// the status bar. See spec §11.1.
    statuses: ServerStatuses,
    current_gen: Option<Uuid>,
    generating: bool,
    /// Which side of the feed the current stream's bubble belongs to:
    /// `Assistant` everywhere but an open dialogue transcript, where each
    /// line carries its speaker's side (spec §9.13,
    /// `AppEvent::TranscriptLine`).
    stream_role: FeedRole,
    /// The current/last generation's reply token counter (shown in the status
    /// bar). Reset when a new generation starts. See spec §11.1.
    gen_tokens: u64,
    /// Tokens in the conversation (prompt) — a client-side estimate until the
    /// server responds, then the exact `usage.prompt_tokens`; `None` while
    /// unknown.
    gen_context: Option<u64>,
    /// Whether `gen_context` is exact (from the server's `usage`). `false` — an
    /// estimate, the status bar marks it with `~`.
    gen_context_exact: bool,
    /// Reasoning tokens ("thoughts") of the current/last generation, from
    /// `usage` (`0` — none / the provider doesn't separate them). The status bar
    /// shows them separately when `> 0`.
    gen_reasoning: u32,
    /// The model of the current generation (`AppEvent::GenerationStarted`), for
    /// the streaming bubble's header (spec §11.3). Kept on the screen rather
    /// than only on the bubble because the turn can open **more** bubbles:
    /// `send_followup_message` starts a second one mid-turn, and it is the same
    /// model writing it.
    gen_model: Option<String>,
    /// What the **engine** said it is running (`AppEvent::EngineModel`), used by
    /// the title's caption when settings name no model — `external` mode with a
    /// blank "Model (opt.)" field. A configured name always wins; see
    /// `Self::model_meta` and docs/research/external-model-name.md §4.
    engine_model: Option<String>,
    /// What the engine said about its slot count (`AppEvent::EngineSlots`) —
    /// handed to the settings screen when it is built, for the hint next to the
    /// `sessions` field (spec §11.6). `None` — it cannot say.
    engine_slots: Option<u32>,
    /// The spellchecker (loads in the background; `None` until ready / with no
    /// dictionaries).
    spell: Option<SpellChecker>,
    /// The input text changed — a spellcheck recheck is needed (debounced).
    spell_dirty: bool,
    /// The input draft changed — it needs saving to the active chat. The loop
    /// picks up the text (`take_dirty_draft`) and sends `SetDraft`. See spec §11.7.
    draft_dirty: bool,
    /// The moment of the last input edit (for the debounce).
    last_edit: Option<Instant>,
    /// The open spellcheck suggestions popup.
    suggest: Option<SuggestPopup>,
    /// The open emoji-picker popup (`Ctrl+B`). See spec §11.5.
    emoji: Option<EmojiPickerState>,
    /// The open `chat://` reference picker (`Ctrl+L`). See spec §11.3.
    chat_links: Option<ChatLinkPickerState>,
    /// The open modal confirmation popup for an irreversible operation
    /// (`Ctrl+R`/`Ctrl+E`); `None` — the popup is closed. See spec §11.7.
    confirm: Option<ConfirmAction>,
    /// A dangerous tool call awaiting the user's answer (spec §9.8). Independent
    /// of [`Self::confirm`]: this one is modal *during* generation.
    pub(super) tool_confirm: Option<ToolConfirm>,
    /// In-feed search (`Ctrl+F`): a single-line query field standing in for the
    /// message input box while it is open. `None` when not searching. The
    /// message box's own text is never touched — see docs/history/in-feed-search.md §3.
    search: Option<Box<InputBox>>,
    /// The last query typed in this chat, so a repeat `Ctrl+F` resumes it (the
    /// emoji picker remembers its selection the same way). Cleared on chat
    /// switch, in `activate_chat`.
    search_last: String,
    /// Whether to ask for confirmation before `Ctrl+R`/`Ctrl+E` (from
    /// `interface.confirm_destructive_keys`; updated by the `Settings` event).
    confirm_destructive: bool,
    /// The index of the last selection in the emoji popup — restored on the
    /// next open (the popup "remembers" the choice).
    emoji_last: usize,
    /// Whether the feed contains risk-group glyphs (emoji whose rendering on
    /// legacy terminals diverges from the model). A cache: recomputed on a full
    /// feed replacement, otherwise only accumulates. See
    /// [`ChatScreen::mark_feed_changed`].
    feed_has_risky: bool,
    /// A full terminal redraw is requested for the next frame (the
    /// `app/runtime` loop picks up the flag via
    /// [`ChatScreen::take_full_redraw`]). Needed wherever a **wide** glyph
    /// (emoji) disappears from the screen: ratatui's per-cell diff doesn't
    /// repaint its trailing half, leaving an artifact on the terminal. See
    /// spec §11.5.
    full_redraw: bool,
    /// The last settings snapshot (config + full profiles + ids of profiles
    /// with a locked scaffold language + the dynamic MCP catalog) — for
    /// opening the settings screen via `Ctrl+P`. Filled in by the `Settings`
    /// event. See spec §11.6, docs/history/i18n.md.
    settings_snapshot: Option<SettingsSnapshot>,
    /// The active theme palette (from `config.interface.theme`). See spec §11.6.
    palette: Palette,
    /// The interface locale (from `config.interface.language`, axis B —
    /// docs/i18n-ui.md). `&'static` — a built-in bundle; updated together with
    /// the palette in `set_settings`.
    loc: &'static Locale,
    /// Whether mouse capture for wheel scrolling is on (the `Ctrl+W` toggle).
    /// Off by default — native mouse text selection works. See spec §11.3.
    mouse_scroll: bool,
    /// Whether background self-model auto-reflection is running (a quiet
    /// status-bar indicator).
    reflecting: bool,
    /// Sub-agent runs out in the background (`AppEvent::BackgroundRuns`,
    /// spec §9.3.2): a quiet indicator with the count; `0` — none.
    background_runs: u32,
    /// Whether background note auto-consolidation ("sleep") is running (a
    /// quiet indicator).
    consolidating: bool,
    /// Whether background self-model auto-consolidation ("self-model sleep")
    /// is running (a quiet indicator). See
    /// docs/history/self-model-consolidation.md.
    self_consolidating: bool,
    /// Whether a background history compaction is running (a quiet indicator).
    /// See spec §6.7, docs/research/history-compression.md §6.5.
    compacting: bool,
    /// The already-composed "retrying n/m in k s" label while the engine waits
    /// before another attempt, or `None` (spec §6.8). Unlike its neighbours this is
    /// a string rather than a flag, because the chip carries the numbers; it is
    /// cleared by content or by the turn finishing, so it cannot outlive the wait.
    retrying: Option<String>,
    /// The composed "sub-agent «name» · round N · tool" labels of the runs in
    /// flight (`AppEvent::SubagentProgress`), one per run in report order —
    /// the chip shows the one line, or the count and the latest when the
    /// model delegated several tasks at once. Empty — no run in flight.
    /// Spec §9.3.2.
    subagents: Vec<(Uuid, String)>,
    /// The generation running on the open conversation when it is a sub-agent
    /// transcript (`ChatActivated::live_turn`, docs/subagent-live.md §3.4):
    /// the chip's guard while `current_gen` stays `None` — the parent's
    /// stream must not land in the transcript's feed.
    live_turn: Option<Uuid>,
    /// The open transcript is a run out in the **background** and still
    /// streaming (`LiveTurn::background`, spec §9.3.2): its stream is no
    /// turn's, so `Esc` goes back to the list instead of cancelling, and
    /// `F6` stops the run — the one key the footer advertises only while it
    /// works (spec §11.2). Cleared when the run's stream ends.
    background_run: bool,
    /// The open sub-agent transcript's messages, kept only while one is open
    /// and running, so a filed round rebuilds the feed stitched exactly as a
    /// landed transcript would be (`TranscriptGrew`).
    transcript: Vec<Message>,
    /// Whether speech (`/tts`) is playing — a quiet "♪ speaking" chip in the
    /// status bar.
    speaking: bool,
    /// Where `Esc` takes the user from here — decides the status bar's `Esc`
    /// hint. Set by `app/runtime`, which owns the back-stack; the screen only
    /// renders the label (FSD: `screens` may not depend on `app`).
    esc_target: EscTarget,
    /// The background RAG-indexing indicator (`/rag add`); `None` — no
    /// indexing in progress.
    rag: Option<RagBanner>,
    /// Cards for the active chat's attached files (`/file attach`) — for the
    /// status-bar chip. Updated by `AppEvent::Attachments`. See
    /// docs/file-attachments.md.
    attachments: Vec<AttachmentInfo>,
    /// Cards for the images staged for the **next message** (`/image attach`) —
    /// for the status-bar chip. Updated by `AppEvent::StagedImages`. Separate
    /// from [`Self::attachments`] because the lifetime differs: an attachment is
    /// pinned to the chat, a staged image leaves this list the moment the
    /// message carrying it is sent. See spec §9.10.
    staged_images: Vec<ImageInfo>,
    /// Impersonation state (`Ctrl+U`); `None` — not running. See spec §11.8.
    impersonation: Option<ImpersonationState>,
    /// Whether a tool was called after the last text chunk of the streaming
    /// reply: the next round's first text is separated by a blank line
    /// (`\n\n`), so the live stream matches a reload (`FeedMessage::from_messages`).
    pending_text_sep: bool,
    /// The same for the "thoughts" block (a `\n` separator between rounds).
    pending_thoughts_sep: bool,
}

impl Default for ChatScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatScreen {
    pub fn new() -> Self {
        Self {
            feed: Vec::new(),
            feed_view: MessageFeed::new(),
            active_chat: None,
            child: None,
            title: String::new(),
            chats: Vec::new(),
            profiles: Vec::new(),
            profile_overlay: None,
            input: InputBox::new(),
            statuses: ServerStatuses {
                chat: ServerStatus::Connecting,
                embed: ServerStatus::NotConfigured,
                impersonation: ServerStatus::NotConfigured,
            },
            current_gen: None,
            generating: false,
            stream_role: FeedRole::Assistant,
            gen_tokens: 0,
            gen_context: None,
            gen_context_exact: false,
            gen_reasoning: 0,
            gen_model: None,
            spell: None,
            spell_dirty: false,
            draft_dirty: false,
            last_edit: None,
            suggest: None,
            emoji: None,
            emoji_last: 0,
            chat_links: None,
            feed_has_risky: false,
            full_redraw: false,
            confirm: None,
            tool_confirm: None,
            search: None,
            search_last: String::new(),
            confirm_destructive: false,
            settings_snapshot: None,
            engine_model: None,
            engine_slots: None,
            palette: Palette::default(),
            loc: locale(crate::shared::i18n::Lang::default()),
            mouse_scroll: false,
            reflecting: false,
            background_runs: 0,
            consolidating: false,
            self_consolidating: false,
            compacting: false,
            retrying: None,
            subagents: Vec::new(),
            live_turn: None,
            background_run: false,
            transcript: Vec::new(),
            speaking: false,
            esc_target: EscTarget::default(),
            rag: None,
            attachments: Vec::new(),
            staged_images: Vec::new(),
            impersonation: None,
            pending_text_sep: false,
            pending_thoughts_sep: false,
        }
    }

    /// Saves the settings snapshot (for opening the settings screen via
    /// `Ctrl+P`) and updates the theme palette (together with the terminal
    /// compatibility mode) and feed-rendering parameters (table row
    /// separators). `mcp` — the MCP host snapshot (the dynamic tool catalog +
    /// server statuses).
    pub fn set_settings(
        &mut self,
        config: AppConfig,
        profiles: Vec<Profile>,
        language_locked: Vec<uuid::Uuid>,
        mcp: crate::features::tools::mcp::McpSnapshot,
        secrets_present: Vec<crate::shared::secrets::SecretKey>,
    ) {
        self.palette = Palette::for_theme(config.interface.theme)
            .with_compat(config.interface.terminal_compat);
        self.loc = locale(config.interface.language);
        self.confirm_destructive = config.interface.confirm_destructive_keys;
        self.feed_view
            .set_table_row_separators(config.interface.table_row_separators);
        self.feed_view
            .set_render_mermaid(config.interface.render_mermaid);
        self.feed_view
            .set_show_model_name(config.interface.show_model_name);
        self.settings_snapshot = Some((config, profiles, language_locked, mcp, secrets_present));
    }

    /// Records what the engine said it is running (`AppEvent::EngineModel`).
    /// `None` — it cannot say, or the engine was just replaced; the caption then
    /// falls back to the configuration alone. See `Self::model_meta`.
    pub fn set_engine_model(&mut self, model: Option<String>) {
        self.engine_model = model;
    }

    /// Records what the engine said about its slot count (`AppEvent::EngineSlots`),
    /// kept here so a settings screen opened later starts with it (spec §11.6).
    pub fn set_engine_slots(&mut self, slots: Option<u32>) {
        self.engine_slots = slots;
    }

    /// The engine's slot count as last reported, for a settings screen being built.
    pub fn engine_slots(&self) -> Option<u32> {
        self.engine_slots
    }

    /// Sets the role names shown in the feed — the active chat's profile
    /// `character_names` (`AppEvent::CharacterNames`). An empty field keeps the
    /// localized header. See spec §11.3.
    pub fn set_character_names(&mut self, names: CharacterNames) {
        self.feed_view.set_role_names(names);
    }

    /// The settings snapshot for building the settings screen (`None` until received).
    pub fn settings_snapshot(&self) -> Option<SettingsSnapshot> {
        self.settings_snapshot.clone()
    }

    /// Whether a copy also goes to the terminal's clipboard over OSC 52
    /// (`interface.clipboard_osc52`). Read from the last settings snapshot —
    /// before the first `Settings` event arrives, the default applies, which is
    /// the conservative `Auto`. See [`crate::shared::osc52`].
    pub fn clipboard_osc52(&self) -> crate::shared::osc52::Osc52Mode {
        self.settings_snapshot
            .as_ref()
            .map(|(config, ..)| config.interface.clipboard_osc52)
            .unwrap_or_default()
    }

    /// Which end of the narrative the self-model screen (`F3`) lists observations
    /// from (`interface.self_model_note_order`, spec §17.7). Read from the last
    /// settings snapshot, like [`Self::clipboard_osc52`] — before the first
    /// `Settings` event the default (newest first) applies.
    pub fn self_model_note_order(&self) -> crate::shared::config::NoteOrder {
        self.settings_snapshot
            .as_ref()
            .map(|(config, ..)| config.interface.self_model_note_order)
            .unwrap_or_default()
    }

    /// The current spellcheck settings `(enabled, selected dictionaries)` from
    /// the last settings snapshot — for (re)loading dictionaries in
    /// `app/runtime.rs`. See spec §11.6.
    pub fn spell_config(&self) -> Option<(bool, &[String])> {
        self.settings_snapshot.as_ref().map(|(c, _, _, _, _)| {
            (
                c.interface.spellcheck_enabled,
                c.interface.selected_dictionaries.as_slice(),
            )
        })
    }

    /// Sets the spellchecker (after the background dictionary load) and
    /// schedules a recheck of the current input.
    pub fn set_spellchecker(&mut self, checker: SpellChecker) {
        self.spell = Some(checker);
        self.spell_dirty = true;
        self.last_edit = None; // recheck immediately
    }

    /// The current spellchecker (or `None` until dictionaries are loaded). The
    /// checker lives here, on the chat screen; `app` lends it to the chat-list
    /// screen for error highlighting in the rename field (`F2`). See spec §11.5.
    pub fn spellchecker(&self) -> Option<&SpellChecker> {
        self.spell.as_ref()
    }

    // ---------- orchestrator event projection (called by the `app` layer) ----------

    pub fn set_server_status(&mut self, statuses: ServerStatuses) {
        self.statuses = statuses;
    }

    /// The current server-statuses snapshot — so `app` can pass it to the
    /// settings screen it's opening (status chips in the "Model/server"
    /// section). See spec §11.6.
    pub fn server_statuses(&self) -> ServerStatuses {
        self.statuses.clone()
    }

    /// Sets/clears the active-auto-reflection flag (a quiet status-bar
    /// indicator). `app` distinguishes the kind of background task (mapping
    /// `AppEvent::BackgroundTask`) — so `screens` doesn't depend on `app`'s
    /// contract (FSD).
    pub fn set_reflecting(&mut self, active: bool) {
        self.reflecting = active;
    }

    /// How many sub-agent runs are out in the background — the quiet
    /// indicator's count (spec §9.3.2); `0` clears it.
    pub fn set_background_runs(&mut self, out: u32) {
        self.background_runs = out;
    }

    /// Sets/clears the active note auto-consolidation flag ("sleep").
    pub fn set_consolidating(&mut self, active: bool) {
        self.consolidating = active;
    }

    /// Sets/clears the active self-model auto-consolidation flag ("self-model sleep").
    pub fn set_self_consolidating(&mut self, active: bool) {
        self.self_consolidating = active;
    }

    /// Sets/clears the active history-compaction flag (`/compact` or the
    /// automatic roll) — a quiet status-bar indicator. See spec §6.7.
    pub fn set_compacting(&mut self, active: bool) {
        self.compacting = active;
    }

    /// Whether one of the app's own silent tasks is running — its slot
    /// taken, streaming or waiting on the lane (`AppEvent::BackgroundTask`,
    /// on from the spawn to the landing): the status bar's own source, and
    /// what `/tasks stop` answers from (spec §11.10).
    pub(super) fn task_running(&self, kind: BackgroundKind) -> bool {
        match kind {
            BackgroundKind::Reflection => self.reflecting,
            BackgroundKind::Consolidation => self.consolidating,
            BackgroundKind::SelfConsolidation => self.self_consolidating,
            BackgroundKind::Compaction => self.compacting,
        }
    }

    /// Sets/clears the active-speech flag (`/tts`) — a status-bar chip.
    pub fn set_speaking(&mut self, active: bool) {
        self.speaking = active;
    }

    /// Tells the screen where `Esc` currently goes, so the status bar's hint
    /// says so. `app/runtime` derives this from its back-stack on every frame
    /// (see `esc_target` there) rather than mirroring it at each place the
    /// stack changes — one source, so the hint cannot drift from the key.
    pub fn set_esc_target(&mut self, target: EscTarget) {
        self.esc_target = target;
    }

    /// A label of active background tasks for the status bar (`None` — nothing
    /// is running). Assembled from the active tasks' labels (a `·` separator) —
    /// generalizes to any number of them (reflection / note sleep / self-model
    /// sleep can run in parallel).
    pub(super) fn background_hint(&self) -> Option<String> {
        // The sub-agent chip: the one run's line, or the count and the
        // latest line when several run at once (spec §9.3.2). Composed before
        // the strip so the borrowed pieces outlive it.
        let subagents_label: Option<String> = self.subagents.last().map(|(_, latest)| {
            if self.subagents.len() == 1 {
                latest.clone()
            } else {
                self.loc.tf(
                    "ui.chat.bg.subagents",
                    &[("n", &self.subagents.len().to_string()), ("latest", latest)],
                )
            }
        });
        let mut parts: Vec<&str> = Vec::new();
        if self.reflecting {
            parts.push(self.loc.t("ui.chat.bg.reflect"));
        }
        if self.consolidating {
            parts.push(self.loc.t("ui.chat.bg.consolidate"));
        }
        if self.self_consolidating {
            parts.push(self.loc.t("ui.chat.bg.self_consolidate"));
        }
        if self.compacting {
            parts.push(self.loc.t("ui.chat.bg.compact"));
        }
        let background_runs = (self.background_runs > 0).then(|| {
            self.loc.tf(
                "ui.chat.bg.background_runs",
                &[("n", &self.background_runs.to_string())],
            )
        });
        if let Some(label) = background_runs.as_deref() {
            parts.push(label);
        }
        // The sub-agent chip after the background tasks: it belongs to the
        // turn in flight, like the retry — the two read together at the end.
        if let Some(label) = subagents_label.as_deref() {
            parts.push(label);
        }
        // Last, so a retry the user is waiting on reads at the end of the strip
        // next to the generation indicator rather than in the middle of the
        // background tasks.
        if let Some(retry) = &self.retrying {
            parts.push(retry);
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    pub fn set_chat_list(&mut self, chats: Vec<ChatSummary>) {
        self.chats = chats;
        self.refresh_known_chats();
    }

    /// Whose transcript the screen is showing, when it is one
    /// (`AppEvent::ChatActivated::child`). Applied **before** `activate_chat`
    /// builds the feed, because the feed is what differs.
    pub fn set_child_view(&mut self, child: Option<ChildView>) {
        self.child = child;
    }

    /// The generation in flight on the conversation just activated
    /// (docs/history/subagent-live.md §3.4–§3.5, §8). On the running turn's
    /// **chat**: the screen is generating again — the stream and the chip
    /// resume into this feed. On the turn's **transcript**: the sub-agent's
    /// own stream resumes into it, seeded with the round so far; the chip is
    /// keyed on the turn, so it stays too.
    pub fn set_live_turn(&mut self, live_turn: Option<Box<crate::app::events::LiveTurn>>) {
        let Some(live) = live_turn else {
            self.live_turn = None;
            self.background_run = false;
            self.subagents.clear();
            return;
        };
        self.live_turn = Some(live.turn);
        // Only a transcript can be a background run's; a turn's chat streams
        // the turn, whatever the flag says.
        self.background_run = live.background && self.child.is_some();
        if self.child.is_none() {
            // The turn's own chat: the rounds so far are in the feed already;
            // the round in progress is seeded below, the rest streams on.
            self.current_gen = Some(live.stream);
            self.generating = true;
            // A continuation's round in progress belongs in the bubble it
            // resumes (`/continue`, spec §6.4), not in a fresh one — re-open
            // it so the partial below and the stream after land there.
            if live.continues {
                self.resume_last_assistant_bubble();
            }
        } else {
            // A running transcript (docs/history/subagent-live.md §8): its own
            // stream, into a bubble of its own — on the current line's side,
            // when the run is a dialogue (spec §9.13).
            self.begin_generation(live.stream, None);
            if live.role == crate::entities::message::MessageRole::User {
                self.set_stream_role(FeedRole::User);
            }
        }
        let Some(partial) = live.partial else {
            return;
        };
        if !partial.thoughts.is_empty() {
            self.push_thoughts(live.stream, &partial.thoughts);
        }
        if !partial.text.is_empty() {
            self.push_chunk(live.stream, &partial.text);
        }
        for tool in partial.tools {
            self.push_tool_call_started(
                live.stream,
                tool.call_id.clone(),
                tool.name.clone(),
                tool.arguments.clone(),
            );
            if let Some((result, images)) = tool.result {
                self.push_tool_call(
                    live.stream,
                    tool.call_id,
                    tool.name,
                    tool.arguments,
                    result,
                    images,
                );
            }
        }
    }

    /// A running transcript filed a round (`AppEvent::TranscriptGrew`): the
    /// feed is rebuilt from every message so far, so the rounds stitch into
    /// one bubble exactly as they will once the run lands; the scroll follows
    /// the tail only if it already did.
    pub fn grow_transcript(&mut self, id: Uuid, messages: &[Message]) {
        if self.active_chat != Some(id) || self.child.is_none() {
            return;
        }
        self.transcript.extend(messages.iter().cloned());
        self.feed = FeedMessage::from_messages(&self.transcript);
        if let Some(child) = &self.child {
            self.feed
                .insert(0, FeedMessage::system(child.system_message.clone()));
        }
        self.feed_has_risky = self.feed.iter().any(feed::feed_msg_has_risky_glyph);
        self.mark_feed_changed();
        self.feed_view.scroll_to_bottom_if_following();
    }

    /// A running dialogue edited its open transcript
    /// (`AppEvent::TranscriptReset`, spec §9.13): a discarded or rewritten
    /// line cannot be expressed by appending, so the feed is rebuilt from the
    /// full replacement.
    pub fn reset_transcript(&mut self, id: Uuid, messages: &[Message]) {
        if self.active_chat != Some(id) || self.child.is_none() {
            return;
        }
        self.transcript = messages.to_vec();
        self.feed = FeedMessage::from_messages(&self.transcript);
        if let Some(child) = &self.child {
            self.feed
                .insert(0, FeedMessage::system(child.system_message.clone()));
        }
        self.feed_has_risky = self.feed.iter().any(feed::feed_msg_has_risky_glyph);
        self.mark_feed_changed();
        self.feed_view.scroll_to_bottom_if_following();
    }

    /// Whether the open "chat" is a read-only sub-agent transcript.
    pub fn read_only(&self) -> bool {
        self.child.is_some()
    }

    /// A chat-changing action on a read-only transcript: the note that says
    /// so and names the way that works — the parent chat (lessons §4).
    /// `true` when the action was refused.
    pub(super) fn refuse_read_only(&mut self, what: &str) -> bool {
        let Some(child) = &self.child else {
            return false;
        };
        let msg = self.loc.tf(
            "ui.chat.read_only",
            &[("what", what), ("parent", &child.parent_title)],
        );
        self.push_note(&msg);
        true
    }

    /// The card for `id` from the list snapshot — a chat's own, or a
    /// sub-agent transcript's, shaped like one (spec §11.2).
    pub(super) fn summary_card(&self, id: Uuid) -> Option<ChatSummary> {
        if let Some(chat) = self.chats.iter().find(|c| c.id == id) {
            return Some(chat.clone());
        }
        self.chats.iter().find_map(|chat| {
            chat.children
                .iter()
                .find(|c| c.id == id)
                .map(|c| chat.child_card(c))
        })
    }

    /// Refreshes the feed's `chat://` address book: this profile's
    /// conversations, the open one included (spec §11.3).
    ///
    /// Derived from the chat-list snapshot the screen already keeps rather than
    /// from an event of its own — the active chat's own entry names its
    /// profile, so the profile boundary (spec §9.5) cannot drift from the list
    /// the user is looking at. The snapshot holds only non-hidden chats, which
    /// is the other half of the scope.
    ///
    /// Called from both sides of the race: the list can arrive before the chat
    /// is activated or after it.
    pub(super) fn refresh_known_chats(&mut self) {
        // The open chat may itself be a transcript; its card names the profile.
        let profile = self
            .active_chat
            .and_then(|id| self.summary_card(id))
            .map(|c| c.profile_id);
        // The profile's chats and their transcripts alike: a `chat://` address
        // a sub-agent's result carries resolves like any other (spec §9.3.2).
        let ids = profile
            .map(|p| {
                self.chats
                    .iter()
                    .filter(|c| c.profile_id == p)
                    .flat_map(|c| std::iter::once(c.id).chain(c.child_ids()))
                    .collect()
            })
            .unwrap_or_default();
        self.feed_view.set_known_chats(ids);
    }

    pub fn set_profile_list(&mut self, profiles: Vec<ProfileSummary>) {
        self.profiles = profiles;
    }

    /// A snapshot of the chat list — for building the list screen via `Esc`
    /// (`OpenChatList`).
    pub fn chat_summaries(&self) -> Vec<ChatSummary> {
        self.chats.clone()
    }

    /// The active chat (the marker in the list screen; `None` until a chat is
    /// selected).
    pub fn active_chat(&self) -> Option<Uuid> {
        self.active_chat
    }

    /// The current theme palette — for rendering the chat-list screen.
    pub fn palette(&self) -> Palette {
        self.palette
    }

    /// The current interface locale — for rendering overlay screens (chat list
    /// / self-model) and broadcasting on a language change. See
    /// docs/i18n-ui.md §3.3.
    pub fn loc(&self) -> &'static Locale {
        self.loc
    }
}

// ---------- submodules (god-object breakup: docs/history/refactoring-god-objects.md, stage 2) ----------

mod attachments;
mod commands;
mod feed;
mod images;
mod impersonation;
mod input;
mod popups;
mod project;
mod rag;
mod render;

#[cfg(test)]
mod tests;
