//! `shared` layer (FSD): small reusable TUI rendering helpers, independent of
//! the upper layers.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};

use crate::shared::theme::Palette;

/// Dims the whole screen (the `DIM` modifier on every buffer cell), so a popup
/// drawn on top doesn't blend into the background. Call **before**
/// `Clear`+rendering the popup: `Clear` then resets the popup's cells to the
/// default (non-dimmed) style, so only the background gets dimmed, while the
/// popup itself stays bright.
///
/// The `DIM` effect is terminal-dependent (Windows Terminal supports it,
/// conhost Windows 10 doesn't), so in compatibility mode (`palette.compat`,
/// spec §11.6) the background is dimmed **by color**: every cell's fg →
/// `palette.muted` (plus `BOLD` is dropped — in a 16-color mapping it gives a
/// "bright" variant and would cancel out the dimming).
pub fn dim_background(frame: &mut Frame, palette: &Palette) {
    let area = frame.area();
    let compat = palette.compat;
    let muted = palette.muted;
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if compat {
                cell.fg = muted;
                cell.modifier.remove(Modifier::BOLD);
            } else {
                cell.modifier |= Modifier::DIM;
            }
        }
    }
}

/// Primes the buffer for a **full redraw** of the next frame: makes every cell
/// provably different from whatever the frame will draw, so ratatui's
/// per-cell diff rewrites the whole screen — including spaces in empty spots —
/// and clears "hanging" terminal artifacts.
///
/// Called on a buffer that then moves into the back buffer via
/// `swap_buffers()` **without a screen flush**: the marker itself never
/// reaches the terminal, it's only a diff base. We don't use a plain clear
/// (`terminal.clear()`) — it sends `ESC[2J`, and the screen blanks for a
/// moment (flicker).
///
/// **Why a space + `HIDDEN`, not a placeholder character.** The marker used to
/// be the character `"\0"`, but that broke rows with VS16 emoji (`🗂️`,
/// `❤️`): for such a cluster `ratatui` **additionally sends its trailing
/// cell** (a workaround for terminals that don't clear the second half of a
/// wide glyph), but **only if its symbol changed** — and `"\0"` always changed
/// it. The `crossterm` backend tracks position by cell number, without
/// accounting for glyph width (`x == last.x + 1` → no `MoveTo`), so such a
/// trailing cell printed one column to the right and shifted the rest of the
/// row: the next wide glyph's right half got overwritten (the terminal wiped
/// it out entirely), the panel border drifted outward.
///
/// A space matches the content of the trailing cell (`ratatui` resets it to
/// default), so such cells don't land in the diff — and no shift occurs.
/// Everything else differs by the [`SENTINEL_MODIFIER`] modifier, which never
/// occurs in the interface (pinned by a test), so redraw completeness isn't
/// compromised.
pub fn prime_full_redraw(buf: &mut Buffer) {
    for cell in buf.content.iter_mut() {
        cell.set_symbol(" ");
        cell.modifier = SENTINEL_MODIFIER;
    }
}

/// Marker modifier for [`prime_full_redraw`]: unused in the interface (the
/// palette and widgets get by with `DIM`/`BOLD`/`ITALIC`/`UNDERLINED`/
/// `REVERSED`), so no cell of a real frame will ever match it.
const SENTINEL_MODIFIER: Modifier = Modifier::HIDDEN;

/// Draws a vertical scrollbar in the **right column** of `area` when the
/// content doesn't fit by height (`total > viewport`); otherwise — a no-op
/// (the bar isn't drawn, so it doesn't clutter short content). `total` —
/// total content rows, `viewport` — visible ones, `position` — the first
/// visible row (the caller scrolls the content itself; the bar is a pure
/// indicator).
///
/// Panels with a border pass an area with a vertical margin of 1
/// (`area.inner(Margin::new(0, 1))`): the bar lands **on the right border
/// line**, without touching its corners and without taking width away from
/// the content. `focused` — which color the border under the bar is drawn in
/// (normal/focused): both the track and the thumb are drawn in that color, so
/// the scrollbar blends with the border, differing only by the `█` fill
/// (thumb) versus the thin `│` (track). The thumb follows the border's color
/// — a border-palette change automatically recolors it too.
pub fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    total: usize,
    viewport: usize,
    position: usize,
    focused: bool,
    palette: &Palette,
) {
    if total <= viewport || area.width == 0 || area.height == 0 {
        return;
    }
    // `content_length` is the number of scroll POSITIONS (total − viewport +
    // 1), not rows: ratatui places the thumb's bottom at the end of the track
    // at position `content_length − 1`, i.e. exactly at full scroll (total −
    // viewport). With `content_length = total` the thumb wouldn't reach the
    // bottom.
    let mut state = ScrollbarState::new(total - viewport + 1)
        .position(position)
        .viewport_content_length(viewport);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .track_style(palette.border_style(focused))
        .thumb_style(palette.border_style(focused));
    frame.render_stateful_widget(bar, area, &mut state);
}

/// What [`screen_chrome`] hands back: where the content goes, where the hotkey
/// grid goes, and the grid itself.
pub struct ScreenChrome {
    /// Inside the panel's border — where the screen draws its own content.
    pub inner: Rect,
    /// The strip below the panel, for [`Self::hotkeys`] or a warning line.
    pub status: Rect,
    pub hotkeys: Vec<Line<'static>>,
}

/// Draws the chrome a full-screen screen opens with: a hotkey grid pinned to the
/// bottom, and a titled panel filling everything above it.
///
/// Six lines that every screen here repeats verbatim — build the grid, size the
/// status row from it, split vertically, build the panel, take its `inner`,
/// render it. The duplication gate found the pair when the changes screen became
/// the fourth; this is the seam the rule says to build when a new thing is a
/// sibling of an existing one (docs/lessons.md §2). The three older screens keep
/// their own copies for now: a mechanical refactor does not share a PR with a
/// feature, and they are this function's obvious next callers.
///
/// `right` is the muted, right-aligned title some screens carry (a match count,
/// a summary); the caller still renders `hotkeys` into `status`, because a screen
/// with a confirmation to show puts that there instead.
pub fn screen_chrome(
    frame: &mut Frame,
    palette: &Palette,
    title: String,
    right: Option<String>,
    hk: &[(&str, &str, bool)],
) -> ScreenChrome {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let hotkeys = palette.hotkey_grid(hk, area.width as usize);
    let status_h = (hotkeys.len() as u16).max(1);
    let [panel_area, status] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(status_h)]).areas(area);
    let mut block = palette.panel(title, true);
    if let Some(right) = right {
        block = block.title(
            Line::from(Span::styled(format!(" {right} "), palette.muted_style())).right_aligned(),
        );
    }
    let inner = block.inner(panel_area);
    frame.render_widget(block, panel_area);
    ScreenChrome {
        inner,
        status,
        hotkeys,
    }
}

/// A modal confirmation: a centred, bordered box with a question and a footer
/// naming the keys that answer it.
///
/// One renderer rather than one per screen. The chat screen has asked
/// destructive questions since the dangerous-tool track (spec §9.8) and the
/// changes screen asks the same shape of question about a revert; `screens` are
/// siblings, so the second one could not have reached the first's without a
/// copy. The **keys** stay with each caller — what confirms differs (`Enter`
/// there, `Enter` here, `Enter`/`A`/`Esc` for a tool call) and only the drawing
/// is common.
pub fn confirm_popup(
    frame: &mut Frame,
    palette: &Palette,
    title: &str,
    question: &str,
    footer: &str,
) {
    // Wide enough to read, never wider than the terminal; three rows of border
    // and text, plus one for a wrapped second line.
    let width = 56u16.min(frame.area().width);
    let area = centered_rect(width, 5, frame.area());
    frame.render_widget(Clear, area);
    let block = palette.panel(title.to_string(), true).title_bottom(
        Line::from(Span::styled(footer.to_string(), palette.muted_style())).centered(),
    );
    let body = Paragraph::new(Line::from(Span::styled(
        question.to_string(),
        Style::new().fg(palette.text),
    )))
    .block(block)
    .wrap(Wrap { trim: true });
    frame.render_widget(body, area);
}

/// A fixed-width/height rectangle centred in `area` (clamped).
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Symbols of the buffer's right column (the scrollbar lives there).
    fn right_column(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| buf[(area.right() - 1, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn scrollbar_renders_thumb_only_on_overflow() {
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(10, 6)).unwrap();
        // Content fits (total <= viewport) → the bar isn't drawn.
        term.draw(|f| render_scrollbar(f, f.area(), 6, 6, 0, false, &palette))
            .unwrap();
        assert!(right_column(&term).iter().all(|s| s != "█" && s != "│"));
        // Overflow → the thumb and track appear in the right column.
        term.draw(|f| render_scrollbar(f, f.area(), 24, 6, 0, false, &palette))
            .unwrap();
        let col = right_column(&term);
        assert!(col.iter().any(|s| s == "█"), "no thumb: {col:?}");
        assert!(col.iter().any(|s| s == "│"), "no track: {col:?}");
    }

    #[test]
    fn scrollbar_thumb_tracks_position() {
        // At the start the thumb is at the top, at full scroll — at the bottom.
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(4, 8)).unwrap();
        let thumb_rows = |term: &Terminal<TestBackend>| -> Vec<usize> {
            right_column(term)
                .iter()
                .enumerate()
                .filter(|(_, s)| *s == "█")
                .map(|(i, _)| i)
                .collect()
        };
        term.draw(|f| render_scrollbar(f, f.area(), 32, 8, 0, false, &palette))
            .unwrap();
        let top = thumb_rows(&term);
        assert_eq!(
            top.first(),
            Some(&0),
            "thumb should be at the top initially: {top:?}"
        );
        // Full scroll: position = total - viewport.
        term.draw(|f| render_scrollbar(f, f.area(), 32, 8, 24, false, &palette))
            .unwrap();
        let bottom = thumb_rows(&term);
        assert_eq!(
            bottom.last(),
            Some(&7),
            "thumb should be at the bottom at full scroll: {bottom:?}"
        );
    }

    #[test]
    fn scrollbar_thumb_uses_border_color() {
        // The thumb is drawn in the border color (like the track), not the
        // text color. In the Auto palette `border` (DarkGray) and `text`
        // (Reset) differ, so the check is meaningful.
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(4, 8)).unwrap();
        term.draw(|f| render_scrollbar(f, f.area(), 32, 8, 0, false, &palette))
            .unwrap();
        let buf = term.backend().buffer();
        let area = buf.area;
        let thumb_fg = (area.top()..area.bottom())
            .map(|y| &buf[(area.right() - 1, y)])
            .find(|c| c.symbol() == "█")
            .map(|c| c.fg);
        assert_eq!(
            thumb_fg,
            Some(palette.border),
            "thumb should be border-colored"
        );
    }

    #[test]
    fn scrollbar_zero_area_is_noop() {
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(4, 4)).unwrap();
        term.draw(|f| {
            let zero = Rect::new(0, 0, 0, 0);
            render_scrollbar(f, zero, 10, 2, 0, false, &palette);
        })
        .unwrap();
    }

    #[test]
    fn prime_full_redraw_repaints_all_but_wide_glyph_tails() {
        // A gate on the sentinel's design (regression: "row drifts right on VS16").
        // Frame: a VS16 emoji with text after it — as in the feed.
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Paragraph, Widget};
        use unicode_width::UnicodeWidthStr;

        let area = Rect::new(0, 0, 20, 2);
        let mut frame = Buffer::empty(area);
        Paragraph::new(Line::from(vec![Span::styled(
            "a 🗂\u{FE0F} хвост",
            Style::new().fg(ratatui::style::Color::Blue),
        )]))
        .render(area, &mut frame);

        let mut sentinel = frame.clone();
        prime_full_redraw(&mut sentinel);
        let updates = sentinel.diff(&frame);

        // (1) No update targets the second half of a wide glyph — otherwise
        // the backend would print it with no `MoveTo` and shift the rest of
        // the row.
        for &(x, y, _) in &updates {
            if x == 0 {
                continue;
            }
            let left = frame[(x - 1, y)].symbol();
            assert!(
                left.width() < 2,
                "update at ({x},{y}) targets the second half of glyph {left:?}"
            );
        }

        // (2) Everything else is repainted: the only uncovered cells are
        // trailing halves of wide glyphs (the glyph itself covers them) — no
        // gaps in the repaint.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if updates.iter().any(|&(ux, uy, _)| (ux, uy) == (x, y)) {
                    continue;
                }
                let is_tail = x > 0 && frame[(x - 1, y)].symbol().width() == 2;
                assert!(is_tail, "cell ({x},{y}) is neither repainted nor a tail");
            }
        }
    }

    #[test]
    fn screen_switch_emits_vs16_tail_without_full_redraw() {
        // Why a screen switch must go through a full redraw (`app/runtime`).
        //
        // For a VS16 cluster (`❤️` = U+2764 U+FE0F) ratatui accompanies it with a
        // trailing cell, sent WHEN ITS SYMBOL CHANGED. Within a single screen,
        // feed edits don't touch the tail (there was a space — a space it
        // stays), but on returning from another screen a foreign symbol sat in
        // its place → the tail goes out to the terminal. The backend tracks
        // position by cell number, without accounting for glyph width (open
        // ratatui#2651): no `MoveTo` is emitted after a wide glyph, and the
        // tail prints one column to the right — the rest of the row drifts
        // right. Symptom: an extra space after `❤️` when returning from the
        // chat list / `F3`, which disappears on scroll (it requests the same
        // full redraw).
        //
        // The sentinel makes the tail's symbol a space, i.e. equal to what the
        // frame will draw — the tail doesn't land in the diff and the row
        // doesn't drift.
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Paragraph, Widget};

        let area = Rect::new(0, 0, 20, 1);
        let mut feed = Buffer::empty(area);
        Paragraph::new(Line::from(vec![Span::styled(
            "a \u{2764}\u{FE0F} tail",
            Style::new(),
        )]))
        .render(area, &mut feed);
        let gx = (0..area.width)
            .find(|&x| feed[(x, 0)].symbol().contains('\u{FE0F}'))
            .expect("VS16 glyph not found in the frame");

        // Returning from another screen: a foreign symbol sat in the tail's place.
        let mut other = Buffer::empty(area);
        Paragraph::new(Line::from("chat list content!!")).render(area, &mut other);
        assert!(
            other
                .diff(&feed)
                .iter()
                .any(|&(x, y, _)| (x, y) == (gx + 1, 0)),
            "a screen switch sends the VS16 tail — without a full redraw the row will drift"
        );

        // An edit within the same screen doesn't send the tail (which is why the
        // bug was only visible on switching, not during normal feed work).
        let mut same = feed.clone();
        same[(area.width - 1, 0)].set_symbol("Z");
        assert!(
            !same
                .diff(&feed)
                .iter()
                .any(|&(x, y, _)| (x, y) == (gx + 1, 0)),
            "an edit within the screen doesn't touch the VS16 tail"
        );

        // Full redraw: the tail isn't emitted → no shift.
        let mut sentinel = feed.clone();
        prime_full_redraw(&mut sentinel);
        assert!(
            !sentinel
                .diff(&feed)
                .iter()
                .any(|&(x, y, _)| (x, y) == (gx + 1, 0)),
            "the sentinel must not send the VS16 tail (otherwise it would shift the row itself)"
        );
    }

    #[test]
    fn sentinel_modifier_is_unused_by_ui() {
        // The marker must not occur in real frames, otherwise a matching cell
        // wouldn't land in the diff and would stay unpainted. The palette gets
        // by with DIM/BOLD/ITALIC/UNDERLINED/REVERSED — check the marker isn't
        // one of them.
        for used in [
            Modifier::DIM,
            Modifier::BOLD,
            Modifier::ITALIC,
            Modifier::UNDERLINED,
            Modifier::REVERSED,
        ] {
            assert!(
                !SENTINEL_MODIFIER.intersects(used),
                "sentinel marker intersects a modifier used in the UI {used:?}"
            );
        }
    }

    #[test]
    fn dim_background_uses_color_in_compat_mode() {
        let mut term = Terminal::new(TestBackend::new(4, 2)).unwrap();
        // Normal mode — the DIM modifier on cells.
        term.draw(|f| {
            dim_background(f, &Palette::default());
            assert!(f.buffer_mut()[(0, 0)].modifier.contains(Modifier::DIM));
        })
        .unwrap();
        // Compatibility mode — a muted color instead of DIM (conhost can't do it).
        let compat = Palette::default().with_compat(true);
        term.draw(|f| {
            dim_background(f, &compat);
            let cell = f.buffer_mut()[(0, 0)].clone();
            assert_eq!(cell.fg, compat.muted);
            assert!(!cell.modifier.contains(Modifier::DIM));
        })
        .unwrap();
    }
}
