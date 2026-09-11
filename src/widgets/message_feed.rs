//! Message feed: markdown rendering of bodies (via [`crate::shared::markdown`]),
//! a collapsible "thoughts" block (CoT), and vertical scrolling. See spec §11.3-11.4.
//!
//! "Thoughts" collapse via a global toggle (`show_thoughts`); tool blocks
//! (name/arguments/result) show inside the assistant's message (M5).
//! Per-message/per-block selection — later. The widget only holds view state
//! (scroll, "follow the tail", thoughts display); the messages themselves
//! belong to UI state and are passed in for rendering.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use uuid::Uuid;

use crate::entities::chat::FeedView;
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::CharacterNames;
use crate::features::tools::present::{self, ToolBlock};
use crate::shared::i18n::Locale;
use crate::shared::markdown;
use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap;

/// Gutter rail to the left of every message line: a colored vertical bar +
/// a space (redesign: "Role rails — colored ▌ in the gutter"). Width — 2 columns.
const RAIL: &str = "▌ ";

/// Role of a feed item (a UI projection). A chat's own system message is
/// never shown; `System` is the bubble a **sub-agent transcript** opens with —
/// the persona its parent composed is the point of reading one (spec §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedRole {
    User,
    Assistant,
    /// The persona of a sub-agent transcript, drawn first.
    System,
    /// A service note (errors, "generation cancelled").
    Note,
}

/// A tool block in the feed: tool name, arguments, and result. See spec §11.3.
///
/// `text_offset` — the call's position **in bytes** within `FeedMessage::text`: how
/// much of the assistant's reply text had been generated BEFORE this call. This way
/// the tool block is drawn exactly at the call site (between text fragments), not in a "header".
#[derive(Debug, Clone)]
pub struct FeedToolCall {
    pub name: String,
    pub arguments: String,
    pub result: String,
    /// Offset (in bytes) into `FeedMessage::text` after which the call was made.
    pub text_offset: usize,
    /// How many images the call returned (spec §9.10). The block shows a chip rather
    /// than the picture — rendering pixels in a terminal is its own roadmap item, and
    /// the count is what makes an otherwise invisible cost visible.
    pub images: usize,
    /// The call's id on the wire, so a live card started by
    /// `AppEvent::ToolCallStarted` is the one `AppEvent::ToolCall` completes
    /// (spec §11.3). `None` for a card built from a stored message — nothing
    /// will complete it.
    pub call_id: Option<String>,
    /// The call is executing right now: the result area shows *running…*
    /// instead of a result. Only a live card is ever running.
    pub running: bool,
}

/// An item of the message feed.
#[derive(Debug, Clone)]
pub struct FeedMessage {
    pub role: FeedRole,
    pub text: String,
    pub thoughts: String,
    /// Tool calls of this assistant message (tool blocks).
    pub tools: Vec<FeedToolCall>,
    /// Whether this message is currently streaming (shows "…" instead of an empty body).
    pub streaming: bool,
    /// Ids of the **domain** messages folded into this feed item — the only link
    /// back from a feed position to a [`Message`], and what
    /// [`MessageFeed::focus_message`] resolves a jump against.
    ///
    /// A `Vec`, not an `Option<Uuid>`, because the projection is genuinely
    /// many-to-one: [`FeedMessage::from_messages`] merges the assistant messages
    /// of an agentic-loop round into a single bubble, so a single id would lie
    /// about which messages a bubble shows. The mapping is unrecoverable after
    /// the fact, hence it is recorded where the merging happens (see
    /// docs/history/chat-search-stage2.md §1.2).
    ///
    /// **Empty** for items with no domain message behind them: service notes and
    /// the live streaming bubble (the streaming path pushes literals and never
    /// goes through `from_messages` — the in-flight reply has no id yet).
    pub message_ids: Vec<Uuid>,
    /// The model that wrote this bubble, from the message's metadata snapshot
    /// (`MessageMetadata::model`) — drawn next to the role header when
    /// `interface.show_model_name` is on (spec §11.3).
    ///
    /// `None` is the honest answer for everything with no model behind it:
    /// user messages, service notes, and any assistant message stored before
    /// the metadata snapshot existed. The live streaming bubble is the one item
    /// that carries it *without* a domain message — the screen fills it from
    /// `AppEvent::GenerationStarted`, which names the same model the finished
    /// message will record, so the header does not change under the reader when
    /// the turn ends.
    pub model: Option<String>,
}

impl FeedMessage {
    /// The system bubble a sub-agent transcript opens with (spec §11.3): the
    /// persona, as the parent composed it. Not a domain message — the
    /// transcript keeps it as a field, like any chat — so it has no id.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: FeedRole::System,
            text: text.into(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: false,
            message_ids: Vec::new(),
            model: None,
        }
    }

    /// A service note for the feed.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            role: FeedRole::Note,
            text: text.into(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: false,
            message_ids: Vec::new(),
            model: None,
        }
    }

    /// Projects a domain message (used to rebuild the feed on chat activation).
    /// System and tool messages are dropped (`None`): tool calls are shown as
    /// blocks inside the assistant's message (`tool_calls`), not separately.
    ///
    /// All of a round's tool calls happen AFTER its text, so their
    /// `text_offset` = the text's length (calls are drawn below it).
    pub fn from_message(msg: &Message) -> Option<Self> {
        let role = match msg.role {
            MessageRole::User => FeedRole::User,
            MessageRole::Assistant => FeedRole::Assistant,
            // A `System` entry inside a message list is a director's
            // intervention in a dialogue transcript (spec §9.13) — drawn as a
            // note row. Top chats keep their system message on the chat, never
            // in the list, so nothing else reaches this arm.
            MessageRole::System => FeedRole::Note,
            MessageRole::Tool => return None,
        };
        let off = msg.text.len();
        // Conversation control tools (followup/rewrite) are control signals,
        // not content: their tool blocks aren't shown in the feed. See spec §9.3.
        let tools = msg
            .tool_calls
            .iter()
            .filter(|tc| !crate::features::tools::control::is_control_tool(&tc.name))
            .map(|tc| FeedToolCall {
                name: tc.name.clone(),
                arguments: tc.arguments.to_string(),
                result: tc.result.clone().unwrap_or_default(),
                text_offset: off,
                images: tc.images,
                call_id: None,
                running: false,
            })
            .collect();
        Some(Self {
            role,
            text: msg.text.clone(),
            thoughts: msg.thoughts.clone().unwrap_or_default(),
            tools,
            streaming: false,
            message_ids: vec![msg.id],
            model: msg
                .metadata
                .as_ref()
                .and_then(|meta| meta.model.clone())
                .filter(|m| !m.is_empty()),
        })
    }

    /// Projects a list of domain messages into the feed with **round stitching**:
    /// consecutive assistant messages (agentic-loop rounds, with tool messages
    /// dropped from between them in history) merge into one "Assistant:" block.
    /// Round texts are concatenated (via a blank line), and each round's `text_offset`
    /// for its calls shifts by the accumulated length — so a live stream and a
    /// reload from history produce the same inline call layout.
    pub fn from_messages(messages: &[Message]) -> Vec<Self> {
        let mut out: Vec<Self> = Vec::new();
        for msg in messages {
            let Some(fm) = Self::from_message(msg) else {
                continue;
            };
            // Merge the assistant round into the previous assistant block — except
            // when the message is flagged `new_bubble` (the "write another message"
            // tool): then it starts a separate bubble. See spec §9.3.
            if fm.role == FeedRole::Assistant
                && !msg.new_bubble
                && let Some(last) = out.last_mut()
                && last.role == FeedRole::Assistant
            {
                Self::merge_round(last, fm);
                continue;
            }
            out.push(fm);
        }
        out
    }

    /// Folds the assistant round `fm` into the previous assistant block `last`
    /// (round stitching): text joined via a blank line, the round's tool
    /// `text_offset`s rebased onto the merged text, thoughts joined via a newline.
    fn merge_round(last: &mut Self, mut fm: Self) {
        if !fm.text.is_empty() {
            if !last.text.is_empty() {
                last.text.push_str("\n\n");
            }
            last.text.push_str(&fm.text);
        }
        let base = last.text.len();
        for mut tc in fm.tools.drain(..) {
            tc.text_offset = base;
            last.tools.push(tc);
        }
        if !fm.thoughts.is_empty() {
            if !last.thoughts.is_empty() {
                last.thoughts.push('\n');
            }
            last.thoughts.push_str(&fm.thoughts);
        }
        // The merge is where the many-to-one mapping happens, so it is
        // where every folded-in id has to be recorded — a jump to any
        // round of the bubble must land on the bubble (§1.2).
        last.message_ids.append(&mut fm.message_ids);
        // One bubble, one header, so one model name. The **first** round's
        // answer wins: it is the one the header was already showing, and a
        // round that stored no metadata (a pure tool-call round has no text and
        // never becomes a message at all) must not blank it out.
        if last.model.is_none() {
            last.model = fm.model.take();
        }
    }
}

/// Feed view state (scroll, thoughts display).
pub struct MessageFeed {
    /// Scroll offset (in lines from the start).
    scroll: usize,
    /// Follow the tail (auto-scroll to bottom on new content).
    follow: bool,
    /// Which foldable blocks are expanded — "thoughts" (`Ctrl+T`) and tool
    /// calls (`Ctrl+O`). Per chat: the screen loads it on activation and sends
    /// every toggle back to the orchestrator, which stores it on the chat
    /// (spec §11.3, docs/feed-collapse.md).
    view: FeedView,
    /// Flag "the feed was just scrolled by the user" — the loop, on it, does a
    /// full terminal repaint (like on resize). Needed because of artifacts in
    /// some terminals (Command Prompt/conhost) on VS16 emoji (`🕸️`,
    /// `🗂️`): they draw such a cluster physically wider than ratatui's model, content
    /// "drifts" horizontally, and on paged scrolling (PageUp/
    /// PageDown) a "hanging" letter is left in the physical cell where the
    /// drifted character used to be — ratatui's per-cell diff no longer touches it. A full
    /// repaint (`terminal.clear`) is guaranteed to erase it. See spec §11.3.
    scrolled: bool,
    /// Horizontal separators between rows of Markdown tables (setting
    /// `interface.table_row_separators`; the chat screen threads it from the settings
    /// snapshot via [`MessageFeed::set_table_row_separators`]).
    table_row_separators: bool,
    /// Render ```mermaid blocks as a diagram (setting `interface.render_mermaid`,
    /// threaded via [`MessageFeed::set_render_mermaid`]). On a render failure the
    /// block is printed as source (hard fallback, see `shared::markdown::mermaid`).
    render_mermaid: bool,
    /// Draw the model name next to the assistant's role header (setting
    /// `interface.show_model_name`, threaded via
    /// [`MessageFeed::set_show_model_name`]). Off by default. A bubble whose
    /// [`FeedMessage::model`] is `None` shows nothing either way. See spec §11.3.
    show_model_name: bool,
    /// The active chat profile's custom role names (threaded via
    /// [`MessageFeed::set_role_names`]). An unset field falls back to the localized
    /// header (`YOU`/`ASSISTANT`). See spec §11.3.
    role_names: CharacterNames,
    /// The history-compaction boundary: `(the id of the first message still sent
    /// verbatim, the rolling summary)`. `None` — nothing is folded, and the feed
    /// looks exactly as it did before the feature existed.
    ///
    /// Only the *request* is shortened — `chat.messages` is never edited, so the
    /// feed still shows every message; the boundary is drawn purely to explain
    /// why the model no longer quotes the early text (spec §6.7,
    /// docs/research/history-compression.md §6.5, fork F8c).
    compaction: Option<(Uuid, String)>,
    /// The conversations a `chat://` address in this feed may resolve to: the
    /// current profile's non-hidden chats, the open one included (spec §9.5,
    /// §11.3). Pushed by the screen from the chat-list snapshot it already
    /// keeps.
    ///
    /// It is an **address book, not decoration**: a reference outside it is
    /// drawn as ordinary text, so the user is never offered a door that opens
    /// onto nothing (docs/lessons.md §4). That makes the styling depend on it,
    /// which is why its fingerprint rides [`CacheKey`] — the same reason
    /// `role_names` does.
    known_chats: Vec<Uuid>,
    /// Where the `chat://` references of the **last drawn frame** sit on screen
    /// (spec §11.3). Rebuilt by every [`MessageFeed::render`] and read by
    /// [`MessageFeed::chat_link_at`], so a click is answered against the frame
    /// the user actually clicked on.
    link_hits: Vec<LinkHit>,
    /// Cache of rendered lines, one block per message (see [`CachedBlock`]).
    /// Index = message position. `build_lines` is called on every dirty frame
    /// (streaming, scrolling) and would re-run markdown+syntect over the WHOLE
    /// history each time; the cache recomputes only the changed messages (checked by fingerprint).
    cache: Vec<CachedBlock>,
    /// Cache key: changing the width/palette/thoughts display resets the whole cache.
    cache_key: Option<CacheKey>,
    /// A "put the view on this feed item" request, consumed by the next
    /// [`MessageFeed::render`]. Deferred by necessity: turning a feed index into
    /// a scroll row needs the block cache built at the current panel width, and
    /// neither exists outside `render` (docs/history/chat-search-stage2.md §1.1).
    pending_focus: Option<usize>,
    /// The feed item the **view position** is tied to, used to re-derive the row
    /// after a rewrap (resize, theme, `Ctrl+T`) invalidates every row offset —
    /// an index survives that, a row does not (§1.5, fork S6).
    ///
    /// Deliberately **not** the same field as [`Self::marker`]: this one is
    /// cleared by a manual scroll, because once the user has steered away a
    /// resize must not yank them back to the message.
    anchor: Option<usize>,
    /// The feed item drawn with the accent rail — "this is where you landed".
    ///
    /// **Survives a manual scroll** (that is the point: you scroll around the
    /// hit to read its context and can still see it), and is only dropped when
    /// the chat changes or a new jump replaces it. Being separate from
    /// [`Self::anchor`] is also what keeps it cheap — it is the only one of the
    /// two in [`CacheKey`], so scrolling no longer invalidates the block cache.
    marker: Option<usize>,
    /// The query whose matches are highlighted **inside the marked message** —
    /// the other half of a jump ("here is the message" / "here is your word in
    /// it", fork **S3(b)**).
    ///
    /// Moves as one with [`Self::marker`] (both are set by
    /// [`MessageFeed::focus_message`] and dropped by
    /// [`MessageFeed::clear_focus`]): a highlight without a marked message has
    /// nothing to highlight in, and a leftover query from a previous jump would
    /// light up the wrong message. A jump scopes it to that one message
    /// deliberately — lighting up the whole chat is noise for *that* gesture.
    highlight: Option<String>,
    /// How far [`Self::highlight`] reaches. A jump lights only the message it
    /// marked; in-feed search (`Ctrl+F`) lights every match in the chat, because
    /// there the whole point is seeing them all. See docs/history/in-feed-search.md §3.
    highlight_scope: HighlightScope,
    /// Every match of [`Self::highlight`] in document order, as an index into the
    /// lines [`MessageFeed::build_lines`] returned. Recomputed there each frame —
    /// never stored across frames, which is what lets a match survive a rewrap
    /// (§1.3: stage 2's fork S6 rejected *storing* an intra-block offset, not
    /// deriving one).
    matches: Vec<usize>,
    /// Which match `next`/`prev` last moved to, an index into [`Self::matches`].
    current_match: usize,
    /// Set by [`Self::next_match`]/[`Self::prev_match`]/[`Self::set_search`]: the
    /// next render scrolls to the current match. A flag rather than a scroll
    /// there and then, because the row is only knowable inside `render` — the
    /// same constraint the jump obeys (§1.1 of docs/history/chat-search-stage2.md).
    scroll_to_match: bool,
    /// [`MessageFeed::build_lines`] dropped the whole cache on this frame (the
    /// key changed), i.e. every row offset it had produced before is stale.
    /// Consumed by `render` to re-derive the anchored row. See [`CacheKey`].
    cache_reset: bool,
}

/// How far the feed's highlight reaches. See [`MessageFeed::highlight_scope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightScope {
    /// Only the message a jump marked — "here is your word in *this* message".
    #[default]
    MarkedOnly,
    /// Every match in the chat — in-feed search.
    WholeFeed,
}

/// Feed cache validity key. Any of these fields affects the layout of every
/// block, so changing it clears the cache. `Palette` — `Copy + Eq` (also the key in the syntect-theme cache).
#[derive(PartialEq)]
struct CacheKey {
    width: usize,
    palette: Palette,
    /// Collapsing either kind of block reshapes every message — hence the whole
    /// view state, not just the thoughts flag it grew out of.
    view: FeedView,
    table_row_separators: bool,
    render_mermaid: bool,
    /// The model name is baked into the assistant's cached header line, so
    /// toggling the setting has to rebuild every block.
    show_model_name: bool,
    /// Interface language (axis B): role headers/the "thoughts" pill/the placeholder
    /// depend on it — a language change clears the feed's block cache.
    lang: crate::shared::i18n::Lang,
    /// Custom role names: they're baked into the cached header lines, so editing
    /// them in settings has to clear the cache.
    role_names: CharacterNames,
    /// The marked feed item (a jump target): its rail is drawn in the accent
    /// color, and that color is baked into the cached block — so moving or
    /// clearing the marker has to reset the cache. Only the **marker** belongs
    /// here; the scroll anchor is not a rendering input.
    marker: Option<usize>,
    /// Fingerprint of the address book ([`MessageFeed::known_chats`]): which
    /// `chat://` references resolve — and so which ones are drawn as links — is
    /// baked into every cached block, so creating or deleting a conversation
    /// has to rebuild them. A hash rather than the list itself: the comparison
    /// runs every frame, and the list grows with the corpus.
    chat_book: u64,
    /// The compaction boundary and its summary: both are drawn into the boundary
    /// message's cached block, so a fresh compaction (or a `/compact` that only
    /// rewrites the summary) has to reset the cache — otherwise the divider
    /// would keep showing the previous roll. The collapse state needs no field
    /// of its own: the summary folds with the "thoughts" blocks, and `view` is
    /// already here (fork F8c).
    compaction: Option<(Uuid, String)>,
    //
    // The searched query is deliberately **absent**: it is applied *after* the
    // cache, so changing it costs no re-render. In-feed search types into a
    // field, and keying the cache on the query would re-run markdown + syntect
    // over the whole chat on every keystroke — up to 70 blocks and 260 K
    // characters on the real corpus (docs/history/in-feed-search.md §1.2).
}

/// One message's cached contribution to the feed (already width-wrapped lines
/// with a rail + a trailing separator) together with the source [`FeedMessage`]'s fingerprint.
struct CachedBlock {
    fingerprint: u64,
    lines: Vec<Line<'static>>,
    /// The conversations this block links to, in the order they are drawn —
    /// what the reference picker offers (spec §11.3). Recorded while the block
    /// is built, because that is where the addresses are recognised and where
    /// they are still whole: after the wrap they may be split across rows.
    links: Vec<Uuid>,
    /// How many of `lines` the role header occupies — the highlight pass skips
    /// them. Recorded here because the header's height is only known while the
    /// block is built: it is normally one line, but a long custom role name can
    /// wrap it, so counting output lines is the only stable answer.
    content_from: usize,
}

impl Default for MessageFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageFeed {
    pub fn new() -> Self {
        Self {
            scroll: 0,
            follow: true,
            view: FeedView::default(),
            scrolled: false,
            // Mirrors the config defaults (`InterfaceSettings::default`): before the
            // first settings snapshot arrives, the feed renders as the default config.
            table_row_separators: false,
            render_mermaid: true,
            show_model_name: false,
            role_names: CharacterNames::default(),
            compaction: None,
            known_chats: Vec::new(),
            link_hits: Vec::new(),
            cache: Vec::new(),
            cache_key: None,
            pending_focus: None,
            anchor: None,
            marker: None,
            highlight: None,
            highlight_scope: HighlightScope::default(),
            matches: Vec::new(),
            current_match: 0,
            scroll_to_match: false,
            cache_reset: false,
        }
    }

    /// Sets the active chat profile's custom role names (spec §5.1). An empty field
    /// means "not set" — the header falls back to the localized default. Changing
    /// the value invalidates the render cache via [`CacheKey`].
    pub fn set_role_names(&mut self, names: CharacterNames) {
        self.role_names = names;
    }

    /// Toggles horizontal separators between rows of Markdown tables
    /// (setting `interface.table_row_separators`). Changing the value invalidates
    /// the render cache via [`CacheKey`].
    pub fn set_table_row_separators(&mut self, on: bool) {
        self.table_row_separators = on;
    }

    /// Toggles rendering ```mermaid blocks as a diagram (setting
    /// `interface.render_mermaid`). Changing the value invalidates the cache via [`CacheKey`].
    pub fn set_render_mermaid(&mut self, on: bool) {
        self.render_mermaid = on;
    }

    /// Toggles the model name next to the assistant's role header (setting
    /// `interface.show_model_name`, spec §11.3). Changing the value invalidates
    /// the cache via [`CacheKey`].
    pub fn set_show_model_name(&mut self, on: bool) {
        self.show_model_name = on;
    }

    /// Sets (or clears) the history-compaction boundary: `(the id of the first
    /// message still sent verbatim, the rolling summary)`. Changing it
    /// invalidates the render cache via [`CacheKey`].
    ///
    /// An id this chat's feed doesn't contain draws **nothing** — the boundary
    /// is a domain message id, and the projection drops `Tool`/`System` messages
    /// entirely, so it can legitimately resolve to no block. Guessing a position
    /// would be worse than staying quiet: the divider's whole job is to say
    /// *where* verbatim history resumes.
    pub fn set_compaction(&mut self, compaction: Option<(Uuid, String)>) {
        self.compaction = compaction;
    }

    /// Sets the address book a `chat://` reference resolves against — the
    /// current profile's non-hidden chats, the open one included (spec §11.3).
    /// Changing it invalidates the render cache via [`CacheKey`], because which
    /// references are drawn as links is baked into every block.
    /// The address book as set — what `chat://` references resolve against.
    #[cfg(test)]
    pub fn known_chats(&self) -> &[Uuid] {
        &self.known_chats
    }

    pub fn set_known_chats(&mut self, ids: Vec<Uuid>) {
        self.known_chats = ids;
    }

    /// Fingerprint of the address book for [`CacheKey`]. Order-sensitive on
    /// purpose: the screen builds the list from one snapshot in one order, so a
    /// reordering means the snapshot changed.
    fn chat_book(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.known_chats.hash(&mut h);
        h.finish()
    }

    /// Every conversation this feed links to, newest block first and each one
    /// only once — what the reference picker offers (spec §11.3).
    ///
    /// Read from the block cache, so it is exactly what is drawn: a reference
    /// the feed did not style is not on this list, and one hidden inside a
    /// collapsed block is not either. The cache is built by `render`, so an
    /// unrendered feed answers with nothing rather than guessing.
    pub fn chat_links(&self) -> Vec<Uuid> {
        let mut seen: Vec<Uuid> = Vec::new();
        for block in self.cache.iter().rev() {
            for id in &block.links {
                if !seen.contains(id) {
                    seen.push(*id);
                }
            }
        }
        seen
    }

    /// The conversation a `chat://` reference at this terminal cell points at
    /// (spec §11.3) — `None` anywhere else, which is every cell of an ordinary
    /// feed.
    ///
    /// Answered against the **last drawn frame**: the map is rebuilt by
    /// `render`, so a click can never land on a stale layout. A feed that has
    /// not been drawn yet has no map and therefore no links, which is the
    /// honest answer rather than a guess.
    pub fn chat_link_at(&self, col: u16, row: u16) -> Option<Uuid> {
        self.link_hits
            .iter()
            .find(|h| h.row == row && (h.start..h.end).contains(&col))
            .map(|h| h.chat)
    }

    /// The feed block the compaction boundary falls on (`None` — nothing folded,
    /// or the boundary message isn't shown in this feed).
    ///
    /// Resolved the same way [`Self::focus_message`] resolves a jump: through
    /// [`FeedMessage::message_ids`], which is the projection's own id mapping —
    /// an agentic round's assistant messages merge many-to-one, so a boundary
    /// landing on a later round must still find its bubble.
    fn boundary_index(&self, messages: &[FeedMessage]) -> Option<usize> {
        let (id, _) = self.compaction.as_ref()?;
        messages.iter().position(|m| m.message_ids.contains(id))
    }

    /// Takes (and resets) the "scrolled by user" flag. The `app/runtime` loop,
    /// when `true`, does `terminal.clear()` before rendering — erases the artifacts
    /// of "drifted" VS16 emoji (see the [`MessageFeed::scrolled`] field).
    pub fn take_scrolled(&mut self) -> bool {
        std::mem::take(&mut self.scrolled)
    }

    /// Toggles showing "thoughts" blocks (`Ctrl+T`).
    pub fn toggle_thoughts(&mut self) {
        self.view.thoughts = !self.view.thoughts;
    }

    /// Toggles showing tool-call arguments and results (`Ctrl+O`). The card's
    /// header stays visible either way — see [`push_tool`].
    pub fn toggle_tools(&mut self) {
        self.view.tools = !self.view.tools;
    }

    /// The current collapse state — sent back to the orchestrator after a
    /// toggle, so it is stored on the chat (spec §11.3).
    pub fn view(&self) -> FeedView {
        self.view
    }

    /// Applies the chat's stored collapse state (on activation). [`CacheKey`]
    /// compares it by value, so switching to a chat that happens to agree costs
    /// no re-render.
    pub fn set_view(&mut self, view: FeedView) {
        self.view = view;
    }

    /// Scroll up (turns off "follow the tail").
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
        self.follow = false;
        self.scrolled = true;
        self.release_anchor();
    }

    /// Scroll down (turns "follow" back on at the very bottom).
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
        self.scrolled = true;
        self.release_anchor();
        // Actual clamping and re-enabling follow — in render (the height is known there).
    }

    /// Resets scroll to the bottom (on chat switch/send).
    ///
    /// **User-initiated only** — see [`Self::scroll_to_bottom_if_following`] for
    /// content that arrives on its own. The marker is deliberately **kept**: it
    /// says where you landed, not where you are looking, and clearing it here
    /// would also wipe the block cache on every send (see [`Self::marker`]).
    pub fn scroll_to_bottom(&mut self) {
        self.follow = true;
        self.release_anchor();
    }

    /// Scroll to the tail **only if the view is already following it**.
    ///
    /// For content that arrives on its own — a tool card, a service note, the
    /// next round's bubble. If the user has scrolled away to read, or jumped to
    /// a message, their position must not be yanked away
    /// (docs/history/chat-search-stage2.md §1.3, §4).
    pub fn scroll_to_bottom_if_following(&mut self) {
        if self.follow {
            self.scroll_to_bottom();
        }
    }

    /// Puts the view on the feed item holding the domain message `id`: the next
    /// render scrolls to it, and its rail is drawn in the accent color until the
    /// chat changes or another jump replaces it. Returns whether the message was
    /// found — an unknown/hidden id (a `Tool`/`System` message, or one from
    /// another chat) is a no-op.
    ///
    /// Resolving the id lives here, not on the caller: the feed **index** is
    /// this widget's coordinate system (`scroll`, `cache`, `anchor` and `marker`
    /// are all keyed by it), the messages are already handed to every other
    /// entry point (`render`/`build_lines`), and the projection's id mapping
    /// ([`FeedMessage::message_ids`]) is the widget's own contract.
    ///
    /// `highlight` — the search query whose matches are recolored inside that
    /// message ([`Self::highlight`]); `None`/empty for a jump with nothing to
    /// highlight. It is a **parameter rather than a separate setter** so it
    /// cannot get out of step with the marker: the two are one jump, and a
    /// query left over from a previous one would light up the wrong message.
    pub fn focus_message(
        &mut self,
        messages: &[FeedMessage],
        id: Uuid,
        highlight: Option<&str>,
    ) -> bool {
        let Some(idx) = messages.iter().position(|m| m.message_ids.contains(&id)) else {
            return false;
        };
        self.pending_focus = Some(idx);
        self.anchor = Some(idx);
        self.marker = Some(idx);
        self.highlight = highlight.filter(|q| !q.is_empty()).map(str::to_owned);
        true
    }

    /// Sets (or clears) the in-feed search query — `Ctrl+F`, as opposed to the
    /// query a jump carries. Highlights **every** match in the chat and rebuilds
    /// the match list on the next render, positioning on the first one.
    ///
    /// Cheap on purpose: stage 3a took the query out of [`CacheKey`], so typing
    /// here costs a normal warm frame rather than re-rendering the chat
    /// (docs/history/in-feed-search.md §1.2).
    pub fn set_search(&mut self, query: Option<&str>) {
        self.highlight = query.filter(|q| !q.is_empty()).map(str::to_owned);
        self.highlight_scope = HighlightScope::WholeFeed;
        self.current_match = 0;
        // Only chase a match once there is a query; clearing must not scroll.
        self.scroll_to_match = self.highlight.is_some();
    }

    /// Leaves in-feed search: drops its highlight and match list. The marker is
    /// left alone — a jump's mark is not this feature's to clear.
    pub fn clear_search(&mut self) {
        self.highlight = None;
        self.highlight_scope = HighlightScope::MarkedOnly;
        self.matches.clear();
        self.current_match = 0;
        self.scroll_to_match = false;
    }

    /// How many matches the last render found, and which one is current (1-based
    /// for display). `None` when there is nothing to count.
    pub fn match_position(&self) -> Option<(usize, usize)> {
        (!self.matches.is_empty()).then(|| (self.current_match + 1, self.matches.len()))
    }

    /// Moves to the next match, wrapping around at the end. A no-op without
    /// matches, so holding the key on a query that matches nothing is harmless.
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current_match = (self.current_match + 1) % self.matches.len();
        self.scroll_to_match = true;
    }

    /// Moves to the previous match, wrapping around at the start.
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current_match = (self.current_match + self.matches.len() - 1) % self.matches.len();
        self.scroll_to_match = true;
    }

    /// Drops the anchor, the marker **and the highlight**: the feed they index
    /// is gone (a chat switch). Cheap here — a rebuilt feed misses every
    /// fingerprint anyway, so the cache was going to be rebuilt regardless.
    pub fn clear_focus(&mut self) {
        self.pending_focus = None;
        self.anchor = None;
        self.marker = None;
        self.highlight = None;
    }

    /// Drops the scroll anchor, keeping the marker: the user is steering now, so
    /// a later rewrap must not pull the view back — but they can still see where
    /// they landed. See [`Self::anchor`] vs [`Self::marker`].
    fn release_anchor(&mut self) {
        self.pending_focus = None;
        self.anchor = None;
    }

    /// The feed index the view must be put on **this frame**: a pending jump
    /// (consumed), or — after a rewrap invalidated every row offset — the
    /// anchored item. Both can only become a row inside `render` (§1.1).
    fn take_focus_target(&mut self) -> Option<usize> {
        let rewrapped = std::mem::take(&mut self.cache_reset);
        if let Some(idx) = self.pending_focus.take() {
            return Some(idx);
        }
        // Following the tail needs no anchor — the clamp already keeps it there.
        if rewrapped && !self.follow {
            self.anchor
        } else {
            None
        }
    }

    /// Test accessor: whether the feed is following the tail (scrolled to bottom).
    #[cfg(test)]
    pub(crate) fn is_following(&self) -> bool {
        self.follow
    }

    /// Test accessor: the current scroll offset in visual rows.
    #[cfg(test)]
    pub(crate) fn scroll_row(&self) -> usize {
        self.scroll
    }

    /// Test accessor: the feed index the view position is anchored to.
    #[cfg(test)]
    pub(crate) fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// Test accessor: the feed index drawn with the accent rail.
    #[cfg(test)]
    pub(crate) fn marker(&self) -> Option<usize> {
        self.marker
    }

    /// Test accessor: the n-th drawn `chat://` reference as
    /// `(first column, row, the column just past it)`.
    #[cfg(test)]
    pub(crate) fn link_hit_for_test(&self, n: usize) -> Option<(u16, u16, u16)> {
        self.link_hits.get(n).map(|h| (h.start, h.row, h.end))
    }

    /// Test accessor: the query highlighted inside the marked message.
    #[cfg(test)]
    pub(crate) fn highlight(&self) -> Option<&str> {
        self.highlight.as_deref()
    }

    /// Draws the feed. `messages` — the active chat's current content. `meta` —
    /// the right-hand title caption (e.g. "gemma-4 · 16k ctx"; empty — don't show).
    // Title/meta/palette/locale — render context; bundling them into a struct for
    // the sake of one call from `chat/render.rs` isn't worth it.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        meta: &str,
        messages: &[FeedMessage],
        palette: &Palette,
        loc: &'static Locale,
    ) {
        // A rounded panel: title with a ◆ marker on the left, meta (model/ctx) on the right.
        let glyphs = palette.glyphs();
        let marker = format!(" {} ", glyphs.title_marker);
        let meta_cell = (!meta.is_empty()).then(|| format!(" {meta} "));
        // The border is a fixed row, so the title is cut to what the marker,
        // the meta and the corners leave — with the "…" that cut carries. A
        // title is not bounded in storage; every surface that draws one in a
        // row bounds it itself (`shared::title::sanitize_title`, spec §11.2).
        let budget = (area.width as usize).saturating_sub(
            2 + wrap::str_width(&marker) + 1 + meta_cell.as_deref().map_or(0, wrap::str_width),
        );
        let (title, _) = wrap::truncate_to_width(title, budget);
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(glyphs.border)
            .border_style(palette.border_style(false))
            .title(Line::from(vec![
                Span::styled(marker, palette.muted_style()),
                Span::styled(format!("{title} "), Style::new().fg(palette.text)),
            ]));
        if let Some(cell) = meta_cell {
            block =
                block.title(Line::from(Span::styled(cell, palette.muted_style())).right_aligned());
        }
        let inner = block.inner(area);
        frame.render_widget(&block, area);

        // Wrap lines to the feed's width ahead of time: this way the number of visual
        // rows matches `lines.len()`, and the scroll/"follow the tail" math
        // below stays row-based (see shared::wrap, ADR 0001).
        let view_w = inner.width.max(1) as usize;
        let unwrapped = self.build_lines(messages, palette, view_w, loc);

        // A jump (or an anchor whose rows a rewrap just invalidated) becomes a
        // scroll row here and nowhere else — the cache it is measured against
        // only exists after `build_lines` at this width (§1.1).
        //
        // `build_lines` concatenates `cache[i].lines` in order, so the per-block
        // lengths address the unwrapped stream exactly; the row offset is then
        // accumulated **through the wrap** rather than read off the cache. The
        // second pass is identity here by construction, not by contract — this
        // stays correct if that ever stops being true (§1.1).
        let target = self.take_focus_target();
        let start = target.and_then(|idx| {
            (idx < self.cache.len())
                .then(|| self.cache.iter().take(idx).map(|b| b.lines.len()).sum())
        });
        // The current in-feed match rides the same accumulation: its row is
        // derived here, every frame, never stored — which is what lets it survive
        // a rewrap where a stored offset could not (§1.3).
        let match_line = std::mem::take(&mut self.scroll_to_match)
            .then(|| self.matches.get(self.current_match).copied())
            .flatten();
        let mut lines: Vec<Line> = Vec::with_capacity(unwrapped.len());
        let mut target_row: Option<usize> = None;
        let mut match_row: Option<usize> = None;
        for (i, line) in unwrapped.iter().enumerate() {
            if start == Some(i) {
                target_row = Some(lines.len());
            }
            if match_line == Some(i) {
                match_row = Some(lines.len());
            }
            lines.extend(wrap::wrap_line(line, view_w));
        }
        // Applied after the jump target, so pressing next/prev wins over a stale
        // pending jump rather than fighting it.
        if let Some(row) = match_row {
            self.scroll = row;
            self.follow = false;
            self.scrolled = true;
        }
        if let Some(row) = target_row {
            self.scroll = row;
            self.follow = false;
            // A jump moves the view like a scroll — same terminal artifacts.
            self.scrolled = true;
        }

        let total = lines.len();
        let view_h = inner.height.max(1) as usize;
        let max_scroll = total.saturating_sub(view_h);

        if self.follow {
            self.scroll = max_scroll;
        } else if self.scroll >= max_scroll {
            // scrolled down to the bottom — follow the tail again
            self.scroll = max_scroll;
            self.follow = true;
        }

        // The click map is built here, from the rows about to be drawn: this is
        // the only point where the wrap, the scroll and the panel's origin have
        // all been applied (see [`visible_link_hits`]).
        self.link_hits = visible_link_hits(&lines, inner, self.scroll, &self.known_chats);

        let paragraph = Paragraph::new(Text::from(lines)).scroll((self.scroll as u16, 0));
        frame.render_widget(paragraph, inner);

        // Scrollbar on the panel's right border (corners untouched) — only when
        // the feed doesn't fit vertically. Doesn't take width away from content; the
        // feed's border is always non-focused (see `border_style(false)` above).
        render_scrollbar(
            frame,
            area.inner(Margin::new(0, 1)),
            total,
            view_h,
            self.scroll,
            false,
            palette,
        );
    }

    /// Builds the feed's lines: role headers, collapsed/expanded "thoughts",
    /// markdown-rendered body, separators. Every message line gets a colored
    /// gutter rail by role (see [`RAIL`]); content is built at width `width - 2`,
    /// then wrapped, and a rail is attached to each visual row.
    fn build_lines(
        &mut self,
        messages: &[FeedMessage],
        palette: &Palette,
        width: usize,
        loc: &'static Locale,
    ) -> Vec<Line<'static>> {
        if messages.is_empty() {
            // We don't cache the empty-feed placeholder.
            return vec![Line::from(Span::styled(
                loc.t("ui.feed.empty").to_string(),
                palette.muted_style(),
            ))];
        }
        // Reset the cache on a change to width/palette/collapse state/language (affects blocks).
        let key = CacheKey {
            width,
            palette: *palette,
            view: self.view,
            table_row_separators: self.table_row_separators,
            render_mermaid: self.render_mermaid,
            show_model_name: self.show_model_name,
            lang: loc.lang(),
            role_names: self.role_names.clone(),
            marker: self.marker,
            chat_book: self.chat_book(),
            compaction: self.compaction.clone(),
        };
        if self.cache_key.as_ref() != Some(&key) {
            self.cache.clear();
            self.cache_key = Some(key);
            // Every row offset the previous cache produced is stale — `render`
            // re-derives the anchored one from this flag.
            self.cache_reset = true;
        }
        // History truncated (Ctrl+E/regenerate) — drop the cache's tail.
        self.cache.truncate(messages.len());

        // Base markdown-render flags from the feed's settings; `soft_break_as_newline`
        // stays per-role (set by push_body for user messages).
        let opts = markdown::RenderOpts {
            table_row_separators: self.table_row_separators,
            render_mermaid: self.render_mermaid,
            ..Default::default()
        };
        // Resolved once per frame, before the loop: the id → block mapping is a
        // property of the whole projection, not of one block.
        let boundary = self.boundary_index(messages);
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut found: Vec<usize> = Vec::new();
        for (idx, item) in messages.iter().enumerate() {
            self.refresh_cached_block(idx, item, palette, width, opts, loc, boundary == Some(idx));
            // The block goes in query-free; the highlight is applied to the
            // *clones* below, which is what keeps it out of the cache.
            let at = lines.len();
            lines.extend(self.cache[idx].lines.iter().cloned());
            // Scoped to the marked message: the user asked "where is my word in
            // *this* message", and lighting up the whole chat would be noise.
            // A jump lights only the message it marked; in-feed search lights
            // every match (docs/history/in-feed-search.md §3, fork F2).
            if let Some(query) = self.highlight.as_deref()
                && (self.highlight_scope == HighlightScope::WholeFeed || self.marker == Some(idx))
            {
                let from = at + self.cache[idx].content_from;
                // `lines[from..]` is this block's tail: later blocks are not
                // appended yet, so the slice cannot reach them.
                highlight_block_tail(&mut lines, from, query, palette.accent, &mut found);
            }
        }
        self.matches = found;
        // The query may have just shrunk the match set under the cursor.
        if self.current_match >= self.matches.len() {
            self.current_match = 0;
        }
        lines
    }

    /// Recomputes message `idx`'s block in the render cache when its fingerprint
    /// no longer matches; an unchanged message keeps its cached block as-is.
    ///
    /// `at_boundary` — this block is where verbatim history resumes, so it is
    /// preceded by the compaction divider. Passed as a flag rather than as the
    /// summary itself so the caller doesn't have to hold a borrow of `self`
    /// across the `&mut self` call; the text is read here. The summary rides
    /// [`CacheKey`], so a changed one clears the cache and the block is rebuilt —
    /// the fingerprint (a property of the *message*) stays out of it.
    #[allow(clippy::too_many_arguments)]
    fn refresh_cached_block(
        &mut self,
        idx: usize,
        item: &FeedMessage,
        palette: &Palette,
        width: usize,
        opts: markdown::RenderOpts,
        loc: &'static Locale,
        at_boundary: bool,
    ) {
        let fp = message_fingerprint(item);
        let hit = self.cache.get(idx).is_some_and(|c| c.fingerprint == fp);
        if !hit {
            let summary = at_boundary
                .then(|| self.compaction.as_ref().map(|(_, s)| s.as_str()))
                .flatten();
            // A streaming/changed message — recompute only its block.
            let built = build_message_block(
                item,
                palette,
                width,
                self.view,
                opts,
                loc,
                &self.role_names,
                self.marker == Some(idx),
                summary,
                &self.known_chats,
                self.show_model_name,
            );
            let cb = CachedBlock {
                fingerprint: fp,
                lines: built.lines,
                links: built.links,
                content_from: built.content_from,
            };
            if idx < self.cache.len() {
                self.cache[idx] = cb;
            } else {
                self.cache.push(cb);
            }
        }
    }
}

/// Recolors `query` matches in `lines[from..]` — one block's tail — recording each
/// occurrence's row into `found`.
///
/// The rail sits in span 0 of every cached line and is passed
/// through untouched — deliberately *without* excluding it from
/// the match text. That was the obvious precaution, and measuring
/// showed it buys nothing: `highlight_line` derives its offsets
/// from the same concatenation it matches over, so including the
/// rail shifts both consistently and the output is identical. No
/// query can match the rail either — `match_ranges` drops tokens
/// under three characters, and `▌ ` is not in any of them. An
/// excluding parameter would be untestable by construction.
fn highlight_block_tail(
    lines: &mut [Line<'static>],
    from: usize,
    query: &str,
    accent: Color,
    found: &mut Vec<usize>,
) {
    for (i, line) in lines[from..].iter_mut().enumerate() {
        // One entry per occurrence, in document order: a single line
        // can hold several, and next/prev steps through matches, not
        // lines.
        for _ in 0..highlight_line(line, query, accent) {
            found.push(from + i);
        }
    }
}

/// Builds one message's contribution to the feed: width-wrapped lines with a colored
/// role rail + a trailing separator (if the body doesn't already end on a blank line).
/// A pure function of (`item`, `palette`, `width`, `view`, `opts`, `names`) —
/// the basis of the cache. `view` — which foldable blocks are expanded (both
/// kinds reshape the block, hence both are in [`CacheKey`]). `opts` — base
/// markdown-render flags (tables/mermaid from
/// settings; `soft_break_as_newline` is added on by `push_body` for the user).
/// `names` — the profile's custom role names (empty field → the localized header).
/// `marked` — this is the jump target ([`MessageFeed::focus_message`]): the rail
/// is drawn in the accent color to mark the whole message. `highlight` — the
/// query recolored inside it (fork S3(b), see [`highlight_line`]); `None` for
/// every message but the marked one. `compaction` — the rolling summary, when
/// this block is where verbatim history resumes: the divider is drawn **above**
/// the block (see [`push_compaction_boundary`]). `known_chats` — the address
/// book a `chat://` reference is resolved against ([`style_chat_links`]).
/// `show_model` — whether the assistant's header carries the model name
/// (`interface.show_model_name`, spec §11.3).
#[allow(clippy::too_many_arguments)]
fn build_message_block(
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    view: FeedView,
    opts: markdown::RenderOpts,
    loc: &'static Locale,
    names: &CharacterNames,
    marked: bool,
    compaction: Option<&str>,
    known_chats: &[Uuid],
    show_model: bool,
) -> BuiltBlock {
    // Content width under the rail (rail = 2 columns).
    let inner = width.saturating_sub(RAIL.chars().count()).max(1);
    let rail = if marked {
        palette.accent
    } else {
        match item.role {
            FeedRole::User => palette.user,
            FeedRole::Assistant => palette.assistant,
            FeedRole::System | FeedRole::Note => palette.muted,
        }
    };
    // Build the message body without a rail, at width `inner`.
    let mut body: Vec<Line<'static>> = Vec::new();
    // Where the message's own content starts, i.e. past the role header. The
    // header is excluded from the highlight because it is not indexed and would
    // fire on ordinary queries: `ASSISTANT` matches a search for "assistant" in
    // every marked assistant bubble, which reads as a bug.
    let mut content_from = 0usize;
    let glyphs = palette.glyphs();
    match item.role {
        FeedRole::User => {
            body.push(role_header(
                &format!(
                    "{} {}",
                    glyphs.user_icon,
                    role_name(names.user_name(), "ui.feed.role.user", loc)
                ),
                palette.user_soft,
                None,
                palette,
            ));
            content_from = body.len();
            push_body(&mut body, item, palette, inner, opts);
        }
        FeedRole::Assistant => {
            body.push(role_header(
                &format!(
                    "{} {}",
                    glyphs.assistant_icon,
                    role_name(names.assistant_name(), "ui.feed.role.assistant", loc)
                ),
                palette.assistant_soft,
                item.model.as_deref().filter(|_| show_model),
                palette,
            ));
            content_from = body.len();
            push_thoughts(&mut body, &item.thoughts, view.thoughts, palette, loc);
            push_assistant_body(&mut body, item, palette, inner, view.tools, opts, loc);
        }
        // The persona of a sub-agent transcript: headed like a role, body as
        // prose, on the muted rail a note uses — it is context, not a turn.
        FeedRole::System => {
            body.push(role_header(
                &format!(
                    "{} {}",
                    glyphs.system_icon,
                    role_name(names.system_name(), "ui.feed.role.system", loc)
                ),
                palette.muted,
                None,
                palette,
            ));
            content_from = body.len();
            push_body(&mut body, item, palette, inner, opts);
        }
        FeedRole::Note => push_body(&mut body, item, palette, inner, opts),
    }
    // If the body already ends on a blank line (a railed gap after a tool card),
    // don't add the railless inter-message separator — otherwise a double gap.
    let body_ends_blank = body
        .last()
        .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(false);
    // Wrap to the content width and attach a rail to every row, noting where the
    // header ends in **output** rows (the highlight pass runs post-cache and so
    // cannot use `content_from`, an index into the unwrapped `body`).
    let mut out: Vec<Line<'static>> = Vec::new();
    // The compaction divider sits **above** the block and belongs to neither
    // message, so it goes in railless and at the full panel width — like the
    // inter-message separator below. Being pushed before `content_row` is
    // measured also keeps it out of the highlight pass, deliberately: the
    // divider's label is chrome, and the summary is derived text no message
    // contains, so lighting it up would offer a match the search screen cannot
    // jump to.
    if let Some(summary) = compaction {
        push_compaction_boundary(&mut out, summary, view.thoughts, width, palette, loc);
    }
    let mut content_row = None;
    let mut links: Vec<Uuid> = Vec::new();
    for (i, mut line) in body.into_iter().enumerate() {
        if i == content_from {
            content_row = Some(out.len());
        }
        // **Before** the wrap, deliberately: an address is 15 columns
        // (`chat://` + 8 hex) and a narrow panel splits it across two rows,
        // where a per-row scan would no longer see it whole.
        links.extend(style_chat_links(&mut line, known_chats, palette));
        for wrapped in wrap::wrap_line(&line, inner) {
            out.push(prepend_rail(wrapped, rail));
        }
    }
    // A header-only block never reaches the index above.
    let content_from = content_row.unwrap_or(out.len());
    // Separator between messages — without a rail.
    if !body_ends_blank {
        out.push(Line::from(""));
    }
    links.dedup();
    BuiltBlock {
        lines: out,
        content_from,
        links,
    }
}

/// One freshly built block, before it enters the cache.
struct BuiltBlock {
    lines: Vec<Line<'static>>,
    /// See [`CachedBlock::content_from`].
    content_from: usize,
    /// See [`CachedBlock::links`].
    links: Vec<Uuid>,
}

/// Draws every **resolvable** `chat://` address in one unwrapped line in the
/// link style, and returns the conversations it points at, in order
/// (spec §11.3, docs/research/chat-uri-links.md §4.3).
///
/// Like the search highlight below, this matches **rendered** text rather than
/// the message source — the renderer destroys that correspondence (see
/// [`highlight_line`]). Unlike it, this runs *before* the wrap and therefore
/// before the cache, because whether a reference is a link is a property of the
/// content and the address book, not of a query the user is still typing.
///
/// Two consequences worth stating rather than discovering: an address inside a
/// code block is styled too (it is on screen and it does resolve), and an
/// address for a conversation that does not exist here is left as plain text.
fn style_chat_links(line: &mut Line<'static>, known: &[Uuid], palette: &Palette) -> Vec<Uuid> {
    let (text, found) = chat_links_in(line, known);
    if found.is_empty() {
        return Vec::new();
    }
    let ranges: Vec<std::ops::Range<usize>> = found.iter().map(|l| l.range.clone()).collect();
    restyle_ranges(line, &text, &ranges, |style| {
        style.fg(palette.accent).add_modifier(Modifier::UNDERLINED)
    });
    found.into_iter().map(|l| l.chat).collect()
}

/// One line's text (its spans concatenated) together with the resolvable
/// `chat://` addresses inside it. The single place a rendered line is asked the
/// question, so the styling pass and the click map cannot disagree about what
/// counts as a reference.
fn chat_links_in(
    line: &Line<'static>,
    known: &[Uuid],
) -> (String, Vec<crate::features::chat_links::ChatLink>) {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let found = crate::features::chat_links::find_refs(&text, known);
    (text, found)
}

/// Where a `chat://` reference sits **on screen**, and where it goes.
/// Coordinates are absolute terminal cells, so a click is answered by a
/// comparison and nothing else (spec §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LinkHit {
    row: u16,
    /// Half-open column range, `[start, end)`.
    start: u16,
    end: u16,
    chat: Uuid,
}

/// Builds the click map for the rows **actually on screen**, in absolute
/// terminal coordinates.
///
/// Derived from the drawn lines each frame rather than stored per block: it is
/// the one place where the second wrap, the scroll and the panel's origin have
/// all already been applied, so no assumption about them can go stale. The cost
/// is bounded by the viewport (tens of rows), not by the conversation.
///
/// A reference the wrap split across two rows yields **no** hit — it is styled
/// (that pass runs before the wrap) but not clickable, and `Ctrl+L` remains the
/// route that always works. Deliberate: half an address is not an address, and
/// guessing which half the user meant is worse than the keyboard.
fn visible_link_hits(
    lines: &[Line<'static>],
    inner: Rect,
    scroll: usize,
    known: &[Uuid],
) -> Vec<LinkHit> {
    if known.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (i, line) in lines
        .iter()
        .skip(scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        let (text, found) = chat_links_in(line, known);
        for link in found {
            let chars: Vec<char> = text[..link.range.start].chars().collect();
            let before = wrap::display_width(&chars) as u16;
            let width = wrap::display_width(&text[link.range.clone()].chars().collect::<Vec<_>>());
            let start = inner.x.saturating_add(before);
            hits.push(LinkHit {
                row: inner.y.saturating_add(i as u16),
                start,
                end: start.saturating_add(width as u16),
                chat: link.chat,
            });
        }
    }
    hits
}

/// Recolors every occurrence of `query` in one **rendered** line to `color`,
/// splitting spans at the match boundaries.
///
/// **Post-render matching** (fork **S3(b)**, docs/history/chat-search-stage2.md
/// §1.4 and §4a): the FTS5 index addresses `Message.text`, but the renderer
/// destroys that correspondence — `normalize_delimiters` rewrites the string
/// *before* parsing, and LaTeX→unicode, mermaid, table layout, syntect→ANSI and
/// two wrapping passes transform it further — so source offsets cannot be
/// mapped onto what is on screen. This therefore searches **what was actually
/// rendered**, which is also what the user is looking at.
///
/// Its limits follow from that, and are approximations rather than bugs (the
/// exact alternative, S3(c), means threading `highlight_ranges` through the
/// whole renderer):
/// - text the renderer **transformed** no longer contains the query and stays
///   unhighlighted — a LaTeX formula turned into unicode, a mermaid diagram
///   drawn in place of its source, a table re-laid-out across cells;
/// - a match **split across two rendered lines** (a line break inside it) is
///   missed, since matching is per line;
/// - conversely, an occurrence in content the index does not cover (thoughts, a
///   tool card) *is* highlighted — it is on screen and it is the user's word.
///
/// Only `fg` is patched: the span's markdown styling (bold, italic, code
/// coloring) has to survive, or highlighting a word inside a heading would
/// flatten the heading.
fn highlight_line(line: &mut Line<'static>, query: &str, color: Color) -> usize {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges = crate::features::chat_search::match_ranges(&text, query);
    if ranges.is_empty() {
        return 0;
    }
    restyle_ranges(line, &text, &ranges, |style| style.fg(color));
    ranges.len()
}

/// Re-splits a line's spans at `ranges`' boundaries and applies `patch` to the
/// pieces that fall inside one. `text` must be the line's spans concatenated —
/// the offsets are into it.
///
/// Shared by the two passes that recolor parts of an already-rendered line: the
/// search highlight ([`highlight_line`]) and the chat-link styling
/// ([`style_chat_links`]). Only the *patch* differs, and it is applied on top of
/// the span's own style so markdown formatting survives — recoloring a word
/// inside a heading must not flatten the heading.
fn restyle_ranges(
    line: &mut Line<'static>,
    text: &str,
    ranges: &[std::ops::Range<usize>],
    patch: impl Fn(Style) -> Style,
) {
    let spans = std::mem::take(&mut line.spans);
    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len() + ranges.len() * 2);
    // Byte offset of the current span within `text`.
    let mut at = 0usize;
    for span in spans {
        let (start, end) = (at, at + span.content.len());
        at = end;
        let cuts = span_cut_points(start, end, ranges);
        for w in cuts.windows(2) {
            let (s, e) = (w[0], w[1]);
            let inside = ranges.iter().any(|r| r.start <= s && e <= r.end);
            let style = if inside {
                patch(span.style)
            } else {
                span.style
            };
            // `s`/`e` are byte offsets into `text`. Both are character
            // boundaries — a match range is one by construction, and a span
            // starts on one — so this cannot split a character, which is the
            // trap that panics on the first Cyrillic match.
            out.push(Span::styled(text[s..e].to_string(), style));
        }
    }
    line.spans = out;
}

/// Cut points inside one span (`start..end`, byte offsets into the line's
/// text): every match boundary strictly within it, bracketed by the span's own
/// ends. Taken per span because a match may **straddle** two of them — bold in
/// the middle of a word, a link, an inline code span — and then both
/// halves have to be recolored. `ranges` is sorted and non-overlapping,
/// so the cuts come out ascending.
fn span_cut_points(start: usize, end: usize, ranges: &[std::ops::Range<usize>]) -> Vec<usize> {
    let mut cuts: Vec<usize> = vec![start];
    for r in ranges {
        for b in [r.start, r.end] {
            if b > start && b < end {
                cuts.push(b);
            }
        }
    }
    cuts.push(end);
    cuts.dedup();
    cuts
}

/// A message's fingerprint over every field that affects rendering. A hash O(len) vs.
/// a render O(len·markdown+syntect) — orders of magnitude cheaper; a streaming message
/// changes `text` with every chunk → the fingerprint mismatches → only it gets recomputed.
fn message_fingerprint(item: &FeedMessage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let role_tag: u8 = match item.role {
        FeedRole::User => 0,
        FeedRole::Assistant => 1,
        FeedRole::Note => 2,
        FeedRole::System => 3,
    };
    role_tag.hash(&mut h);
    item.text.hash(&mut h);
    item.thoughts.hash(&mut h);
    item.streaming.hash(&mut h);
    // Drawn into the header line, so a bubble that learns its model name (the
    // streaming one, on the next activation) has to be rebuilt.
    item.model.hash(&mut h);
    item.tools.len().hash(&mut h);
    for tc in &item.tools {
        tc.name.hash(&mut h);
        tc.arguments.hash(&mut h);
        tc.result.hash(&mut h);
        tc.text_offset.hash(&mut h);
        tc.running.hash(&mut h);
    }
    h.finish()
}

/// The header text for a role: the profile's custom name, or the localized default
/// under `key`. Feed headers are set in caps, so a custom name is uppercased too
/// (Unicode-aware, so non-Latin scripts are uppercased as well).
fn role_name(custom: Option<&str>, key: &str, loc: &'static Locale) -> String {
    match custom {
        Some(name) => name.to_uppercase(),
        None => loc.t(key).to_string(),
    }
}

/// A role-header line: icon + name in caps, colored with the role's "soft"
/// variant, optionally followed by `model` — the model that wrote the message
/// (`interface.show_model_name`, spec §11.3).
///
/// The name rides the header rather than a line of its own: it is an attribute
/// of the bubble, and a second line would cost a row in every assistant message.
/// Muted and unbolded, like the "thoughts" pill — it answers a question the
/// reader asks occasionally, so it must not compete with the role itself.
fn role_header(text: &str, color: Color, model: Option<&str>, palette: &Palette) -> Line<'static> {
    let mut spans = vec![Span::styled(
        text.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )];
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        spans.push(Span::styled(format!("  {model}"), palette.muted_style()));
    }
    Line::from(spans)
}

/// Prepends the colored gutter rail [`RAIL`] to a line, preserving the source line's
/// style and alignment.
///
/// The line-level style (`line.style`) is **folded into the content spans**, and
/// `out.style` is reset to default — otherwise line-level modifiers (e.g. `DIM`
/// on `───` dividers and table borders, see `shared::markdown`) would leak onto
/// the rail itself (its span only sets `fg`, not touching modifiers), making it
/// look a different color next to such lines.
fn prepend_rail(line: Line<'static>, rail: Color) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(RAIL.to_string(), Style::new().fg(rail)));
    // The span's final style = line.style.patch(span.style); we fold the same into
    // the span itself, so the rail doesn't depend on the line's style.
    for span in line.spans {
        let merged = line.style.patch(span.style);
        spans.push(Span::styled(span.content, merged));
    }
    let mut out = Line::from(spans);
    out.alignment = line.alignment;
    out
}

/// Adds the history-compaction boundary: a muted divider saying that everything
/// above is represented to the model by a summary rather than verbatim, carrying
/// that summary as a foldable block.
///
/// The summary folds with the CoT ("thoughts") blocks — same `Ctrl+T`, same
/// per-chat `FeedView.thoughts` state, no new hotkey and no new field (fork
/// **F8c**, docs/research/history-compression.md §6.5). Collapsed by default,
/// like every foldable block: collapsed the row ends on the same
/// `· keycap` pill the thoughts/tool cards use, expanded the keycap drops away
/// and the summary follows on a `│ ` gutter.
///
/// Lines go out **railless and at the full panel width**: the boundary belongs
/// between messages, not to one — the same reasoning as the inter-message
/// separator. They are not wrapped here; `render`'s second pass does that, as it
/// does for the thoughts block.
fn push_compaction_boundary(
    lines: &mut Vec<Line<'static>>,
    summary: &str,
    expanded: bool,
    width: usize,
    palette: &Palette,
    loc: &'static Locale,
) {
    let muted = palette.muted_style();
    let glyphs = palette.glyphs();
    let marker = if expanded {
        glyphs.expanded
    } else {
        glyphs.collapsed
    };
    let mut spans = vec![
        Span::styled(format!("{marker} "), muted),
        Span::styled(
            loc.t("ui.feed.compacted").to_string(),
            muted.add_modifier(Modifier::ITALIC),
        ),
    ];
    if !expanded {
        // The same ` · ` + keycap the collapsed thoughts and tool pills end on
        // (there the separator arrives inside `ui.feed.thoughts_lines`, which
        // this pill has no counterpart for — there is no count to show).
        spans.push(Span::styled(" · ", muted));
        spans.push(palette.keycap("Ctrl+T"));
    }
    // Fill out to the panel width with the rule glyph, so the row reads as a
    // divider rather than as a stray line of text — `rule()` in
    // `shared::markdown` stretches `---` the same way. A prefix that already
    // fills the width simply gets no fill; the outer wrap handles the overflow.
    let used: usize = spans
        .iter()
        .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
        .sum();
    if let Some(pad) = width.checked_sub(used + 1).filter(|p| *p > 0) {
        spans.push(Span::styled(format!(" {}", "─".repeat(pad)), muted));
    }
    lines.push(Line::from(spans));
    if !expanded || summary.is_empty() {
        return;
    }
    for t in summary.lines() {
        lines.push(Line::from(Span::styled(
            format!("│ {t}"),
            muted.add_modifier(Modifier::ITALIC),
        )));
    }
    // Keep the summary from gluing onto the role header of the message below.
    lines.push(Line::from(""));
}

/// Adds the "thoughts" block: collapsed — a "pill" with a line count and a key hint,
/// expanded — content on the `│` gutter.
fn push_thoughts(
    lines: &mut Vec<Line<'static>>,
    thoughts: &str,
    expanded: bool,
    palette: &Palette,
    loc: &'static Locale,
) {
    if thoughts.is_empty() {
        return;
    }
    let muted = palette.muted_style();
    let glyphs = palette.glyphs();
    if !expanded {
        let count = thoughts.lines().count();
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", glyphs.collapsed), muted),
            Span::styled(
                loc.t("ui.feed.thoughts").to_string(),
                muted.add_modifier(Modifier::ITALIC),
            ),
            Span::styled(
                loc.tf("ui.feed.thoughts_lines", &[("n", &count.to_string())]),
                muted,
            ),
            palette.keycap("Ctrl+T"),
        ]));
        return;
    }
    lines.push(Line::from(Span::styled(
        format!("{} {}", glyphs.expanded, loc.t("ui.feed.thoughts")),
        muted.add_modifier(Modifier::ITALIC),
    )));
    for t in thoughts.lines() {
        lines.push(Line::from(Span::styled(
            format!("│ {t}"),
            muted.add_modifier(Modifier::ITALIC),
        )));
    }
}

/// Adds the assistant's reply body with tool blocks **at the call sites**: a text
/// fragment before the call → tool block → the next fragment, etc. (see spec §11.3).
/// Each call's `text_offset` splits `item.text` into markdown fragments.
///
/// Tool blocks are separated from text (and from neighboring tool blocks) by a blank
/// line above and below, so they don't blend into the message; consecutive calls are
/// separated by exactly one blank line (`ensure_blank_line` collapses adjacent ones).
fn push_assistant_body(
    lines: &mut Vec<Line<'static>>,
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    tools_expanded: bool,
    opts: markdown::RenderOpts,
    loc: &'static Locale,
) {
    let text = item.text.as_str();
    let mut pos = 0usize;
    let mut produced = false;
    for tool in &item.tools {
        let off = clamp_boundary(text, tool.text_offset.min(text.len())).max(pos);
        if off > pos {
            push_markdown_fragment(lines, &text[pos..off], palette, width, opts);
        }
        // A blank line before the call (collapses if the previous one is already blank —
        // e.g. between two consecutive calls).
        ensure_blank_line(lines);
        push_tool(lines, tool, palette, width, tools_expanded, opts, loc);
        // A blank (railed) line AFTER the card — so the rail continues under
        // the result regardless of whether text/another call follows. Adjacent
        // `ensure_blank_line` calls collapse (a no-op before the next call/text),
        // and at the end of a message this gap replaces the inter-message separator
        // (see `build_lines`). Previously the gap after the last call only gave a
        // railless separator, and the rail cut off at the result — noticeable for
        // `python_exec`, whose result is often the round's finale.
        ensure_blank_line(lines);
        produced = true;
        pos = off;
    }
    if pos < text.len() && push_markdown_fragment(lines, &text[pos..], palette, width, opts) {
        produced = true;
    }
    // An empty streaming reply (no text or calls yet) — an "…" indicator.
    if !produced && item.streaming {
        lines.push(Line::from("…").dim());
    }
}

/// Appends a blank separator line if the last line isn't already blank.
/// This way adjacent separators (e.g. "after a call" + "before the next call")
/// collapse into one blank line. A no-op on an empty buffer (no leading blank).
fn ensure_blank_line(lines: &mut Vec<Line<'static>>) {
    let blank = lines
        .last()
        .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(true);
    if !blank {
        lines.push(Line::from(""));
    }
}

/// Renders a text fragment as markdown (if not empty). Returns whether it produced any lines.
fn push_markdown_fragment(
    lines: &mut Vec<Line<'static>>,
    fragment: &str,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) -> bool {
    if fragment.trim().is_empty() {
        return false;
    }
    let rendered = markdown::render_with(fragment, width, palette, opts);
    lines.extend(rendered.lines);
    true
}

/// A single tool block (a tool-call card): a header `⚒ name(suffix)` in the
/// tool's color, then argument and result blocks prepared by the presenter
/// [`present`] (highlighted code, console output, markdown prose, plain
/// text). Everything **wraps by width** (not truncated). See spec §11.3.
///
/// When `expanded` is false (the default, `Ctrl+O`) only the header is drawn,
/// with the collapsed pill's marker and keycap appended — so the feed still says
/// *what* ran, and the arguments/results are one keystroke away. The pill is
/// omitted when the presenter produced nothing to hide (a call with no arguments
/// and no result yet), mirroring [`push_thoughts`] on empty thoughts.
fn push_tool(
    lines: &mut Vec<Line<'static>>,
    tool: &FeedToolCall,
    palette: &Palette,
    width: usize,
    expanded: bool,
    opts: markdown::RenderOpts,
    loc: &'static Locale,
) {
    let head_style = Style::default()
        .fg(palette.tool_soft)
        .add_modifier(Modifier::BOLD);
    // Expanded, the reader has asked to see the call — so the request is shown
    // in full (the header alone truncates and cannot carry structured
    // arguments). Collapsed, the header *is* the summary. See spec §11.3.
    let detail = if expanded {
        present::ArgDetail::Full
    } else {
        present::ArgDetail::Compact
    };
    let p = present::present(&tool.name, &tool.arguments, &tool.result, detail);
    let header = match &p.header_suffix {
        Some(suffix) => format!("{}({suffix})", tool.name),
        None => tool.name.clone(),
    };
    // The first row carries the ⚒ icon (a width-2 emoji glyph — followed by two spaces so
    // it doesn't merge with the name), continuations align under the name. In
    // compatibility mode — an ASCII prefix of the same role (see GlyphSet::tool_head).
    let glyphs = palette.glyphs();
    push_wrapped(
        lines,
        glyphs.tool_head,
        glyphs.tool_cont,
        &header,
        width,
        head_style,
    );
    // An image the tool returned is in the prompt but nowhere on screen (spec §9.10):
    // the feed renders text, so without this chip the only visible trace would be
    // whatever the tool happened to say. Shown in both modes — it is not a detail that
    // `Ctrl+O` reveals, it is part of what the call did.
    let muted = palette.muted_style();
    // The call has not returned yet (`AppEvent::ToolCallStarted`, spec §11.3):
    // say so where the result will go, in both modes — a card with arguments
    // and no result would otherwise read as a call that answered nothing.
    if tool.running {
        let chip = Span::styled(
            format!("{} {}", glyphs.busy, loc.t("ui.feed.tool_running")),
            muted.add_modifier(Modifier::ITALIC),
        );
        push_trailing(lines, vec![chip], glyphs.tool_cont, width);
    }
    if tool.images > 0 {
        let chip = Span::styled(
            loc.tf("ui.feed.tool_images", &[("n", &tool.images.to_string())]),
            muted,
        );
        push_trailing(lines, vec![chip], glyphs.tool_cont, width);
    }
    if !expanded {
        // Nothing to reveal — no pill (an argument-less call whose result hasn't
        // arrived yet).
        if p.args.is_empty() && p.result.is_empty() {
            return;
        }
        // Appended to the header's **last** row rather than pushed as a line of
        // its own: one line per call collapsed, same as expanded — unless it
        // would not fit there, in which case the whole pill takes the next row
        // (`push_trailing`).
        let pill = vec![
            Span::styled(format!("{} ", glyphs.collapsed), muted),
            Span::styled(
                loc.t("ui.feed.tool_details").to_string(),
                muted.add_modifier(Modifier::ITALIC),
            ),
            // The same ` · ` the thoughts pill puts before its keycap (there it
            // arrives inside `ui.feed.thoughts_lines`, which this pill has no
            // counterpart for — there is no count to show).
            Span::styled(" · ", muted),
            palette.keycap("Ctrl+O"),
        ];
        push_trailing(lines, pill, glyphs.tool_cont, width);
        return;
    }
    for block in &p.args {
        push_block(lines, block, palette, width, false, opts, loc);
    }
    // A gap between the request and the answer: expanded, the argument list can
    // run for several rows, and without a break it reads as one wall with the
    // result. The `│` gutter **continues** through it — a bare blank row would
    // cut the card in two. Only when there is something on both sides of it.
    if !p.args.is_empty() && !p.result.is_empty() {
        lines.push(Line::from(Span::styled(
            "│".to_string(),
            Style::default().fg(palette.muted),
        )));
    }
    for block in &p.result {
        push_block(lines, block, palette, width, true, opts, loc);
    }
}

/// Draws one block of a tool card. `is_result` changes the leading gutter: the result
/// starts with `└ ` (a corner), an argument with `│ ` (a vertical bar, "nested under
/// the header"). The `│`/`└` gutter is WGL4-safe under compatibility mode too.
fn push_block(
    lines: &mut Vec<Line<'static>>,
    block: &ToolBlock,
    palette: &Palette,
    width: usize,
    is_result: bool,
    opts: markdown::RenderOpts,
    loc: &'static Locale,
) {
    let gutter_style = Style::default().fg(palette.muted);
    match block {
        ToolBlock::Plain(text) => {
            let (first, cont) = if is_result {
                ("└ ", "  ")
            } else {
                ("│ ", "│ ")
            };
            push_wrapped(lines, first, cont, text, width, gutter_style);
        }
        ToolBlock::Code { lang, text } => {
            let hl = markdown::highlight_code(text, lang, palette);
            push_gutter_lines(lines, hl, "│ ", "│ ", gutter_style, width);
        }
        ToolBlock::Markdown(text) => {
            let body_w = width.saturating_sub(2).max(1);
            let rendered = markdown::render_with(text, body_w, palette, opts);
            push_gutter_lines(lines, rendered.lines, "└ ", "  ", gutter_style, width);
        }
        ToolBlock::Console(c) => push_console(lines, c, palette, width, loc),
    }
}

/// Draws `python_exec`'s console output: stdout (text color), stderr (error
/// color), and the exit code (warning color) as separate sections. Each
/// section — a label on the `└ ` gutter and content on `│ `.
fn push_console(
    lines: &mut Vec<Line<'static>>,
    console: &present::Console,
    palette: &Palette,
    width: usize,
    loc: &'static Locale,
) {
    let label = Style::default().fg(palette.muted);
    let out_style = Style::default().fg(palette.text);
    let err_style = Style::default().fg(palette.error);
    if !console.stdout.trim().is_empty() {
        push_wrapped(lines, "└ ", "  ", "stdout", width, label);
        push_wrapped(lines, "│ ", "│ ", &console.stdout, width, out_style);
    }
    if !console.stderr.trim().is_empty() {
        push_wrapped(lines, "└ ", "  ", "stderr", width, err_style);
        push_wrapped(lines, "│ ", "│ ", &console.stderr, width, err_style);
    }
    if let Some(code) = console.exit {
        let warn = Style::default().fg(palette.warning);
        // The **interface** language (axis B), not the profile's: the tool
        // result carried the label in the agent's language and `parse_console`
        // stripped it (`present::exit_labels`), so what is drawn here is chrome
        // for the reader — hence `ui.feed.exit_code`, not `python.console.exit`.
        push_wrapped(
            lines,
            "└ ",
            "  ",
            &format!("{} {code}", loc.t("ui.feed.exit_code")),
            width,
            warn,
        );
    }
    if !console.files.trim().is_empty() {
        // Under the universal label, as `stdout` is drawn; the lines are the tool's own —
        // the folder and each file's name — so the user can find what the call saved
        // (docs/sandbox-file-exchange.md §11 S9).
        push_wrapped(lines, "└ ", "  ", "files", width, label);
        push_wrapped(lines, "│ ", "│ ", &console.files, width, out_style);
    }
}

/// Places ready (already styled) lines under gutter prefixes, wrapping
/// each by width. `first`/`cont` — prefixes for the first/subsequent rows. The
/// line-level style is folded into the content spans (as in [`prepend_rail`]), so
/// line-level modifiers (e.g. `DIM` on table borders) don't leak onto the gutter.
fn push_gutter_lines(
    lines: &mut Vec<Line<'static>>,
    src: Vec<Line<'static>>,
    first: &str,
    cont: &str,
    gutter_style: Style,
    width: usize,
) {
    let first_w = wrap::display_width(&first.chars().collect::<Vec<_>>());
    let cont_w = wrap::display_width(&cont.chars().collect::<Vec<_>>());
    let body_w = width.saturating_sub(first_w.max(cont_w)).max(1);
    let mut first_row = true;
    for line in src {
        let line_style = line.style;
        let align = line.alignment;
        for wrapped in wrap::wrap_line(&line, body_w) {
            let prefix = if first_row { first } else { cont };
            first_row = false;
            let mut spans = Vec::with_capacity(wrapped.spans.len() + 1);
            spans.push(Span::styled(prefix.to_string(), gutter_style));
            for span in wrapped.spans {
                spans.push(Span::styled(span.content, line_style.patch(span.style)));
            }
            let mut out = Line::from(spans);
            out.alignment = align;
            lines.push(out);
        }
    }
}

/// Wraps `text` by visual width with gutter prefixes and pushes into `lines`.
/// `first` — the prefix for the block's very first row, `cont` — for all subsequent rows
/// (both wrap continuations and new logical lines of the source). Each line of
/// `text` is wrapped separately, preserving line breaks.
fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    first: &str,
    cont: &str,
    text: &str,
    width: usize,
    style: Style,
) {
    let first_w = wrap::display_width(&first.chars().collect::<Vec<_>>());
    let cont_w = wrap::display_width(&cont.chars().collect::<Vec<_>>());
    let body_w = width.saturating_sub(first_w.max(cont_w)).max(1);
    let mut first_row = true;
    for src in text.split('\n') {
        let chars: Vec<char> = src.chars().collect();
        for (s, e) in wrap::wrap_ranges(&chars, body_w) {
            let prefix = if first_row { first } else { cont };
            first_row = false;
            let content: String = chars[s..e].iter().collect();
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), style),
                Span::styled(content, style),
            ]));
        }
    }
}

/// The gap between a row's text and a trailing group appended to it.
const TRAILING_GAP: &str = "  ";

/// Appends a trailing group of spans — the collapsed pill, the images chip — to
/// the last line: on the same row when it fits beside what is already there,
/// and otherwise **whole on a row of its own** under `cont` (the card's name
/// indent). The wrap that follows (`build_message_block`) breaks at spaces, so
/// a group appended regardless of room came apart — `▸ details ·` at the end of
/// one row and the keycap alone at the start of the next, under the icon —
/// which reads as a stray label rather than a pill. `width` is the wrap width,
/// so a fit measured here is a fit there.
fn push_trailing(
    lines: &mut Vec<Line<'static>>,
    group: Vec<Span<'static>>,
    cont: &str,
    width: usize,
) {
    let span_width =
        |spans: &[Span<'_>]| -> usize { spans.iter().map(|s| wrap::str_width(&s.content)).sum() };
    let group_w = span_width(&group);
    if let Some(last) = lines.last_mut()
        && span_width(&last.spans) + TRAILING_GAP.len() + group_w <= width
    {
        // The gap takes the group's own style — the head span's, so the
        // whole group reads as one decoration.
        let style = group.first().map(|s| s.style).unwrap_or_default();
        last.spans.push(Span::styled(TRAILING_GAP, style));
        last.spans.extend(group);
        return;
    }
    let mut spans = Vec::with_capacity(group.len() + 1);
    spans.push(Span::raw(cont.to_string()));
    spans.extend(group);
    lines.push(Line::from(spans));
}

/// The nearest (rounding down) valid char boundary for a byte offset.
fn clamp_boundary(text: &str, mut off: usize) -> usize {
    while off < text.len() && !text.is_char_boundary(off) {
        off += 1;
    }
    off.min(text.len())
}

/// Adds the message body: markdown for user, dim text for notes.
fn push_body(
    lines: &mut Vec<Line<'static>>,
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) {
    if item.text.is_empty() {
        if item.streaming {
            lines.push(Line::from("…").dim());
        }
        return;
    }
    match item.role {
        FeedRole::Note => {
            for line in item.text.split('\n') {
                lines.push(Line::from(Span::from(line.to_string()).dim()));
            }
        }
        _ => {
            // markdown → Text; copy lines into the shared buffer. For a user
            // message, single line breaks (Shift+Enter) are kept as real
            // line breaks (GFM style), otherwise the text would merge into one paragraph.
            let opts = markdown::RenderOpts {
                soft_break_as_newline: true,
                ..opts
            };
            let rendered = markdown::render_with(&item.text, width, palette, opts);
            lines.extend(rendered.lines);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// A feed with tool cards **expanded** — the arguments/results are collapsed
    /// by default (`Ctrl+O`), so every test that asserts on a card's contents
    /// has to ask for them.
    fn feed_with_tools() -> MessageFeed {
        let mut feed = MessageFeed::new();
        feed.toggle_tools();
        feed
    }

    fn msg(role: FeedRole, text: &str, thoughts: &str) -> FeedMessage {
        FeedMessage {
            role,
            text: text.to_string(),
            thoughts: thoughts.to_string(),
            tools: Vec::new(),
            streaming: false,
            message_ids: vec![Uuid::new_v4()],
            model: None,
        }
    }

    #[test]
    fn tool_blocks_render() {
        let mut feed = feed_with_tools();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{\"content\":\"x\"}".into(),
            result: "Заметка сохранена".into(),
            text_offset: m.text.len(),
            images: 0,
            call_id: None,
            running: false,
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 80, ru());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("⚒") && joined.contains("note_save"));
        assert!(joined.contains("Заметка сохранена"));
    }

    /// Every span of a rendered feed, joined — the shape most of these tests
    /// assert on.
    fn joined(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect()
    }

    #[test]
    fn tool_card_is_collapsed_by_default() {
        // The default (`Ctrl+O`, spec §11.3): the header says WHAT ran, the
        // arguments and result are folded away behind the pill.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            // Multi-line, so the presenter puts it in a *block* rather than the
            // header suffix — a short scalar argument stays in the header on
            // purpose, as the call's one-line summary.
            arguments: "{\"code\":\"a = 1\\nprint(a)\"}".into(),
            result: "stdout:\n1".into(),
            text_offset: m.text.len(),
            images: 0,
            call_id: None,
            running: false,
        });
        let out = joined(&feed.build_lines(&[m.clone()], &Palette::default(), 80, ru()));
        assert!(out.contains("python_exec"), "the header stays: {out}");
        assert!(!out.contains("print(a)"), "arguments hidden: {out}");
        assert!(!out.contains("stdout"), "result hidden: {out}");
        // The pill mirrors the thoughts one: marker + label + the key that opens it.
        let glyphs = Palette::default().glyphs();
        assert!(out.contains(glyphs.collapsed), "collapsed marker: {out}");
        // Label, then the same ` · ` the thoughts pill puts before its keycap,
        // then the key — the two pills have to read alike.
        assert!(out.contains("детали · "), "label, separator, keycap: {out}");
        assert!(out.contains("Ctrl+O"), "{out}");
        // Exactly one line per call — the pill rides the header, it isn't a
        // second line of its own.
        let card_rows = feed
            .build_lines(&[m], &Palette::default(), 80, ru())
            .iter()
            .filter(|l| joined(std::slice::from_ref(l)).contains("python_exec"))
            .count();
        assert_eq!(card_rows, 1, "one row per collapsed call");
    }

    /// A collapsed `web_search` with a long query — the header wraps, and how
    /// much of its last row is left decides where the pill goes.
    fn long_search_call() -> FeedMessage {
        let mut m = msg(FeedRole::Assistant, "", "");
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: "{\"query\":\"gpt-oss-120b vs GPT-4o comparison architecture benchmarks multimodal\"}"
                .into(),
            result: "results".into(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
        });
        m
    }

    /// The rows of a rendered feed as plain strings.
    fn rows(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| joined(std::slice::from_ref(l)))
            .collect()
    }

    #[test]
    fn collapsed_pill_moves_whole_to_the_next_row_when_it_does_not_fit() {
        // At 60 columns the header's last row is `architecture benchmarks
        // multimodal)` and the pill cannot follow it. It used to be appended
        // anyway and come apart in the wrap — `▸ details ·` at the end of that
        // row, `Ctrl+O` alone at the start of the next, under the icon.
        let mut feed = MessageFeed::new();
        let glyphs = Palette::default().glyphs();
        let out = rows(&feed.build_lines(&[long_search_call()], &Palette::default(), 60, ru()));
        let pill_row = out
            .iter()
            .find(|r| r.contains("Ctrl+O"))
            .unwrap_or_else(|| panic!("the pill is drawn: {out:#?}"));
        // Whole: the marker, the label and the keycap share one row...
        assert!(
            pill_row.contains(&format!("{} детали · ", glyphs.collapsed)),
            "the pill is one piece: {pill_row:?}"
        );
        // ...of its own, below the header, aligned under the name like a
        // wrapped header row — not under the `⚒` icon.
        assert!(
            !pill_row.contains("multimodal"),
            "a row of its own: {pill_row:?}"
        );
        assert!(
            pill_row.starts_with(&format!("{RAIL}{}{} ", glyphs.tool_cont, glyphs.collapsed)),
            "aligned under the name: {pill_row:?}"
        );
        // Nothing of the pill leaked onto the header's row.
        let header_row = out
            .iter()
            .find(|r| r.contains("multimodal"))
            .unwrap_or_else(|| panic!("the header's last row: {out:#?}"));
        assert!(
            !header_row.contains(glyphs.collapsed),
            "the header row carries no half of the pill: {header_row:?}"
        );
    }

    #[test]
    fn collapsed_pill_stays_on_the_header_row_when_it_fits() {
        // The same call at 80 columns leaves `multimodal)` alone on the last
        // row with room to spare — the pill rides it, one row per call.
        let mut feed = MessageFeed::new();
        let out = rows(&feed.build_lines(&[long_search_call()], &Palette::default(), 80, ru()));
        let pill_row = out
            .iter()
            .find(|r| r.contains("Ctrl+O"))
            .unwrap_or_else(|| panic!("the pill is drawn: {out:#?}"));
        assert!(
            pill_row.contains("multimodal)") && pill_row.contains("детали · "),
            "the pill follows the header on its row: {pill_row:?}"
        );
    }

    #[test]
    fn images_chip_follows_the_same_rule_as_the_pill() {
        // The chip is appended the same way, so it moves whole too; the pill
        // then measures against the chip's row and joins it when it fits.
        let mut feed = MessageFeed::new();
        let glyphs = Palette::default().glyphs();
        let mut m = long_search_call();
        m.tools[0].images = 3;
        let out = rows(&feed.build_lines(&[m], &Palette::default(), 60, ru()));
        let chip = ru().tf("ui.feed.tool_images", &[("n", "3")]);
        let chip_row = out
            .iter()
            .find(|r| r.contains(&chip))
            .unwrap_or_else(|| panic!("the chip is whole on one row: {out:#?}"));
        assert!(
            !chip_row.contains("multimodal"),
            "the chip did not fit the header row: {chip_row:?}"
        );
        assert!(
            chip_row.starts_with(&format!("{RAIL}{}{chip}", glyphs.tool_cont)),
            "aligned under the name: {chip_row:?}"
        );
        assert!(
            chip_row.contains("Ctrl+O"),
            "the pill fits beside the chip and shares its row: {chip_row:?}"
        );
    }

    #[test]
    fn toggle_tools_reveals_arguments_and_result() {
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{\"content\":\"x\"}".into(),
            result: "Заметка сохранена".into(),
            text_offset: m.text.len(),
            images: 0,
            call_id: None,
            running: false,
        });
        feed.toggle_tools();
        let out = joined(&feed.build_lines(&[m.clone()], &Palette::default(), 80, ru()));
        assert!(out.contains("Заметка сохранена"), "{out}");
        // Expanded, the `⚒` header is the marker — no pill, no keycap.
        assert!(!out.contains("Ctrl+O"), "no pill when expanded: {out}");
        // ...and back.
        feed.toggle_tools();
        let out = joined(&feed.build_lines(&[m], &Palette::default(), 80, ru()));
        assert!(!out.contains("Заметка сохранена"), "{out}");
    }

    #[test]
    fn expanding_a_card_shows_the_request_in_full() {
        // The header is a title — it truncates at 100 characters and cannot
        // carry a structured argument at all. Collapsed that is the summary the
        // reader asked for; expanded it would be a lie, so the rest is listed
        // below in full (spec §11.3).
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "", "");
        m.tools.push(FeedToolCall {
            name: "fetch_url".into(),
            arguments: concat!(
                r#"{"url":"https://example.org/a/very/long/path/that/goes/on/and/on/well/past/any/header","#,
                r#""focus":"memory management modes","headers":["a","b"]}"#
            )
            .into(),
            result: "ok".into(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
        });
        let collapsed = joined(&feed.build_lines(&[m.clone()], &Palette::default(), 100, ru()));
        assert!(collapsed.contains('…'), "collapsed truncates: {collapsed}");
        // What falls off is the tail of the header, and the header lists the
        // arguments in the tool's own order (`url` first — see FIELD_ORDER).
        assert!(
            !collapsed.contains("management modes"),
            "collapsed stays a summary: {collapsed}"
        );

        feed.toggle_tools();
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 100, ru()));
        let full = rows.join("");
        // Expanded: the header is the tool's name alone...
        let head = rows.iter().find(|r| r.contains("fetch_url")).unwrap();
        assert!(
            !head.contains('(') && !head.contains('…'),
            "the name alone: {head:?}"
        );
        // ...and every argument is listed below, whole.
        assert!(
            full.contains(
                "https://example.org/a/very/long/path/that/goes/on/and/on/well/past/any/header"
            ),
            "the whole URL: {full}"
        );
        assert!(
            full.contains(r#"headers: ["a","b"]"#),
            "the argument the header could not carry: {full}"
        );
        // ...separated from the result by exactly one gap row, which **keeps the
        // `│` gutter**: a bare blank row would cut the card in two.
        let i_result = rows.iter().position(|r| r.contains("└ ok")).unwrap();
        let gap = &rows[i_result - 1];
        assert!(!is_blank_row(gap), "the gutter continues: {gap:?}");
        assert!(
            gap.trim_end().ends_with('│'),
            "gutter only, nothing else on the row: {gap:?}"
        );
        // Exactly one: the row above the gap is already an argument.
        assert!(
            rows[i_result - 2].contains(": "),
            "one gap row, then the arguments: {:?}",
            &rows[i_result - 3..=i_result]
        );
    }

    #[test]
    fn a_card_with_no_arguments_gets_no_blank_row() {
        // The separator only exists to part two things; with nothing on one side
        // it would be a stray gap.
        let mut feed = feed_with_tools();
        let mut m = msg(FeedRole::Assistant, "", "");
        m.tools.push(FeedToolCall {
            name: "current_time".into(),
            arguments: "{}".into(),
            result: "12:00".into(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
        });
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 60, ru()));
        let i_head = rows
            .iter()
            .position(|r| r.contains("current_time"))
            .unwrap();
        assert!(
            rows[i_head + 1].contains("12:00"),
            "the result follows the header directly: {:?}",
            &rows[i_head..]
        );
    }

    #[test]
    fn collapsed_card_with_nothing_to_hide_has_no_pill() {
        // A call with no arguments whose result hasn't arrived yet: there is
        // nothing behind the pill, so promising one would be a lie (the same
        // rule `push_thoughts` follows for empty thoughts).
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "", "");
        m.tools.push(FeedToolCall {
            name: "current_time".into(),
            arguments: String::new(),
            result: String::new(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
        });
        let out = joined(&feed.build_lines(&[m], &Palette::default(), 80, ru()));
        assert!(out.contains("current_time"), "the header stays: {out}");
        assert!(
            !out.contains("Ctrl+O"),
            "nothing to reveal — no pill: {out}"
        );
    }

    #[test]
    fn collapse_pill_is_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let mut feed = MessageFeed::new();
            let mut m = msg(FeedRole::Assistant, "ok", "");
            m.tools.push(FeedToolCall {
                name: "note_save".into(),
                arguments: "{}".into(),
                result: "saved".into(),
                text_offset: m.text.len(),
                images: 0,
                call_id: None,
                running: false,
            });
            let out = joined(&feed.build_lines(&[m], &Palette::default(), 80, loc));
            assert!(
                out.contains(loc.t("ui.feed.tool_details")),
                "{lang:?}: {out}"
            );
        }
    }

    #[test]
    fn expanded_thoughts_and_exit_code_are_localized() {
        // Both labels used to be hardcoded Russian: an `en` interface showed
        // "мысли" above an expanded CoT block and "код возврата:" under a <!-- cyrillic-ok -->
        // python console. They follow the **interface** language (axis B) — the
        // tool result's own label was in the agent's language and the presenter
        // already stripped it.
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let mut feed = feed_with_tools();
            feed.toggle_thoughts();
            let mut m = msg(FeedRole::Assistant, "ok", "reasoning");
            m.tools.push(FeedToolCall {
                name: "python_exec".into(),
                arguments: r#"{"code":"import sys; sys.exit(3)"}"#.into(),
                result: format!("stderr:\nboom\n\n{} 3", loc.t("python.console.exit")),
                text_offset: m.text.len(),
                images: 0,
                call_id: None,
                running: false,
            });
            let out = joined(&feed.build_lines(&[m], &Palette::default(), 80, ru()));
            // Rendered under `ru()` above on purpose: the labels must follow the
            // locale handed to the feed, not the one the tool result was written
            // in — so an `en` result under a `ru` interface still reads Russian.
            assert!(out.contains(ru().t("ui.feed.thoughts")), "{lang:?}: {out}");
            assert!(
                out.contains(&format!("{} 3", ru().t("ui.feed.exit_code"))),
                "{lang:?}: {out}"
            );
        }
        // ...and the same feed under an `en` interface has no Russian in either.
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let mut feed = feed_with_tools();
        feed.toggle_thoughts();
        let mut m = msg(FeedRole::Assistant, "ok", "reasoning");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"x"}"#.into(),
            result: format!("stdout:\nhi\n\n{} 3", en.t("python.console.exit")),
            text_offset: m.text.len(),
            images: 0,
            call_id: None,
            running: false,
        });
        let out = joined(&feed.build_lines(&[m], &Palette::default(), 80, en));
        assert!(
            !out.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
            "Cyrillic leaked into an en feed: {out}"
        );
        assert!(
            out.contains("exit code: 3") && out.contains("thinking"),
            "{out}"
        );
    }

    #[test]
    fn role_headers_localized_for_all_langs() {
        // The feed's role headers (axis B) follow the interface language: under `en` — "YOU"/
        // "ASSISTANT", under `ru` — the Russian equivalents (no Cyrillic in English).
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let mut feed = MessageFeed::new();
            let msgs = vec![
                msg(FeedRole::User, "hi", ""),
                msg(FeedRole::Assistant, "ok", ""),
            ];
            let lines = feed.build_lines(&msgs, &Palette::default(), 80, loc);
            let joined: String = lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect();
            assert!(
                joined.contains(loc.t("ui.feed.role.user"))
                    && joined.contains(loc.t("ui.feed.role.assistant")),
                "{lang:?}: role headers not from the bundle"
            );
        }
        // Explicitly: en headers in Latin script.
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        assert_eq!(en.t("ui.feed.role.assistant"), "ASSISTANT");
        assert_eq!(en.t("ui.feed.role.user"), "YOU");
    }

    /// A custom role name from the profile replaces the localized header and is
    /// uppercased to match the feed's style; an unset field keeps its default.
    #[test]
    fn custom_role_names_replace_headers_in_caps() {
        let mut feed = MessageFeed::new();
        feed.set_role_names(CharacterNames {
            user: "Гайя".into(),
            assistant: "анна".into(),
            system: String::new(),
        });
        let msgs = vec![
            msg(FeedRole::User, "привет", ""),
            msg(FeedRole::Assistant, "здравствуй", ""),
        ];
        let joined = flatten(&feed.build_lines(&msgs, &Palette::default(), 80, ru()));
        assert!(
            joined.contains("ГАЙЯ") && joined.contains("АННА"),
            "{joined}"
        );
        assert!(!joined.contains("ВЫ") && !joined.contains("АССИСТЕНТ"));

        // Only the assistant named — the user keeps the localized header.
        let mut feed = MessageFeed::new();
        feed.set_role_names(CharacterNames {
            assistant: "Анна".into(),
            ..Default::default()
        });
        let joined = flatten(&feed.build_lines(&msgs, &Palette::default(), 80, ru()));
        assert!(joined.contains("ВЫ") && joined.contains("АННА"), "{joined}");
    }

    /// Names are baked into the cached header lines — changing them has to reset
    /// the block cache (otherwise a rename in settings wouldn't show up).
    #[test]
    fn changing_role_names_invalidates_cache() {
        let mut feed = MessageFeed::new();
        let msgs = vec![msg(FeedRole::User, "привет", "")];
        let before = flatten(&feed.build_lines(&msgs, &Palette::default(), 80, ru()));
        assert!(before.contains("ВЫ"));
        feed.set_role_names(CharacterNames {
            user: "Гайя".into(),
            ..Default::default()
        });
        let after = flatten(&feed.build_lines(&msgs, &Palette::default(), 80, ru()));
        assert!(after.contains("ГАЙЯ"), "{after}");
    }

    /// Every span carrying the link style, as text (spec §11.3).
    fn linked_spans(lines: &[Line<'static>], palette: &Palette) -> Vec<String> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| {
                s.style.fg == Some(palette.accent)
                    && s.style.add_modifier.contains(Modifier::UNDERLINED)
            })
            .map(|s| s.content.to_string())
            .collect()
    }

    fn chat_uuid(n: u128) -> Uuid {
        Uuid::from_u128(0x1a2b_3c4d_0000_4000_8000_0000_0000_0000 + n)
    }

    /// The address is drawn as a link, and the conversation it points at is
    /// offered to the picker.
    #[test]
    fn a_resolvable_chat_address_becomes_a_link() {
        let palette = Palette::default();
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        let text = format!("см. {}", crate::features::chat_links::uri(id));
        let msgs = vec![msg(FeedRole::Assistant, &text, "")];
        let lines = feed.build_lines(&msgs, &palette, 80, ru());

        assert_eq!(linked_spans(&lines, &palette), vec!["chat://1a2b3c4d"]);
        assert_eq!(feed.chat_links(), vec![id]);
    }

    /// The rule the whole design rests on: an address that goes nowhere is not
    /// offered as a door (docs/lessons.md §4).
    #[test]
    fn an_unknown_address_stays_plain_text() {
        let palette = Palette::default();
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![chat_uuid(1)]);
        let msgs = vec![msg(FeedRole::Assistant, "см. chat://deadbeef", "")];
        let lines = feed.build_lines(&msgs, &palette, 80, ru());

        assert!(linked_spans(&lines, &palette).is_empty());
        assert!(feed.chat_links().is_empty());
        // Still readable — nothing is swallowed.
        assert!(flatten(&lines).contains("chat://deadbeef"));
    }

    /// Styling before the wrap is the whole reason detection lives in the
    /// builder: at this width the address would be split across two rows, and a
    /// per-row scan would no longer see it (fork F3).
    #[test]
    fn an_address_survives_a_narrow_panel() {
        let palette = Palette::default();
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        let text = format!(
            "подробности лежат в {}",
            crate::features::chat_links::uri(id)
        );
        let msgs = vec![msg(FeedRole::Assistant, &text, "")];
        let lines = feed.build_lines(&msgs, &palette, 18, ru());

        let linked: String = linked_spans(&lines, &palette).concat();
        assert_eq!(linked, "chat://1a2b3c4d", "{:?}", flatten(&lines));
        assert_eq!(feed.chat_links(), vec![id]);
    }

    /// The markdown link form the model is taught to write: the URL suffix the
    /// renderer appends is the address, and the closing paren is not part of it.
    #[test]
    fn the_markdown_link_form_is_recognized() {
        let palette = Palette::default();
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        let text = format!("[Бюджет]({})", crate::features::chat_links::uri(id));
        let msgs = vec![msg(FeedRole::Assistant, &text, "")];
        let lines = feed.build_lines(&msgs, &palette, 80, ru());

        assert_eq!(linked_spans(&lines, &palette), vec!["chat://1a2b3c4d"]);
        assert!(flatten(&lines).contains("Бюджет"));
    }

    /// The picker's order is "the conversation you just read about first", and
    /// one conversation is offered once however often it is cited.
    #[test]
    fn links_are_deduped_and_newest_block_first() {
        let palette = Palette::default();
        // Distinct in their first eight hex characters — that prefix *is* the
        // address, and a shared one resolves to nothing by design.
        let (a, b) = (
            chat_uuid(1),
            Uuid::from_u128(0x9f8e_7d6c_0000_4000_8000_0000_0000_0002),
        );
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![a, b]);
        let msgs = vec![
            msg(
                FeedRole::Assistant,
                &format!(
                    "{} и снова {}",
                    crate::features::chat_links::uri(a),
                    crate::features::chat_links::uri(a)
                ),
                "",
            ),
            msg(
                FeedRole::Assistant,
                &crate::features::chat_links::uri(b),
                "",
            ),
        ];
        feed.build_lines(&msgs, &palette, 80, ru());

        assert_eq!(feed.chat_links(), vec![b, a]);
    }

    /// The address book is a rendering input, so changing it has to rebuild the
    /// blocks — the `role_names` precedent (fork F3).
    #[test]
    fn changing_the_address_book_invalidates_cache() {
        let palette = Palette::default();
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        let text = format!("см. {}", crate::features::chat_links::uri(id));
        let msgs = vec![msg(FeedRole::Assistant, &text, "")];

        feed.build_lines(&msgs, &palette, 80, ru());
        assert!(feed.chat_links().is_empty(), "nothing known yet");

        feed.set_known_chats(vec![id]);
        feed.build_lines(&msgs, &palette, 80, ru());
        assert_eq!(feed.chat_links(), vec![id], "the book must reach the cache");
    }

    /// Draws the feed into a test terminal so the click map exists, and returns
    /// the cell coordinates the address occupies on screen.
    fn render_and_find_link(feed: &mut MessageFeed, msgs: &[FeedMessage], w: u16, h: u16) -> Rect {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            feed.render(f, f.area(), "Чат", "", msgs, &Palette::default(), ru());
        })
        .unwrap();
        let hit = feed.link_hits.first().copied().expect("a drawn reference");
        Rect::new(hit.start, hit.row, hit.end - hit.start, 1)
    }

    /// A click on the address follows it; the cells on either side of it do not.
    #[test]
    fn a_click_on_the_address_resolves_and_its_neighbours_do_not() {
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        let msgs = vec![msg(
            FeedRole::Assistant,
            &format!("см. {} дальше", crate::features::chat_links::uri(id)),
            "",
        )];
        let at = render_and_find_link(&mut feed, &msgs, 60, 12);

        assert_eq!(feed.chat_link_at(at.x, at.y), Some(id), "first cell");
        assert_eq!(
            feed.chat_link_at(at.x + at.width - 1, at.y),
            Some(id),
            "last cell"
        );
        // Half-open range: the cell past the end is prose again.
        assert_eq!(feed.chat_link_at(at.x + at.width, at.y), None);
        assert_eq!(feed.chat_link_at(at.x.saturating_sub(1), at.y), None);
        assert_eq!(feed.chat_link_at(at.x, at.y + 1), None, "another row");
    }

    /// The map is in absolute terminal cells, so the panel's border and the
    /// rail must already be accounted for — an off-by-two here would make every
    /// click land two columns to the left of what the user sees.
    #[test]
    fn the_click_map_is_in_absolute_screen_cells() {
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        let uri = crate::features::chat_links::uri(id);
        let msgs = vec![msg(FeedRole::Assistant, &uri, "")];
        let at = render_and_find_link(&mut feed, &msgs, 60, 12);

        // Panel border (1) + rail ("▌ ", 2 columns) — the address starts there.
        assert_eq!(at.x, 3, "border + rail");
        assert_eq!(at.width as usize, uri.chars().count());
    }

    /// An unrendered feed has no map, and answering "no link" is the honest
    /// answer rather than a guess against a layout that does not exist.
    #[test]
    fn an_undrawn_feed_has_no_clickable_links() {
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        feed.build_lines(
            &[msg(
                FeedRole::Assistant,
                &crate::features::chat_links::uri(id),
                "",
            )],
            &Palette::default(),
            80,
            ru(),
        );
        assert_eq!(feed.chat_link_at(3, 1), None);
    }

    /// Scrolling moves the map with the content: the click map is rebuilt from
    /// the rows actually drawn, so a reference scrolled out of view stops being
    /// clickable and one scrolled into view starts.
    #[test]
    fn the_map_follows_the_scroll() {
        let id = chat_uuid(1);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        let mut msgs = vec![msg(
            FeedRole::Assistant,
            &crate::features::chat_links::uri(id),
            "",
        )];
        msgs.extend((0..30).map(|i| msg(FeedRole::User, &format!("реплика-{i}"), "")));

        // The tail is what a fresh feed shows, and the address is far above it.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 10)).unwrap();
        term.draw(|f| {
            feed.render(f, f.area(), "Чат", "", &msgs, &Palette::default(), ru());
        })
        .unwrap();
        assert!(
            feed.link_hits.is_empty(),
            "a reference off-screen is not clickable"
        );

        feed.scroll_up(usize::MAX);
        term.draw(|f| {
            feed.render(f, f.area(), "Чат", "", &msgs, &Palette::default(), ru());
        })
        .unwrap();
        let hit = feed.link_hits.first().copied().expect("scrolled into view");
        assert_eq!(feed.chat_link_at(hit.start, hit.row), Some(id));
    }

    /// Concatenates the rendered lines' text (a helper for header assertions).
    fn flatten(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect()
    }

    #[test]
    fn compat_palette_renders_without_emoji() {
        // Compatibility mode: role headers, collapsed "thoughts", and the tool card
        // are drawn with safe glyphs — no emoji/rare characters in the feed.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "думал");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{}".into(),
            result: "ок".into(),
            text_offset: m.text.len(),
            images: 0,
            call_id: None,
            running: false,
        });
        let user = msg(FeedRole::User, "привет", "");
        let compat = Palette::default().with_compat(true);
        let lines = feed.build_lines(&[user, m], &compat, 80, ru());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("* АССИСТЕНТ") && joined.contains("> ВЫ"));
        assert!(
            joined.contains("# note_save"),
            "ASCII tool prefix: {joined}"
        );
        assert!(joined.contains("► мысли"), "compat thoughts pill: {joined}");
        for banned in ['✦', '❯', '⚒', '▸'] {
            assert!(!joined.contains(banned), "{banned} left behind: {joined}");
        }
    }

    #[test]
    fn python_tool_renders_highlighted_code_and_console() {
        // python_exec: the argument code — as a highlighted block (RGB colors), the result —
        // as a stdout console section with content.
        let mut feed = feed_with_tools();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"print(42)"}"#.into(),
            result: "stdout:\n42".into(),
            text_offset: m.text.len(),
            images: 0,
            call_id: None,
            running: false,
        });
        let palette = Palette::for_theme(crate::shared::config::Theme::Dark);
        let lines = feed.build_lines(&[m], &palette, 80, ru());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // The header carries no raw JSON, code and console are present.
        assert!(joined.contains("python_exec"), "{joined}");
        assert!(!joined.contains("{\"code\""), "raw JSON must not be shown");
        assert!(joined.contains("print(42)"), "code: {joined}");
        assert!(
            joined.contains("stdout") && joined.contains("42"),
            "console: {joined}"
        );
        // Highlighting assigned an RGB color to at least one code span.
        let has_rgb = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.content.contains("print") && matches!(s.style.fg, Some(Color::Rgb(..))));
        assert!(has_rgb, "expected a highlighted (RGB) code span");
    }

    #[test]
    fn python_stderr_uses_error_color() {
        let mut feed = feed_with_tools();
        let palette = Palette::default();
        let mut m = msg(FeedRole::Assistant, "", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"raise SystemExit(1)"}"#.into(),
            result: "stderr:\nTraceback here\n\nкод возврата: 1".into(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
        });
        let lines = feed.build_lines(&[m], &palette, 80, ru());
        // The line with the stderr text is colored the error color.
        let err_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("Traceback here")));
        let err_line = err_line.expect("stderr line");
        assert!(
            err_line
                .spans
                .iter()
                .any(|s| s.content.contains("Traceback") && s.style.fg == Some(palette.error)),
            "stderr must be the error color"
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("код возврата: 1"), "{joined}");
    }

    #[test]
    fn tool_call_renders_after_preceding_text_inline() {
        // Text before the call → tool block → text after the call: check the line order.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "Ищу погоду.\n\nГотово: ясно.", "");
        let off = "Ищу погоду.".len();
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: "{}".into(),
            result: "ясно".into(),
            text_offset: off,
            images: 0,
            call_id: None,
            running: false,
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 80, ru());
        let rows: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let idx_before = rows.iter().position(|r| r.contains("Ищу погоду")).unwrap();
        let idx_tool = rows.iter().position(|r| r.contains("web_search")).unwrap();
        let idx_after = rows.iter().position(|r| r.contains("Готово")).unwrap();
        assert!(
            idx_before < idx_tool && idx_tool < idx_after,
            "expected order: text-before < call < text-after, got {idx_before}/{idx_tool}/{idx_after}"
        );
    }

    /// Helper: render lines into a list of "does the line have non-blank content" —
    /// handy for finding blank separator lines.
    fn row_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Whether a visual row is blank once the gutter rail (`▌` + spaces) is stripped.
    fn is_blank_row(r: &str) -> bool {
        r.trim_matches(|c| c == '▌' || c == ' ').is_empty()
    }

    #[test]
    fn tool_block_separated_from_text_by_blank_lines() {
        // text-before → call → text-after: there should be blank lines around the call.
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "доПОСЛЕ", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: String::new(),
            result: String::new(),
            text_offset: "до".len(),
            images: 0,
            call_id: None,
            running: false,
        });
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 80, ru()));
        let i_before = rows.iter().position(|r| r.contains("до")).unwrap();
        let i_tool = rows.iter().position(|r| r.contains("note_save")).unwrap();
        let i_after = rows.iter().position(|r| r.contains("ПОСЛЕ")).unwrap();
        // Between text-before and the call — exactly one blank line (railed/guttered).
        assert!(rows[i_before + 1..i_tool].iter().all(|r| is_blank_row(r)));
        assert_eq!(
            i_tool - i_before,
            2,
            "expected one blank line before the call"
        );
        // Between the call and text-after — exactly one blank line.
        assert_eq!(
            i_after - i_tool,
            2,
            "expected one blank line after the call"
        );
    }

    #[test]
    fn tool_last_in_message_keeps_railed_trailing_blank() {
        // The call is the message's last element (the result = the round's finale, typical for
        // python_exec). A RAILED gap should follow the card (the rail continues
        // under the result), not a railless inter-message separator, and exactly
        // one (no double gap).
        let mut feed = feed_with_tools();
        let mut m = msg(FeedRole::Assistant, "Считаю.", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"print(1)"}"#.into(),
            result: "stdout:\n1".into(),
            text_offset: "Считаю.".len(),
            images: 0,
            call_id: None,
            running: false,
        });
        let next = msg(FeedRole::User, "дальше", "");
        let rows = row_texts(&feed.build_lines(&[m, next], &Palette::default(), 60, ru()));
        // From the **end**: the argument listing shows the code too (`print(1)`),
        // so a forward search for "1" finds the request, not the result.
        let i_result = rows.iter().rposition(|r| r.contains('1')).unwrap();
        let i_next = rows.iter().position(|r| r.contains("дальше")).unwrap();
        // Between the result and the next message's header — exactly one blank
        // line, and it carries a rail (not a railless separator `[]`).
        let between: Vec<&String> = rows[i_result + 1..i_next].iter().collect();
        let blanks = between.iter().filter(|r| is_blank_row(r)).count();
        assert_eq!(blanks, 1, "expected one blank line: {between:?}");
        assert!(
            rows[i_next - 1].contains('▌'),
            "the gap after the card should carry a rail: {:?}",
            rows[i_next - 1]
        );
    }

    #[test]
    fn consecutive_tool_blocks_have_single_blank_between() {
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "", "");
        for name in ["first_tool", "second_tool"] {
            m.tools.push(FeedToolCall {
                name: name.into(),
                arguments: String::new(),
                result: String::new(),
                text_offset: 0,
                images: 0,
                call_id: None,
                running: false,
            });
        }
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 80, ru()));
        let i1 = rows.iter().position(|r| r.contains("first_tool")).unwrap();
        let i2 = rows.iter().position(|r| r.contains("second_tool")).unwrap();
        // Between two consecutive calls — exactly one blank line.
        assert_eq!(i2 - i1, 2, "expected one blank line between adjacent calls");
        assert!(rows[i1 + 1..i2].iter().all(|r| is_blank_row(r)));
    }

    #[test]
    fn long_tool_result_wraps_not_truncated() {
        let mut feed = feed_with_tools();
        let mut m = msg(FeedRole::Assistant, "ок", "");
        let long = "слово ".repeat(40); // ~240 characters — definitely wider than the narrow feed
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: String::new(),
            result: long.trim_end().into(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 30, ru());
        // The result isn't truncated (no "…") and is laid out across several rows (rail + gutter
        // `└`/continuation indent).
        let gutter_rows = lines
            .iter()
            .filter(|l| {
                let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
                s.contains("слово")
            })
            .count();
        assert!(
            gutter_rows > 1,
            "a long result should wrap, rows: {gutter_rows}"
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains('…'),
            "the result must not be truncated with an ellipsis"
        );
    }

    #[test]
    fn followup_starts_new_bubble_and_hides_control_tool() {
        use crate::entities::message::{Message, MessageRole, ToolCallRecord};

        // A1 (with a send_followup_message call) → tool → A2 (new_bubble).
        let mut a1 = Message::assistant("Первое сообщение.");
        a1.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            images: 0,
            id: "c1".into(),
            name: "send_followup_message".into(),
            arguments: serde_json::json!({}),
            result: Some("ок".into()),
            subagent: None,
        }];
        let tool_msg = {
            let mut m = Message::new(MessageRole::Tool, "ок");
            m.tool_call_id = Some("c1".into());
            m.tool_name = Some("send_followup_message".into());
            m
        };
        let mut a2 = Message::assistant("Второе сообщение.");
        a2.new_bubble = true;

        let feed = FeedMessage::from_messages(&[a1, tool_msg, a2]);
        // Two separate assistant bubbles (not merged).
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].text, "Первое сообщение.");
        assert_eq!(feed[1].text, "Второе сообщение.");
        // A control tool's service call isn't shown in the feed.
        assert!(
            feed[0].tools.is_empty(),
            "a control call should not produce a tool block"
        );
    }

    #[test]
    fn assistant_rounds_without_new_bubble_still_merge() {
        use crate::entities::message::{Message, MessageRole, ToolCallRecord};
        // A regular agentic round (note_save) still merges into one bubble.
        let mut a1 = Message::assistant("Ищу.");
        a1.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            images: 0,
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({}),
            result: Some("ok".into()),
            subagent: None,
        }];
        let tool_msg = {
            let mut m = Message::new(MessageRole::Tool, "ok");
            m.tool_call_id = Some("c1".into());
            m
        };
        let a2 = Message::assistant("Готово.");
        let feed = FeedMessage::from_messages(&[a1, tool_msg, a2]);
        assert_eq!(feed.len(), 1, "regular rounds merge");
        assert_eq!(feed[0].tools.len(), 1, "a regular tool block is visible");
    }

    /// The chat list's counter promises this projection's bubble count
    /// (`entities::chat::visible_message_count`, spec §11.2). FSD keeps the
    /// two in different layers, so this is the one place the two statements
    /// of the rule are held together — at every prefix of a history that
    /// exercises every branch: a greeting (leading assistant), an agentic
    /// exchange with a tool row between rounds, a `new_bubble` followup, a
    /// system row, the next question.
    #[test]
    fn bubble_count_agrees_with_the_list_counter() {
        use crate::entities::chat::visible_message_count;
        use crate::entities::message::{Message, MessageRole};

        let mut followup = Message::assistant("Ещё одно.");
        followup.new_bubble = true;
        let messages = [
            Message::assistant("Приветствие."),
            Message::user("вопрос"),
            Message::assistant(""),
            Message::new(MessageRole::Tool, "результат"),
            Message::assistant("Ответ."),
            followup,
            Message::new(MessageRole::System, "инструкция"),
            Message::user("ещё вопрос"),
        ];
        for upto in 0..=messages.len() {
            assert_eq!(
                FeedMessage::from_messages(&messages[..upto]).len(),
                visible_message_count(&messages[..upto]),
                "the list and the feed disagree at prefix {upto}"
            );
        }
    }

    /// A `System` entry in a message list is a dialogue transcript's director
    /// intervention (spec §9.13) and draws as a note row; tool messages still
    /// draw nothing of their own.
    #[test]
    fn from_message_projects_system_as_a_note() {
        let sys = Message::new(MessageRole::System, "Director: wrap up");
        let note = FeedMessage::from_message(&sys).unwrap();
        assert_eq!(note.role, FeedRole::Note);
        assert_eq!(note.text, "Director: wrap up");
        assert!(FeedMessage::from_message(&Message::new(MessageRole::Tool, "r")).is_none());
        let user = Message::user("hi");
        assert_eq!(
            FeedMessage::from_message(&user).unwrap().role,
            FeedRole::User
        );
    }

    #[test]
    fn collapsed_thoughts_show_indicator_not_content() {
        let mut feed = MessageFeed::new(); // show_thoughts = false
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "ответ", "секрет\nмысль")],
            &Palette::default(),
            80,
            ru(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("▸ мысли"));
        assert!(!joined.contains("секрет"));
    }

    #[test]
    fn expanded_thoughts_show_content() {
        let mut feed = MessageFeed::new();
        feed.toggle_thoughts();
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "ответ", "секрет")],
            &Palette::default(),
            80,
            ru(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("секрет"));
    }

    #[test]
    fn user_message_preserves_single_newlines() {
        // A user message with Shift+Enter (a single \n) should preserve
        // line breaks, not merge into one paragraph (GFM style).
        let mut feed = MessageFeed::new();
        let lines = feed.build_lines(
            &[msg(FeedRole::User, "Привет!\nКак дела?", "")],
            &Palette::default(),
            80,
            ru(),
        );
        let rows = row_texts(&lines);
        let i_first = rows.iter().position(|r| r.contains("Привет!")).unwrap();
        let i_second = rows.iter().position(|r| r.contains("Как дела?")).unwrap();
        assert_ne!(
            i_first, i_second,
            "the two lines should end up on different visual rows"
        );
        // And no line should contain both phrases (not merged into one).
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("Привет!") && r.contains("Как дела?")),
            "lines must not merge into a single row"
        );
    }

    #[test]
    fn markdown_is_applied_to_body() {
        let mut feed = MessageFeed::new();
        // LaTeX applies inside $…$ (delimiter-scoped, see shared::markdown)
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "формула $x^2$", "")],
            &Palette::default(),
            80,
            ru(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("x²"));
    }

    #[test]
    fn rail_is_not_dimmed_next_to_table_borders() {
        // The left rail should have a clean role color with no line-level modifiers
        // (e.g. DIM on table borders/dividers), otherwise it's a different color next to them.
        let mut feed = MessageFeed::new();
        let table = "| a | b |\n|---|---|\n| 1 | 2 |";
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, table, "")],
            &Palette::default(),
            80,
            ru(),
        );
        for line in &lines {
            // The rail — the first span with the "▌" character.
            if let Some(rail) = line.spans.first()
                && rail.content.starts_with('▌')
            {
                assert!(
                    !rail.style.add_modifier.contains(Modifier::DIM),
                    "the rail must not inherit DIM from a table/divider line"
                );
            }
        }
    }

    #[test]
    fn table_row_separators_follow_setting_and_invalidate_cache() {
        // By default (mirroring the config default) there are no row separators —
        // only under the header; enabling the setting adds them, and changing the value
        // resets the cache (same message/width/palette — only the flag changes).
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let table = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
        let mids = |lines: &[Line<'static>]| {
            lines
                .iter()
                .filter(|l| l.spans.iter().any(|s| s.content.contains('├')))
                .count()
        };
        let off = feed.build_lines(&[msg(FeedRole::Assistant, table, "")], &palette, 80, ru());
        assert_eq!(mids(&off), 1, "by default: only under the header");
        feed.set_table_row_separators(true);
        let on = feed.build_lines(&[msg(FeedRole::Assistant, table, "")], &palette, 80, ru());
        assert_eq!(
            mids(&on),
            2,
            "enabled: header separator + one row separator (cache reset by key)"
        );
    }

    #[test]
    fn render_mermaid_follows_setting_and_invalidates_cache() {
        // By default (mirroring the config default) a mermaid block renders as a diagram;
        // turning the setting off reverts to the source, and changing the value resets the
        // cache (same message/width/palette — only the flag changes).
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let md = "```mermaid\nflowchart LR\n    A[Start] --> B[End]\n```";
        let joined = |lines: &[Line<'static>]| -> String {
            lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect()
        };
        let on = feed.build_lines(&[msg(FeedRole::Assistant, md, "")], &palette, 90, ru());
        assert!(
            joined(&on).contains('┌') && !joined(&on).contains("```"),
            "on by default — a diagram, not the source"
        );
        feed.set_render_mermaid(false);
        let off = feed.build_lines(&[msg(FeedRole::Assistant, md, "")], &palette, 90, ru());
        assert!(
            joined(&off).contains("```mermaid"),
            "disabled — source (cache reset by key)"
        );
    }

    /// The model name rides the assistant's header only when the setting asks
    /// for it, and toggling it rebuilds the blocks (it is baked into the cached
    /// header line). Spec §11.3.
    #[test]
    fn model_name_follows_setting_and_invalidates_cache() {
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let mut item = msg(FeedRole::Assistant, "ответ", "");
        item.model = Some("gemma-4-31b".into());
        let joined = |lines: &[Line<'static>]| -> String {
            lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect()
        };

        let off = feed.build_lines(std::slice::from_ref(&item), &palette, 80, ru());
        assert!(
            !joined(&off).contains("gemma-4-31b"),
            "off by default — the header names only the role"
        );

        feed.set_show_model_name(true);
        let on = feed.build_lines(std::slice::from_ref(&item), &palette, 80, ru());
        let header = on
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .any(|s| s.content.contains(ru().t("ui.feed.role.assistant")))
            })
            .expect("the assistant header row");
        let text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("gemma-4-31b"),
            "enabled — the name sits on the header row itself (cache reset by key)"
        );
        // Muted and unbolded, like the "thoughts" pill: the role stays the
        // prominent half of the row.
        let name = header
            .spans
            .iter()
            .find(|s| s.content.contains("gemma-4-31b"))
            .unwrap();
        assert_eq!(name.style.fg, Some(palette.muted));
        assert!(!name.style.add_modifier.contains(Modifier::BOLD));
    }

    /// With the setting on, a message that carries no model name draws exactly
    /// what it drew before the feature existed — which is every message stored
    /// before the metadata snapshot, and every user message.
    #[test]
    fn a_message_without_a_model_name_is_unchanged_by_the_setting() {
        let palette = Palette::default();
        let item = msg(FeedRole::Assistant, "ответ", "");
        let mut off = MessageFeed::new();
        let mut on = MessageFeed::new();
        on.set_show_model_name(true);
        let rows = |feed: &mut MessageFeed| -> Vec<String> {
            row_texts(&feed.build_lines(std::slice::from_ref(&item), &palette, 80, ru()))
        };
        assert_eq!(rows(&mut off), rows(&mut on));
    }

    /// The name comes from the message's own metadata, and a stitched bubble
    /// keeps the first round's answer — one bubble, one header, one model.
    #[test]
    fn the_model_name_is_read_from_metadata_and_survives_round_stitching() {
        use crate::entities::message::{MessageMetadata, ToolCallRecord};

        let meta = |model: &str| MessageMetadata {
            sampling: Default::default(),
            mode: Default::default(),
            model: Some(model.to_string()),
            finish: None,
        };
        let mut r1 = Message::assistant("ищу");
        r1.metadata = Some(meta("gemma-4-31b"));
        r1.tool_calls = vec![ToolCallRecord {
            id: "c1".into(),
            name: "web_search".into(),
            arguments: serde_json::json!({}),
            result: Some("ок".into()),
            thought_signature: None,
            images: 0,
            subagent: None,
        }];
        let tool = {
            let mut m = Message::new(MessageRole::Tool, "ок");
            m.tool_call_id = Some("c1".into());
            m
        };
        let mut r2 = Message::assistant("нашёл");
        r2.metadata = Some(meta("gemma-4-31b"));

        let feed = FeedMessage::from_messages(&[r1, tool, r2]);
        assert_eq!(feed.len(), 1, "the rounds stitch into one bubble");
        assert_eq!(feed[0].model.as_deref(), Some("gemma-4-31b"));

        // A user message and a message stored before the snapshot existed carry
        // no name at all.
        let plain = FeedMessage::from_messages(&[
            Message::user("привет"),
            Message::assistant("здравствуйте"),
        ]);
        assert!(plain.iter().all(|m| m.model.is_none()));
    }

    #[test]
    fn streamed_mermaid_stays_source_until_fence_closes() {
        // Simulating streaming: until the closing fence arrives, the block shows
        // as source (not a flickering diagram stub); once the fence arrives,
        // the message's fingerprint changes, the cache recomputes the block → a diagram.
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let joined = |lines: &[Line<'static>]| -> String {
            lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect()
        };
        // Chunk 1: unclosed but syntactically valid stub.
        let partial = "```mermaid\nflowchart LR\n    A[Start] --> B[End]";
        let mid = feed.build_lines(&[msg(FeedRole::Assistant, partial, "")], &palette, 90, ru());
        assert!(
            joined(&mid).contains("```mermaid") && !joined(&mid).contains('┌'),
            "during streaming — source, not a diagram: {}",
            joined(&mid)
        );
        // Chunk 2: the closing fence arrived — same message, text extended.
        let full = "```mermaid\nflowchart LR\n    A[Start] --> B[End]\n```";
        let done = feed.build_lines(&[msg(FeedRole::Assistant, full, "")], &palette, 90, ru());
        assert!(
            joined(&done).contains('┌') && !joined(&done).contains("```"),
            "once the fence completes — a diagram: {}",
            joined(&done)
        );
    }

    // ---------- jump to a message (docs/history/chat-search-stage2.md, stage 2a) ----------

    /// Renders the feed into a `TestBackend` and returns the visible text rows.
    /// A jump is only applied inside `render` (§1.1), so every focus assertion
    /// has to go through a real frame.
    fn visible(feed: &mut MessageFeed, messages: &[FeedMessage], w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| feed.render(f, f.area(), "Чат", "", messages, &Palette::default(), ru()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| buf[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// A feed of `n` user messages, each with a unique id and a findable body.
    fn numbered(n: usize) -> Vec<FeedMessage> {
        (0..n)
            .map(|i| msg(FeedRole::User, &format!("сообщение-{i}"), ""))
            .collect()
    }

    /// Long bodies, so a width change really does re-wrap them.
    fn numbered_long(n: usize) -> Vec<FeedMessage> {
        (0..n)
            .map(|i| {
                msg(
                    FeedRole::User,
                    &format!("сообщение-{i} с довольно длинным текстом который переносится"),
                    "",
                )
            })
            .collect()
    }

    /// The mapping a jump resolves against: every domain message records its id,
    /// and a merged agentic bubble carries **all** of its rounds' ids — the
    /// property `message_ids` is a `Vec` for (§1.2).
    #[test]
    fn from_messages_records_ids_including_every_merged_round() {
        use crate::entities::message::{Message, MessageRole, ToolCallRecord};
        let user = Message::user("вопрос");
        let mut r1 = Message::assistant("Ищу.");
        r1.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            images: 0,
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({}),
            result: Some("ok".into()),
            subagent: None,
        }];
        let tool = {
            let mut m = Message::new(MessageRole::Tool, "ok");
            m.tool_call_id = Some("c1".into());
            m
        };
        let r2 = Message::assistant("Готово.");
        let (uid, id1, id2) = (user.id, r1.id, r2.id);

        let feed = FeedMessage::from_messages(&[user, r1, tool, r2]);
        assert_eq!(feed.len(), 2, "the two rounds merge into one bubble");
        assert_eq!(feed[0].message_ids, vec![uid]);
        assert_eq!(
            feed[1].message_ids,
            vec![id1, id2],
            "a merged bubble must carry every round's id, or a jump to the \
             second round would find nothing"
        );
    }

    /// Focusing scrolls the message into view; the **first** message is already
    /// at row 0, a later one is not (so the scroll really moved).
    #[test]
    fn focus_scrolls_the_message_into_view() {
        let messages = numbered(12);
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, messages[9].message_ids[0], None));
        let rows = visible(&mut feed, &messages, 40, 10);
        assert!(
            rows.iter().any(|r| r.contains("сообщение-9")),
            "the focused message must be visible: {rows:?}"
        );
        assert!(feed.scroll_row() > 0, "a later message moves the scroll");
        assert!(!feed.is_following(), "a jump turns off tail-following");

        // The first message is at the top — focusing it leaves scroll at 0.
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, messages[0].message_ids[0], None));
        let rows = visible(&mut feed, &messages, 40, 10);
        assert!(rows.iter().any(|r| r.contains("сообщение-0")), "{rows:?}");
        assert_eq!(feed.scroll_row(), 0);
    }

    /// In-feed search lights **every** match, unlike a jump (fork F2), and the
    /// counter must equal what is highlighted — if they can drift, the counter is
    /// a lie. Asserted together for exactly that reason.
    #[test]
    fn search_highlights_every_match_and_counts_them() {
        let palette = Palette::default();
        let messages = vec![
            msg(FeedRole::User, "первое про маркер", ""),
            msg(FeedRole::Assistant, "второе про маркер и снова маркер", ""),
        ];
        let mut feed = MessageFeed::new();
        feed.set_search(Some("маркер"));
        let lines = feed.build_lines(&messages, &palette, 60, ru());

        let hits = accented(&lines, &palette);
        assert_eq!(hits, vec!["маркер"; 3], "every occurrence must light up");
        assert_eq!(
            feed.match_position(),
            Some((1, hits.len())),
            "the counter must equal the highlights, and start on the first"
        );
    }

    /// **The reason next/prev resolves a line rather than a message** (§1.3): the
    /// largest real message is 38,782 characters, so with message-granular
    /// jumping "next" would leave the viewport where it was. Asserted on the
    /// scroll row moving, not on the match index.
    #[test]
    fn next_match_moves_the_viewport_inside_one_long_message() {
        // One message, matches at the very start and the very end.
        let body = format!("маркер{}маркер", "заполнение ".repeat(400));
        let messages = vec![msg(FeedRole::Assistant, &body, "")];
        let mut feed = MessageFeed::new();
        feed.set_search(Some("маркер"));
        let _ = visible(&mut feed, &messages, 40, 10);
        assert_eq!(feed.match_position(), Some((1, 2)));
        let first = feed.scroll_row();

        feed.next_match();
        let _ = visible(&mut feed, &messages, 40, 10);
        assert_eq!(feed.match_position(), Some((2, 2)));
        assert!(
            feed.scroll_row() > first,
            "next must move the view inside one long message: {} -> {}",
            first,
            feed.scroll_row()
        );
    }

    #[test]
    fn next_and_prev_wrap_around() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::User, "маркер раз маркер два", "")];
        let mut feed = MessageFeed::new();
        feed.set_search(Some("маркер"));
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        assert_eq!(feed.match_position(), Some((1, 2)));
        feed.next_match();
        assert_eq!(feed.match_position(), Some((2, 2)));
        feed.next_match();
        assert_eq!(feed.match_position(), Some((1, 2)), "wraps past the end");
        feed.prev_match();
        assert_eq!(feed.match_position(), Some((2, 2)), "wraps past the start");
    }

    /// Navigating a query that matches nothing must not panic or move anything —
    /// holding the key on a typo is the ordinary case.
    #[test]
    fn navigating_without_matches_is_a_no_op() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::User, "ничего похожего", "")];
        let mut feed = MessageFeed::new();
        feed.set_search(Some("маркер"));
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        assert_eq!(feed.match_position(), None);
        feed.next_match();
        feed.prev_match();
        assert_eq!(feed.match_position(), None);
    }

    /// Leaving search must not clear a jump's mark: the two share the highlight
    /// slot but the mark is not this feature's to drop.
    #[test]
    fn clear_search_drops_the_highlight_but_keeps_the_marker() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::User, "про маркер", "")];
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, messages[0].message_ids[0], None));
        feed.set_search(Some("маркер"));
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        assert!(feed.match_position().is_some());

        feed.clear_search();
        let lines = feed.build_lines(&messages, &palette, 60, ru());
        assert!(accented(&lines, &palette).is_empty(), "highlight gone");
        assert_eq!(feed.match_position(), None, "match list gone");
        assert_eq!(feed.marker(), Some(0), "the jump's mark stays");
    }

    /// Stage 3a's payoff, from this feature's side: typing costs a warm frame.
    #[test]
    fn typing_a_search_query_does_not_invalidate_the_cache() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::User, "маркер и метка рядом", "")];
        let mut feed = MessageFeed::new();
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        feed.cache_reset = false;
        for q in ["мар", "марк", "маркер"] {
            feed.set_search(Some(q));
            let _ = feed.build_lines(&messages, &palette, 60, ru());
            assert!(!feed.cache_reset, "query {q:?} must not rebuild a block");
        }
    }

    /// A rewrap wipes the cache and changes every row offset, so the anchor is
    /// the message index, not a row (§1.5, fork S6). Asserted as "the message is
    /// still in view", not as a row number — the row legitimately changes.
    #[test]
    fn focus_survives_a_resize() {
        let messages = numbered_long(12);
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, messages[8].message_ids[0], None));

        let wide = visible(&mut feed, &messages, 60, 10);
        assert!(wide.iter().any(|r| r.contains("сообщение-8")), "{wide:?}");
        let row_wide = feed.scroll_row();

        let narrow = visible(&mut feed, &messages, 30, 10);
        assert!(
            narrow.iter().any(|r| r.contains("сообщение-8")),
            "after a resize the anchored message must still be in view: {narrow:?}"
        );
        assert_ne!(
            row_wide,
            feed.scroll_row(),
            "the rewrap should have moved the row — otherwise the test proves nothing"
        );
    }

    /// The split (anchor vs marker): a manual scroll means "I am steering now",
    /// so the **anchor** goes — but the **marker** stays, because the point of
    /// marking the message is that you can scroll around it to read its context
    /// and still see where you landed.
    #[test]
    fn manual_scroll_releases_the_anchor_but_keeps_the_marker() {
        let messages = numbered(12);
        for scroll in [
            (|f: &mut MessageFeed| f.scroll_up(2)) as fn(&mut MessageFeed),
            |f: &mut MessageFeed| f.scroll_down(2),
        ] {
            let mut feed = MessageFeed::new();
            feed.focus_message(&messages, messages[9].message_ids[0], None);
            let _ = visible(&mut feed, &messages, 40, 10);
            assert_eq!(feed.anchor(), Some(9));
            assert_eq!(feed.marker(), Some(9));

            scroll(&mut feed);
            assert_eq!(feed.anchor(), None, "a manual scroll releases the anchor");
            assert_eq!(feed.marker(), Some(9), "but the marker stays put");
            // And the marker is still drawn after the next frame.
            let _ = visible(&mut feed, &messages, 40, 10);
            assert_eq!(feed.marker(), Some(9));
        }
    }

    /// The other half of the split: once the anchor is released, a rewrap must
    /// **not** pull the view back to the message. (Before the split this was the
    /// same field, so a resize after scrolling away yanked you back.)
    #[test]
    fn resize_after_a_manual_scroll_does_not_jump_back() {
        let messages = numbered_long(12);
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[8].message_ids[0], None);
        let _ = visible(&mut feed, &messages, 60, 10);

        // Scroll all the way up, deliberately away from the jump target.
        feed.scroll_up(1000);
        let before = visible(&mut feed, &messages, 60, 10);
        assert!(
            before.iter().any(|r| r.contains("сообщение-0")),
            "precondition: at the top of the feed: {before:?}"
        );

        let after = visible(&mut feed, &messages, 30, 10);
        assert!(
            after.iter().any(|r| r.contains("сообщение-0")),
            "a resize must not yank the view back to the jump target: {after:?}"
        );
        assert_eq!(feed.scroll_row(), 0);
        assert_eq!(feed.marker(), Some(8), "the marker still survives");
    }

    /// The marked message is marked by its rail (fork S3 — mark the whole
    /// message, no in-content highlighting); others keep their role color.
    #[test]
    fn marked_message_rail_uses_accent() {
        let palette = Palette::default();
        let messages = numbered(4);
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[2].message_ids[0], None);
        let lines = feed.build_lines(&messages, &palette, 40, ru());

        let rail_color = |needle: &str| -> Option<Color> {
            lines
                .iter()
                .find(|l| l.spans.iter().any(|s| s.content.contains(needle)))
                .and_then(|l| l.spans.first())
                .and_then(|s| s.style.fg)
        };
        assert_eq!(rail_color("сообщение-2"), Some(palette.accent));
        assert_eq!(rail_color("сообщение-1"), Some(palette.user));
        assert_ne!(palette.accent, palette.user, "the marker must be visible");
    }

    /// Only the marker is a rendering input, so scrolling — which releases the
    /// anchor — must **not** invalidate the block cache. This is the whole point
    /// of the split: with one shared field the first scroll after a jump
    /// re-ran markdown+syntect over the entire chat.
    #[test]
    fn scrolling_after_a_jump_does_not_invalidate_the_cache() {
        let palette = Palette::default();
        let messages = numbered(12);
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[9].message_ids[0], None);
        let _ = feed.build_lines(&messages, &palette, 40, ru());

        // A warm cache reports no reset on an unchanged frame...
        feed.cache_reset = false;
        let _ = feed.build_lines(&messages, &palette, 40, ru());
        assert!(!feed.cache_reset, "precondition: the cache is warm");

        feed.scroll_up(3);
        let _ = feed.build_lines(&messages, &palette, 40, ru());
        assert!(
            !feed.cache_reset,
            "releasing the anchor must not wipe the block cache"
        );

        // ...while moving the marker legitimately does (the rail color is baked in).
        feed.focus_message(&messages, messages[2].message_ids[0], None);
        let _ = feed.build_lines(&messages, &palette, 40, ru());
        assert!(feed.cache_reset, "a new marker changes the rendered rail");
    }

    /// An id the feed doesn't show — a `Tool`/`System` message, or one from
    /// another chat — is a no-op, not a panic (and the view stays at the tail).
    #[test]
    fn focus_on_unknown_id_is_a_no_op() {
        let messages = numbered(12);
        let mut feed = MessageFeed::new();
        assert!(!feed.focus_message(&messages, Uuid::new_v4(), None));
        assert_eq!(feed.anchor(), None);
        assert_eq!(feed.marker(), None);
        let _ = visible(&mut feed, &messages, 40, 10);
        assert!(feed.is_following(), "the feed stays on the tail");
        // Also safe with nothing in the feed at all.
        assert!(!feed.focus_message(&[], Uuid::new_v4(), None));
    }

    /// A chat switch is the one place that drops the marker too — it indexes a
    /// feed that no longer exists. The highlight goes with it: it is scoped to
    /// the marked message, so it would otherwise light up whatever lands at that
    /// index in the next chat.
    #[test]
    fn clear_focus_drops_anchor_marker_and_highlight() {
        let messages = numbered(6);
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[3].message_ids[0], Some("сообщение"));
        assert!(feed.highlight.is_some());
        feed.clear_focus();
        assert_eq!(feed.anchor(), None);
        assert_eq!(feed.marker(), None);
        assert_eq!(feed.highlight, None);
    }

    // ---- highlighting the query inside the jumped-to message (fork S3(b)) ----

    /// The text of every span drawn in the accent color — i.e. every highlight.
    ///
    /// The leading rail is skipped: on the marked message it is accent too (that
    /// is the marker), and it is not a highlight. The bodies below are plain
    /// prose on purpose, since markdown paints headings, links and code in the
    /// accent color of its own accord.
    fn accented(lines: &[Line<'static>], palette: &Palette) -> Vec<String> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().skip(1))
            .filter(|s| s.style.fg == Some(palette.accent))
            .map(|s| s.content.to_string())
            .collect()
    }

    /// The gap this closes: a jump marked the message but left the searched word
    /// unhighlighted inside it, while the results list *did* highlight it.
    ///
    /// And the other half — the highlight is scoped to the marked message.
    /// Lighting up every occurrence in the chat is noise, and was not what was
    /// asked for: "where is my word in **this** message".
    ///
    /// Cyrillic throughout is deliberate: the ranges are byte offsets while the
    /// matching runs over characters, so a mix-up slices mid-character and
    /// panics right here.
    #[test]
    fn highlight_marks_the_query_inside_the_marked_message_only() {
        let palette = Palette::default();
        let messages = vec![
            msg(FeedRole::User, "первое упоминание маркера", ""),
            msg(FeedRole::User, "второе упоминание маркера", ""),
        ];
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, messages[1].message_ids[0], Some("маркер")));
        let lines = feed.build_lines(&messages, &palette, 60, ru());

        // Exactly one hit across the **whole feed** proves both halves at once:
        // the marked message shows it, and the other message — which contains the
        // same word — does not.
        assert_eq!(
            accented(&lines, &palette),
            vec!["маркер"],
            "the marked message, and only it, must show where the query matched"
        );
    }

    /// A jump with nothing to highlight (and an ordinary chat switch, which
    /// passes `None` all the way down) must not color anything.
    #[test]
    fn no_query_highlights_nothing() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::User, "упоминание маркера", "")];
        for query in [None, Some("")] {
            let mut feed = MessageFeed::new();
            feed.focus_message(&messages, messages[0].message_ids[0], query);
            let lines = feed.build_lines(&messages, &palette, 60, ru());
            assert!(
                accented(&lines, &palette).is_empty(),
                "query {query:?} must not highlight"
            );
        }
    }

    /// A match may straddle a span boundary — bold in the middle of a word, a
    /// link, an inline code span — and both halves have to light up. The
    /// markdown styling has to survive: recoloring must patch `fg` only, or
    /// highlighting a word would flatten the emphasis it sits in.
    #[test]
    fn highlight_spans_a_style_boundary_and_keeps_the_markdown_style() {
        let palette = Palette::default();
        // The emphasis splits the searched word: this renders as three spans —
        // the plain head of the word, its bold tail, then the rest of the line.
        let messages = vec![msg(FeedRole::User, "мар**кер** дальше", "")];
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[0].message_ids[0], Some("маркер"));
        let lines = feed.build_lines(&messages, &palette, 60, ru());

        let hits = accented(&lines, &palette);
        assert_eq!(
            hits.concat(),
            "маркер",
            "both halves of a straddling match must be highlighted: {hits:?}"
        );
        let bold = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "кер")
            .expect("the emphasized half");
        assert!(
            bold.style.add_modifier.contains(Modifier::BOLD),
            "the emphasis must survive the highlight: {:?}",
            bold.style
        );
        assert_eq!(bold.style.fg, Some(palette.accent));
    }

    /// The highlight is applied **before** wrapping, so `wrap::wrap_line` (which
    /// carries per-character styles through) keeps it on whichever row the match
    /// lands on. Applied afterwards it would have to reckon with the rail.
    #[test]
    fn highlight_survives_wrapping_onto_a_later_row() {
        let palette = Palette::default();
        let body = format!("{} маркер в самом хвосте", "слово ".repeat(20));
        let messages = vec![msg(FeedRole::User, &body, "")];
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[0].message_ids[0], Some("маркер"));
        let lines = feed.build_lines(&messages, &palette, 40, ru());
        let lines = &lines;
        let row = lines
            .iter()
            .position(|l| {
                l.spans
                    .iter()
                    .skip(1)
                    .any(|s| s.style.fg == Some(palette.accent))
            })
            .expect("the match must still be highlighted after wrapping");
        assert!(
            row > 1,
            "precondition: the match has to land past the first wrapped row, \
             otherwise the test proves nothing (row {row} of {})",
            lines.len()
        );
        assert_eq!(accented(lines, &palette), vec!["маркер"]);
    }

    /// The role header is excluded on purpose: it is not indexed, and
    /// `ASSISTANT` would light up on a plain search for "assistant" in every
    /// marked bubble — which reads as a bug.
    #[test]
    fn the_role_header_is_never_highlighted() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::Assistant, "речь про ассистента", "")];
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[0].message_ids[0], Some("ассистент"));
        let lines = feed.build_lines(&messages, &palette, 60, ru());

        let header = &lines[0];
        assert!(
            header.spans.iter().any(|s| s.content.contains("АССИСТЕНТ")),
            "precondition: the header spells the role out: {header:?}"
        );
        assert_eq!(
            accented(&lines, &palette),
            vec!["ассистент"],
            "only the body matches, never the header"
        );
    }

    /// **The property this stage exists for** (docs/history/in-feed-search.md §1.2).
    ///
    /// The query used to be part of `CacheKey`, so changing it cleared every
    /// block and re-ran markdown + syntect over the whole chat. In-feed search
    /// types into a field, so that would have been the cost of each keystroke.
    /// The highlight now lands on the lines handed to the renderer instead, and
    /// the cache stays warm — while the new query is still what gets colored.
    #[test]
    fn changing_the_query_does_not_invalidate_the_block_cache() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::User, "маркер и метка рядом", "")];
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[0].message_ids[0], Some("маркер"));
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        feed.cache_reset = false;
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        assert!(!feed.cache_reset, "precondition: the cache is warm");

        feed.focus_message(&messages, messages[0].message_ids[0], Some("метка"));
        let lines = feed.build_lines(&messages, &palette, 60, ru());
        assert!(
            !feed.cache_reset,
            "a new query must not rebuild any block — that is the whole point"
        );
        assert_eq!(
            accented(&lines, &palette),
            vec!["метка"],
            "and the new query is the one highlighted"
        );
    }

    /// Content that arrives on its own must not yank a reader back to the tail,
    /// while a user-initiated jump to the bottom still does (§1.3, §4).
    #[test]
    fn tail_following_is_only_forced_by_user_initiated_scrolling() {
        let mut feed = MessageFeed::new();
        feed.scroll_up(3);
        assert!(!feed.is_following());
        feed.scroll_to_bottom_if_following();
        assert!(
            !feed.is_following(),
            "arriving content must not force follow"
        );
        feed.scroll_to_bottom();
        assert!(feed.is_following(), "user-initiated: back to the tail");
        feed.scroll_to_bottom_if_following();
        assert!(feed.is_following(), "already following — stays");
    }

    #[test]
    fn scroll_up_disables_follow() {
        let mut feed = MessageFeed::new();
        assert!(feed.follow);
        feed.scroll_up(3);
        assert!(!feed.follow);
    }

    #[test]
    fn scroll_sets_take_once_flag() {
        // Scrolling (up/down) raises the flag, `take_scrolled` takes it exactly once
        // (the loop, on it, does a full repaint — erasing VS16-emoji artifacts).
        let mut feed = MessageFeed::new();
        assert!(!feed.take_scrolled(), "flag not raised before scrolling");
        feed.scroll_up(3);
        assert!(feed.take_scrolled(), "flag raised after scrolling");
        assert!(!feed.take_scrolled(), "the flag is taken exactly once");
        feed.scroll_down(3);
        assert!(feed.take_scrolled());
    }

    #[test]
    fn render_does_not_panic() {
        let mut feed = MessageFeed::new();
        let messages = vec![
            msg(FeedRole::User, "привет", ""),
            msg(
                FeedRole::Assistant,
                "# Заголовок\n\nответ с `кодом`",
                "мысль",
            ),
            FeedMessage::note("(генерация отменена)"),
        ];
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| {
            feed.render(
                f,
                f.area(),
                "Чат",
                "gemma-4 · 16k ctx",
                &messages,
                &Palette::default(),
                ru(),
            )
        })
        .unwrap();
    }

    /// The panel's top border is a row like any other: a title too long for
    /// what the marker and the meta leave is cut there, with the "…" that
    /// says so, and the meta keeps its corner. Nothing bounds a title in
    /// storage (`shared::title::sanitize_title`, spec §11.2).
    #[test]
    fn a_long_chat_title_is_cut_on_the_border_and_marked() {
        let top = |term: &Terminal<TestBackend>| -> String {
            let buf = term.backend().buffer();
            (buf.area.left()..buf.area.right())
                .map(|x| buf[(x, buf.area.top())].symbol().to_string())
                .collect()
        };
        let mut feed = MessageFeed::new();
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        let long = "заголовок который не помещается в верхнюю рамку никак";
        let meta = "gemma-4 · 16k ctx";
        term.draw(|f| feed.render(f, f.area(), long, meta, &[], &Palette::default(), ru()))
            .unwrap();
        let row = top(&term);
        assert!(row.contains('…'), "a cut title says so: {row}");
        assert!(row.contains(meta), "the meta keeps its corner: {row}");
        assert!(row.starts_with('╭') && row.ends_with('╮'), "{row}");
        // Room for all of it — no marker, and the title is whole.
        let mut term = Terminal::new(TestBackend::new(100, 8)).unwrap();
        term.draw(|f| feed.render(f, f.area(), long, meta, &[], &Palette::default(), ru()))
            .unwrap();
        let row = top(&term);
        assert!(row.contains(long), "{row}");
        assert!(!row.contains('…'), "{row}");
    }

    #[test]
    fn scrollbar_appears_only_when_feed_overflows() {
        // The "█" thumb on the right border — only when there are more lines than the feed's height.
        let right_col = |term: &Terminal<TestBackend>| -> Vec<String> {
            let buf = term.backend().buffer();
            let area = buf.area;
            (area.top()..area.bottom())
                .map(|y| buf[(area.right() - 1, y)].symbol().to_string())
                .collect()
        };
        let mut feed = MessageFeed::new();
        let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
        let short = vec![msg(FeedRole::User, "привет", "")];
        term.draw(|f| feed.render(f, f.area(), "Чат", "", &short, &Palette::default(), ru()))
            .unwrap();
        assert!(
            !right_col(&term).iter().any(|s| s == "█"),
            "a short feed — no scrollbar thumb"
        );
        let many: Vec<FeedMessage> = (0..30)
            .map(|i| msg(FeedRole::User, &format!("строка {i}"), ""))
            .collect();
        term.draw(|f| feed.render(f, f.area(), "Чат", "", &many, &Palette::default(), ru()))
            .unwrap();
        assert!(
            right_col(&term).iter().any(|s| s == "█"),
            "an overflowing feed — with a scrollbar thumb"
        );
    }

    /// Collects a line's span content into a tuple (text, fg) — for comparison.
    fn line_sig(l: &Line<'static>) -> Vec<(String, Option<Color>)> {
        l.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect()
    }

    /// Grid body of [`cache_matches_fresh_render`]: for one scenario and one
    /// compaction boundary, sweep widths × palettes × collapse state (both
    /// kinds) and require the warm cache to match a fresh render.
    fn assert_warm_matches_fresh(messages: &[FeedMessage], compaction: Option<(Uuid, String)>) {
        for width in [40usize, 80] {
            for palette in [Palette::default(), Palette::default().with_compat(true)] {
                for thoughts in [false, true] {
                    for tools in [false, true] {
                        let view = FeedView { thoughts, tools };
                        let mut warm = MessageFeed::new();
                        warm.set_view(view);
                        warm.set_compaction(compaction.clone());
                        // warm the cache with repeated calls
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let warm_lines = warm.build_lines(messages, &palette, width, ru());

                        let mut fresh = MessageFeed::new();
                        fresh.set_view(view);
                        fresh.set_compaction(compaction.clone());
                        let fresh_lines = fresh.build_lines(messages, &palette, width, ru());

                        let w: Vec<_> = warm_lines.iter().map(line_sig).collect();
                        let f: Vec<_> = fresh_lines.iter().map(line_sig).collect();
                        assert_eq!(
                            w,
                            f,
                            "cache diverged: width={width} view={view:?} \
                             compaction={}",
                            compaction.is_some()
                        );
                    }
                }
            }
        }
    }

    /// A warm cache gives line-for-line identical output to a fresh render — across
    /// scenarios, widths, palettes, and collapse state (both kinds).
    #[test]
    fn cache_matches_fresh_render() {
        let tool_msg = {
            let mut m = msg(FeedRole::Assistant, "готово", "");
            m.tools.push(FeedToolCall {
                name: "note_save".into(),
                arguments: "{}".into(),
                result: "ок".into(),
                text_offset: m.text.len(),
                images: 0,
                call_id: None,
                running: false,
            });
            m
        };
        let scenarios: Vec<Vec<FeedMessage>> = vec![
            vec![msg(FeedRole::User, "привет как дела сегодня", "")],
            vec![
                msg(FeedRole::User, "вопрос", ""),
                msg(
                    FeedRole::Assistant,
                    "# Заголовок\n\nответ с `кодом` и формулой $x^2 + \\alpha$",
                    "рассуждение модели",
                ),
            ],
            vec![tool_msg],
        ];
        for messages in &scenarios {
            // The compaction boundary reshapes the block it lands on, so it is a
            // cache input like the collapse state — covered here too.
            let folded = Some((messages[0].message_ids[0], "ранее обсудили X".to_string()));
            for compaction in [None, folded] {
                assert_warm_matches_fresh(messages, compaction);
            }
        }
    }

    /// Streaming: at every step of text growth, a warm cache matches a fresh render
    /// (only the tail message gets recomputed).
    #[test]
    fn cache_matches_fresh_during_streaming() {
        let mut warm = MessageFeed::new();
        let palette = Palette::default();
        let full = "Это ответ ассистента, который растёт по чанкам стриминга.";
        let total = full.chars().count();
        for end in (1..=total).step_by(3) {
            let partial: String = full.chars().take(end).collect();
            let mut m = msg(FeedRole::Assistant, &partial, "");
            m.streaming = true;
            let messages = vec![msg(FeedRole::User, "спроси", ""), m];
            let warm_lines = warm.build_lines(&messages, &palette, 60, ru());
            let mut fresh = MessageFeed::new();
            let fresh_lines = fresh.build_lines(&messages, &palette, 60, ru());
            let w: Vec<_> = warm_lines.iter().map(line_sig).collect();
            let f: Vec<_> = fresh_lines.iter().map(line_sig).collect();
            assert_eq!(w, f, "stream cache diverged at end={end}");
        }
    }

    /// Editing a message's text invalidates the cache (new text visible, old one gone).
    #[test]
    fn cache_invalidates_on_message_edit() {
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let join = |ls: &[Line<'static>]| -> String {
            ls.iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
                .collect()
        };
        let l1 = feed.build_lines(
            &[msg(FeedRole::Assistant, "первый вариант", "")],
            &palette,
            80,
            ru(),
        );
        assert!(join(&l1).contains("первый вариант"));
        let l2 = feed.build_lines(
            &[msg(FeedRole::Assistant, "другой текст", "")],
            &palette,
            80,
            ru(),
        );
        let j2 = join(&l2);
        assert!(j2.contains("другой текст"), "{j2}");
        assert!(!j2.contains("первый вариант"), "stale cache: {j2}");
    }

    /// History truncation (Ctrl+E/regenerate): after shortening, a warm cache's output
    /// matches a fresh one (the cache's tail is dropped).
    #[test]
    fn cache_handles_history_truncation() {
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let long = vec![
            msg(FeedRole::User, "раз", ""),
            msg(FeedRole::Assistant, "ответ раз", ""),
            msg(FeedRole::User, "два", ""),
            msg(FeedRole::Assistant, "ответ два", ""),
        ];
        let _ = feed.build_lines(&long, &palette, 80, ru());
        let warm = feed.build_lines(&long[..2], &palette, 80, ru());
        let mut fresh = MessageFeed::new();
        let fresh_lines = fresh.build_lines(&long[..2], &palette, 80, ru());
        let w: Vec<_> = warm.iter().map(line_sig).collect();
        let f: Vec<_> = fresh_lines.iter().map(line_sig).collect();
        assert_eq!(w, f);
    }

    // ---------- history-compaction boundary (spec §6.7, fork F8c) ----------

    /// The divider's localized label — the row the boundary is found by.
    fn compacted_label() -> &'static str {
        ru().t("ui.feed.compacted")
    }

    /// The index of the compaction divider among rendered rows.
    fn divider_row(rows: &[String]) -> Option<usize> {
        rows.iter().position(|r| r.contains(compacted_label()))
    }

    /// The divider sits **immediately above** the block where verbatim history
    /// resumes — that placement is the whole message ("everything above is a
    /// summary"), so it is asserted positionally rather than by mere presence.
    /// And it is drawn only when a compaction is set: an untouched chat must
    /// look exactly as it did before the feature existed.
    #[test]
    fn compaction_divider_precedes_the_boundary_block() {
        let messages = numbered(4);
        let boundary = messages[2].message_ids[0];

        let mut plain = MessageFeed::new();
        let rows = row_texts(&plain.build_lines(&messages, &Palette::default(), 80, ru()));
        assert!(
            divider_row(&rows).is_none(),
            "no compaction — no divider: {rows:?}"
        );

        let mut feed = MessageFeed::new();
        feed.set_compaction(Some((boundary, "ранее обсудили X".into())));
        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        let at = divider_row(&rows).expect("the divider must be drawn");
        let target = rows
            .iter()
            .position(|r| r.contains("сообщение-2"))
            .expect("the boundary message must still be shown");
        let previous = rows
            .iter()
            .position(|r| r.contains("сообщение-1"))
            .expect("the folded messages are still shown — only the request shrinks");
        assert!(
            previous < at && at < target,
            "the divider belongs between the folded part and the boundary block, \
             got previous={previous} divider={at} target={target}: {rows:?}"
        );
    }

    /// Collapsed by default, like every foldable block, and folded by the CoT
    /// toggle rather than one of its own (fork **F8c**): the pill names `Ctrl+T`,
    /// and `toggle_thoughts` is what reveals the summary.
    #[test]
    fn compaction_summary_folds_with_the_thoughts_blocks() {
        let messages = numbered(3);
        let mut feed = MessageFeed::new();
        feed.set_compaction(Some((
            messages[1].message_ids[0],
            "ранее обсудили X".into(),
        )));

        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        let at = divider_row(&rows).expect("the divider must be drawn");
        assert!(
            !rows.iter().any(|r| r.contains("ранее обсудили X")),
            "collapsed by default — the summary must not be on screen: {rows:?}"
        );
        assert!(
            rows[at].contains("Ctrl+T"),
            "the collapsed pill names the key that reveals it: {:?}",
            rows[at]
        );

        feed.toggle_thoughts();
        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        let at = divider_row(&rows).expect("the divider stays when expanded");
        assert!(
            rows.iter().any(|r| r.contains("ранее обсудили X")),
            "Ctrl+T must reveal the summary: {rows:?}"
        );
        assert!(
            !rows[at].contains("Ctrl+T"),
            "expanded, the keycap drops away (as the thoughts block does): {:?}",
            rows[at]
        );
    }

    /// A boundary the feed can't place draws **nothing**. The id is a domain
    /// message id and the projection drops `Tool`/`System` messages entirely, so
    /// this is reachable in principle — and guessing a position would put the
    /// divider where verbatim history does *not* resume, which is worse than
    /// staying quiet.
    #[test]
    fn compaction_with_an_unknown_boundary_draws_nothing() {
        let messages = numbered(3);
        let mut feed = MessageFeed::new();
        feed.set_compaction(Some((Uuid::new_v4(), "ранее обсудили X".into())));
        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        assert!(divider_row(&rows).is_none(), "{rows:?}");
        assert!(
            !rows.iter().any(|r| r.contains("ранее обсудили X")),
            "{rows:?}"
        );
    }

    /// A merged agentic bubble carries every round's id, so a boundary landing on
    /// a *later* round must still find the bubble — the reason the resolution goes
    /// through `message_ids` rather than through a position.
    #[test]
    fn compaction_boundary_resolves_through_a_merged_bubble() {
        use crate::entities::message::Message;
        let user = Message::user("вопрос");
        let r1 = Message::assistant("Ищу.");
        let r2 = Message::assistant("Готово.");
        let second_round = r2.id;
        let messages = FeedMessage::from_messages(&[user, r1, r2]);
        assert_eq!(messages.len(), 2, "the rounds merge into one bubble");

        let mut feed = MessageFeed::new();
        feed.set_compaction(Some((second_round, "ранее обсудили X".into())));
        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        let at = divider_row(&rows).expect("a merged round's id must still resolve");
        let bubble = rows.iter().position(|r| r.contains("Ищу.")).unwrap();
        assert!(
            at < bubble,
            "the divider precedes the whole bubble: {rows:?}"
        );
    }

    /// The summary rides `CacheKey`, so a fresh roll over the same boundary has to
    /// repaint. Without it the divider would keep showing the previous summary —
    /// invisible to every other assertion, since the block's own fingerprint
    /// never changed.
    #[test]
    fn changing_the_summary_invalidates_the_cache() {
        let messages = numbered(3);
        let boundary = messages[1].message_ids[0];
        let mut feed = MessageFeed::new();
        feed.toggle_thoughts(); // show the summary itself, not just the pill
        feed.set_compaction(Some((boundary, "первое резюме".into())));
        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        assert!(rows.iter().any(|r| r.contains("первое резюме")), "{rows:?}");

        feed.set_compaction(Some((boundary, "второе резюме".into())));
        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        assert!(rows.iter().any(|r| r.contains("второе резюме")), "{rows:?}");
        assert!(
            !rows.iter().any(|r| r.contains("первое резюме")),
            "stale cache: {rows:?}"
        );

        // Clearing it takes the divider away again.
        feed.set_compaction(None);
        let rows = row_texts(&feed.build_lines(&messages, &Palette::default(), 80, ru()));
        assert!(divider_row(&rows).is_none(), "{rows:?}");
    }

    /// The divider fills the panel exactly, so it reads as one rule rather than
    /// as a stray line of text — in both glyph sets and both collapse states.
    ///
    /// A panel too narrow for the label is covered separately below: the row is
    /// then left long and the outer wrap deals with it, exactly as it does for
    /// the "thoughts" pill.
    #[test]
    fn compaction_divider_fills_the_panel_width() {
        let messages = numbered(2);
        for width in [60usize, 80] {
            for palette in [Palette::default(), Palette::default().with_compat(true)] {
                for expanded in [false, true] {
                    let mut feed = MessageFeed::new();
                    if expanded {
                        feed.toggle_thoughts();
                    }
                    feed.set_compaction(Some((
                        messages[1].message_ids[0],
                        "ранее обсудили X".into(),
                    )));
                    let lines = feed.build_lines(&messages, &palette, width, ru());
                    let rows = row_texts(&lines);
                    let at = divider_row(&rows).expect("the divider must be drawn");
                    assert_eq!(
                        line_width(&lines[at]),
                        width,
                        "the divider must span the panel at width={width} \
                         expanded={expanded} compat={}: {:?}",
                        palette.compat,
                        rows[at]
                    );
                }
            }
        }
    }

    /// Whatever the panel width, every row the divider produces fits it once the
    /// feed's own wrap pass has run — the invariant `render` relies on (a longer
    /// row would spill past the panel border).
    ///
    /// Measured **trimmed**: `wrap_ranges` lets a word-boundary space hang one
    /// column past the edge, which every other block lives with too (it is what
    /// `trim_row_trailing_ws` exists for in the table and code-block paths) and
    /// which draws nothing. Asserting on the untrimmed width would be asserting
    /// a promise the wrap has never made.
    #[test]
    fn compaction_divider_respects_the_wrap_invariant_when_cramped() {
        let messages = numbered(2);
        for width in [20usize, 30, 46] {
            for expanded in [false, true] {
                let mut feed = MessageFeed::new();
                if expanded {
                    feed.toggle_thoughts();
                }
                feed.set_compaction(Some((
                    messages[1].message_ids[0],
                    "ранее обсудили X, а также Y и Z, и это довольно длинное резюме".into(),
                )));
                let lines = feed.build_lines(&messages, &Palette::default(), width, ru());
                for line in &lines {
                    for wrapped in wrap::wrap_line(line, width) {
                        let text: String = row_texts(std::slice::from_ref(&wrapped)).remove(0);
                        let w = wrap::display_width(&text.trim_end().chars().collect::<Vec<_>>());
                        assert!(
                            w <= width,
                            "row {w} columns wide at width={width} \
                             expanded={expanded}: {text:?}"
                        );
                    }
                }
            }
        }
    }

    /// A rendered line's width in columns.
    fn line_width(line: &Line<'static>) -> usize {
        line.spans
            .iter()
            .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
            .sum()
    }
}
