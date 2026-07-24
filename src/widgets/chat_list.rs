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
    fn visible(&self) -> Vec<ChatSummary> {
        filter_and_sort(&self.all, &self.query, self.sort)
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
            return match physical {
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
                _ => ChatListAction::None,
            };
        }
        match key.code {
            KeyCode::F(10) => ChatListAction::Quit, // a second way to quit
            KeyCode::Esc => ChatListAction::Close,
            KeyCode::Enter => match self.selected_id() {
                Some(id) => ChatListAction::Switch(id),
                None => ChatListAction::None,
            },
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ChatListAction::None
            }
            KeyCode::Down => {
                let len = self.visible().len();
                if len > 0 {
                    self.selected = (self.selected + 1).min(len - 1);
                }
                ChatListAction::None
            }
            // Paged selection movement: step by a "page" (fixed,
            // since the list height is only known at render time).
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(PAGE_STEP);
                ChatListAction::None
            }
            KeyCode::PageDown => {
                let len = self.visible().len();
                if len > 0 {
                    self.selected = (self.selected + PAGE_STEP).min(len - 1);
                }
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
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
                ChatListAction::None
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.selected = 0;
                ChatListAction::None
            }
            _ => ChatListAction::None,
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

        // The field and the column for the "/" keycap on the right.
        let [field, cap] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(3)]).areas(inner);

        let mut spans = vec![
            Span::styled(format!("{} ", glyphs.search), palette.muted_style()),
            Span::styled(self.query.clone(), Style::new().fg(palette.text)),
            Span::styled(glyphs.caret, palette.muted_style()),
        ];
        if self.query.is_empty() {
            spans.push(Span::styled(
                loc.t("ui.chatlist.search_placeholder"),
                palette.muted_style(),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), field);
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
        let (title, title_w) = truncate_to_width(&chat.title, max_title);
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
        let items: [(&str, &str, bool); 11] = [
            ("↑↓ PgUp/Dn Home/End", loc.t("ui.chatlist.hk.select"), false),
            ("Enter", loc.t("ui.chatlist.hk.open"), false),
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
        palette.hotkey_grid(&items, width)
    }
}

/// The visible width of a string in terminal columns.
fn display_width_str(s: &str) -> usize {
    wrap::display_width(&s.chars().collect::<Vec<_>>())
}

/// A localized sort-mode label for the indicator (`Tab` in the chat list).
fn sort_label(sort: SortMode, loc: &'static Locale) -> &'static str {
    match sort {
        SortMode::Created => loc.t("ui.sort.created"),
        SortMode::Modified => loc.t("ui.sort.modified"),
    }
}

/// Truncates a string to width `max` columns, adding "…" on truncation. Returns
/// the truncated string and its actual width. At `max == 0` — an empty string.
fn truncate_to_width(s: &str, max: usize) -> (String, usize) {
    let chars: Vec<char> = s.chars().collect();
    let full = wrap::display_width(&chars);
    if full <= max {
        return (s.to_string(), full);
    }
    if max == 0 {
        return (String::new(), 0);
    }
    // Leave room for "…" (1 column).
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut w = 0;
    for i in 0..chars.len() {
        let cw = wrap::width_at(&chars, i);
        if w + cw > budget {
            break;
        }
        w += cw;
        out.push(chars[i]);
    }
    out.push('…');
    (out, w + 1)
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
