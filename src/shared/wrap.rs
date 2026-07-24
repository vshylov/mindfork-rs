//! Word wrapping for widgets that own their scrolling/cursor (`message_feed`,
//! `input_box`). See ADR 0001: widgets own rendering, so we wrap lines ourselves
//! — ahead of time, before `Paragraph` — so the number of visual rows matches
//! the number of lines (otherwise scroll math and cursor position break).
//!
//! Width is measured in terminal columns via `unicode-width` (Cyrillic/Latin
//! = 1, emoji/CJK = 2), not in characters — otherwise wide characters "spill"
//! past the edge.
//!
//! **A deliberate width limitation — ZWJ families and flags.** [`width_at`]
//! fixes the two common emoji-cluster cases (the U+FE0F presentation selector
//! and the skin-tone modifier), but **ZWJ sequences** (`👨‍👩‍👧` = base+ZWJ+base+…
//! → 2+0+2+0+2 = 6 columns by the model vs the ~2 the terminal draws) and
//! **flags** (a pair of regional indicators → 4 vs ~2) are counted per scalar.
//! There is no "right" answer here: terminals draw such clusters differently
//! (monospacing of ZWJ emoji is not guaranteed), so there is nothing to tune the
//! model against. What matters is that **the cursor and navigation move by
//! grapheme clusters** ([`prev_boundary`]/[`next_boundary`], UAX #29) — only the
//! width diverges, not positioning/deletion. See spec §11.5.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// The "emoji presentation" selector (U+FE0F): turns a default-text character
/// (e.g. ❤ U+2764, width 1 per `unicode-width`) into an emoji the terminal draws
/// 2 columns wide. See [`width_at`].
const EMOJI_VS: char = '\u{FE0F}';

/// Character width in terminal columns per `unicode-width` (control → 0).
/// **Context-independent** — for emoji clusters (base + variation
/// selector/modifier) use [`width_at`]/[`display_width`], which account for
/// neighbors.
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Skin-tone modifier (U+1F3FB..U+1F3FF): attaches to the preceding emoji and
/// **adds no** columns (the base is already width 2). On its own `unicode-width`
/// counts it as width 2, so without the correction `👍🏽` would measure as 4
/// columns instead of 2.
fn is_emoji_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Regional indicator (U+1F1E6..U+1F1FF): a pair of these forms one flag emoji.
/// Needed by [`hard_break`] so a flag is not cut between visual rows (the second
/// indicator of the pair must not "drift" to the start of the next row).
fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Width of `chars[i]` in columns **accounting for the emoji cluster**. Causal
/// (looks only at the previous character), so the cumulative width of any prefix
/// is correct — needed both for wrapping and for cursor positioning:
/// - the U+FE0F emoji-presentation selector adds the missing column to the
///   preceding "text" character (`❤` width 1 + U+FE0F = 2 columns);
/// - the skin-tone modifier adds no columns (the base emoji is already width 2).
///
/// Terminals (Windows Terminal etc.) draw such clusters 2 columns wide; without
/// the correction the cursor "drifts" from the text on emoji (see spec §11.5).
pub fn width_at(chars: &[char], i: usize) -> usize {
    let c = chars[i];
    if is_emoji_modifier(c) {
        return 0;
    }
    if c == EMOJI_VS {
        let prev = if i > 0 { char_width(chars[i - 1]) } else { 0 };
        // Bring the preceding character up to width 2 (for an already-wide one adds 0).
        return 2usize.saturating_sub(prev);
    }
    char_width(c)
}

/// Total width of a character slice in columns — accounting for emoji clusters
/// (see [`width_at`]).
pub fn display_width(chars: &[char]) -> usize {
    (0..chars.len()).map(|i| width_at(chars, i)).sum()
}

/// Grapheme-cluster boundary **to the left** of position `col` (character
/// index): the start of the cluster the cursor is in/before. The cursor and
/// deletion must move by clusters, not by Unicode scalars — otherwise `❤️`
/// (`❤`+U+FE0F), `👍🏽` (emoji + skin-tone modifier), ZWJ sequences and flags are
/// traversed/deleted by halves (the cursor lands in the middle of an emoji,
/// Backspace leaves an "orphaned" variation selector). UAX #29 (extended
/// grapheme clusters) via `unicode-segmentation`. See spec §11.5.
///
/// Streams boundaries, stopping at the first `≥ col` — does not collect all
/// boundaries into a `Vec` (only one neighbor is needed, called on every
/// `←`/`Backspace`).
pub fn prev_boundary(chars: &[char], col: usize) -> usize {
    let s: String = chars.iter().collect();
    let mut prev = 0;
    let mut acc = 0;
    for g in s.graphemes(true) {
        acc += g.chars().count();
        if acc >= col {
            break;
        }
        prev = acc;
    }
    prev
}

/// Grapheme-cluster boundary **to the right** of position `col` (mirror of
/// [`prev_boundary`]) — the end of the cluster the cursor is on. Streams
/// boundaries, returning the first `> col`.
pub fn next_boundary(chars: &[char], col: usize) -> usize {
    let s: String = chars.iter().collect();
    let mut acc = 0;
    for g in s.graphemes(true) {
        acc += g.chars().count();
        if acc > col {
            return acc;
        }
    }
    chars.len()
}

/// Nearest grapheme-cluster boundary **at position `col` or to its left**
/// (character index): the largest boundary `≤ col`. If `col` is already a
/// boundary, returns `col`. Needed by navigation/wrapping so the cursor and
/// break points do not land in the middle of an emoji cluster (`❤️` = base +
/// variation selector, a flag = a pair of indicators) — otherwise
/// `insert`/`backspace` would split the cluster (an orphaned variation
/// selector). UAX #29. Streams boundaries, advancing until the next one exceeds
/// `col`. See spec §11.5.
pub fn snap_boundary(chars: &[char], col: usize) -> usize {
    let s: String = chars.iter().collect();
    let mut acc = 0;
    for g in s.graphemes(true) {
        let next = acc + g.chars().count();
        if next > col {
            break;
        }
        acc = next;
    }
    acc
}

/// Hard break point for a long word at index `i` (the character that overflowed
/// the row width), snapped to a grapheme-cluster boundary — so an emoji with a
/// variation selector (`❤️`) or a flag (a pair of regional indicators) is not
/// cut between rows. **Guarantees progress**: the result is always `> start`
/// (otherwise `wrap_ranges` would loop). Fast path: if the character at the
/// break does not "continue" a cluster (ordinary text/Cyrillic/CJK), returns `i`
/// without examining boundaries — otherwise wrapping long ASCII "words" would be
/// O(n²).
fn hard_break(chars: &[char], start: usize, i: usize) -> usize {
    let c = chars[i];
    if c != EMOJI_VS && !is_regional_indicator(c) {
        return i;
    }
    let b = snap_boundary(chars, i);
    if b > start {
        b
    } else {
        next_boundary(chars, start)
    }
}

/// Splits a logical character line into visual rows no wider than `width`.
///
/// Wrapping prefers word boundaries (after spaces); a word longer than the width
/// is broken by characters. At least one character goes into each row (guards
/// against looping on a character wider than `width`). Returns index ranges
/// `[start, end)` of the source slice, one per row. For an empty slice — one
/// empty row `(0, 0)` (preserves blank lines as visual rows).
pub fn wrap_ranges(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    if width == 0 {
        return vec![(0, chars.len())];
    }
    let n = chars.len();
    let mut rows = Vec::new();
    let mut start = 0;
    while start < n {
        let mut w = 0;
        let mut i = start;
        // Index of the last space that fit in the row (soft-wrap candidate).
        let mut last_ws: Option<usize> = None;
        let end = loop {
            if i >= n {
                break n;
            }
            let cw = width_at(chars, i);
            // Always take the first character of a row, even if wider than `width`.
            if w + cw > width && i > start {
                if chars[i].is_whitespace() {
                    // Exactly on a word boundary: the whitespace "tail" can be
                    // "spilled" past the edge (it is invisible) and kept in this
                    // row — then the next row starts with a word, not a space.
                    while i < n && chars[i].is_whitespace() {
                        i += 1;
                    }
                    break i;
                }
                break match last_ws {
                    // soft wrap after the last space
                    Some(ws) => ws + 1,
                    // word longer than the width — hard break by characters (on a
                    // cluster boundary, so `❤️`/a flag is not cut between rows)
                    None => hard_break(chars, start, i),
                };
            }
            if chars[i].is_whitespace() {
                last_ws = Some(i);
            }
            w += cw;
            i += 1;
        };
        rows.push((start, end));
        start = end;
    }
    rows
}

/// Wraps a styled line into visual rows of width `width`, preserving span styles
/// (markdown highlighting, spellcheck underlines) and the line style/alignment.
pub fn wrap_line(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    // Expand spans into per-character (char, style) — easier to wrap that way.
    let mut chars: Vec<char> = Vec::new();
    let mut styles: Vec<Style> = Vec::new();
    for span in &line.spans {
        for c in span.content.chars() {
            chars.push(c);
            styles.push(span.style);
        }
    }
    wrap_ranges(&chars, width)
        .into_iter()
        .map(|(s, e)| {
            // Reassemble the row's spans, merging adjacent chars of equal style.
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut k = s;
            while k < e {
                let st = styles[k];
                let mut buf = String::new();
                while k < e && styles[k] == st {
                    buf.push(chars[k]);
                    k += 1;
                }
                spans.push(Span::styled(buf, st));
            }
            let mut out = Line::from(spans);
            out.style = line.style;
            out.alignment = line.alignment;
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// Convenience helper: wraps a line and collects the text of each row.
    fn wrap_text(s: &str, width: usize) -> Vec<String> {
        let cs = chars(s);
        wrap_ranges(&cs, width)
            .into_iter()
            .map(|(a, b)| cs[a..b].iter().collect())
            .collect()
    }

    #[test]
    fn empty_line_is_one_empty_row() {
        assert_eq!(wrap_ranges(&[], 10), vec![(0, 0)]);
    }

    #[test]
    fn short_line_fits_in_one_row() {
        assert_eq!(wrap_text("привет", 10), vec!["привет"]);
    }

    #[test]
    fn wraps_on_word_boundary() {
        // Three Cyrillic words at width 8: both fit (4+1+3=8); the separating
        // space stays at the end of the row (invisible), the next row starts
        // with a word (the fixture's widths drive the expected split).
        assert_eq!(wrap_text("один два три", 8), vec!["один два ", "три"]);
    }

    #[test]
    fn long_word_is_hard_broken() {
        // a word longer than the width is broken by characters
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn emoji_presentation_selector_makes_width_two() {
        // ❤ (U+2764, width 1 per unicode-width) + U+FE0F → terminal draws 2 columns.
        let cs = chars("❤\u{FE0F}");
        assert_eq!(display_width(&cs), 2);
        // Without the selector it stays text (width 1).
        assert_eq!(display_width(&chars("❤")), 1);
        // Cumulative width is causal: a prefix of a single ❤ = 1, with selector = 2.
        assert_eq!(width_at(&cs, 0), 1);
        assert_eq!(width_at(&cs, 1), 1);
    }

    #[test]
    fn skin_tone_modifier_adds_no_columns() {
        // 👍 (width 2) + skin-tone modifier 🏽 → a single emoji of width 2, not 4.
        assert_eq!(display_width(&chars("👍🏽")), 2);
        // A supplementary-plane emoji without a modifier — ordinary 2 columns.
        assert_eq!(display_width(&chars("😊")), 2);
    }

    #[test]
    fn selector_after_wide_emoji_adds_nothing() {
        // An already-wide emoji + U+FE0F (a redundant selector) stays width 2.
        let cs = chars("😊\u{FE0F}");
        assert_eq!(display_width(&cs), 2);
    }

    #[test]
    fn grapheme_boundaries_group_emoji_clusters() {
        // "a❤️b👍🏽": a | ❤+U+FE0F | b | 👍+modifier → boundaries 0,1,3,4,6.
        let cs = chars("a❤\u{FE0F}b👍🏽");
        assert_eq!(cs.len(), 6);
        // left of the end (6) — the start of the 👍🏽 cluster (4)
        assert_eq!(prev_boundary(&cs, 6), 4);
        // left of 4 — 'b' (3)
        assert_eq!(prev_boundary(&cs, 3), 1);
        // right of 1 — the end of ❤️ (3)
        assert_eq!(next_boundary(&cs, 1), 3);
        // a single scalar emoji 😊 — ordinary boundary ±1
        let e = chars("😊x");
        assert_eq!(next_boundary(&e, 0), 1);
        assert_eq!(prev_boundary(&e, 1), 0);
    }

    #[test]
    fn snap_boundary_lands_on_cluster_start() {
        // "❤️x" = ❤(U+2764) + U+FE0F + x → cluster boundaries 0, 2, 3.
        let cs = chars("❤\u{FE0F}x");
        assert_eq!(cs.len(), 3);
        assert_eq!(snap_boundary(&cs, 0), 0);
        // index 1 — middle of the ❤️ cluster → snap to its start (0)
        assert_eq!(snap_boundary(&cs, 1), 0);
        assert_eq!(snap_boundary(&cs, 2), 2);
        assert_eq!(snap_boundary(&cs, 3), 3);
        // past the end — end of string
        assert_eq!(snap_boundary(&cs, 99), 3);
        // pure ASCII — every position is already a boundary (snap is identity)
        let ascii = chars("abcd");
        for i in 0..=4 {
            assert_eq!(snap_boundary(&ascii, i), i);
        }
    }

    #[test]
    fn hard_break_keeps_vs16_cluster_together() {
        // A long "word" without spaces where the width break lands exactly on the
        // U+FE0F selector: without a snap the base ❤ would stay in this row and
        // the variation selector would "drift" to the start of the next.
        // "aa❤️bb" at width 3: "aa" | "❤️b" | "b".
        let cs = chars("aa❤\u{FE0F}bb");
        assert_eq!(cs.len(), 6);
        let rows = wrap_ranges(&cs, 3);
        // no row starts with a lone U+FE0F selector
        for &(s, _) in &rows {
            assert_ne!(cs[s], '\u{FE0F}', "row started with an orphaned selector");
        }
        // the ❤️ cluster (indices 2,3) is entirely in one row
        assert!(
            rows.iter().any(|&(s, e)| s <= 2 && e >= 4),
            "❤️ split across rows: {rows:?}"
        );
    }

    #[test]
    fn hard_break_keeps_flag_together() {
        // Flag 🇬🇧 = a pair of regional indicators (2 scalars). On a hard break of
        // a long word the pair must not drift apart across rows.
        let cs = chars("aa🇬🇧aa");
        // width 2 → "aa" | 🇬🇧 (whole, wider than the row — taken as the first
        // cluster) | "aa"
        let rows = wrap_ranges(&cs, 2);
        assert!(
            rows.iter().any(|&(s, e)| s == 2 && e == 4),
            "flag split: {rows:?}"
        );
    }

    #[test]
    fn wide_chars_count_two_columns() {
        // emoji of width 2: at width 3 one fits plus a space
        let cs = chars("🔧🔧");
        // each emoji = 2 columns, width 3 → one per row
        assert_eq!(wrap_ranges(&cs, 3), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn single_wide_char_never_loops() {
        // a character wider than the width is still placed (one per row)
        let cs = chars("🔧");
        assert_eq!(wrap_ranges(&cs, 1), vec![(0, 1)]);
    }

    #[test]
    fn zero_width_returns_whole_line() {
        let cs = chars("abc");
        assert_eq!(wrap_ranges(&cs, 0), vec![(0, 3)]);
    }

    #[test]
    fn wrap_line_preserves_styles() {
        use ratatui::style::Stylize;
        // First span red, second plain; Cyrillic fixture, wrap at width 6.
        let line = Line::from(vec![Span::raw("раз ").red(), Span::raw("два три")]);
        let rows = wrap_line(&line, 6);
        // collect all the text back — content is not lost
        let joined: String = rows
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(joined, "раз два три");
        // the first row kept the red span
        assert!(rows[0].spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn wrap_line_keeps_empty_row_for_blank_line() {
        let line = Line::from("");
        let rows = wrap_line(&line, 10);
        assert_eq!(rows.len(), 1);
    }
}
