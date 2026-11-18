//! A list of one-line rows to pick one from, over the chat: what `/fork` and
//! `/branch` open when they are typed bare (docs/research/chat-branches.md
//! §3.6, spec §11.3).
//!
//! Self-contained like the reference picker it is modelled on
//! ([`super::chat_link_picker`]): it holds a snapshot of rows and answers key
//! presses with a [`RowPickerAction`] the layer above executes. What a row
//! *means* — a message to fork at, a branch to switch to — is the layer
//! above's: the picker answers with the row's index.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseEvent};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem};

use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;
use crate::shared::ui::{ListScroll, MinSize, legend_width, mark_selected};
use crate::shared::wrap;

/// Action the overlay asks the layer above to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowPickerAction {
    /// The key press was handled inside the overlay.
    None,
    /// Close the overlay, nothing picked.
    Cancel,
    /// The row at this index was picked.
    Pick(usize),
}

/// One row: what names it, what it says, and what stands at its right end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickerRow {
    /// A short column drawn muted before the text: a message's number, a
    /// variant's figure. Kept whole.
    pub lead: String,
    /// The row's text: cut with its "…" to what the other two columns leave.
    pub text: String,
    /// A short column drawn muted at the right end. Kept whole.
    pub trail: String,
}

/// State of the list.
pub struct RowPickerState {
    title: String,
    /// The key legend on the bottom border (a `ui.*` bundle key).
    footer: &'static str,
    rows: Vec<PickerRow>,
    selected: usize,
    /// The popup is capped at the screen's height, so a long list scrolls —
    /// see [`ListScroll`].
    scroll: ListScroll,
}

impl RowPickerState {
    /// Opens the list over `rows`, on row `selected` (clamped to the list).
    pub fn new(title: String, footer: &'static str, rows: Vec<PickerRow>, selected: usize) -> Self {
        let selected = selected.min(rows.len().saturating_sub(1));
        Self {
            title,
            footer,
            rows,
            selected,
            scroll: ListScroll::default(),
        }
    }

    /// A mouse event over the popup: the wheel scrolls it, the selection
    /// pushed along by the edge it would leave; a press on a row selects it.
    /// `true` when a press landed on a row — the chat makes a double click its
    /// `Enter` (docs/research/mouse-always-captured.md F11–F12).
    pub fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        self.scroll.on_mouse(mouse, &mut self.selected)
    }

    /// The first screen cell row `item` was drawn on, for tests that click it.
    #[cfg(test)]
    pub fn cell_of(&self, item: usize) -> Option<(u16, u16)> {
        self.scroll.cell_of(item)
    }

    /// The selected row's index, for tests.
    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Handles a key press, returning the action to execute.
    pub fn on_key(&mut self, key: KeyEvent) -> RowPickerAction {
        if key.kind != KeyEventKind::Press {
            return RowPickerAction::None;
        }
        let last = self.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return RowPickerAction::Cancel,
            KeyCode::Enter if self.rows.is_empty() => return RowPickerAction::Cancel,
            KeyCode::Enter => return RowPickerAction::Pick(self.selected),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            _ => {}
        }
        RowPickerAction::None
    }

    /// The smallest window the overlay is drawn in (spec §11.1.1): its key
    /// legend whole on the border — the overlay is never narrower than that —
    /// around three rows of the list.
    pub fn min_size(footer: &'static str, loc: &'static Locale) -> MinSize {
        MinSize::new(legend_width(loc.t(footer)), 5)
    }

    /// Draws the overlay centered in `area`.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        palette: &Palette,
        loc: &'static Locale,
    ) {
        let rows = (self.rows.len() as u16 + 2).clamp(5, area.height);
        let min_w = Self::min_size(self.footer, loc).width;
        // Wide: a row is a line of somebody's message between two columns
        // that are kept whole, and what the text gets is what is left.
        let popup = centered_rect(90, 44.max(min_w), rows, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel(self.title.clone(), true)
            .title_bottom(Line::from(Span::styled(
                loc.t(self.footer),
                palette.muted_style(),
            )));
        // The selection's marker (the monochrome mode) is a column of every
        // row, and comes out of the text like the two side columns.
        let mark = palette.selected_mark().map_or(0, wrap::str_width);
        let inner = (popup.width as usize).saturating_sub(2 + mark);
        let lead_w = self
            .rows
            .iter()
            .map(|r| wrap::str_width(&r.lead))
            .max()
            .unwrap_or(0);
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|r| {
                // The lead is a column: padded to the widest, so the texts
                // start under one another.
                let lead = format!("{:<lead_w$}  ", r.lead);
                let trail = if r.trail.is_empty() {
                    String::new()
                } else {
                    format!("  {}", r.trail)
                };
                let budget = inner.saturating_sub(wrap::str_width(&lead) + wrap::str_width(&trail));
                let (text, _) = wrap::truncate_to_width(&r.text, budget);
                ListItem::new(Line::from(vec![
                    Span::styled(lead, palette.muted_style()),
                    Span::styled(text, Style::new().fg(palette.text)),
                    Span::styled(trail, palette.muted_style()),
                ]))
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
            self.rows.len(),
            popup.height.saturating_sub(2) as usize, // the panel's borders
            (!self.rows.is_empty()).then_some(self.selected),
        );
    }
}

/// How far `PageUp`/`PageDown` move the selection.
const PAGE: usize = 10;

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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    const FOOTER: &str = "ui.branch.pick.footer_fork";

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn rows(n: usize) -> Vec<PickerRow> {
        (1..=n)
            .map(|i| PickerRow {
                lead: format!("#{i}"),
                text: format!("message {i}"),
                trail: String::new(),
            })
            .collect()
    }

    fn screen(picker: &mut RowPickerState, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| picker.render(f, f.area(), &Palette::default(), ru()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn enter_picks_the_selected_row_and_esc_cancels() {
        let mut p = RowPickerState::new("t".into(), FOOTER, rows(3), 2);
        assert_eq!(p.on_key(key(KeyCode::Enter)), RowPickerAction::Pick(2));
        assert_eq!(p.on_key(key(KeyCode::Up)), RowPickerAction::None);
        assert_eq!(p.on_key(key(KeyCode::Enter)), RowPickerAction::Pick(1));
        assert_eq!(p.on_key(key(KeyCode::Esc)), RowPickerAction::Cancel);
    }

    #[test]
    fn the_selection_stays_within_the_list() {
        let mut p = RowPickerState::new("t".into(), FOOTER, rows(25), 99);
        assert_eq!(p.selected(), 24, "opened past the end: on the last row");
        p.on_key(key(KeyCode::Down));
        assert_eq!(p.selected(), 24);
        p.on_key(key(KeyCode::PageUp));
        assert_eq!(p.selected(), 14);
        p.on_key(key(KeyCode::Home));
        p.on_key(key(KeyCode::Up));
        assert_eq!(p.selected(), 0);
        p.on_key(key(KeyCode::PageDown));
        assert_eq!(p.selected(), 10);
        p.on_key(key(KeyCode::End));
        assert_eq!(p.selected(), 24);

        // An empty list has nothing to pick: `Enter` closes it.
        let mut empty = RowPickerState::new("t".into(), FOOTER, Vec::new(), 0);
        assert_eq!(empty.on_key(key(KeyCode::Down)), RowPickerAction::None);
        assert_eq!(empty.on_key(key(KeyCode::Enter)), RowPickerAction::Cancel);
    }

    /// The side columns are kept whole and the text is the one cut, with the
    /// mark that says so; the leads are one column, so the texts line up.
    #[test]
    fn a_long_row_is_cut_in_its_text_alone() {
        let long = PickerRow {
            lead: "#128".into(),
            text: "a very long first line of a message that cannot fit the popup at all".into(),
            trail: "12 · 2026-11-18".into(),
        };
        let short = PickerRow {
            lead: "#7".into(),
            text: "short".into(),
            trail: String::new(),
        };
        let mut p = RowPickerState::new("t".into(), FOOTER, vec![long, short], 0);
        let drawn = screen(&mut p, 60, 12);
        let row = drawn.iter().find(|r| r.contains("#128")).expect("the row");
        assert!(row.contains('…'), "{row:?}");
        assert!(row.contains("12 · 2026-11-18"), "{row:?}");
        let other = drawn.iter().find(|r| r.contains("#7")).expect("the row");
        let col = |r: &str, s: &str| wrap::str_width(&r[..r.find(s).unwrap()]);
        assert_eq!(col(row, "a very"), col(other, "short"), "{drawn:?}");
    }
}
