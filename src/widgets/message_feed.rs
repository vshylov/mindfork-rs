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

/// Role of a feed item (a UI projection; system messages aren't shown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedRole {
    User,
    Assistant,
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
}

impl FeedMessage {
    /// A service note for the feed.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            role: FeedRole::Note,
            text: text.into(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: false,
            message_ids: Vec::new(),
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
            MessageRole::Tool | MessageRole::System => return None,
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
            })
            .collect();
        Some(Self {
            role,
            text: msg.text.clone(),
            thoughts: msg.thoughts.clone().unwrap_or_default(),
            tools,
            streaming: false,
            message_ids: vec![msg.id],
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
            let Some(mut fm) = Self::from_message(msg) else {
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
                continue;
            }
            out.push(fm);
        }
        out
    }
}

/// Feed view state (scroll, thoughts display).
pub struct MessageFeed {
    /// Scroll offset (in lines from the start).
    scroll: usize,
    /// Follow the tail (auto-scroll to bottom on new content).
    follow: bool,
    /// Show the expanded "thoughts" block.
    show_thoughts: bool,
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
    /// The active chat profile's custom role names (threaded via
    /// [`MessageFeed::set_role_names`]). An unset field falls back to the localized
    /// header (`YOU`/`ASSISTANT`). See spec §11.3.
    role_names: CharacterNames,
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
    /// light up the wrong message. Scoped to that one message deliberately —
    /// highlighting every occurrence in the chat is noise.
    highlight: Option<String>,
    /// [`MessageFeed::build_lines`] dropped the whole cache on this frame (the
    /// key changed), i.e. every row offset it had produced before is stale.
    /// Consumed by `render` to re-derive the anchored row. See [`CacheKey`].
    cache_reset: bool,
}

/// Feed cache validity key. Any of these fields affects the layout of every
/// block, so changing it clears the cache. `Palette` — `Copy + Eq` (also the key in the syntect-theme cache).
#[derive(PartialEq)]
struct CacheKey {
    width: usize,
    palette: Palette,
    show_thoughts: bool,
    table_row_separators: bool,
    render_mermaid: bool,
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
    /// The highlighted query: its matches are recolored inside the marked
    /// block's cached lines, so a new jump (or a chat switch clearing it) has to
    /// rebuild them. Cheap — it only changes when a jump happens, which moves
    /// `marker` and resets the cache anyway.
    highlight: Option<String>,
}

/// One message's cached contribution to the feed (already width-wrapped lines
/// with a rail + a trailing separator) together with the source [`FeedMessage`]'s fingerprint.
struct CachedBlock {
    fingerprint: u64,
    lines: Vec<Line<'static>>,
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
            show_thoughts: false,
            scrolled: false,
            // Mirrors the config defaults (`InterfaceSettings::default`): before the
            // first settings snapshot arrives, the feed renders as the default config.
            table_row_separators: false,
            render_mermaid: true,
            role_names: CharacterNames::default(),
            cache: Vec::new(),
            cache_key: None,
            pending_focus: None,
            anchor: None,
            marker: None,
            highlight: None,
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

    /// Takes (and resets) the "scrolled by user" flag. The `app/runtime` loop,
    /// when `true`, does `terminal.clear()` before rendering — erases the artifacts
    /// of "drifted" VS16 emoji (see the [`MessageFeed::scrolled`] field).
    pub fn take_scrolled(&mut self) -> bool {
        std::mem::take(&mut self.scrolled)
    }

    /// Toggles showing "thoughts" blocks.
    pub fn toggle_thoughts(&mut self) {
        self.show_thoughts = !self.show_thoughts;
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
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(glyphs.border)
            .border_style(palette.border_style(false))
            .title(Line::from(vec![
                Span::styled(format!(" {} ", glyphs.title_marker), palette.muted_style()),
                Span::styled(format!("{title} "), Style::new().fg(palette.text)),
            ]));
        if !meta.is_empty() {
            block = block.title(
                Line::from(Span::styled(format!(" {meta} "), palette.muted_style()))
                    .right_aligned(),
            );
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
        let mut lines: Vec<Line> = Vec::with_capacity(unwrapped.len());
        let mut target_row: Option<usize> = None;
        for (i, line) in unwrapped.iter().enumerate() {
            if start == Some(i) {
                target_row = Some(lines.len());
            }
            lines.extend(wrap::wrap_line(line, view_w));
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
        // Reset the cache on a change to width/palette/thoughts display/language (affects blocks).
        let key = CacheKey {
            width,
            palette: *palette,
            show_thoughts: self.show_thoughts,
            table_row_separators: self.table_row_separators,
            render_mermaid: self.render_mermaid,
            lang: loc.lang(),
            role_names: self.role_names.clone(),
            marker: self.marker,
            highlight: self.highlight.clone(),
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
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (idx, item) in messages.iter().enumerate() {
            let fp = message_fingerprint(item);
            let hit = self.cache.get(idx).is_some_and(|c| c.fingerprint == fp);
            if !hit {
                // A streaming/changed message — recompute only its block.
                let marked = self.marker == Some(idx);
                let block = build_message_block(
                    item,
                    palette,
                    width,
                    self.show_thoughts,
                    opts,
                    loc,
                    &self.role_names,
                    marked,
                    // The highlight is scoped to the marked message: the user
                    // asked "where is my word in *this* message", and lighting
                    // up the whole chat would be noise.
                    if marked {
                        self.highlight.as_deref()
                    } else {
                        None
                    },
                );
                let cb = CachedBlock {
                    fingerprint: fp,
                    lines: block,
                };
                if idx < self.cache.len() {
                    self.cache[idx] = cb;
                } else {
                    self.cache.push(cb);
                }
            }
            lines.extend(self.cache[idx].lines.iter().cloned());
        }
        lines
    }
}

/// Builds one message's contribution to the feed: width-wrapped lines with a colored
/// role rail + a trailing separator (if the body doesn't already end on a blank line).
/// A pure function of (`item`, `palette`, `width`, `show_thoughts`, `opts`, `names`) —
/// the basis of the cache. `opts` — base markdown-render flags (tables/mermaid from
/// settings; `soft_break_as_newline` is added on by `push_body` for the user).
/// `names` — the profile's custom role names (empty field → the localized header).
/// `marked` — this is the jump target ([`MessageFeed::focus_message`]): the rail
/// is drawn in the accent color to mark the whole message. `highlight` — the
/// query recolored inside it (fork S3(b), see [`highlight_line`]); `None` for
/// every message but the marked one.
#[allow(clippy::too_many_arguments)]
fn build_message_block(
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
    show_thoughts: bool,
    opts: markdown::RenderOpts,
    loc: &'static Locale,
    names: &CharacterNames,
    marked: bool,
    highlight: Option<&str>,
) -> Vec<Line<'static>> {
    // Content width under the rail (rail = 2 columns).
    let inner = width.saturating_sub(RAIL.chars().count()).max(1);
    let rail = if marked {
        palette.accent
    } else {
        match item.role {
            FeedRole::User => palette.user,
            FeedRole::Assistant => palette.assistant,
            FeedRole::Note => palette.muted,
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
            ));
            content_from = body.len();
            push_thoughts(&mut body, &item.thoughts, show_thoughts, palette, loc);
            push_assistant_body(&mut body, item, palette, inner, opts);
        }
        FeedRole::Note => push_body(&mut body, item, palette, inner, opts),
    }
    // Recolor the searched query inside the jumped-to message, **before** the
    // wrap below: `wrap::wrap_line` carries per-character styles through, so a
    // highlight applied here survives being wrapped (and being wrapped again in
    // `render`), while applying it afterwards would have to look past the rail.
    if let Some(query) = highlight {
        for line in &mut body[content_from..] {
            highlight_line(line, query, palette.accent);
        }
    }
    // If the body already ends on a blank line (a railed gap after a tool card),
    // don't add the railless inter-message separator — otherwise a double gap.
    let body_ends_blank = body
        .last()
        .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(false);
    // Wrap to the content width and attach a rail to every row.
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in body {
        for wrapped in wrap::wrap_line(&line, inner) {
            out.push(prepend_rail(wrapped, rail));
        }
    }
    // Separator between messages — without a rail.
    if !body_ends_blank {
        out.push(Line::from(""));
    }
    out
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
fn highlight_line(line: &mut Line<'static>, query: &str, color: Color) {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges = crate::features::chat_search::match_ranges(&text, query);
    if ranges.is_empty() {
        return;
    }
    let spans = std::mem::take(&mut line.spans);
    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len() + ranges.len() * 2);
    // Byte offset of the current span within `text`.
    let mut at = 0usize;
    for span in spans {
        let (start, end) = (at, at + span.content.len());
        at = end;
        // Cut points inside this span: every match boundary strictly within it.
        // Taken per span because a match may **straddle** two of them — bold in
        // the middle of a word, a link, an inline code span — and then both
        // halves have to be recolored. `ranges` is sorted and non-overlapping,
        // so the cuts come out ascending.
        let mut cuts: Vec<usize> = vec![start];
        for r in &ranges {
            for b in [r.start, r.end] {
                if b > start && b < end {
                    cuts.push(b);
                }
            }
        }
        cuts.push(end);
        cuts.dedup();
        for w in cuts.windows(2) {
            let (s, e) = (w[0], w[1]);
            let inside = ranges.iter().any(|r| r.start <= s && e <= r.end);
            let style = if inside {
                span.style.fg(color)
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
    };
    role_tag.hash(&mut h);
    item.text.hash(&mut h);
    item.thoughts.hash(&mut h);
    item.streaming.hash(&mut h);
    item.tools.len().hash(&mut h);
    for tc in &item.tools {
        tc.name.hash(&mut h);
        tc.arguments.hash(&mut h);
        tc.result.hash(&mut h);
        tc.text_offset.hash(&mut h);
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

/// A role-header line: icon + name in caps, colored with the role's "soft" variant.
fn role_header(text: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    ))
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
        format!("{} мысли", glyphs.expanded),
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
    opts: markdown::RenderOpts,
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
        push_tool(lines, tool, palette, width, opts);
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
fn push_tool(
    lines: &mut Vec<Line<'static>>,
    tool: &FeedToolCall,
    palette: &Palette,
    width: usize,
    opts: markdown::RenderOpts,
) {
    let head_style = Style::default()
        .fg(palette.tool_soft)
        .add_modifier(Modifier::BOLD);
    let p = present::present(&tool.name, &tool.arguments, &tool.result);
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
    for block in &p.args {
        push_block(lines, block, palette, width, false, opts);
    }
    for block in &p.result {
        push_block(lines, block, palette, width, true, opts);
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
        ToolBlock::Console(c) => push_console(lines, c, palette, width),
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
        push_wrapped(
            lines,
            "└ ",
            "  ",
            &format!("код возврата: {code}"),
            width,
            warn,
        );
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

    fn msg(role: FeedRole, text: &str, thoughts: &str) -> FeedMessage {
        FeedMessage {
            role,
            text: text.to_string(),
            thoughts: thoughts.to_string(),
            tools: Vec::new(),
            streaming: false,
            message_ids: vec![Uuid::new_v4()],
        }
    }

    #[test]
    fn tool_blocks_render() {
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{\"content\":\"x\"}".into(),
            result: "Заметка сохранена".into(),
            text_offset: m.text.len(),
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 80, ru());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("⚒") && joined.contains("note_save"));
        assert!(joined.contains("Заметка сохранена"));
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
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"print(42)"}"#.into(),
            result: "stdout:\n42".into(),
            text_offset: m.text.len(),
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
        let mut feed = MessageFeed::new();
        let palette = Palette::default();
        let mut m = msg(FeedRole::Assistant, "", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"raise SystemExit(1)"}"#.into(),
            result: "stderr:\nTraceback here\n\nкод возврата: 1".into(),
            text_offset: 0,
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
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "Считаю.", "");
        m.tools.push(FeedToolCall {
            name: "python_exec".into(),
            arguments: r#"{"code":"print(1)"}"#.into(),
            result: "stdout:\n1".into(),
            text_offset: "Считаю.".len(),
        });
        let next = msg(FeedRole::User, "дальше", "");
        let rows = row_texts(&feed.build_lines(&[m, next], &Palette::default(), 60, ru()));
        let i_result = rows.iter().position(|r| r.contains('1')).unwrap();
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
        let mut feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "ок", "");
        let long = "слово ".repeat(40); // ~240 characters — definitely wider than the narrow feed
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: String::new(),
            result: long.trim_end().into(),
            text_offset: 0,
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
            id: "c1".into(),
            name: "send_followup_message".into(),
            arguments: serde_json::json!({}),
            result: Some("ок".into()),
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
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({}),
            result: Some("ok".into()),
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

    #[test]
    fn from_message_skips_system() {
        let sys = Message::new(MessageRole::System, "s");
        assert!(FeedMessage::from_message(&sys).is_none());
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
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({}),
            result: Some("ok".into()),
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
        let _ = feed.build_lines(&messages, &palette, 60, ru());

        assert_eq!(
            accented(&feed.cache[1].lines, &palette),
            vec!["маркер"],
            "the marked message must show where the query matched"
        );
        assert!(
            accented(&feed.cache[0].lines, &palette).is_empty(),
            "another message with the same word must stay untouched"
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
            let _ = feed.build_lines(&messages, &palette, 60, ru());
            assert!(
                accented(&feed.cache[0].lines, &palette).is_empty(),
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
        let _ = feed.build_lines(&messages, &palette, 60, ru());

        let hits = accented(&feed.cache[0].lines, &palette);
        assert_eq!(
            hits.concat(),
            "маркер",
            "both halves of a straddling match must be highlighted: {hits:?}"
        );
        let bold = feed.cache[0]
            .lines
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
        let _ = feed.build_lines(&messages, &palette, 40, ru());

        let lines = &feed.cache[0].lines;
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
        let _ = feed.build_lines(&messages, &palette, 60, ru());

        let header = &feed.cache[0].lines[0];
        assert!(
            header.spans.iter().any(|s| s.content.contains("АССИСТЕНТ")),
            "precondition: the header spells the role out: {header:?}"
        );
        assert_eq!(
            accented(&feed.cache[0].lines, &palette),
            vec!["ассистент"],
            "only the body matches, never the header"
        );
    }

    /// The highlight is baked into the cached block, so changing the query has
    /// to reset the cache — otherwise a second jump would show the first jump's
    /// highlight.
    #[test]
    fn changing_the_query_invalidates_the_block_cache() {
        let palette = Palette::default();
        let messages = vec![msg(FeedRole::User, "маркер и метка рядом", "")];
        let mut feed = MessageFeed::new();
        feed.focus_message(&messages, messages[0].message_ids[0], Some("маркер"));
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        feed.cache_reset = false;
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        assert!(!feed.cache_reset, "precondition: the cache is warm");

        feed.focus_message(&messages, messages[0].message_ids[0], Some("метка"));
        let _ = feed.build_lines(&messages, &palette, 60, ru());
        assert!(feed.cache_reset, "a new query changes the rendered block");
        assert_eq!(accented(&feed.cache[0].lines, &palette), vec!["метка"]);
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

    /// A warm cache gives line-for-line identical output to a fresh render — across
    /// scenarios, widths, palettes, and thoughts-display state.
    #[test]
    fn cache_matches_fresh_render() {
        let tool_msg = {
            let mut m = msg(FeedRole::Assistant, "готово", "");
            m.tools.push(FeedToolCall {
                name: "note_save".into(),
                arguments: "{}".into(),
                result: "ок".into(),
                text_offset: m.text.len(),
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
            for width in [40usize, 80] {
                for palette in [Palette::default(), Palette::default().with_compat(true)] {
                    for show in [false, true] {
                        let mut warm = MessageFeed::new();
                        if show {
                            warm.toggle_thoughts();
                        }
                        // warm the cache with repeated calls
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let _ = warm.build_lines(messages, &palette, width, ru());
                        let warm_lines = warm.build_lines(messages, &palette, width, ru());

                        let mut fresh = MessageFeed::new();
                        if show {
                            fresh.toggle_thoughts();
                        }
                        let fresh_lines = fresh.build_lines(messages, &palette, width, ru());

                        let w: Vec<_> = warm_lines.iter().map(line_sig).collect();
                        let f: Vec<_> = fresh_lines.iter().map(line_sig).collect();
                        assert_eq!(w, f, "cache diverged: width={width} show={show}");
                    }
                }
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
}
