//! The `chat://` reference picker (`Ctrl+L`). See spec §11.3 and
//! docs/research/chat-uri-links.md §4.5.
//!
//! The feed draws a resolvable `chat://` address as a link; this overlay is how
//! it is *followed*. A keyboard route rather than a click, because mouse
//! capture is `Ctrl+W` and off by default — turning it on costs native terminal
//! selection, so a click would be unreachable for most users (fork F4). The
//! mouse is a second stage, not the way in.
//!
//! Self-contained like the profile picker it is modelled on: it holds a
//! snapshot and answers key presses with a [`ChatLinkAction`] the layer above
//! executes.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;
use crate::shared::ui::{ListScroll, MinSize, legend_width, mark_selected};
use crate::shared::wrap;

/// Action the overlay asks the layer above to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatLinkAction {
    /// The key press was handled inside the overlay.
    None,
    /// Close the overlay without following a reference.
    Cancel,
    /// Open the picked conversation.
    Open(Uuid),
}

/// State of the reference picker.
pub struct ChatLinkPickerState {
    /// The conversations this chat links to, in the order the feed offers them
    /// (the most recently drawn first).
    chats: Vec<ChatSummary>,
    /// The open conversation, when the feed links to it: picking it is not a
    /// switch, and saying so is the overlay's job rather than a silent no-op
    /// (docs/lessons.md §4).
    current: Option<Uuid>,
    selected: usize,
    /// The popup is capped at the screen's height, so a chat with many
    /// references does scroll — see [`ListScroll`].
    scroll: ListScroll,
}

impl ChatLinkPickerState {
    /// Opens the picker over a reference snapshot. `current` — the open chat's
    /// id, so an entry pointing back at it can be marked.
    pub fn new(chats: Vec<ChatSummary>, current: Option<Uuid>) -> Self {
        Self {
            chats,
            current,
            selected: 0,
            scroll: ListScroll::default(),
        }
    }

    /// Id of the selected conversation (if the list isn't empty).
    pub fn selected_id(&self) -> Option<Uuid> {
        self.chats.get(self.selected).map(|c| c.id)
    }

    /// Handles a key press, returning the action to execute.
    pub fn on_key(&mut self, key: KeyEvent) -> ChatLinkAction {
        if key.kind != KeyEventKind::Press {
            return ChatLinkAction::None;
        }
        match key.code {
            KeyCode::Esc => ChatLinkAction::Cancel,
            KeyCode::Enter => match self.selected_id() {
                Some(id) => ChatLinkAction::Open(id),
                None => ChatLinkAction::Cancel,
            },
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ChatLinkAction::None
            }
            KeyCode::Down => {
                if !self.chats.is_empty() {
                    self.selected = (self.selected + 1).min(self.chats.len() - 1);
                }
                ChatLinkAction::None
            }
            _ => ChatLinkAction::None,
        }
    }

    /// The smallest window the overlay is drawn in (spec §11.1): its key
    /// legend whole on the border — the overlay is never narrower than that —
    /// around three rows of the list.
    pub fn min_size(loc: &'static Locale) -> MinSize {
        MinSize::new(legend_width(loc.t("ui.chat_links.footer")), 5)
    }

    /// Draws the overlay centered in `area`.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        palette: &Palette,
        loc: &'static Locale,
    ) {
        let rows = (self.chats.len() as u16 + 2).clamp(5, area.height);
        let popup = centered_rect(60, 40.max(Self::min_size(loc).width), rows, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel(
                // The chat-list icon rather than a new one: this is a list of
                // conversations, and new chrome glyphs carry a width/WGL4
                // discipline of their own (docs/lessons.md §5).
                format!(
                    "{} {}",
                    palette.glyphs().chats_icon,
                    loc.t("ui.chat_links.title")
                ),
                true,
            )
            .title_bottom(Line::from(Span::styled(
                loc.t("ui.chat_links.footer"),
                palette.muted_style(),
            )));
        let items: Vec<ListItem> = self
            .chats
            .iter()
            .map(|c| {
                // The date is what tells two same-named conversations apart —
                // the same thing the chat list leans on — and the "current"
                // mark says which one is open, so both keep their columns and
                // the title takes what is left, cut with the "…" that says so
                // (a title is not bounded in storage, spec §11.2).
                let date = format!("  {}", c.modified_at.format("%Y-%m-%d"));
                let current = (self.current == Some(c.id))
                    .then(|| format!("  {}", loc.t("ui.chat_links.current")));
                // The selection's marker (the monochrome mode) is a column of
                // every row, and comes out of the title like the rest.
                let mark = palette.selected_mark().map_or(0, wrap::str_width);
                let budget = (popup.width as usize).saturating_sub(
                    2 + mark
                        + wrap::str_width(&date)
                        + current.as_deref().map_or(0, wrap::str_width),
                );
                let (title, _) = wrap::truncate_to_width(&c.title, budget);
                let mut spans = vec![Span::styled(title, Style::new().fg(palette.text))];
                spans.push(Span::styled(date, palette.muted_style()));
                if let Some(current) = current {
                    spans.push(Span::styled(current, palette.muted_style()));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list = mark_selected(
            List::new(items)
                .block(block)
                .highlight_style(Style::new().reversed()),
            palette,
        );
        self.scroll.render(
            frame,
            list,
            popup,
            self.chats.len(),
            popup.height.saturating_sub(2) as usize, // the panel's borders
            (!self.chats.is_empty()).then_some(self.selected),
        );
    }
}

/// A rectangle centered in `area`: `pct_x` percent width (≥`min_w`), fixed height.
fn centered_rect(pct_x: u16, min_w: u16, height: u16, area: Rect) -> Rect {
    let w = area.width.saturating_mul(pct_x) / 100;
    let [h_area] = Layout::horizontal([Constraint::Length(w.max(min_w).min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v_area] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h_area);
    v_area
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn chat(title: &str) -> ChatSummary {
        let mut c = ChatSummary::fixture(title);
        c.message_count = 1;
        c
    }

    #[test]
    fn arrows_move_and_stay_inside_the_list() {
        let chats = vec![chat("a"), chat("b")];
        let (first, last) = (chats[0].id, chats[1].id);
        let mut s = ChatLinkPickerState::new(chats, None);

        assert_eq!(s.selected_id(), Some(first));
        s.on_key(key(KeyCode::Up));
        assert_eq!(s.selected_id(), Some(first), "no wrap past the top");
        s.on_key(key(KeyCode::Down));
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected_id(), Some(last), "no wrap past the bottom");
    }

    /// A title too long for the popup is cut with the "…" that says so, and
    /// the date it would have pushed off the row keeps its place — nothing
    /// bounds a title in storage (spec §11.2).
    #[test]
    fn a_long_title_is_cut_and_the_date_survives() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let long = "заголовок который заведомо шире этого всплывающего окна";
        let mut s = ChatLinkPickerState::new(vec![chat(long)], None);
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term.draw(|f| s.render(f, f.area(), &Palette::default(), ru()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        let row = (buf.area.top()..buf.area.bottom())
            .map(|y| {
                (buf.area.left()..buf.area.right())
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .find(|row| row.contains("заголовок"))
            .expect("the row is drawn");
        assert!(row.contains('…'), "a cut title says so: {row}");
        assert!(
            row.contains(&chrono::Local::now().format("%Y").to_string()),
            "the date keeps its place: {row}"
        );
    }

    /// The monochrome mode marks the selected reference — reverse video was
    /// all that said it — and the marker's columns come out of the title, so
    /// the date and the "this conversation" label keep theirs (spec §11.6).
    #[test]
    fn the_selected_reference_is_marked_and_the_row_still_fits() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let long = "title that is certainly wider than this popup will ever be";
        let chats = vec![chat(long), chat("short")];
        let current = chats[0].id;
        let mut s = ChatLinkPickerState::new(chats, Some(current));
        let drawn = |s: &mut ChatLinkPickerState, palette: &Palette| -> Vec<String> {
            let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
            term.draw(|f| s.render(f, f.area(), palette, ru())).unwrap();
            crate::shared::ui::tests::buffer_rows(term.backend().buffer())
        };
        let year = chrono::Local::now().format("%Y").to_string();
        let current_label = ru().t("ui.chat_links.current");

        let rows = drawn(&mut s, &Palette::mono());
        let first = rows.iter().find(|r| r.contains(current_label)).unwrap();
        assert!(first.contains("│› tit"), "{first}");
        assert!(first.contains('…'), "the title is what gives way: {first}");
        assert!(first.contains(&year), "the date keeps its place: {first}");
        assert!(first.contains(current_label), "and the label: {first}");
        assert!(
            first.trim_end().ends_with('│'),
            "inside the border: {first}"
        );
        let second = rows.iter().find(|r| r.contains("short")).unwrap();
        assert!(second.contains("│  short"), "{second}");

        let rows = drawn(&mut s, &Palette::default());
        let first = rows.iter().find(|r| r.contains(current_label)).unwrap();
        assert!(first.contains("│tit"), "no marker elsewhere: {first}");
    }

    /// A chat with more references than the screen can hold scrolls, and it
    /// scrolls the same way in both directions: the selection crosses the
    /// visible rows first, the window moves only when it has to
    /// ([`ListScroll`], docs/lessons.md §5).
    #[test]
    fn a_long_reference_list_scrolls_symmetrically() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let chats: Vec<ChatSummary> = (0..30).map(|i| chat(&format!("чат {i}"))).collect();
        let mut s = ChatLinkPickerState::new(chats, None);
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        let mut draw = |s: &mut ChatLinkPickerState| {
            term.draw(|f| s.render(f, f.area(), &Palette::default(), ru()))
                .unwrap();
        };

        draw(&mut s);
        for _ in 0..29 {
            s.on_key(key(KeyCode::Down));
            draw(&mut s);
        }
        let bottom = s.scroll.offset();
        assert!(bottom > 0, "the popup is capped at the screen's height");

        s.on_key(key(KeyCode::Up));
        draw(&mut s);
        assert_eq!(
            s.scroll.offset(),
            bottom,
            "the rows must stay put while the selection can still move inside them"
        );

        for _ in 0..29 {
            s.on_key(key(KeyCode::Up));
            draw(&mut s);
        }
        assert_eq!(s.scroll.offset(), 0);
    }

    #[test]
    fn enter_opens_and_esc_cancels() {
        let chats = vec![chat("a")];
        let id = chats[0].id;
        let mut s = ChatLinkPickerState::new(chats, None);
        assert_eq!(s.on_key(key(KeyCode::Enter)), ChatLinkAction::Open(id));
        assert_eq!(s.on_key(key(KeyCode::Esc)), ChatLinkAction::Cancel);
    }

    /// A repeat (`KeyEventKind::Repeat`) or release must not open anything —
    /// the same guard the other overlays carry.
    #[test]
    fn only_presses_are_handled() {
        let mut s = ChatLinkPickerState::new(vec![chat("a")], None);
        let mut ev = key(KeyCode::Enter);
        ev.kind = KeyEventKind::Release;
        assert_eq!(s.on_key(ev), ChatLinkAction::None);
    }

    /// An empty list cannot be opened into: `Enter` closes instead of picking
    /// nothing.
    #[test]
    fn an_empty_list_cancels_on_enter() {
        let mut s = ChatLinkPickerState::new(Vec::new(), None);
        assert_eq!(s.on_key(key(KeyCode::Enter)), ChatLinkAction::Cancel);
        assert_eq!(s.selected_id(), None);
    }
}
