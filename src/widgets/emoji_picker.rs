//! The emoji-picker popup (`Ctrl+B` in the chat window): a grid of popular emoji, selection
//! via arrows, inserting the pick into the input box at the cursor. See spec §11.5.
//!
//! The widget is self-contained (FSD: `screens → widgets`): it only holds the selection
//! index, and responds to key presses with [`EmojiPickerAction`] — executed by the chat
//! screen (inserts the emoji via [`InputBox::insert_str`]). The emoji themselves are a
//! static list below; cursor/deletion by grapheme clusters is already correct
//! in the input box (see spec §11.5), so multi-scalar emoji (`👍🏽`) that
//! the user pastes from the clipboard or types themselves edit with no surprises.
//!
//! **The list itself is deliberately kept free of VS16 clusters** (`❤️`/`✌️`): in a
//! fixed-width grid they break the row's terminal-output layout —
//! details and mechanics at [`EMOJIS`].

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::shared::theme::Palette;

/// The most common emoji (4 rows of [`COLS`]). Order — from "faces" to gestures and
/// symbols; the list is fixed (extend it by editing here). The grid's width was chosen
/// so the popup's bottom hint fits in full.
///
/// **Invariant (pinned by the gate test [`tests::emoji_list_is_width2_without_vs16`]):
/// every emoji is exactly 2 columns and carries NO VS16 presentation selector (U+FE0F).**
///
/// Why no VS16 (`❤️` = ❤ + U+FE0F, `✌️` = ✌ + U+FE0F): for such a cluster
/// `ratatui` **deliberately sends the terminal its trailing cell** (a workaround for terminals
/// that don't clear a wide glyph's second half), while the `crossterm` backend tracks position
/// **by cell number, without accounting for glyph width** — so the trailing cell is printed
/// **without `MoveTo`** and physically lands one column to the right. The rest of the row drifts: the
/// next wide emoji's right half gets overwritten (the terminal blanks it out
/// entirely — this is how `🔥` used to disappear behind `❤️`), and the popup's border gets
/// pushed outward. Especially visible on a full redraw ([`crate::screens::chat::ChatScreen`]
/// requests one for popup actions), where ALL cells become "changed".
/// So instead of VS16 variants we keep supplementary-plane emoji
/// (`🤞`, `💖`) — they're honestly 2 columns wide and produce no trailing cells.
const EMOJIS: &[&str] = &[
    "😀", "😃", "😄", "😁", "😆", "😅", "😂", "🤣", "😊", "😍", "😘", //
    "😎", "🤔", "😉", "🙂", "🥳", "😴", "😭", "😢", "😡", "🤯", "🙄", //
    "👍", "👎", "👏", "🙏", "🙌", "💪", "🤝", "👀", "🤷", "👌", "🤞", //
    "💖", "🔥", "✨", "🎉", "💯", "✅", "❌", "⭐", "🚀", "💔", "🎯", //
];

/// Number of emoji per grid row.
const COLS: usize = 11;

/// An emoji cell's width in columns (` 😀 ` — a space + a width-2 emoji + a space).
const CELL_W: u16 = 4;

/// An action the popup asks the chat screen to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum EmojiPickerAction {
    /// The press was handled inside the popup (repaint).
    None,
    /// Close the popup without inserting.
    Cancel,
    /// Insert the emoji into the input box and close the popup.
    Pick(String),
}

/// State of the emoji-picker popup.
pub struct EmojiPickerState {
    /// The index of the selected emoji in [`EMOJIS`].
    selected: usize,
}

impl Default for EmojiPickerState {
    fn default() -> Self {
        Self::new()
    }
}

impl EmojiPickerState {
    /// Opens the popup (selection — on the first emoji).
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    /// Opens the popup with a restored selection (the index is clamped to bounds) —
    /// so the popup "remembers" the last chosen emoji between openings.
    pub fn with_selected(idx: usize) -> Self {
        Self {
            selected: idx.min(EMOJIS.len() - 1),
        }
    }

    /// The current selection index (the screen saves it to restore on the
    /// next open, via [`Self::with_selected`]).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The emoji under the selection (always valid: the list is non-empty and the index is clamped).
    fn selected_emoji(&self) -> String {
        EMOJIS[self.selected.min(EMOJIS.len() - 1)].to_string()
    }

    /// Handles a key press, returning the action to perform. Navigation —
    /// via arrows across the grid; `Enter` inserts, `Esc` closes.
    pub fn on_key(&mut self, key: KeyEvent) -> EmojiPickerAction {
        if key.kind != KeyEventKind::Press {
            return EmojiPickerAction::None;
        }
        match key.code {
            KeyCode::Esc => EmojiPickerAction::Cancel,
            KeyCode::Enter => EmojiPickerAction::Pick(self.selected_emoji()),
            KeyCode::Left => {
                self.selected = self.selected.saturating_sub(1);
                EmojiPickerAction::None
            }
            KeyCode::Right => {
                self.selected = (self.selected + 1).min(EMOJIS.len() - 1);
                EmojiPickerAction::None
            }
            KeyCode::Up => {
                if self.selected >= COLS {
                    self.selected -= COLS;
                }
                EmojiPickerAction::None
            }
            KeyCode::Down => {
                if self.selected + COLS < EMOJIS.len() {
                    self.selected += COLS;
                }
                EmojiPickerAction::None
            }
            _ => EmojiPickerAction::None,
        }
    }

    /// Draws the popup centered in `area`: an emoji grid, the selected cell — reversed.
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        palette: &Palette,
        loc: &'static crate::shared::i18n::Locale,
    ) {
        let rows = EMOJIS.len().div_ceil(COLS) as u16;
        let width = COLS as u16 * CELL_W + 2; // +2 — the border
        let popup = centered_rect(width, rows + 2, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel(format!("☺ {}", loc.t("ui.emoji.title")), true)
            .title_bottom(Line::from(Span::styled(
                loc.t("ui.emoji.footer"),
                palette.muted_style(),
            )));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let selected = self.selected.min(EMOJIS.len() - 1);
        let lines: Vec<Line> = EMOJIS
            .chunks(COLS)
            .enumerate()
            .map(|(r, row)| {
                let spans: Vec<Span> = row
                    .iter()
                    .enumerate()
                    .map(|(c, emoji)| {
                        let idx = r * COLS + c;
                        // Spaces around the emoji: the glyph is width 2, plus a margin — so
                        // ` {emoji} ` gives an even cell and a "button" look for the selected one.
                        let cell = format!(" {emoji} ");
                        if idx == selected {
                            // A dark selection background (like the selected row in the chat
                            // list) — doesn't blend with the colored glyph, unlike
                            // reversed (a light background used to wash out the emoji).
                            Span::styled(cell, Style::new().fg(palette.text).bg(palette.keycap_bg))
                        } else {
                            Span::styled(cell, Style::new().fg(palette.text))
                        }
                    })
                    .collect();
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// A rectangle centered in `area` with a fixed width/height (clamped).
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_picks_first_by_default() {
        let mut s = EmojiPickerState::new();
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            EmojiPickerAction::Pick(EMOJIS[0].to_string())
        );
    }

    #[test]
    fn arrows_navigate_grid() {
        let mut s = EmojiPickerState::new();
        // right → the second emoji in the first row
        assert_eq!(s.on_key(key(KeyCode::Right)), EmojiPickerAction::None);
        assert_eq!(s.selected, 1);
        // down → to the row below (same column)
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected, 1 + COLS);
        // up → back
        s.on_key(key(KeyCode::Up));
        assert_eq!(s.selected, 1);
        // left → to the first
        s.on_key(key(KeyCode::Left));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn navigation_clamps_at_bounds() {
        let mut s = EmojiPickerState::new();
        // left/up from the first cell — no movement
        s.on_key(key(KeyCode::Left));
        s.on_key(key(KeyCode::Up));
        assert_eq!(s.selected, 0);
        // reach the last cell and try to go further — clamps
        for _ in 0..EMOJIS.len() + 5 {
            s.on_key(key(KeyCode::Right));
        }
        assert_eq!(s.selected, EMOJIS.len() - 1);
        // down from the last row — no movement
        let last = s.selected;
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected, last);
    }

    #[test]
    fn with_selected_restores_and_clamps() {
        // Restores the index...
        assert_eq!(EmojiPickerState::with_selected(5).selected(), 5);
        // ...and clamps an out-of-bounds one to the last item.
        assert_eq!(
            EmojiPickerState::with_selected(9999).selected(),
            EMOJIS.len() - 1
        );
    }

    #[test]
    fn esc_cancels() {
        let mut s = EmojiPickerState::new();
        assert_eq!(s.on_key(key(KeyCode::Esc)), EmojiPickerAction::Cancel);
    }

    #[test]
    fn emoji_list_is_width2_without_vs16() {
        // A gate on the list's invariant (see the [`EMOJIS`] doc): every grid cell is
        // ` emoji ` at exactly width CELL_W, and VS16 clusters break the row's terminal
        // output (ratatui sends their trailing cell, the backend prints it without MoveTo
        // → the rest of the row drifts, the neighboring emoji goes blank, the border shifts).
        use unicode_width::UnicodeWidthStr;
        for (i, emoji) in EMOJIS.iter().enumerate() {
            assert!(
                !emoji.chars().any(|c| c == '\u{FE0F}'),
                "EMOJIS[{i}] = {emoji:?} — a VS16 cluster: use a supplementary-plane \
                 variant instead (e.g. ❤️→💖, ✌️→🤞)"
            );
            assert_eq!(
                emoji.width(),
                CELL_W as usize - 2,
                "EMOJIS[{i}] = {emoji:?} — width isn't 2 columns, the grid will drift"
            );
        }
    }

    #[test]
    fn grid_is_full_rows() {
        // The list fills rows of COLS exactly — otherwise the grid is "ragged".
        assert_eq!(EMOJIS.len() % COLS, 0);
    }

    #[test]
    fn sentinel_repaint_never_writes_into_second_half_of_wide_glyph() {
        // A gate on the row-shift mechanics (a regression: "after closing, 🔥
        // disappeared and the border broke"). A full redraw (the buffer sentinel, see
        // `ChatScreen::request_full_redraw`) makes ALL cells "changed". For a
        // VS16 cluster, `ratatui` in such a frame additionally sends its trailing
        // cell, and the `crossterm` backend tracks position by cell number without accounting for
        // glyph width (`last_pos.x + 1`) — such a trailing cell is printed without `MoveTo`,
        // physically lands one column to the right, and shifts the rest of the row.
        //
        // A terminal-independent invariant: diff updates never target the
        // second half of a wide glyph.
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use unicode_width::UnicodeWidthStr;

        let picker = EmojiPickerState::new();
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| picker.render(f, f.area(), &Palette::default(), ru()))
            .unwrap();
        let painted = term.backend().buffer().clone();
        let mut sentinel = painted.clone();
        crate::shared::ui::prime_full_redraw(&mut sentinel);
        for (x, y, _) in sentinel.diff(&painted) {
            if x == 0 {
                continue;
            }
            let left = painted[(x - 1, y)].symbol();
            assert!(
                left.width() < 2,
                "the update at ({x},{y}) targets the second half of a wide glyph \
                 {left:?} — it would print without MoveTo and shift the rest of the row"
            );
        }
    }

    #[test]
    fn upstream_repaints_styled_tail_on_close_but_not_on_selection_move() {
        // A canary on the boundary of the upstream fix. A wide emoji occupies TWO cells —
        // its own (the "symbol") and a trailing one, which ratatui resets to default. conhost
        // doesn't clear the second half itself, so the diff is required to rewrite it.
        //
        // ratatui-core 0.1.2 (ratatui#2585) learned to send the tail, but ONLY when
        // a wide glyph was replaced by narrower content AND carried a style visible on
        // an empty cell (a background, REVERSED/UNDERLINED/BLINK/CROSSED_OUT). The test pins both
        // sides of this boundary:
        //
        //   (A) closing the popup — the fix works, the selected cell's tail arrives;
        //   (B) shifting the selection — the glyph stays wide (`prev_width > next_width`
        //       doesn't hold), the tail does NOT arrive; the background drifts off the previous
        //       cell, and its half sticks around on conhost.
        //
        // Because of (B), `ChatScreen::handle_emoji_key` still requests a full
        // redraw. If part (B) fails — upstream closed this case too, the workaround
        // can be removed. If (A) fails — upstream regressed.
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::buffer::Buffer;
        use ratatui::style::Color;

        let render = |state: &EmojiPickerState| {
            let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
            term.draw(|f| state.render(f, f.area(), &Palette::default(), ru()))
                .unwrap();
            term.backend().buffer().clone()
        };

        let painted = render(&EmojiPickerState::new());
        let area = painted.area;

        // The selected cell: a wide emoji with a selection background.
        let (x, y) = (area.top()..area.bottom())
            .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
            .find(|&(x, y)| painted[(x, y)].symbol() == EMOJIS[0])
            .expect("emoji not found in the rendered grid");
        assert_ne!(
            painted[(x, y)].bg,
            Color::Reset,
            "the selection carries a background"
        );
        assert_eq!(
            painted[(x + 1, y)].symbol(),
            " ",
            "the wide glyph's trailing cell is reset to default"
        );

        // (A) Closing the popup: the glyph was replaced by emptiness → upstream sends the tail itself.
        let closed = Buffer::empty(area);
        assert!(
            painted
                .diff(&closed)
                .iter()
                .any(|&(ux, uy, _)| (ux, uy) == (x + 1, y)),
            "ratatui ≥ 0.1.2 must send the tail of a disappeared styled glyph"
        );

        // (B) Shifting the selection: the glyph stays put and stays wide — the tail won't arrive.
        let moved = render(&EmojiPickerState::with_selected(1));
        assert_eq!(
            moved[(x, y)].bg,
            Color::Reset,
            "the cell lost the selection background"
        );
        assert!(
            !painted
                .diff(&moved)
                .iter()
                .any(|&(ux, uy, _)| (ux, uy) == (x + 1, y)),
            "upstream closed the selection-shift case too — the full redraw can be removed"
        );

        // What a full redraw (the buffer sentinel, as in `app/runtime`) gives us:
        let tail_after_full_redraw = |next: &Buffer| {
            let mut sentinel = painted.clone();
            crate::shared::ui::prime_full_redraw(&mut sentinel);
            sentinel
                .diff(next)
                .iter()
                .any(|&(ux, uy, _)| (ux, uy) == (x + 1, y))
        };
        // on close — it recovers the tail (including for NON-styled glyphs, which
        // upstream doesn't send): a safety net on top of the 0.1.2 fix;
        assert!(
            tail_after_full_redraw(&closed),
            "a full redraw must rewrite the trailing cell on close"
        );
        // on a shift — it does NOT recover it: the glyph stayed wide, and the sentinel
        // isn't allowed to write into the second half of a wide glyph (the backend would print it
        // without `MoveTo` and shift the row — ratatui#2651). So for a shift a full redraw adds
        // nothing over the regular diff: the background is removed by reprinting the glyph
        // itself in cell `x`, which arrives anyway.
        assert!(
            !tail_after_full_redraw(&moved),
            "the sentinel doesn't write into the second half of a wide glyph (ratatui#2651)"
        );
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let s = EmojiPickerState::new();
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f, f.area(), &Palette::default(), ru()))
                .unwrap();
        }
    }
}
