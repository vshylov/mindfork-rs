//! Message feed: markdown rendering of bodies (via [`crate::shared::markdown`]),
//! a collapsible "thoughts" block (CoT), and vertical scrolling. See spec §11.3-11.4.
//!
//! "Thoughts" and tool cards fold all at once (`Ctrl+T`, `Ctrl+O`) or one block
//! alone, by a click on its pill ([`FeedView`],
//! docs/research/block-fold-and-partial-copy.md); tool blocks
//! (name/arguments/result) show inside the assistant's message (M5). The widget
//! only holds view state (scroll, "follow the tail", what is folded); the
//! messages themselves belong to UI state and are passed in for rendering.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use uuid::Uuid;

use crate::entities::chat::{FeedView, FoldKey};
use crate::entities::chat_branch::LineMark as VariantMark;
use crate::entities::feed_landing::FeedPlace;
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::CharacterNames;
use crate::features::tools::present::{self, ToolBlock};
use crate::shared::config::CodeBlockBackground;
use crate::shared::i18n::Locale;
use crate::shared::markdown::{self, LineMark};
use crate::shared::mouse::{AUTOSCROLL_STEP, SelectUnit};
use crate::shared::theme::Palette;
use crate::shared::ui::{self, render_scrollbar};
use crate::shared::wrap;
use crate::widgets::feed_selection::{self, FeedPoint, FeedRows, RowFact};
use crate::widgets::token_counter::TokenCounter;

/// Gutter rail to the left of every message line: a colored vertical bar +
/// a space (redesign: "Role rails — colored ▌ in the gutter"). Width — 2 columns.
const RAIL: &str = "▌ ";

/// The rails of the **monochrome** mode (spec §11.6), where the bar has no
/// colour to say whose message a row belongs to — or that it is the one a
/// jump landed on — and says it by shape. All WGL4, all as wide as [`RAIL`].
const RAIL_MONO_USER: &str = RAIL;
const RAIL_MONO_ASSISTANT: &str = "║ ";
const RAIL_MONO_QUIET: &str = "│ ";
const RAIL_MONO_MARKED: &str = "█ ";

/// The gutter of a foldable block's content — thoughts, the compaction
/// summary: `│ ` before every source line.
const GUTTER: &str = "│ ";

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
    /// The block this card is, for a click on its pill
    /// (docs/research/block-fold-and-partial-copy.md §3.2): its message and its
    /// index in that message's `tool_calls`. `None` for a live card — it has no
    /// stored message yet, so it takes no click and follows `Ctrl+O` alone.
    pub fold: Option<FoldKey>,
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
    /// How long the voice note was, for a user message that came as one from
    /// the messenger (`Message::voice`, ADR 0013 stage C): drawn beside the
    /// role header.
    pub voice: Option<u32>,
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
            voice: None,
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
            voice: None,
        }
    }

    /// Whether this item is a message of the conversation, and so takes a
    /// number (docs/research/chat-branches.md §3.5): the person's and the
    /// assistant's bubbles — the echo and the reply being written among them,
    /// which are about to be stored — and a note row a stored message stands
    /// behind (a task notification, a director's line). The screen's own
    /// notes and a transcript's persona are not messages.
    ///
    /// Over a feed projected from stored messages this is every item, so the
    /// numbers are the ranks of `entities::chat::visible_rows` — what the chat
    /// card counts and `/copy last N` takes.
    pub fn is_numbered(&self) -> bool {
        match self.role {
            FeedRole::User | FeedRole::Assistant => true,
            FeedRole::Note => !self.message_ids.is_empty(),
            FeedRole::System => false,
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
        // Enumerated **before** the filter: a card's key is its index in the
        // stored `tool_calls`, which a hidden control call takes a place in.
        let tools = msg
            .tool_calls
            .iter()
            .enumerate()
            .filter(|(_, tc)| !crate::features::tools::control::is_control_tool(&tc.name))
            .map(|(call, tc)| FeedToolCall {
                name: tc.name.clone(),
                arguments: tc.arguments.to_string(),
                result: tc.result.clone().unwrap_or_default(),
                text_offset: off,
                images: tc.images,
                call_id: None,
                running: false,
                fold: Some(FoldKey::Tool {
                    message: msg.id,
                    call,
                }),
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
            voice: msg.voice.map(|v| v.seconds),
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
    /// (spec §11.3, docs/history/feed-collapse.md).
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
    /// The background of an unhighlighted code block (setting
    /// `interface.code_block_background`, threaded via
    /// [`MessageFeed::set_code_block_background`]).
    code_block_background: CodeBlockBackground,
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
    /// No chat engine is configured at all (the server reports `NotConfigured`),
    /// set by the screen from the status snapshot. An empty feed then says how to
    /// connect a model instead of inviting a message that can only be refused —
    /// the first thing a fresh install shows (docs/research/public-release-readiness.md
    /// §3.2). The placeholder is never cached, so this rides no [`CacheKey`].
    engine_missing: bool,
    /// Where the `chat://` references of the **last drawn frame** sit on screen
    /// (spec §11.3). Rebuilt by every [`MessageFeed::render`] and read by
    /// [`MessageFeed::chat_link_at`], so a click is answered against the frame
    /// the user actually clicked on.
    link_hits: Vec<LinkHit>,
    /// Where the foldable blocks' pills of the **last drawn frame** sit on
    /// screen — what [`MessageFeed::fold_at`] answers a click against, like
    /// [`Self::link_hits`] (docs/research/block-fold-and-partial-copy.md §3.5).
    fold_hits: Vec<FoldHit>,
    /// Where the row-end tags of the **last drawn frame** sit on screen — a
    /// message's number and the arrows of its variants mark: what
    /// [`MessageFeed::tap_at`] answers a click against, like
    /// [`Self::fold_hits`] (docs/research/chat-branches.md §3.7).
    tap_hits: Vec<TapCell>,
    /// Which variant of its place a bubble of the line is, by the bubble's
    /// first message: `(variant, of)` for the bubbles that have other variants
    /// parked ([`MessageFeed::set_variants`]). Drawn before the bubble's
    /// number, `‹2/3› #11`.
    variants: std::collections::HashMap<Uuid, (u16, u16)>,
    /// Cache of rendered lines, one block per message (see [`CachedBlock`]).
    /// Index = message position. `build_lines` is called on every dirty frame
    /// (streaming, scrolling) and would re-run markdown+syntect over the WHOLE
    /// history each time; the cache recomputes only the changed messages (checked by fingerprint).
    cache: Vec<CachedBlock>,
    /// Cache key: changing the width/palette/thoughts display resets the whole cache.
    cache_key: Option<CacheKey>,
    /// A "put the view here" request, consumed by the next
    /// [`MessageFeed::render`]. Deferred by necessity: turning a feed index into
    /// a scroll row needs the block cache built at the current panel width, and
    /// neither exists outside `render` (docs/history/chat-search-stage2.md §1.1).
    pending: Option<Pending>,
    /// The feed item drawn with the accent rail — "this is where you landed".
    ///
    /// **Survives a manual scroll** (that is the point: you scroll around the
    /// hit to read its context and can still see it), and is only dropped when
    /// the chat changes or a new jump replaces it. Being separate from where
    /// the view stands is also what keeps it cheap — it is in [`CacheKey`] and
    /// the scroll is not, so scrolling does not invalidate the block cache.
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
    /// Consumed by `render` to put the view back on what it showed. See [`CacheKey`].
    cache_reset: bool,
    /// Where the rows of the **last drawn frame** sit on screen — what a press,
    /// a drag and the selection's highlight are read against, like the click
    /// map ([`Self::link_hits`]): the frame the user pointed at, not a stored
    /// assumption about it. `None` before the first frame and over an empty
    /// feed (docs/research/mouse-always-captured.md §5.3).
    drawn: Option<DrawnRows>,
    /// The feed's text selection: `(start, end)` in reading order, both cells
    /// in it (§5.3–§5.4). At most one selection is on the screen, the feed's or
    /// a field's — the chat screen ends the other when one starts.
    ///
    /// **What drops it** (research F9): every [`CacheKey`] change, since each
    /// one rewraps every row; a block it covers rebuilt or removed — a
    /// regenerate, a take-back; and the assistant starting to answer
    /// ([`Self::set_selectable`], F6). A message or a note appended below
    /// keeps it: a point is anchored to its block.
    selection: Option<(FeedPoint, FeedPoint)>,
    /// The left-button gesture under way in the feed ([`Self::press`] to
    /// [`Self::release`]).
    gesture: Option<Gesture>,
    /// Whether a selection can be made: not while the assistant answers
    /// (research F6) — the feed scrolls itself then and markdown re-forms the
    /// text as it is written, both moving what a selection points at.
    selectable: bool,
}

/// Where the next frame puts the view ([`MessageFeed::pending`]), in the
/// widget's own coordinates: a block is a feed index, a row is counted inside
/// its block (docs/research/feed-landing.md §3.3–§3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// The block's `row`-th row on the view's first row — a jump (row 0), a
    /// place restored. A row past the block's end is its last: the block was
    /// rewrapped since the row was read.
    Row { block: usize, row: usize },
    /// The block's first row left where `held` puts the view, when that shows
    /// it; on the view's first row otherwise. `held` is the view's first row
    /// before the feed was rebuilt, `None` when it cannot be found again.
    Reveal {
        block: usize,
        held: Option<(usize, usize)>,
    },
    /// A search hit in the block: its first row on the view's first when the
    /// first highlighted match is on the screen that way, and otherwise that
    /// match a third of the view down — the word that was searched for is what
    /// the person came to read (docs/research/feed-landing.md, fork F3).
    Hit { block: usize },
    /// What was on the view's first row before a rewrap: the block's `row`-th
    /// row of the `of` it had then, found again among the rows it has now —
    /// by the line it was a piece of where the frame recorded one and the
    /// block still has it ([`TopLine`]), by its share of the block otherwise
    /// ([`rewrapped_row`]; research fork F2).
    Kept {
        block: usize,
        row: usize,
        of: usize,
        line: Option<TopLine>,
    },
}

/// A logical line of a block: the rows a wrap broke one line into
/// ([`RowFact::soft`] says which rows continue the one above), and what the
/// line says. Made with the block and cached beside its rows
/// ([`CachedBlock::said`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Said {
    /// Its first row in the block.
    row: usize,
    /// How many rows it was broken into.
    pieces: usize,
    /// What it says, whitespace apart, hashed ([`feed_selection::ink`]);
    /// `None` for a line that says nothing — a blank row, the separator.
    key: Option<u64>,
}

/// The logical lines of a block's rows.
fn said(lines: &[Line<'static>], facts: &[RowFact]) -> Vec<Said> {
    use std::hash::Hasher;
    let mut out: Vec<Said> = Vec::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut inked = 0;
    for (row, (line, fact)) in lines.iter().zip(facts).enumerate() {
        // A block's first row begins a line, whatever it says of itself.
        if row == 0 || !fact.soft {
            if let Some(last) = out.last_mut() {
                last.pieces = row - last.row;
                last.key = (inked > 0).then(|| hasher.finish());
            }
            out.push(Said {
                row,
                pieces: 1,
                key: None,
            });
            hasher = std::collections::hash_map::DefaultHasher::new();
            inked = 0;
        }
        inked += feed_selection::ink(line, *fact, &mut hasher);
    }
    if let Some(last) = out.last_mut() {
        last.pieces = lines.len() - last.row;
        last.key = (inked > 0).then(|| hasher.finish());
    }
    out
}

/// What the view's first row is, in terms a rewrap does not change: the line
/// it is a piece of, named by what the line **says**.
///
/// A rewrap breaks a line into other pieces and leaves its text the text it
/// was, so the line is found again exactly — at another width, and with a
/// fold opened or closed elsewhere in the block. A share of the block's rows
/// is a row or so off instead, and a window dragged by its edge is rewrapped
/// at every width it passes: an error of a row a step walks the view away
/// from what the person was reading. A line's *number* is no better: a table
/// above it is laid out in other rows at another width. Both found by the pty
/// run of stage 3 (docs/research/feed-landing.md, fork F2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopLine {
    /// A piece of a line that says something: the `nth` of the block's `of`
    /// lines that say `key`, and the row's place among its `pieces`.
    Text {
        key: u64,
        nth: usize,
        of: usize,
        piece: usize,
        pieces: usize,
    },
    /// A blank row, `above` rows over the next line that says something.
    /// Blank rows are not rewrapped, so the distance holds.
    Above {
        key: u64,
        nth: usize,
        of: usize,
        above: usize,
    },
    /// A blank row with only blank rows under it in its block — the
    /// separator: so many rows from the block's end.
    FromEnd(usize),
}

impl TopLine {
    /// What row `row` is in a block of `rows` rows whose lines are `said`.
    fn of(said: &[Said], rows: usize, row: usize) -> Option<Self> {
        let row = row.min(rows.checked_sub(1)?);
        let i = said.partition_point(|l| l.row <= row).checked_sub(1)?;
        // Which of the lines that say the same line `j` is, and of how many.
        let among = |j: usize| {
            let same = |l: &&Said| l.key == said[j].key;
            (
                said[..j].iter().filter(same).count(),
                said.iter().filter(same).count(),
            )
        };
        if let Some(key) = said[i].key {
            let (nth, of) = among(i);
            return Some(Self::Text {
                key,
                nth,
                of,
                piece: row - said[i].row,
                pieces: said[i].pieces,
            });
        }
        Some(
            match (i + 1..said.len()).find_map(|j| said[j].key.map(|key| (j, key))) {
                Some((j, key)) => {
                    let (nth, of) = among(j);
                    Self::Above {
                        key,
                        nth,
                        of,
                        above: said[j].row - row,
                    }
                }
                None => Self::FromEnd(rows - row),
            },
        )
    }

    /// The row that shows it in the block as rewrapped: `rows` rows, lines
    /// `said`. `None` where the line cannot be told — it says something else
    /// now (a table's row at another width), or the block has another number
    /// of lines that say the same.
    fn row_in(self, said: &[Said], rows: usize) -> Option<usize> {
        let find = |key: u64, nth: usize, of: usize| {
            let mut same = said.iter().filter(|l| l.key == Some(key));
            (same.clone().count() == of)
                .then(|| same.nth(nth))
                .flatten()
        };
        match self {
            Self::Text {
                key,
                nth,
                of,
                piece,
                pieces,
            } => {
                let line = find(key, nth, of)?;
                Some(line.row + rewrapped_row(piece, pieces, line.pieces))
            }
            Self::Above {
                key,
                nth,
                of,
                above,
            } => find(key, nth, of)?.row.checked_sub(above),
            Self::FromEnd(n) => rows.checked_sub(n),
        }
    }
}

/// The geometry of the last drawn frame ([`MessageFeed::drawn`]).
#[derive(Debug, Clone)]
struct DrawnRows {
    /// The panel's inner area — the rows' cells.
    inner: Rect,
    /// The first row drawn, as an index into the whole feed's rows.
    scroll: usize,
    /// The furthest the feed scrolls (`render`'s `max_scroll`).
    max_scroll: usize,
    /// Where each block's rows begin in the whole feed's rows.
    starts: Vec<usize>,
    /// How many rows the feed has.
    total: usize,
    /// What the first row drawn is, in terms a rewrap does not change — what
    /// one puts the view back on ([`TopLine`]).
    top: Option<TopLine>,
}

impl DrawnRows {
    fn holds(&self, col: u16, row: u16) -> bool {
        let r = self.inner;
        (r.x..r.x.saturating_add(r.width)).contains(&col)
            && (r.y..r.y.saturating_add(r.height)).contains(&row)
    }

    /// The block holding the feed's `abs`-th row, and the row within it.
    fn locate(&self, abs: usize) -> (usize, usize) {
        let block = self.starts.partition_point(|&s| s <= abs).saturating_sub(1);
        (block, abs - self.starts[block])
    }

    /// How many rows the block had in this frame.
    fn rows_of(&self, block: usize) -> usize {
        let end = self.starts.get(block + 1).copied().unwrap_or(self.total);
        end.saturating_sub(self.starts[block])
    }
}

/// The row of a block that shows what its `row`-th row showed before a rewrap
/// took it from `of` rows to `rows`: the same share of the way through it. A
/// block the rewrap left alone keeps the row exactly; the first row stays the
/// first and the last the last. Rounded **down**: where a row became two, the
/// view starts on the first of them and not in the middle of the sentence.
fn rewrapped_row(row: usize, of: usize, rows: usize) -> usize {
    if of <= 1 || rows == 0 {
        return 0;
    }
    let (was_last, last) = (of - 1, rows - 1);
    row.min(was_last) * last / was_last
}

/// A left-button gesture in the feed: what its press took, and where the
/// pointer is now.
#[derive(Debug, Clone, Copy)]
struct Gesture {
    /// A character at a time, or the word or the line of a double or a
    /// triple press ([`SelectUnit`]).
    unit: SelectUnit,
    /// What the press took: its cell, or the word or the line under it.
    origin: (FeedPoint, FeedPoint),
    /// The pointer's cell since the first drag. Resolved again after every
    /// frame — a wheel or an autoscroll moves the rows under a pointer that
    /// stays.
    pointer: Option<(u16, u16)>,
    /// When the next autoscroll step is due.
    next_step: Option<std::time::Instant>,
}

/// The feed's cached blocks, as [`FeedRows`].
struct Blocks<'a>(&'a [CachedBlock]);

impl FeedRows for Blocks<'_> {
    fn block_count(&self) -> usize {
        self.0.len()
    }
    fn row_count(&self, block: usize) -> usize {
        self.0[block].lines.len()
    }
    fn row(&self, block: usize, row: usize) -> (&Line<'static>, RowFact) {
        let b = &self.0[block];
        (&b.lines[row], b.facts[row])
    }
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
    /// Collapsing either kind of block reshapes every message — hence both
    /// flags. The blocks a click flipped are **not** here: a click changes one
    /// block, and that block's own fold state rides its cache entry
    /// ([`CachedBlock::folds`]), so it is the one rebuilt.
    thoughts: bool,
    tools: bool,
    table_row_separators: bool,
    render_mermaid: bool,
    code_block_background: CodeBlockBackground,
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
    /// here; where the view stands is not a rendering input.
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
    /// The expanded state of the block's foldable parts ([`fold_state`]): a
    /// click on one pill changes this, not [`CacheKey`], so the one block it
    /// folded is rebuilt and no other.
    folds: u64,
    /// The message's number, drawn into its first row ([`numbers`]). A
    /// property of the item's place in the feed, not of the item, so it rides
    /// the entry beside the fingerprint: a block is rebuilt when the messages
    /// above it change in count.
    number: Option<u32>,
    /// The bubble's variants mark, drawn before the number — checked like it.
    variant: Option<(u16, u16)>,
    lines: Vec<Line<'static>>,
    /// One per row of `lines`: what of it is the rail, and whether it
    /// continues the line above — what a selection needs and the bare `Line`
    /// no longer says (docs/research/mouse-always-captured.md §5.4).
    facts: Vec<RowFact>,
    /// The block's logical lines and what each says ([`Said`]) — what a
    /// rewrap finds the view's first row by. Made on first use and kept with
    /// the build: only the block a reader's view starts in is ever asked, and
    /// reading every row of a long reply at each chunk of it would be paid by
    /// a feed that follows its tail and needs none of it (2.5 ms beside a
    /// 10 ms build, measured on a 40K-character reply).
    said: std::cell::OnceCell<Vec<Said>>,
    /// The conversations this block links to, in the order they are drawn —
    /// what the reference picker offers (spec §11.3). Recorded while the block
    /// is built, because that is where the addresses are recognised and where
    /// they are still whole: after the wrap they may be split across rows.
    links: Vec<Uuid>,
    /// Where the block's pills are in its rows ([`PillHit`]) — what the click
    /// map is made of. Recorded while the block is built, as `links` are,
    /// because only the build knows which spans are a pill.
    pills: Vec<PillHit>,
    /// Where the block's row-end tag is in its rows ([`TapHit`]): the number
    /// and the arrows of the variants mark.
    taps: Vec<TapHit>,
    /// How many of `lines` the role header occupies — the highlight pass skips
    /// them. Recorded here because the header's height is only known while the
    /// block is built: it is normally one line, but a long custom role name can
    /// wrap it, so counting output lines is the only stable answer.
    content_from: usize,
}

impl CachedBlock {
    /// The block's logical lines ([`Self::said`]).
    fn said(&self) -> &[Said] {
        self.said.get_or_init(|| said(&self.lines, &self.facts))
    }
}

impl Default for MessageFeed {
    fn default() -> Self {
        Self::new()
    }
}

/// What is drawn around the feed's rows (spec §11.1.1): the panel's border,
/// with the chat's title and the model caption on its top and the token
/// counter at its bottom right corner (spec §11.3) — or nothing, in a
/// window with no rows to spare for a border. A bare feed wraps to the
/// window's full width, has no scrollbar, and ends on the last line of text
/// rather than on the padding row under it.
#[derive(Debug, Clone, Copy)]
pub enum FeedFrame<'a> {
    Bordered {
        title: &'a str,
        caption: &'a FeedCaption,
        /// The token counter; `None` — there is none on this screen.
        counter: Option<TokenCounter>,
    },
    Bare,
}

/// The feed header's right-hand caption — the model behind the chat — in the
/// forms the header can afford ([`fit_header`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedCaption {
    /// The model's name; empty — there is no caption.
    pub name: String,
    /// What follows the name where the row has room for it: a managed model's
    /// context size.
    pub detail: Option<String>,
}

impl FeedCaption {
    /// The caption in full: the name and, behind a `·`, its detail.
    pub fn full(&self) -> String {
        match &self.detail {
            Some(detail) if !self.name.is_empty() => format!("{} · {detail}", self.name),
            _ => self.name.clone(),
        }
    }

    /// The forms the header may show, widest first: with the detail, then the
    /// name alone. None for an empty caption.
    fn forms(&self) -> Vec<String> {
        if self.name.is_empty() {
            return Vec::new();
        }
        let mut forms = vec![self.full()];
        if self.detail.is_some() {
            forms.push(self.name.clone());
        }
        forms
    }
}

/// What the feed's header holds in `avail` columns: the chat's title — cut,
/// with the "…" a cut carries, when it has to be — and the caption beside it.
///
/// **The title outranks the caption** (spec §11.3): the title is owed half the
/// row, or all of itself when it is shorter, and the caption is shown in the
/// widest form that leaves that much — first without its detail, then not at
/// all. The caption used to be whole at any width and the title got the
/// remainder: a 45-column window showed five letters of the title beside a
/// 33-column model name, and a 30-column one showed no title and a caption
/// clipped from its left (docs/research/small-terminal.md §2.3). Where the row
/// has room for both, nothing changes — a long title is still the one cut.
/// The rule is every border's with two titles ([`ui::fit_title_pair`]).
fn fit_header(title: &str, caption: &FeedCaption, avail: usize) -> (String, Option<String>) {
    ui::fit_title_pair(title, &caption.forms(), avail)
}

/// The widest of the token counter's `forms` whose cell — the form with a
/// space on either side — fits in `avail` columns of the bottom border; the
/// notes are given up first, then the counter (spec §11.3). It has the row to
/// itself, so nothing else is owed a share of it.
fn fit_counter(forms: &[String], avail: usize) -> Option<&str> {
    forms
        .iter()
        .map(String::as_str)
        .find(|form| wrap::str_width(form) + 2 <= avail)
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
            code_block_background: CodeBlockBackground::default(),
            show_model_name: false,
            role_names: CharacterNames::default(),
            compaction: None,
            known_chats: Vec::new(),
            // The screen starts at `Connecting`, so nothing flashes before the
            // orchestrator's first status arrives.
            engine_missing: false,
            link_hits: Vec::new(),
            fold_hits: Vec::new(),
            tap_hits: Vec::new(),
            variants: std::collections::HashMap::new(),
            cache: Vec::new(),
            cache_key: None,
            pending: None,
            marker: None,
            highlight: None,
            highlight_scope: HighlightScope::default(),
            matches: Vec::new(),
            current_match: 0,
            scroll_to_match: false,
            cache_reset: false,
            drawn: None,
            selection: None,
            gesture: None,
            selectable: true,
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

    /// Sets the background of an unhighlighted code block (setting
    /// `interface.code_block_background`, spec §11.4). Changing the value
    /// invalidates the cache via [`CacheKey`].
    pub fn set_code_block_background(&mut self, background: CodeBlockBackground) {
        self.code_block_background = background;
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

    /// Sets the variants marks of the line's bubbles
    /// (`AppEvent::Branches`, docs/research/chat-branches.md §3.7). A mark is
    /// a property of a block's place like its number, so it is checked beside
    /// the block's fingerprint and not in [`CacheKey`]: a fork or a switch
    /// rebuilds the blocks whose mark changed, and no other.
    pub fn set_variants(&mut self, marks: &[VariantMark]) {
        self.variants = marks
            .iter()
            .map(|m| (m.message, (m.variant, m.of)))
            .collect();
    }

    /// What a click at this terminal cell is on, as last drawn: a message's
    /// number, or an arrow of its variants mark — `None` anywhere else.
    pub fn tap_at(&self, col: u16, row: u16) -> Option<Tap> {
        self.tap_hits
            .iter()
            .find(|h| h.row == row && (h.start..h.end).contains(&col))
            .map(|h| h.tap)
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

    /// Whether no chat engine is configured — an empty feed then explains the
    /// ways to connect one (see [`Self::engine_missing`]).
    pub fn set_engine_missing(&mut self, missing: bool) {
        self.engine_missing = missing;
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

    /// Toggles showing "thoughts" blocks (`Ctrl+T`) — every one of them, the
    /// ones a click flipped included ([`FeedView::toggle_thoughts`]).
    pub fn toggle_thoughts(&mut self) {
        self.view.toggle_thoughts();
    }

    /// Toggles showing tool-call arguments and results (`Ctrl+O`), every card's.
    /// The card's header stays visible either way — see [`push_tool`].
    pub fn toggle_tools(&mut self) {
        self.view.toggle_tools();
    }

    /// The foldable block whose pill is at this terminal cell, as last drawn —
    /// `None` anywhere else, and on the reply being written, which has no keys
    /// yet (docs/research/block-fold-and-partial-copy.md §3.4).
    pub fn fold_at(&self, col: u16, row: u16) -> Option<FoldKey> {
        self.fold_hits
            .iter()
            .find(|h| h.row == row && (h.start..h.end).contains(&col))
            .map(|h| h.key)
    }

    /// A click on a block's pill: that block alone changes state, and only its
    /// own cached block is rebuilt. The feed stops following its tail, so the
    /// rows above the pill — unchanged by the toggle — keep it where it was
    /// clicked; where the content becomes shorter than the view, the clamp moves
    /// the view (§3.4).
    pub fn toggle_block(&mut self, key: FoldKey) {
        self.view.toggle_block(key);
        self.follow = false;
    }

    /// The current collapse state — sent back to the orchestrator after a
    /// toggle, so it is stored on the chat (spec §11.3).
    pub fn view(&self) -> FeedView {
        self.view.clone()
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
        self.release_pending();
    }

    /// Scroll down (turns "follow" back on at the very bottom).
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
        self.scrolled = true;
        self.release_pending();
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
        self.release_pending();
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

    /// Puts the view on the feed's first row — a conversation opened to be
    /// read from its start (docs/research/feed-landing.md §3.2). A feed that
    /// fits the panel is at its last row too, and follows its tail again from
    /// the next frame.
    pub fn scroll_to_top(&mut self) {
        self.scroll = 0;
        self.follow = false;
        self.scrolled = true;
        self.release_pending();
    }

    /// Puts the view on the feed item holding the domain message `id`: the next
    /// render scrolls to it, and its rail is drawn in the accent color until the
    /// chat changes or another jump replaces it. Returns whether the message was
    /// found — an unknown/hidden id (a `Tool`/`System` message, or one from
    /// another chat) is a no-op.
    ///
    /// Resolving the id lives here, not on the caller: the feed **index** is
    /// this widget's coordinate system (`scroll`, `cache`, a pending request and
    /// `marker` are all keyed by it), the messages are already handed to every other
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
        let Some(idx) = block_of(messages, id) else {
            return false;
        };
        self.marker = Some(idx);
        self.highlight = highlight.filter(|q| !q.is_empty()).map(str::to_owned);
        // With a query the view goes to the match, which only the frame that
        // highlights it can find; without one, to the message's first row.
        self.land(match self.highlight {
            Some(_) => Pending::Hit { block: idx },
            None => Pending::Row { block: idx, row: 0 },
        });
        true
    }

    /// Takes a request for where the next frame puts the view.
    ///
    /// **The feed stops following its tail here, not in the frame that
    /// applies the request.** Between the two, content can arrive on its own —
    /// the note that says which variant is on the line, or which parked
    /// branch a hit is in, comes right after the activation that asked — and
    /// what arrives on its own scrolls a following feed to its end
    /// ([`Self::scroll_to_bottom_if_following`]), which drops a pending
    /// request. A feed that has been asked to stand somewhere is not
    /// following.
    fn land(&mut self, pending: Pending) {
        self.pending = Some(pending);
        self.follow = false;
    }

    /// The view's first row in the frame last drawn, as a place that can be
    /// found again after the feed is rebuilt (docs/research/feed-landing.md
    /// §3.4). `messages` is the feed that frame drew.
    ///
    /// A block with no message behind it — a note, a transcript's persona, a
    /// reply still being written — is not a place: the next bubble below that
    /// has one stands in, at its first row. `None` before the first frame,
    /// over an empty feed, and where nothing below the view's first row is a
    /// stored message — the feed's last rows, which the tail already names.
    pub fn place(&self, messages: &[FeedMessage]) -> Option<FeedPlace> {
        let drawn = self.drawn.as_ref()?;
        let (block, row) = drawn.locate(drawn.scroll);
        if let Some(&message) = messages.get(block).and_then(|m| m.message_ids.first()) {
            return Some(FeedPlace { message, row });
        }
        messages
            .iter()
            .skip(block + 1)
            .find_map(|m| m.message_ids.first())
            .map(|&message| FeedPlace { message, row: 0 })
    }

    /// Puts the view back on a place read earlier ([`Self::place`]): the next
    /// render scrolls to that row of that message's block, clamped to it — the
    /// row is only right at the width it was read at. Nothing is marked — the
    /// person is back where they were, not somewhere they were sent. `false`
    /// when the feed does not show the message.
    pub fn restore_place(&mut self, messages: &[FeedMessage], place: FeedPlace) -> bool {
        let Some(block) = block_of(messages, place.message) else {
            return false;
        };
        self.land(Pending::Row {
            block,
            row: place.row,
        });
        true
    }

    /// Shows the bubble holding the message `id`, whose content has just
    /// changed under the person — a variant switch (research §3.3). `held` is
    /// the view's first row from before the feed was rebuilt: the next render
    /// puts the view back there, and leaves it there when that shows the
    /// bubble's first row — the mark that was clicked stays under the pointer.
    /// Where it does not, or the held row is gone, the bubble's first row
    /// becomes the view's first. Either way the feed stops following its tail
    /// until the view is at the last row again. `false` when the feed does not
    /// show the message.
    pub fn reveal_message(
        &mut self,
        messages: &[FeedMessage],
        id: Uuid,
        held: Option<FeedPlace>,
    ) -> bool {
        let Some(block) = block_of(messages, id) else {
            return false;
        };
        let held = held.and_then(|p| block_of(messages, p.message).map(|b| (b, p.row)));
        self.land(Pending::Reveal { block, held });
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

    /// Drops a pending request, the marker **and the highlight**: the feed
    /// they index is gone (a chat switch). Cheap here — a rebuilt feed misses
    /// every fingerprint anyway, so the cache was going to be rebuilt regardless.
    pub fn clear_focus(&mut self) {
        self.pending = None;
        self.marker = None;
        self.highlight = None;
    }

    /// Drops a request no frame has applied yet, keeping the marker: the user
    /// is steering now, so the view goes where they scroll it — but they can
    /// still see where they landed.
    fn release_pending(&mut self) {
        self.pending = None;
    }

    /// Where the view must be put **this frame**: a pending request
    /// (consumed), or — after a rewrap invalidated every row offset — what was
    /// on the view's first row before it. Both can only become a row inside
    /// `render` (§1.1).
    ///
    /// **A rewrap keeps what is on the screen** (docs/research/feed-landing.md,
    /// fork F2). `scroll` is a row number, and a resize, a theme, `Ctrl+T`,
    /// `Ctrl+O` or a new compaction boundary gives every block other rows: the
    /// number then names some other text. The frame last drawn says which
    /// block that row was in and which line of it ([`DrawnRows`]), so the view
    /// is put back on that line of that block ([`TopLine`]) — or, where the
    /// line cannot be told any more, as far through the block as it was. This
    /// used to be done for a jump alone, by an index it left behind and a
    /// manual scroll released, so a reader who had scrolled anywhere was moved
    /// by a resize. The row is derived from the last frame at the moment it is
    /// needed and never stored (fork S6's objection was to storing one).
    fn take_focus_target(&mut self) -> Option<Pending> {
        let rewrapped = std::mem::take(&mut self.cache_reset);
        if let Some(pending) = self.pending.take() {
            return Some(pending);
        }
        // Following the tail needs no keeping — the clamp already holds it there.
        if !rewrapped || self.follow {
            return None;
        }
        // `scroll`, not the frame's own: a scroll no frame has drawn yet is in
        // the old rows too, and so is the first row a new chat was opened at.
        let drawn = self.drawn.as_ref()?;
        let at = self.scroll.min(drawn.max_scroll);
        let (block, row) = drawn.locate(at);
        Some(Pending::Kept {
            block,
            row,
            of: drawn.rows_of(block),
            // The line the frame recorded is the line of the row it drew first.
            line: drawn.top.filter(|_| at == drawn.scroll),
        })
    }

    /// Where a block's rows begin in the feed's rows as the cache holds them,
    /// and how many it has. `None` for an index past the feed.
    fn block_rows(&self, block: usize) -> Option<(usize, usize)> {
        let rows = self.cache.get(block)?.lines.len();
        let start = self.cache.iter().take(block).map(|b| b.lines.len()).sum();
        Some((start, rows))
    }

    /// The scroll row a pending request comes to, against the cache
    /// `build_lines` has just filled; `view_h` is the panel's height in rows.
    fn pending_row(&self, pending: Pending, view_h: usize) -> Option<usize> {
        match pending {
            Pending::Row { block, row } => {
                let (start, rows) = self.block_rows(block)?;
                Some(start + row.min(rows.saturating_sub(1)))
            }
            Pending::Reveal { block, held } => {
                let (first, _) = self.block_rows(block)?;
                let held = held
                    .and_then(|(block, row)| self.pending_row(Pending::Row { block, row }, view_h));
                Some(match held {
                    Some(top) if (top..top + view_h).contains(&first) => top,
                    _ => first,
                })
            }
            Pending::Hit { block } => {
                let (first, rows) = self.block_rows(block)?;
                // The matches `build_lines` has just highlighted, as rows of
                // the feed; a jump's are all inside the block it marked.
                let hit = self
                    .matches
                    .iter()
                    .copied()
                    .find(|row| (first..first + rows).contains(row));
                Some(match hit {
                    // Below the screenful that starts with the message: the
                    // match a third of the view down, its lead-in above it.
                    Some(row) if row >= first + view_h => row - view_h / 3,
                    // On that screenful — or not drawn at all (folded away,
                    // or text the renderer transformed): the message's start.
                    _ => first,
                })
            }
            Pending::Kept {
                block,
                row,
                of,
                line,
            } => {
                let (start, rows) = self.block_rows(block)?;
                let row = line
                    .and_then(|line| line.row_in(self.cache[block].said(), rows))
                    .unwrap_or_else(|| rewrapped_row(row, of, rows));
                Some(start + row.min(rows.saturating_sub(1)))
            }
        }
    }

    /// Whether the feed is following its tail: at its last row, and staying
    /// there as content arrives.
    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Whether a request for where the view stands is waiting for the frame
    /// that applies it ([`Self::pending`]).
    pub fn awaits_frame(&self) -> bool {
        self.pending.is_some()
    }

    /// The message a jump's mark is on — the first of its bubble — in the feed
    /// the mark indexes. Read before that feed is rebuilt in place, and given
    /// back to [`Self::remark`] after.
    pub fn marked(&self, messages: &[FeedMessage]) -> Option<Uuid> {
        messages.get(self.marker?)?.message_ids.first().copied()
    }

    /// Puts a jump's mark back on its message in a feed rebuilt in place — a
    /// refresh of the open chat (docs/research/feed-landing.md §3.6). The mark
    /// is a feed index, and the rebuild dropped the feed's notes, so the
    /// message is found again by id. A mark whose message is gone goes, and
    /// the jump's highlight with it; an in-feed search's highlight is the
    /// whole feed's and stays.
    pub fn remark(&mut self, messages: &[FeedMessage], marked: Option<Uuid>) {
        self.marker = marked.and_then(|id| block_of(messages, id));
        if self.marker.is_none() && self.highlight_scope == HighlightScope::MarkedOnly {
            self.highlight = None;
        }
    }

    /// Test accessor: the current scroll offset in visual rows.
    #[cfg(test)]
    pub(crate) fn scroll_row(&self) -> usize {
        self.scroll
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

    // ---------- the selection (docs/research/mouse-always-captured.md §5.3–§5.4) ----------

    /// Whether `(col, row)` is on the feed's rows as last drawn — the panel a
    /// press there belongs to. `false` over an empty feed.
    pub fn holds(&self, col: u16, row: u16) -> bool {
        self.drawn.as_ref().is_some_and(|d| d.holds(col, row))
    }

    /// A left press at `(col, row)`, `unit` from the click count. A press a
    /// character at a time changes nothing yet — the drag after it selects; a
    /// double or a triple press selects the word or the logical line under it
    /// at once. `false` when it began nothing: off the feed's rows, or nothing
    /// selectable (the assistant answering, research F6).
    pub fn press(&mut self, col: u16, row: u16, unit: SelectUnit) -> bool {
        if !self.selectable {
            return false;
        }
        let Some(p) = self.point_at(col, row, false) else {
            return false;
        };
        // A double or a triple click beside the text — past a row's last
        // glyph, on a blank row, on the rail — selects nothing: it is the
        // click that ends a selection, however fast it came after the last.
        let unit = if self.on_text(p) {
            unit
        } else {
            SelectUnit::Char
        };
        let rows = Blocks(&self.cache);
        let origin = match unit {
            SelectUnit::Char => (p, p),
            SelectUnit::Word => feed_selection::word_bounds(&rows, p),
            SelectUnit::Line => feed_selection::line_bounds(&rows, p),
        };
        if unit != SelectUnit::Char {
            self.selection = Some(origin);
        }
        self.gesture = Some(Gesture {
            unit,
            origin,
            pointer: None,
            next_step: None,
        });
        true
    }

    /// A left drag of the gesture [`Self::press`] began, the pointer at
    /// `(col, row)`: the selection runs from what the press took to the
    /// pointer, replacing the one before it. The pointer is clamped to the
    /// panel — a selection never leaves the panel its press was in — and one
    /// past its top or bottom edge scrolls it ([`Self::autoscroll`]). `true`
    /// when the selection changed.
    pub fn drag(&mut self, col: u16, row: u16) -> bool {
        let Some(gesture) = self.gesture.as_mut() else {
            return false;
        };
        if gesture.pointer == Some((col, row)) {
            return false;
        }
        gesture.pointer = Some((col, row));
        self.follow_pointer()
    }

    /// The left button released: the gesture is over, the selection stays.
    pub fn release(&mut self) {
        self.gesture = None;
    }

    /// One step of a drag held past the panel's top or bottom edge: the feed
    /// scrolls a row toward it, at most once an [`AUTOSCROLL_STEP`], and the
    /// selection's head follows in the frame this asks for. The terminal sends
    /// nothing while the pointer rests, so the loop's tick drives this. `true`
    /// when it scrolled.
    pub fn autoscroll(&mut self, now: std::time::Instant) -> bool {
        let (Some(gesture), Some(drawn)) = (self.gesture.as_ref(), self.drawn.as_ref()) else {
            return false;
        };
        let Some((_, row)) = gesture.pointer else {
            return false;
        };
        let up = row < drawn.inner.y;
        let down = row >= drawn.inner.y.saturating_add(drawn.inner.height);
        let room = if up {
            self.scroll > 0
        } else {
            self.scroll < drawn.max_scroll
        };
        if !(up || down) || !room || gesture.next_step.is_some_and(|due| now < due) {
            return false;
        }
        if let Some(gesture) = self.gesture.as_mut() {
            gesture.next_step = Some(now + AUTOSCROLL_STEP);
        }
        if up {
            self.scroll_up(1);
        } else {
            self.scroll_down(1);
        }
        true
    }

    /// Whether the feed has a selection — what `Ctrl+C` copies when it does.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// Whether the cell under `(col, row)` is **highlighted** by the selection:
    /// a click there keeps it, one anywhere else in the feed ends it (research
    /// §5.3). What is painted, not what lies between the selection's ends in
    /// reading order: that span also holds the empty cells beside every row it
    /// crosses, and a click there kept a selection the user saw nothing of
    /// under the pointer (the owner's live check, 2026-11-15).
    pub fn selection_holds(&self, col: u16, row: u16) -> bool {
        let (Some((start, end)), Some(p)) = (self.selection, self.point_at(col, row, false)) else {
            return false;
        };
        feed_selection::painted_on(&Blocks(&self.cache), start, end, p.block, p.row)
            .is_some_and(|cols| cols.contains(&p.col))
    }

    /// Whether the feed's cell `p` is on its row's text
    /// ([`feed_selection::content_cols`]).
    fn on_text(&self, p: FeedPoint) -> bool {
        let block = &self.cache[p.block];
        feed_selection::content_cols(&block.lines[p.row], block.facts[p.row])
            .is_some_and(|cols| cols.contains(&p.col))
    }

    /// Ends the selection and any gesture making one. `true` when there was a
    /// selection — a frame is due.
    pub fn clear_selection(&mut self) -> bool {
        self.gesture = None;
        self.selection.take().is_some()
    }

    /// Whether a selection can be made — the chat screen says not while the
    /// assistant answers (research F6), and the one standing is dropped then.
    pub fn set_selectable(&mut self, on: bool) {
        self.selectable = on;
        if !on {
            self.clear_selection();
        }
    }

    /// The text a copy takes from the selection: what is drawn, less the rail,
    /// with the rows of a wrapped line joined back into it
    /// ([`feed_selection::text`]). `None` without a selection.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection?;
        Some(feed_selection::text(&Blocks(&self.cache), start, end))
    }

    /// The feed's cell under the screen cell `(col, row)` of the last drawn
    /// frame. `clamp` — a pointer off the panel counts as on its nearest edge
    /// (a drag); without it a cell off the panel is `None` (a press). A cell
    /// under the last row of a short feed is on its last row.
    fn point_at(&self, col: u16, row: u16, clamp: bool) -> Option<FeedPoint> {
        let drawn = self.drawn.as_ref()?;
        let inner = drawn.inner;
        if drawn.total == 0
            || inner.width == 0
            || inner.height == 0
            || (!clamp && !drawn.holds(col, row))
        {
            return None;
        }
        let x = col.clamp(inner.x, inner.x + inner.width.saturating_sub(1)) - inner.x;
        let y = usize::from(row.saturating_sub(inner.y)).min(usize::from(inner.height) - 1);
        let abs = (drawn.scroll + y).min(drawn.total - 1);
        let (block, r) = drawn.locate(abs);
        Some(FeedPoint::new(block, r, x))
    }

    /// Resolves a drag's head where its pointer is in the last drawn frame.
    /// `true` when the selection changed.
    fn follow_pointer(&mut self) -> bool {
        let Some(gesture) = self.gesture else {
            return false;
        };
        let Some((col, row)) = gesture.pointer else {
            return false;
        };
        let Some(head) = self.point_at(col, row, true) else {
            return false;
        };
        let span = feed_selection::grow(&Blocks(&self.cache), gesture.unit, gesture.origin, head);
        let changed = self.selection != Some(span);
        self.selection = Some(span);
        changed
    }

    /// Drops the selection and the gesture when either reaches block `from`
    /// or past it — the feed no longer has those blocks.
    fn forget_selection_from(&mut self, from: usize) {
        if self.selection.is_some_and(|(_, end)| end.block >= from)
            || self.gesture.is_some_and(|g| g.origin.1.block >= from)
        {
            self.clear_selection();
        }
    }

    /// Drops the selection and the gesture when either covers block `idx`,
    /// whose rows are being made anew.
    fn forget_selection_in(&mut self, idx: usize) {
        let covers =
            |(start, end): (FeedPoint, FeedPoint)| (start.block..=end.block).contains(&idx);
        if self.selection.is_some_and(covers) || self.gesture.is_some_and(|g| covers(g.origin)) {
            self.clear_selection();
        }
    }

    /// Paints the selection's visible cells on the frame just drawn — the
    /// selection backdrop, or in the monochrome mode the mark the frame's
    /// last pass keeps as reverse video ([`Palette::selection`]). From the
    /// content's first column, past the rail, to each row's last glyph
    /// ([`feed_selection::painted`]).
    fn paint_selection(&self, buf: &mut ratatui::buffer::Buffer, palette: &Palette) {
        let (Some((start, end)), Some(drawn)) = (self.selection, self.drawn.as_ref()) else {
            return;
        };
        let rows = Blocks(&self.cache);
        let inner = drawn.inner;
        let last = (drawn.scroll + usize::from(inner.height)).min(drawn.total);
        for abs in drawn.scroll..last {
            let (block, row) = drawn.locate(abs);
            let Some(cols) = feed_selection::painted_on(&rows, start, end, block, row) else {
                continue;
            };
            let y = inner.y + (abs - drawn.scroll) as u16;
            for col in cols.start..cols.end.min(inner.width) {
                if let Some(cell) = buf.cell_mut((inner.x + col, y)) {
                    let style = palette.selection(cell.style());
                    cell.set_style(style);
                }
            }
        }
    }

    /// Draws the feed. `messages` — the active chat's current content;
    /// `header` — the panel's border with the chat's title and the model
    /// caption on it, or nothing around the rows ([`FeedFrame`]).
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        header: FeedFrame,
        messages: &[FeedMessage],
        palette: &Palette,
        loc: &'static Locale,
    ) {
        let inner = match header {
            FeedFrame::Bordered {
                title,
                caption,
                counter,
            } => Self::render_border(frame, area, title, caption, counter, palette, loc),
            FeedFrame::Bare => area,
        };
        let bare = matches!(header, FeedFrame::Bare);

        // Every row of a message's block is already wrapped to the feed's width
        // (`build_message_block`): the number of visual rows matches
        // `lines.len()`, and the scroll/"follow the tail" math below stays
        // row-based (see shared::wrap, ADR 0001). Only the empty feed's
        // placeholder is wrapped here.
        let view_w = inner.width.max(1) as usize;
        let unwrapped = self.build_lines(messages, palette, view_w, loc);

        // A jump, a place restored, a reveal (or the view's own first row,
        // which a rewrap just invalidated) becomes a scroll row here and nowhere else — the
        // cache it is measured against only exists after `build_lines` at this
        // width (§1.1).
        //
        // `build_lines` concatenates `cache[i].lines` in order, so the per-block
        // lengths address the stream exactly. The rows are drawn as the blocks
        // hold them — a contract since the feed's selection, which addresses a
        // row as (block, row) (docs/research/mouse-always-captured.md §5.4).
        // An empty feed has no blocks, so its placeholder takes no request.
        let view_h = inner.height.max(1) as usize;
        let target_row = self
            .take_focus_target()
            .and_then(|pending| self.pending_row(pending, view_h));
        // The current in-feed match rides the same accumulation: its row is
        // derived here, every frame, never stored — which is what lets it survive
        // a rewrap where a stored offset could not (§1.3).
        let match_line = std::mem::take(&mut self.scroll_to_match)
            .then(|| self.matches.get(self.current_match).copied())
            .flatten();
        let placeholder = messages.is_empty();
        let mut lines: Vec<Line> = Vec::with_capacity(unwrapped.len());
        let mut match_row: Option<usize> = None;
        for (i, line) in unwrapped.into_iter().enumerate() {
            if match_line == Some(i) {
                match_row = Some(lines.len());
            }
            if placeholder {
                lines.extend(wrap::wrap_line(&line, view_w));
            } else {
                lines.push(line);
            }
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
        // The tail of the stream is the separator under the last message — a
        // blank row. A bare feed has few rows (spec §11.1.1) and ends on the
        // last line of text instead: the tail it follows is the stream less
        // that row, so a four-row feed shows four rows of the conversation.
        let tail_pad = usize::from(bare && lines.last().is_some_and(|l| l.width() == 0));
        let max_scroll = (total - tail_pad).saturating_sub(view_h);

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
        self.fold_hits = if placeholder {
            Vec::new()
        } else {
            visible_fold_hits(&self.cache, inner, self.scroll)
        };
        self.tap_hits = if placeholder {
            Vec::new()
        } else {
            visible_tap_hits(&self.cache, inner, self.scroll)
        };
        // The selection is read against the same frame (research §5.3).
        self.drawn = (!placeholder).then(|| {
            let mut starts = Vec::with_capacity(self.cache.len());
            let mut at = 0usize;
            for block in &self.cache {
                starts.push(at);
                at += block.lines.len();
            }
            let mut drawn = DrawnRows {
                inner,
                scroll: self.scroll,
                max_scroll,
                starts,
                total,
                top: None,
            };
            // A feed that follows its tail is kept by the clamp, not by a
            // line: nothing to read for it.
            if !self.follow {
                let (block, row) = drawn.locate(self.scroll);
                drawn.top = self
                    .cache
                    .get(block)
                    .and_then(|b| TopLine::of(b.said(), b.lines.len(), row));
            }
            drawn
        });
        if placeholder {
            self.selection = None;
            self.gesture = None;
        }
        // A drag's head is where its pointer is in *this* frame: a wheel or an
        // autoscroll since the last one moved the rows under it.
        self.follow_pointer();

        let paragraph = Paragraph::new(Text::from(lines)).scroll((self.scroll as u16, 0));
        frame.render_widget(paragraph, inner);
        // Painted over the frame, outside the block cache like the search
        // highlight, so a selection costs no re-render.
        self.paint_selection(frame.buffer_mut(), palette);

        // Scrollbar on the panel's right border (corners untouched) — only when
        // the feed doesn't fit vertically. Doesn't take width away from content; the
        // feed's border is always non-focused (see `border_style(false)` above).
        // A bare feed has no border column for it.
        if !bare {
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
    }

    /// The bordered feed's panel: rounded, the title with a ◆ marker on the
    /// left, the caption (model/ctx) on the right, the token counter at the
    /// bottom right corner. Returns the rows inside.
    fn render_border(
        frame: &mut Frame,
        area: Rect,
        title: &str,
        caption: &FeedCaption,
        counter: Option<TokenCounter>,
        palette: &Palette,
        loc: &'static Locale,
    ) -> Rect {
        let glyphs = palette.glyphs();
        let marker = format!(" {} ", glyphs.title_marker);
        // The border is a fixed row: the title and the caption share what the
        // marker and the corners leave ([`fit_header`]). A title is not bounded
        // in storage; every surface that draws one in a row bounds it itself
        // (`shared::title::sanitize_title`, spec §11.2).
        let avail = (area.width as usize).saturating_sub(2 + wrap::str_width(&marker) + 1);
        let (title, caption) = fit_header(title, caption, avail);
        let meta_cell = caption.map(|form| format!(" {form} "));
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
        // The counter has the bottom row to itself: the corners and one run
        // of the border before it are all it leaves.
        if let Some(counter) = counter {
            let forms = counter.forms(loc);
            let avail = (area.width as usize).saturating_sub(3);
            if let Some(form) = fit_counter(&forms, avail) {
                block = block.title_bottom(
                    Line::from(Span::styled(format!(" {form} "), counter.style(palette)))
                        .right_aligned(),
                );
            }
        }
        let inner = block.inner(area);
        frame.render_widget(&block, area);
        inner
    }

    /// What an empty feed says: an invitation to write, or — with no engine to
    /// answer — the four ways to start. One logical line per route; `render`
    /// wraps them to the panel like any other line.
    fn empty_placeholder(&self, palette: &Palette, loc: &'static Locale) -> Vec<Line<'static>> {
        let line =
            |key: &str| Line::from(Span::styled(loc.t(key).to_string(), palette.muted_style()));
        if !self.engine_missing {
            return vec![line("ui.feed.empty")];
        }
        vec![
            line("ui.feed.no_engine.title"),
            Line::default(),
            line("ui.feed.no_engine.cloud"),
            line("ui.feed.no_engine.server"),
            line("ui.feed.no_engine.local"),
            line("ui.feed.no_engine.demo"),
        ]
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
            return self.empty_placeholder(palette, loc);
        }
        // Reset the cache on a change to width/palette/collapse state/language (affects blocks).
        let key = CacheKey {
            width,
            palette: *palette,
            thoughts: self.view.thoughts,
            tools: self.view.tools,
            table_row_separators: self.table_row_separators,
            render_mermaid: self.render_mermaid,
            code_block_background: self.code_block_background,
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
            // re-derives the view's from this flag.
            self.cache_reset = true;
            // So is every point of a selection: the rows it named are rewrapped
            // (research F9).
            self.clear_selection();
        }
        // History truncated (Ctrl+E/regenerate) — drop the cache's tail.
        self.cache.truncate(messages.len());
        self.forget_selection_from(messages.len());

        // Base markdown-render flags from the feed's settings; `soft_break_as_newline`
        // stays per-role (set by push_body for user messages).
        let opts = markdown::RenderOpts {
            table_row_separators: self.table_row_separators,
            render_mermaid: self.render_mermaid,
            code_block_background: self.code_block_background,
            ..Default::default()
        };
        // Resolved once per frame, before the loop: the id → block mapping is a
        // property of the whole projection, not of one block.
        let boundary = self.boundary_index(messages);
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut found: Vec<usize> = Vec::new();
        for ((idx, item), number) in messages.iter().enumerate().zip(numbers(messages)) {
            let place = BlockPlace {
                at_boundary: boundary == Some(idx),
                number,
                variant: item
                    .message_ids
                    .first()
                    .and_then(|id| self.variants.get(id).copied()),
            };
            self.refresh_cached_block(idx, item, palette, width, opts, loc, place);
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
                let block = &self.cache[idx];
                let from = at + block.content_from;
                // `lines[from..]` is this block's tail: later blocks are not
                // appended yet, so the slice cannot reach them.
                let facts = &block.facts[block.content_from..];
                highlight_block_tail(&mut lines, from, facts, query, palette, &mut found);
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
    /// `place.at_boundary` — this block is where verbatim history resumes, so
    /// it is preceded by the compaction divider. Passed as a flag rather than
    /// as the summary itself so the caller doesn't have to hold a borrow of
    /// `self` across the `&mut self` call; the text is read here. The summary
    /// rides [`CacheKey`], so a changed one clears the cache and the block is
    /// rebuilt — the fingerprint (a property of the *message*) stays out of it.
    /// So does `place.number`, which is checked beside it.
    #[allow(clippy::too_many_arguments)]
    fn refresh_cached_block(
        &mut self,
        idx: usize,
        item: &FeedMessage,
        palette: &Palette,
        width: usize,
        opts: markdown::RenderOpts,
        loc: &'static Locale,
        place: BlockPlace,
    ) {
        let fp = message_fingerprint(item);
        let boundary = place
            .at_boundary
            .then(|| self.compaction.as_ref().map(|(id, _)| *id))
            .flatten();
        let folds = fold_state(item, &self.view, boundary);
        let hit = self.cache.get(idx).is_some_and(|c| {
            c.fingerprint == fp
                && c.folds == folds
                && c.number == place.number
                && c.variant == place.variant
        });
        if !hit {
            // The block's rows are about to be made anew: a selection in them
            // no longer points at what it took.
            if self.cache.get(idx).is_some() {
                self.forget_selection_in(idx);
            }
            let summary =
                boundary.and_then(|id| self.compaction.as_ref().map(|(_, s)| (id, s.as_str())));
            // A streaming/changed message, or one a click folded — recompute
            // only its block.
            let built = build_message_block(
                item,
                palette,
                width,
                &self.view,
                opts,
                loc,
                &self.role_names,
                self.marker == Some(idx),
                summary,
                &self.known_chats,
                self.show_model_name,
                place.number,
                place.variant,
            );
            let cb = CachedBlock {
                fingerprint: fp,
                folds,
                number: place.number,
                variant: place.variant,
                taps: built.taps,
                said: std::cell::OnceCell::new(),
                lines: built.lines,
                facts: built.facts,
                links: built.links,
                pills: built.pills,
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
    facts: &[RowFact],
    query: &str,
    palette: &Palette,
    found: &mut Vec<usize>,
) {
    for (i, line) in lines[from..].iter_mut().enumerate() {
        // A message's number is the last span of the row it ends
        // ([`push_number`]) and no part of the message: a query of its digits
        // must not light it, or count it as a match.
        let number = facts
            .get(i)
            .is_some_and(|f| f.tail > 0)
            .then(|| line.spans.pop())
            .flatten();
        // One entry per occurrence, in document order: a single line
        // can hold several, and next/prev steps through matches, not
        // lines.
        for _ in 0..highlight_line(line, query, palette) {
            found.push(from + i);
        }
        line.spans.extend(number);
    }
}

/// A message's lines before the feed's wrap, each with its [`LineMark`]: the
/// columns of a prefix drawn as text — a quote's `> `, the `│ ` gutter of the
/// thoughts and the summary, a tool card's indent and icon — and whether the
/// line continues the one above, where a builder wrapped it itself
/// (docs/research/mouse-always-captured.md §5.4, stage 2b). Every builder
/// pushes through it, so the lines and their marks cannot drift apart; the
/// block's wrap turns them into [`RowFact`]s.
#[derive(Default)]
struct Body {
    lines: Vec<Line<'static>>,
    marks: Vec<LineMark>,
    /// Which spans of which lines are a foldable block's pill — the click
    /// target (docs/research/block-fold-and-partial-copy.md §3.4–§3.5).
    pills: Vec<PillMark>,
    /// The line whose first row ends on the message's number, when one is
    /// drawn (docs/research/chat-branches.md §3.5).
    numbered: Option<NumberedLine>,
}

/// The line of a [`Body`] that carries the message's number.
#[derive(Clone, Copy)]
struct NumberedLine {
    line: usize,
    /// The line is the message's own text — a note row has no header — so it
    /// is wrapped again, narrower, when its first row leaves the number no
    /// room. A header is fitted before it gets here and is never rewrapped.
    reflow: bool,
}

/// A pill in a [`Body`]: the spans of one of its lines a click folds the block
/// by.
struct PillMark {
    line: usize,
    spans: Range<usize>,
    key: FoldKey,
}

/// A pill in a built block: its row there and its columns in the panel's
/// content (the rail included), half-open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PillHit {
    row: usize,
    start: u16,
    end: u16,
    key: FoldKey,
}

/// A pill on the screen, in terminal cells — [`MessageFeed::fold_hits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FoldHit {
    row: u16,
    start: u16,
    end: u16,
    key: FoldKey,
}

impl Body {
    /// Marks `spans` of line `line` as the pill of the block `key` names. A
    /// block with no key — the reply being written — draws its pill and takes
    /// no click.
    fn mark_pill(&mut self, line: usize, spans: Range<usize>, key: Option<FoldKey>) {
        if let Some(key) = key {
            self.pills.push(PillMark { line, spans, key });
        }
    }

    /// A line with no prefix that continues nothing.
    fn push(&mut self, line: Line<'static>) {
        self.push_marked(line, LineMark::default());
    }

    fn push_marked(&mut self, line: Line<'static>, mark: LineMark) {
        self.lines.push(line);
        self.marks.push(mark);
    }

    /// A markdown render's lines with the marks it made.
    fn extend_marked(&mut self, rendered: markdown::Marked) {
        self.lines.extend(rendered.lines);
        self.marks.extend(rendered.marks);
    }

    fn last(&self) -> Option<&Line<'static>> {
        self.lines.last()
    }

    fn last_mut(&mut self) -> Option<&mut Line<'static>> {
        self.lines.last_mut()
    }

    fn len(&self) -> usize {
        self.lines.len()
    }
}

/// The marks of `n` rows a builder wrapped one source line into, under a
/// prefix `prefix` columns wide on each: the first row continues what the line
/// did, every later one continues the row above.
fn wrapped_marks(prefix: usize, first: LineMark) -> impl Fn(usize) -> LineMark {
    move |k| LineMark {
        prefix: prefix as u16,
        continues: first.continues || k > 0,
        glue: if k == 0 { first.glue } else { None },
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
/// (`interface.show_model_name`, spec §11.3). `number` — the message's number
/// ([`numbers`]), drawn at the right end of the row that opens it: the
/// header's, or a note's first (docs/research/chat-branches.md §3.5).
#[allow(clippy::too_many_arguments)]
fn build_message_block(
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    view: &FeedView,
    opts: markdown::RenderOpts,
    loc: &'static Locale,
    names: &CharacterNames,
    marked: bool,
    compaction: Option<(Uuid, &str)>,
    known_chats: &[Uuid],
    show_model: bool,
    number: Option<u32>,
    variant: Option<(u16, u16)>,
) -> BuiltBlock {
    // Content width under the rail (rail = 2 columns).
    let inner = width.saturating_sub(RAIL.chars().count()).max(1);
    let rail = rail_of(item.role, marked, palette);
    // The tag the message's row ends with: its number, and before it the
    // variants mark where the bubble has other variants parked — `‹2/3› #11`
    // (docs/research/chat-branches.md §3.7).
    let tag = number.map(|n| RowTag::new(n, variant));
    let number = tag.as_ref().map(RowTag::text);
    // What the tag takes of its row: itself, a column before it and the
    // margin after it.
    let reserved = number
        .as_deref()
        .map_or(0, |n| 1 + wrap::str_width(n) + NUMBER_MARGIN);
    // Build the message body without a rail, at width `inner`.
    let mut body = Body::default();
    // Where the message's own content starts, i.e. past the role header. The
    // header is excluded from the highlight because it is not indexed and would
    // fire on ordinary queries: `ASSISTANT` matches a search for "assistant" in
    // every marked assistant bubble, which reads as a bug.
    let mut content_from = 0usize;
    let glyphs = palette.glyphs();
    match item.role {
        FeedRole::User => {
            let voice = item.voice.map(|s| voice_mark(s, loc));
            let role = format!(
                "{} {}",
                glyphs.user_icon,
                role_name(names.user_name(), "ui.feed.role.user", loc)
            );
            push_header(
                &mut body,
                &role,
                palette.user_soft,
                voice.as_deref(),
                palette,
                header_room(&role, inner, reserved),
            );
            content_from = body.len();
            push_body(&mut body, item, palette, inner, opts);
        }
        FeedRole::Assistant => {
            let role = format!(
                "{} {}",
                glyphs.assistant_icon,
                role_name(names.assistant_name(), "ui.feed.role.assistant", loc)
            );
            push_header(
                &mut body,
                &role,
                palette.assistant_soft,
                item.model.as_deref().filter(|_| show_model),
                palette,
                header_room(&role, inner, reserved),
            );
            content_from = body.len();
            let key = thoughts_key(item);
            push_thoughts(
                &mut body,
                &item.thoughts,
                key.map_or(view.thoughts, |k| view.expanded(&k)),
                key,
                inner,
                palette,
                loc,
            );
            push_assistant_body(&mut body, item, palette, inner, view, opts, loc);
        }
        // The persona of a sub-agent transcript: headed like a role, body as
        // prose, on the muted rail a note uses — it is context, not a turn.
        FeedRole::System => {
            let role = format!(
                "{} {}",
                glyphs.system_icon,
                role_name(names.system_name(), "ui.feed.role.system", loc)
            );
            push_header(
                &mut body,
                &role,
                palette.muted,
                None,
                palette,
                header_room(&role, inner, reserved),
            );
            content_from = body.len();
            push_body(&mut body, item, palette, inner, opts);
        }
        // A note has no header: its number ends its own first row.
        FeedRole::Note => {
            push_body(&mut body, item, palette, inner, opts);
            if reserved > 0 && body.len() > 0 {
                body.numbered = Some(NumberedLine {
                    line: 0,
                    reflow: true,
                });
            }
        }
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
    // Every row a block holds is a row as drawn: wrapped here, to the panel's
    // width, the divider included — `render` draws the rows as they are, and a
    // selection addresses them by (block, row) (research §5.4). Each row's
    // [`RowFact`] is made with it: the wrap is the one place that knows which
    // rows continue a line.
    let mut facts: Vec<RowFact> = Vec::new();
    let mut pills: Vec<PillHit> = Vec::new();
    let mut taps: Vec<TapHit> = Vec::new();
    if let Some((boundary, summary)) = compaction {
        let mut divider = Body::default();
        let key = FoldKey::Summary(boundary);
        let expanded = view.expanded(&key);
        push_compaction_boundary(&mut divider, summary, expanded, key, width, palette, loc);
        let Body {
            lines: divider_lines,
            marks: divider_marks,
            pills: divider_pills,
            ..
        } = divider;
        for (i, (line, mark)) in divider_lines.into_iter().zip(divider_marks).enumerate() {
            let targets = pill_columns(&divider_pills, i, &line);
            let rows = wrap::wrap_line(&line, width);
            place_pills(&mut pills, &targets, &rows, out.len(), 0);
            for (k, wrapped) in rows.into_iter().enumerate() {
                out.push(wrapped);
                facts.push(RowFact::of_piece(0, mark, k));
            }
        }
    }
    let lead = wrap::str_width(rail.glyph) as u16;
    let mut content_row = None;
    let mut links: Vec<Uuid> = Vec::new();
    let Body {
        lines: body_lines,
        marks: body_marks,
        pills: body_pills,
        numbered,
    } = body;
    for (i, (mut line, mark)) in body_lines.into_iter().zip(body_marks).enumerate() {
        if i == content_from {
            content_row = Some(out.len());
        }
        // Measured before the references are styled: styling splits a span,
        // never changes a width.
        let targets = pill_columns(&body_pills, i, &line);
        // **Before** the wrap, deliberately: an address is 15 columns
        // (`chat://` + 8 hex) and a narrow panel splits it across two rows,
        // where a per-row scan would no longer see it whole.
        links.extend(style_chat_links(&mut line, known_chats, palette));
        let mut rows = wrap::wrap_line(&line, inner);
        // The message's number, when this is the line that carries it.
        let number = number
            .as_deref()
            .zip(numbered.filter(|n| n.line == i))
            .map(|(text, at)| (text, at.reflow));
        if let Some((_, true)) = number
            && rows
                .first()
                .is_some_and(|r| text_cols(r) + reserved > inner)
            && inner >= reserved + NUMBERED_NOTE_MIN_COLS
        {
            rows = wrap::wrap_line(&line, inner - reserved);
        }
        place_pills(&mut pills, &targets, &rows, out.len(), lead);
        for (k, mut wrapped) in rows.into_iter().enumerate() {
            let mut fact = RowFact::of_piece(lead, mark, k);
            if k == 0
                && let Some((text, _)) = number
            {
                fact.tail = push_number(&mut wrapped, text, inner, palette);
                if fact.tail > 0
                    && let Some(tag) = &tag
                {
                    // The tag ends where the margin begins, the rail counted.
                    let end = usize::from(lead) + inner - NUMBER_MARGIN;
                    taps.extend(tag.taps(out.len(), end, item.message_ids.first().copied()));
                }
            }
            out.push(prepend_rail(wrapped, &rail));
            facts.push(fact);
        }
    }
    // A header-only block never reaches the index above.
    let content_from = content_row.unwrap_or(out.len());
    // Separator between messages — without a rail.
    if !body_ends_blank {
        out.push(Line::from(""));
        facts.push(RowFact::default());
    }
    links.dedup();
    BuiltBlock {
        lines: out,
        facts,
        content_from,
        links,
        pills,
        taps,
    }
}

/// The thoughts block's key: the bubble's **first** message. The reply being
/// written has no message yet, and no key.
fn thoughts_key(item: &FeedMessage) -> Option<FoldKey> {
    item.message_ids.first().copied().map(FoldKey::Thoughts)
}

/// The expanded state of each of `item`'s foldable parts under `view` — its
/// thoughts, each tool card, the summary above it — hashed: what a click on
/// one pill changes in this block alone ([`CachedBlock::folds`]). A part with
/// no key hashes as `None`, so a bubble that gains its keys (the reply being
/// written, stored) is rebuilt with its pills.
fn fold_state(item: &FeedMessage, view: &FeedView, boundary: Option<Uuid>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let state = |key: Option<FoldKey>| key.map(|k| view.expanded(&k));
    state(thoughts_key(item)).hash(&mut h);
    for tool in &item.tools {
        state(tool.fold).hash(&mut h);
    }
    state(boundary.map(FoldKey::Summary)).hash(&mut h);
    h.finish()
}

/// The columns of the pills [`Body::mark_pill`] put on line `index`, from its
/// spans' widths, with their keys.
fn pill_columns(pills: &[PillMark], index: usize, line: &Line<'_>) -> Vec<(u16, u16, FoldKey)> {
    let width = |spans: &[Span<'_>]| -> u16 {
        spans
            .iter()
            .map(|s| wrap::str_width(&s.content))
            .sum::<usize>() as u16
    };
    pills
        .iter()
        .filter(|p| p.line == index)
        .map(|p| {
            let end = p.spans.end.min(line.spans.len());
            let start = p.spans.start.min(end);
            (
                width(&line.spans[..start]),
                width(&line.spans[..end]),
                p.key,
            )
        })
        .collect()
}

/// Records where a line's pills landed among the `rows` its wrap made of it,
/// the first at block row `first_row`, past a rail `lead` columns wide. A line
/// that fits one row — every pill does, but in a very narrow panel — keeps the
/// pill's columns; one the panel wrapped takes each of its rows whole: half a
/// pill is still the pill.
fn place_pills(
    pills: &mut Vec<PillHit>,
    targets: &[(u16, u16, FoldKey)],
    rows: &[Line<'static>],
    first_row: usize,
    lead: u16,
) {
    for &(start, end, key) in targets {
        if let [_] = rows {
            pills.push(PillHit {
                row: first_row,
                start: lead + start,
                end: lead + end,
                key,
            });
            continue;
        }
        for (k, row) in rows.iter().enumerate() {
            pills.push(PillHit {
                row: first_row + k,
                start: lead,
                end: lead + row.width() as u16,
                key,
            });
        }
    }
}

/// The pills of the rows on screen, in terminal cells: each block's pills
/// moved by where its rows begin, kept when inside the `scroll` window of
/// `inner` (docs/research/block-fold-and-partial-copy.md §3.5).
fn visible_fold_hits(cache: &[CachedBlock], inner: Rect, scroll: usize) -> Vec<FoldHit> {
    let mut hits = Vec::new();
    let mut at = 0usize;
    let rows = scroll..scroll + inner.height as usize;
    for block in cache {
        for pill in &block.pills {
            let abs = at + pill.row;
            if rows.contains(&abs) {
                hits.push(FoldHit {
                    row: inner.y + (abs - scroll) as u16,
                    start: inner.x + pill.start,
                    end: inner.x + pill.end,
                    key: pill.key,
                });
            }
        }
        at += block.lines.len();
    }
    hits
}

/// What a click on the tag a message's row ends with does
/// (docs/research/chat-branches.md §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    /// An arrow of a variants mark, `‹` (`step` −1) or `›` (+1): the
    /// neighbouring variant of the bubble that starts with `message`.
    Variant { message: Uuid, step: i8 },
    /// A message's number.
    Number(u32),
}

/// A tag's target in a built block: its row there and its columns in the
/// panel's content (the rail included), half-open — like [`PillHit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TapHit {
    row: usize,
    start: u16,
    end: u16,
    tap: Tap,
}

/// A tag's target on the screen, in terminal cells — [`MessageFeed::tap_hits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TapCell {
    row: u16,
    start: u16,
    end: u16,
    tap: Tap,
}

/// The row-end tags of the rows on screen, in terminal cells — the fold map's
/// arithmetic ([`visible_fold_hits`]) over [`CachedBlock::taps`].
fn visible_tap_hits(cache: &[CachedBlock], inner: Rect, scroll: usize) -> Vec<TapCell> {
    let mut hits = Vec::new();
    let mut at = 0usize;
    let rows = scroll..scroll + inner.height as usize;
    for block in cache {
        for tap in &block.taps {
            let abs = at + tap.row;
            if rows.contains(&abs) {
                hits.push(TapCell {
                    row: inner.y + (abs - scroll) as u16,
                    start: inner.x + tap.start,
                    end: inner.x + tap.end,
                    tap: tap.tap,
                });
            }
        }
        at += block.lines.len();
    }
    hits
}

/// One freshly built block, before it enters the cache.
struct BuiltBlock {
    lines: Vec<Line<'static>>,
    /// See [`CachedBlock::facts`].
    facts: Vec<RowFact>,
    /// See [`CachedBlock::content_from`].
    content_from: usize,
    /// See [`CachedBlock::links`].
    links: Vec<Uuid>,
    /// See [`CachedBlock::pills`].
    pills: Vec<PillHit>,
    /// See [`CachedBlock::taps`].
    taps: Vec<TapHit>,
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
///
/// In the monochrome mode the link style is stripped from the frame, and an
/// address that resolves is told from one that does not by the angle brackets
/// plain text has always put around a URI: `<chat://1a2b3c4d>`. The click map
/// reads the drawn rows again ([`visible_link_hits`]) and finds the address
/// inside them where it is.
fn style_chat_links(line: &mut Line<'static>, known: &[Uuid], palette: &Palette) -> Vec<Uuid> {
    let (text, found) = chat_links_in(line, known);
    if found.is_empty() {
        return Vec::new();
    }
    let ranges: Vec<std::ops::Range<usize>> = found.iter().map(|l| l.range.clone()).collect();
    restyle_ranges(line, &text, &ranges, |style| {
        style.fg(palette.accent).add_modifier(Modifier::UNDERLINED)
    });
    if palette.mono {
        bracket_ranges(line, &text, &ranges);
    }
    found.into_iter().map(|l| l.chat).collect()
}

/// Puts `<` before and `>` after every range of a line — the ranges being
/// byte offsets into `text`, the line's spans concatenated, sorted and not
/// overlapping. Each bracket takes the style of the span it lands in.
fn bracket_ranges(line: &mut Line<'static>, text: &str, ranges: &[std::ops::Range<usize>]) {
    let marks: Vec<(usize, &'static str)> = ranges
        .iter()
        .flat_map(|r| [(r.start, "<"), (r.end, ">")])
        .collect();
    let spans = std::mem::take(&mut line.spans);
    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len() + marks.len() * 2);
    let mut next = 0usize;
    // Byte offset of the current span within `text`.
    let mut at = 0usize;
    let mut last_style = Style::default();
    for span in spans {
        let (start, end) = (at, at + span.content.len());
        at = end;
        let mut from = start;
        // A mark at a span's very end belongs to the next span's start — or,
        // after the last span, to the tail below — so it is placed once.
        while let Some(&(pos, mark)) = marks.get(next).filter(|(pos, _)| *pos < end) {
            if pos > from {
                out.push(Span::styled(text[from..pos].to_string(), span.style));
            }
            out.push(Span::styled(mark, span.style));
            from = pos;
            next += 1;
        }
        if end > from {
            out.push(Span::styled(text[from..end].to_string(), span.style));
        }
        last_style = span.style;
    }
    for &(_, mark) in &marks[next..] {
        out.push(Span::styled(mark, last_style));
    }
    line.spans = out;
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

/// Marks every occurrence of `query` in one **rendered** line as a search
/// match ([`Palette::search_match`] — the accent colour, or reverse video in
/// the monochrome mode), splitting spans at the match boundaries.
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
/// Only the mark is patched: the span's markdown styling (bold, italic, code
/// coloring) has to survive, or highlighting a word inside a heading would
/// flatten the heading.
fn highlight_line(line: &mut Line<'static>, query: &str, palette: &Palette) -> usize {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges = crate::features::chat_search::match_ranges(&text, query);
    if ranges.is_empty() {
        return 0;
    }
    restyle_ranges(line, &text, &ranges, |style| palette.search_match(style));
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
    item.voice.hash(&mut h);
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

/// A voice note's mark beside the person's header: its length, `m:ss`.
fn voice_mark(seconds: u32, loc: &'static Locale) -> String {
    let time = format!("{}:{:02}", seconds / 60, seconds % 60);
    loc.tf("ui.feed.voice", &[("time", &time)])
}

/// A role-header line: icon + name in caps, colored with the role's "soft"
/// variant, optionally followed by `model` — the model that wrote the message
/// (`interface.show_model_name`, spec §11.3) — or, on the person's header, a
/// voice note's mark (ADR 0013, stage C).
///
/// The name rides the header rather than a line of its own: it is an attribute
/// of the bubble, and a second line would cost a row in every assistant message.
/// Muted and unbolded, like the "thoughts" pill — it answers a question the
/// reader asks occasionally, so it must not compete with the role itself.
///
/// `room` — the columns the header's own text may take when the message's
/// number ends its row ([`header_room`]); `None` — no number is drawn and the
/// header is the one drawn before numbers existed. With a number the row is
/// shared role-first: the role's name is whole, the number has its place, and
/// the second text gets what is left — whole, or cut with its "…", or, with
/// less than [`HEADER_NOTE_MIN_COLS`] left, not drawn (spec §11.3).
fn role_header(
    text: &str,
    color: Color,
    model: Option<&str>,
    palette: &Palette,
    room: Option<usize>,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        text.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )];
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        let shown = match room {
            None => Some(model.to_string()),
            Some(room) => {
                let left = room.saturating_sub(wrap::str_width(text) + HEADER_NOTE_GAP.len());
                (wrap::str_width(model) <= left || left >= HEADER_NOTE_MIN_COLS)
                    .then(|| wrap::truncate_to_width(model, left).0)
            }
        };
        if let Some(shown) = shown.filter(|m| !m.is_empty()) {
            spans.push(Span::styled(
                format!("{HEADER_NOTE_GAP}{shown}"),
                palette.muted_style(),
            ));
        }
    }
    Line::from(spans)
}

/// What stands between a role's name and the text after it on a header's row.
const HEADER_NOTE_GAP: &str = "  ";

/// The least a cut model name is worth drawing: three of its characters and
/// the mark of the cut. With less room it is left out.
const HEADER_NOTE_MIN_COLS: usize = 4;

/// The least width a numbered note's text is wrapped to. In a panel too narrow
/// to leave that beside the number, the note keeps the row and has no number.
const NUMBERED_NOTE_MIN_COLS: usize = 8;

/// Pushes a role's header onto `body` and, when its row has room for the
/// message's number (`room`, [`header_room`]), names it as the line that
/// carries one.
fn push_header(
    body: &mut Body,
    role: &str,
    color: Color,
    model: Option<&str>,
    palette: &Palette,
    room: Option<usize>,
) {
    if room.is_some() {
        body.numbered = Some(NumberedLine {
            line: body.len(),
            reflow: false,
        });
    }
    body.push(role_header(role, color, model, palette, room));
}

/// The columns a header's own text has on a row `inner` wide that ends on the
/// message's number, which takes `reserved` of them. `None` when no number is
/// drawn: the message has none, or its role's name leaves it no room — the
/// name is owed its place first.
fn header_room(role: &str, inner: usize, reserved: usize) -> Option<usize> {
    (reserved > 0 && wrap::str_width(role) + reserved <= inner).then(|| inner - reserved)
}

/// The width of a row's text, in the columns the feed's wrap counts, less the
/// whitespace it ends on: the wrap keeps the whitespace it broke at on the
/// upper row and lets it run past the edge ([`wrap::wrap_ranges`]), so a row's
/// whole width says nothing about the room after its text.
fn text_cols(row: &Line<'_>) -> usize {
    let chars: Vec<char> = row.spans.iter().flat_map(|s| s.content.chars()).collect();
    let text = chars
        .iter()
        .rposition(|c| !c.is_whitespace())
        .map_or(0, |last| last + 1);
    wrap::display_width(&chars[..text])
}

/// Takes the whitespace `row` ends on off it and returns its width.
fn trim_row_end(row: &mut Line<'static>) -> usize {
    let mut trimmed = 0;
    while let Some(last) = row.spans.last_mut() {
        let kept = last.content.trim_end().len();
        trimmed += wrap::str_width(&last.content[kept..]);
        if kept == 0 {
            row.spans.pop();
            continue;
        }
        if kept < last.content.len() {
            last.content = last.content[..kept].to_string().into();
        }
        break;
    }
    trimmed
}

/// The columns left empty between a message's number and the right edge of
/// the feed's content (the owner's decision, 2026-11-18): the number does not
/// touch the border or the scrollbar on it.
const NUMBER_MARGIN: usize = 1;

/// Ends `row` on the message's number `text`, [`NUMBER_MARGIN`] short of the
/// right edge of a row `inner` wide, and returns the row's tail
/// ([`RowFact::tail`]): the number and the padding before it. A row whose text
/// leaves no room for a column of padding, the number and the margin is left
/// as it is, and the answer is 0.
///
/// The whitespace the row ended on — what the wrap broke at, which a copy
/// joins the next row by — is redrawn as the first of the padding and stays
/// the row's content: the tail starts after it. Where the number leaves room
/// for less of it than there was, the row keeps what fits, a column at least.
///
/// The number is the row's **last span**: [`highlight_block_tail`] takes it
/// off by that before it looks for a query.
fn push_number(row: &mut Line<'static>, text: &str, inner: usize, palette: &Palette) -> u16 {
    let used = text_cols(row);
    let number = wrap::str_width(text);
    if used + 1 + number + NUMBER_MARGIN > inner {
        return 0;
    }
    let own = trim_row_end(row);
    let gap = inner - NUMBER_MARGIN - used - number;
    row.spans.push(Span::raw(" ".repeat(gap)));
    row.spans
        .push(Span::styled(text.to_string(), palette.muted_style()));
    (number + gap.saturating_sub(own)) as u16
}

/// The tag a message's row ends with: its number, and before it the variants
/// mark of a bubble whose place has other variants parked.
struct RowTag {
    number: u32,
    variant: Option<(u16, u16)>,
}

impl RowTag {
    fn new(number: u32, variant: Option<(u16, u16)>) -> Self {
        Self { number, variant }
    }

    fn mark(&self) -> Option<String> {
        self.variant.map(|(k, of)| format!("‹{k}/{of}›"))
    }

    fn number_text(&self) -> String {
        format!("#{}", self.number)
    }

    /// As drawn: `‹2/3› #11`, or `#11`.
    fn text(&self) -> String {
        match self.mark() {
            Some(mark) => format!("{mark} {}", self.number_text()),
            None => self.number_text(),
        }
    }

    /// The tag's click targets on block row `row`, the tag ending before
    /// column `end`: the number, and the two arrows of a mark — which name
    /// the bubble by its first message, `first`, so a bubble with no stored
    /// message yet has none.
    fn taps(&self, row: usize, end: usize, first: Option<Uuid>) -> Vec<TapHit> {
        let hit = |start: usize, end: usize, tap: Tap| TapHit {
            row,
            start: start as u16,
            end: end as u16,
            tap,
        };
        let number = wrap::str_width(&self.number_text());
        let mut taps = vec![hit(end - number, end, Tap::Number(self.number))];
        if let (Some(mark), Some(message)) = (self.mark(), first) {
            let width = wrap::str_width(&mark);
            let start = end - number - 1 - width;
            taps.push(hit(start, start + 1, Tap::Variant { message, step: -1 }));
            taps.push(hit(
                start + width - 1,
                start + width,
                Tap::Variant { message, step: 1 },
            ));
        }
        taps
    }
}

/// The number of each feed item, in order: the messages of the conversation
/// counted from 1, `None` for what is not one ([`FeedMessage::is_numbered`]).
fn numbers(messages: &[FeedMessage]) -> impl Iterator<Item = Option<u32>> + '_ {
    let mut last = 0u32;
    messages.iter().map(move |m| {
        m.is_numbered().then(|| {
            last += 1;
            last
        })
    })
}

/// The feed index of the bubble holding the domain message `id` — the
/// projection's own id mapping ([`FeedMessage::message_ids`]): an agentic
/// round's messages merge many-to-one, so any of them finds its bubble.
/// `None` for a message the feed does not show.
fn block_of(messages: &[FeedMessage], id: Uuid) -> Option<usize> {
    messages.iter().position(|m| m.message_ids.contains(&id))
}

/// The feed item that shows `#number` — what a command that names a message
/// by its number acts on (docs/research/chat-branches.md §3.6): the screen
/// resolves the number here, against what it draws, and sends the id of the
/// message the item carries.
pub fn numbered_item(messages: &[FeedMessage], number: u32) -> Option<&FeedMessage> {
    messages
        .iter()
        .zip(numbers(messages))
        .find_map(|(m, n)| (n == Some(number)).then_some(m))
}

/// The number `item` shows in `messages`, when it is one of them and numbered.
pub fn number_of(messages: &[FeedMessage], first_message: Uuid) -> Option<u32> {
    messages
        .iter()
        .zip(numbers(messages))
        .find_map(|(m, n)| (m.message_ids.first() == Some(&first_message)).then_some(n)?)
}

/// The last number the feed shows; 0 when it holds no message.
pub fn last_number(messages: &[FeedMessage]) -> u32 {
    numbers(messages).flatten().last().unwrap_or(0)
}

/// Where a block stands in the feed — what its build needs beyond the item
/// itself, and what its cache entry is checked against.
#[derive(Clone, Copy)]
struct BlockPlace {
    /// Verbatim history resumes here: the compaction divider is drawn above.
    at_boundary: bool,
    /// The message's number ([`numbers`]).
    number: Option<u32>,
    /// Which variant of its place the bubble is, when the place has others
    /// parked ([`MessageFeed::set_variants`]).
    variant: Option<(u16, u16)>,
}

/// A message's rail: the glyph and its colour. The colour says the role — and
/// the accent, that this is the message a jump landed on; in the monochrome
/// mode, where no colour reaches the screen, the glyph says both.
struct Rail {
    glyph: &'static str,
    color: Color,
}

fn rail_of(role: FeedRole, marked: bool, palette: &Palette) -> Rail {
    let color = if marked {
        palette.accent
    } else {
        match role {
            FeedRole::User => palette.user,
            FeedRole::Assistant => palette.assistant,
            FeedRole::System | FeedRole::Note => palette.muted,
        }
    };
    let glyph = if !palette.mono {
        RAIL
    } else if marked {
        RAIL_MONO_MARKED
    } else {
        match role {
            FeedRole::User => RAIL_MONO_USER,
            FeedRole::Assistant => RAIL_MONO_ASSISTANT,
            FeedRole::System | FeedRole::Note => RAIL_MONO_QUIET,
        }
    };
    Rail { glyph, color }
}

/// Prepends the colored gutter rail ([`Rail`]) to a line, preserving the source line's
/// style and alignment.
///
/// The line-level style (`line.style`) is **folded into the content spans**, and
/// `out.style` is reset to default — otherwise line-level modifiers (e.g. `DIM`
/// on `───` dividers and table borders, see `shared::markdown`) would leak onto
/// the rail itself (its span only sets `fg`, not touching modifiers), making it
/// look a different color next to such lines.
fn prepend_rail(line: Line<'static>, rail: &Rail) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(
        rail.glyph.to_string(),
        Style::new().fg(rail.color),
    ));
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
    lines: &mut Body,
    summary: &str,
    expanded: bool,
    key: FoldKey,
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
    // The pill a click folds the summary by: the marker, the label and the
    // keycap — not the rule that fills the rest of the row (§3.4).
    let pill = 0..spans.len();
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
    lines.mark_pill(lines.len() - 1, pill, Some(key));
    if !expanded || summary.is_empty() {
        return;
    }
    for t in summary.lines() {
        push_gutter_line(lines, t, width, palette);
    }
    // Keep the summary from gluing onto the role header of the message below.
    lines.push(Line::from(""));
}

/// One source line of a foldable block's content, on its [`GUTTER`] — muted
/// italic, left for the feed to wrap.
///
/// In the monochrome mode the line is wrapped **here**, to `width`, with the
/// gutter on every row: muted italic is what tells a wrapped row of a thought
/// from the reply under it, and where that is stripped the gutter is all
/// there is — so it cannot stop at the first row (spec §11.6).
fn push_gutter_line(lines: &mut Body, text: &str, width: usize, palette: &Palette) {
    let style = palette.muted_style().add_modifier(Modifier::ITALIC);
    let gutter = wrap::str_width(GUTTER);
    if !palette.mono {
        lines.push_marked(
            Line::from(vec![
                Span::styled(GUTTER, style),
                Span::styled(text.to_string(), style),
            ]),
            wrapped_marks(gutter, LineMark::default())(0),
        );
        return;
    }
    let marks = wrapped_marks(gutter, LineMark::default());
    let rows = wrap::wrap_hanging(
        &[Span::styled(GUTTER, style)],
        &Line::from(Span::styled(text.to_string(), style)),
        width,
    );
    for (k, row) in rows.into_iter().enumerate() {
        lines.push_marked(row, marks(k));
    }
}

/// Adds the "thoughts" block: collapsed — a "pill" with a line count and a key hint,
/// expanded — content on the `│` gutter ([`push_gutter_line`]; `width` is the
/// content width under the rail). Either header row is the pill a click folds
/// this block alone by (`key`; none on the reply being written).
fn push_thoughts(
    lines: &mut Body,
    thoughts: &str,
    expanded: bool,
    key: Option<FoldKey>,
    width: usize,
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
        lines.mark_pill(lines.len() - 1, 0..4, key);
        return;
    }
    lines.push(Line::from(Span::styled(
        format!("{} {}", glyphs.expanded, loc.t("ui.feed.thoughts")),
        muted.add_modifier(Modifier::ITALIC),
    )));
    lines.mark_pill(lines.len() - 1, 0..1, key);
    for t in thoughts.lines() {
        push_gutter_line(lines, t, width, palette);
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
    lines: &mut Body,
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    view: &FeedView,
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
        let expanded = tool.fold.map_or(view.tools, |k| view.expanded(&k));
        push_tool(lines, tool, palette, width, expanded, opts, loc);
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
fn ensure_blank_line(lines: &mut Body) {
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
    lines: &mut Body,
    fragment: &str,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) -> bool {
    if fragment.trim().is_empty() {
        return false;
    }
    lines.extend_marked(markdown::render_marked(fragment, width, palette, opts));
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
    lines: &mut Body,
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
        let (line, spans) = push_trailing(lines, pill, glyphs.tool_cont, width);
        lines.mark_pill(line, spans, tool.fold);
        return;
    }
    // Expanded, the card ends its header on a `▾ details` pill of its own
    // (fork F5b): the other foldable blocks show their marker expanded too, and
    // it is what a click folds this card alone by. No keycap — the expanded
    // thoughts have none either. Nothing to fold, no pill.
    if !p.args.is_empty() || !p.result.is_empty() {
        let pill = vec![
            Span::styled(format!("{} ", glyphs.expanded), muted),
            Span::styled(
                loc.t("ui.feed.tool_details").to_string(),
                muted.add_modifier(Modifier::ITALIC),
            ),
        ];
        let (line, spans) = push_trailing(lines, pill, glyphs.tool_cont, width);
        lines.mark_pill(line, spans, tool.fold);
    }
    for block in &p.args {
        push_block(lines, block, palette, width, false, opts, loc);
    }
    // A gap between the request and the answer: expanded, the argument list can
    // run for several rows, and without a break it reads as one wall with the
    // result. The `│` gutter **continues** through it — a bare blank row would
    // cut the card in two. Only when there is something on both sides of it.
    if !p.args.is_empty() && !p.result.is_empty() {
        lines.push_marked(
            Line::from(Span::styled(
                "│".to_string(),
                Style::default().fg(palette.muted),
            )),
            LineMark {
                prefix: 1,
                ..LineMark::default()
            },
        );
    }
    for block in &p.result {
        push_block(lines, block, palette, width, true, opts, loc);
    }
}

/// Draws one block of a tool card. `is_result` changes the leading gutter: the result
/// starts with `└ ` (a corner), an argument with `│ ` (a vertical bar, "nested under
/// the header"). The `│`/`└` gutter is WGL4-safe under compatibility mode too.
fn push_block(
    lines: &mut Body,
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
            let marks = vec![LineMark::default(); hl.len()];
            let src = markdown::Marked { lines: hl, marks };
            push_gutter_lines(lines, src, "│ ", "│ ", gutter_style, width);
        }
        ToolBlock::Markdown(text) => {
            let body_w = width.saturating_sub(2).max(1);
            let rendered = markdown::render_marked(text, body_w, palette, opts);
            push_gutter_lines(lines, rendered, "└ ", "  ", gutter_style, width);
        }
        ToolBlock::Console(c) => push_console(lines, c, palette, width, loc),
    }
}

/// Draws `python_exec`'s console output: stdout (text color), stderr (error
/// color), and the exit code (warning color) as separate sections. Each
/// section — a label on the `└ ` gutter and content on `│ `.
fn push_console(
    lines: &mut Body,
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
        // (docs/history/sandbox-file-exchange.md §11 S9).
        push_wrapped(lines, "└ ", "  ", "files", width, label);
        push_wrapped(lines, "│ ", "│ ", &console.files, width, out_style);
    }
}

/// Places ready (already styled) lines under gutter prefixes, wrapping
/// each by width. `first`/`cont` — prefixes for the first/subsequent rows. The
/// line-level style is folded into the content spans (as in [`prepend_rail`]), so
/// line-level modifiers (e.g. `DIM` on table borders) don't leak onto the gutter.
fn push_gutter_lines(
    lines: &mut Body,
    src: markdown::Marked,
    first: &str,
    cont: &str,
    gutter_style: Style,
    width: usize,
) {
    let first_w = wrap::display_width(&first.chars().collect::<Vec<_>>());
    let cont_w = wrap::display_width(&cont.chars().collect::<Vec<_>>());
    let body_w = width.saturating_sub(first_w.max(cont_w)).max(1);
    let mut first_row = true;
    for (line, mark) in src.lines.into_iter().zip(src.marks) {
        let line_style = line.style;
        let align = line.alignment;
        for (k, wrapped) in wrap::wrap_line(&line, body_w).into_iter().enumerate() {
            let prefix = if first_row { first } else { cont };
            first_row = false;
            // The gutter is a prefix of every row; the source line's own (a
            // quote's `> `) only of its first.
            let own = if k == 0 { usize::from(mark.prefix) } else { 0 };
            let row_mark = wrapped_marks(wrap::str_width(prefix) + own, mark)(k);
            let mut spans = Vec::with_capacity(wrapped.spans.len() + 1);
            spans.push(Span::styled(prefix.to_string(), gutter_style));
            for span in wrapped.spans {
                spans.push(Span::styled(span.content, line_style.patch(span.style)));
            }
            let mut out = Line::from(spans);
            out.alignment = align;
            lines.push_marked(out, row_mark);
        }
    }
}

/// Wraps `text` by visual width with gutter prefixes and pushes into `lines`.
/// `first` — the prefix for the block's very first row, `cont` — for all subsequent rows
/// (both wrap continuations and new logical lines of the source). Each line of
/// `text` is wrapped separately, preserving line breaks.
fn push_wrapped(lines: &mut Body, first: &str, cont: &str, text: &str, width: usize, style: Style) {
    let first_w = wrap::display_width(&first.chars().collect::<Vec<_>>());
    let cont_w = wrap::display_width(&cont.chars().collect::<Vec<_>>());
    let body_w = width.saturating_sub(first_w.max(cont_w)).max(1);
    let mut first_row = true;
    for src in text.split('\n') {
        let chars: Vec<char> = src.chars().collect();
        for (k, (s, e)) in wrap::wrap_ranges(&chars, body_w).into_iter().enumerate() {
            let prefix = if first_row { first } else { cont };
            first_row = false;
            let content: String = chars[s..e].iter().collect();
            let mark = wrapped_marks(wrap::str_width(prefix), LineMark::default())(k);
            lines.push_marked(
                Line::from(vec![
                    Span::styled(prefix.to_string(), style),
                    Span::styled(content, style),
                ]),
                mark,
            );
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
/// so a fit measured here is a fit there. Returns where the group landed — its
/// line and its spans there — for a pill to be marked by ([`Body::mark_pill`]).
fn push_trailing(
    lines: &mut Body,
    group: Vec<Span<'static>>,
    cont: &str,
    width: usize,
) -> (usize, Range<usize>) {
    let span_width =
        |spans: &[Span<'_>]| -> usize { spans.iter().map(|s| wrap::str_width(&s.content)).sum() };
    let group_w = span_width(&group);
    let n = group.len();
    let line = lines.len().saturating_sub(1);
    if let Some(last) = lines.last_mut()
        && span_width(&last.spans) + TRAILING_GAP.len() + group_w <= width
    {
        // The gap takes the group's own style — the head span's, so the
        // whole group reads as one decoration.
        let style = group.first().map(|s| s.style).unwrap_or_default();
        last.spans.push(Span::styled(TRAILING_GAP, style));
        let from = last.spans.len();
        last.spans.extend(group);
        return (line, from..from + n);
    }
    let mut spans = Vec::with_capacity(group.len() + 1);
    spans.push(Span::raw(cont.to_string()));
    spans.extend(group);
    lines.push_marked(
        Line::from(spans),
        LineMark {
            prefix: wrap::str_width(cont) as u16,
            ..LineMark::default()
        },
    );
    (lines.len() - 1, 1..1 + n)
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
    lines: &mut Body,
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
            lines.extend_marked(markdown::render_marked(&item.text, width, palette, opts));
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

    /// No caption: the header most tests draw.
    const BARE: FeedCaption = FeedCaption {
        name: String::new(),
        detail: None,
    };

    /// The bordered panel most tests draw, titled and without a caption.
    const PANEL: FeedFrame<'static> = FeedFrame::Bordered {
        title: "Чат",
        caption: &BARE,
        counter: None,
    };

    fn caption(name: &str, detail: Option<&str>) -> FeedCaption {
        FeedCaption {
            name: name.to_string(),
            detail: detail.map(String::from),
        }
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
            voice: None,
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
            fold: None,
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
            fold: None,
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
            fold: None,
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
    fn a_voice_note_is_marked_beside_the_persons_header() {
        let mut m = Message::user("remind me about the dentist").with_voice(75);
        let item = FeedMessage::from_message(&m).unwrap();
        assert_eq!(item.voice, Some(75));
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let mut feed = MessageFeed::new();
        let lines = feed.build_lines(&[item], &Palette::default(), 80, en);
        let header = lines
            .iter()
            .map(|l| joined(std::slice::from_ref(l)))
            .find(|l| l.contains("YOU"))
            .unwrap();
        assert!(header.contains("voice message 1:15"), "{header}");
        m.voice = None;
        let typed = FeedMessage::from_message(&m).unwrap();
        let all = joined(&feed.build_lines(&[typed], &Palette::default(), 80, en));
        assert!(!all.contains("voice message"), "{all}");
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
            fold: None,
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
            fold: None,
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
            fold: None,
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
            fold: None,
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
                fold: None,
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
                fold: None,
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
            fold: None,
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
            feed.render(f, f.area(), PANEL, msgs, &Palette::default(), ru());
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

    /// A bare feed (spec §11.1.1) is rows of the conversation to the window's
    /// edges: no border, no title, the text wrapped to the full width, no
    /// scrollbar column, and — with few rows — the last row is the last line
    /// of text, not the padding under it. The click map moves with the
    /// origin: the rail is at column 0, the address at 2.
    #[test]
    fn a_bare_feed_is_rows_of_the_conversation_to_the_edge() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let id = chat_uuid(1);
        let uri = crate::features::chat_links::uri(id);
        let mut feed = MessageFeed::new();
        feed.set_known_chats(vec![id]);
        // A line of exactly 38 columns: whole in a bare 40-column feed, and
        // wrapped inside a bordered one, whose rows hold 36.
        let line = format!("{uri} {}", "x".repeat(38 - uri.chars().count() - 1));
        let msgs = vec![
            msg(FeedRole::User, "a question", ""),
            msg(FeedRole::Assistant, &line, ""),
        ];
        let rows = |feed: &mut MessageFeed, header: FeedFrame, h: u16| -> Vec<String> {
            let mut term = Terminal::new(TestBackend::new(40, h)).unwrap();
            term.draw(|f| feed.render(f, f.area(), header, &msgs, &Palette::default(), ru()))
                .unwrap();
            crate::shared::ui::tests::buffer_rows(term.backend().buffer())
        };

        let bare = rows(&mut feed, FeedFrame::Bare, 4);
        assert!(
            bare.iter().all(|r| !r.contains(['╭', '│', '╰', '█'])),
            "{bare:?}"
        );
        assert!(bare[3].starts_with(&format!("▌ {uri} x")), "{bare:?}");
        assert!(
            bare[3].ends_with('x'),
            "the line is whole, to the edge: {:?}",
            bare[3]
        );
        assert_eq!(
            feed.link_hit_for_test(0),
            Some((2, 3, 2 + uri.chars().count() as u16))
        );
        assert!(feed.is_following());

        // The same feed with a border: the line wraps, and the padding row
        // under the message is the last one.
        let boxed = rows(&mut feed, PANEL, 6);
        assert!(
            boxed[0].starts_with('╭') && boxed[5].starts_with('╰'),
            "{boxed:?}"
        );
        assert_eq!(
            boxed[4].trim_matches(['│', '█', ' ']),
            "",
            "the padding row: {boxed:?}"
        );
        assert!(
            boxed[3].contains("xx") && !boxed[3].ends_with("x│"),
            "wrapped: {boxed:?}"
        );

        // Not following: a bare feed scrolled up shows what it is scrolled
        // to, and the bottom is reached one row before the padding.
        let bare = rows(&mut feed, FeedFrame::Bare, 2);
        assert!(bare[1].starts_with(&format!("▌ {uri}")), "{bare:?}");
        feed.scroll_up(1);
        let up = rows(&mut feed, FeedFrame::Bare, 2);
        assert!(up[1].starts_with("▌ ✦"), "the header row: {up:?}");
        assert!(!feed.is_following());
        feed.scroll_down(1);
        let _ = rows(&mut feed, FeedFrame::Bare, 2);
        assert!(feed.is_following(), "the bottom is the last line of text");
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
            feed.render(f, f.area(), PANEL, &msgs, &Palette::default(), ru());
        })
        .unwrap();
        assert!(
            feed.link_hits.is_empty(),
            "a reference off-screen is not clickable"
        );

        feed.scroll_up(usize::MAX);
        term.draw(|f| {
            feed.render(f, f.area(), PANEL, &msgs, &Palette::default(), ru());
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
            fold: None,
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
            fold: None,
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
            fold: None,
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
            fold: None,
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
            fold: None,
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
            fold: None,
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
                fold: None,
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
            fold: None,
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

    /// The folded pill counts the thoughts' lines in words that read right at
    /// one: it said "thinking · 1 lines" (docs/journal/ui-feed.md).
    #[test]
    fn the_folded_pill_counts_lines_in_words_that_fit_one() {
        use crate::shared::i18n::{Lang, locale};
        let pill = |thoughts: &str, lang: Lang| {
            let lines = MessageFeed::new().build_lines(
                &[msg(FeedRole::Assistant, "answer", thoughts)],
                &Palette::default(),
                80,
                locale(lang),
            );
            joined(&lines)
        };
        for (thoughts, en, ru) in [
            ("one", "thinking · lines: 1 · ", "мысли · 1 стр. · "),
            ("one\ntwo", "thinking · lines: 2 · ", "мысли · 2 стр. · "),
        ] {
            let (en_pill, ru_pill) = (pill(thoughts, Lang::En), pill(thoughts, Lang::Ru));
            assert!(en_pill.contains(en), "{en_pill}");
            assert!(ru_pill.contains(ru), "{ru_pill}");
        }
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

    /// An unhighlighted code block is reversed by default (mirroring the
    /// config default), in the assistant's reply and the user's message alike;
    /// the normal background takes the reverse away, and changing the value
    /// resets the cache (same messages/width/palette — only the setting
    /// changes). Spec §11.4.
    #[test]
    fn code_block_background_follows_setting_and_invalidates_cache() {
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let md = "```\nplain text, no language\n```";
        let messages = [
            msg(FeedRole::Assistant, md, ""),
            msg(FeedRole::User, md, ""),
        ];
        let reversed = |lines: &[Line<'static>]| -> usize {
            lines
                .iter()
                .filter(|l| {
                    l.spans.iter().any(|s| s.content.contains("plain text"))
                        && (l.style.add_modifier.contains(Modifier::REVERSED)
                            || l.spans
                                .iter()
                                .any(|s| s.style.add_modifier.contains(Modifier::REVERSED)))
                })
                .count()
        };
        let inverted = feed.build_lines(&messages, &palette, 80, ru());
        assert_eq!(reversed(&inverted), 2, "inverted by default, in both roles");
        feed.set_code_block_background(CodeBlockBackground::Normal);
        let normal = feed.build_lines(&messages, &palette, 80, ru());
        assert_eq!(
            reversed(&normal),
            0,
            "normal — no reverse (cache reset by key)"
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
            provider: None,
            cost_nanos: None,
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

    // ---------- the empty feed (docs/research/public-release-readiness.md §3.2) ----------

    /// With no engine configured, an empty feed names the four routes — the
    /// settings by chord and by command, a local server by `/local`, a local
    /// build, the demo — instead of
    /// inviting a message that can only be refused; with one, the invitation
    /// comes back. Wide enough that no route wraps mid-phrase.
    #[test]
    fn an_empty_feed_without_an_engine_explains_how_to_start() {
        let mut feed = MessageFeed::new();
        let routes = [
            "Ctrl+P",
            "/settings",
            "/local",
            "mindfork llama setup",
            "mindfork demo",
        ];

        let text = visible(&mut feed, &[], 240, 12).join("\n");
        assert!(text.contains(ru().t("ui.feed.empty")), "{text}");
        assert!(routes.iter().all(|r| !text.contains(r)), "{text}");

        feed.set_engine_missing(true);
        let text = visible(&mut feed, &[], 240, 12).join("\n");
        assert!(!text.contains(ru().t("ui.feed.empty")), "{text}");
        for route in routes {
            assert!(text.contains(route), "{route} missing:\n{text}");
        }

        feed.set_engine_missing(false);
        let text = visible(&mut feed, &[], 240, 12).join("\n");
        assert!(text.contains(ru().t("ui.feed.empty")), "{text}");
    }

    /// The routes are for an empty chat only: a conversation already on screen
    /// is not covered by them, whatever the engine's state.
    #[test]
    fn a_chat_with_messages_shows_no_setup_routes() {
        let mut feed = MessageFeed::new();
        feed.set_engine_missing(true);
        let text = visible(&mut feed, &numbered(2), 240, 12).join("\n");
        assert!(text.contains("сообщение-1"), "{text}");
        assert!(!text.contains("mindfork demo"), "{text}");
    }

    // ---------- jump to a message (docs/history/chat-search-stage2.md, stage 2a) ----------

    /// Renders the feed into a `TestBackend` and returns the visible text rows.
    /// A jump is only applied inside `render` (§1.1), so every focus assertion
    /// has to go through a real frame.
    fn visible(feed: &mut MessageFeed, messages: &[FeedMessage], w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| feed.render(f, f.area(), PANEL, messages, &Palette::default(), ru()))
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

    /// A conversation opened to be read from its start
    /// (docs/research/feed-landing.md §3.2): the first row, and the feed does
    /// not follow its tail — unless it fits the panel, where the first row and
    /// the last are one view.
    #[test]
    fn scroll_to_top_shows_the_first_row() {
        let messages = numbered(12);
        let mut feed = MessageFeed::new();
        let rows = visible(&mut feed, &messages, 40, 10);
        assert!(
            rows.iter().any(|r| r.contains("сообщение-11")) && feed.scroll_row() > 0,
            "precondition: a new feed is at its tail: {rows:?}"
        );

        feed.scroll_to_top();
        let rows = visible(&mut feed, &messages, 40, 10);
        assert!(rows.iter().any(|r| r.contains("сообщение-0")), "{rows:?}");
        assert_eq!(feed.scroll_row(), 0);
        assert!(!feed.is_following(), "read from the start, not watched");

        let short = numbered(1);
        let mut feed = MessageFeed::new();
        feed.scroll_to_top();
        visible(&mut feed, &short, 40, 10);
        assert!(feed.is_following(), "a feed that fits is at its last row");
    }

    /// The view's first row is read as a place and found again in a feed
    /// built anew — another chat was open in between (research §3.4). The
    /// rows that were on the screen are the rows that come back, a row inside
    /// a block included, and nothing is marked: the person is where they
    /// were, not somewhere they were sent.
    #[test]
    fn a_place_brings_back_the_rows_that_were_on_the_screen() {
        let messages = numbered_long(12);
        let mut feed = MessageFeed::new();
        visible(&mut feed, &messages, 30, 10);
        feed.scroll_up(9);
        let mut before = visible(&mut feed, &messages, 30, 10);
        let mut place = feed
            .place(&messages)
            .expect("a message is on the first row");
        // Inside a block, not on its first row: the half of a place a jump
        // never needed.
        while place.row == 0 {
            feed.scroll_up(1);
            before = visible(&mut feed, &messages, 30, 10);
            place = feed.place(&messages).unwrap();
        }
        assert!(!feed.is_following(), "precondition: scrolled away");

        let mut again = MessageFeed::new();
        assert!(again.restore_place(&messages, place));
        let after = visible(&mut again, &messages, 30, 10);
        assert_eq!(after, before, "the same rows, on the same screen rows");
        assert!(!again.is_following());
        assert_eq!(again.marker(), None, "nothing is marked");
        assert_eq!(again.place(&messages), Some(place), "and it reads back");
    }

    /// A place taken while the feed followed its tail is the last screenful,
    /// and restoring it follows the tail again: the clamp, not a flag carried
    /// across.
    #[test]
    fn a_place_at_the_tail_follows_the_tail_again() {
        let messages = numbered(12);
        let mut feed = MessageFeed::new();
        let before = visible(&mut feed, &messages, 40, 10);
        let place = feed.place(&messages).unwrap();

        let mut again = MessageFeed::new();
        assert!(again.restore_place(&messages, place));
        assert_eq!(visible(&mut again, &messages, 40, 10), before);
        assert!(again.is_following());
    }

    /// A row read at one width can be past the block's end at another: the
    /// block's last row then, never the next message's (fork S6 — a row
    /// inside a block is applied once and clamped, not trusted).
    #[test]
    fn a_place_past_its_blocks_end_is_the_blocks_last_row() {
        let messages = numbered(8);
        let id = messages[2].message_ids[0];
        let mut feed = MessageFeed::new();
        assert!(feed.restore_place(
            &messages,
            FeedPlace {
                message: id,
                row: 999
            }
        ));
        visible(&mut feed, &messages, 40, 8);
        let landed = feed.place(&messages).unwrap();
        assert_eq!(landed.message, id, "still that message");
        assert!(landed.row < 999);

        feed.scroll_down(1);
        visible(&mut feed, &messages, 40, 8);
        assert_eq!(
            feed.place(&messages),
            Some(FeedPlace {
                message: messages[3].message_ids[0],
                row: 0
            }),
            "one row below it the next message begins"
        );
    }

    /// What is not a place: a note has no message behind it, so the next
    /// bubble that has one stands in at its first row; a feed with nothing
    /// stored below the view's first row has none, and neither has a feed no
    /// frame has drawn. A message the feed does not show cannot be gone back
    /// to.
    #[test]
    fn a_place_names_a_stored_message_or_nothing() {
        let mut messages = numbered(8);
        messages.insert(0, FeedMessage::note("заметка"));
        let mut feed = MessageFeed::new();
        assert_eq!(feed.place(&messages), None, "no frame yet");
        feed.scroll_to_top();
        visible(&mut feed, &messages, 40, 8);
        assert_eq!(
            feed.place(&messages),
            Some(FeedPlace {
                message: messages[1].message_ids[0],
                row: 0
            })
        );

        let notes = vec![FeedMessage::note("а"), FeedMessage::note("б")];
        let mut feed = MessageFeed::new();
        visible(&mut feed, &notes, 40, 8);
        assert_eq!(feed.place(&notes), None);

        let gone = FeedPlace {
            message: Uuid::new_v4(),
            row: 0,
        };
        let mut feed = MessageFeed::new();
        assert!(!feed.restore_place(&messages, gone));
        assert!(!feed.reveal_message(&messages, gone.message, None));
        visible(&mut feed, &messages, 40, 8);
        assert!(feed.is_following(), "a refused landing left the feed alone");
    }

    /// A variant switch (research §3.3): the bubble's content changes and its
    /// first row is on the screen, so the view does not move — the mark that
    /// was clicked is still under the pointer — though the feed now ends far
    /// below and no longer follows its tail.
    #[test]
    fn reveal_leaves_a_header_on_the_screen_where_it_is() {
        let mut messages = numbered(12);
        let mut feed = MessageFeed::new();
        let before = visible(&mut feed, &messages, 40, 10);
        let scroll = feed.scroll_row();
        assert!(feed.is_following(), "precondition: at the tail");
        let header = before
            .iter()
            .position(|r| r.contains("сообщение-11"))
            .expect("the last bubble is on the screen");

        // The feed rebuilt with another variant in the last bubble's place.
        let held = feed.place(&messages);
        let last = messages.len() - 1;
        messages[last] = msg(FeedRole::User, &"вариант ".repeat(80), "");
        let id = messages[last].message_ids[0];
        assert!(feed.reveal_message(&messages, id, held));
        let after = visible(&mut feed, &messages, 40, 10);

        assert_eq!(feed.scroll_row(), scroll, "the view did not move");
        assert!(
            after[header].contains("вариант"),
            "the variant begins on the row its predecessor did: {after:?}"
        );
        // Less the border column: the scrollbar's thumb is drawn on it, and a
        // longer feed has a shorter one.
        let body = |rows: &[String]| -> Vec<String> {
            rows[1..header]
                .iter()
                .map(|r| r.chars().take(39).collect())
                .collect()
        };
        assert_eq!(body(&after), body(&before), "the rows above it stand");
        assert!(!feed.is_following(), "the feed ends below the view now");
    }

    /// The same switch asked for from elsewhere (`/branch N K` typed at the
    /// tail): the bubble is not on the screen, so its first row becomes the
    /// view's first. So it does when the held row is gone, or was never read.
    #[test]
    fn reveal_puts_a_header_off_the_screen_on_the_first_row() {
        let messages = numbered(12);
        let id = messages[2].message_ids[0];
        let on_top = Some(FeedPlace {
            message: id,
            row: 0,
        });

        let mut feed = MessageFeed::new();
        visible(&mut feed, &messages, 40, 10);
        let held = feed.place(&messages);
        assert!(feed.reveal_message(&messages, id, held));
        visible(&mut feed, &messages, 40, 10);
        assert_eq!(feed.place(&messages), on_top);
        assert!(!feed.is_following());

        let gone = Some(FeedPlace {
            message: Uuid::new_v4(),
            row: 3,
        });
        for held in [gone, None] {
            let mut feed = MessageFeed::new();
            visible(&mut feed, &messages, 40, 10);
            assert!(feed.reveal_message(&messages, id, held));
            visible(&mut feed, &messages, 40, 10);
            assert_eq!(feed.place(&messages), on_top, "held: {held:?}");
        }
    }

    /// A reveal below the view: the bubble is put on the first row, where the
    /// feed has that many rows under it; near the end the view is the last
    /// screenful and the bubble is on it.
    #[test]
    fn reveal_of_a_bubble_below_the_view_brings_it_up() {
        let messages = numbered(12);
        let mut feed = MessageFeed::new();
        feed.scroll_to_top();
        visible(&mut feed, &messages, 40, 10);
        let held = feed.place(&messages);
        let id = messages[6].message_ids[0];
        assert!(feed.reveal_message(&messages, id, held));
        let rows = visible(&mut feed, &messages, 40, 10);
        assert_eq!(
            feed.place(&messages),
            Some(FeedPlace {
                message: id,
                row: 0
            }),
            "{rows:?}"
        );
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

    /// A rewrap wipes the cache and changes every row offset, so the view is
    /// put back on the message it showed, not on a row number (§1.5, fork S6).
    /// Asserted as "the message is still in view", not as a row number — the
    /// row legitimately changes.
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
            "after a resize the message jumped to must still be in view: {narrow:?}"
        );
        assert_ne!(
            row_wide,
            feed.scroll_row(),
            "the rewrap should have moved the row — otherwise the test proves nothing"
        );
    }

    /// The split (where the view stands vs the marker): a manual scroll means
    /// "I am steering now", so a jump no frame has applied yet is dropped — but
    /// the **marker** stays, because the point of marking the message is that
    /// you can scroll around it to read its context and still see where you
    /// landed.
    #[test]
    fn a_manual_scroll_drops_a_pending_jump_but_keeps_the_marker() {
        let messages = numbered(12);
        for scroll in [
            (|f: &mut MessageFeed| f.scroll_up(2)) as fn(&mut MessageFeed),
            |f: &mut MessageFeed| f.scroll_down(2),
        ] {
            // After the jump was drawn: the marker outlives the scroll.
            let mut feed = MessageFeed::new();
            feed.focus_message(&messages, messages[9].message_ids[0], None);
            let _ = visible(&mut feed, &messages, 40, 10);
            assert!(!feed.awaits_frame(), "the frame applied the jump");
            assert_eq!(feed.marker(), Some(9));
            scroll(&mut feed);
            assert_eq!(feed.marker(), Some(9), "the marker stays put");
            // And the marker is still drawn after the next frame.
            let _ = visible(&mut feed, &messages, 40, 10);
            assert_eq!(feed.marker(), Some(9));

            // Before any frame: the person scrolled first, and the jump is off.
            let mut feed = MessageFeed::new();
            feed.focus_message(&messages, messages[9].message_ids[0], None);
            assert!(feed.awaits_frame());
            scroll(&mut feed);
            assert!(!feed.awaits_frame(), "a manual scroll drops the request");
            assert_eq!(feed.marker(), Some(9), "but the marker stays put");
        }
    }

    /// The other half of the split: a rewrap must **not** pull the view back
    /// to the message once the person has scrolled away from it. (The view was
    /// once tied to the marked message itself, so a resize after scrolling
    /// away yanked you back.)
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

    /// A rewrap keeps what a reader **scrolled to** (docs/research/feed-landing.md,
    /// fork F2). `scroll` is a row number, and at another width that number is
    /// some other message: the view used to be kept for a jump alone, and a
    /// reader who had scrolled anywhere by hand was moved by a resize.
    #[test]
    fn a_resize_keeps_the_message_a_reader_scrolled_to() {
        let messages = numbered_long(12);
        let mut feed = MessageFeed::new();
        visible(&mut feed, &messages, 60, 10);
        feed.scroll_up(12);
        visible(&mut feed, &messages, 60, 10);
        let before = feed
            .place(&messages)
            .expect("a message is on the first row");
        let row_wide = feed.scroll_row();
        assert!(!feed.is_following(), "precondition: scrolled away");

        visible(&mut feed, &messages, 26, 10);
        let after = feed.place(&messages).unwrap();
        assert_eq!(
            after.message, before.message,
            "the message on the first row is the one that was there"
        );
        assert_ne!(
            feed.scroll_row(),
            row_wide,
            "the rewrap moved its row — otherwise the test proves nothing"
        );

        // And back: the same message again.
        visible(&mut feed, &messages, 60, 10);
        assert_eq!(feed.place(&messages).unwrap().message, before.message);
    }

    /// The same for the two keys that reshape every block: thoughts unfolded
    /// above the view (`Ctrl+T`) add rows there, and the reader stays on the
    /// message they were reading.
    #[test]
    fn unfolding_thoughts_above_the_view_keeps_the_message_on_the_screen() {
        let thoughts = "мысль ".repeat(40);
        let messages: Vec<FeedMessage> = (0..10)
            .map(|i| msg(FeedRole::Assistant, &format!("ответ-{i}"), &thoughts))
            .collect();
        let mut feed = MessageFeed::new();
        visible(&mut feed, &messages, 40, 10);
        feed.scroll_up(8);
        visible(&mut feed, &messages, 40, 10);
        let before = feed.place(&messages).unwrap();
        let rows = feed.scroll_row();

        feed.toggle_thoughts();
        visible(&mut feed, &messages, 40, 10);
        assert_eq!(feed.place(&messages).unwrap().message, before.message);
        assert!(
            feed.scroll_row() > rows,
            "the blocks above grew, and the view went down with its message"
        );

        feed.toggle_thoughts();
        visible(&mut feed, &messages, 40, 10);
        assert_eq!(feed.place(&messages).unwrap().message, before.message);
    }

    /// What a rewrap does not touch: a feed at its tail still follows it, and a
    /// conversation opened at its first row stays there though the frame last
    /// drawn was another conversation's, scrolled somewhere.
    #[test]
    fn a_rewrap_moves_neither_the_tail_nor_a_feed_opened_at_its_start() {
        let messages = numbered_long(12);
        let mut feed = MessageFeed::new();
        visible(&mut feed, &messages, 60, 10);
        visible(&mut feed, &messages, 26, 10);
        assert!(feed.is_following(), "the tail through a resize");

        feed.scroll_up(9);
        visible(&mut feed, &messages, 26, 10);
        assert!(
            feed.scroll_row() > 0,
            "precondition: the last frame is mid-feed"
        );
        // Another conversation, opened at its start; its rows are other rows.
        let other = numbered(9);
        feed.clear_focus();
        feed.scroll_to_top();
        feed.set_show_model_name(true);
        visible(&mut feed, &other, 26, 10);
        assert_eq!(feed.scroll_row(), 0);
    }

    /// The share of the way through a block: exact where the block kept its
    /// rows, the first row the first and the last the last where it did not.
    #[test]
    fn a_rewrapped_row_is_the_same_share_of_its_block() {
        for row in 0..7 {
            assert_eq!(rewrapped_row(row, 7, 7), row, "an untouched block");
        }
        assert_eq!(rewrapped_row(0, 9, 30), 0);
        assert_eq!(rewrapped_row(8, 9, 30), 29);
        assert_eq!(rewrapped_row(4, 9, 17), 8, "the middle stays the middle");
        assert_eq!(rewrapped_row(20, 21, 3), 2);
        assert_eq!(rewrapped_row(10, 21, 3), 1);
        assert_eq!(rewrapped_row(5, 9, 12), 6, "rounded down: 6.9 is row 6");
        assert_eq!(
            rewrapped_row(99, 9, 30),
            29,
            "a row past the block is its last"
        );
        assert_eq!(rewrapped_row(3, 1, 12), 0);
        assert_eq!(rewrapped_row(3, 5, 0), 0);
    }

    /// A block's lines from `(pieces, what it says)`: 0 says nothing.
    fn lines_of(lines: &[(usize, u64)]) -> (Vec<Said>, usize) {
        let mut row = 0;
        let said = lines
            .iter()
            .map(|&(pieces, key)| {
                let line = Said {
                    row,
                    pieces,
                    key: (key != 0).then_some(key),
                };
                row += pieces;
                line
            })
            .collect();
        (said, row)
    }

    /// Where row `row` of the block `was` is in the block `now`.
    fn found(was: &[(usize, u64)], row: usize, now: &[(usize, u64)]) -> Option<usize> {
        let (was, rows) = lines_of(was);
        let (now, now_rows) = lines_of(now);
        TopLine::of(&was, rows, row)?.row_in(&now, now_rows)
    }

    /// A row is a piece of a line, and the line is found again by what it
    /// says — at another width, where it and the lines above it are broken
    /// into other pieces, and with lines added or taken away above it.
    #[test]
    fn a_row_is_found_again_by_what_its_line_says() {
        // A header, a paragraph in three pieces, a blank row, a line, the separator.
        let was = [(1, 7), (3, 11), (1, 0), (1, 13), (1, 0)];
        // The same block: every row is itself.
        for row in 0..7 {
            assert_eq!(found(&was, row, &was), Some(row), "row {row}");
        }
        // Narrower: the paragraph in six pieces. Its first piece is its first,
        // its second of three a third of the way in, the line below it is
        // where it now is, and the blank row above that line is above it.
        let narrow = [(1, 7), (6, 11), (1, 0), (1, 13), (1, 0)];
        assert_eq!(found(&was, 1, &narrow), Some(1));
        assert_eq!(found(&was, 2, &narrow), Some(3));
        assert_eq!(found(&was, 5, &narrow), Some(8));
        assert_eq!(found(&was, 4, &narrow), Some(7), "the blank row over it");
        assert_eq!(
            found(&was, 6, &narrow),
            Some(9),
            "the separator, from the end"
        );
        // And back, from a line's first piece: exactly where it was.
        assert_eq!(found(&narrow, 8, &was), Some(5));
        assert_eq!(found(&narrow, 1, &was), Some(1));

        // A fold opened above: four lines more, and the line is still found —
        // where a line's number, or a share of the rows, would point elsewhere.
        let unfolded = [
            (1, 7),
            (1, 21),
            (2, 22),
            (1, 23),
            (1, 0),
            (3, 11),
            (1, 0),
            (1, 13),
            (1, 0),
        ];
        assert_eq!(found(&was, 5, &unfolded), Some(10));
        assert_eq!(found(&unfolded, 10, &was), Some(5));

        // Two lines that say the same are told apart by their order...
        let twice = [(1, 5), (1, 0), (1, 5), (1, 9)];
        let twice_narrow = [(2, 5), (1, 0), (2, 5), (1, 9)];
        assert_eq!(found(&twice, 2, &twice_narrow), Some(3));
        // ...and not told at all once there is another number of them.
        assert_eq!(found(&twice, 2, &[(1, 5), (1, 9)]), None);
        // A line that says something else now is not found.
        assert_eq!(
            found(&was, 5, &[(1, 7), (3, 11), (1, 0), (2, 99), (1, 0)]),
            None
        );
        // A row past the block is its last; a block with no rows has none.
        assert_eq!(found(&was, 99, &was), Some(6));
        assert_eq!(found(&[], 0, &was), None);
    }

    /// The lines of real rows: a wrapped paragraph is one line that says the
    /// same at two widths, and the separator under a message says nothing.
    #[test]
    fn a_blocks_lines_say_the_same_at_any_width() {
        let messages = numbered_long(1);
        let palette = Palette::default();
        let mut feed = MessageFeed::new();
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        let wide = feed.cache[0].said().to_vec();
        let _ = feed.build_lines(&messages, &palette, 24, ru());
        let narrow = feed.cache[0].said().to_vec();

        let keys = |said: &[Said]| said.iter().map(|l| l.key).collect::<Vec<_>>();
        assert_eq!(
            keys(&wide),
            keys(&narrow),
            "the same lines, saying the same"
        );
        assert!(
            narrow.iter().map(|l| l.pieces).sum::<usize>()
                > wide.iter().map(|l| l.pieces).sum::<usize>(),
            "in more pieces"
        );
        assert_eq!(wide.last().unwrap().key, None, "the separator says nothing");
        assert!(wide[0].key.is_some() && wide[0].key != wide[1].key);
    }

    /// A window dragged by its edge is rewrapped at every width it passes. The
    /// view is kept by the line on its first row, which no width changes: at
    /// every width that line is the first row, and the view comes back to the
    /// row it left. A share of the block's rows is not that — a long paragraph
    /// above takes more rows at a narrow width and the short lines under it do
    /// not, so the share points into the paragraph — and an error at every
    /// step walks the view away (found by the pty run of stage 3).
    #[test]
    fn a_window_dragged_through_many_widths_keeps_the_line_on_its_first_row() {
        let mut messages = numbered(8);
        let body = format!(
            "{}\n\nпервая строка\n\nвторая строка\n\nтретья строка\n\n{}",
            "длинный абзац ".repeat(30),
            "хвост абзаца ".repeat(30)
        );
        messages[3] = msg(FeedRole::Assistant, &body, "");
        let id = messages[3].message_ids[0];
        let mut feed = MessageFeed::new();
        // Put the view on the short line in the middle of the block.
        assert!(feed.focus_message(&messages, id, None));
        let mut rows = visible(&mut feed, &messages, 62, 10);
        while !rows[1].contains("вторая строка") {
            feed.scroll_down(1);
            rows = visible(&mut feed, &messages, 62, 10);
            assert!(!feed.is_following(), "the line is in the feed: {rows:?}");
        }
        let (left, row) = (rows, feed.scroll_row());

        for _ in 0..3 {
            for width in (24..62).rev().chain(25..=62) {
                let rows = visible(&mut feed, &messages, width, 10);
                assert!(
                    rows[1].contains("вторая строка"),
                    "at {width} columns the first row is {:?}",
                    rows[1]
                );
            }
        }
        assert_eq!(feed.scroll_row(), row, "the row it left");
        assert_eq!(visible(&mut feed, &messages, 62, 10), left);
    }

    /// A long message whose only match is far below its start, as the feed's
    /// `block`-th of a dozen.
    fn with_a_deep_match(block: usize, word: &str) -> Vec<FeedMessage> {
        let mut messages = numbered(12);
        let body = format!("{} {word} {}", "вода ".repeat(120), "вода ".repeat(20));
        messages[block] = msg(FeedRole::Assistant, &body, "");
        messages
    }

    /// A search hit lands with its **match** on the screen (research fork F3).
    /// The view used to be put on the message's first row, and in a message
    /// taller than the window the word that was searched for was below it.
    #[test]
    fn a_hit_deep_in_a_long_message_lands_with_the_match_on_the_screen() {
        let messages = with_a_deep_match(5, "жемчуг");
        let id = messages[5].message_ids[0];
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, id, Some("жемчуг")));
        let rows = visible(&mut feed, &messages, 40, 12);
        let at = rows
            .iter()
            .position(|r| r.contains("жемчуг"))
            .unwrap_or_else(|| panic!("the match is on the screen: {rows:?}"));
        // Ten rows of feed inside the border: the match a third of them down.
        assert_eq!(at, 1 + 10 / 3, "{rows:?}");
        let place = feed.place(&messages).unwrap();
        assert_eq!(place.message, id, "still inside the message that was hit");
        assert!(place.row > 0, "and not at its start");
        assert_eq!(feed.marker(), Some(5));

        // A resize after it keeps the view inside that message (fork F2).
        visible(&mut feed, &messages, 30, 12);
        assert_eq!(feed.place(&messages).unwrap().message, id);
    }

    /// Where the match is on the screenful that starts with the message, the
    /// message is shown from its start, as before — and so it is where no
    /// match is drawn at all, or the jump carries no query.
    #[test]
    fn a_hit_near_the_start_shows_the_message_from_its_start() {
        let messages = with_a_deep_match(5, "жемчуг");
        let id = messages[5].message_ids[0];
        let start = Some(FeedPlace {
            message: id,
            row: 0,
        });
        for query in [Some("вода"), Some("нет такого слова"), None] {
            let mut feed = MessageFeed::new();
            assert!(feed.focus_message(&messages, id, query));
            visible(&mut feed, &messages, 40, 12);
            assert_eq!(feed.place(&messages), start, "{query:?}");
        }
    }

    /// ...and so it does in the feed's **first** message, whose rows begin at
    /// row 0 — the one block where a range reckoned wrongly from its first
    /// row is empty (docs/research/cargo-mutants.md §10.3).
    #[test]
    fn a_hit_deep_in_the_feeds_first_message_lands_on_the_match_too() {
        let messages = with_a_deep_match(0, "жемчуг");
        let id = messages[0].message_ids[0];
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, id, Some("жемчуг")));
        let rows = visible(&mut feed, &messages, 40, 12);
        let at = rows
            .iter()
            .position(|r| r.contains("жемчуг"))
            .unwrap_or_else(|| panic!("the match is on the screen: {rows:?}"));
        assert_eq!(at, 1 + 10 / 3, "{rows:?}");
        assert!(feed.place(&messages).unwrap().row > 0);
    }

    /// The two cases of a reveal the others did not reach
    /// (docs/research/cargo-mutants.md §10.3): a view scrolled a few rows
    /// down and a bubble **below** it — its first row becomes the view's
    /// first; and a view at the feed's first row with the bubble on the screen
    /// — it stays where it is.
    #[test]
    fn reveal_from_a_scrolled_view_and_at_the_feeds_first_row() {
        let messages = numbered(12);
        let mut feed = MessageFeed::new();
        feed.scroll_to_top();
        visible(&mut feed, &messages, 40, 10);
        feed.scroll_down(4);
        visible(&mut feed, &messages, 40, 10);
        assert_eq!(feed.scroll_row(), 4, "precondition: a few rows down");
        let held = feed.place(&messages);
        let id = messages[8].message_ids[0];
        assert!(feed.reveal_message(&messages, id, held));
        let rows = visible(&mut feed, &messages, 40, 10);
        assert_eq!(
            feed.place(&messages),
            Some(FeedPlace {
                message: id,
                row: 0
            }),
            "{rows:?}"
        );

        let mut feed = MessageFeed::new();
        feed.scroll_to_top();
        visible(&mut feed, &messages, 40, 10);
        let held = feed.place(&messages);
        assert!(feed.reveal_message(&messages, messages[1].message_ids[0], held));
        let rows = visible(&mut feed, &messages, 40, 10);
        assert!(
            rows.iter().any(|r| r.contains("сообщение-1")),
            "precondition: the bubble is on the first screenful: {rows:?}"
        );
        assert_eq!(feed.scroll_row(), 0, "and the view did not move");
    }

    /// A logical line is its rows — the first, and how many a wrap broke it
    /// into — and what they say; a row that says nothing has no key. Read off
    /// rows made by hand, where every figure is known: the pieces of a line
    /// that is not its block's first, the last line's, and a last line that
    /// says something. Until now the keys were compared across widths and the
    /// rest was seen only through views it happened not to move
    /// (docs/research/cargo-mutants.md §10.3).
    #[test]
    fn a_blocks_lines_are_its_rows_and_what_they_say() {
        let hard = RowFact::default();
        let soft = RowFact {
            soft: true,
            ..RowFact::default()
        };
        let rows = |texts: &[&'static str]| -> Vec<Line<'static>> {
            texts.iter().map(|t| Line::from(*t)).collect()
        };
        let lines = rows(&["alpha ", "beta", "", "gamma ", "delta"]);
        let got = said(&lines, &[hard, soft, hard, hard, soft]);
        assert_eq!(
            got.iter().map(|l| (l.row, l.pieces)).collect::<Vec<_>>(),
            [(0, 2), (2, 1), (3, 2)]
        );
        assert!(got[0].key.is_some() && got[2].key.is_some(), "{got:?}");
        assert_eq!(got[1].key, None, "a blank row says nothing");
        assert_ne!(got[0].key, got[2].key);
        // The same line in one piece says the same.
        let whole = said(&rows(&["alpha beta"]), &[hard]);
        assert_eq!((whole[0].row, whole[0].pieces), (0, 1));
        assert_eq!(whole[0].key, got[0].key);

        // And on real rows, at two widths: the lines tile the block's rows.
        let messages = numbered_long(1);
        let palette = Palette::default();
        let mut feed = MessageFeed::new();
        for width in [60, 24] {
            let _ = feed.build_lines(&messages, &palette, width, ru());
            let block = &feed.cache[0];
            let lines = block.said();
            for pair in lines.windows(2) {
                assert_eq!(
                    pair[0].row + pair[0].pieces,
                    pair[1].row,
                    "{width}: {lines:?}"
                );
            }
            let last = lines.last().unwrap();
            assert_eq!(last.row + last.pieces, block.lines.len(), "{width}");
        }
    }

    /// A blank row is told by its distance — to the next line that says
    /// something, or to the block's end — whatever the distance is: the first
    /// of three blank rows, the first of two under the last line. With one
    /// blank row between two lines the distance is 1, and a wrong reckoning of
    /// it reads the same (docs/research/cargo-mutants.md §10.3).
    #[test]
    fn a_blank_row_is_found_by_its_distance_whatever_it_is() {
        // A header, three blank rows, a line in two pieces, two blank rows.
        let was = [(1, 7), (1, 0), (1, 0), (1, 0), (2, 13), (1, 0), (1, 0)];
        let (_, rows) = lines_of(&was);
        for row in 0..rows {
            assert_eq!(found(&was, row, &was), Some(row), "row {row}");
        }
        // The line in five pieces: the rows over it stand over it as they did,
        // and the two under it are as far from the end.
        let narrow = [(1, 7), (1, 0), (1, 0), (1, 0), (5, 13), (1, 0), (1, 0)];
        for row in 1..=3 {
            assert_eq!(found(&was, row, &narrow), Some(row), "row {row}");
        }
        assert_eq!(found(&was, 6, &narrow), Some(9));
        assert_eq!(found(&was, 7, &narrow), Some(10));
        // A line more above them: one row lower.
        let more = [
            (1, 7),
            (1, 21),
            (1, 0),
            (1, 0),
            (1, 0),
            (2, 13),
            (1, 0),
            (1, 0),
        ];
        assert_eq!(found(&was, 1, &more), Some(2));
    }

    /// A scroll no frame has drawn is kept through a rewrap too, by its share
    /// of the block: the frame recorded the line of the row **it** drew first,
    /// and the view is on another row now. The share needs how many rows the
    /// block had, and nothing read that (docs/research/cargo-mutants.md
    /// §10.3): with 0 or 1 for it, every such view went to its block's first
    /// row.
    #[test]
    fn a_scroll_not_yet_drawn_is_kept_through_a_rewrap_by_its_share() {
        let mut messages = numbered(6);
        messages[2] = msg(FeedRole::Assistant, &"слово ".repeat(150), "");
        let id = messages[2].message_ids[0];
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, id, None));
        visible(&mut feed, &messages, 60, 10);
        let (start, wide) = feed.block_rows(2).unwrap();
        assert_eq!(
            feed.scroll_row(),
            start,
            "precondition: the block's first row"
        );
        // Seven rows down, and no frame between this and the rewrap.
        feed.scroll_down(7);
        visible(&mut feed, &messages, 30, 10);
        let (_, narrow) = feed.block_rows(2).unwrap();
        assert!(wide > 8 && narrow > wide, "{wide} rows, then {narrow}");
        let place = feed.place(&messages).unwrap();
        assert_eq!(place.message, id);
        assert_eq!(place.row, rewrapped_row(7, wide, narrow));
        assert!(place.row > 7, "further into a block that grew");
    }

    /// A refresh rebuilds the open chat's feed in place, and a jump's mark is
    /// a feed index: it is read as a message before ([`MessageFeed::marked`])
    /// and put back after ([`MessageFeed::remark`]) — on the same message at
    /// its new index, the match in it still lit; gone, with the jump's
    /// highlight, when the message is gone; and an in-feed search, which marks
    /// nothing, keeps its highlight. No test called `remark`
    /// (docs/research/cargo-mutants.md §10.3).
    #[test]
    fn a_refresh_puts_the_jumps_mark_back_on_its_message() {
        let palette = Palette::default();
        let mut messages = numbered(4);
        messages.insert(0, FeedMessage::note("заметка"));
        let id = messages[3].message_ids[0];
        let mut feed = MessageFeed::new();
        assert!(feed.focus_message(&messages, id, Some("сообщение")));
        assert_eq!(feed.marker(), Some(3));
        let marked = feed.marked(&messages);
        assert_eq!(marked, Some(id));

        // The feed rebuilt without its note: the message is one index up.
        let rebuilt = messages[1..].to_vec();
        feed.remark(&rebuilt, marked);
        assert_eq!(feed.marker(), Some(2));
        let lines = feed.build_lines(&rebuilt, &palette, 60, ru());
        assert_eq!(
            accented(&lines, &palette),
            ["сообщение"],
            "the match in the marked message, and only it"
        );

        // The message gone: no mark, and the jump's highlight with it.
        let without: Vec<FeedMessage> = rebuilt
            .iter()
            .filter(|m| m.message_ids[0] != id)
            .cloned()
            .collect();
        feed.remark(&without, marked);
        assert_eq!(feed.marker(), None);
        assert!(feed.highlight.is_none());
        let lines = feed.build_lines(&without, &palette, 60, ru());
        assert!(accented(&lines, &palette).is_empty());

        // An in-feed search marks nothing and lights the whole feed: a refresh
        // leaves it lit.
        let mut feed = MessageFeed::new();
        feed.set_search(Some("сообщение"));
        let marked = feed.marked(&rebuilt);
        assert_eq!(marked, None);
        feed.remark(&rebuilt, marked);
        let lines = feed.build_lines(&rebuilt, &palette, 60, ru());
        assert_eq!(accented(&lines, &palette).len(), 4);
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

    /// Only the marker is a rendering input, so scrolling must **not**
    /// invalidate the block cache. This is the whole point of the split: with
    /// one shared field the first scroll after a jump re-ran markdown+syntect
    /// over the entire chat.
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
        assert!(!feed.cache_reset, "a scroll must not wipe the block cache");

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
        assert!(!feed.awaits_frame());
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
    fn clear_focus_drops_the_request_the_marker_and_the_highlight() {
        let messages = numbered(6);
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[3].message_ids[0], Some("сообщение"));
        assert!(feed.highlight.is_some());
        assert!(feed.awaits_frame());
        feed.clear_focus();
        assert!(!feed.awaits_frame());
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
                FeedFrame::Bordered {
                    title: "Чат",
                    caption: &caption("gemma-4", Some("16k ctx")),
                    counter: None,
                },
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
        let cap = caption("gemma-4", Some("16k ctx"));
        let header = FeedFrame::Bordered {
            title: long,
            caption: &cap,
            counter: None,
        };
        term.draw(|f| feed.render(f, f.area(), header, &[], &Palette::default(), ru()))
            .unwrap();
        let row = top(&term);
        assert!(row.contains('…'), "a cut title says so: {row}");
        assert!(row.contains(meta), "the meta keeps its corner: {row}");
        assert!(row.starts_with('╭') && row.ends_with('╮'), "{row}");
        // Room for all of it — no marker, and the title is whole.
        let mut term = Terminal::new(TestBackend::new(100, 8)).unwrap();
        term.draw(|f| feed.render(f, f.area(), header, &[], &Palette::default(), ru()))
            .unwrap();
        let row = top(&term);
        assert!(row.contains(long), "{row}");
        assert!(!row.contains('…'), "{row}");
    }

    /// The header's row is shared title-first (spec §11.3): the caption is
    /// whole while the title still gets half the row — or all of itself —
    /// then gives up its context size, then itself. A title longer than its
    /// share is the one cut, as it always was, where the row has room for the
    /// caption.
    #[test]
    fn the_header_gives_the_title_its_share_before_the_caption() {
        let cap = caption("gemma-4-12B-it-Q5_K_M", Some("16k ctx"));
        let title = "Gemma 4 on a 12 GB GPU"; // 22 columns
        let fit = |avail: usize| fit_header(title, &cap, avail);
        let whole = Some("gemma-4-12B-it-Q5_K_M · 16k ctx".to_string());
        let name = Some("gemma-4-12B-it-Q5_K_M".to_string());

        // Room for both: the cell is the caption and a space either side.
        assert_eq!(fit(22 + 33), (title.to_string(), whole.clone()));
        // One column short of that: the context size goes, the title stays.
        assert_eq!(fit(22 + 32), (title.to_string(), name.clone()));
        assert_eq!(fit(22 + 23), (title.to_string(), name));
        // No room for the name beside a whole title: the title alone.
        assert_eq!(fit(22 + 22), (title.to_string(), None));
        // Less than the title: it is cut, and says so.
        let (cut, none) = fit(12);
        assert!(cut.ends_with('…') && wrap::str_width(&cut) <= 12, "{cut:?}");
        assert_eq!(none, None);
        assert_eq!(fit(0), (String::new(), None));

        // A title longer than half the row is owed half: the caption stays
        // whole, and the title is what is cut — the wide-window rule, as before.
        let long = "a title that goes on for much longer than any header has room for";
        let (cut, shown) = fit_header(long, &cap, 110);
        assert_eq!(shown, whole);
        assert_eq!(
            wrap::str_width(&cut),
            65,
            "whole: it fits what the caption left"
        );
        let (cut, shown) = fit_header(long, &cap, 80);
        assert_eq!(shown, whole);
        assert!(
            cut.ends_with('…') && wrap::str_width(&cut) == 80 - 33,
            "{cut:?}"
        );

        // A caption with no detail has one form. A short one stays for as
        // long as it leaves the title half the row — the title is then the
        // one cut — and goes when it would not.
        let plain = caption("gpt-6-sol", None);
        let short = Some("gpt-6-sol".to_string());
        assert_eq!(
            fit_header(title, &plain, 33),
            (title.to_string(), short.clone())
        );
        let (cut, shown) = fit_header(title, &plain, 21);
        assert_eq!((wrap::str_width(&cut), shown), (10, short));
        assert_eq!(fit_header(title, &plain, 20).1, None);
        // An empty caption has no form at all.
        assert_eq!(fit_header(title, &BARE, 60), (title.to_string(), None));
        assert_eq!(cap.full(), "gemma-4-12B-it-Q5_K_M · 16k ctx");
    }

    /// The symptom the rule answers, on the border itself: a 45-column header
    /// showed five letters of the title beside a whole model caption, and a
    /// 30-column one no title and a caption clipped from the left.
    #[test]
    fn a_narrow_header_keeps_the_title_and_sheds_the_caption() {
        let cap = caption("gemma-4-12B-it-Q5_K_M", Some("16k ctx"));
        let title = "Gemma 4 on a 12 GB GPU";
        let header = FeedFrame::Bordered {
            title,
            caption: &cap,
            counter: None,
        };
        let top = |width: u16| -> String {
            let mut feed = MessageFeed::new();
            let mut term = Terminal::new(TestBackend::new(width, 5)).unwrap();
            term.draw(|f| feed.render(f, f.area(), header, &[], &Palette::default(), ru()))
                .unwrap();
            crate::shared::ui::tests::buffer_rows(term.backend().buffer()).remove(0)
        };
        let row = top(116);
        assert!(
            row.contains(title) && row.contains("Q5_K_M · 16k ctx"),
            "{row}"
        );
        let row = top(57);
        assert!(
            row.contains(title) && row.contains("Q5_K_M ") && !row.contains("ctx"),
            "{row}"
        );
        for width in [45, 30] {
            let row = top(width);
            assert!(
                row.contains(title) && !row.contains("Q5_K_M"),
                "{width}: {row}"
            );
            assert!(
                row.starts_with("╭ ◆ ") && row.ends_with('╮'),
                "{width}: {row}"
            );
        }
    }

    /// The token counter sits at the bottom border's right corner (spec
    /// §11.3), in its turn's style, and fits the row the way a caption does:
    /// the notes go first, then the counter — never a cut number. The
    /// scrollbar on the right border leaves the corner row alone.
    #[test]
    fn the_counter_sits_at_the_bottom_right_corner() {
        let counter = TokenCounter {
            tokens: 10,
            context: Some(14577),
            context_exact: true,
            reasoning: 10,
            cached: Some(13300),
            ..TokenCounter::default()
        };
        let draw = |width: u16, counter: Option<TokenCounter>, messages: &[FeedMessage]| {
            let header = FeedFrame::Bordered {
                title: "Чат",
                caption: &BARE,
                counter,
            };
            let mut feed = MessageFeed::new();
            let mut term = Terminal::new(TestBackend::new(width, 6)).unwrap();
            term.draw(|f| feed.render(f, f.area(), header, messages, &Palette::default(), ru()))
                .unwrap();
            term.backend().buffer().clone()
        };
        let bottom = |buf: &ratatui::buffer::Buffer| {
            crate::shared::ui::tests::buffer_rows(buf).pop().unwrap()
        };
        let full = " токены: 14587 (рассужд. 10 · кэш 91%) ";
        let row = bottom(&draw(80, Some(counter), &[]));
        assert!(row.ends_with(&format!("{full}╯")), "{row}");
        assert!(row.starts_with("╰─"), "{row}");
        // Narrower than the full form: the notes go, the number stays whole.
        let row = bottom(&draw(30, Some(counter), &[]));
        assert!(row.ends_with(" токены: 14587 ╯"), "{row}");
        assert!(!row.contains("рассужд"), "{row}");
        // Narrower than the bare number: no counter, an unbroken border.
        let row = bottom(&draw(17, Some(counter), &[]));
        assert!(!row.contains("14587") && row.ends_with("─╯"), "{row}");
        // No counter, or nothing counted yet: the border as it always was.
        for none in [None, Some(TokenCounter::default())] {
            let row = bottom(&draw(80, none, &[]));
            assert_eq!(row, format!("╰{}╯", "─".repeat(78)));
        }
        // Muted after the turn, bright during it.
        let p = Palette::default();
        let at = |buf: &ratatui::buffer::Buffer| buf[(70, 5)].style().fg;
        assert_eq!(at(&draw(80, Some(counter), &[])), Some(p.muted));
        let live = TokenCounter {
            generating: true,
            ..counter
        };
        assert_eq!(at(&draw(80, Some(live), &[])), Some(p.accent));
        // An overflowing feed draws its scrollbar and keeps the counter whole.
        let many: Vec<FeedMessage> = (0..30)
            .map(|i| msg(FeedRole::User, &format!("строка {i}"), ""))
            .collect();
        let buf = draw(80, Some(counter), &many);
        assert!(bottom(&buf).ends_with(&format!("{full}╯")));
        let right: String = (1..5).map(|y| buf[(79, y)].symbol().to_string()).collect();
        assert!(right.contains('█'), "{right}");
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
        term.draw(|f| feed.render(f, f.area(), PANEL, &short, &Palette::default(), ru()))
            .unwrap();
        assert!(
            !right_col(&term).iter().any(|s| s == "█"),
            "a short feed — no scrollbar thumb"
        );
        let many: Vec<FeedMessage> = (0..30)
            .map(|i| msg(FeedRole::User, &format!("строка {i}"), ""))
            .collect();
        term.draw(|f| feed.render(f, f.area(), PANEL, &many, &Palette::default(), ru()))
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
                        let view = FeedView {
                            thoughts,
                            tools,
                            ..FeedView::default()
                        };
                        let mut warm = MessageFeed::new();
                        warm.set_view(view.clone());
                        warm.set_compaction(compaction.clone());
                        // warm the cache with repeated calls
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let warm_lines = warm.build_lines(messages, &palette, width, ru());

                        let mut fresh = MessageFeed::new();
                        fresh.set_view(view.clone());
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
                fold: None,
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

    /// The index of the compaction divider's last row among rendered rows.
    /// A block's rows come wrapped to the panel (the selection addresses them
    /// as drawn), so on a narrow one the label runs over two.
    fn divider_row(rows: &[String]) -> Option<usize> {
        let mut words = compacted_label().split_whitespace();
        let (first, last) = (words.next()?, words.last()?);
        let start = rows.iter().position(|r| r.contains(first))?;
        rows[start..]
            .iter()
            .position(|r| r.contains(last))
            .map(|i| start + i)
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

    // ---- the monochrome mode (spec §11.6, docs/history/theme-modes.md §9) ----

    /// The rail of every row of a built feed — its first span.
    fn rails(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .filter(|l| l.spans.len() > 1)
            .map(|l| l.spans[0].content.to_string())
            .collect()
    }

    /// The rail's colour is what says whose message a row belongs to once its
    /// header has scrolled away. Without colours the rail says it by shape —
    /// and keeps its two columns, so nothing under it moves.
    #[test]
    fn the_rail_says_the_role_by_shape_where_it_has_no_colour() {
        let one = |role: FeedRole, palette: &Palette| -> Vec<String> {
            let messages = vec![msg(role, "first line\n\nsecond line", "")];
            let mut rails = rails(&MessageFeed::new().build_lines(&messages, palette, 40, ru()));
            rails.dedup();
            rails
        };
        let mono = Palette::mono();
        assert_eq!(one(FeedRole::User, &mono), [RAIL_MONO_USER]);
        assert_eq!(one(FeedRole::Assistant, &mono), [RAIL_MONO_ASSISTANT]);
        assert_eq!(one(FeedRole::System, &mono), [RAIL_MONO_QUIET]);
        assert_eq!(one(FeedRole::Note, &mono), [RAIL_MONO_QUIET]);

        let shapes = [
            RAIL_MONO_USER,
            RAIL_MONO_ASSISTANT,
            RAIL_MONO_QUIET,
            RAIL_MONO_MARKED,
        ];
        for (i, a) in shapes.iter().enumerate() {
            assert_eq!(wrap::str_width(a), wrap::str_width(RAIL), "{a:?}");
            for b in &shapes[i + 1..] {
                assert_ne!(a, b, "two roles under one rail");
            }
        }

        // Every other mode draws the one rail it always drew.
        for role in [FeedRole::User, FeedRole::Assistant, FeedRole::Note] {
            assert_eq!(one(role, &Palette::default()), [RAIL], "{role:?}");
        }
    }

    /// The message a jump landed on is marked by its rail's colour — by the
    /// rail's shape where there is none — and the word that was searched for
    /// is reverse video inside it (fork A: a search match is one of the two
    /// things that keep it).
    #[test]
    fn a_jump_marks_its_message_and_its_match_without_colour() {
        use crate::shared::theme::MONO_MARK;
        let palette = Palette::mono();
        let messages = vec![
            msg(FeedRole::User, "nothing to see", ""),
            msg(FeedRole::Assistant, "here is the **marker** you wanted", ""),
        ];
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[1].message_ids[0], Some("marker"));
        let lines = feed.build_lines(&messages, &palette, 60, ru());

        let mut seen = rails(&lines);
        seen.dedup();
        assert_eq!(seen, [RAIL_MONO_USER, RAIL_MONO_MARKED]);

        let marked: Vec<&Span> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.bg == Some(MONO_MARK))
            .collect();
        let text: String = marked.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "marker");
        for span in marked {
            assert_ne!(
                span.style.fg,
                Some(palette.accent),
                "the mark is the backdrop, the text keeps its own style"
            );
            assert!(
                span.style.add_modifier.contains(Modifier::BOLD),
                "what was under the match is left under it"
            );
        }
        // …and the emphasis around the word is said in text.
        assert!(
            joined(&lines).contains("the **marker** you"),
            "{}",
            joined(&lines)
        );
    }

    /// In-feed search (`Ctrl+F`) marks every match in the chat the same way.
    #[test]
    fn in_feed_search_marks_every_match_without_colour() {
        use crate::shared::theme::MONO_MARK;
        let messages = vec![
            msg(FeedRole::User, "a needle here", ""),
            msg(FeedRole::Assistant, "and a needle there", ""),
        ];
        let count = |palette: &Palette| -> (usize, usize) {
            let mut feed = MessageFeed::new();
            feed.set_search(Some("needle"));
            let lines = feed.build_lines(&messages, palette, 60, ru());
            let on_mark = lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .filter(|s| s.style.bg == Some(MONO_MARK) && s.content == "needle")
                .count();
            (on_mark, accented(&lines, palette).len())
        };
        assert_eq!(count(&Palette::mono()).0, 2);
        assert_eq!(count(&Palette::default()), (0, 2), "colour, and no mark");
    }

    /// A `chat://` address that opens a chat is a link, one that does not is
    /// text — and the link style is all that says which. Without it the
    /// address is bracketed, and the click map still finds it.
    #[test]
    fn a_resolvable_address_is_bracketed_where_it_cannot_be_styled() {
        let id = chat_uuid(1);
        let unknown = "chat://0badc0de";
        let text = format!(
            "see {} and also {unknown}, then **{}**.",
            crate::features::chat_links::uri(id),
            crate::features::chat_links::uri(id),
        );
        let msgs = vec![msg(FeedRole::Assistant, &text, "")];
        let built = |palette: &Palette| -> (String, Vec<Uuid>, MessageFeed) {
            let mut feed = MessageFeed::new();
            feed.set_known_chats(vec![id]);
            let lines = feed.build_lines(&msgs, palette, 100, ru());
            (flatten(&lines), feed.chat_links(), feed)
        };

        let (mono, links, _) = built(&Palette::mono());
        assert!(mono.contains("see <chat://1a2b3c4d> and"), "{mono}");
        assert!(
            mono.contains("**<chat://1a2b3c4d>**."),
            "inside a styled span, and at its very end: {mono}"
        );
        assert!(
            mono.contains(&format!("also {unknown},")),
            "an address that opens nothing stays as written: {mono}"
        );
        assert_eq!(mono.matches('<').count(), 2, "{mono}");
        assert_eq!(mono.matches('>').count(), 2, "{mono}");
        assert_eq!(links, vec![id], "the picker lists what the feed marks");

        let (plain, links, _) = built(&Palette::default());
        assert!(!plain.contains('<') && !plain.contains('>'), "{plain}");
        assert_eq!(links, vec![id]);
    }

    /// The click map is rebuilt from the drawn rows, so the brackets around
    /// an address shift where it is clicked — by the one column of the `<`.
    #[test]
    fn a_bracketed_address_is_clicked_where_it_is_drawn() {
        let id = chat_uuid(1);
        let text = format!("see {}", crate::features::chat_links::uri(id));
        let msgs = vec![msg(FeedRole::Assistant, &text, "")];
        let hit = |palette: &Palette| -> LinkHit {
            let mut feed = MessageFeed::new();
            feed.set_known_chats(vec![id]);
            let lines = feed.build_lines(&msgs, palette, 60, ru());
            let hits = visible_link_hits(&lines, Rect::new(0, 0, 60, 10), 0, &[id]);
            assert_eq!(hits.len(), 1, "{hits:?}");
            hits[0]
        };
        let (plain, mono) = (hit(&Palette::default()), hit(&Palette::mono()));
        assert_eq!(mono.chat, id);
        assert_eq!(mono.row, plain.row);
        assert_eq!(mono.start, plain.start + 1, "past the opening bracket");
        assert_eq!(mono.end - mono.start, plain.end - plain.start);
    }

    /// Muted italic is what tells a wrapped row of a thought from the reply
    /// under it. Without it the gutter is all there is, so it is on every
    /// row — and the rows still fit the panel.
    #[test]
    fn a_thought_keeps_its_gutter_on_every_row_without_styling() {
        let thought =
            "a long thought that certainly does not fit on a single row of a narrow panel";
        let messages = vec![msg(FeedRole::Assistant, "the reply", thought)];
        let built = |palette: &Palette, width: usize| -> Vec<String> {
            let mut feed = MessageFeed::new();
            feed.toggle_thoughts();
            row_texts(&feed.build_lines(&messages, palette, width, ru()))
        };
        let width = 30;
        let label = ru().t("ui.feed.thoughts");

        let rows = built(&Palette::mono(), width);
        let from = rows.iter().position(|r| r.contains(label)).unwrap() + 1;
        let to = rows.iter().position(|r| r.contains("the reply")).unwrap();
        let thought_rows: Vec<&String> = rows[from..to]
            .iter()
            .filter(|r| !r.trim().is_empty())
            .collect();
        assert!(thought_rows.len() > 2, "the premise: it wraps: {rows:#?}");
        for row in &thought_rows {
            assert!(
                row.starts_with(&format!("{RAIL_MONO_ASSISTANT}{GUTTER}")),
                "a row of the thought without its gutter: {row:?}"
            );
            assert!(wrap::str_width(row) <= width, "{row:?}");
        }
        let text: String = thought_rows
            .iter()
            .map(|r| r.trim_start_matches(&format!("{RAIL_MONO_ASSISTANT}{GUTTER}")))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(
            text.split_whitespace().collect::<Vec<_>>(),
            thought.split_whitespace().collect::<Vec<_>>(),
            "nothing of the thought is lost to the wrap"
        );

        // Every other mode builds the line it always built: one gutter, and
        // the wrap left to the feed.
        let rows = built(&Palette::default(), 200);
        assert!(
            rows.contains(&format!("{RAIL}{GUTTER}{thought}")),
            "{rows:#?}"
        );
    }

    /// The compaction summary is the same kind of block on the same gutter.
    #[test]
    fn the_compaction_summary_keeps_its_gutter_on_every_row_without_styling() {
        let summary = "earlier the two of them discussed a great many things at length";
        let messages = numbered(3);
        let mut feed = MessageFeed::new();
        feed.set_compaction(Some((messages[1].message_ids[0], summary.into())));
        feed.toggle_thoughts();
        let width = 28;
        let rows = row_texts(&feed.build_lines(&messages, &Palette::mono(), width, ru()));
        let at = divider_row(&rows).expect("the divider must be drawn");
        let summary_rows: Vec<&String> = rows[at + 1..]
            .iter()
            .take_while(|r| !r.trim().is_empty())
            .collect();
        assert!(summary_rows.len() > 2, "the premise: it wraps: {rows:#?}");
        for row in &summary_rows {
            assert!(row.starts_with(GUTTER), "{row:?}");
            assert!(wrap::str_width(row) <= width, "{row:?}");
        }
    }

    // ---------- the selection (docs/research/mouse-always-captured.md §5.3–§5.4) ----------

    /// Draws the feed in a `w`×`h` terminal with the bordered panel.
    fn draw_with(
        term: &mut Terminal<TestBackend>,
        feed: &mut MessageFeed,
        msgs: &[FeedMessage],
        palette: &Palette,
    ) {
        term.draw(|f| feed.render(f, f.area(), PANEL, msgs, palette, ru()))
            .unwrap();
    }

    fn drawn(
        feed: &mut MessageFeed,
        msgs: &[FeedMessage],
        w: u16,
        h: u16,
    ) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        draw_with(&mut term, feed, msgs, &Palette::default());
        term
    }

    /// The screen cell where `needle` first appears — one cell per character,
    /// so for text without wide glyphs.
    fn cell_of(term: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
        let buf = term.backend().buffer();
        for y in 0..buf.area.height {
            let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            let text: String = row.concat();
            if let Some(at) = text.find(needle) {
                return (text[..at].chars().count() as u16, y);
            }
        }
        panic!("{needle:?} is not on screen");
    }

    #[test]
    fn a_drag_over_a_wrapped_paragraph_copies_it_as_one_line() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let msgs = vec![msg(FeedRole::User, text, "")];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 24, 12);
        let (c0, r0) = cell_of(&term, "alpha");
        let (c1, r1) = cell_of(&term, "kappa");
        assert!(r1 > r0 + 1, "the premise: the paragraph wraps over rows");
        assert!(feed.press(c0, r0, SelectUnit::Char));
        assert!(
            feed.selected_text().is_none(),
            "a press alone selects nothing"
        );
        assert!(feed.drag(c1 + 4, r1));
        assert_eq!(feed.selected_text().as_deref(), Some(text));
        // Backwards — from the end to the start — is the same text.
        feed.release();
        assert!(feed.clear_selection());
        assert!(feed.press(c1 + 4, r1, SelectUnit::Char));
        assert!(feed.drag(c0, r0));
        assert_eq!(feed.selected_text().as_deref(), Some(text));
    }

    #[test]
    fn a_drag_over_a_code_block_copies_its_lines_without_padding_or_rail() {
        let msgs = vec![msg(
            FeedRole::Assistant,
            "Code:\n\n```\nfn main() {\n    run();\n}\n```\n\nDone.",
            "",
        )];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 40, 16);
        let (c0, r0) = cell_of(&term, "fn main");
        let (_, r1) = cell_of(&term, "}");
        assert!(feed.press(c0, r0, SelectUnit::Char));
        // Past the right edge: the pointer is clamped to the panel.
        assert!(feed.drag(60, r1));
        assert_eq!(
            feed.selected_text().as_deref(),
            Some("fn main() {\n    run();\n}")
        );
    }

    /// The highlight is the selection backdrop on the selected glyphs — not on
    /// the rail, not past the row's text — and in the monochrome mode the mark
    /// the frame's last pass turns into reverse video.
    #[test]
    fn the_highlight_is_the_selection_style_in_both_colour_modes() {
        let msgs = vec![msg(FeedRole::User, "one two three", "")];
        for palette in [Palette::default(), Palette::mono()] {
            let mut feed = MessageFeed::new();
            let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
            draw_with(&mut term, &mut feed, &msgs, &palette);
            let (c, r) = cell_of(&term, "two");
            assert!(feed.press(c, r, SelectUnit::Char));
            assert!(feed.drag(c + 2, r));
            draw_with(&mut term, &mut feed, &msgs, &palette);
            let buf = term.backend().buffer();
            let marked = palette.selection(Style::new()).bg;
            let bg = |x: u16| buf[(x, r)].bg;
            for x in c..c + 3 {
                assert_eq!(Some(bg(x)), marked, "{x} in the selection");
            }
            assert_ne!(Some(bg(c - 1)), marked, "the space before it");
            assert_ne!(Some(bg(c + 3)), marked, "the space after it");
            // A drag from the rail takes the row from its first glyph.
            feed.release();
            assert!(feed.press(1, r, SelectUnit::Char));
            assert!(feed.drag(c + 2, r));
            draw_with(&mut term, &mut feed, &msgs, &palette);
            let buf = term.backend().buffer();
            assert_ne!(Some(buf[(1, r)].bg), marked, "the rail");
            assert_ne!(Some(buf[(2, r)].bg), marked, "the rail's space");
            assert_eq!(Some(buf[(3, r)].bg), marked, "the first glyph");
            assert_eq!(feed.selected_text().as_deref(), Some("one two"));
        }
    }

    /// Every change that rewraps the rows drops the selection (research F9);
    /// a message appended below keeps it.
    #[test]
    fn a_rewrap_drops_the_selection_and_a_message_below_keeps_it() {
        let base = vec![
            msg(FeedRole::User, "first message here", ""),
            msg(FeedRole::Assistant, "second message there", "a thought"),
        ];
        let select = |feed: &mut MessageFeed, msgs: &[FeedMessage]| -> Terminal<TestBackend> {
            let term = drawn(feed, msgs, 40, 16);
            let (c, r) = cell_of(&term, "first");
            assert!(feed.press(c, r, SelectUnit::Char));
            assert!(feed.drag(c + 4, r));
            feed.release();
            assert_eq!(feed.selected_text().as_deref(), Some("first"));
            term
        };
        // Appended below: kept, the same text.
        let mut feed = MessageFeed::new();
        let mut term = select(&mut feed, &base);
        let mut longer = base.clone();
        longer.push(msg(FeedRole::User, "third", ""));
        draw_with(&mut term, &mut feed, &longer, &Palette::default());
        assert_eq!(feed.selected_text().as_deref(), Some("first"));
        // A resize.
        let mut feed = MessageFeed::new();
        select(&mut feed, &base);
        drawn(&mut feed, &base, 36, 16);
        assert!(!feed.has_selection(), "a resize");
        // Ctrl+T and Ctrl+O.
        for toggle in [MessageFeed::toggle_thoughts, MessageFeed::toggle_tools] {
            let mut feed = MessageFeed::new();
            let mut term = select(&mut feed, &base);
            toggle(&mut feed);
            draw_with(&mut term, &mut feed, &base, &Palette::default());
            assert!(!feed.has_selection(), "a fold toggled");
        }
        // The theme.
        let mut feed = MessageFeed::new();
        let mut term = select(&mut feed, &base);
        let light = Palette::built_in("light").unwrap();
        draw_with(&mut term, &mut feed, &base, &light);
        assert!(!feed.has_selection(), "the theme");
    }

    /// A block the selection covers made anew — rewritten, or taken back —
    /// drops it; one outside it does not.
    #[test]
    fn a_selected_block_rebuilt_or_removed_drops_the_selection() {
        let base = vec![
            msg(FeedRole::User, "keep this", ""),
            msg(FeedRole::Assistant, "answer one", ""),
        ];
        let select_answer = |feed: &mut MessageFeed| -> Terminal<TestBackend> {
            let term = drawn(feed, &base, 40, 16);
            let (c, r) = cell_of(&term, "answer");
            assert!(feed.press(c, r, SelectUnit::Word));
            assert_eq!(feed.selected_text().as_deref(), Some("answer"));
            feed.release();
            term
        };
        let mut feed = MessageFeed::new();
        let mut term = select_answer(&mut feed);
        let mut rewritten = base.clone();
        rewritten[1].text = "answer two".into();
        draw_with(&mut term, &mut feed, &rewritten, &Palette::default());
        assert!(!feed.has_selection(), "its block rewritten");

        let mut feed = MessageFeed::new();
        let mut term = select_answer(&mut feed);
        draw_with(&mut term, &mut feed, &base[..1], &Palette::default());
        assert!(!feed.has_selection(), "its block taken back");

        // The first block's selection survives the second's rewrite.
        let mut feed = MessageFeed::new();
        let mut term = drawn(&mut feed, &base, 40, 16);
        let (c, r) = cell_of(&term, "keep");
        assert!(feed.press(c, r, SelectUnit::Word));
        draw_with(&mut term, &mut feed, &rewritten, &Palette::default());
        assert_eq!(feed.selected_text().as_deref(), Some("keep"));
    }

    /// A double press takes the word, a triple the line — at once, before any
    /// drag; a drag after a double press grows by words.
    #[test]
    fn a_double_and_a_triple_press_select_a_word_and_a_line() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let msgs = vec![msg(FeedRole::User, text, "")];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 24, 12);
        let (c, r) = cell_of(&term, "gamma");
        assert!(feed.press(c + 2, r, SelectUnit::Word));
        assert_eq!(feed.selected_text().as_deref(), Some("gamma"));
        let (d, dr) = cell_of(&term, "delta");
        assert!(feed.drag(d + 1, dr));
        assert_eq!(feed.selected_text().as_deref(), Some("gamma delta"));
        feed.release();
        assert!(feed.press(c + 2, r, SelectUnit::Line));
        assert_eq!(
            feed.selected_text().as_deref(),
            Some(text),
            "the whole wrapped line"
        );
    }

    /// A click keeps the selection only on its highlight. The empty cells
    /// beside the rows it spans lie between its ends in reading order, and a
    /// click there kept it — the owner found it in every terminal, on the long
    /// selections a scroll makes (2026-11-15).
    #[test]
    fn only_the_highlight_holds_the_selection() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let msgs = vec![msg(FeedRole::User, text, "")];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 24, 12);
        let (c0, r0) = cell_of(&term, "alpha");
        let (c1, r1) = cell_of(&term, "kappa");
        assert!(feed.press(c0, r0, SelectUnit::Char));
        assert!(feed.drag(c1 + 4, r1));
        feed.release();
        let middle = r0 + 1;
        assert!(feed.selection_holds(c0 + 1, middle), "on the highlight");
        assert!(
            !feed.selection_holds(21, middle),
            "beside a middle row's text"
        );
        assert!(!feed.selection_holds(1, middle), "on the rail");
        assert!(feed.selection_holds(c1 + 4, r1), "the last glyph");
        assert!(!feed.selection_holds(c0, r1 + 1), "the separator below");
    }

    /// A double or a triple click beside the text selects nothing — it would
    /// otherwise take the row's last word, after a click meant to end a
    /// selection — and on the text it selects as ever.
    #[test]
    fn a_double_click_beside_the_text_selects_nothing() {
        let msgs = vec![msg(FeedRole::User, "one two three", "")];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 30, 8);
        let (c, r) = cell_of(&term, "three");
        for unit in [SelectUnit::Word, SelectUnit::Line] {
            assert!(
                feed.press(c + 10, r, unit),
                "{unit:?}: a press all the same"
            );
            assert!(!feed.has_selection(), "{unit:?} beside the text");
            feed.release();
            assert!(feed.press(1, r, unit));
            assert!(!feed.has_selection(), "{unit:?} on the rail");
            feed.release();
        }
        assert!(feed.press(c + 1, r, SelectUnit::Word));
        assert_eq!(feed.selected_text().as_deref(), Some("three"));
    }

    /// A drag held past the panel's top edge scrolls it a row a step, and the
    /// selection grows to every row it passes — none skipped (Claude Code
    /// #83961) — until the top, where it stops.
    #[test]
    fn a_drag_past_the_edge_scrolls_and_the_copy_holds_every_row_it_covered() {
        let msgs = numbered(30);
        let mut feed = MessageFeed::new();
        let mut term = drawn(&mut feed, &msgs, 30, 12);
        let (c, r) = cell_of(&term, "сообщение-29");
        // On its last character: the press's cell is the selection's end.
        assert!(feed.press(c + 11, r, SelectUnit::Char));
        assert!(feed.drag(c, 0), "above the panel's rows: on its top row");
        let mut steps = 0;
        let mut now = std::time::Instant::now();
        while feed.autoscroll(now) {
            // Not again before the step is due.
            assert!(!feed.autoscroll(now));
            draw_with(&mut term, &mut feed, &msgs, &Palette::default());
            now += AUTOSCROLL_STEP;
            steps += 1;
            assert!(steps < 1000, "it stops at the top");
        }
        assert!(steps > 20, "the premise: it scrolled a long way ({steps})");
        assert_eq!(feed.scroll_row(), 0);
        let text = feed.selected_text().unwrap();
        for i in 0..30 {
            assert!(text.contains(&format!("сообщение-{i}")), "{i}: {text}");
        }
        // Released, nothing scrolls on.
        feed.release();
        assert!(!feed.autoscroll(now + AUTOSCROLL_STEP));
    }

    /// While the assistant answers the feed selects nothing, and a selection
    /// standing then is dropped (research F6).
    #[test]
    fn nothing_is_selectable_while_the_assistant_answers() {
        let msgs = vec![msg(FeedRole::User, "a question", "")];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 30, 8);
        let (c, r) = cell_of(&term, "question");
        assert!(feed.press(c, r, SelectUnit::Word));
        assert!(feed.has_selection());
        feed.set_selectable(false);
        assert!(!feed.has_selection(), "dropped as the reply starts");
        assert!(!feed.press(c, r, SelectUnit::Word));
        assert!(!feed.drag(c + 3, r));
        assert!(!feed.has_selection());
        feed.set_selectable(true);
        assert!(feed.press(c, r, SelectUnit::Word), "and back once it ends");
    }

    /// A click keeps the selection where it lands inside it; the empty feed
    /// has no rows to select.
    #[test]
    fn the_selection_knows_its_cells_and_an_empty_feed_has_none() {
        let msgs = vec![msg(FeedRole::User, "one two three", "")];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 30, 8);
        let (c, r) = cell_of(&term, "two");
        assert!(feed.holds(c, r));
        assert!(feed.press(c, r, SelectUnit::Word));
        assert!(feed.selection_holds(c + 1, r));
        assert!(!feed.selection_holds(c + 4, r));
        assert!(!feed.holds(0, 0), "the border");
        assert!(feed.clear_selection());
        assert!(!feed.clear_selection(), "nothing left to end");

        let mut empty = MessageFeed::new();
        drawn(&mut empty, &[], 30, 8);
        assert!(!empty.holds(5, 3));
        assert!(!empty.press(5, 3, SelectUnit::Char));
    }

    /// The rows a block holds are the rows drawn: every one fits the panel, so
    /// `render` draws them unwrapped and a selection addresses them as
    /// (block, row) — the compaction divider, folds open, a tool card, a
    /// table and a diagram included.
    #[test]
    fn every_cached_row_fits_the_panel() {
        let mut assistant = msg(
            FeedRole::Assistant,
            "| a | b |\n|---|---|\n| one long cell of text | two |\n\n```mermaid\ngraph TD\nA-->B\n```\n\nand a paragraph that is long enough to wrap over several rows",
            "a thought that is also rather long and wraps over the rows of the panel",
        );
        assistant.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: "{\"query\":\"a query long enough to wrap over the panel's rows\"}".into(),
            result: "a result that is also long enough to need several rows of the panel".into(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
            fold: None,
        });
        let mut msgs = numbered_long(3);
        msgs.push(assistant);
        for palette in [Palette::default(), Palette::mono()] {
            for width in [12usize, 20, 33, 60] {
                let mut feed = MessageFeed::new();
                let summary = "a summary of what was said before, long enough to wrap";
                feed.set_compaction(Some((msgs[1].message_ids[0], summary.into())));
                feed.toggle_thoughts();
                feed.toggle_tools();
                feed.build_lines(&msgs, &palette, width, ru());
                for (b, block) in feed.cache.iter().enumerate() {
                    assert_eq!(block.lines.len(), block.facts.len());
                    for (i, row) in block.lines.iter().enumerate() {
                        assert_eq!(
                            wrap::wrap_line(row, width).len(),
                            1,
                            "width {width}, block {b}, row {i}: {row:?}"
                        );
                    }
                }
            }
        }
    }

    // ---------- prefixes left out of a copy (research §5.4, stage 2b) ----------

    /// Draws `msgs` in a `w`×`h` bordered panel tall enough to hold them, drags
    /// from the panel's first cell to its last, and returns the copy.
    fn copy_all(
        feed: &mut MessageFeed,
        msgs: &[FeedMessage],
        w: u16,
        h: u16,
        palette: &Palette,
    ) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        draw_with(&mut term, feed, msgs, palette);
        assert!(feed.press(1, 1, SelectUnit::Char));
        assert!(feed.drag(w - 2, h - 2));
        feed.release();
        feed.selected_text().unwrap()
    }

    /// A quote is copied without its `> `, its wrapped rows joined back into
    /// the line — in the full mode, where the feed wraps it, and in the
    /// monochrome mode, where the prefix is drawn on every row.
    #[test]
    fn a_quote_is_copied_without_its_prefix_and_as_one_line() {
        let quote = "one two three four five six seven eight nine ten eleven";
        let msgs = vec![msg(FeedRole::Assistant, &format!("> {quote}\n\nafter"), "")];
        for palette in [Palette::default(), Palette::mono()] {
            let mut feed = MessageFeed::new();
            let text = copy_all(&mut feed, &msgs, 30, 20, &palette);
            assert!(text.contains(&format!("\n{quote}\n")), "{text:?}");
            assert!(!text.contains('>'), "{text:?}");
        }
    }

    /// Thoughts, unfolded, are copied without their `│ ` gutter, a wrapped
    /// thought as one line — in both colour modes.
    #[test]
    fn thoughts_are_copied_without_their_gutter() {
        let thought = "a thought that runs well past the width of this narrow panel";
        let msgs = vec![msg(FeedRole::Assistant, "the answer", thought)];
        for palette in [Palette::default(), Palette::mono()] {
            let mut feed = MessageFeed::new();
            feed.toggle_thoughts();
            let text = copy_all(&mut feed, &msgs, 30, 20, &palette);
            assert!(text.contains(&format!("\n{thought}\n")), "{text:?}");
            assert!(!text.contains(GUTTER), "{text:?}");
        }
    }

    /// A tool card is copied without its gutters and indent — the `│ ` of the
    /// arguments, the `└ ` of the result, the icon of the header — its long
    /// lines joined back.
    #[test]
    fn a_tool_card_is_copied_without_its_gutters() {
        let mut m = msg(FeedRole::Assistant, "done", "");
        let result = "a result long enough to wrap over several rows of the card";
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{\"content\":\"x\"}".into(),
            result: result.into(),
            text_offset: 0,
            images: 0,
            call_id: None,
            running: false,
            fold: None,
        });
        let mut feed = feed_with_tools();
        let text = copy_all(&mut feed, &[m], 34, 24, &Palette::default());
        assert!(
            text.contains("\nnote_save"),
            "the header without the icon: {text:?}"
        );
        assert!(text.contains(&format!("\n{result}")), "{text:?}");
        for chrome in ["│ ", "└ ", "⚒"] {
            assert!(!text.contains(chrome), "{chrome:?} left in: {text:?}");
        }
    }

    /// A code block's long line, wrapped by the renderer at a space it then
    /// trimmed to square the background off, is copied as the line it was —
    /// the space put back.
    #[test]
    fn a_wrapped_code_line_is_copied_with_its_spaces() {
        let line = "cargo test --bin mindfork -- feed_selection --nocapture";
        let msgs = vec![msg(FeedRole::Assistant, &format!("```\n{line}\n```"), "")];
        let mut feed = MessageFeed::new();
        let text = copy_all(&mut feed, &msgs, 30, 20, &Palette::default());
        assert!(text.contains(&format!("\n{line}\n")), "{text:?}");
    }

    /// The compaction summary is copied without its gutter.
    #[test]
    fn the_compaction_summary_is_copied_without_its_gutter() {
        let summary = "earlier the two of them discussed a great many things at length";
        let messages = numbered(2);
        let mut feed = MessageFeed::new();
        feed.set_compaction(Some((messages[1].message_ids[0], summary.into())));
        feed.toggle_thoughts();
        let text = copy_all(&mut feed, &messages, 30, 24, &Palette::default());
        assert!(text.contains(&format!("\n{summary}\n")), "{text:?}");
        assert!(!text.contains(GUTTER), "{text:?}");
    }

    /// A prefix is chrome to the highlight too: it is not painted.
    #[test]
    fn a_quotes_prefix_is_not_painted() {
        let msgs = vec![msg(FeedRole::Assistant, "> quoted words", "")];
        let mut feed = MessageFeed::new();
        let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
        draw_with(&mut term, &mut feed, &msgs, &Palette::default());
        let (c, r) = cell_of(&term, "quoted");
        assert!(feed.press(1, r, SelectUnit::Char));
        assert!(feed.drag(c + 3, r));
        draw_with(&mut term, &mut feed, &msgs, &Palette::default());
        let marked = Palette::default().selection(Style::new()).bg;
        let buf = term.backend().buffer();
        assert_ne!(Some(buf[(c - 2, r)].bg), marked, "the `>`");
        assert_eq!(Some(buf[(c, r)].bg), marked, "the text");
        assert_eq!(feed.selected_text().as_deref(), Some("quot"));
    }

    // ---------- one block folded by a click on its pill (block-fold research §3.4–§3.5) ----------

    /// A stored assistant bubble with thoughts and two tool cards, keyed as
    /// `from_message` keys them: every kind of pill but the summary's.
    fn foldable(id: Uuid) -> FeedMessage {
        let mut m = msg(FeedRole::Assistant, "ответ", "мысль раз\nмысль два");
        m.message_ids = vec![id];
        for (call, name) in ["alpha_tool", "beta_tool"].into_iter().enumerate() {
            m.tools.push(FeedToolCall {
                name: name.into(),
                arguments: "{\"q\":\"x\"}".into(),
                result: format!("{name} result"),
                text_offset: m.text.len(),
                images: 0,
                call_id: None,
                running: false,
                fold: Some(FoldKey::Tool { message: id, call }),
            });
        }
        m
    }

    /// The cell where `needle` first appears on the first row holding `row`,
    /// read back from the drawn buffer (docs/lessons.md §5) — a wide glyph's
    /// trailing cell is drawn as a blank, so characters count cells.
    fn cell_in_row(term: &Terminal<TestBackend>, row: &str, needle: &str) -> (u16, u16) {
        let buf = term.backend().buffer();
        for y in 0..buf.area.height {
            let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            if text.contains(row)
                && let Some(at) = text.find(needle)
            {
                let x = text[..at].chars().count() as u16;
                assert_eq!(
                    buf[(x, y)].symbol(),
                    needle.chars().next().unwrap().to_string(),
                    "{text:?}"
                );
                return (x, y);
            }
        }
        panic!("{needle:?} on a row with {row:?} is not on screen");
    }

    fn screen_has(term: &Terminal<TestBackend>, needle: &str) -> bool {
        let buf = term.backend().buffer();
        (0..buf.area.height).any(|y| {
            let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            text.contains(needle)
        })
    }

    /// Each pill takes a click on its own cells — the marker, the label, the
    /// keycap — and folds its block alone; the header beside it, the rail and
    /// the text are ordinary feed. Expanded, a card ends its header on a
    /// `▾ details` pill (F5b) that folds it back. In the full and the
    /// monochrome glyphs alike.
    #[test]
    fn a_click_on_a_pill_folds_that_block_alone() {
        for palette in [Palette::default(), Palette::mono()] {
            let g = palette.glyphs();
            let (shut, open) = (g.collapsed, g.expanded);
            let id = Uuid::new_v4();
            let msgs = vec![msg(FeedRole::User, "вопрос", ""), foldable(id)];
            let mut feed = MessageFeed::new();
            let mut term = Terminal::new(TestBackend::new(70, 30)).unwrap();
            draw_with(&mut term, &mut feed, &msgs, &palette);

            let thoughts = cell_in_row(&term, "мысли", &format!("{shut} мысли"));
            assert_eq!(
                feed.fold_at(thoughts.0, thoughts.1),
                Some(FoldKey::Thoughts(id))
            );
            let keycap = cell_in_row(&term, "мысли", "Ctrl+T");
            assert_eq!(
                feed.fold_at(keycap.0, keycap.1),
                Some(FoldKey::Thoughts(id))
            );
            assert_eq!(feed.fold_at(thoughts.0 - 1, thoughts.1), None, "the rail");
            let pill = cell_in_row(&term, "alpha_tool", &format!("{shut} детали"));
            let alpha = FoldKey::Tool {
                message: id,
                call: 0,
            };
            assert_eq!(feed.fold_at(pill.0, pill.1), Some(alpha));
            let name = cell_in_row(&term, "alpha_tool", "alpha_tool");
            assert_eq!(
                feed.fold_at(name.0, name.1),
                None,
                "the header beside the pill"
            );
            let text = cell_in_row(&term, "ответ", "ответ");
            assert_eq!(feed.fold_at(text.0, text.1), None);

            feed.toggle_block(alpha);
            draw_with(&mut term, &mut feed, &msgs, &palette);
            assert!(screen_has(&term, "alpha_tool result"), "the card opened");
            assert!(!screen_has(&term, "beta_tool result"), "the other did not");
            assert!(!screen_has(&term, "мысль раз"), "nor the thoughts");
            let back = cell_in_row(&term, "alpha_tool", &format!("{open} детали"));
            assert_eq!(feed.fold_at(back.0, back.1), Some(alpha));
            assert!(feed.view().flipped.contains(&alpha));

            feed.toggle_block(FoldKey::Thoughts(id));
            draw_with(&mut term, &mut feed, &msgs, &palette);
            assert!(screen_has(&term, "мысль раз"));
            let header = cell_in_row(&term, "мысли", &format!("{open} мысли"));
            assert_eq!(
                feed.fold_at(header.0, header.1),
                Some(FoldKey::Thoughts(id))
            );
            feed.toggle_block(alpha);
            draw_with(&mut term, &mut feed, &msgs, &palette);
            assert!(!screen_has(&term, "alpha_tool result"), "folded back");
        }
    }

    /// A click rebuilds the one block it folded: the cache is not cleared, and
    /// the other bubble keeps its rows as they were — the same allocation.
    #[test]
    fn a_click_rebuilds_only_its_block() {
        let (one, two) = (Uuid::new_v4(), Uuid::new_v4());
        let msgs = vec![foldable(one), foldable(two)];
        let mut feed = MessageFeed::new();
        let _ = drawn(&mut feed, &msgs, 70, 30);
        let first = feed.cache[0].lines.as_ptr();
        let second = feed.cache[1].lines.as_ptr();
        feed.toggle_block(FoldKey::Tool {
            message: two,
            call: 1,
        });
        let _ = drawn(&mut feed, &msgs, 70, 30);
        assert_eq!(
            feed.cache[0].lines.as_ptr(),
            first,
            "the other block is kept"
        );
        assert_ne!(feed.cache[1].lines.as_ptr(), second, "this one is rebuilt");
        assert!(!feed.cache_reset, "and the cache was not cleared");
    }

    /// The pill stays on the row it was clicked on: a click ends following, so
    /// the rows the expansion adds go below it instead of pushing it up.
    #[test]
    fn the_pill_stays_on_its_row() {
        let id = Uuid::new_v4();
        let mut msgs: Vec<FeedMessage> = (0..12)
            .map(|i| msg(FeedRole::User, &format!("строка {i}"), ""))
            .collect();
        msgs.push(foldable(id));
        let mut feed = MessageFeed::new();
        let mut term = Terminal::new(TestBackend::new(70, 16)).unwrap();
        draw_with(&mut term, &mut feed, &msgs, &Palette::default());
        assert!(
            feed.is_following(),
            "the premise: the feed follows its tail"
        );
        let (_, row) = cell_in_row(&term, "alpha_tool", "▸ детали");
        feed.toggle_block(FoldKey::Tool {
            message: id,
            call: 0,
        });
        assert!(!feed.is_following());
        draw_with(&mut term, &mut feed, &msgs, &Palette::default());
        let (_, after) = cell_in_row(&term, "alpha_tool", "▾ детали");
        assert_eq!(after, row);
    }

    /// A pill that does not fit beside its header goes to a row of its own and
    /// is the target there; a card with nothing to fold has no pill, collapsed
    /// or expanded; the reply being written draws its pills and takes no click.
    #[test]
    fn pills_in_a_narrow_panel_on_an_empty_card_and_while_writing() {
        let id = Uuid::new_v4();
        let mut narrow = foldable(id);
        narrow.tools.truncate(1);
        narrow.tools[0].name = "a_rather_long_tool_name".into();
        let msgs = vec![narrow];
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &msgs, 34, 20);
        let (_, header) = cell_in_row(&term, "a_rather_long", "a_rather_long");
        let pill = cell_in_row(&term, "детали", "▸ детали");
        assert!(
            pill.1 > header,
            "the premise: the pill moved to the next row"
        );
        assert_eq!(
            feed.fold_at(pill.0, pill.1),
            Some(FoldKey::Tool {
                message: id,
                call: 0
            })
        );

        let mut empty = foldable(id);
        empty.thoughts.clear();
        empty.tools.truncate(1);
        empty.tools[0].arguments = "{}".into();
        empty.tools[0].result.clear();
        for tools in [false, true] {
            let mut feed = MessageFeed::new();
            if tools {
                feed.toggle_tools();
            }
            let term = drawn(&mut feed, std::slice::from_ref(&empty), 70, 12);
            assert!(!screen_has(&term, "детали"), "tools expanded: {tools}");
            assert!(feed.fold_hits.is_empty());
        }

        let mut writing = foldable(id);
        writing.message_ids.clear();
        writing.streaming = true;
        for tool in &mut writing.tools {
            tool.fold = None;
        }
        let mut feed = MessageFeed::new();
        let term = drawn(&mut feed, &[writing], 70, 20);
        assert!(screen_has(&term, "▸ мысли") && screen_has(&term, "▸ детали"));
        assert!(
            feed.fold_hits.is_empty(),
            "nothing to click while it is written"
        );
    }

    /// The compaction divider folds the summary alone, by its marker, label
    /// and keycap — not by the rule that fills the rest of its row.
    #[test]
    fn the_summary_folds_alone_by_its_pill() {
        let boundary = Uuid::new_v4();
        let mut later = msg(FeedRole::User, "дальше", "");
        later.message_ids = vec![boundary];
        let msgs = vec![foldable(Uuid::new_v4()), later];
        let mut feed = MessageFeed::new();
        feed.set_compaction(Some((boundary, "краткое резюме".into())));
        let mut term = Terminal::new(TestBackend::new(70, 30)).unwrap();
        draw_with(&mut term, &mut feed, &msgs, &Palette::default());
        let pill = cell_in_row(&term, "ранние", "▸ ранние");
        assert_eq!(
            feed.fold_at(pill.0, pill.1),
            Some(FoldKey::Summary(boundary))
        );
        let rule = cell_in_row(&term, "ранние", "───");
        assert_eq!(feed.fold_at(rule.0 + 1, rule.1), None, "the rule");

        feed.toggle_block(FoldKey::Summary(boundary));
        draw_with(&mut term, &mut feed, &msgs, &Palette::default());
        assert!(screen_has(&term, "краткое резюме"));
        assert!(!screen_has(&term, "мысль раз"), "the thoughts stay folded");
    }

    // ---- Message numbers (docs/research/chat-branches.md §3.5) ----

    /// One block built on its own: its rows, and the facts beside them.
    fn block(item: &FeedMessage, width: usize, number: Option<u32>) -> BuiltBlock {
        build_message_block(
            item,
            &Palette::default(),
            width,
            &FeedView::default(),
            markdown::RenderOpts::default(),
            ru(),
            &CharacterNames::default(),
            false,
            None,
            &[],
            true,
            number,
            None,
        )
    }

    /// A built block as the selection reads it.
    struct Built<'a>(&'a BuiltBlock);

    impl FeedRows for Built<'_> {
        fn block_count(&self) -> usize {
            1
        }
        fn row_count(&self, _block: usize) -> usize {
            self.0.lines.len()
        }
        fn row(&self, _block: usize, row: usize) -> (&Line<'static>, RowFact) {
            (&self.0.lines[row], self.0.facts[row])
        }
    }

    /// What a copy of the whole block takes.
    fn copied(built: &BuiltBlock) -> String {
        let last = built.lines.len() - 1;
        feed_selection::text(
            &Built(built),
            FeedPoint::new(0, 0, 0),
            FeedPoint::new(0, last, u16::MAX),
        )
    }

    /// A number is a message's rank among the bubbles the chat card counts
    /// (`entities::chat::visible_message_count`) — at every prefix of the
    /// history `bubble_count_agrees_with_the_list_counter` walks, so the last
    /// number on screen is the card's count and N in `/copy last N` counts
    /// the same things.
    #[test]
    fn the_numbers_of_a_feed_are_the_ranks_the_chat_card_counts() {
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
            let feed = FeedMessage::from_messages(&messages[..upto]);
            let count = visible_message_count(&messages[..upto]) as u32;
            assert_eq!(
                numbers(&feed).collect::<Vec<_>>(),
                (1..=count).map(Some).collect::<Vec<_>>(),
                "the numbers and the card disagree at prefix {upto}"
            );
        }
    }

    /// What is not a message of the conversation takes no number and no place
    /// in the count: a transcript's persona and the screen's own notes. The
    /// echo and the reply being written have no stored message yet and are
    /// numbered — they are about to be one.
    #[test]
    fn the_screens_notes_and_a_persona_take_no_number() {
        use crate::entities::message::{Message, MessageRole};
        let stored = FeedMessage::from_message(&Message::new(MessageRole::System, "Режиссёр"))
            .expect("a system row is a note");
        let mut echo = msg(FeedRole::User, "вопрос", "");
        echo.message_ids.clear();
        let mut live = msg(FeedRole::Assistant, "", "");
        live.message_ids.clear();
        live.streaming = true;
        let feed = [
            FeedMessage::system("персона"),
            echo,
            FeedMessage::note("служебная"),
            stored,
            live,
        ];
        assert_eq!(
            numbers(&feed).collect::<Vec<_>>(),
            vec![None, Some(1), None, Some(2), Some(3)]
        );
    }

    /// The number stands at the right end of the header's row, and the padding
    /// and the number are the row's tail. With no number the header is the one
    /// drawn before numbers existed.
    #[test]
    fn a_message_number_ends_its_headers_row() {
        let m = msg(FeedRole::Assistant, "ответ", "");
        let bare = block(&m, 40, None);
        let header = rows(&bare.lines)[0].clone();
        assert!(header.ends_with("АССИСТЕНТ"), "{header:?}");
        assert_eq!(bare.facts[0].tail, 0);

        let built = block(&m, 40, Some(10));
        let row = rows(&built.lines)[0].clone();
        assert!(row.starts_with(&header), "{row:?}");
        assert!(row.ends_with(" #10"), "{row:?}");
        assert_eq!(
            wrap::str_width(&row),
            40 - NUMBER_MARGIN,
            "a column short of the panel's edge"
        );
        assert_eq!(
            usize::from(built.facts[0].tail),
            wrap::str_width(&row) - wrap::str_width(&header),
            "everything past the header's text is the tail"
        );
        assert_eq!(
            built.lines.len(),
            bare.lines.len(),
            "the number costs no row"
        );
        assert_eq!(copied(&built), copied(&bare), "and is not copied");
    }

    /// The header's row is shared role-first: the role's name whole, then the
    /// number, then the model's name — whole, cut with its mark, or left out.
    /// A row the role's name fills has no number, and is drawn as before.
    #[test]
    fn a_headers_row_is_shared_role_first() {
        let mut m = msg(FeedRole::Assistant, "ответ", "");
        m.model = Some("gemini-4-hydrogen-preview".into());
        let header = |width| rows(&block(&m, width, Some(10)).lines)[0].clone();

        let wide = header(60);
        assert!(wide.contains("  gemini-4-hydrogen-preview "), "{wide:?}");
        assert!(wide.ends_with(" #10"), "{wide:?}");

        // 30 columns: 28 under the rail, 5 for the number with its column
        // before and its margin after, 11 for the role, 2 for the gap — 10
        // left for the name.
        let cut = header(30);
        assert!(cut.contains("  gemini-4-… "), "{cut:?}");
        assert!(cut.ends_with(" #10"), "{cut:?}");
        assert_eq!(wrap::str_width(&cut), 30 - NUMBER_MARGIN);

        // 20 columns leave the name none: it is left out, not cut to "…".
        let dropped = header(20);
        assert!(
            !dropped.contains("gemini") && !dropped.contains('…'),
            "{dropped:?}"
        );
        assert!(dropped.ends_with(" #10"), "{dropped:?}");

        // 14 columns: the role's name and the number do not both fit, so the
        // number goes and the header is what it was — the name wrapped under it.
        let narrow = block(&m, 14, Some(10));
        let all = rows(&narrow.lines).join("\n");
        assert!(!all.contains("#10"), "{all:?}");
        assert!(all.contains("gemini"), "{all:?}");
        assert!(narrow.facts.iter().all(|f| f.tail == 0));
        assert_eq!(all, rows(&block(&m, 14, None).lines).join("\n"));
    }

    /// A note with a stored message behind it has no header, so its number
    /// ends its own first row. A first row the text fills is wrapped again,
    /// narrower — and a copy still takes the text as it was written, the
    /// whitespace at the wrap included.
    #[test]
    fn a_stored_note_is_numbered_on_its_first_row() {
        let mut short = FeedMessage::note("Режиссёр: закругляйтесь");
        short.message_ids = vec![Uuid::new_v4()];
        let built = block(&short, 40, Some(7));
        let row = rows(&built.lines)[0].clone();
        assert!(row.starts_with("▌ Режиссёр: закругляйтесь "), "{row:?}");
        assert!(row.ends_with(" #7"), "{row:?}");
        assert_eq!(wrap::str_width(&row), 40 - NUMBER_MARGIN);
        assert_eq!(copied(&built), "Режиссёр: закругляйтесь");

        // Thirty-eight columns under the rail, and a first row of thirty-seven:
        // the number needs four of them.
        let text = "раз два три четыре пять шесть семь да восемь девять десять";
        let mut long = FeedMessage::note(text);
        long.message_ids = vec![Uuid::new_v4()];
        let plain = block(&long, 40, None);
        assert!(
            wrap::str_width(rows(&plain.lines)[0].trim_end()) + 4 > 40,
            "the control: unnumbered, the first row leaves the number no room"
        );
        let built = block(&long, 40, Some(7));
        let drawn = rows(&built.lines);
        assert!(drawn[0].ends_with(" #7"), "{drawn:?}");
        assert_eq!(wrap::str_width(&drawn[0]), 40 - NUMBER_MARGIN);
        assert!(drawn[1..].iter().all(|r| !r.contains('#')), "{drawn:?}");
        assert_eq!(
            copied(&built),
            text,
            "the wrap's whitespace is still the text's"
        );

        // The screen's own note: no number, and no row of it changes.
        let service = FeedMessage::note(text);
        assert_eq!(rows(&block(&service, 40, None).lines), rows(&plain.lines));
    }

    /// A query of a number's digits lights the message's text and not its
    /// number, which is no part of the message — and is not counted as a
    /// match. The control names no tail and finds the number too.
    #[test]
    fn a_number_is_not_a_search_match() {
        let palette = Palette::default();
        let mut note = FeedMessage::note("порт 1234 открыт");
        note.message_ids = vec![Uuid::new_v4()];
        let built = block(&note, 40, Some(123));
        assert!(rows(&built.lines)[0].ends_with(" #123"));

        let mut lines = built.lines.clone();
        let mut found = Vec::new();
        highlight_block_tail(&mut lines, 0, &built.facts, "123", &palette, &mut found);
        assert_eq!(found, vec![0], "the text's one match");
        let number = lines[0].spans.last().expect("the number's span");
        assert_eq!(number.content, "#123");
        assert_eq!(number.style, palette.muted_style(), "not lit");

        let untold = vec![RowFact::default(); built.facts.len()];
        let mut lines = built.lines.clone();
        let mut found = Vec::new();
        highlight_block_tail(&mut lines, 0, &untold, "123", &palette, &mut found);
        assert_eq!(found, vec![0, 0], "the control: the number is found too");
    }

    /// A block's number is not in its fingerprint — it is a property of its
    /// place — so the cache checks it beside: a block whose message did not
    /// change is rebuilt when the count above it does.
    #[test]
    fn a_block_is_rebuilt_when_its_number_changes() {
        let mut feed = MessageFeed::new();
        let user = msg(FeedRole::User, "вопрос", "");
        let service = FeedMessage::note("заметка");
        let mut stored = FeedMessage::note("заметка");
        stored.message_ids = vec![Uuid::new_v4()];
        let header = |rows: &[String]| {
            rows.iter()
                .find(|r| r.contains("ВЫ"))
                .cloned()
                .expect("the person's header")
        };

        let first =
            rows(&feed.build_lines(&[service, user.clone()], &Palette::default(), 40, ru()));
        assert!(header(&first).ends_with(" #1"), "{first:?}");
        assert!(!first[0].contains('#'), "the screen's note has no number");

        let second = rows(&feed.build_lines(&[stored, user], &Palette::default(), 40, ru()));
        assert!(second[0].ends_with(" #1"), "{second:?}");
        assert!(header(&second).ends_with(" #2"), "{second:?}");
    }

    /// Through the whole render: every message of the feed carries its number
    /// in one column, at the panel's right edge.
    #[test]
    fn the_numbers_stand_in_one_column_at_the_feeds_right_edge() {
        let mut feed = MessageFeed::new();
        let screen = visible(&mut feed, &numbered(3), 40, 14);
        let columns: Vec<usize> = (1..=3)
            .map(|n| {
                let tag = format!("#{n}");
                let row = screen
                    .iter()
                    .find(|r| r.contains("ВЫ") && r.contains(&tag))
                    .unwrap_or_else(|| panic!("no header with {tag}: {screen:?}"));
                wrap::str_width(&row[..row.find(&tag).unwrap()])
            })
            .collect();
        assert_eq!(columns, vec![columns[0]; 3], "{screen:?}");
        // Nothing but the panel's own edge stands to the right of a number.
        let row = screen.iter().find(|r| r.contains("#1")).unwrap();
        let after = row[row.find("#1").unwrap() + 2..].trim();
        assert!(after.chars().count() <= 1, "{row:?}");
    }

    // ---- The variants mark and the tag's click targets (research §3.7) ----

    /// A bubble whose place has other variants parked carries its figure
    /// before its number, and the tag is chrome whole: one tail, nothing of it
    /// copied.
    #[test]
    fn a_bubble_with_variants_carries_its_mark_before_the_number() {
        let m = msg(FeedRole::User, "вопрос", "");
        let plain = block(&m, 40, Some(11));
        let built = build_message_block(
            &m,
            &Palette::default(),
            40,
            &FeedView::default(),
            markdown::RenderOpts::default(),
            ru(),
            &CharacterNames::default(),
            false,
            None,
            &[],
            true,
            Some(11),
            Some((2, 3)),
        );
        let row = rows(&built.lines)[0].clone();
        assert!(row.ends_with(" ‹2/3› #11"), "{row:?}");
        assert_eq!(wrap::str_width(&row), 40 - NUMBER_MARGIN);
        assert_eq!(copied(&built), copied(&plain));
        assert_eq!(built.lines.len(), plain.lines.len());

        // The targets: the number, then the two arrows, on the header's row.
        let end = (40 - NUMBER_MARGIN) as u16;
        let message = m.message_ids[0];
        assert_eq!(
            built.taps,
            vec![
                TapHit {
                    row: 0,
                    start: end - 3,
                    end,
                    tap: Tap::Number(11)
                },
                TapHit {
                    row: 0,
                    start: end - 9,
                    end: end - 8,
                    tap: Tap::Variant { message, step: -1 }
                },
                TapHit {
                    row: 0,
                    start: end - 5,
                    end: end - 4,
                    tap: Tap::Variant { message, step: 1 }
                },
            ]
        );
        // With no mark the number is the tag's one target.
        assert_eq!(plain.taps.len(), 1);
        assert_eq!(plain.taps[0].tap, Tap::Number(11));
        // And a row with no room for the tag has none.
        assert!(block(&m, 6, Some(11)).taps.is_empty());
    }

    /// Through the whole render: a click on a cell of the tag is answered
    /// against the frame that was drawn; a mark set after the first frame
    /// redraws the block it is on, and that one alone.
    #[test]
    fn a_tag_is_found_under_the_pointer_as_it_was_drawn() {
        let messages = numbered(3);
        let marked = messages[1].message_ids[0];
        let mut feed = MessageFeed::new();
        let before = visible(&mut feed, &messages, 40, 14);
        assert!(!before.iter().any(|r| r.contains('‹')), "{before:?}");

        feed.set_variants(&[VariantMark {
            message: marked,
            variant: 1,
            of: 2,
        }]);
        let screen = visible(&mut feed, &messages, 40, 14);
        let (y, row) = screen
            .iter()
            .enumerate()
            .find(|(_, r)| r.contains("‹1/2› #2"))
            .unwrap_or_else(|| panic!("the mark is not drawn: {screen:?}"));
        let column = |tag: &str| wrap::str_width(&row[..row.find(tag).unwrap()]) as u16;
        let y = y as u16;
        assert_eq!(
            feed.tap_at(column("‹"), y),
            Some(Tap::Variant {
                message: marked,
                step: -1
            })
        );
        assert_eq!(
            feed.tap_at(column("›"), y),
            Some(Tap::Variant {
                message: marked,
                step: 1
            })
        );
        assert_eq!(feed.tap_at(column("#2"), y), Some(Tap::Number(2)));
        assert_eq!(feed.tap_at(column("#2") + 1, y), Some(Tap::Number(2)));
        // Between the mark and the number, on the figure, and on the header's
        // text: nothing.
        assert_eq!(feed.tap_at(column("#2") - 1, y), None);
        assert_eq!(feed.tap_at(column("1/2"), y), None);
        assert_eq!(feed.tap_at(column("ВЫ"), y), None);
        // The other bubbles keep their numbers, and gained no mark.
        assert_eq!(screen.iter().filter(|r| r.contains('‹')).count(), 1);
    }
}
