//! `shared` layer (FSD): small reusable TUI rendering helpers, independent of
//! the upper layers.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Clear, HighlightSpacing, List, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};

use crate::shared::i18n::Locale;
use crate::shared::theme::{MONO_MARK, Palette};

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

/// Puts the palette's canvas under a **finished** frame — the full colour mode
/// (spec §11.6, docs/history/theme-modes.md §4.2). Every cell whose background is
/// still the terminal's default gets the canvas, and every cell whose
/// foreground is gets the palette's text colour; a colour a widget set is left
/// exactly as it is. In the system mode the palette has no canvas and nothing
/// is touched.
///
/// Call it **last** — after the screen and after any overlay. That is the
/// point of doing this as a pass rather than as a base style under every
/// widget: a popup's `Clear` resets its cells to the terminal's default, the
/// logo and highlighted code never ask the palette, and the next widget
/// somebody writes will not remember either. Whatever reached the buffer
/// without a colour is caught here.
///
/// Attributes are not read: `REVERSED` over a filled cell swaps the canvas and
/// the text colour, which is what it did with the terminal's own two.
pub fn paint_canvas(buf: &mut Buffer, palette: &Palette) {
    let canvas = palette.canvas;
    if canvas == Color::Reset {
        return;
    }
    for cell in buf.content.iter_mut() {
        if cell.bg == Color::Reset {
            cell.bg = canvas;
        }
        if cell.fg == Color::Reset {
            cell.fg = palette.text;
        }
    }
}

/// Takes every colour and every text attribute off a **finished** frame — the
/// monochrome mode (spec §11.6, docs/history/theme-modes.md §9). What is left is the
/// terminal's own two colours, and reverse video on the cells a widget drew
/// on [`MONO_MARK`]: a text selection and a search match, the two things with
/// no glyph to fall back on. In any other mode nothing is touched.
///
/// A pass for the reason [`paint_canvas`] is one, only more so: attributes
/// never went through the palette at all — widgets set bold, dim, italic,
/// underline and reverse directly — and the logo and highlighted code bring
/// colours of their own. Stripping the frame is the one way that cannot miss
/// any of them, including the ones not written yet.
///
/// Reverse video is opt-in **by name**: a popup's `reversed()` row and an
/// unhighlighted code block lose theirs here like everything else. Which is
/// why the pass is only half of the mode — where styling was the only thing
/// saying something, the widget has to say it with a glyph
/// ([`Palette::mono`]).
pub fn strip_styles(buf: &mut Buffer, palette: &Palette) {
    if !palette.mono {
        return;
    }
    for cell in buf.content.iter_mut() {
        let marked = cell.bg == MONO_MARK;
        cell.fg = Color::Reset;
        cell.bg = Color::Reset;
        cell.underline_color = Color::Reset;
        cell.modifier = if marked {
            Modifier::REVERSED
        } else {
            Modifier::empty()
        };
    }
}

/// The passes over a finished frame, in one call: the canvas of the full
/// mode, the stripping of the monochrome one. Each is a no-op outside its
/// mode, and a palette is never in both. Call it **last** — after the screen
/// and after any overlay.
pub fn finish_frame(buf: &mut Buffer, palette: &Palette) {
    paint_canvas(buf, palette);
    strip_styles(buf, palette);
}

/// Gives a list the marker of its selected row where the mode needs one
/// ([`Palette::selected_mark`]): the rows that say which one is selected by
/// styling alone — a `highlight_style` and nothing else — say it with a
/// glyph in the monochrome mode. Every row gives up the marker's width, so
/// the list does not shift as the selection moves.
pub fn mark_selected<'a>(list: List<'a>, palette: &Palette) -> List<'a> {
    match palette.selected_mark() {
        Some(mark) => list
            .highlight_symbol(mark)
            .highlight_spacing(HighlightSpacing::Always),
        None => list,
    }
}

/// Erase the whole screen (`ED 2`). Terminals erase **with the current
/// background** — which is the whole reason [`set_background`] exists.
pub const ERASE_SCREEN: &str = "\x1b[2J";

/// Back to the terminal's default colours and attributes (`SGR 0`). ratatui's
/// backend assumes exactly that state at the start of every draw, so nothing
/// that sets a colour by hand may leave it set.
pub const RESET_STYLE: &str = "\x1b[0m";

/// The sequence that makes `canvas` the terminal's current background, or
/// nothing for `Color::Reset` (the terminal's own is already current).
///
/// Used once per change of canvas, around an erase: a cell the app never
/// writes keeps the colour the last erase gave it, and there is such a cell
/// behind every wide glyph — ratatui resets a wide glyph's trailing cell and
/// leaves it out of the diff, so a terminal that does not give it the
/// glyph's colours itself shows whatever was there. Windows 11's console host
/// does give them, measured (docs/history/theme-modes.md §8); this is the guard for
/// one that does not.
///
/// Built here and written by the caller, like `shared/osc52.rs`: that is what
/// lets a test assert the exact bytes.
pub fn set_background(canvas: Color) -> String {
    match canvas {
        Color::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        // A canvas is absolute or absent (`Palette::canvas`); anything else
        // has no business being painted, and saying nothing is the safe half.
        _ => String::new(),
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

/// How long one spinner glyph stays up — and so how often an otherwise idle
/// screen writes to the terminal while a spinner runs, which is the number
/// that matters. Windows Terminal re-finds the URLs it detects in the text
/// only once its output has been quiet for 100 ms (`ControlCore`'s
/// `outputIdle`, a debounce). A spinner repainting every 50 ms tick never let
/// it: every detected link stayed on the row it held before the layout last
/// moved, so the indexing banner's own row pushed the feed up, and hovering
/// the line under a link underlined it and read its text as the URI
/// (`Invalid URI`). With the loop's 50 ms tick the frames land 150–250 ms
/// apart — a quiet gap longer than the debounce after each of them
/// (docs/journal/ui-feed.md, "the spinner leaves the terminal a quiet gap").
pub const SPINNER_STEP: Duration = Duration::from_millis(200);

/// A spinner's clock: the glyph it shows is a function of the time since it
/// started, not of the frames drawn, and the loop repaints for it only when
/// that glyph changes ([`Spinner::due`]). A frame counter got both wrong: the
/// spin sped up with every frame something else asked for (typing, a stream),
/// and the loop had to draw on every tick to move it at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spinner {
    started: Instant,
    /// The step the last frame showed (`None` — not drawn yet).
    drawn: Option<u128>,
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Spinner {
    /// A spinner starting now, on its first glyph.
    pub fn new() -> Self {
        Self::started_at(Instant::now())
    }

    fn started_at(started: Instant) -> Self {
        Self {
            started,
            drawn: None,
        }
    }

    /// Whole [`SPINNER_STEP`]s from the start to `now`.
    fn step_at(&self, now: Instant) -> u128 {
        now.saturating_duration_since(self.started).as_millis() / SPINNER_STEP.as_millis()
    }

    /// Whether a frame drawn now would show another glyph than the last one
    /// did — or none has been drawn yet.
    pub fn due(&self) -> bool {
        self.due_at(Instant::now())
    }

    fn due_at(&self, now: Instant) -> bool {
        self.drawn != Some(self.step_at(now))
    }

    /// The glyph of `frames` to draw now, recorded as drawn.
    pub fn glyph(&mut self, frames: &[char]) -> char {
        self.glyph_at(frames, Instant::now())
    }

    fn glyph_at(&mut self, frames: &[char], now: Instant) -> char {
        let step = self.step_at(now);
        self.drawn = Some(step);
        frames[(step % frames.len() as u128) as usize]
    }
}

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
/// One shape rather than one per screen. The chat screen has asked
/// destructive questions since the dangerous-tool track (spec §9.8) and the
/// changes screen asks the same shape of question about a revert; `screens` are
/// siblings, so the second one could not have reached the first's without a
/// copy. The **keys** stay with each caller — what confirms differs (`Enter`
/// there, `Enter` here, `Enter`/`A`/`Esc` for a tool call) and only the drawing
/// is common.
///
/// A [`Prompt`] rather than a drawing call, because a screen with the question
/// open also has to say what window it needs ([`Prompt::min_size`]).
pub fn confirm_prompt(palette: &Palette, title: &str, question: &str, footer: &str) -> Prompt {
    Prompt {
        title: title.to_string(),
        body: vec![Line::from(Span::styled(
            question.to_string(),
            Style::new().fg(palette.text),
        ))],
        legend: footer.to_string(),
        // Wide enough to read, never wider than the terminal; three rows of
        // border and text, plus one for a wrapped second line.
        max_width: 56,
        min_rows: 5,
        trim: true,
    }
}

/// The narrowest window a modal question is asked in: below it the wrap is a
/// word a row, and the placeholder says more ([`render_too_small`]).
pub const MODAL_MIN_WIDTH: u16 = 20;

/// A modal question: a centred, bordered box holding what is asked and the
/// keys that answer it (spec §9.8, §11.1).
///
/// **The keys are never clipped.** They sit on the bottom border while they
/// fit it; a box narrower than its legend carries it as the body's last lines
/// instead, wrapped like the rest. A border title that does not fit is cut at
/// the corner with no mark, and what fell off the tool confirmation in a
/// 57-column window was the key that runs the call
/// (docs/research/small-terminal.md §2.5).
///
/// The box is as tall as its wrapped text ([`Self::rows`]) — and a window that
/// cannot hold it whole is not one the question is asked in: the screen
/// reports [`Self::min_size`], and the runtime draws the placeholder rather
/// than a question with its subject or its keys cut off.
pub struct Prompt {
    pub title: String,
    pub body: Vec<Line<'static>>,
    /// The keys, as a locale writes them for a border — padded with a space
    /// either side, which is dropped when they move into the body.
    pub legend: String,
    /// The box's width where the window has room for it.
    pub max_width: u16,
    /// The box's height when its text needs less (borders included).
    pub min_rows: u16,
    /// Whether wrapped rows drop their leading whitespace. A question does;
    /// code being approved keeps its indentation.
    pub trim: bool,
}

impl Prompt {
    /// The box's width in a window `window_width` columns wide.
    fn width(&self, window_width: u16) -> u16 {
        self.max_width.min(window_width)
    }

    /// The box as a paragraph `width` columns wide, borders included.
    fn paragraph(&self, palette: &Palette, width: u16) -> Paragraph<'static> {
        let mut block = palette.panel(self.title.clone(), true);
        let mut body = self.body.clone();
        if legend_width(&self.legend) <= width {
            block = block.title_bottom(
                Line::from(Span::styled(self.legend.clone(), palette.muted_style())).centered(),
            );
        } else {
            body.push(Line::from(Span::styled(
                self.legend.trim().to_string(),
                palette.muted_style(),
            )));
        }
        Paragraph::new(body)
            .block(block)
            .wrap(Wrap { trim: self.trim })
    }

    /// The rows the box takes at `width`, borders included. `line_count` is
    /// ratatui's own, over ratatui's own word wrapping: counting by hand would
    /// be a second implementation of the wrap whose only job is to agree with
    /// the first, and every disagreement is a row of the question nobody saw.
    fn rows(&self, palette: &Palette, width: u16) -> u16 {
        let rows = self
            .paragraph(palette, width)
            .line_count(width.saturating_sub(2));
        u16::try_from(rows).unwrap_or(u16::MAX).max(self.min_rows)
    }

    /// The smallest window the question can be shown whole in, at the width
    /// the window `area` has (a narrower box wraps into more rows).
    pub fn min_size(&self, palette: &Palette, area: Rect) -> MinSize {
        let width = self.width(area.width.max(MODAL_MIN_WIDTH));
        MinSize::new(MODAL_MIN_WIDTH, self.rows(palette, width))
    }

    /// Draws the box centred in the frame.
    pub fn render(&self, frame: &mut Frame, palette: &Palette) {
        let full = frame.area();
        let width = self.width(full.width);
        let area = centered_rect(width, self.rows(palette, width), full);
        frame.render_widget(Clear, area);
        frame.render_widget(self.paragraph(palette, width), area);
    }
}

/// The columns a bordered popup needs to carry `legend` whole on its border:
/// the text and the two corners.
pub fn legend_width(legend: &str) -> u16 {
    u16::try_from(str_width(legend) + 2).unwrap_or(u16::MAX)
}

/// The smallest window a layer — a screen, or a popup over it — is drawn in
/// (spec §11.1.1, docs/research/small-terminal.md F4). Below it the runtime
/// draws [`render_too_small`] instead of a frame with parts missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinSize {
    pub width: u16,
    pub height: u16,
}

impl MinSize {
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    /// The size that holds both layers: a popup is drawn over its screen, so
    /// the window has to be enough for either.
    pub fn max(self, other: Self) -> Self {
        Self::new(self.width.max(other.width), self.height.max(other.height))
    }

    /// Whether a window of `area` is at least this size.
    pub fn fits(self, area: Rect) -> bool {
        area.width >= self.width && area.height >= self.height
    }
}

/// The minimum of a full-screen panel with a list in it — the self-model,
/// the message search, the tasks: the border, five rows of the list, and a
/// row of key hints; thirty columns of a row.
pub const SCREEN_MIN_SIZE: MinSize = MinSize::new(30, 8);

/// `width×height`, the way the placeholder names a size.
fn size_label(width: u16, height: u16) -> String {
    format!("{width}×{height}")
}

/// The keys that work under the placeholder (spec §11.1.1). Everything else
/// is dropped: the screen a key was meant for is not the one on the terminal,
/// so `Enter` would answer a question nobody read and a typed line would be
/// sent unseen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WayOut {
    /// Quit, and nothing else: the window is too small for the screen itself,
    /// and the way back to it is the window's edge.
    Quit,
    /// `Esc` as well: something is open **over** the screen — a popup, the
    /// help, a screen over the chat — and it may be all that does not fit.
    /// `Esc` is the key that closes, declines or goes back, never the one
    /// that confirms or sends, and without it a picker opened in a window the
    /// chat fits and the picker does not could only be left by resizing or by
    /// quitting.
    EscOrQuit,
}

/// The placeholder's lines for a window `area`, top to bottom: what is wrong,
/// the size the window has against the one it needs, and the keys that work.
/// A line that does not fit the width is left out rather than cut — the size
/// falls back to the needed one alone, the keys to `Esc` alone — and when the
/// rows run short the size outlives the title, which outlives the keys.
fn too_small_lines(
    area: Rect,
    need: MinSize,
    way_out: WayOut,
    loc: &'static Locale,
) -> Vec<(TooSmallLine, String)> {
    let have = size_label(area.width, area.height);
    let want = size_label(need.width, need.height);
    let fits = |s: &String| str_width(s) <= area.width as usize;
    let size = vec![
        loc.tf("ui.too_small.size", &[("have", &have), ("need", &want)]),
        want.clone(),
    ];
    let (esc, quit) = (loc.t("ui.too_small.esc"), loc.t("ui.too_small.quit"));
    let keys = match way_out {
        WayOut::Quit => vec![quit.to_string()],
        WayOut::EscOrQuit => vec![format!("{esc} · {quit}"), esc.to_string()],
    };
    // In keeping order; drawn in the order of the enum.
    let mut lines: Vec<(TooSmallLine, String)> = [
        (TooSmallLine::Size, size),
        (
            TooSmallLine::Title,
            vec![loc.t("ui.too_small.title").to_string()],
        ),
        (TooSmallLine::Keys, keys),
    ]
    .into_iter()
    .filter_map(|(kind, forms)| forms.into_iter().find(fits).map(|text| (kind, text)))
    .take(area.height as usize)
    .collect();
    lines.sort_by_key(|(kind, _)| *kind);
    lines
}

/// The placeholder's lines, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TooSmallLine {
    Title,
    Size,
    Keys,
}

/// Draws the "window too small" notice over the whole frame: the window is
/// smaller than the layer in front needs (`need`), and a frame with parts
/// missing would be drawn otherwise (spec §11.1.1). `way_out` — the keys that
/// work while it is up, which its last line names.
pub fn render_too_small(
    frame: &mut Frame,
    palette: &Palette,
    loc: &'static Locale,
    need: MinSize,
    way_out: WayOut,
) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let lines: Vec<Line> = too_small_lines(area, need, way_out, loc)
        .into_iter()
        .map(|(kind, text)| {
            let style = match kind {
                TooSmallLine::Title => Style::new().fg(palette.text).add_modifier(Modifier::BOLD),
                TooSmallLine::Size | TooSmallLine::Keys => palette.muted_style(),
            };
            Line::from(Span::styled(text, style)).centered()
        })
        .collect();
    let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let [middle] = Layout::vertical([Constraint::Length(rows)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Paragraph::new(lines), middle);
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
pub(crate) mod tests {
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

    /// A buffer as the rows of text it shows — for the tests that read what a
    /// widget drew, here and in the widgets.
    pub(crate) fn buffer_rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| buf[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// A small frame holding one of everything the pass has to tell apart: bare
    /// text, a role colour, a selection backdrop, reverse video, a wide glyph
    /// and empty cells.
    fn mixed_frame(palette: &Palette) -> Buffer {
        use ratatui::widgets::Widget;
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        Paragraph::new(vec![
            Line::from(vec![
                Span::raw("bare "),
                Span::styled("role", Style::new().fg(palette.user)),
            ]),
            Line::from(vec![
                Span::styled("sel", Style::new().bg(palette.keycap_bg)),
                Span::styled(" rev", Style::new().add_modifier(Modifier::REVERSED)),
            ]),
            Line::from("a 😀 b"),
        ])
        .render(area, &mut buf);
        buf
    }

    #[test]
    fn the_system_mode_leaves_the_frame_alone() {
        // The regression guard for the default: no canvas, no pass — the
        // buffer is what the widgets drew, cell for cell.
        for theme in [
            crate::shared::config::Theme::Auto,
            crate::shared::config::Theme::Dark,
            crate::shared::config::Theme::Light,
        ] {
            let palette = Palette::for_theme(theme);
            let drawn = mixed_frame(&palette);
            let mut painted = drawn.clone();
            paint_canvas(&mut painted, &palette);
            assert_eq!(painted, drawn, "{theme:?}");
        }
    }

    #[test]
    fn the_full_mode_fills_what_the_widgets_left_at_the_default() {
        for name in crate::shared::theme::FULL_THEMES {
            let palette = Palette::full(name);
            let drawn = mixed_frame(&palette);
            let mut painted = drawn.clone();
            paint_canvas(&mut painted, &palette);

            for (before, after) in drawn.content.iter().zip(&painted.content) {
                // Nothing is left for the terminal to colour — the empty cells
                // and a wide glyph's trailing cell included.
                assert_ne!(after.bg, Color::Reset, "{name}: {:?}", after.symbol());
                assert_ne!(after.fg, Color::Reset, "{name}: {:?}", after.symbol());
                // A colour a widget chose is kept; only the default is filled.
                let want_bg = if before.bg == Color::Reset {
                    palette.canvas
                } else {
                    before.bg
                };
                let want_fg = if before.fg == Color::Reset {
                    palette.text
                } else {
                    before.fg
                };
                assert_eq!((after.fg, after.bg), (want_fg, want_bg), "{name}");
                // The text and its attributes are none of the pass's business.
                assert_eq!(after.symbol(), before.symbol(), "{name}");
                assert_eq!(after.modifier, before.modifier, "{name}");
            }
            // The three cases by name, so a vacuous loop cannot pass for them.
            let cell = |x: u16, y: u16| &painted[(x, y)];
            assert_eq!(cell(0, 0).fg, palette.text, "{name}: bare text");
            assert_eq!(cell(5, 0).fg, palette.user, "{name}: a role colour");
            assert_eq!(cell(0, 1).bg, palette.keycap_bg, "{name}: the backdrop");
            assert!(cell(4, 1).modifier.contains(Modifier::REVERSED), "{name}");
            assert_eq!(cell(11, 2).bg, palette.canvas, "{name}: an empty cell");
        }
    }

    #[test]
    fn a_popup_cleared_over_the_frame_gets_the_canvas_back() {
        // The case a base style under the widgets would have missed: `Clear`
        // resets its area to the terminal's default, and the popup drawn into
        // it would sit on the terminal's background in the middle of the
        // canvas. The pass runs after it.
        use ratatui::widgets::{Block, Widget};
        let palette = Palette::full("light");
        let mut buf = mixed_frame(&palette);
        paint_canvas(&mut buf, &palette);
        let popup = Rect::new(2, 0, 6, 3);
        Clear.render(popup, &mut buf);
        Block::bordered().render(popup, &mut buf);
        assert_eq!(buf[(3, 1)].bg, Color::Reset, "the premise: Clear resets");
        paint_canvas(&mut buf, &palette);
        assert!(buf.content.iter().all(|c| c.bg != Color::Reset));
    }

    #[test]
    fn the_monochrome_pass_leaves_nothing_but_the_mark() {
        use crate::shared::theme::MONO_MARK;
        use ratatui::widgets::Widget;
        let palette = Palette::mono();
        let mut buf = mixed_frame(&palette);
        // What the two marks draw, on top of whatever styling was under them
        // — and an attribute the frame above does not have.
        Paragraph::new(Line::from(vec![
            Span::styled(
                "m",
                palette.search_match(Style::new().bold().fg(Color::Red)),
            ),
            Span::styled("s", palette.selection(Style::new().underlined())),
            Span::styled("u", Style::new().underlined().underline_color(Color::Red)),
        ]))
        .render(Rect::new(8, 2, 3, 1), &mut buf);
        let drawn = buf.clone();
        assert_eq!(drawn[(8, 2)].bg, MONO_MARK, "the premise: a mark is drawn");
        assert!(drawn[(4, 1)].modifier.contains(Modifier::REVERSED));

        strip_styles(&mut buf, &palette);

        for (before, after) in drawn.content.iter().zip(&buf.content) {
            let what = before.symbol();
            assert_eq!(after.symbol(), what, "the text is not the pass's");
            assert_eq!(after.fg, Color::Reset, "{what:?}");
            assert_eq!(after.bg, Color::Reset, "{what:?}");
            assert_eq!(after.underline_color, Color::Reset, "{what:?}");
            let want = if before.bg == MONO_MARK {
                Modifier::REVERSED
            } else {
                Modifier::empty()
            };
            assert_eq!(after.modifier, want, "{what:?}");
        }
        // By name, so a vacuous loop cannot pass for them: the two marks are
        // reverse video and nothing else…
        assert_eq!(buf[(8, 2)].modifier, Modifier::REVERSED, "a search match");
        assert_eq!(buf[(9, 2)].modifier, Modifier::REVERSED, "a selection");
        // …and reverse video a widget set by hand is gone with the rest.
        assert_eq!(buf[(4, 1)].modifier, Modifier::empty(), "a popup's row");
        assert_eq!(buf[(0, 1)].bg, Color::Reset, "the selection backdrop");
    }

    #[test]
    fn only_the_monochrome_mode_is_stripped() {
        let mut palettes = vec![Palette::default()];
        palettes.extend(crate::shared::theme::FULL_THEMES.map(Palette::full));
        for palette in palettes {
            let drawn = mixed_frame(&palette);
            let mut passed = drawn.clone();
            strip_styles(&mut passed, &palette);
            assert_eq!(passed, drawn);
            // The two passes in one call: what the canvas does, and no more.
            let mut finished = drawn.clone();
            finish_frame(&mut finished, &palette);
            let mut painted = drawn.clone();
            paint_canvas(&mut painted, &palette);
            assert_eq!(finished, painted);
        }
        let mono = Palette::mono();
        let mut finished = mixed_frame(&mono);
        finish_frame(&mut finished, &mono);
        assert!(
            finished
                .content
                .iter()
                .all(|c| c.fg == Color::Reset && c.bg == Color::Reset && c.modifier.is_empty()),
            "the monochrome palette paints no canvas and is stripped"
        );
    }

    #[test]
    fn a_popup_over_the_frame_is_stripped_with_it() {
        // The pass runs after the overlay, so a dimmed background and a
        // popup's own styling go the same way as the screen's.
        use ratatui::widgets::{Block, Widget};
        let palette = Palette::mono();
        let mut buf = mixed_frame(&palette);
        for cell in buf.content.iter_mut() {
            cell.modifier |= Modifier::DIM; // what `dim_background` does
        }
        let popup = Rect::new(2, 0, 6, 3);
        Clear.render(popup, &mut buf);
        Block::bordered()
            .border_style(palette.border_style(true))
            .render(popup, &mut buf);
        strip_styles(&mut buf, &palette);
        assert!(buf.content.iter().all(|c| c.modifier.is_empty()));
        assert!(buf.content.iter().all(|c| c.fg == Color::Reset));
        assert_eq!(buf[(2, 0)].symbol(), "┌", "the border is what is left");
    }

    #[test]
    fn a_list_gets_its_marker_only_in_the_monochrome_mode() {
        use ratatui::widgets::StatefulWidget;
        let rows = |palette: &Palette| -> Vec<String> {
            let area = Rect::new(0, 0, 8, 3);
            let mut buf = Buffer::empty(area);
            let list = mark_selected(
                List::new(["one", "two", "three"]).highlight_style(Style::new().reversed()),
                palette,
            );
            let mut state = ListState::default().with_selected(Some(1));
            StatefulWidget::render(list, area, &mut buf, &mut state);
            buffer_rows(&buf)
        };
        assert_eq!(
            rows(&Palette::default()),
            ["one     ", "two     ", "three   "]
        );
        // The rows do not shift as the selection moves: the unselected ones
        // are indented by the marker's width.
        assert_eq!(rows(&Palette::mono()), ["  one   ", "› two   ", "  three "]);
    }

    #[test]
    fn the_erase_sets_the_canvas_and_the_reset_gives_it_back() {
        // The exact bytes: 24-bit background, erase display, reset.
        let canvas = crate::shared::theme::CANVAS_DARK;
        assert_eq!(set_background(canvas), "\x1b[48;2;15;17;21m");
        assert_eq!(ERASE_SCREEN, "\x1b[2J");
        assert_eq!(RESET_STYLE, "\x1b[0m");
        // Going back to the system mode erases with the terminal's own
        // background, which is what is current when nothing was set.
        assert_eq!(set_background(Color::Reset), "");
        // A named colour is not a canvas: nothing is written rather than a
        // shade the terminal would pick.
        assert_eq!(set_background(Color::Blue), "");
    }

    const FRAMES: &[char] = &['a', 'b', 'c'];

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_spinner_steps_with_time_not_with_frames() {
        let t0 = Instant::now();
        let mut s = Spinner::started_at(t0);
        // However many frames are drawn within one step, they show one glyph —
        // a repaint someone else asked for does not speed the spin up.
        for _ in 0..5 {
            assert_eq!(s.glyph_at(FRAMES, t0), 'a');
        }
        assert_eq!(s.glyph_at(FRAMES, t0 + SPINNER_STEP - ms(1)), 'a');
        assert_eq!(s.glyph_at(FRAMES, t0 + SPINNER_STEP), 'b');
        assert_eq!(s.glyph_at(FRAMES, t0 + SPINNER_STEP * 2), 'c');
        assert_eq!(s.glyph_at(FRAMES, t0 + SPINNER_STEP * 3), 'a', "wraps");
    }

    #[test]
    fn a_spinner_is_due_only_when_its_glyph_would_change() {
        let t0 = Instant::now();
        let mut s = Spinner::started_at(t0);
        assert!(s.due_at(t0), "never drawn — the first frame is due");
        s.glyph_at(FRAMES, t0);
        // The idle ticks inside a step write nothing to the terminal.
        assert!(!s.due_at(t0 + ms(50)));
        assert!(!s.due_at(t0 + SPINNER_STEP - ms(1)));
        assert!(s.due_at(t0 + SPINNER_STEP), "the next glyph is due");
        // Drawn late in its step (the loop draws on its next tick), the glyph
        // is due again at the step's boundary, not a whole step after the draw.
        s.glyph_at(FRAMES, t0 + SPINNER_STEP + ms(40));
        assert!(!s.due_at(t0 + SPINNER_STEP * 2 - ms(1)));
        assert!(s.due_at(t0 + SPINNER_STEP * 2));
    }

    // ---------- small windows (spec §11.1.1) ----------

    use crate::shared::i18n::{Lang, locale};

    /// A question with a legend wider than a narrow box and a body of two
    /// lines, the second one indented like code.
    fn question() -> Prompt {
        Prompt {
            title: "Tool call".into(),
            body: vec![Line::raw("Run this call?"), Line::raw("    indented()")],
            legend: " Enter — run · A — allow for this turn · Esc — decline ".into(),
            max_width: 72,
            min_rows: 0,
            trim: false,
        }
    }

    /// Draws `prompt` alone in a `width`×`height` window and returns the rows.
    fn prompt_rows(prompt: &Prompt, width: u16, height: u16) -> Vec<String> {
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| prompt.render(f, &palette)).unwrap();
        buffer_rows(term.backend().buffer())
    }

    /// The rows between a box's borders, joined by spaces — a wrapped
    /// sentence reads as it was written.
    fn inner_text(rows: &[String]) -> String {
        rows.iter()
            .filter(|r| r.starts_with('│'))
            .map(|r| r.trim_matches('│').trim())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The keys that answer a question are on its border while the border
    /// holds them, and the last lines of its body when it does not — never
    /// cut at the corner. The width the legend needs, corners included, is
    /// the exact threshold: one column less and it moves.
    #[test]
    fn a_prompts_keys_are_on_the_border_or_in_the_body_never_cut() {
        let prompt = question();
        let legend = prompt.legend.trim();
        let needs = legend_width(&prompt.legend);
        assert_eq!(needs, 57, "the text and the two corners");

        let rows = prompt_rows(&prompt, needs, 12);
        assert_eq!(rows.len(), 12);
        let shown: Vec<&String> = rows.iter().filter(|r| !r.trim().is_empty()).collect();
        assert_eq!(shown.len(), 4, "border, two lines, border: {shown:?}");
        assert!(shown[3].contains(legend), "on the border: {shown:?}");
        assert!(!inner_text(&rows).contains("Enter"), "and not in the body");

        for width in (MODAL_MIN_WIDTH..needs).rev() {
            let rows = prompt_rows(&prompt, width, 20);
            let body = inner_text(&rows);
            assert!(
                body.ends_with(legend),
                "{width} columns: the keys close the body, whole — {body:?}"
            );
            let bottom = rows.iter().rfind(|r| r.starts_with('╰')).unwrap();
            assert!(!bottom.contains("Esc"), "{width}: not half on the border");
            assert!(rows.iter().any(|r| r.contains("    indented()")), "{width}");
        }
    }

    /// The box is as tall as its wrapped text, and that is what a screen
    /// reports as the window the question needs: a window one row shorter
    /// would cut the box, which is where a question stops being asked.
    #[test]
    fn a_prompts_minimum_is_the_rows_its_text_wraps_into() {
        let palette = Palette::default();
        let prompt = question();
        let mut last = 0;
        for width in [100, 72, 57, 40, 30, 20] {
            let need = prompt.min_size(&palette, Rect::new(0, 0, width, 50));
            assert_eq!(need.width, MODAL_MIN_WIDTH);
            let rows = prompt_rows(&prompt, width, 50);
            let drawn = rows.iter().filter(|r| !r.trim().is_empty()).count();
            assert_eq!(usize::from(need.height), drawn, "{width}: {rows:#?}");
            assert!(need.height >= last, "narrower is never shorter");
            last = need.height;
        }
        assert!(last > 4, "the control: twenty columns do wrap it");
        // A window narrower than any a question is asked in is measured at
        // the narrowest one that is — the size the placeholder then names.
        assert_eq!(
            prompt.min_size(&palette, Rect::new(0, 0, 5, 50)),
            prompt.min_size(&palette, Rect::new(0, 0, MODAL_MIN_WIDTH, 50)),
        );
    }

    /// A destructive question keeps the five rows it always had when its
    /// text needs fewer, and grows past them instead of cutting the text.
    #[test]
    fn a_confirmation_keeps_its_five_rows_and_grows_past_them() {
        let palette = Palette::default();
        let short = confirm_prompt(
            &palette,
            "Confirmation",
            "Delete it?",
            " Enter — yes · Esc — no ",
        );
        assert_eq!(short.min_size(&palette, Rect::new(0, 0, 80, 24)).height, 5);
        let long = confirm_prompt(
            &palette,
            "Confirmation",
            &"a question long enough to wrap ".repeat(8),
            " Enter — yes · Esc — no ",
        );
        let need = long.min_size(&palette, Rect::new(0, 0, 30, 24));
        assert!(need.height > 5, "{need:?}");
        let rows = prompt_rows(&long, 30, need.height);
        assert!(rows[0].starts_with('╭') && rows.last().unwrap().starts_with('╰'));
        assert!(rows.last().unwrap().contains("Enter — yes · Esc — no"));
    }

    #[test]
    fn a_minimum_holds_both_layers_and_fits_by_both_sides() {
        let (screen, popup) = (MinSize::new(20, 9), MinSize::new(46, 6));
        assert_eq!(screen.max(popup), MinSize::new(46, 9));
        let need = MinSize::new(46, 9);
        assert!(need.fits(Rect::new(0, 0, 46, 9)));
        assert!(!need.fits(Rect::new(0, 0, 45, 9)), "a column short");
        assert!(!need.fits(Rect::new(0, 0, 46, 8)), "a row short");
    }

    /// The placeholder, in every built-in language: the three lines where
    /// there is room, the size outliving the title and the title the key as
    /// the rows run out, and a line that does not fit left out — never cut.
    #[test]
    fn the_placeholder_says_what_it_can_in_the_room_it_has() {
        let need = MinSize::new(46, 12);
        for &lang in Lang::ALL {
            let loc = locale(lang);
            let lines = |w: u16, h: u16, way_out: WayOut| {
                too_small_lines(Rect::new(0, 0, w, h), need, way_out, loc)
            };
            let kinds = |w: u16, h: u16| -> Vec<TooSmallLine> {
                lines(w, h, WayOut::Quit)
                    .into_iter()
                    .map(|(kind, _)| kind)
                    .collect()
            };
            use TooSmallLine::{Keys, Size, Title};
            assert_eq!(kinds(45, 6), [Title, Size, Keys], "{lang:?}");
            assert_eq!(kinds(45, 2), [Title, Size], "{lang:?}");
            assert_eq!(kinds(45, 1), [Size], "{lang:?}");
            assert_eq!(kinds(45, 0), [], "{lang:?}");
            // Every line is within the width it was given, at any width.
            for w in 0..=60u16 {
                for way_out in [WayOut::Quit, WayOut::EscOrQuit] {
                    for (_, text) in lines(w, 3, way_out) {
                        assert!(str_width(&text) <= w as usize, "{lang:?} {w}: {text:?}");
                    }
                }
            }
            // Too narrow for the sentence, the size is the needed one alone…
            let narrow = lines(8, 1, WayOut::Quit);
            assert_eq!(narrow, [(Size, "46×12".to_string())], "{lang:?}");
            // …and too narrow for that, there is nothing to say.
            assert!(lines(4, 3, WayOut::Quit).is_empty());

            let full = lines(45, 6, WayOut::Quit);
            assert!(full[1].1.contains("45×6") && full[1].1.contains("46×12"));
            // The last line names the keys that work, and only those.
            assert!(
                full[2].1.contains("Ctrl+Q") && !full[2].1.contains("Esc"),
                "{full:?}"
            );
            let both = lines(45, 6, WayOut::EscOrQuit);
            assert!(
                both[2].1.contains("Esc") && both[2].1.contains("Ctrl+Q"),
                "{both:?}"
            );
            // Too narrow for both keys: the one that leads back, not the one
            // that ends the session.
            let esc = lines(20, 6, WayOut::EscOrQuit);
            let keys = esc.iter().find(|(kind, _)| *kind == Keys).unwrap();
            assert_eq!(keys.1, loc.t("ui.too_small.esc"), "{lang:?}");
        }
    }

    /// Drawn: the whole frame is the notice — what stood there is cleared —
    /// its lines are centred, and no size is too small to draw it in.
    #[test]
    fn the_placeholder_takes_the_whole_frame_at_any_size() {
        let palette = Palette::default();
        let loc = locale(Lang::En);
        let need = MinSize::new(46, 12);
        let draw = |w: u16, h: u16| -> Vec<String> {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| {
                let area = f.area();
                f.render_widget(Paragraph::new(vec![Line::raw("XXXXXXXXXXXX"); 40]), area);
                render_too_small(f, &palette, loc, need, WayOut::Quit);
            })
            .unwrap();
            buffer_rows(term.backend().buffer())
        };
        for (w, h) in [(0, 0), (1, 1), (3, 40), (200, 1), (45, 6), (120, 40)] {
            let rows = draw(w, h);
            assert!(rows.iter().all(|r| !r.contains('X')), "{w}×{h}: {rows:?}");
        }
        let rows = draw(45, 6);
        let title = rows
            .iter()
            .position(|r| r.contains("Window too small"))
            .unwrap();
        let blank_below = rows[title + 3..]
            .iter()
            .filter(|r| r.trim().is_empty())
            .count();
        assert!(
            title.abs_diff(blank_below) <= 1 && title + 3 + blank_below == 6,
            "three lines, centred in six rows: {rows:#?}"
        );
        let line = &rows[title];
        let (left, right) = (
            line.len() - line.trim_start().len(),
            line.len() - line.trim_end().len(),
        );
        assert!(left.abs_diff(right) <= 1, "centred: {line:?}");
    }
}
