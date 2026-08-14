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
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use uuid::Uuid;

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
use crate::shared::ui::{dim_background, render_scrollbar};
use crate::widgets::chat_link_picker::{ChatLinkAction, ChatLinkPickerState};
use crate::widgets::emoji_picker::{EmojiPickerAction, EmojiPickerState};
use crate::widgets::impersonation_preview;
use crate::widgets::input_box::InputBox;
use crate::widgets::message_feed::{FeedMessage, FeedRole, MessageFeed};
use crate::widgets::profile_list::{ProfileListAction, ProfileListState};
use crate::widgets::status_bar::{self, EscTarget};

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

/// An irreversible operation that requires confirmation in a modal popup
/// (`Ctrl+R`/`Ctrl+E`, when the setting `interface.confirm_destructive_keys` is
/// on). See spec §11.7.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConfirmAction {
    /// Regenerate the last reply (`Ctrl+R`).
    Regenerate,
    /// Delete the last exchange (`Ctrl+E`).
    DeleteExchange,
}

impl ConfirmAction {
    /// The intent this operation confirms.
    fn intent(self) -> ChatIntent {
        match self {
            ConfirmAction::Regenerate => ChatIntent::RegenerateLast,
            ConfirmAction::DeleteExchange => ChatIntent::DeleteLastExchange,
        }
    }

    /// The confirmation popup's question text (localized).
    fn prompt(self, loc: &'static Locale) -> &'static str {
        match self {
            ConfirmAction::Regenerate => loc.t("ui.confirm.regenerate"),
            ConfirmAction::DeleteExchange => loc.t("ui.confirm.delete_exchange"),
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

/// A tab of the help/"About" dialog (`F1`/`?`), KDE/Qt-style. See spec §11.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HelpTab {
    /// Name/author/version and links (site, repository, crate).
    About,
    /// The hotkey list.
    Hotkeys,
    /// Input-box commands (`/rag …`, `/tts …`).
    Commands,
    /// The application's license text (MIT).
    License,
    /// The disclaimer covering model output, tools and automated actions
    /// (`DISCLAIMER.md`) — a supplement to the license, not part of it.
    Disclaimer,
    /// Third-party components, their versions and licenses.
    Components,
}

impl HelpTab {
    /// Tabs in display order (the tab strip's order).
    pub(super) const ALL: [HelpTab; 6] = [
        Self::About,
        Self::Hotkeys,
        Self::Commands,
        Self::License,
        Self::Disclaimer,
        Self::Components,
    ];

    /// The tab's position in [`Self::ALL`].
    fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap()
    }

    /// The locale key for the tab's name (for the tab strip).
    pub(super) fn label_key(self) -> &'static str {
        match self {
            Self::About => "ui.help.tab.about",
            Self::Hotkeys => "ui.help.tab.hotkeys",
            Self::Commands => "ui.help.tab.commands",
            Self::License => "ui.help.tab.license",
            Self::Disclaimer => "ui.help.tab.disclaimer",
            Self::Components => "ui.help.tab.components",
        }
    }
}

/// The tab the help dialog opens on by default (and until a choice is first
/// remembered): `F1`/`?` — the familiar help key, and "Hotkeys" is the most
/// sought-after content; "About" is the neighboring tab.
pub(super) const DEFAULT_HELP_TAB: HelpTab = HelpTab::Hotkeys;

/// The help dialog's state: the active tab + its content's scroll position
/// (reset on tab switch). Opens on `F1`/`?`. See spec §11.7.
pub(super) struct HelpState {
    pub(super) tab: HelpTab,
    /// The first visible row of the active tab's content (clamped in `render_help`).
    pub(super) scroll: usize,
}

impl HelpState {
    /// Open on the given tab (on reopening — on the last-selected one,
    /// [`ChatScreen::help_last_tab`]).
    pub(super) fn open(tab: HelpTab) -> Self {
        Self { tab, scroll: 0 }
    }

    /// The next tab (wrapping); resets scroll.
    pub(super) fn next_tab(&mut self) {
        let n = HelpTab::ALL.len();
        self.tab = HelpTab::ALL[(self.tab.index() + 1) % n];
        self.scroll = 0;
    }

    /// The previous tab (wrapping); resets scroll.
    pub(super) fn prev_tab(&mut self) {
        let n = HelpTab::ALL.len();
        self.tab = HelpTab::ALL[(self.tab.index() + n - 1) % n];
        self.scroll = 0;
    }
}

/// The spellcheck suggestions popup for the word under the cursor. See spec §11.5.
struct SuggestPopup {
    word: String,
    row: usize,
    start: usize,
    end: usize,
    items: Vec<SuggestItem>,
    selected: usize,
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
struct RagBanner {
    /// The indicator's current text (without the spinner).
    text: String,
    /// A repaint-tick counter for the spinner animation.
    tick: usize,
}

/// The chat screen: all UI state and its rendering.
pub struct ChatScreen {
    feed: Vec<FeedMessage>,
    feed_view: MessageFeed,
    active_chat: Option<Uuid>,
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
    /// The help/"About" dialog (`F1`/`?`): tabs + the active tab's scroll
    /// position; `None` — closed. See spec §11.7.
    help: Option<HelpState>,
    /// The last-opened tab of the help dialog — restored on reopening (the
    /// popup "remembers" the choice, like the emoji picker).
    help_last_tab: HelpTab,
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
            gen_tokens: 0,
            gen_context: None,
            gen_context_exact: false,
            gen_reasoning: 0,
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
            help: None,
            help_last_tab: DEFAULT_HELP_TAB,
            palette: Palette::default(),
            loc: locale(crate::shared::i18n::Lang::default()),
            mouse_scroll: false,
            reflecting: false,
            consolidating: false,
            self_consolidating: false,
            compacting: false,
            retrying: None,
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
        self.settings_snapshot = Some((config, profiles, language_locked, mcp, secrets_present));
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
        let profile = self
            .active_chat
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|c| c.profile_id);
        let ids = profile
            .map(|p| {
                self.chats
                    .iter()
                    .filter(|c| c.profile_id == p)
                    .map(|c| c.id)
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
mod feed;
mod images;
mod impersonation;
mod input;
mod popups;
mod rag;
mod render;

#[cfg(test)]
mod tests;
