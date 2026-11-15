//! A text selection over the visible cells of one read-only panel — the help,
//! an opened note, a diff, a version of the self-model
//! (docs/research/mouse-always-captured.md F13, §5.8).
//!
//! The cheap form the owner chose for text nobody edits: what is selected is
//! what the panel shows, read back from the drawn buffer — no knowledge of
//! wraps, no scrolling under a drag. A drag from a press in the panel selects,
//! clamped to it; each row's trailing blanks are trimmed and the rows joined by
//! `\n`. When what the panel shows changes — a scroll, a tab, a resize — the
//! selection points at other text, so it is dropped: the rows recorded at each
//! draw are compared with the ones before.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

use crate::shared::theme::Palette;

/// A panel's selection, kept between frames (the panel's state owns one).
#[derive(Debug, Default, Clone)]
pub struct CellSelection {
    /// The panel's area as last drawn, and the symbol of each of its cells,
    /// row by row (empty for a wide glyph's trailing cell).
    area: Rect,
    rows: Vec<Vec<String>>,
    /// The press, and the pointer once a drag has moved it — screen cells.
    anchor: Option<(u16, u16)>,
    head: Option<(u16, u16)>,
    /// A press in the panel is under way: its drag and release are ours.
    pressed: bool,
}

impl CellSelection {
    /// After the panel is drawn into `area` of `buf`: records what it shows
    /// — dropping the selection if that changed — and paints the selection
    /// over it with the field's own backdrop (`Palette::selection`; the mark
    /// the monochrome pass keeps as reverse video).
    pub fn record(&mut self, buf: &mut Buffer, area: Rect, palette: &Palette) {
        let rows: Vec<Vec<String>> = (area.y..area.bottom())
            .map(|y| {
                // The cell after a wide glyph is its trailing half: ratatui
                // draws a blank there, which is no text of the row's.
                let mut covered = false;
                (area.x..area.right())
                    .map(|x| {
                        let symbol = buf.cell((x, y)).map_or("", |c| c.symbol());
                        if std::mem::take(&mut covered) {
                            return String::new();
                        }
                        covered = UnicodeWidthStr::width(symbol) > 1;
                        symbol.to_string()
                    })
                    .collect()
            })
            .collect();
        if area != self.area || rows != self.rows {
            self.clear();
        }
        self.area = area;
        self.rows = rows;
        let Some((start, end)) = self.span() else {
            return;
        };
        for (y, x0, x1) in self.runs(start, end) {
            for x in x0..=x1 {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    let style = palette.selection(cell.style());
                    cell.set_style(style);
                }
            }
        }
    }

    /// A left-button report: a press in the panel starts a gesture (and drops
    /// a selection standing), a drag selects from the press to the pointer,
    /// clamped to the panel, the release ends it. `true` when the report was
    /// the panel's — the caller gives it to nothing else.
    pub fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        let at = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.clear();
                if !self.area.contains(Position::new(at.0, at.1)) {
                    return false;
                }
                self.anchor = Some(at);
                self.pressed = true;
                true
            }
            MouseEventKind::Drag(MouseButton::Left) if self.pressed => {
                if Some(at) != self.anchor || self.head.is_some() {
                    self.head = Some(self.clamp(at));
                }
                true
            }
            MouseEventKind::Up(MouseButton::Left) if self.pressed => {
                self.pressed = false;
                true
            }
            _ => false,
        }
    }

    /// Whether something is selected.
    #[cfg(test)]
    pub fn has_selection(&self) -> bool {
        self.span().is_some()
    }

    /// The selected text: each row's cells in the selection, its trailing
    /// blanks trimmed, the rows joined by `\n`. `None` without a selection.
    pub fn text(&self) -> Option<String> {
        let (start, end) = self.span()?;
        let lines: Vec<String> = self
            .runs(start, end)
            .map(|(y, x0, x1)| {
                let row = &self.rows[usize::from(y - self.area.y)];
                let from = usize::from(x0 - self.area.x);
                let to = usize::from(x1 - self.area.x);
                let text: String = row[from..=to].concat();
                text.trim_end().to_string()
            })
            .collect();
        Some(lines.join("\n").trim_end().to_string())
    }

    /// Drops the selection and any gesture under way.
    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.pressed = false;
    }

    /// The selection's ends in reading order — (column, row) cells — once a
    /// drag has made one.
    fn span(&self) -> Option<((u16, u16), (u16, u16))> {
        let (a, h) = (self.anchor?, self.head?);
        let (start, end) = if (a.1, a.0) <= (h.1, h.0) {
            (a, h)
        } else {
            (h, a)
        };
        // A wide glyph is in when either of its cells is: a start on its
        // trailing cell steps back onto the glyph.
        let start_col = if start.0 > self.area.x && self.symbol(start).is_empty() {
            start.0 - 1
        } else {
            start.0
        };
        Some(((start_col, start.1), end))
    }

    /// The selected cells, row by row: `(row, first column, last column)`.
    fn runs(
        &self,
        start: (u16, u16),
        end: (u16, u16),
    ) -> impl Iterator<Item = (u16, u16, u16)> + '_ {
        let right = self.area.right().saturating_sub(1);
        (start.1..=end.1).map(move |y| {
            let x0 = if y == start.1 { start.0 } else { self.area.x };
            let x1 = if y == end.1 { end.0 } else { right };
            (y, x0, x1)
        })
    }

    fn symbol(&self, (x, y): (u16, u16)) -> &str {
        self.rows
            .get(usize::from(y.saturating_sub(self.area.y)))
            .and_then(|row| row.get(usize::from(x.saturating_sub(self.area.x))))
            .map_or("", String::as_str)
    }

    /// The pointer clamped into the panel — a drag past its edge selects to it.
    fn clamp(&self, (x, y): (u16, u16)) -> (u16, u16) {
        let a = self.area;
        (
            x.clamp(a.x, a.right().saturating_sub(1)),
            y.clamp(a.y, a.bottom().saturating_sub(1)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::style::Style;

    fn at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn panel(lines: &[&str]) -> (Buffer, Rect) {
        let area = Rect::new(2, 1, 12, lines.len() as u16);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
        for (i, line) in lines.iter().enumerate() {
            buf.set_string(area.x, area.y + i as u16, line, Style::new());
        }
        (buf, area)
    }

    /// A drag selects the cells from the press to the pointer in reading
    /// order — backward too — each row's trailing blanks trimmed; a drag past
    /// the panel's edge is clamped to it; a press alone selects nothing.
    #[test]
    fn a_drag_selects_the_visible_cells_and_copies_their_text() {
        let (mut buf, area) = panel(&["first line", "second", "third row"]);
        let mut sel = CellSelection::default();
        let palette = Palette::default();
        sel.record(&mut buf, area, &palette);
        let left = MouseButton::Left;
        assert!(
            !sel.on_mouse(at(MouseEventKind::Down(left), 0, 0)),
            "outside"
        );
        assert!(sel.on_mouse(at(MouseEventKind::Down(left), 8, 1)));
        assert!(!sel.has_selection(), "a press alone");
        sel.on_mouse(at(MouseEventKind::Drag(left), 4, 3));
        sel.on_mouse(at(MouseEventKind::Up(left), 4, 3));
        assert_eq!(sel.text().as_deref(), Some("line\nsecond\nthi"));
        sel.record(&mut buf, area, &palette);
        assert_eq!(buf[(8, 1)].bg, palette.selection(Style::new()).bg.unwrap());
        assert!(sel.has_selection(), "the same text drawn again keeps it");
        // Backward, and past the panel's left edge.
        sel.on_mouse(at(MouseEventKind::Down(left), 7, 2));
        sel.on_mouse(at(MouseEventKind::Drag(left), 0, 1));
        assert_eq!(sel.text().as_deref(), Some("first line\nsecond"));
    }

    /// What the panel shows changing — a scroll, another tab — drops the
    /// selection: it would point at other text.
    #[test]
    fn new_text_under_the_selection_drops_it() {
        let (mut buf, area) = panel(&["alpha", "beta"]);
        let mut sel = CellSelection::default();
        let palette = Palette::default();
        sel.record(&mut buf, area, &palette);
        let left = MouseButton::Left;
        sel.on_mouse(at(MouseEventKind::Down(left), 2, 1));
        sel.on_mouse(at(MouseEventKind::Drag(left), 4, 2));
        assert!(sel.has_selection());
        let (mut scrolled, _) = panel(&["beta", "gamma"]);
        sel.record(&mut scrolled, area, &palette);
        assert!(!sel.has_selection());
        assert_eq!(sel.text(), None);
    }

    /// A wide glyph is in when either of its cells is.
    #[test]
    fn a_wide_glyph_at_the_start_is_taken_whole() {
        let (mut buf, area) = panel(&["a😀bc"]);
        let mut sel = CellSelection::default();
        sel.record(&mut buf, area, &Palette::default());
        let left = MouseButton::Left;
        // The emoji takes columns 3–4; the press is on its trailing half.
        sel.on_mouse(at(MouseEventKind::Down(left), 4, 1));
        sel.on_mouse(at(MouseEventKind::Drag(left), 5, 1));
        assert_eq!(sel.text().as_deref(), Some("😀b"));
    }
}
