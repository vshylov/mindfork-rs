//! The mindfork logo drawn in terminal cells (see docs/branding.md §5).
//!
//! The brand icon is pixel art on a 16×16 grid made of five rectangles, so it
//! can be drawn **natively**, with no raster images: one terminal cell carries
//! **two vertical pixels** via half-block glyphs `▀`/`▄`/`█`. The
//! transparent variant is drawn (no backdrop) — the glyph sits on the terminal's background and is
//! equally at home in the dark and light theme.
//!
//! Next to the glyph is drawn the **wordmark** — the word `mindfork` in its own pixel
//! font (the SVG wordmark doesn't work for the terminal: its text has been converted to curves).
//! Together they form a **horizontal lockup** — the same composition as
//! `artwork/mindfork-wordmark.svg`: the word is half the glyph's height, its baseline sits one
//! row above the glyph's bottom.
//!
//! **Colors are the fixed brand ones, not from the palette**: the logo isn't retinted by the
//! theme (decision R3 in docs/branding.md — the interface's `accent` means "activity",
//! retinting would break that semantics). The one exception — `mind` in the wordmark: in the brand this is
//! "text on dark"/"text on light" (separate `-dark`/`-light` variants), i.e.
//! a color derived from the background, so here it's taken from the palette (`text`) — this way one lockup works
//! in both themes. The `▀`/`▄`/`█` glyphs are in WGL4, so the older-terminal
//! compatibility mode (conhost) needs no separate substitution — like the `█` scrollbar and
//! `▌` role rails.
//!
//! The single source of truth for the glyph's geometry is
//! `artwork/mindfork-icon-transparent.svg`; the test `glyph_matches_artwork_svg` checks
//! the table below against it, so the code and the asset don't silently drift apart.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// The brand accent — the glyph's stem (`#c25a27`).
const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);
/// The glyph's branches (`#5c6370`).
const GRAY: Color = Color::Rgb(0x5c, 0x63, 0x70);

/// The glyph's rectangles on a 16×16 grid: `(x, y, w, h, color)`.
const GLYPH: [(u8, u8, u8, u8, Color); 5] = [
    (7, 2, 2, 12, ORANGE),
    (11, 2, 2, 5, GRAY),
    (9, 5, 2, 2, GRAY),
    (3, 7, 2, 5, GRAY),
    (5, 10, 2, 2, GRAY),
];

/// The ink bounds on the grid (`x` 3…13, `y` 2…14) — we draw only these, no empty margins.
const INK_X: (u8, u8) = (3, 13);
const INK_Y: (u8, u8) = (2, 14);

/// The logo's width in terminal columns.
pub const LOGO_COLS: u16 = (INK_X.1 - INK_X.0) as u16;
/// The logo's height in rows: two pixels per row.
pub const LOGO_ROWS: u16 = (INK_Y.1 - INK_Y.0) as u16 / 2;

/// The wordmark's pixel font: one row per pixel row of the glyph (`#` — ink).
///
/// The lettering follows the brand wordmark (JetBrains Mono ExtraBold): lowercase letters,
/// ascenders/descenders on `d`/`f`/`k`, and **2-pixel strokes** — the same weight as the
/// icon glyph's bars (its grid also uses 2-unit bars). A 1-pixel stroke would give a light
/// weight clashing with the chunky glyph, while a letter with a 2-pixel stem (`i`)
/// would look twice as heavy among them.
///
/// Height — **8 pixels = 4 terminal rows**: the brand defines the ascender/descender height as
/// `0.5227 × S`, where `S` is the icon size (16), i.e. ≈ 8 pixels (not half the
/// glyph's 12-pixel ink). This also gives an x-height of 6 pixels, into which the 2-pixel
/// strokes and the gap between them fit exactly.
///
/// A separate compromise — `f`. Between its hook (at ascender height) and the crossbar
/// (at x-height) there are 0 rows left in the pixel budget, so a 2-pixel hook
/// used to merge with the crossbar into a solid block. The hook is made **1 pixel**: it
/// lands in the cell's top half (`▀`), the bottom stays empty — a gap shows,
/// and the crossbar sits at x-height, on the same row as the top bars of `n`/`o`/`r`.
///
/// A dedicated pixel font is needed: in the SVG wordmark the text has been converted to curves
/// (docs/branding.md §2), there's nothing to rasterize them with in the terminal.
#[rustfmt::skip]
const WORDMARK: [(char, [&str; WORDMARK_PX_ROWS]); 8] = [
    ('m', ["........", "........", "########", "########", "##.##.##", "##.##.##", "##.##.##", "##.##.##"]),
    ('i', ["##",       "##",       "..",       "##",       "##",       "##",       "##",       "##"      ]),
    ('n', ["......",   "......",   "######",   "######",   "##..##",   "##..##",   "##..##",   "##..##"  ]),
    ('d', ["....##",   "....##",   "######",   "######",   "##..##",   "##..##",   "######",   "######"  ]),
    ('f', ["..####",   "..##..",   "######",   "######",   "..##..",   "..##..",   "..##..",   "..##.."  ]),
    ('o', ["......",   "......",   "######",   "######",   "##..##",   "##..##",   "######",   "######"  ]),
    ('r', [".....",    ".....",    "#####",    "#####",    "##...",    "##...",    "##...",    "##..."   ]),
    ('k', ["##....",   "##....",   "##.###",   "##.###",   "####..",   "####..",   "##.###",   "##.###"  ]),
];

/// The wordmark's height in pixels (even — half-block glyphs land evenly into rows).
const WORDMARK_PX_ROWS: usize = 8;
/// Letter spacing, in columns.
const WORDMARK_TRACKING: u16 = 1;
/// How many leading letters are "mind" (text color); the rest — "fork" (accent).
const MIND_LETTERS: usize = 4;

/// The wordmark's height in terminal rows.
pub const WORDMARK_ROWS: u16 = WORDMARK_PX_ROWS as u16 / 2;
/// The wordmark's width in terminal columns.
pub const WORDMARK_COLS: u16 = wordmark_cols();

/// The "glyph → word" gap in columns (the brand's proportion: 0.36 × the icon size
/// minus the empty margin to the right of the ink, docs/branding.md §5).
const LOCKUP_GAP: u16 = 3;
/// The lockup row where the word starts: its baseline ends up one
/// row above the glyph's bottom (the brand — `0.7418 × S` from the icon's top), the same as in
/// the SVG lockup.
const WORDMARK_TOP_ROW: u16 = 1;

/// The horizontal lockup's width (glyph + gap + word) in columns.
pub const LOCKUP_COLS: u16 = LOGO_COLS + LOCKUP_GAP + WORDMARK_COLS;
/// The lockup's height in rows: the word is shorter than the glyph, so the glyph sets it.
pub const LOCKUP_ROWS: u16 = LOGO_ROWS;

// The word must fit within the glyph's height — otherwise `lockup_lines` would silently clip
// its bottom rows. Editing `WORDMARK_TOP_ROW`/the font fails the build, not the picture.
const _: () = assert!(WORDMARK_TOP_ROW + WORDMARK_ROWS <= LOCKUP_ROWS);

/// The word's total width: glyphs plus the spacing between them.
const fn wordmark_cols() -> u16 {
    let mut total = 0;
    let mut i = 0;
    while i < WORDMARK.len() {
        if i > 0 {
            total += WORDMARK_TRACKING;
        }
        total += WORDMARK[i].1[0].len() as u16;
        i += 1;
    }
    total
}

/// The color of grid pixel `(x, y)`, if it's filled in.
fn pixel(x: u8, y: u8) -> Option<Color> {
    GLYPH
        .iter()
        .find(|(gx, gy, gw, gh, _)| x >= *gx && x < gx + gw && y >= *gy && y < gy + gh)
        .map(|(_, _, _, _, color)| *color)
}

/// The logo as lines for insertion into any widget: `LOGO_ROWS` rows of `LOGO_COLS` cells.
///
/// A pair of vertical pixels is encoded as one cell: both halves the same color —
/// `█`; different — `▀` (the top in `fg`, the bottom in `bg`); one half — `▀`/`▄` with no
/// background (this way the logo doesn't drag a backdrop rectangle along with it).
pub fn logo_lines() -> Vec<Line<'static>> {
    (0..LOGO_ROWS)
        .map(|row| {
            let top_y = INK_Y.0 + (row as u8) * 2;
            let spans = (INK_X.0..INK_X.1)
                .map(|x| match (pixel(x, top_y), pixel(x, top_y + 1)) {
                    (Some(t), Some(b)) if t == b => Span::styled("█", Style::new().fg(t)),
                    (Some(t), Some(b)) => Span::styled("▀", Style::new().fg(t).bg(b)),
                    (Some(t), None) => Span::styled("▀", Style::new().fg(t)),
                    (None, Some(b)) => Span::styled("▄", Style::new().fg(b)),
                    (None, None) => Span::raw(" "),
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

/// The word's columns: a color and a `WORDMARK_PX_ROWS`-pixel column for each.
///
/// Letters are separated by an empty column (letter spacing), so color is a column property:
/// "mind" is drawn with the `mind` color, "fork" — with the fixed brand accent.
fn wordmark_columns(mind: Color) -> Vec<(Color, [bool; WORDMARK_PX_ROWS])> {
    let mut cols = Vec::with_capacity(WORDMARK_COLS as usize);
    for (i, (_, rows)) in WORDMARK.iter().enumerate() {
        if i > 0 {
            cols.push((mind, [false; WORDMARK_PX_ROWS]));
        }
        let color = if i < MIND_LETTERS { mind } else { ORANGE };
        for x in 0..rows[0].len() {
            let mut px = [false; WORDMARK_PX_ROWS];
            for (y, row) in rows.iter().enumerate() {
                px[y] = row.as_bytes()[x] == b'#';
            }
            cols.push((color, px));
        }
    }
    cols
}

/// The word `mindfork` as lines: `WORDMARK_ROWS` rows of `WORDMARK_COLS` cells.
///
/// `mind` takes the passed-in color (in the caller — `palette.text`), `fork` — the fixed brand
/// accent; the pixel-pair encoding is the same as the glyph's. No background is set —
/// the word has no backdrop.
pub fn wordmark_lines(mind: Color) -> Vec<Line<'static>> {
    let cols = wordmark_columns(mind);
    (0..WORDMARK_ROWS)
        .map(|row| {
            let (top, bottom) = (row as usize * 2, row as usize * 2 + 1);
            let spans = cols
                .iter()
                .map(|(color, px)| match (px[top], px[bottom]) {
                    (true, true) => Span::styled("█", Style::new().fg(*color)),
                    (true, false) => Span::styled("▀", Style::new().fg(*color)),
                    (false, true) => Span::styled("▄", Style::new().fg(*color)),
                    (false, false) => Span::raw(" "),
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

/// The horizontal "glyph + word" lockup — `LOCKUP_ROWS` rows.
///
/// The word is shorter than the glyph, so lockup rows outside its range carry only the glyph
/// (we don't pad with trailing spaces — rows are drawn left-aligned).
pub fn lockup_lines(mind: Color) -> Vec<Line<'static>> {
    let word = wordmark_lines(mind);
    logo_lines()
        .into_iter()
        .enumerate()
        .map(|(row, logo)| {
            let mut spans = logo.spans;
            let word_row = (row as u16)
                .checked_sub(WORDMARK_TOP_ROW)
                .and_then(|i| word.get(i as usize));
            if let Some(word_row) = word_row {
                spans.push(Span::raw(" ".repeat(LOCKUP_GAP as usize)));
                spans.extend(word_row.spans.iter().cloned());
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `<rect …/>` from the SVG icon: `(x, y, w, h, fill)`.
    fn parse_rects(svg: &str) -> Vec<(u8, u8, u8, u8, String)> {
        fn attr(tag: &str, name: &str) -> String {
            let key = format!("{name}=\"");
            let start = tag.find(&key).expect("attribute") + key.len();
            tag[start..].split('"').next().unwrap().to_string()
        }
        svg.split("<rect")
            .skip(1)
            .map(|tag| {
                let tag = tag.split('>').next().unwrap();
                (
                    attr(tag, "x").parse().unwrap(),
                    attr(tag, "y").parse().unwrap(),
                    attr(tag, "width").parse().unwrap(),
                    attr(tag, "height").parse().unwrap(),
                    attr(tag, "fill"),
                )
            })
            .collect()
    }

    fn hex(color: Color) -> String {
        match color {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            other => panic!("expected Rgb, got {other:?}"),
        }
    }

    /// A gate against code/asset drift: the `GLYPH` table must match
    /// `artwork/mindfork-icon-transparent.svg` — the single source of geometry.
    #[test]
    fn glyph_matches_artwork_svg() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/artwork/mindfork-icon-transparent.svg"
        );
        let svg = std::fs::read_to_string(path).expect("the icon is in place");
        let from_svg = parse_rects(&svg);
        let from_code: Vec<_> = GLYPH
            .iter()
            .map(|(x, y, w, h, c)| (*x, *y, *w, *h, hex(*c)))
            .collect();
        assert_eq!(
            from_svg, from_code,
            "GLYPH diverged from artwork/mindfork-icon-transparent.svg — \
             update the table or the asset"
        );
    }

    /// The 16×16 grid and ink bounds in the SVG match what's baked into the constants.
    #[test]
    fn ink_bounds_match_svg_viewbox() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/artwork/mindfork-icon-transparent.svg"
        );
        let svg = std::fs::read_to_string(path).expect("the icon is in place");
        assert!(
            svg.contains("viewBox=\"0 0 16 16\""),
            "the grid isn't 16×16"
        );
        let rects = parse_rects(&svg);
        let (min_x, max_x) = (
            rects.iter().map(|r| r.0).min().unwrap(),
            rects.iter().map(|r| r.0 + r.2).max().unwrap(),
        );
        let (min_y, max_y) = (
            rects.iter().map(|r| r.1).min().unwrap(),
            rects.iter().map(|r| r.1 + r.3).max().unwrap(),
        );
        assert_eq!((min_x, max_x), INK_X);
        assert_eq!((min_y, max_y), INK_Y);
        // The ink height is even — otherwise half-block glyphs won't land evenly into rows.
        assert_eq!((max_y - min_y) % 2, 0);
    }

    #[test]
    fn lines_have_expected_shape() {
        let lines = logo_lines();
        assert_eq!(lines.len(), LOGO_ROWS as usize);
        assert_eq!(LOGO_ROWS, 6);
        assert_eq!(LOGO_COLS, 10);
        for line in &lines {
            assert_eq!(line.spans.len(), LOGO_COLS as usize);
            // Each cell — exactly one column.
            for span in &line.spans {
                assert_eq!(span.content.chars().count(), 1);
            }
        }
    }

    /// The (orange) stem runs solid through every row — the glyph hasn't "fallen apart".
    #[test]
    fn orange_trunk_present_in_every_row() {
        for (i, line) in logo_lines().iter().enumerate() {
            let has_orange = line
                .spans
                .iter()
                .any(|s| s.style.fg == Some(ORANGE) || s.style.bg == Some(ORANGE));
            assert!(has_orange, "row {i} has no stem");
        }
    }

    /// Only WGL4-safe glyphs are used (conhost compatibility mode).
    #[test]
    fn uses_only_wgl4_block_glyphs() {
        for line in logo_lines().into_iter().chain(lockup_lines(Color::White)) {
            for span in line.spans {
                let ch = span.content.chars().next().unwrap();
                assert!(
                    matches!(ch, '█' | '▀' | '▄' | ' '),
                    "unexpected glyph {ch:?}"
                );
            }
        }
    }

    /// The wordmark font is intact: the word is correct, every letter has a rectangular
    /// matrix and at least one pixel (otherwise a letter would "disappear" silently).
    #[test]
    fn wordmark_font_is_well_formed() {
        let word: String = WORDMARK.iter().map(|(c, _)| *c).collect();
        assert_eq!(word, "mindfork");
        for (ch, rows) in WORDMARK {
            let w = rows[0].len();
            assert!(w > 0, "letter {ch:?} has zero width");
            for row in rows {
                assert_eq!(row.len(), w, "letter {ch:?} has rows of differing width");
                assert!(
                    row.bytes().all(|b| b == b'#' || b == b'.' || b == b' '),
                    "letter {ch:?} has a stray character in its matrix"
                );
            }
            assert!(
                rows.iter().any(|r| r.contains('#')),
                "letter {ch:?} is empty"
            );
        }
    }

    /// The word draws at its declared size, and `mind`/`fork` — in different colors
    /// ("mind" is theme-dependent, "fork" — the fixed brand accent).
    #[test]
    fn wordmark_has_expected_size_and_split() {
        const MIND: Color = Color::Rgb(0xe4, 0xe4, 0xe7);
        let lines = wordmark_lines(MIND);
        assert_eq!(lines.len(), WORDMARK_ROWS as usize);
        assert_eq!(WORDMARK_ROWS, 4);
        for line in &lines {
            assert_eq!(line.spans.len(), WORDMARK_COLS as usize);
        }
        // The word's width = the sum of letters + the spacing between them.
        let letters: u16 = WORDMARK.iter().map(|(_, r)| r[0].len() as u16).sum();
        assert_eq!(
            WORDMARK_COLS,
            letters + WORDMARK_TRACKING * (WORDMARK.len() as u16 - 1)
        );
        // "fork" starts right where "mind" ends, including its trailing spacing.
        let mind_cols: usize = WORDMARK
            .iter()
            .take(MIND_LETTERS)
            .map(|(_, r)| r[0].len() + WORDMARK_TRACKING as usize)
            .sum();
        let row = &lines[1]; // the x-height row: every letter has ink there
        assert!(
            row.spans[..mind_cols]
                .iter()
                .all(|s| s.style.fg != Some(ORANGE)),
            "\"mind\" must not be the accent color"
        );
        assert!(
            row.spans[mind_cols..]
                .iter()
                .any(|s| s.style.fg == Some(ORANGE)),
            "\"fork\" is drawn with the fixed brand accent"
        );
    }

    /// A lockup is the glyph on the left and the word on the right: the first `LOGO_COLS`
    /// columns match a standalone glyph byte-for-byte, the word doesn't overlap it.
    #[test]
    fn lockup_places_wordmark_right_of_glyph() {
        let logo = logo_lines();
        let lockup = lockup_lines(Color::White);
        assert_eq!(lockup.len(), LOCKUP_ROWS as usize);
        for (row, (with_word, glyph)) in lockup.iter().zip(&logo).enumerate() {
            assert_eq!(
                with_word.spans[..LOGO_COLS as usize],
                glyph.spans[..],
                "row {row}: the glyph changed"
            );
            let width: usize = with_word
                .spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum();
            let has_word =
                (WORDMARK_TOP_ROW..WORDMARK_TOP_ROW + WORDMARK_ROWS).contains(&(row as u16));
            assert_eq!(
                has_word,
                width > LOGO_COLS as usize,
                "row {row}: the word is where it wasn't expected (or vice versa)"
            );
        }
        assert_eq!(LOCKUP_COLS, LOGO_COLS + LOCKUP_GAP + WORDMARK_COLS);
    }
}
