//! `shared` layer (FSD): small reusable TUI rendering helpers, independent of
//! the upper layers.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Clear, List, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

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

/// A list's scroll position, kept **between frames**.
///
/// ratatui moves a list's offset only as far as it must to bring the selection
/// into view, so a `ListState` built inside a `render` function recomputes the
/// offset from zero on every draw — which reads as "scroll until the selection
/// is the *last* visible row". Downward that looks like a window correctly
/// following the selection; upward the list scrolls on every press and the
/// selection never walks up to the top row (docs/lessons.md §5). The offset is
/// state, not decoration: the widget owns one of these and calls
/// [`ListScroll::render`] instead of building a `ListState` itself.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ListScroll {
    offset: usize,
}

impl ListScroll {
    /// The first visible row of the last draw — what [`render_scrollbar`] takes
    /// as its `position`.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Draws `list` into `area` with the selection on `selected` (`None` — no
    /// selection), carrying the scroll position over from the previous frame.
    ///
    /// `len` — the number of items, `view_h` — the rows the list actually draws
    /// into (`area.height` minus the block's borders, when it has one). The two
    /// clamp the stored offset to the list's tail before the draw, so a list
    /// that has shrunk under it (a filter, a deletion) cannot leave the window
    /// hanging past its end. For items taller than one row the clamp is merely
    /// conservative — ratatui still scrolls down to the selection.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        list: List<'_>,
        area: Rect,
        len: usize,
        view_h: usize,
        selected: Option<usize>,
    ) {
        self.offset = self.offset.min(len.saturating_sub(view_h));
        let mut state = ListState::default().with_offset(self.offset);
        if len > 0
            && let Some(sel) = selected
        {
            state.select(Some(sel.min(len - 1)));
        }
        frame.render_stateful_widget(list, area, &mut state);
        self.offset = state.offset();
    }
}

/// Keeps `selected` inside a `view`-row window starting at `scroll`: the
/// offset moves only as far as it must, in either direction, so the selection
/// can walk to the top row as well as the bottom one. For screens that draw
/// their rows through a `Paragraph` rather than a `List` — the changes and
/// tasks screens — where [`ListScroll`] does not apply.
pub fn keep_visible(scroll: usize, selected: usize, view: usize) -> usize {
    if view == 0 {
        return 0;
    }
    if selected < scroll {
        selected
    } else if selected >= scroll + view {
        selected + 1 - view
    } else {
        scroll
    }
}

/// Gap between hotkey-grid columns, and between the chat bar's status pill and
/// the grid beside it.
pub const HINT_GAP: usize = 3;

/// One hint's cell width in columns: the keycap (the label plus the two spaces
/// [`Palette::keycap`] wraps it in), a space, and the description.
pub fn hint_cell_width(key: &str, desc: &str) -> usize {
    str_width(key) + 2 + 1 + str_width(desc)
}

/// The cell widths of a whole hint list, in its display order.
pub fn hint_cell_widths(items: &[(&str, &str, bool)]) -> Vec<usize> {
    items
        .iter()
        .map(|(key, desc, _)| hint_cell_width(key, desc))
        .collect()
}

/// Places `cell_w.len()` cells on a grid of `cols` columns: they fill
/// row-by-row, left-to-right/top-to-bottom, but **an incomplete bottom row is
/// right-aligned** — its cells take the rightmost columns, under the full rows
/// above. Returns `grid[row][col] = Some(cell index)`, the column widths (the
/// max over the cells actually placed in each column) and the block's total
/// width (columns + gaps).
///
/// The right-aligned bottom row is what makes a wrapped hint land exactly under
/// the column above it rather than as a floating group — see spec §11.1 and the
/// journal entry *status bar — pill top-left, hotkey grid right*.
pub fn hint_grid_layout(
    cell_w: &[usize],
    cols: usize,
) -> (Vec<Vec<Option<usize>>>, Vec<usize>, usize) {
    let n = cell_w.len();
    let rows = n.div_ceil(cols);
    let full = (rows - 1) * cols; // cells in full rows
    let empty_lead = cols - (n - full); // empty columns at the start of the bottom row

    let mut grid = vec![vec![None; cols]; rows];
    for i in 0..n {
        let (r, c) = if i < full {
            (i / cols, i % cols)
        } else {
            (rows - 1, empty_lead + (i - full))
        };
        grid[r][c] = Some(i);
    }

    let mut colw = vec![0usize; cols];
    for row in &grid {
        for (c, cell) in row.iter().enumerate() {
            if let Some(i) = cell {
                colw[c] = colw[c].max(cell_w[*i]);
            }
        }
    }
    let block_w = colw.iter().sum::<usize>() + HINT_GAP * cols.saturating_sub(1);
    (grid, colw, block_w)
}

/// The most columns whose grid fits into `avail` (→ the fewest rows); `None`
/// when not even a single column does.
pub fn widest_hint_grid(cell_w: &[usize], avail: usize) -> Option<usize> {
    (1..=cell_w.len())
        .rev()
        .find(|&c| hint_grid_layout(cell_w, c).2 <= avail)
}

/// Everything a hint block needs beyond its items: how wide the strip is, how
/// many columns to use, what shares its top row, and which cell is the one
/// highlighted mode light.
pub struct HintGrid<'a> {
    /// Hints in display order: key, description, and whether the key is
    /// "dangerous" (a red keycap — delete and friends).
    pub items: &'a [(&'a str, &'a str, bool)],
    /// Each item's cell width ([`hint_cell_widths`]) — passed in because the
    /// caller has usually computed it already to choose `cols`.
    pub cell_w: &'a [usize],
    /// Columns to lay the block out in ([`widest_hint_grid`], or the chat bar's
    /// own capped choice).
    pub cols: usize,
    /// The strip's full width; the block hugs its right edge.
    pub width: usize,
    /// Spans to put at the **left of the top row** — the chat bar's status
    /// pill. `None` on the screens, whose footers are hints and nothing else.
    pub lead: Option<Vec<Span<'static>>>,
    /// The one cell whose description is drawn in `accent` instead of muted —
    /// the chat bar's mouse-mode light. `None` elsewhere.
    pub accent: Option<usize>,
}

/// Draws a hint block right-aligned to `grid.width`: columns line up
/// vertically, an incomplete bottom row lands under the columns above, and
/// `grid.lead` (when given) opens the top row on the left.
///
/// The one hint-grid renderer in the application — every footer and the chat
/// bar's corner block come out of here, so they cannot drift apart in
/// alignment, spacing or keycap styling (docs/history/status-hints-unified.md §2.1).
pub fn render_hint_grid(grid: &HintGrid, palette: &Palette) -> Vec<Line<'static>> {
    if grid.items.is_empty() || grid.cols == 0 {
        return Vec::new();
    }
    let (cells, colw, block_w) = hint_grid_layout(grid.cell_w, grid.cols);
    let lead_w = grid.width.saturating_sub(block_w);
    let mut lead = grid.lead.clone();

    let mut out: Vec<Line<'static>> = Vec::new();
    for (r, row) in cells.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        push_row_lead(&mut spans, r, &mut lead, lead_w);
        // A cell is padded out to its column's width (+ a gap, except in the
        // last column); an empty column is all spaces, so columns line up.
        for (c, cell) in row.iter().enumerate() {
            let gap = if c + 1 < grid.cols { HINT_GAP } else { 0 };
            match cell {
                Some(i) => push_hint_cell(&mut spans, grid, palette, *i, colw[c], gap),
                None => spans.push(Span::raw(" ".repeat(colw[c] + gap))),
            }
        }
        out.push(Line::from(spans));
    }
    out
}

/// One occupied grid cell: the hint — its description accent-highlighted when
/// this is the block's mode light, its keycap red when the key is "dangerous" —
/// followed by padding out to `col_w` plus the trailing `gap`. A column is
/// never narrower than the cells in it, so the padding cannot underflow.
fn push_hint_cell(
    spans: &mut Vec<Span<'static>>,
    grid: &HintGrid,
    palette: &Palette,
    i: usize,
    col_w: usize,
    gap: usize,
) {
    let (key, desc, danger) = grid.items[i];
    if grid.accent == Some(i) {
        spans.extend(palette.hint_highlight_value(key, desc, palette.accent));
    } else {
        spans.extend(palette.hint_marked(key, desc, danger));
    }
    let pad = col_w.saturating_sub(grid.cell_w[i]) + gap;
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
}

/// The left margin of grid row `r`: on the top row — the lead cluster (taken
/// out of `lead`), everywhere else — spaces, since the block is right-aligned.
fn push_row_lead(
    spans: &mut Vec<Span<'static>>,
    r: usize,
    lead: &mut Option<Vec<Span<'static>>>,
    lead_w: usize,
) {
    if r == 0
        && let Some(pill) = lead.take()
    {
        let pill_w: usize = pill.iter().map(|s| str_width(&s.content)).sum();
        spans.extend(pill);
        if lead_w > pill_w {
            spans.push(Span::raw(" ".repeat(lead_w - pill_w)));
        }
    } else if lead_w > 0 {
        spans.push(Span::raw(" ".repeat(lead_w)));
    }
}

/// A screen's footer: hints laid out **right-aligned** into the widest grid
/// that fits `width`, wrapping to as many rows as they need. Empty for an empty
/// list.
///
/// Unlike the chat bar's corner block, a footer never sheds a hint: it has the
/// whole width and no status pill competing for it, so it grows a row instead
/// (user's decision, 2026-08-31 — docs/history/status-hints-unified.md §2.1).
pub fn hotkey_grid(
    palette: &Palette,
    items: &[(&str, &str, bool)],
    width: usize,
) -> Vec<Line<'static>> {
    if items.is_empty() {
        return Vec::new();
    }
    let cell_w = hint_cell_widths(items);
    let cols = widest_hint_grid(&cell_w, width).unwrap_or(1);
    render_hint_grid(
        &HintGrid {
            items,
            cell_w: &cell_w,
            cols,
            width,
            lead: None,
            accent: None,
        },
        palette,
    )
}

/// The visible width of a string in terminal columns.
fn str_width(s: &str) -> usize {
    crate::shared::wrap::display_width(&s.chars().collect::<Vec<_>>())
}

/// What [`screen_chrome`] hands back: where the content goes, where the hotkey
/// grid goes, and the grid itself.
pub struct ScreenChrome {
    /// Inside the panel's border — where the screen draws its own content.
    pub inner: Rect,
    /// The panel itself, border included. A screen that draws a scrollbar
    /// **on** the right border (`render_scrollbar`) measures it from here.
    pub panel: Rect,
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
/// sibling of an existing one (docs/lessons.md §2). The search, self-model and
/// settings screens joined it when their footers moved onto the one hint grid
/// (docs/history/status-hints-unified.md).
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
    let hotkeys = hotkey_grid(palette, hk, area.width as usize);
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
        panel: panel_area,
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

    /// A hint block's rows as plain strings.
    fn grid_text(items: &[(&str, &str, bool)], width: usize) -> Vec<String> {
        hotkey_grid(&Palette::default(), items, width)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Every footer in the application is right-aligned and wraps rather than
    /// sheds — the one property the screens share with the chat bar's corner
    /// block (docs/history/status-hints-unified.md §2.1).
    #[test]
    fn a_footer_is_right_aligned_and_wraps() {
        let items: &[(&str, &str, bool)] = &[
            ("Tab", "section", false),
            ("↑↓", "fields", false),
            ("Enter", "edit", false),
            ("Esc", "back", false),
            ("Ctrl+Q", "quit", false),
        ];
        // Wide — one row, flush against the right edge.
        let wide = grid_text(items, 200);
        assert_eq!(wide.len(), 1);
        assert_eq!(
            wide[0].chars().count(),
            200,
            "the row is padded out to the width"
        );
        assert!(wide[0].trim_end().ends_with("quit"), "{:?}", wide[0]);
        assert!(wide[0].starts_with("    "), "left margin: {:?}", wide[0]);
        // Narrow — more rows, and every hint is still there: a footer never sheds.
        let narrow = grid_text(items, 24);
        assert!(narrow.len() > 1);
        let all = narrow.join("");
        for (key, _, _) in items {
            assert!(all.contains(key), "{key} was shed: {narrow:?}");
        }
        // An empty set — no rows at all (the caller reserves a minimum height).
        assert!(hotkey_grid(&Palette::default(), &[], 80).is_empty());
        // A width no single cell fits into still lays out (one column, clipped
        // by the terminal) rather than panicking — `render` is called on every
        // frame, including the first one on a 1-column pane.
        for w in [0usize, 1, 2] {
            assert_eq!(grid_text(items, w).len(), items.len());
        }
    }

    /// An incomplete bottom row takes the **rightmost** columns, so a wrapped
    /// hint lands exactly under the column above it instead of reading as a
    /// floating group. Asserted in character positions, on the rendered rows.
    #[test]
    fn a_wrapped_row_lands_under_the_columns_above() {
        // Four equal-width cells at a width that fits three per row.
        let items: &[(&str, &str, bool)] = &[
            ("K1", "aaaa", false),
            ("K2", "bbbb", false),
            ("K3", "cccc", false),
            ("K4", "dddd", false),
        ];
        let cell = hint_cell_width("K1", "aaaa");
        let width = cell * 3 + HINT_GAP * 2;
        let rows = grid_text(items, width);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(
            rows[1].find("K4"),
            rows[0].find("K3"),
            "the wrapped cell sits under the last column: {rows:?}"
        );
        assert!(
            rows[1][..rows[1].find("K4").unwrap()].trim().is_empty(),
            "everything left of the wrapped cell is padding: {rows:?}"
        );
        assert_eq!(rows[1].chars().count(), width);
    }

    /// The layout is the same one the chat bar's corner block uses, so the two
    /// cannot drift: the block hugs the right edge at every column count.
    #[test]
    fn the_block_width_accounts_for_every_column_and_gap() {
        let cell_w = vec![10, 12, 8, 14, 9];
        for cols in 1..=cell_w.len() {
            let (grid, colw, block_w) = hint_grid_layout(&cell_w, cols);
            assert_eq!(grid[0].len(), cols);
            assert_eq!(
                block_w,
                colw.iter().sum::<usize>() + HINT_GAP * (cols - 1),
                "cols={cols}"
            );
            // Every cell is placed exactly once.
            let mut seen: Vec<usize> = grid.iter().flatten().flatten().copied().collect();
            seen.sort_unstable();
            assert_eq!(seen, (0..cell_w.len()).collect::<Vec<_>>(), "cols={cols}");
        }
        // The widest grid that fits is the one with the fewest rows.
        assert_eq!(widest_hint_grid(&cell_w, 1000), Some(5));
        assert_eq!(widest_hint_grid(&cell_w, 5), None);
    }

    /// A "dangerous" key is drawn in the danger color; its neighbour is not.
    #[test]
    fn a_dangerous_key_gets_a_red_keycap() {
        let p = Palette::default();
        let items: &[(&str, &str, bool)] = &[("Del", "delete", true), ("Esc", "back", false)];
        let line = &hotkey_grid(&p, items, 80)[0];
        let keycap = |key: &str| {
            line.spans
                .iter()
                .find(|s| s.content.trim() == key)
                .unwrap_or_else(|| panic!("no {key} keycap"))
                .style
                .fg
        };
        assert_eq!(keycap("Del"), Some(p.keycap_danger));
        assert_eq!(keycap("Esc"), Some(p.keycap_fg));
    }

    /// Symbols of the buffer's right column (the scrollbar lives there).
    fn right_column(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| buf[(area.right() - 1, y)].symbol().to_string())
            .collect()
    }

    /// One draw of `len` single-row items into an `h`-row area, with the
    /// selection on `selected`.
    fn draw(scroll: &mut ListScroll, len: usize, selected: usize, h: u16) {
        let mut term = Terminal::new(TestBackend::new(12, h)).unwrap();
        term.draw(|f| {
            let items: Vec<ratatui::widgets::ListItem> =
                (0..len).map(|i| format!("row {i}").into()).collect();
            let area = f.area();
            scroll.render(f, List::new(items), area, len, h as usize, Some(selected));
        })
        .unwrap();
    }

    /// The window follows the selection instead of being dragged by it: the
    /// selection crosses the visible rows first, and only a selection that has
    /// left the window moves it — the same in both directions. Guarding the
    /// helper guards every list built on it (docs/lessons.md §5).
    #[test]
    fn the_window_moves_only_when_the_selection_leaves_it() {
        let mut scroll = ListScroll::default();
        // Landing on the last item scrolls it to the bottom row.
        draw(&mut scroll, 30, 29, 10);
        assert_eq!(scroll.offset(), 20);
        // Moving up inside the window leaves the rows exactly where they were.
        draw(&mut scroll, 30, 28, 10);
        assert_eq!(scroll.offset(), 20);
        draw(&mut scroll, 30, 20, 10);
        assert_eq!(scroll.offset(), 20, "the selection is on the top row");
        // Past the top row — now, and only now, the list scrolls, by one row.
        draw(&mut scroll, 30, 19, 10);
        assert_eq!(scroll.offset(), 19);
        // Symmetrically at the other edge: down through the window, then a row.
        draw(&mut scroll, 30, 28, 10);
        assert_eq!(scroll.offset(), 19);
        draw(&mut scroll, 30, 29, 10);
        assert_eq!(scroll.offset(), 20);
    }

    /// A list that has shrunk under the stored offset (a filter, a deletion)
    /// must not leave the window hanging past its new tail — a couple of rows
    /// at the top and empty space below.
    #[test]
    fn a_shrunken_list_pulls_the_window_back_to_its_tail() {
        let mut scroll = ListScroll::default();
        draw(&mut scroll, 30, 29, 10);
        assert_eq!(scroll.offset(), 20);
        draw(&mut scroll, 12, 11, 10);
        assert_eq!(scroll.offset(), 2, "the last ten of twelve rows");
        // Shorter than the window — everything is visible from the first row.
        draw(&mut scroll, 4, 3, 10);
        assert_eq!(scroll.offset(), 0);
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
