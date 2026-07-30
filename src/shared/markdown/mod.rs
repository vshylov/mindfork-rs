//! Markdown → `ratatui::Text` render + a light unicode approximation of LaTeX.
//! See spec §11.4 and docs/decisions/0003-own-markdown-renderer.md.
//!
//! Markdown is parsed directly via `pulldown-cmark` through our own "writer"
//! (`Writer`): markdown → `Text`. This gives theming via [`Palette`] (not
//! hardcoded colors), and later — native tables and math events. Code-block
//! highlighting is `syntect` + `ansi-to-tui`. Full LaTeX and rendering to an
//! image are deliberately NOT implemented.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};

use ansi_to_tui::IntoText;
use pulldown_cmark::{
    Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SynColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

use crate::shared::theme::Palette;
use crate::shared::wrap;

/// Markdown-render behavior flags (width and palette stay separate arguments,
/// as before). Precedent — `RenderOpts` on [`crate::widgets::input_box`]:
/// options as a struct instead of a growing list of positional bools.
/// `Default` — a "clean" CommonMark render (all flags off); the caller opts
/// in to what it needs (the feed threads flags from interface settings).
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOpts {
    /// Treat a "soft" break (a single `\n` in the source) as a real line
    /// break (GFM style, like GitHub comments). `false` — standard
    /// CommonMark: a single newline collapses into a space. Needed for
    /// **user messages**: text typed with `Shift+Enter` should render line
    /// by line, not merge into one paragraph. In table cells a line break is
    /// always a space regardless (the table does its own row layout).
    pub soft_break_as_newline: bool,
    /// Horizontal separators (`├───┼───┤`) between table body rows — a
    /// "grid" look. `false` — compact: a separator only under the header.
    /// Controlled by the `interface.table_row_separators` setting (see spec
    /// §11.4).
    pub table_row_separators: bool,
    /// Render ```mermaid blocks as a diagram (Unicode/ASCII, the
    /// `mermaid-text` crate) instead of the source. Flowchart/sequence only;
    /// on any failure — a fallback to the source as a code block (see the
    /// [`mermaid`] submodule). Controlled by the `interface.render_mermaid`
    /// setting (spec §11.4); `false` (Default) — the previous behavior, the
    /// source.
    pub render_mermaid: bool,
}

/// Renders a markdown string into an owning [`Text`] (ready to show/cache)
/// with default flags ([`RenderOpts::default`]).
///
/// `width` — panel width in columns (used for table layout). `palette` sets
/// the colors (headings/links/code/quotes) for the current theme.
///
/// LaTeX is processed **only inside math delimiters** (`$…$`/`$$…$$`; the
/// forms `\(…\)`/`\[…\]` are normalized to them in [`normalize_delimiters`]).
/// The content of the parser's math events goes through [`latex_to_unicode`]
/// (arrows, fractions, indices, symbols), and the parser itself strips the
/// delimiters. "Bare" commands outside delimiters (`\alpha` with no `$`) are
/// NOT touched. Returns a `'static`-`Text`.
// Production paths (the feed) pass flags explicitly via `render_with`; the
// module's own tests use the defaults facade — kept as a public surface (see
// ADR 0003).
#[allow(dead_code)]
pub fn render(input: &str, width: usize, palette: &Palette) -> Text<'static> {
    render_with(input, width, palette, RenderOpts::default())
}

/// Like [`render`], but with explicit behavior flags (see [`RenderOpts`]).
pub fn render_with(
    input: &str,
    width: usize,
    palette: &Palette,
    opts: RenderOpts,
) -> Text<'static> {
    let normalized = normalize_delimiters(input);
    let mut parse_opts = Options::empty();
    parse_opts.insert(Options::ENABLE_STRIKETHROUGH);
    parse_opts.insert(Options::ENABLE_TASKLISTS);
    parse_opts.insert(Options::ENABLE_MATH);
    parse_opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(&normalized, parse_opts);
    let mut writer = Writer::new(*palette, width);
    writer.soft_break_as_newline = opts.soft_break_as_newline;
    writer.table_row_separators = opts.table_row_separators;
    writer.render_mermaid = opts.render_mermaid;
    // An iterator with byte ranges: Writer uses the source to tell a closed
    // code block apart from one truncated mid-stream (the mermaid path, see
    // fenced_block_is_closed).
    writer.run(&normalized, parser.into_offset_iter());
    Text::from(writer.lines)
}

/// Highlights a code block `code` in language `lang` **with no enclosing
/// ` ``` `** and no width wrapping — for embedding into feed tool cards (see
/// [`crate::widgets::message_feed`]). Returns one line per source line,
/// colored via [`build_code_theme`] (the same theme as fenced markdown
/// blocks). If the language isn't recognized ([`resolve_syntax`] missed) —
/// unhighlighted lines in `palette.text` color (not reversed — more readable
/// for not-strictly-code arguments). Wrapping long lines is the caller's job.
pub fn highlight_code(code: &str, lang: &str, palette: &Palette) -> Vec<Line<'static>> {
    let Some(syntax) = resolve_syntax(lang) else {
        return code
            .split('\n')
            .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(palette.text))))
            .collect();
    };
    let theme = code_theme(palette);
    let mut hl = HighlightLines::new(syntax, theme);
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in LinesWithEndings::from(code) {
        out.extend(highlight_line_or_plain(&mut hl, line));
    }
    // Highlighting often adds a trailing empty line — strip it so the card
    // doesn't get an extra blank row below the block.
    while out
        .last()
        .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
    {
        out.pop();
    }
    out
}

// ---------- styles from the palette ----------

/// Style of a heading at `level` (1 — largest).
fn heading_style(level: u8, palette: &Palette) -> Style {
    let base = Style::new().fg(palette.accent).add_modifier(Modifier::BOLD);
    match level {
        1 => base.add_modifier(Modifier::UNDERLINED),
        2 => base,
        _ => base.add_modifier(Modifier::ITALIC),
    }
}

/// Style of an unhighlighted code block — reversed (theme-independent).
fn code_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Style of inline code (`like this`) — a quiet "chip" as in the design
/// mockup: soft muted text on a "keycap" background, not a harsh reverse
/// (REVERSED was too loud). Themed via [`Palette`].
fn inline_code_style(palette: &Palette) -> Style {
    Style::new().fg(palette.keycap_fg).bg(palette.keycap_bg)
}

/// Link style.
fn link_style(palette: &Palette) -> Style {
    Style::new()
        .fg(palette.accent)
        .add_modifier(Modifier::UNDERLINED)
}

/// Blockquote style (theme-independent).
fn blockquote_style() -> Style {
    Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC)
}

/// Cache of syntect code-highlighting themes, **built from [`Palette`]** (see
/// [`build_code_theme`]). The theme used to be hardcoded (`base16-ocean.dark`)
/// and didn't track dark/light/auto — ADR 0003 flagged this as future work.
///
/// The key is the palette (only three distinct ones exist: auto/dark/light),
/// so the `Box::leak` is bounded and justified: `HighlightLines<'static>`
/// needs a theme with `'static` lifetime, and the number of themes is finite
/// and lives for the whole process. The alternative (theme on `render`'s
/// stack + a lifetime on `Writer`) would complicate the type for savings that
/// don't exist.
static CODE_THEMES: LazyLock<Mutex<HashMap<Palette, &'static Theme>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns (building on first access and caching) the syntect code-highlight
/// theme for this palette.
fn code_theme(palette: &Palette) -> &'static Theme {
    let mut cache = CODE_THEMES.lock().expect("CODE_THEMES poisoned");
    cache
        .entry(*palette)
        .or_insert_with(|| Box::leak(Box::new(build_code_theme(palette))))
}

// ---------- submodules (breaking up the god object: docs/history/refactoring-god-objects.md, stage 6) ----------

mod code;
mod html;
mod latex;
mod mermaid;
mod speak;
mod table;
mod writer;

// The speech extractor (TTS, docs/research/tts.md §5) — a second consumer of
// the same parser events, so it lives next to the render — one function
// exposed externally.
#[allow(unused_imports)]
pub use self::speak::speakable_text;

// Internal wiring: Writer (writer) + highlighting (code) + tables (table) +
// LaTeX (latex) + diagrams (mermaid) are visible to each other and to mod.rs
// via a re-export (the external surface is render/render_with only, defined
// here).
use self::{code::*, html::*, latex::*, mermaid::*, table::*, writer::*};

/// Shared render test helpers used by several submodules' tests
/// (code/latex/table/writer). See docs/history/refactoring-god-objects.md.
#[cfg(test)]
pub(super) mod testkit {
    use super::*;

    /// Collects the whole render's text into one string (for content checks):
    /// spans within one line are joined with no separator, lines are joined
    /// with `\n`.
    pub(super) fn rendered_text(input: &str) -> String {
        rendered_text_w(input, 80)
    }

    /// Like [`rendered_text`], but with a given width.
    pub(super) fn rendered_text_w(input: &str, width: usize) -> String {
        render(input, width, &Palette::default())
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Maximum width (in columns) among the render's lines.
    pub(super) fn max_line_width(input: &str, width: usize) -> usize {
        let text = render(input, width, &Palette::default());
        text.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0)
    }

    /// Collects the set of foreground colors of the render's spans.
    pub(super) fn fg_colors(input: &str, palette: &Palette) -> Vec<Color> {
        render(input, 80, palette)
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| s.style.fg)
            .collect()
    }

    pub(super) const TABLE_MD: &str = "\
| Алгоритм | Время | Память |
| :--- | :--- | :--- |
| QuickSort | O(n log n) | O(log n) |
| MergeSort | O(n log n) | O(n) |";

    pub(super) const CODE_MD: &str = "```rust\nfn main() {\n    let s = \"hi\";\n    // c\n}\n```";
}
