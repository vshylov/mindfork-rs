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
}

impl EscTarget {
    /// The bundle key of the `Esc` description. `ChatList` deliberately returns
    /// the same key [`HOTKEYS`] carries, so the const stays honest as the default.
    fn hint_key(self) -> &'static str {
        match self {
            EscTarget::ChatList => "ui.status.hotkey.chats",
            EscTarget::SearchResults => "ui.status.hotkey.results",
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
/// wheel scrolling is on (otherwise native text selection). `background` — a quiet indicator of a
/// background task (auto-reflection/consolidation; `None` — none). See spec §11.1, §11.3.
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
    /// Where `Esc` goes from here — it decides the `Esc` hint's wording.
    pub esc_target: EscTarget,
}

/// The speaking-indicator glyph (WGL4, width 1 column — the hotkey grid doesn't "shift").
const SPEAKING_GLYPH: char = '♪';

/// The attached-files glyph. Like `♪` it is WGL4 and one column wide, so it needs
/// no compat-mode replacement (an emoji paperclip would be two columns and would
/// shift the hotkey grid on legacy terminals).
const ATTACH_GLYPH: char = '§';

/// Draws the status line in `area`. When hotkeys don't fit by width, they
/// wrap onto the next lines as a neat grid (like in the chat-list overlay);
/// the caller reserves height for this via [`height`]. See spec §11.1, §11.3.
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
/// exactly this height (minimum 1). Hotkeys wrap when they don't fit.
pub fn height(width: usize, model: &StatusModel, palette: &Palette, loc: &'static Locale) -> u16 {
    lines(width, model, palette, loc)
        .len()
        .clamp(1, u16::MAX as usize) as u16
}

/// Lays hotkeys out in a neat grid, **right-aligned** to `width` — the same
/// wrapping logic as the chat screen's status bar, but without the status pill
/// (`state = None`). Column count — the max that fits the width (→ fewest rows);
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
    let n = hotkeys.len();
    let cols = (1..=n)
        .rev()
        .find(|&c| grid_layout(&cell_w, c).2 <= width)
        .unwrap_or(1);
    right_grid(hotkeys, &cell_w, cols, width, None, None, palette)
}

/// Builds the status bar's lines. On the left, on the **top** row — "state" (the status
/// pill, generation indicator, token counter), left-aligned. On the right —
/// hotkeys (starting with the mouse toggle `Ctrl+W`, whose description = the current mode), laid out
/// as a neat grid and **right-aligned**: when everything fits — one line
/// (pill on the left, hotkeys on the right); when it doesn't — hotkeys wrap DOWN into a grid
/// whose columns line up vertically, and an incomplete (wrapped) row is right-aligned —
/// its keys land exactly under the columns of the row above, not drawing attention to
/// the left/middle part of the window. The status pill shares the top row with the grid. See
/// spec §11.1, §11.3.
fn lines(
    width: usize,
    model: &StatusModel,
    palette: &Palette,
    loc: &'static Locale,
) -> Vec<Line<'static>> {
    let state = state_spans(model, palette, loc);
    let state_w = spans_width(&state);

    let hotkeys = hotkey_list(model, loc);
    let n = hotkeys.len();
    // In "scroll" mode, highlight the mouse toggle's description (index 0) with the
    // `accent` color — the same one that highlights markdown headings in the feed.
    let accent_idx = model.mouse_scroll.then_some(0usize);

    // Each cell's width: "key" (+2 for padding) + a space + the description.
    let cell_w: Vec<usize> = hotkeys
        .iter()
        .map(|(key, desc)| str_w(key) + 2 + 1 + str_w(desc))
        .collect();

    // Pick the max number of columns (→ fewest rows) at which the status pill
    // can share the top line with the right-aligned grid (`state + GAP + block ≤ width`).
    let mut cols = 0;
    for c in (1..=n).rev() {
        if state_w + GAP + grid_layout(&cell_w, c).2 <= width {
            cols = c;
            break;
        }
    }
    if cols > 0 {
        return right_grid(
            &hotkeys,
            &cell_w,
            cols,
            width,
            Some(state),
            accent_idx,
            palette,
        );
    }

    // Too narrow even for one column next to the pill — "state" gets its own
    // top line, and hotkeys go into a right-aligned grid below it (no overlap).
    cols = (1..=n)
        .rev()
        .find(|&c| grid_layout(&cell_w, c).2 <= width)
        .unwrap_or(1);
    let mut out = vec![Line::from(state)];
    out.extend(right_grid(
        &hotkeys, &cell_w, cols, width, None, accent_idx, palette,
    ));
    out
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
    // A quiet indicator of a background task (auto-reflection/consolidation) — muted,
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
            esc_target: EscTarget::default(),
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
                esc_target: EscTarget::default(),
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
                esc_target: EscTarget::default(),
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
    fn hotkeys_wrap_to_grid_when_narrow() {
        // Narrow width → hotkeys don't fit and wrap onto the next lines.
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        let h = height(40, &m, &Palette::default(), ru());
        assert!(h > 1, "expected hotkeys to wrap, height = {h}");
        // All hotkeys are present despite wrapping (including the mouse toggle).
        let flat = flat(&ready(), false, 0, None, false, false);
        assert!(flat.contains("Ctrl+W") && flat.contains("мышь: выделение"));
        for (_, desc) in HOTKEYS {
            assert!(flat.contains(ru().t(desc)));
        }
    }

    /// Renders the status bar at width `w` and returns the buffer's rows as text.
    fn rows(w: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let p = Palette::default();
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        let h = height(w as usize, &m, &p, ru());
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, f.area(), &m, &p, ru())).unwrap();
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
            esc_target: EscTarget::SearchResults,
        };
        term.draw(|f| render(f, f.area(), &m, &Palette::default(), ru()))
            .unwrap();
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
