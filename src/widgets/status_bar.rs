//! Status bar: server status, a generation indicator, a reply/context token
//! counter, key hints. See spec §11.1. Model/profile —
//! later.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::shared::i18n::Locale;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::theme::Palette;
use crate::shared::wrap;

/// The one hotkey whose description is not fixed: `Esc` means "go back", and
/// where back goes depends on how the chat was reached (see [`EscTarget`]).
/// Named so the substitution in [`lines`] isn't a bare string match.
const ESC_KEY: &str = "Esc";

/// The help hotkey — the one hint the corner block sheds last (see
/// [`keep_order`]) and the last-resort row when nothing fits beside the pill
/// ([`lines`]). Named like [`ESC_KEY`] so the lookup isn't a bare string match.
const HELP_KEY: &str = "F1";

/// Keys of the status bar's fixed hotkeys (after the mouse toggle `Ctrl+W`, whose
/// description depends on the mode). "Key" + a description key (localized in [`lines`]).
/// The `Esc` entry carries its **default** description; the live one comes from
/// [`StatusModel::esc_target`].
const HOTKEYS: [(&str, &str); 5] = [
    ("F1", "ui.status.hotkey.help"),
    (ESC_KEY, "ui.status.hotkey.chats"),
    ("Ctrl+N", "ui.status.hotkey.new"),
    ("Ctrl+P", "ui.status.hotkey.settings"),
    ("Ctrl+Q", "ui.status.hotkey.quit"),
];

/// Where `Esc` takes the user from the chat. The status bar is the only place
/// that answers this on screen, so the hint has to move with the key: a chat
/// opened from the message-search results goes **back to them** first, and a bar
/// still saying "chats" would be the old answer in exactly the flow where it
/// surprised someone.
///
/// The chat screen is told which one applies and renders the label; it learns
/// nothing about searching, and the back-stack itself lives in `app/runtime`
/// (FSD: `screens` may not depend on `app`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EscTarget {
    /// The chat list — the ordinary case.
    #[default]
    ChatList,
    /// The results the chat was opened from.
    SearchResults,
    /// The conversation a `chat://` reference was followed from (spec §11.3).
    PreviousChat,
}

impl EscTarget {
    /// The bundle key of the `Esc` description. `ChatList` deliberately returns
    /// the same key [`HOTKEYS`] carries, so the const stays honest as the default.
    fn hint_key(self) -> &'static str {
        match self {
            EscTarget::ChatList => "ui.status.hotkey.chats",
            EscTarget::SearchResults => "ui.status.hotkey.results",
            EscTarget::PreviousChat => "ui.status.hotkey.back_chat",
        }
    }
}

/// Gap between hotkey columns and between the status pill and the hotkey grid.
const GAP: usize = 3;

/// A state snapshot for the status line — the screen assembles it in one place
/// ([`ChatScreen::status_model`](crate::screens::chat::ChatScreen)), so a new
/// indicator adds a field rather than extending the `render`/`height` signatures.
/// `tokens` — reply tokens (live), `context` — conversation (prompt) tokens (`None`
/// — unknown), `context_exact` — whether this is the exact number from the server's `usage`
/// (otherwise an estimate, marked with `~`). `mouse_scroll` — whether mouse capture for
/// wheel scrolling is on (otherwise native text selection). `background` — a quiet indicator of
/// the running background tasks, already composed into one label by the caller
/// (auto-reflection, note/self-model consolidation, history compaction; `None` —
/// nothing running). See spec §11.1, §11.3.
pub struct StatusModel<'a> {
    pub statuses: &'a ServerStatuses,
    pub generating: bool,
    pub tokens: u64,
    pub context: Option<u64>,
    pub context_exact: bool,
    /// Reasoning tokens ("thoughts") in the reply (included in `tokens`); `0` — don't show.
    pub reasoning: u32,
    pub mouse_scroll: bool,
    pub background: Option<&'a str>,
    /// Speech synthesis is running (`/tts`) — a quiet "speaking" chip. See spec §11.9.
    pub speaking: bool,
    /// Files attached to the chat (`/file attach`) — a quiet chip with their
    /// count and standing token cost; `None` — nothing attached. See
    /// docs/file-attachments.md.
    pub attachments: Option<&'a str>,
    /// Images staged for the **next message** (`/image attach`) — a quiet chip
    /// with their count and token cost; `None` — nothing staged. A second chip
    /// rather than a line in the first: the two costs behave differently
    /// (an attachment is paid every turn, a staged image once, on the next
    /// send), and merging them would claim otherwise. See spec §9.10.
    pub staged_images: Option<&'a str>,
    /// Where `Esc` goes from here — it decides the `Esc` hint's wording.
    pub esc_target: EscTarget,
    /// The open "chat" is a sub-agent transcript, read-only (spec §11.2): a
    /// quiet chip says so, next to the input box's title that says the same.
    pub read_only: bool,
}

/// The speaking-indicator glyph (WGL4, width 1 column — the hotkey grid doesn't "shift").
const SPEAKING_GLYPH: char = '♪';

/// The attached-files glyph. Like `♪` it is WGL4 and one column wide, so it needs
/// no compat-mode replacement (an emoji paperclip would be two columns and would
/// shift the hotkey grid on legacy terminals).
const ATTACH_GLYPH: char = '§';

/// The staged-images glyph. Same constraint as [`ATTACH_GLYPH`]: WGL4 and
/// exactly one column, so it needs no compat-mode replacement and cannot shift
/// the hotkey grid (an emoji camera/picture would be two columns wide). ASCII
/// `#` reads as "a picture" next to `§` "a document" without borrowing either
/// glyph's meaning.
const IMAGE_GLYPH: char = '#';

/// The read-only-transcript glyph. WGL4, one column, like the three above;
/// `≡` reads as "a document" without borrowing a neighbour's meaning.
const TRANSCRIPT_GLYPH: char = '≡';

/// Draws the status line in `area`. The hotkeys sit beside the status pill as
/// a right-aligned corner grid at most [`HINT_ROWS_MAX`] rows deep — a crowded
/// pill sheds hints rather than growing the bar (see [`lines`]); the caller
/// reserves height via [`height`]. See spec §11.1, §11.3.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    model: &StatusModel,
    palette: &Palette,
    loc: &'static Locale,
) {
    let lines = lines(area.width as usize, model, palette, loc);
    frame.render_widget(Paragraph::new(lines), area);
}

/// How many lines the status bar takes at width `width` — the caller reserves
/// exactly this height (minimum 1). Never more than [`HINT_ROWS_MAX`]: the
/// corner grid is capped, and the degenerate fallback is one `F1` row.
pub fn height(width: usize, model: &StatusModel, palette: &Palette, loc: &'static Locale) -> u16 {
    lines(width, model, palette, loc)
        .len()
        .clamp(1, u16::MAX as usize) as u16
}

/// Lays hotkeys out in a neat grid, **right-aligned** to `width` — the
/// settings screen's layout: with no status pill to crowd it, and unlike the
/// chat bar's corner block ([`lines`]), it wraps to as many rows as it needs
/// and never sheds an entry. Column count — the max that fits the width (→ fewest rows);
/// on overflow hotkeys wrap down, columns line up vertically, and an
/// incomplete bottom row is right-aligned under the columns above. Returns empty for
/// an empty set. Used by the settings screen (see spec §11.1, §11.6).
pub fn hotkey_lines(
    width: usize,
    hotkeys: &[(&str, &str)],
    palette: &Palette,
) -> Vec<Line<'static>> {
    if hotkeys.is_empty() {
        return Vec::new();
    }
    // Cell width as in `lines`: "key" (+2 for padding) + a space + the description.
    let cell_w: Vec<usize> = hotkeys
        .iter()
        .map(|(key, desc)| str_w(key) + 2 + 1 + str_w(desc))
        .collect();
    let cols = widest_grid(&cell_w, width).unwrap_or(1);
    right_grid(hotkeys, &cell_w, cols, width, None, None, palette)
}

/// Builds the status bar's lines. On the left of the **top** row — "state"
/// (the status pill: server chips, generation indicator, token counter, the
/// quiet chips), left-aligned. On the right — hotkeys (starting with the mouse
/// toggle `Ctrl+W`, whose description = the current mode), laid out as a neat
/// right-aligned grid **beside the pill**, at most [`HINT_ROWS_MAX`] rows
/// deep, columns lined up vertically and an incomplete bottom row landing
/// exactly under the columns above.
///
/// The grid never moves below the pill and never grows past that depth: when
/// the pill leaves it too little width, the block **sheds hints** instead
/// ([`trim_to_fit`], lowest [`keep_order`] rank first) — a busy pill costs
/// hints, not rows, so the hints always read as one corner block and the bar
/// never exceeds [`HINT_ROWS_MAX`] rows. Before this rule the bar had a second
/// arrangement — the grid on a full-width line below the pill — and the pill
/// growing every turn (generation chip, token counter) teleported the hints
/// between the two, planting a wall of keycaps right under the indicators
/// (user's decision, 2026-08-30). Every hidden hint is still on the `F1` help
/// list, which is why `F1` sheds last; in the degenerate case where not even
/// it fits beside the pill, it alone takes a row below — the sole remnant of
/// the old layout. See spec §11.1, §11.3.
fn lines(
    width: usize,
    model: &StatusModel,
    palette: &Palette,
    loc: &'static Locale,
) -> Vec<Line<'static>> {
    let state = state_spans(model, palette, loc);
    let state_w = spans_width(&state);

    let hotkeys = hotkey_list(model, loc);
    // In "scroll" mode, highlight the mouse toggle's description (index 0) with the
    // `accent` color — the same one that highlights markdown headings in the feed.
    let accent_idx = model.mouse_scroll.then_some(0usize);

    // Each cell's width: "key" (+2 for padding) + a space + the description.
    let cell_w: Vec<usize> = hotkeys
        .iter()
        .map(|(key, desc)| str_w(key) + 2 + 1 + str_w(desc))
        .collect();

    let avail = width.saturating_sub(state_w + GAP);
    let kept = trim_to_fit(&cell_w, avail, model.mouse_scroll);
    if !kept.is_empty() {
        let sub: Vec<(&str, &str)> = kept.iter().map(|&i| hotkeys[i]).collect();
        let sub_w: Vec<usize> = kept.iter().map(|&i| cell_w[i]).collect();
        // The accent follows the mouse toggle into the kept sub-list — or is
        // dropped with it (it never is while anything is kept: scroll mode
        // pins the toggle at the top of `keep_order`).
        let accent = accent_idx.and_then(|a| kept.iter().position(|&i| i == a));
        let cols = corner_cols(&sub_w, avail).unwrap_or(1);
        return right_grid(&sub, &sub_w, cols, width, Some(state), accent, palette);
    }

    // Degenerate: the pill leaves no corner that could anchor on `F1`. The
    // help entry stays reachable — alone on a row of its own, right-aligned
    // (the sole remnant of the old grid-below-the-pill layout).
    debug_assert_eq!(hotkeys[HELP_IDX].0, HELP_KEY);
    let mut out = vec![Line::from(state)];
    if cell_w[HELP_IDX] <= width {
        out.extend(right_grid(
            &hotkeys[HELP_IDX..=HELP_IDX],
            &cell_w[HELP_IDX..=HELP_IDX],
            1,
            width,
            None,
            None,
            palette,
        ));
    }
    out
}

/// The most rows the chat bar's hint block may take. Two is the tallest block
/// that still reads as a corner legend — the reference look is the 3×2 grid
/// beside a quiet pill; deeper, and it becomes the one-hint-per-line column
/// the bar used to stack beside a busy pill. Past this depth the block sheds
/// hints ([`trim_to_fit`]) rather than growing.
const HINT_ROWS_MAX: usize = 2;

/// [`HELP_KEY`]'s index in [`hotkey_list`]'s order (right after the prepended
/// mouse toggle). `keep_order_matches_the_hotkey_list` pins the mapping.
const HELP_IDX: usize = 1;

/// The order the corner block **keeps** hints when the pill leaves it too
/// little width — later entries shed first. `F1` outlives everything: it opens
/// the full per-screen hotkey list every hidden hint is still on (spec §11.7).
/// `Esc` next — its label carries live navigation state ([`EscTarget`]). Then
/// quit, the mouse toggle, settings, new chat. In scroll mode the mouse toggle
/// instead jumps to the front: accent-highlighted, it is the one thing on
/// screen saying why native selection is off — a mode light, not a hint.
/// Values index [`hotkey_list`]'s order; `keep_order_matches_the_hotkey_list`
/// pins the mapping.
fn keep_order(mouse_pinned: bool) -> [usize; 6] {
    if mouse_pinned {
        [0, HELP_IDX, 2, 5, 4, 3]
    } else {
        [HELP_IDX, 2, 5, 0, 4, 3]
    }
}

/// The hints the corner block shows at `avail` columns: walk [`keep_order`]
/// and keep every hint whose addition still fits a ≤[`HINT_ROWS_MAX`]-row grid
/// (one that does not is passed over, so the narrower ones after it still
/// get their chance). Returned in display order; empty when nothing fits.
/// A block that lost `F1` is refused outright — a legend hiding its own door
/// to the full list while lesser hints stay would be an artifact of cell
/// widths, not a priority — and the caller falls back to the `F1`-below row.
/// Within a turn the pill only grows (the token counter), so hints shed
/// monotonically — no flicker — and return when the turn's chips leave.
fn trim_to_fit(cell_w: &[usize], avail: usize, mouse_pinned: bool) -> Vec<usize> {
    let mut kept: Vec<usize> = Vec::new();
    for idx in keep_order(mouse_pinned) {
        let mut candidate = kept.clone();
        candidate.push(idx);
        candidate.sort_unstable();
        let w: Vec<usize> = candidate.iter().map(|&i| cell_w[i]).collect();
        if corner_cols(&w, avail).is_some() {
            kept = candidate;
        }
    }
    if !kept.contains(&HELP_IDX) {
        return Vec::new();
    }
    kept
}

/// The most columns whose grid fits `avail` within [`HINT_ROWS_MAX`] rows (→
/// the fewest rows); `None` when no allowed column count fits. Unlike
/// [`widest_grid`] it refuses to go deeper instead of narrower — the chat bar
/// sheds hints at that point ([`trim_to_fit`]).
fn corner_cols(cell_w: &[usize], avail: usize) -> Option<usize> {
    (cell_w.len().div_ceil(HINT_ROWS_MAX)..=cell_w.len())
        .rev()
        .find(|&c| grid_layout(cell_w, c).2 <= avail)
}

/// The most columns whose grid fits into `avail` (→ the fewest rows); `None`
/// when not even a single column does.
fn widest_grid(cell_w: &[usize], avail: usize) -> Option<usize> {
    (1..=cell_w.len())
        .rev()
        .find(|&c| grid_layout(cell_w, c).2 <= avail)
}

/// The "state" cluster on the left of the top row: server chips + generation +
/// token counter + the quiet background/speaking/attachment chips.
///
/// The chat server is always shown (with the disconnect reason — it blocks
/// generation); embeddings and impersonation — separate chips, only when configured
/// (`NotConfigured`, including impersonation in `shared`, → the chip is hidden).
fn state_spans(model: &StatusModel, palette: &Palette, loc: &'static Locale) -> Vec<Span<'static>> {
    let statuses = model.statuses;
    let sep = || Span::styled("  │  ", Style::new().fg(palette.border));
    let muted = palette.muted_style();

    let mut state = chat_chip(&statuses.chat, palette, loc);
    for chip in [
        secondary_chip(loc.t("ui.status.chip.embed"), &statuses.embed, palette),
        secondary_chip(
            loc.t("ui.status.chip.imp"),
            &statuses.impersonation,
            palette,
        ),
    ]
    .into_iter()
    .flatten()
    {
        state.push(Span::raw("  ")); // gap between chips
        state.extend(chip);
    }

    if model.generating {
        state.push(sep());
        state.push(Span::styled(
            format!(
                "{} {}",
                palette.glyphs().busy,
                loc.t("ui.status.generating")
            ),
            palette.accent_style(),
        ));
    }
    if let Some(counter) = token_counter_span(model, palette, loc) {
        state.push(sep());
        state.push(counter);
    }
    // A quiet indicator of the running background tasks (composed by the caller) — muted,
    // the `✻` glyph (compat — `*`) width 1 column (the grid layout doesn't "shift").
    if let Some(hint) = model.background {
        state.push(sep());
        state.push(Span::styled(
            format!("{} {hint}", palette.glyphs().background),
            muted,
        ));
    }
    // A quiet speaking indicator — in the same muted style. The `♪` note (U+266A)
    // is in WGL4 and width 1 column, so it isn't replaced in compat mode
    // (like the feed's `▌` rails and the `█` scrollbar). See spec §11.9.
    if model.speaking {
        state.push(sep());
        state.push(Span::styled(
            format!("{SPEAKING_GLYPH} {}", loc.t("ui.status.speaking")),
            muted,
        ));
    }
    // Attached files (`/file attach`): a quiet chip — they are re-sent on every
    // turn, so their standing cost must be visible without opening anything.
    if let Some(files) = model.attachments {
        state.push(sep());
        state.push(Span::styled(format!("{ATTACH_GLYPH} {files}"), muted));
    }
    // Images staged for the next message (`/image attach`): the same quiet
    // style, its own chip — this cost is about to be paid once, by the send the
    // user is composing, and it disappears with it.
    if let Some(images) = model.staged_images {
        state.push(sep());
        state.push(Span::styled(format!("{IMAGE_GLYPH} {images}"), muted));
    }
    // A sub-agent transcript is read-only: the chip is the standing reminder,
    // the refusals say it again when a key is pressed.
    if model.read_only {
        state.push(sep());
        state.push(Span::styled(
            format!("{TRANSCRIPT_GLYPH} {}", loc.t("ui.status.read_only")),
            muted,
        ));
    }
    state
}

/// The combined token counter (conversation + reply): bright during generation (grows
/// live), muted afterward (the last turn's final result). Marked with `~` while the
/// conversation (prompt) is a client-side estimate, i.e. before the exact number arrives from the server's `usage`.
/// `None` — nothing to count yet (the counter is hidden).
fn token_counter_span(
    model: &StatusModel,
    palette: &Palette,
    loc: &'static Locale,
) -> Option<Span<'static>> {
    if model.tokens == 0 && model.context.is_none() {
        return None;
    }
    let total = model.context.unwrap_or(0) + model.tokens;
    let approx = if model.context.is_some() && !model.context_exact {
        "~"
    } else {
        ""
    };
    // Reasoning tokens ("thoughts") — a separate annotation, they're included in the total.
    let reason = if model.reasoning > 0 {
        loc.tf(
            "ui.status.reasoning",
            &[("n", &model.reasoning.to_string())],
        )
    } else {
        String::new()
    };
    let label = loc.tf(
        "ui.status.tokens",
        &[
            ("approx", approx),
            ("total", &total.to_string()),
            ("reason", &reason),
        ],
    );
    if model.generating {
        Some(Span::styled(label, palette.accent_style()))
    } else {
        Some(Span::styled(label, palette.muted_style()))
    }
}

/// Hotkeys: the mouse toggle `Ctrl+W` (description = the current mode) + the fixed ones.
fn hotkey_list(model: &StatusModel, loc: &'static Locale) -> Vec<(&'static str, &'static str)> {
    let mouse_desc = if model.mouse_scroll {
        loc.t("ui.status.mouse.scroll")
    } else {
        loc.t("ui.status.mouse.select")
    };
    let mut hotkeys: Vec<(&'static str, &'static str)> = vec![("Ctrl+W", mouse_desc)];
    hotkeys.extend(HOTKEYS.iter().map(|(key, default)| {
        // `Esc` is the one hint whose meaning moves with the state; the rest are
        // fixed. Keeping it in the const preserves one ordered list.
        let desc_key = if *key == ESC_KEY {
            model.esc_target.hint_key()
        } else {
            *default
        };
        (*key, loc.t(desc_key))
    }));
    hotkeys
}

/// The glyph and color of a server's status for the status line's chip. Glyphs are width 1
/// column (no emoji) — the line's layout doesn't "shift" because of them. Ready — `●` in both
/// sets (WGL4-safe); "connecting"/"no connection" in compat mode — `○`/`×`.
fn status_glyph(status: &ServerStatus, palette: &Palette) -> (&'static str, Color) {
    let glyphs = palette.glyphs();
    match status {
        ServerStatus::Ready => ("●", palette.success),
        ServerStatus::Connecting => (glyphs.status_connecting, palette.warning),
        ServerStatus::NotConfigured => (glyphs.status_off, palette.warning),
        ServerStatus::Disconnected(_) => (glyphs.status_off, palette.error),
    }
}

/// Builds a chip: a bold glyph + a label (with a leading space), both in the status color.
fn chip(glyph: &'static str, label: String, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(glyph, Style::new().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {label}"), Style::new().fg(color)),
    ]
}

/// The chat-server chip — always shown. Besides the glyph and label it carries the reason
/// text when the server is unavailable/not configured: it blocks generation, and the
/// user needs to know why (secondary servers omit the reason for compactness).
fn chat_chip(status: &ServerStatus, palette: &Palette, loc: &'static Locale) -> Vec<Span<'static>> {
    let (glyph, color) = status_glyph(status, palette);
    let label = match status {
        ServerStatus::Ready | ServerStatus::Connecting => loc.t("ui.status.chip.chat").to_string(),
        ServerStatus::NotConfigured => loc.t("ui.status.chip.chat_off").to_string(),
        ServerStatus::Disconnected(why) => loc.tf("ui.status.chip.chat_down", &[("why", why)]),
    };
    chip(glyph, label, color)
}

/// A secondary server's chip (embeddings/impersonation): glyph + label, color by status,
/// no reason text (compact). `None` when the server isn't configured
/// (`NotConfigured`) — the chip is hidden, doesn't clutter the line (impersonation in
/// `shared` mode is also `NotConfigured`).
fn secondary_chip(
    label: &str,
    status: &ServerStatus,
    palette: &Palette,
) -> Option<Vec<Span<'static>>> {
    if matches!(status, ServerStatus::NotConfigured) {
        return None;
    }
    let (glyph, color) = status_glyph(status, palette);
    Some(chip(glyph, label.to_string(), color))
}

/// Lays hotkeys out on a grid of `cols` columns: cells fill row-by-row,
/// left-to-right/top-to-bottom, but **an incomplete bottom row is right-aligned** (its cells
/// take the rightmost columns, under the full rows). Returns a map
/// `grid[row][col] = Some(cell index)`, column widths (max over each column's cells)
/// and the block's total width (the sum of columns + gaps). This way wrapped keys land
/// exactly under the columns of the row above (as in the chat-list window), not as a "floating" group.
fn grid_layout(cell_w: &[usize], cols: usize) -> (Vec<Vec<Option<usize>>>, Vec<usize>, usize) {
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
    let block_w = colw.iter().sum::<usize>() + GAP * cols.saturating_sub(1);
    (grid, colw, block_w)
}

/// Draws hotkeys as a grid right-aligned to `width` (columns aligned
/// vertically; an incomplete bottom row — under the rightmost columns). If
/// `state` is given, the status pill is inserted at the left edge of the **top** row.
fn right_grid(
    hotkeys: &[(&str, &str)],
    cell_w: &[usize],
    cols: usize,
    width: usize,
    mut state: Option<Vec<Span<'static>>>,
    accent_idx: Option<usize>,
    palette: &Palette,
) -> Vec<Line<'static>> {
    let (grid, colw, block_w) = grid_layout(cell_w, cols);
    let lead = width.saturating_sub(block_w);

    let mut out: Vec<Line<'static>> = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        push_row_lead(&mut spans, r, &mut state, lead);
        // Columns: a cell is padded out to the column's width (+ a gap, except the last),
        // an empty column — entirely spaces (this way columns line up vertically).
        for (c, cell) in row.iter().enumerate() {
            let gap = if c + 1 < cols { GAP } else { 0 };
            match cell {
                Some(i) => push_hotkey_cell(
                    &mut spans,
                    hotkeys[*i],
                    accent_idx == Some(*i),
                    colw[c].saturating_sub(cell_w[*i]) + gap,
                    palette,
                ),
                None => spans.push(Span::raw(" ".repeat(colw[c] + gap))),
            }
        }
        out.push(Line::from(spans));
    }
    out
}

/// The left margin of grid row `r`: on the top row — the status pill (taken out
/// of `state`), the rest (and other rows) — spaces (the grid is right-aligned).
fn push_row_lead(
    spans: &mut Vec<Span<'static>>,
    r: usize,
    state: &mut Option<Vec<Span<'static>>>,
    lead: usize,
) {
    if r == 0
        && let Some(pill) = state.take()
    {
        let state_w = spans_width(&pill);
        spans.extend(pill);
        if lead > state_w {
            spans.push(Span::raw(" ".repeat(lead - state_w)));
        }
    } else if lead > 0 {
        spans.push(Span::raw(" ".repeat(lead)));
    }
}

/// One occupied grid cell: the hint (its description accent-highlighted when
/// `accented`) followed by `pad` spaces out to the column's width + gap.
fn push_hotkey_cell(
    spans: &mut Vec<Span<'static>>,
    (key, desc): (&str, &str),
    accented: bool,
    pad: usize,
    palette: &Palette,
) {
    if accented {
        spans.extend(palette.hint_highlight_value(key, desc, palette.accent));
    } else {
        spans.extend(palette.hint(key, desc));
    }
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
}

/// The visible width of a string in terminal columns.
fn str_w(s: &str) -> usize {
    wrap::display_width(&s.chars().collect::<Vec<_>>())
}

/// Total visible width of a set of spans.
fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| str_w(&s.content)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// A status snapshot: chat `chat`, embeddings/impersonation not configured (chips hidden).
    fn only_chat(chat: ServerStatus) -> ServerStatuses {
        ServerStatuses {
            chat,
            embed: ServerStatus::NotConfigured,
            impersonation: ServerStatus::NotConfigured,
        }
    }

    /// A snapshot with a ready chat server (secondaries not configured).
    fn ready() -> ServerStatuses {
        only_chat(ServerStatus::Ready)
    }

    /// A status snapshot for tests (background = `None`).
    fn model<'a>(
        statuses: &'a ServerStatuses,
        generating: bool,
        tokens: u64,
        context: Option<u64>,
        context_exact: bool,
        mouse_scroll: bool,
    ) -> StatusModel<'a> {
        StatusModel {
            statuses,
            generating,
            tokens,
            context,
            context_exact,
            reasoning: 0,
            mouse_scroll,
            background: None,
            speaking: false,
            attachments: None,
            staged_images: None,
            esc_target: EscTarget::default(),
            read_only: false,
        }
    }

    /// The whole status-bar text as one string (wide width → one line).
    fn flat(
        statuses: &ServerStatuses,
        generating: bool,
        tokens: u64,
        context: Option<u64>,
        context_exact: bool,
        mouse_scroll: bool,
    ) -> String {
        let m = model(
            statuses,
            generating,
            tokens,
            context,
            context_exact,
            mouse_scroll,
        );
        lines(200, &m, &Palette::default(), ru())
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    fn text(statuses: &ServerStatuses, generating: bool) -> String {
        flat(statuses, generating, 0, None, false, false)
    }

    #[test]
    fn shows_chat_state() {
        assert!(text(&ready(), false).contains("чат"));
        assert!(text(&only_chat(ServerStatus::NotConfigured), false).contains("не настроен"));
        assert!(
            text(&only_chat(ServerStatus::Disconnected("boom".into())), false).contains("boom")
        );
    }

    #[test]
    fn localized_for_all_langs() {
        // The status bar (axis B) under every language: the chat chip and the quit hotkey — from
        // that language's bundle; en — Latin script. Also checks there's no panic on different
        // widths of the translated text.
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let flat: String = lines(200, &m, &Palette::default(), loc)
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect();
            assert!(
                flat.contains(loc.t("ui.status.chip.chat")),
                "{lang:?}: {flat}"
            );
            assert!(
                flat.contains(loc.t("ui.status.hotkey.quit")),
                "{lang:?}: {flat}"
            );
        }
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        assert_eq!(en.t("ui.status.chip.chat"), "chat");
    }

    #[test]
    fn secondary_chips_shown_only_when_configured() {
        // Embeddings and impersonation configured — both chips are visible.
        let all = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::Connecting,
        };
        let t = text(&all, false);
        assert!(
            t.contains("чат") && t.contains("эмб") && t.contains("имп"),
            "{t}"
        );
        // Impersonation in `shared` / embeddings off (NotConfigured) — chips hidden.
        let t = text(&ready(), false);
        assert!(
            t.contains("чат") && !t.contains("эмб") && !t.contains("имп"),
            "{t}"
        );
    }

    #[test]
    fn compat_palette_swaps_chip_and_activity_glyphs() {
        // In compatibility mode: "connecting" — `○`, a disconnect — `×`, generation — `»`,
        // a background task — `*`; readiness stays `●` (WGL4-safe).
        let compat = Palette::default().with_compat(true);
        let flat_compat = |statuses: &ServerStatuses, generating: bool| -> String {
            let m = StatusModel {
                statuses,
                generating,
                tokens: 0,
                context: None,
                context_exact: false,
                reasoning: 0,
                mouse_scroll: false,
                background: Some("рефлексия"),
                speaking: false,
                attachments: None,
                staged_images: None,
                esc_target: EscTarget::default(),
                read_only: false,
            };
            lines(200, &m, &compat, ru())
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect()
        };
        let t = flat_compat(&only_chat(ServerStatus::Connecting), true);
        assert!(t.contains("○ чат"), "connecting compat glyph: {t}");
        assert!(t.contains("» генерация"), "generation compat glyph: {t}");
        assert!(
            t.contains("* рефлексия"),
            "background-task compat glyph: {t}"
        );
        for banned in ['◐', '⟳', '✻'] {
            assert!(!t.contains(banned), "{banned} left behind: {t}");
        }
        let t = flat_compat(&only_chat(ServerStatus::Disconnected("boom".into())), false);
        assert!(t.contains("× чат"), "disconnect compat glyph: {t}");
        let t = flat_compat(&ready(), false);
        assert!(t.contains("● чат"), "readiness stays ●: {t}");
    }

    #[test]
    fn chip_color_reflects_status() {
        let palette = Palette::default();
        // The glyph's color (first span) by status: ready — success, disconnect — error.
        let glyph_fg = |status: ServerStatus| {
            chat_chip(&status, &palette, ru())
                .first()
                .and_then(|s| s.style.fg)
                .unwrap()
        };
        assert_eq!(glyph_fg(ServerStatus::Ready), palette.success);
        assert_eq!(
            glyph_fg(ServerStatus::Disconnected("x".into())),
            palette.error
        );
        assert_eq!(glyph_fg(ServerStatus::Connecting), palette.warning);
    }

    #[test]
    fn generating_indicator_toggles() {
        assert!(text(&ready(), true).contains("генерация"));
        assert!(!text(&ready(), false).contains("генерация"));
    }

    #[test]
    fn shows_mouse_mode() {
        let mode = |scroll| flat(&ready(), false, 0, None, false, scroll);
        assert!(mode(true).contains("мышь: прокрутка"));
        assert!(mode(false).contains("мышь: выделение"));
    }

    #[test]
    fn scroll_mode_highlights_only_value() {
        let palette = Palette::default();
        let span_fg = |scroll, needle: &str| {
            let statuses = ready();
            let m = model(&statuses, false, 0, None, false, scroll);
            lines(200, &m, &palette, ru())
                .iter()
                .flat_map(|l| l.spans.clone())
                .find(|s| s.content.contains(needle))
                .and_then(|s| s.style.fg)
        };
        // In scroll mode, only the mode value is highlighted (with the `accent` color, as
        // markdown headings are), while the "mouse:" label stays muted — separate spans.
        assert_eq!(span_fg(true, "прокрутка"), Some(palette.accent));
        assert_eq!(span_fg(true, "мышь:"), Some(palette.muted));
        // In selection mode — the whole description is muted.
        assert_eq!(span_fg(false, "мышь: выделение"), Some(palette.muted));
    }

    #[test]
    fn token_counter_shows_summed_total() {
        let line = |tokens, context, exact| flat(&ready(), true, tokens, context, exact, false);
        // No tokens and no context — the counter is hidden.
        assert!(!line(0, None, false).contains("токены"));
        // Reply only (conversation unknown) — sum = reply, no `~`.
        assert!(line(42, None, false).contains("токены: 42"));
        // Sum of conversation and reply; while the conversation is an estimate, marked with `~`.
        assert!(line(42, Some(1000), false).contains("токены: ~1042"));
        // The exact number from usage — no `~`.
        assert!(line(42, Some(1000), true).contains("токены: 1042"));
        // Visible right at the start: conversation present, reply still 0 → sum = conversation.
        assert!(line(0, Some(1000), false).contains("токены: ~1000"));
    }

    #[test]
    fn token_counter_shows_reasoning_when_present() {
        let statuses = ready();
        let with_reason = |reasoning: u32| -> String {
            let m = StatusModel {
                statuses: &statuses,
                generating: true,
                tokens: 42,
                context: Some(1000),
                context_exact: true,
                reasoning,
                mouse_scroll: false,
                background: None,
                speaking: false,
                attachments: None,
                staged_images: None,
                esc_target: EscTarget::default(),
                read_only: false,
            };
            lines(200, &m, &Palette::default(), ru())
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect()
        };
        // Reasoning tokens are shown with a separate annotation (included in the total).
        assert!(with_reason(300).contains("токены: 1042 (рассужд. 300)"));
        // Zero reasoning tokens — no annotation.
        let none = with_reason(0);
        assert!(none.contains("токены: 1042"));
        assert!(!none.contains("рассужд"));
    }

    #[test]
    fn hotkeys_fit_on_one_line_when_wide() {
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        let n = lines(200, &m, &Palette::default(), ru()).len();
        assert_eq!(n, 1);
    }

    #[test]
    fn a_narrow_bar_wraps_to_two_rows_and_sheds_the_rest() {
        // At 40 columns the corner beside the chat chip holds four hints as a
        // 2×2 block; the wide mouse toggle and settings shed (both are on the
        // F1 help list) instead of stacking extra rows.
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        assert_eq!(height(40, &m, &Palette::default(), ru()), 2);
        let text = rows_of(40, &m).join("\n");
        for kept in ["F1", "справка", "чаты", "новый", "выход"] {
            assert!(text.contains(kept), "{kept} kept: {text}");
        }
        for shed in ["мышь", "настройки"] {
            assert!(!text.contains(shed), "{shed} shed: {text}");
        }
        // Every hint is back where the line has room (the `flat` width, 200).
        let flat = flat(&ready(), false, 0, None, false, false);
        assert!(flat.contains("Ctrl+W") && flat.contains("мышь: выделение"));
        for (_, desc) in HOTKEYS {
            assert!(flat.contains(ru().t(desc)));
        }
    }

    /// The status line of a busy chat — the state that used to stack the hints
    /// into one tall column: two servers, generation, a token counter with
    /// thoughts, attached files (the screenshot behind this rule).
    fn busy(statuses: &ServerStatuses) -> StatusModel<'_> {
        StatusModel {
            generating: true,
            tokens: 1218,
            context: Some(36490),
            context_exact: true,
            reasoning: 1218,
            attachments: Some("файлы: 2 (~532)"),
            ..model(statuses, false, 0, None, false, false)
        }
    }

    /// Renders the status bar at width `w` and returns the buffer's rows as text.
    fn rows(w: u16) -> Vec<String> {
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        rows_of(w, &m)
    }

    /// Renders `m` at width `w` (at its own height) and returns the buffer's rows.
    fn rows_of(w: u16, m: &StatusModel) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let p = Palette::default();
        let h = height(w as usize, m, &p, ru());
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, f.area(), m, &p, ru())).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn chips_left_hotkeys_right_on_one_line() {
        // Wide — one line: the status chip is left-aligned, hotkeys — right.
        let r = rows(160);
        assert_eq!(r.len(), 1);
        let line = &r[0];
        assert!(
            line.trim_start().starts_with("●"),
            "chip on the left: {line:?}"
        );
        assert!(
            line.trim_end().ends_with("выход"),
            "the last hotkey is right-aligned: {line:?}"
        );
        // Between the chip and hotkeys — a visible gap (they aren't stuck together).
        assert!(
            line.contains("чат   "),
            "there's a gap after the chip: {line:?}"
        );
    }

    #[test]
    fn wrapped_grid_is_right_aligned_with_pill_on_top_line() {
        // Narrow — hotkeys wrap into a neat grid, right-aligned; the status chip
        // shares the TOP row with the grid, the wrap lands exactly under the column above.
        let r = rows(100);
        assert_eq!(r.len(), 2, "expected a wrap to 2 lines: {r:?}");
        // The status chip — on the top row, on the left.
        assert!(
            r[0].trim_start().starts_with("●"),
            "chip on top, on the left: {:?}",
            r[0]
        );
        // The wrapped hotkey — on the bottom row, right-aligned (the left part is empty).
        assert!(
            r[1].trim_start().starts_with("Ctrl+Q") && r[1].starts_with(" "),
            "the wrap is right-aligned, the left part is empty: {:?}",
            r[1]
        );
        // Columns line up vertically: `Ctrl+Q` (bottom) exactly under `Ctrl+P` (top).
        // Compare position in CHARACTERS (Cyrillic is multibyte → the byte offset isn't
        // the column; here every character is width 1, so character == column).
        let char_col = |line: &str, pat: &str| line.find(pat).map(|b| line[..b].chars().count());
        assert_eq!(
            char_col(&r[1], "Ctrl+Q"),
            char_col(&r[0], "Ctrl+P"),
            "the wrap lands exactly under the column above:\n{:?}\n{:?}",
            r[0],
            r[1]
        );
    }

    #[test]
    fn trim_keeps_the_highest_ranked_hints_that_fit() {
        // The cell widths of the chat's six hotkeys in the ru bundle: one wide
        // cell (the mouse toggle) and five short ones. Synthetic on purpose —
        // the rule is about widths, and shouldn't be re-measured against wording.
        let cells = [24, 12, 10, 14, 18, 14];
        // Room for one row — everything stays, as one row of six.
        assert_eq!(trim_to_fit(&cells, 107, false), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(corner_cols(&cells, 107), Some(6));
        // The quiet pill's remainder at 120 columns — everything stays, as the
        // reference 3×2 grid.
        assert_eq!(trim_to_fit(&cells, 71, false), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(corner_cols(&cells, 71), Some(3));
        // The generating pill's remainder — settings and new-chat shed.
        assert_eq!(trim_to_fit(&cells, 54, false), vec![0, 1, 2, 5]);
        // Tighter still — the wide mouse toggle sheds too, and new-chat comes
        // back in its place: a too-wide cell is passed over, not a lock-out.
        assert_eq!(trim_to_fit(&cells, 32, false), vec![1, 2, 3, 5]);
        // Barely a corner: F1 anchors a one-column block, then nothing — a
        // block that cannot hold F1 is refused even when Esc alone would fit.
        assert_eq!(trim_to_fit(&cells, 12, false), vec![1, 2]);
        assert!(trim_to_fit(&cells, 11, false).is_empty());
        // Scroll mode pins the mouse toggle ahead of everything: it stays at a
        // width where it shed above (a one-column block with F1)…
        assert_eq!(trim_to_fit(&cells, 25, true), vec![0, 1]);
        // …and where even it cannot fit, it is passed over like any other.
        assert_eq!(trim_to_fit(&cells, 23, true), vec![1, 2]);
    }

    /// `keep_order` addresses hints by index into `hotkey_list`'s order — this
    /// pins the mapping so a reordering of the list cannot silently reshuffle
    /// the shedding priority (or detach `HELP_IDX` from `F1`).
    #[test]
    fn keep_order_matches_the_hotkey_list() {
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        let keys: Vec<&str> = hotkey_list(&m, ru()).iter().map(|(k, _)| *k).collect();
        assert_eq!(keys[HELP_IDX], HELP_KEY);
        let named = |order: [usize; 6]| order.map(|i| keys[i]);
        assert_eq!(
            named(keep_order(false)),
            ["F1", "Esc", "Ctrl+Q", "Ctrl+W", "Ctrl+P", "Ctrl+N"]
        );
        assert_eq!(
            named(keep_order(true)),
            ["Ctrl+W", "F1", "Esc", "Ctrl+Q", "Ctrl+P", "Ctrl+N"]
        );
    }

    #[test]
    fn a_busy_pill_sheds_hints_but_keeps_the_corner_block() {
        // The state that once stacked six hint rows, then sent the hints onto
        // a full-width row below the pill: at 120 columns the busy pill leaves
        // the corner 32. Now the wide hints shed (mouse toggle, settings) and
        // the rest stay a 2×2 block beside the pill — two rows, and no keycap
        // ever sits under the indicators.
        let statuses = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::NotConfigured,
        };
        let r = rows_of(120, &busy(&statuses));
        assert_eq!(r.len(), 2, "pill plus a two-row corner block: {r:?}");
        assert!(
            r[0].trim_start().starts_with('●') && r[0].contains("токены"),
            "the pill keeps the top row: {:?}",
            r[0]
        );
        assert!(
            r[0].contains("F1") && r[0].trim_end().ends_with("чаты"),
            "kept hints share the pill's row: {:?}",
            r[0]
        );
        assert!(
            r[1].starts_with(' ') && r[1].trim_end().ends_with("выход"),
            "the wrapped row is right-aligned: {:?}",
            r[1]
        );
        // Columns line up: the wrap lands exactly under the column above.
        // Compare position in CHARACTERS (Cyrillic is multibyte).
        let char_col = |line: &str, pat: &str| line.find(pat).map(|b| line[..b].chars().count());
        assert_eq!(
            char_col(&r[1], "Ctrl+N"),
            char_col(&r[0], "F1"),
            "\n{:?}\n{:?}",
            r[0],
            r[1]
        );
        let text = r.join("\n");
        assert!(
            !text.contains("мышь") && !text.contains("настройки"),
            "shed hints are hidden, not wrapped: {text}"
        );
    }

    /// The screenshot behind the corner rule: a generating pill of 63 columns
    /// at a 120-column window used to send all six hints onto a full-width row
    /// below — a wall of keycaps starting under the server chips. Now the two
    /// widest hints shed and the block stays beside the pill.
    #[test]
    fn the_generating_pill_keeps_the_hints_in_the_corner() {
        let statuses = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::NotConfigured,
        };
        let m = StatusModel {
            generating: true,
            tokens: 4732,
            context: Some(22597),
            context_exact: true,
            reasoning: 4732,
            ..model(&statuses, false, 0, None, false, false)
        };
        let r = rows_of(120, &m);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r[0].contains("токены: 27329 (рассужд. 4732)"), "{:?}", r[0]);
        assert!(
            r[0].trim_end().ends_with("справка"),
            "the mouse toggle and F1 share the pill's row: {:?}",
            r[0]
        );
        let char_col = |line: &str, pat: &str| line.find(pat).map(|b| line[..b].chars().count());
        assert_eq!(
            char_col(&r[1], "Esc"),
            char_col(&r[0], "Ctrl+W"),
            "a right-aligned 2×2 block, nothing under the pill:\n{:?}\n{:?}",
            r[0],
            r[1]
        );
        let text = r.join("\n");
        assert!(
            !text.contains("новый") && !text.contains("настройки"),
            "{text}"
        );
    }

    /// Scroll mode survives shedding: the accent-highlighted mouse toggle is a
    /// mode light, pinned ahead of everything, and keeps its accent inside the
    /// trimmed block.
    #[test]
    fn scroll_mode_pins_the_mouse_toggle_through_trimming() {
        let statuses = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::NotConfigured,
        };
        let mut m = busy(&statuses);
        m.mouse_scroll = true;
        let p = Palette::default();
        let ls = lines(120, &m, &p, ru());
        assert_eq!(ls.len(), 2);
        let accent_span = ls
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("прокрутка"))
            .expect("the pinned mouse toggle is shown");
        assert_eq!(accent_span.style.fg, Some(p.accent));
        let flat: String = ls
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(flat.contains("F1"), "F1 rides along: {flat}");
        assert!(!flat.contains("настройки"), "sheds still happen: {flat}");
    }

    /// A pill that fills the window leaves no corner that could anchor on
    /// `F1` — the bar falls back to the help entry alone on a row below, the
    /// one hint that must survive every layout.
    #[test]
    fn a_wall_of_pill_leaves_f1_alone_below() {
        let statuses = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::NotConfigured,
        };
        let r = rows_of(96, &busy(&statuses));
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r[0].contains("файлы"), "the pill keeps its row: {:?}", r[0]);
        let below = r[1].trim();
        assert!(
            below.starts_with("F1") && below.ends_with("справка"),
            "F1 alone: {:?}",
            r[1]
        );
        assert!(r[1].starts_with(' '), "right-aligned: {:?}", r[1]);
    }

    #[test]
    fn the_bar_never_exceeds_two_rows_and_keeps_f1() {
        // The invariant the corner rule buys, stated across widths, languages
        // and states: the bar is never taller than HINT_ROWS_MAX rows however
        // full the pill (before, a busy pill cost up to five extra rows, then
        // a full-width hint row), and the help entry survives at every width
        // that can hold its cell.
        let p = Palette::default();
        let quiet_statuses = ready();
        let quiet = model(&quiet_statuses, false, 0, None, false, false);
        let scroll = model(&quiet_statuses, false, 0, None, false, true);
        let busy_statuses = ServerStatuses {
            chat: ServerStatus::Disconnected("connection refused".into()),
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::Connecting,
        };
        let busy = busy(&busy_statuses);
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let f1_cell = 2 + 2 + 1 + str_w(loc.t("ui.status.hotkey.help"));
            for w in 12..=240usize {
                for m in [&quiet, &scroll, &busy] {
                    let ls = lines(w, m, &p, loc);
                    assert!(
                        ls.len() <= HINT_ROWS_MAX,
                        "{lang:?} w={w}: {} rows",
                        ls.len()
                    );
                    let flat: String = ls
                        .iter()
                        .flat_map(|l| l.spans.iter())
                        .map(|s| s.content.as_ref())
                        .collect();
                    assert!(
                        w < f1_cell || flat.contains("F1"),
                        "{lang:?} w={w}: F1 missing: {flat}"
                    );
                }
            }
        }
    }

    #[test]
    fn hotkey_lines_wraps_and_right_aligns() {
        let p = Palette::default();
        let items: &[(&str, &str)] = &[
            ("Tab", "секция"),
            ("↑↓", "поля"),
            ("Enter", "правка"),
            ("Esc", "назад"),
            ("Ctrl+C", "выход"),
        ];
        // Wide — one line, right-aligned (ends with the last key's description).
        let wide = hotkey_lines(200, items, &p);
        assert_eq!(wide.len(), 1);
        let flat: String = wide[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(flat.contains("Tab") && flat.trim_end().ends_with("выход"));
        // Narrow — wraps onto several lines.
        assert!(hotkey_lines(24, items, &p).len() > 1);
        // An empty set — no lines.
        assert!(hotkey_lines(80, &[], &p).is_empty());
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(60, 1)).unwrap();
        let all = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::Connecting,
        };
        let m = StatusModel {
            statuses: &all,
            generating: true,
            tokens: 123,
            context: Some(456),
            context_exact: true,
            reasoning: 0,
            mouse_scroll: true,
            background: Some("рефлексия"),
            speaking: true,
            attachments: Some("файлы: 2 (~3.1k)"),
            staged_images: Some("изображения: 1 (~1.2k)"),
            esc_target: EscTarget::SearchResults,
            read_only: false,
        };
        term.draw(|f| render(f, f.area(), &m, &Palette::default(), ru()))
            .unwrap();
    }

    /// The staged-images chip is a second, independent one — a file attachment
    /// and an image staged for the next message are different costs and must not
    /// share a slot. Its glyph stays **one column** wide, or the right-aligned
    /// hotkey grid shifts on the terminals that measure it differently
    /// (docs/lessons.md §5).
    #[test]
    fn staged_images_chip_is_separate_and_one_column() {
        let statuses = ready();
        let text = |files, images| {
            let mut m = model(&statuses, false, 0, None, false, false);
            m.attachments = files;
            m.staged_images = images;
            lines(200, &m, &Palette::default(), ru())
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        // Nothing staged — no image chip at all.
        let empty = text(None, None);
        assert!(!empty.contains(IMAGE_GLYPH), "{empty}");
        // Staged images get their own chip and do not displace the files one.
        let both = text(Some("файлы: 2 (~3.1k)"), Some("изображения: 1 (~1.2k)"));
        assert!(both.contains("§ файлы: 2"), "{both}");
        assert!(both.contains("# изображения: 1"), "{both}");
        // And images alone show without any files attached.
        let alone = text(None, Some("изображения: 1 (~1.2k)"));
        assert!(alone.contains("# изображения: 1"), "{alone}");
        assert!(!alone.contains(ATTACH_GLYPH), "{alone}");
        assert_eq!(str_w(&IMAGE_GLYPH.to_string()), 1, "one column");
    }

    /// `Esc` means "go back", and the bar is the only place on screen that says
    /// where — so the hint has to follow the key rather than always claiming the
    /// chat list.
    #[test]
    fn esc_hint_follows_where_back_goes() {
        let statuses = ready();
        let chats = ru().t("ui.status.hotkey.chats");
        let results = ru().t("ui.status.hotkey.results");
        assert_ne!(chats, results, "the two labels must be distinguishable");

        let text = |target| {
            let mut m = model(&statuses, false, 0, None, false, false);
            m.esc_target = target;
            lines(200, &m, &Palette::default(), ru())
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };

        let ordinary = text(EscTarget::ChatList);
        assert!(ordinary.contains(chats), "{ordinary}");
        assert!(!ordinary.contains(results), "{ordinary}");

        let from_results = text(EscTarget::SearchResults);
        assert!(from_results.contains(results), "{from_results}");
        assert!(!from_results.contains(chats), "{from_results}");
    }
}
