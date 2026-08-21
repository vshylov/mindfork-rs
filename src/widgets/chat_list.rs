//! The chat-list overlay: a search filter, two sort orders, in-place rename,
//! create/clone/delete. Opens/closes via `Esc`. See spec §11.2.
//!
//! The widget is self-contained: it holds the list snapshot and input state, and
//! responds to key presses with [`ChatListAction`] (executed by `app` — the sole writer).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::features::chat_search_sort::{SortMode, filter_and_sort};
use crate::features::rename_chat::sanitize_title;
use crate::features::spellcheck::SpellChecker;
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap;
use crate::widgets::input_box::{InputBox, RenderOpts};

/// The paged-selection-move step for `PageUp`/`PageDown`. Fixed,
/// since the actual list height is only known at render time.
const PAGE_STEP: usize = 10;

/// An action the overlay asks the upper layer to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatListAction {
    /// Nothing (the press was handled inside the overlay).
    None,
    /// Close the overlay.
    Close,
    /// Quit the app (`Ctrl+Q`/`F10`).
    Quit,
    /// Make a chat active (and close the overlay).
    Switch(Uuid),
    /// Create a new chat.
    New,
    /// Clone a chat.
    Clone(Uuid),
    /// Copy the whole chat conversation to the clipboard.
    Copy(Uuid),
    /// Soft-delete a chat.
    Delete(Uuid),
    /// Rename a chat.
    Rename { id: Uuid, title: String },
    /// Auto-title: the model reads the conversation and comes up with a title.
    AutoRename(Uuid),
    /// Run a full-text search over chat **content** with this raw query (content
    /// mode, `Ctrl+F`). The widget stays dumb: it never builds an FTS5 query —
    /// that rule lives in one place, the orchestrator (see
    /// docs/research/chat-content-search.md §7a). Results come back via
    /// [`ChatListState::set_search_results`].
    SearchContent(String),
    /// Open the message-level search screen for the current query (content
    /// mode, `Ctrl+G`). Like [`Self::SearchContent`] the widget stays dumb —
    /// the query is raw. See docs/history/chat-search-stage2.md (stage 2b).
    SearchMessages { query: String, sort: SortMode },
    /// Open a chat **at its first message matching the query** (`Enter` in
    /// content mode). Which message that is can only be answered by the
    /// orchestrator, which owns both the index and the chat — the widget just
    /// says which chat and what was searched for.
    OpenFirstMatch { chat: Uuid, query: String },
}

/// What the search line searches. Title mode is the historical behaviour (a
/// substring of the title, filtered locally); content mode asks the
/// orchestrator for the chats whose **messages** match. Toggled with `Ctrl+F`.
/// See docs/research/chat-content-search.md §7 (fork F4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchScope {
    /// Substring of the chat title (the previous, and default, behaviour).
    #[default]
    Title,
    /// Full-text search over message text.
    Content,
}

impl SearchScope {
    fn toggled(self) -> Self {
        match self {
            SearchScope::Title => SearchScope::Content,
            SearchScope::Content => SearchScope::Title,
        }
    }
}

/// The overlay's input mode.
enum Mode {
    /// Typing into the search line.
    Search,
    /// Renaming the selected chat in place. Text and cursor are driven by a
    /// single-line [`InputBox`] (`set_single_line`) — this gives spellcheck,
    /// word-wise navigation/deletion (`Ctrl+←/→`, `Ctrl+Backspace/Delete`),
    /// `Ctrl+Home/End`, clear/restore (`Ctrl+K`), and clipboard paste "for free". `InputBox`
    /// is large — boxed (clippy::large_enum_variant). `spell_dirty` marks
    /// that the error highlighting needs recomputing (see [`Self::recheck_rename_spelling`]).
    Rename {
        id: Uuid,
        input: Box<InputBox>,
        spell_dirty: bool,
    },
}

/// State of the chat-list overlay.
pub struct ChatListState {
    /// A snapshot of all visible chats (updated from `AppEvent::ChatList`).
    all: Vec<ChatSummary>,
    query: String,
    sort: SortMode,
    /// What the query searches: the title (locally) or message content (via the
    /// orchestrator). Toggled with `Ctrl+F`.
    scope: SearchScope,
    /// The chats the last content search matched — `None` means "no result yet,
    /// show everything" (also what an unsearchable query yields, so content mode
    /// with a too-short query behaves exactly like an empty one). Deliberately
    /// kept across keystrokes: replacing it only when a new result arrives is
    /// what stops the full list flashing between them.
    results: Option<Vec<Uuid>>,
    /// The query those `results` answer. Unused by stage 1's filter (a stale
    /// result is still applied — showing the previous answer beats showing
    /// everything), but stage 2 needs it to highlight matches.
    results_query: String,
    /// Selection index within the currently filtered list.
    selected: usize,
    mode: Mode,
    /// The current operation error (auto-title/delete/clone) for the dedicated area.
    /// Reset on the next key press.
    error: Option<String>,
    /// Operation confirmation (e.g. "copied") for the same area, but as
    /// success. Reset on the next key press. Mutually exclusive with `error`.
    notice: Option<String>,
}

impl ChatListState {
    /// Opens the overlay with a list snapshot; selection — on the active chat.
    pub fn new(chats: Vec<ChatSummary>, active: Option<Uuid>) -> Self {
        let mut state = Self {
            all: chats,
            query: String::new(),
            sort: SortMode::default(),
            scope: SearchScope::default(),
            results: None,
            results_query: String::new(),
            selected: 0,
            mode: Mode::Search,
            error: None,
            notice: None,
        };
        if let Some(active) = active {
            let visible = state.visible();
            if let Some(idx) = visible.iter().position(|c| c.id == active) {
                state.selected = idx;
            }
        }
        state
    }

    /// Updates the list snapshot (after changes to the chat set), keeping selection
    /// on the same chat where possible. If the selected chat disappeared (e.g. deleted),
    /// selection stays at the **same position** (the next chat in the list, or a
    /// new last one when the last was deleted), rather than jumping to the first item —
    /// as is conventional for lists.
    pub fn set_chats(&mut self, chats: Vec<ChatSummary>) {
        let current = self.selected_id();
        let prev_index = self.selected;
        self.all = chats;
        let visible = self.visible();
        self.selected = current
            .and_then(|id| visible.iter().position(|c| c.id == id))
            .unwrap_or(prev_index);
        self.clamp_selection();
    }

    /// The current filtered/sorted list.
    ///
    /// In content mode the title substring filter is deliberately **not**
    /// applied: the query is answered by the index, and re-applying it to the
    /// title would hide the very chats the search just found. The user's sort
    /// still orders the result — stage 1 *filters* rather than ranks, because
    /// trigram's `bm25` is weak (docs/research/chat-content-search.md §7a).
    fn visible(&self) -> Vec<ChatSummary> {
        match self.scope {
            SearchScope::Title => filter_and_sort(&self.all, &self.query, self.sort),
            SearchScope::Content => {
                let mut out = filter_and_sort(&self.all, "", self.sort);
                if let Some(ids) = &self.results {
                    let matched: std::collections::HashSet<Uuid> = ids.iter().copied().collect();
                    out.retain(|c| matched.contains(&c.id));
                }
                out
            }
        }
    }

    /// Applies a content-search result (`AppEvent::ChatSearchResults`).
    /// `chat_ids: None` means "not a searchable query" — show everything, like
    /// an empty query. A result for an older query is still applied: the
    /// previous answer is a better thing to show than the whole list.
    pub fn set_search_results(&mut self, query: String, chat_ids: Option<Vec<Uuid>>) {
        self.results_query = query;
        self.results = chat_ids;
        self.clamp_selection();
    }

    /// Reopens the list still searching message content for `query` — used
    /// when the message-level results screen closes, so `Esc` returns to the
    /// search the user was doing rather than to an empty list. The results
    /// themselves are refetched by the caller (the same round-trip typing a
    /// query does).
    pub fn restore_content_query(&mut self, query: String) {
        self.query = query;
        self.scope = SearchScope::Content;
        self.selected = 0;
    }

    /// Emits a content search for the current query — when the query changed in
    /// content mode, or on switching into it.
    fn search_action(&self) -> ChatListAction {
        ChatListAction::SearchContent(self.query.clone())
    }

    /// What an edit to the query means: nothing in title mode (the filter is
    /// local), a fresh content search otherwise.
    fn query_changed(&self) -> ChatListAction {
        match self.scope {
            SearchScope::Title => ChatListAction::None,
            SearchScope::Content => self.search_action(),
        }
    }

    /// The selected chat's id (if the list isn't empty).
    pub fn selected_id(&self) -> Option<Uuid> {
        self.visible().get(self.selected).map(|c| c.id)
    }

    fn clamp_selection(&mut self) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// Sets the error text for the overlay's dedicated area. Reset
    /// on the next key press, so it doesn't stay hanging around forever.
    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
        self.notice = None;
    }

    /// Sets an operation confirmation (success) for the same area. Reset
    /// on the next key press.
    pub fn set_notice(&mut self, message: String) {
        self.notice = Some(message);
        self.error = None;
    }

    /// Handles a key press, returning the action to perform.
    pub fn on_key(&mut self, key: KeyEvent) -> ChatListAction {
        if key.kind != KeyEventKind::Press {
            return ChatListAction::None;
        }
        // Any key press clears a previously shown error/confirmation (they don't linger).
        self.error = None;
        self.notice = None;
        match &mut self.mode {
            Mode::Rename { .. } => self.on_key_rename(key),
            Mode::Search => self.on_key_search(key),
        }
    }

    fn on_key_search(&mut self, key: KeyEvent) -> ChatListAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl shortcuts are matched by "physical" Latin key — work under
        // any layout (see shared::keys). Any other Ctrl+character is swallowed,
        // so it doesn't end up in the search line.
        if ctrl && let Some(physical) = keys::hotkey_char(&key) {
            return self.on_ctrl_search(physical);
        }
        match key.code {
            KeyCode::F(10) => ChatListAction::Quit, // a second way to quit
            KeyCode::Esc => ChatListAction::Close,
            KeyCode::Enter => self.open_selected(),
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ChatListAction::None
            }
            KeyCode::Down => {
                self.select_down(1);
                ChatListAction::None
            }
            // Paged selection movement: step by a "page" (fixed,
            // since the list height is only known at render time).
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(PAGE_STEP);
                ChatListAction::None
            }
            KeyCode::PageDown => {
                self.select_down(PAGE_STEP);
                ChatListAction::None
            }
            // Jump to the first/last chat.
            KeyCode::Home => {
                self.selected = 0;
                ChatListAction::None
            }
            KeyCode::End => {
                self.selected = self.visible().len().saturating_sub(1);
                ChatListAction::None
            }
            KeyCode::Tab => {
                self.sort = self.sort.toggled();
                self.clamp_selection();
                ChatListAction::None
            }
            KeyCode::F(2) => {
                self.start_rename();
                ChatListAction::None
            }
            // Copy the whole conversation of the selected chat to the clipboard.
            KeyCode::F(5) => match self.selected_id() {
                Some(id) => ChatListAction::Copy(id),
                None => ChatListAction::None,
            },
            KeyCode::Delete => match self.selected_id() {
                Some(id) => ChatListAction::Delete(id),
                None => ChatListAction::None,
            },
            // Editing the query: in title mode the filter is local (no action),
            // in content mode every change asks the index for a fresh answer.
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
                self.query_changed()
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.selected = 0;
                self.query_changed()
            }
            _ => ChatListAction::None,
        }
    }

    /// A Ctrl shortcut in search mode, matched by "physical" Latin key
    /// (see [`Self::on_key_search`]).
    fn on_ctrl_search(&mut self, physical: char) -> ChatListAction {
        match physical {
            // Quitting works from the chat list too; moved to Ctrl+Q/F10 (Ctrl+C
            // is freed up). See docs/history/input-selection-undo-mouse.md §B.
            'q' => ChatListAction::Quit,
            'n' => ChatListAction::New,
            'd' => match self.selected_id() {
                Some(id) => ChatListAction::Clone(id),
                None => ChatListAction::None,
            },
            // Auto-title the selected chat, done by the model.
            'r' => match self.selected_id() {
                Some(id) => ChatListAction::AutoRename(id),
                None => ChatListAction::None,
            },
            // Toggle title ↔ content search. Switching *into* content mode
            // asks for results right away, so the mode takes effect on the
            // text already typed; switching back needs no round-trip (the
            // title filter is local).
            'f' => {
                self.scope = self.scope.toggled();
                self.selected = 0;
                match self.scope {
                    SearchScope::Content => self.search_action(),
                    SearchScope::Title => ChatListAction::None,
                }
            }
            // Go from "which chats mention this" to "where exactly": the
            // message-level results screen. Content mode only — in title
            // mode the query is a title substring, which is not a thing to
            // search message text for; the hint is hidden there too, so no
            // advertised key is a no-op.
            'g' => match self.scope {
                SearchScope::Content => ChatListAction::SearchMessages {
                    query: self.query.clone(),
                    sort: self.sort,
                },
                SearchScope::Title => ChatListAction::None,
            },
            _ => ChatListAction::None,
        }
    }

    /// `Enter`: opens the selected chat. In content mode the chat is opened
    /// **at its first match** rather than at the tail: the user asked where
    /// this text is, so landing on it is strictly more useful than landing at
    /// the end of the conversation (stage 2a's jump). Title mode is unchanged.
    fn open_selected(&self) -> ChatListAction {
        match (self.selected_id(), self.scope) {
            (Some(id), SearchScope::Content) if !self.query.trim().is_empty() => {
                ChatListAction::OpenFirstMatch {
                    chat: id,
                    query: self.query.clone(),
                }
            }
            (Some(id), _) => ChatListAction::Switch(id),
            (None, _) => ChatListAction::None,
        }
    }

    /// Moves the selection down by `step`, clamped to the list's last item.
    fn select_down(&mut self, step: usize) {
        let len = self.visible().len();
        if len > 0 {
            self.selected = (self.selected + step).min(len - 1);
        }
    }

    /// `F2`: switches into rename mode for the selected chat (a no-op on an
    /// empty list).
    fn start_rename(&mut self) {
        if let Some(chat) = self.visible().get(self.selected) {
            // A single-line `InputBox` with the current title (cursor at the end).
            // `set_single_line` — before `set_text` (the single-line invariant).
            let mut input = InputBox::new();
            input.set_single_line(true);
            input.set_text(&chat.title);
            self.mode = Mode::Rename {
                id: chat.id,
                input: Box::new(input),
                spell_dirty: true,
            };
        }
    }

    fn on_key_rename(&mut self, key: KeyEvent) -> ChatListAction {
        let Mode::Rename {
            id,
            input,
            spell_dirty,
        } = &mut self.mode
        else {
            return ChatListAction::None;
        };
        // All Ctrl combinations for the field (clear `Ctrl+K`, word-wise navigation `Ctrl+←/→`,
        // word deletion `Ctrl+Backspace/Delete`, `Ctrl+Home/End`, undo/redo
        // `Ctrl+Z/Y`) are handled by `InputBox` itself in `on_key` (layout-independent);
        // it swallows unrecognized ones. `Ctrl+K` → `Edited` → flag the highlighting to recompute.
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Search;
                ChatListAction::None
            }
            KeyCode::Enter => {
                let id = *id;
                let title = input.text();
                let action = match sanitize_title(&title) {
                    Some(title) => ChatListAction::Rename { id, title },
                    None => ChatListAction::None,
                };
                self.mode = Mode::Search;
                action
            }
            // Everything else (typing, word/character navigation, deletion, `Home/End`)
            // is handled by `InputBox` itself; on an actual edit we flag the error
            // highlighting for a recompute (cursor movement doesn't need it). See [`KeyOutcome`].
            _ => {
                if input.on_key(key).edited() {
                    *spell_dirty = true;
                }
                ChatListAction::None
            }
        }
    }

    /// Inserts clipboard text into the rename field (if it's open).
    /// Line breaks collapse into a space (`InputBox` is single-line). Outside rename
    /// mode — a no-op (paste isn't needed in the search line).
    pub fn handle_paste(&mut self, text: &str) {
        if let Mode::Rename {
            input, spell_dirty, ..
        } = &mut self.mode
        {
            input.insert_str(text);
            *spell_dirty = true;
        }
    }

    /// Recomputes spellcheck highlighting in the rename field, if it's
    /// open and flagged "dirty" (after an edit/paste). Returns `true` if the
    /// highlighting was updated (a repaint is needed). The widget doesn't own the checker —
    /// `app` lends it (the owner lives in the chat screen). See spec §11.5.
    pub fn recheck_rename_spelling(&mut self, spell: &SpellChecker) -> bool {
        let Mode::Rename {
            input, spell_dirty, ..
        } = &mut self.mode
        else {
            return false;
        };
        if !*spell_dirty {
            return false;
        }
        let ranges = vec![spell.misspellings(&input.text())];
        input.set_misspelled(ranges);
        *spell_dirty = false;
        true
    }

    /// Draws the fullscreen chat-list window (`area`). `active` — the current
    /// active chat (the marker). `&mut self` — the rename field draws an [`InputBox`]
    /// (it needs `&mut` for scroll/cursor). See spec §11.2.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        active: Option<Uuid>,
        palette: &Palette,
        loc: &'static Locale,
    ) {
        frame.render_widget(Clear, area);

        // At the bottom — a status line with "keycaps" (like on the chat screen): a neat
        // hotkey grid (the row count depends on width). Compute it ahead of time to
        // reserve exactly the height it needs.
        let status_lines = self.status_lines(palette, area.width as usize, loc);
        let status_h = (status_lines.len() as u16).max(1);
        let [main_area, status_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(status_h)]).areas(area);

        // The list panel: a rounded border, title on the left, dialog count on the right.
        let count = self.all.len();
        let title = format!(
            "{} {}",
            palette.glyphs().chats_icon,
            loc.t("ui.chatlist.title")
        );
        let block = palette.panel(title, true).title(
            Line::from(Span::styled(
                format!(
                    " {} ",
                    loc.tf("ui.chatlist.count", &[("n", &count.to_string())])
                ),
                palette.muted_style(),
            ))
            .right_aligned(),
        );
        let inner = block.inner(main_area);
        frame.render_widget(block, main_area);

        // Inside the panel: the search line (bordered, 3 rows), the list, and — if present —
        // an operation-status line (error/confirmation).
        let mut constraints = vec![Constraint::Length(3), Constraint::Min(1)];
        if self.error.is_some() || self.notice.is_some() {
            constraints.push(Constraint::Length(1));
        }
        let chunks = Layout::vertical(constraints).split(inner);
        let (search_area, list_area) = (chunks[0], chunks[1]);

        // --- the search line (bordered, with a `/` keycap on the right) OR the
        // rename field (a single-line `InputBox`: spellcheck, word-wise navigation,
        // a real cursor, horizontal scroll) ---
        if let Mode::Rename { input, .. } = &mut self.mode {
            input.render(
                frame,
                search_area,
                RenderOpts::focused(loc.t("ui.chatlist.rename_title")),
                palette,
            );
        } else {
            self.render_search(frame, search_area, palette, loc);
        }

        // --- the list ---
        let visible = self.visible();
        let width = list_area.width as usize;
        let items: Vec<ListItem> = visible
            .iter()
            .enumerate()
            .map(|(i, c)| {
                ListItem::new(self.item_line(c, i == self.selected, active, palette, width, loc))
            })
            .collect();
        // Selection — a soft backdrop (like the tint in the mockup), not inverting the whole line;
        // the selected row's green rail is added in `item_line`.
        let list = List::new(items).highlight_style(Style::new().bg(palette.keycap_bg));
        let mut list_state = ListState::default();
        if !visible.is_empty() {
            list_state.select(Some(self.selected.min(visible.len() - 1)));
        }
        frame.render_stateful_widget(list, list_area, &mut list_state);

        // The scrollbar on the "Chats" panel's right border — when there are more
        // chats than the list's visible height. The bar occupies only the list's rows
        // (the search line and status aren't touched); position — the list's actual offset after rendering.
        render_scrollbar(
            frame,
            Rect {
                x: main_area.x,
                y: list_area.y,
                width: main_area.width,
                height: list_area.height,
            },
            visible.len(),
            list_area.height as usize,
            list_state.offset(),
            true, // the panel border — in the focused color (panel(_, true))
            palette,
        );

        // --- the operation-status area: error (red) or confirmation (success) ---
        if let Some(err) = &self.error {
            let line = Line::from(vec![
                Span::from(format!("{} ", palette.glyphs().warn)).fg(palette.error),
                Span::from(err.clone()).fg(palette.error),
            ]);
            frame.render_widget(Paragraph::new(line), chunks[2]);
        } else if let Some(notice) = &self.notice {
            let line = Line::from(vec![
                Span::from(format!("{} ", palette.glyphs().ok)).fg(palette.success),
                Span::from(notice.clone()).fg(palette.success),
            ]);
            frame.render_widget(Paragraph::new(line), chunks[2]);
        }

        // --- the status line (hotkeys, like on the chat screen) ---
        frame.render_widget(Paragraph::new(status_lines), status_area);
    }

    /// Draws the bordered search line; on the right — the "/" keycap (a focus hint).
    fn render_search(
        &self,
        frame: &mut Frame,
        area: Rect,
        palette: &Palette,
        loc: &'static Locale,
    ) {
        let glyphs = palette.glyphs();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(glyphs.border)
            .border_style(palette.border_style(true));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // The field, the current search mode, and the column for the "/" keycap
        // on the right. The mode is shown because it changes what typing does
        // (`Ctrl+F`, see [`SearchScope`]); it is accented in content mode — the
        // departure from the historical behaviour — and muted in title mode.
        let mode = mode_label(self.scope, loc);
        let mode_w = display_width_str(mode) as u16 + 1;
        let [field, mode_area, cap] = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Length(mode_w),
            Constraint::Length(3),
        ])
        .areas(inner);

        let mut spans = vec![
            Span::styled(format!("{} ", glyphs.search), palette.muted_style()),
            Span::styled(self.query.clone(), Style::new().fg(palette.text)),
            Span::styled(glyphs.caret, palette.muted_style()),
        ];
        if self.query.is_empty() {
            spans.push(Span::styled(
                match self.scope {
                    SearchScope::Title => loc.t("ui.chatlist.search_placeholder"),
                    SearchScope::Content => loc.t("ui.chatlist.search_placeholder_content"),
                },
                palette.muted_style(),
            ));
        }
        let mode_style = match self.scope {
            SearchScope::Title => palette.muted_style(),
            SearchScope::Content => Style::new().fg(palette.accent),
        };
        frame.render_widget(Paragraph::new(Line::from(spans)), field);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(mode, mode_style))),
            mode_area,
        );
        frame.render_widget(Paragraph::new(Line::from(palette.keycap("/"))), cap);
    }

    /// A chat row: the selected row's colored rail, a dot (green for the active chat),
    /// the title on the left, and the message count right-aligned (`width` — the width
    /// of the list area). A long title is truncated with an ellipsis.
    fn item_line(
        &self,
        chat: &ChatSummary,
        is_selected: bool,
        active: Option<Uuid>,
        palette: &Palette,
        width: usize,
        loc: &'static Locale,
    ) -> Line<'static> {
        let is_active = active == Some(chat.id);
        // The selected row's rail (2 columns) + the dot (2 columns) = the prefix.
        let rail = if is_selected {
            Span::styled("▌ ", Style::new().fg(palette.success))
        } else {
            Span::raw("  ")
        };
        let dot_color = if is_active {
            palette.success
        } else {
            palette.border
        };
        let title_style = if is_active {
            Style::new().fg(palette.text).bold()
        } else {
            Style::new().fg(palette.text)
        };

        let count = loc.tf(
            "ui.chatlist.messages",
            &[("n", &chat.message_count.to_string())],
        );
        let count_w = display_width_str(&count);
        const PREFIX_W: usize = 4; // rail (2) + the "● " dot (2)
        const TRAIL: usize = 1; // right-hand margin
        // Available width for the title (a minimum 1-column gap before the counter).
        let max_title = width.saturating_sub(PREFIX_W + count_w + TRAIL + 1);
        let (title, title_w) = wrap::truncate_to_width(&chat.title, max_title);
        let gap = width
            .saturating_sub(PREFIX_W + title_w + count_w + TRAIL)
            .max(1);

        Line::from(vec![
            rail,
            Span::styled("● ", Style::new().fg(dot_color)),
            Span::styled(title, title_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(count, palette.muted_style()),
        ])
    }

    /// Status (hotkey) lines at the bottom of the screen — like on the chat screen: "keycaps"
    /// on a muted background + muted descriptions, laid out in a neat grid
    /// (columns line up vertically). The number of rows is fitted to width `width`:
    /// we take the max number of columns that fit the width (fewest rows). In
    /// rename mode — a single hint line.
    fn status_lines(
        &self,
        palette: &Palette,
        width: usize,
        loc: &'static Locale,
    ) -> Vec<Line<'static>> {
        if let Mode::Rename { .. } = self.mode {
            return vec![Line::from(vec![
                palette.keycap("Enter"),
                Span::styled(
                    format!(" {}   ", loc.t("ui.chatlist.rename.save")),
                    palette.muted_style(),
                ),
                palette.keycap("Esc"),
                Span::styled(
                    format!(" {}", loc.t("ui.chatlist.rename.cancel")),
                    palette.muted_style(),
                ),
            ])];
        }

        // Pairs "key — description — is it dangerous" (order = read left-to-right,
        // top-to-bottom). `Tab` carries the current sort mode. The grid is laid out by
        // the shared helper `Palette::hotkey_grid` (the same one the chat's status bar uses).
        let sort_desc = loc.tf("ui.chatlist.sort", &[("sort", sort_label(self.sort, loc))]);
        // `Ctrl+F` carries the current search mode, the same way `Tab` carries
        // the sort mode.
        let search_desc = loc.tf(
            "ui.chatlist.search_mode",
            &[("mode", mode_label(self.scope, loc))],
        );
        let mut items: Vec<(&str, &str, bool)> = vec![
            ("↑↓ PgUp/Dn Home/End", loc.t("ui.chatlist.hk.select"), false),
            ("Enter", loc.t("ui.chatlist.hk.open"), false),
            ("Ctrl+F", search_desc.as_str(), false),
            ("F2", loc.t("ui.chatlist.hk.rename"), false),
            ("Ctrl+R", loc.t("ui.chatlist.hk.autoname"), false),
            ("Ctrl+N", loc.t("ui.chatlist.hk.new"), false),
            ("Ctrl+D", loc.t("ui.chatlist.hk.clone"), false),
            ("F5", loc.t("ui.chatlist.hk.copy"), false),
            ("Del", loc.t("ui.chatlist.hk.delete"), true),
            ("Esc", loc.t("ui.chatlist.hk.back"), false),
            ("Ctrl+Q", loc.t("ui.chatlist.hk.quit"), false),
            ("Tab", sort_desc.as_str(), false),
        ];
        // Only in content mode, because that is the only mode it does anything
        // in — an advertised key that is a no-op is worse than a missing hint.
        if self.scope == SearchScope::Content {
            items.push(("Ctrl+G", loc.t("ui.chatlist.hk.search_messages"), false));
        }
        palette.hotkey_grid(&items, width)
    }
}

/// The visible width of a string in terminal columns.
fn display_width_str(s: &str) -> usize {
    wrap::display_width(&s.chars().collect::<Vec<_>>())
}

/// A localized search-mode label (the search line and the `Ctrl+F` hint).
fn mode_label(scope: SearchScope, loc: &'static Locale) -> &'static str {
    match scope {
        SearchScope::Title => loc.t("ui.chatlist.mode.title"),
        SearchScope::Content => loc.t("ui.chatlist.mode.content"),
    }
}

/// A localized sort-mode label for the indicator (`Tab` in the chat list).
fn sort_label(sort: SortMode, loc: &'static Locale) -> &'static str {
    match sort {
        SortMode::Created => loc.t("ui.sort.created"),
        SortMode::Modified => loc.t("ui.sort.modified"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn chat(title: &str) -> ChatSummary {
        ChatSummary {
            id: Uuid::new_v4(),
            profile_id: Uuid::nil(),
            title: title.to_string(),
            created_at: Utc::now(),
            modified_at: Utc::now(),
            message_count: 0,
        }
    }

    #[test]
    fn typing_filters_and_enter_switches() {
        let chats = vec![chat("Альфа"), chat("Бета")];
        let beta_id = chats[1].id;
        let mut s = ChatListState::new(chats, None);

        for c in "Бет".chars() {
            assert_eq!(s.on_key(key(KeyCode::Char(c))), ChatListAction::None);
        }
        // one chat remains — and it's the selected one
        assert_eq!(s.selected_id(), Some(beta_id));
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Switch(beta_id)
        );
    }

    #[test]
    fn ctrl_f_toggles_search_mode_and_asks_for_content_results() {
        let mut s = ChatListState::new(vec![chat("Альфа")], None);
        for c in "тек".chars() {
            assert_eq!(s.on_key(key(KeyCode::Char(c))), ChatListAction::None);
        }
        assert_eq!(s.scope, SearchScope::Title);

        // Switching into content mode searches the text already typed, rather
        // than waiting for the next keystroke.
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('f'))),
            ChatListAction::SearchContent("тек".into())
        );
        assert_eq!(s.scope, SearchScope::Content);
        // Also under a Cyrillic layout (physical F = Ctrl+а).
        assert_eq!(s.on_key(ctrl(KeyCode::Char('а'))), ChatListAction::None);
        assert_eq!(s.scope, SearchScope::Title, "and back again");
    }

    #[test]
    fn editing_the_query_searches_in_content_mode_only() {
        let mut s = ChatListState::new(vec![chat("Альфа")], None);
        // Title mode: filtering is local, so no round-trip.
        assert_eq!(s.on_key(key(KeyCode::Char('a'))), ChatListAction::None);
        assert_eq!(s.on_key(key(KeyCode::Backspace)), ChatListAction::None);

        s.on_key(ctrl(KeyCode::Char('f')));
        assert_eq!(
            s.on_key(key(KeyCode::Char('x'))),
            ChatListAction::SearchContent("x".into())
        );
        assert_eq!(
            s.on_key(key(KeyCode::Char('y'))),
            ChatListAction::SearchContent("xy".into())
        );
        assert_eq!(
            s.on_key(key(KeyCode::Backspace)),
            ChatListAction::SearchContent("x".into())
        );
    }

    #[test]
    fn content_results_filter_the_list_and_ignore_the_title() {
        // The point of content mode: a chat whose *title* does not contain the
        // query still shows up, because its messages matched.
        let chats = vec![chat("Альфа"), chat("Бета"), chat("Гамма")];
        let (alpha, gamma) = (chats[0].id, chats[2].id);
        let mut s = ChatListState::new(chats, None);
        s.on_key(ctrl(KeyCode::Char('f')));

        // Nothing back yet — everything is shown, exactly like an empty query.
        for c in "нечто".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.visible().len(), 3, "no result yet — show everything");

        s.set_search_results("нечто".into(), Some(vec![alpha, gamma]));
        let visible: Vec<Uuid> = s.visible().iter().map(|c| c.id).collect();
        assert_eq!(visible.len(), 2);
        assert!(visible.contains(&alpha) && visible.contains(&gamma));

        // An unsearchable query (too short for trigram) filters nothing.
        s.set_search_results("не".into(), None);
        assert_eq!(s.visible().len(), 3);

        // An empty result is an empty list — not "show everything".
        s.set_search_results("нечто".into(), Some(vec![]));
        assert!(s.visible().is_empty());
        assert_eq!(s.selected_id(), None);
    }

    #[test]
    fn stale_results_are_still_applied_rather_than_flashing_the_full_list() {
        // Results arrive a keystroke or two behind what is typed. Showing the
        // previous answer beats showing every chat between keystrokes.
        let chats = vec![chat("Альфа"), chat("Бета")];
        let alpha = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(ctrl(KeyCode::Char('f')));
        for c in "текст".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }

        s.set_search_results("тек".into(), Some(vec![alpha]));
        assert_eq!(s.visible().len(), 1, "an older answer is still applied");
        assert_eq!(s.selected_id(), Some(alpha));
    }

    /// Stage 2b: in content mode `Enter` opens the chat **at its first match**
    /// rather than at its tail — the user asked where this text is. Title mode
    /// keeps the historical plain switch.
    #[test]
    fn enter_opens_at_the_first_match_in_content_mode_only() {
        let chats = vec![chat("Альфа")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);

        // Title mode — unchanged.
        for c in "Аль".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.on_key(key(KeyCode::Enter)), ChatListAction::Switch(id));

        // Content mode — a jump, carrying the raw query for the orchestrator to
        // resolve against the index.
        s.on_key(ctrl(KeyCode::Char('f')));
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::OpenFirstMatch {
                chat: id,
                query: "Аль".into()
            }
        );

        // With nothing typed there is no match to open at — a plain switch.
        for _ in 0..3 {
            s.on_key(key(KeyCode::Backspace));
        }
        assert_eq!(s.on_key(key(KeyCode::Enter)), ChatListAction::Switch(id));
    }

    /// `Ctrl+G` hands the query to the message-level screen — and only in
    /// content mode, where the hint for it is also the only place it is shown.
    #[test]
    fn ctrl_g_asks_for_message_search_in_content_mode_only() {
        let mut s = ChatListState::new(vec![chat("Альфа")], None);
        for c in "марк".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('g'))),
            ChatListAction::None,
            "title mode has no message search"
        );

        s.on_key(ctrl(KeyCode::Char('f')));
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('g'))),
            ChatListAction::SearchMessages {
                query: "марк".into(),
                sort: SortMode::default(),
            }
        );
        // Also under a Cyrillic layout (physical G = Ctrl+п).
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('п'))),
            ChatListAction::SearchMessages {
                query: "марк".into(),
                sort: SortMode::default(),
            }
        );
    }

    #[test]
    fn the_message_search_hint_is_shown_only_in_content_mode() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let render = |state: &mut ChatListState| {
            let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
            term.draw(|f| state.render(f, f.area(), None, &Palette::default(), ru()))
                .unwrap();
            format!("{:?}", term.backend().buffer())
        };
        let mut s = ChatListState::new(vec![chat("Альфа")], None);
        assert!(
            !render(&mut s).contains("Ctrl+G"),
            "an advertised key that does nothing is worse than no hint"
        );
        s.on_key(ctrl(KeyCode::Char('f')));
        assert!(render(&mut s).contains("Ctrl+G"));
    }

    /// Closing the message-level screen brings the list back **still searching**
    /// — the user came from a content search, and an empty title-mode list would
    /// throw that away.
    #[test]
    fn restore_content_query_reopens_the_list_in_content_mode() {
        let chats = vec![chat("Альфа"), chat("Бета")];
        let alpha = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.restore_content_query("маркер".into());
        assert_eq!(s.query, "маркер");
        assert_eq!(s.scope, SearchScope::Content);
        // The title filter is not applied in content mode, so until results
        // arrive everything is shown — then they filter it.
        assert_eq!(s.visible().len(), 2);
        s.set_search_results("маркер".into(), Some(vec![alpha]));
        assert_eq!(s.selected_id(), Some(alpha));
    }

    #[test]
    fn title_mode_is_unaffected_by_content_results() {
        // Everything above must leave the historical behaviour alone.
        let chats = vec![chat("Альфа"), chat("Бета")];
        let beta = chats[1].id;
        let mut s = ChatListState::new(chats, None);
        // A result that arrived while content mode was on…
        s.on_key(ctrl(KeyCode::Char('f')));
        s.set_search_results("q".into(), Some(vec![]));
        // …must not survive the switch back to title search.
        s.on_key(ctrl(KeyCode::Char('f')));
        assert_eq!(s.visible().len(), 2);
        for c in "Бет".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.selected_id(), Some(beta));
    }

    #[test]
    fn search_mode_is_visible_in_the_search_line() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let render = |state: &mut ChatListState| {
            let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
            term.draw(|f| state.render(f, f.area(), None, &Palette::default(), ru()))
                .unwrap();
            format!("{:?}", term.backend().buffer())
        };

        let mut s = ChatListState::new(vec![chat("Альфа")], None);
        let dump = render(&mut s);
        assert!(dump.contains("названия"), "{dump}");
        s.on_key(ctrl(KeyCode::Char('f')));
        let dump = render(&mut s);
        assert!(
            dump.contains("содержимое"),
            "the mode must be visible — it changes what typing does: {dump}"
        );
    }

    #[test]
    fn esc_closes() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        assert_eq!(s.on_key(key(KeyCode::Esc)), ChatListAction::Close);
    }

    #[test]
    fn ctrl_q_and_f10_quit() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('q'))), ChatListAction::Quit);
        // Also under a Cyrillic layout (physical Q = Ctrl+й).
        assert_eq!(s.on_key(ctrl(KeyCode::Char('й'))), ChatListAction::Quit);
        // F10 — a second way to quit.
        assert_eq!(s.on_key(key(KeyCode::F(10))), ChatListAction::Quit);
        // Ctrl+C is no longer quit (freed up for copying).
        assert_ne!(s.on_key(ctrl(KeyCode::Char('c'))), ChatListAction::Quit);
    }

    #[test]
    fn ctrl_n_and_ctrl_d_and_delete() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('n'))), ChatListAction::New);
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('d'))),
            ChatListAction::Clone(id)
        );
        assert_eq!(s.on_key(key(KeyCode::Delete)), ChatListAction::Delete(id));
    }

    #[test]
    fn ctrl_shortcuts_work_under_cyrillic_layout() {
        // Russian layout: Ctrl+т (physical N) — new, Ctrl+в (physical D) — clone.
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('т'))), ChatListAction::New);
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('в'))),
            ChatListAction::Clone(id)
        );
        // Cyrillic search text still gets typed (without Ctrl).
        s.on_key(key(KeyCode::Char('я')));
        assert_eq!(s.query, "я");
    }

    #[test]
    fn f5_requests_copy_of_selected() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.on_key(key(KeyCode::F(5))), ChatListAction::Copy(id));
    }

    #[test]
    fn notice_is_set_and_cleared_on_next_key() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        s.set_notice("скопировано".into());
        assert_eq!(s.notice.as_deref(), Some("скопировано"));
        // Setting an error clears a confirmation and vice versa (mutually exclusive).
        s.set_error("боль".into());
        assert!(s.notice.is_none());
        s.set_notice("ок".into());
        assert!(s.error.is_none());
        // Any key press clears the confirmation.
        s.on_key(key(KeyCode::Down));
        assert!(s.notice.is_none());
    }

    #[test]
    fn ctrl_r_requests_auto_rename_of_selected() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('r'))),
            ChatListAction::AutoRename(id)
        );
        // Also under a Cyrillic layout (physical R = Ctrl+к).
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('к'))),
            ChatListAction::AutoRename(id)
        );
    }

    #[test]
    fn f2_enters_rename_and_enter_commits() {
        let chats = vec![chat("Старое")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);

        assert_eq!(s.on_key(key(KeyCode::F(2))), ChatListAction::None);
        // clear the buffer and type a new name
        for _ in 0.."Старое".chars().count() {
            s.on_key(key(KeyCode::Backspace));
        }
        for c in "Новое".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "Новое".into()
            }
        );
    }

    #[test]
    fn rename_cursor_moves_and_edits_in_middle() {
        // The cursor in rename mode moves via arrows/Home/End, edits go
        // at the cursor's position (not only at the end).
        let chats = vec![chat("abc")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2))); // buffer "abc", cursor at the end (3)
        s.on_key(key(KeyCode::Home)); // cursor → 0
        s.on_key(key(KeyCode::Char('X'))); // insert at the start → "Xabc", cursor 1
        s.on_key(key(KeyCode::End)); // cursor → 4 (end)
        s.on_key(key(KeyCode::Char('Y'))); // insert at the end → "XabcY", cursor 5
        s.on_key(key(KeyCode::Home)); // cursor → 0
        s.on_key(key(KeyCode::Right)); // cursor 1
        s.on_key(key(KeyCode::Right)); // cursor 2 (before 'b')
        s.on_key(key(KeyCode::Delete)); // delete 'b' in the middle → "XacY"
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "XacY".into()
            }
        );
    }

    #[test]
    fn rename_supports_input_box_word_navigation() {
        // The rename field is a single-line `InputBox`, so `Ctrl+Backspace`
        // deletes the whole word (the old character-by-character buffer couldn't do this).
        let chats = vec![chat("один два")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2))); // two-word title, cursor at the end
        s.on_key(ctrl(KeyCode::Backspace)); // delete the last word, leaving a trailing space
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "один".into() // sanitize_title strips the trailing space
            }
        );
    }

    #[test]
    fn rename_clear_with_ctrl_k_undo_with_ctrl_z() {
        // `Ctrl+K` clears the field, `Ctrl+Z` restores the text (the shared undo model, §C).
        let chats = vec![chat("Старое имя")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2)));
        s.on_key(ctrl(KeyCode::Char('k'))); // delete all the text
        s.on_key(ctrl(KeyCode::Char('z'))); // undo — restore what was deleted
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "Старое имя".into()
            }
        );
    }

    #[test]
    fn rename_spellcheck_underlines_misspelled_word() {
        use crate::features::spellcheck::SpellChecker;
        use std::collections::HashSet;
        let dict = spellbook::Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        let spell = SpellChecker::new(vec![dict], HashSet::new(), None);

        let mut s = ChatListState::new(vec![chat("helo")], None);
        s.on_key(key(KeyCode::F(2))); // enter rename mode, text "helo"
        // The spelling recompute flags "helo" as an error (a non-empty range).
        assert!(s.recheck_rename_spelling(&spell));
        match &s.mode {
            Mode::Rename {
                input, spell_dirty, ..
            } => {
                assert!(!*spell_dirty, "the flag is reset after a recompute");
                assert!(!input.misspelled_is_empty(), "the error must be underlined");
            }
            _ => panic!("expected rename mode"),
        }
        // Outside rename mode a recompute is a no-op.
        s.on_key(key(KeyCode::Esc));
        assert!(!s.recheck_rename_spelling(&spell));
    }

    #[test]
    fn rename_paste_inserts_into_field() {
        let chats = vec![chat("a")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2))); // "a", cursor at the end
        s.handle_paste("bc"); // paste from the clipboard
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "abc".into()
            }
        );
    }

    #[test]
    fn rename_esc_cancels_without_action() {
        let chats = vec![chat("Старое")];
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2)));
        assert_eq!(s.on_key(key(KeyCode::Esc)), ChatListAction::None);
        // back in search mode: typing filters again
        s.on_key(key(KeyCode::Char('x')));
        assert_eq!(s.selected_id(), None); // 'x' matched nothing
    }

    #[test]
    fn page_up_down_move_selection_by_page() {
        // 25 chats; PageDown moves by PAGE_STEP without going past the end, PageUp — backward.
        let chats: Vec<ChatSummary> = (0..25).map(|i| chat(&format!("чат {i}"))).collect();
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.selected, 0);
        s.on_key(key(KeyCode::PageDown));
        assert_eq!(s.selected, PAGE_STEP);
        s.on_key(key(KeyCode::PageDown));
        assert_eq!(s.selected, 2 * PAGE_STEP);
        // A third PageDown clamps to the last item (24), not overshooting the edge.
        s.on_key(key(KeyCode::PageDown));
        assert_eq!(s.selected, 24);
        s.on_key(key(KeyCode::PageUp));
        assert_eq!(s.selected, 24 - PAGE_STEP);
        // PageUp from near the top saturates at 0 (no underflow).
        s.on_key(key(KeyCode::PageUp));
        s.on_key(key(KeyCode::PageUp));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn home_end_jump_to_first_and_last() {
        let chats: Vec<ChatSummary> = (0..25).map(|i| chat(&format!("чат {i}"))).collect();
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::End));
        assert_eq!(s.selected, 24);
        s.on_key(key(KeyCode::Home));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn home_end_on_empty_list_is_noop() {
        let mut s = ChatListState::new(vec![], None);
        assert_eq!(s.on_key(key(KeyCode::End)), ChatListAction::None);
        assert_eq!(s.selected, 0);
        assert_eq!(s.on_key(key(KeyCode::Home)), ChatListAction::None);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn page_down_on_empty_list_is_noop() {
        let mut s = ChatListState::new(vec![], None);
        assert_eq!(s.on_key(key(KeyCode::PageDown)), ChatListAction::None);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn tab_toggles_sort_indicator() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        let before = s.sort;
        s.on_key(key(KeyCode::Tab));
        assert_ne!(s.sort, before);
    }

    #[test]
    fn error_is_set_and_cleared_on_next_key() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        s.set_error("боль".into());
        assert_eq!(s.error.as_deref(), Some("боль"));
        // Any key press clears the error.
        s.on_key(key(KeyCode::Down));
        assert!(s.error.is_none());
    }

    #[test]
    fn render_with_error_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut state = ChatListState::new(vec![chat("Альфа")], None);
        state.set_error("Недостаточно сообщений для авто-названия".into());
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| state.render(f, f.area(), None, &Palette::default(), ru()))
                .unwrap();
        }
    }

    #[test]
    fn scrollbar_appears_only_when_list_overflows() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // The "█" thumb on the panel's right border — only when there are more chats than the height.
        let has_thumb = |term: &Terminal<TestBackend>| {
            let buf = term.backend().buffer();
            let x = buf.area.right() - 1; // the border column of the "Chats" panel
            (buf.area.top()..buf.area.bottom()).any(|y| buf[(x, y)].symbol() == "█")
        };
        // Width 72 — the hotkey grid at the bottom is 2 columns (doesn't eat into the list's height).
        let mut term = Terminal::new(TestBackend::new(72, 24)).unwrap();
        let mut short = ChatListState::new(vec![chat("A"), chat("B")], None);
        term.draw(|f| short.render(f, f.area(), None, &Palette::default(), ru()))
            .unwrap();
        assert!(!has_thumb(&term), "a short list — no scrollbar thumb");
        let chats: Vec<ChatSummary> = (0..40).map(|i| chat(&format!("Чат {i}"))).collect();
        let mut long = ChatListState::new(chats, None);
        term.draw(|f| long.render(f, f.area(), None, &Palette::default(), ru()))
            .unwrap();
        assert!(has_thumb(&term), "a long list — with a scrollbar thumb");
    }

    #[test]
    fn render_does_not_panic_on_small_and_normal_areas() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut state = ChatListState::new(vec![chat("Альфа"), chat("Бета")], None);
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| state.render(f, f.area(), None, &Palette::default(), ru()))
                .unwrap();
        }
    }

    #[test]
    fn set_chats_keeps_selection_on_same_chat() {
        let chats = vec![chat("A"), chat("B"), chat("C")];
        let b_id = chats[1].id;
        let mut s = ChatListState::new(chats.clone(), None);
        s.on_key(key(KeyCode::Down)); // select B (index 1 in Modified order ~ the source order)
        let sel = s.selected_id();
        // update the list with the same set — selection stays on the same chat
        s.set_chats(chats);
        assert_eq!(s.selected_id(), sel);
        let _ = b_id;
    }

    #[test]
    fn deleting_selected_keeps_position_not_first() {
        // Deleting the selected chat (a re-emit of the list without it) leaves selection
        // at the same position — the next chat in the list ends up under it, rather
        // than jumping to the first item.
        let chats = vec![chat("A"), chat("B"), chat("C")];
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::Down)); // selection at position 1
        assert_eq!(s.selected, 1);
        // Take the order from the actual `visible` list (sorting may differ from
        // insertion order) — so the test doesn't depend on close-together timestamps.
        let victim_id = s.selected_id().unwrap();
        let next_id = s.visible()[2].id; // will end up under position 1 after deletion

        let remaining: Vec<ChatSummary> = s
            .visible()
            .into_iter()
            .filter(|c| c.id != victim_id)
            .collect();
        s.set_chats(remaining);

        assert_eq!(s.selected, 1, "the selection position is preserved");
        assert_eq!(
            s.selected_id(),
            Some(next_id),
            "under the selection — the former next chat, not the first one"
        );
    }

    #[test]
    fn deleting_last_selected_clamps_to_new_last() {
        // Deleting the selected last chat moves selection to the new last one
        // (not to the first).
        let chats = vec![chat("A"), chat("B"), chat("C")];
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::End)); // selection on the last one (position 2)
        assert_eq!(s.selected, 2);
        let victim_id = s.selected_id().unwrap();
        let new_last_id = s.visible()[1].id; // will become the new last one

        let remaining: Vec<ChatSummary> = s
            .visible()
            .into_iter()
            .filter(|c| c.id != victim_id)
            .collect();
        s.set_chats(remaining);

        assert_eq!(s.selected, 1, "selection clamps to the new last one");
        assert_eq!(s.selected_id(), Some(new_last_id));
    }
}
