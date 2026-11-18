//! The feed's own text selection — the arithmetic of it
//! (docs/research/mouse-always-captured.md §5.3–§5.4, spec §11.3): where a
//! point of a selection sits, what of a row is chrome, which cells a selection
//! paints and the text a copy takes.
//!
//! The state — the selection, the gesture making it, the frame it is read
//! against — is [`MessageFeed`](super::message_feed::MessageFeed)'s; this
//! module only reads rows, through [`FeedRows`], so it is tested on rows
//! without drawing them.
//!
//! **What is copied is what is drawn** (research F4): rendered markdown — no
//! `**`, a code block without its fences, a table in box-drawing characters —
//! less the chrome a row's [`RowFact`] names, with the rows of one logical line
//! joined back into it. The source of a whole conversation is `F5` / `/copy`.

use std::ops::Range;

use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;

use crate::shared::markdown::LineMark;
use crate::shared::mouse::SelectUnit;
use crate::shared::wrap::{self, CharClass};

/// What a copy needs to know about one row of a feed block that its text does
/// not say. Made where the row is made (`message_feed::build_message_block`)
/// and cached beside it: after that the row is a bare `Line`, and which of its
/// cells are the rail, or whether the line above it continues into it, cannot
/// be told from the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowFact {
    /// Columns of chrome the row starts with — the rail, and a prefix drawn
    /// as text after it (a quote's `> `, a gutter, a tool card's indent:
    /// [`LineMark::prefix`]). A selection is never painted on them and a copy
    /// never takes them. 0 on a railless row: the separator under a message,
    /// the compaction divider.
    pub lead: u16,
    /// The row continues the logical line of the row above it: the feed's
    /// wrap, or a builder's ([`LineMark::continues`]), broke the line there. A
    /// copy joins the two back into one line ([`Self::glue`] says how).
    pub soft: bool,
    /// How a copy joins this row to the one above, when it is soft: `None` —
    /// as they are, the feed's own wrap having kept the whitespace it broke at
    /// on the upper row; `Some(n)` — a renderer trimmed the upper row at the
    /// wrap and padded it (a code block, squared off for its background), so
    /// its trailing whitespace is padding, and `n` spaces stood at the break.
    pub glue: Option<u8>,
    /// Columns of chrome the row ends with — a message's number at the right
    /// end of its header's row, with the padding that puts it there
    /// (docs/research/chat-branches.md §3.5). Like [`Self::lead`]: never
    /// painted, never copied. 0 on every other row.
    pub tail: u16,
}

impl RowFact {
    /// The fact of piece `k` of a line the block's wrap split, under a rail
    /// `rail` columns wide: the first piece carries the line's own prefix and
    /// what it continued; every later one starts at the rail — the wrap does
    /// not repeat a prefix — and continues the piece above.
    pub fn of_piece(rail: u16, mark: LineMark, k: usize) -> Self {
        if k == 0 {
            Self {
                lead: rail + mark.prefix,
                soft: mark.continues,
                glue: mark.glue,
                tail: 0,
            }
        } else {
            Self {
                lead: rail,
                soft: true,
                glue: None,
                tail: 0,
            }
        }
    }
}

/// A cell of the feed's content: the block (one message's rows), the row
/// within the block, the column within the row — 0 is the panel's first
/// column, the rail's. Ordered as the text reads: block, row, column.
///
/// Anchored to blocks rather than to rows of the whole feed, so that a message
/// or a note appended below leaves every point above it where it was
/// (research §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeedPoint {
    pub block: usize,
    pub row: usize,
    pub col: u16,
}

impl FeedPoint {
    pub fn new(block: usize, row: usize, col: u16) -> Self {
        Self { block, row, col }
    }
}

/// The feed's rows as they are drawn, block by block, each with its
/// [`RowFact`].
pub trait FeedRows {
    fn block_count(&self) -> usize;
    fn row_count(&self, block: usize) -> usize;
    /// `row` of `block` — both in range.
    fn row(&self, block: usize, row: usize) -> (&Line<'static>, RowFact);
}

/// One drawn grapheme cluster of a row.
#[derive(Debug, Clone)]
struct Glyph {
    /// The first column it covers.
    col: u16,
    /// How many columns: two for a wide glyph.
    width: u16,
    /// Its text, as a byte range of the row's text.
    bytes: Range<usize>,
    /// What a double click takes it with ([`CharClass`]).
    class: CharClass,
}

impl Glyph {
    /// The column past its last cell.
    fn end(&self) -> u16 {
        self.col.saturating_add(self.width)
    }

    /// Whether it covers any column of `from..=to` — a wide glyph is in when
    /// either of its cells is (research §5.4).
    fn meets(&self, from: u16, to: u16) -> bool {
        self.col <= to && self.end().max(self.col.saturating_add(1)) > from
    }
}

/// A row's text (its spans concatenated) and its glyphs, in order, at the
/// columns the feed draws them in.
fn glyphs(line: &Line<'_>) -> (String, Vec<Glyph>) {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let mut out = Vec::new();
    let mut col: u16 = 0;
    for (at, g) in text.grapheme_indices(true) {
        let chars: Vec<char> = g.chars().collect();
        let width = wrap::display_width(&chars) as u16;
        out.push(Glyph {
            col,
            width,
            bytes: at..at + g.len(),
            class: CharClass::of(chars[0]),
        });
        col = col.saturating_add(width);
    }
    (text, out)
}

/// The glyphs of the row's content: past its lead and before its tail, which
/// is counted back from the row's last drawn cell.
fn content(glyphs: &[Glyph], fact: RowFact) -> impl DoubleEndedIterator<Item = &Glyph> {
    let limit = glyphs
        .last()
        .map_or(0, Glyph::end)
        .saturating_sub(fact.tail);
    glyphs
        .iter()
        .filter(move |g| g.col >= fact.lead && g.col < limit)
}

/// The glyphs of the row's content that meet `from..=to`.
fn taken(glyphs: &[Glyph], fact: RowFact, from: u16, to: u16) -> impl Iterator<Item = &Glyph> {
    content(glyphs, fact).filter(move |g| g.meets(from, to))
}

/// The columns a selection paints on one row: `from..=to` (the row's part of
/// the selection) less the lead and the tail, and less the content's trailing
/// whitespace — a
/// code block's padding and the space at a wrap stay unpainted, so the
/// highlight ends on the row's last glyph (research §5.4). Half-open; `None`
/// when nothing on the row is painted.
pub fn painted(line: &Line<'_>, fact: RowFact, from: u16, to: u16) -> Option<Range<u16>> {
    let (_, glyphs) = glyphs(line);
    let content_end = content(&glyphs, fact)
        .rev()
        .find(|g| g.class != CharClass::Space)
        .map(Glyph::end)?;
    let mut cells = taken(&glyphs, fact, from, to).filter(|g| g.col < content_end);
    let first = cells.next()?;
    let last = cells.last().unwrap_or(first);
    Some(first.col..last.end())
}

/// The columns of a row's text: from past its lead to the end of its last
/// glyph that is not whitespace, the tail left out. `None` for a row with nothing on it — a blank
/// row, the separator under a message. A double or a triple click selects only
/// where it lands on these; beside the text it is a plain click.
pub fn content_cols(line: &Line<'_>, fact: RowFact) -> Option<Range<u16>> {
    let (_, glyphs) = glyphs(line);
    let end = content(&glyphs, fact)
        .rev()
        .find(|g| g.class != CharClass::Space)
        .map(Glyph::end)?;
    (fact.lead < end).then_some(fact.lead..end)
}

/// The part of the selection on the row at `(block, row)`: its columns,
/// inclusive. The first row starts at the selection's start, the last ends at
/// its end, and every row between is whole.
fn row_cols(start: FeedPoint, end: FeedPoint, block: usize, row: usize) -> (u16, u16) {
    let from = if (block, row) == (start.block, start.row) {
        start.col
    } else {
        0
    };
    let to = if (block, row) == (end.block, end.row) {
        end.col
    } else {
        u16::MAX
    };
    (from, to)
}

/// Every row of `start..=end`, in order, with its columns ([`row_cols`]).
fn each_row(
    rows: &impl FeedRows,
    start: FeedPoint,
    end: FeedPoint,
) -> impl Iterator<Item = (usize, usize, u16, u16)> + '_ {
    (start.block..=end.block.min(rows.block_count().saturating_sub(1)))
        .flat_map(move |block| {
            let first = if block == start.block { start.row } else { 0 };
            let last = if block == end.block {
                end.row
            } else {
                rows.row_count(block).saturating_sub(1)
            };
            let count = rows.row_count(block);
            (first..=last)
                .filter(move |&row| row < count)
                .map(move |row| (block, row))
        })
        .map(move |(block, row)| {
            let (from, to) = row_cols(start, end, block, row);
            (block, row, from, to)
        })
}

/// The columns [`painted`] on the row at `(block, row)` for the selection
/// `start..=end`, or `None` when the row is outside it or nothing on it is.
pub fn painted_on(
    rows: &impl FeedRows,
    start: FeedPoint,
    end: FeedPoint,
    block: usize,
    row: usize,
) -> Option<Range<u16>> {
    if (block, row) < (start.block, start.row) || (block, row) > (end.block, end.row) {
        return None;
    }
    let (from, to) = row_cols(start, end, block, row);
    let (line, fact) = rows.row(block, row);
    painted(line, fact, from, to)
}

/// The text a copy takes from the selection `start..=end` (research §5.4):
/// each row's content cut to the selection's columns, the rows of one logical
/// line joined by nothing, the logical lines by `\n`, and the whitespace that
/// ends a logical line — a code block's padding — trimmed, as is the
/// whitespace the selection ends on. A selection that ends on the empty rows
/// under a message (the separator) does not end the copy on a newline.
pub fn text(rows: &impl FeedRows, start: FeedPoint, end: FeedPoint) -> String {
    let mut out = String::new();
    let mut picked: Vec<(usize, usize, String)> = Vec::new();
    for (block, row, from, to) in each_row(rows, start, end) {
        let (line, fact) = rows.row(block, row);
        let (text, glyphs) = glyphs(line);
        let piece: String = taken(&glyphs, fact, from, to)
            .map(|g| &text[g.bytes.clone()])
            .collect();
        picked.push((block, row, piece));
    }
    let last = picked.len().saturating_sub(1);
    for (i, (block, row, piece)) in picked.iter().enumerate() {
        // The next row continues this one's line when it is the same
        // block's next row and the wrap broke the line there.
        let next = (i < last && picked[i + 1].0 == *block).then(|| rows.row(*block, row + 1).1);
        if let Some(next) = next.filter(|f| f.soft) {
            match next.glue {
                None => out.push_str(piece),
                Some(n) => {
                    out.push_str(piece.trim_end());
                    out.extend(std::iter::repeat_n(' ', usize::from(n)));
                }
            }
        } else {
            out.push_str(piece.trim_end());
            if i < last {
                out.push('\n');
            }
        }
    }
    let kept = out.trim_end_matches('\n').len();
    out.truncate(kept);
    out
}

/// The word under `p` (research F5): the run of one [`CharClass`] holding the
/// glyph at its column — a word, a run of punctuation, or the whitespace
/// between them — within the row's content. A column past the row's end takes
/// the row's last run, one on the rail its first; a row with no content
/// selects nothing but its point.
pub fn word_bounds(rows: &impl FeedRows, p: FeedPoint) -> (FeedPoint, FeedPoint) {
    let (line, fact) = rows.row(p.block, p.row);
    let (_, glyphs) = glyphs(line);
    let content: Vec<&Glyph> = content(&glyphs, fact).collect();
    let Some(at) = content
        .iter()
        .rposition(|g| g.col <= p.col)
        .or((!content.is_empty()).then_some(0))
    else {
        return (p, p);
    };
    let class = content[at].class;
    let first = content[..at]
        .iter()
        .rposition(|g| g.class != class)
        .map_or(0, |i| i + 1);
    let last = content[at..]
        .iter()
        .position(|g| g.class != class)
        .map_or(content.len() - 1, |i| at + i - 1);
    (
        FeedPoint::new(p.block, p.row, content[first].col),
        FeedPoint::new(p.block, p.row, content[last].end().saturating_sub(1)),
    )
}

/// The logical line under `p` (research F5): the rows the wrap made of it,
/// from its first row's content to its last row's end.
pub fn line_bounds(rows: &impl FeedRows, p: FeedPoint) -> (FeedPoint, FeedPoint) {
    let mut first = p.row;
    while first > 0 && rows.row(p.block, first).1.soft {
        first -= 1;
    }
    let mut last = p.row;
    while last + 1 < rows.row_count(p.block) && rows.row(p.block, last + 1).1.soft {
        last += 1;
    }
    (
        FeedPoint::new(p.block, first, 0),
        FeedPoint::new(p.block, last, u16::MAX),
    )
}

/// The selection a gesture makes: from what its press took (`origin` — a
/// point, or the word or the line under it) to `head`, grown by the same unit,
/// so a drag after a double click selects whole words. Returns `(start, end)`
/// in reading order.
pub fn grow(
    rows: &impl FeedRows,
    unit: SelectUnit,
    origin: (FeedPoint, FeedPoint),
    head: FeedPoint,
) -> (FeedPoint, FeedPoint) {
    let (from, to) = match unit {
        SelectUnit::Char => (head, head),
        SelectUnit::Word => word_bounds(rows, head),
        SelectUnit::Line => line_bounds(rows, head),
    };
    if from < origin.0 {
        (from, origin.1)
    } else {
        (origin.0, to.max(origin.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    /// Blocks of rows, each row its text and its fact.
    struct Rows(Vec<Vec<(Line<'static>, RowFact)>>);

    impl FeedRows for Rows {
        fn block_count(&self) -> usize {
            self.0.len()
        }
        fn row_count(&self, block: usize) -> usize {
            self.0[block].len()
        }
        fn row(&self, block: usize, row: usize) -> (&Line<'static>, RowFact) {
            let (line, fact) = &self.0[block][row];
            (line, *fact)
        }
    }

    const RAILED: RowFact = RowFact {
        lead: 2,
        soft: false,
        glue: None,
        tail: 0,
    };
    const SOFT: RowFact = RowFact {
        lead: 2,
        soft: true,
        glue: None,
        tail: 0,
    };
    const BARE: RowFact = RowFact {
        lead: 0,
        soft: false,
        glue: None,
        tail: 0,
    };

    /// A railed row: the rail in a span of its own, as the feed draws it.
    fn railed(text: &str) -> Line<'static> {
        Line::from(vec![Span::raw("▌ "), Span::raw(text.to_string())])
    }

    fn rows(blocks: Vec<Vec<(Line<'static>, RowFact)>>) -> Rows {
        Rows(blocks)
    }

    fn pt(block: usize, row: usize, col: u16) -> FeedPoint {
        FeedPoint::new(block, row, col)
    }

    /// A paragraph the wrap broke in three: the copy is the line it was.
    #[test]
    fn a_wrapped_paragraph_copies_as_one_line() {
        let r = rows(vec![vec![
            (railed("the quick brown "), RAILED),
            (railed("fox jumps over "), SOFT),
            (railed("the lazy dog"), SOFT),
            (Line::from(""), BARE),
        ]]);
        assert_eq!(
            text(&r, pt(0, 0, 0), pt(0, 3, 0)),
            "the quick brown fox jumps over the lazy dog"
        );
        // From the middle of one row to the middle of another.
        assert_eq!(text(&r, pt(0, 0, 6), pt(0, 1, 6)), "quick brown fox j");
    }

    /// Rows of separate lines are joined by a newline, and each line's
    /// padding — a code block's background fill — is left behind.
    #[test]
    fn a_code_block_copies_without_its_padding_or_the_rail() {
        let r = rows(vec![vec![
            (railed("fn main() {       "), RAILED),
            (railed("    run();        "), RAILED),
            (railed("}                 "), RAILED),
        ]]);
        assert_eq!(
            text(&r, pt(0, 0, 0), pt(0, 2, u16::MAX)),
            "fn main() {\n    run();\n}"
        );
    }

    /// Across messages: the separator under one is an empty line between the
    /// two, the role header is text like any other.
    #[test]
    fn a_selection_across_blocks_keeps_their_rows_apart() {
        let r = rows(vec![
            vec![
                (railed("YOU"), RAILED),
                (railed("hello"), RAILED),
                (Line::from(""), BARE),
            ],
            vec![(railed("ASSISTANT"), RAILED), (railed("hi there"), RAILED)],
        ]);
        assert_eq!(
            text(&r, pt(0, 1, 0), pt(1, 1, 5)),
            "hello\n\nASSISTANT\nhi t"
        );
    }

    /// A wide glyph is taken when either of its cells is selected — at the
    /// start and at the end of a selection alike.
    #[test]
    fn a_wide_glyph_is_in_when_either_half_is() {
        // Columns: rail 0-1, `a` 2, `世` 3-4, `b` 5, `界` 6-7, `c` 8.
        let r = rows(vec![vec![(railed("a世b界c"), RAILED)]]);
        assert_eq!(text(&r, pt(0, 0, 4), pt(0, 0, 6)), "世b界");
        assert_eq!(text(&r, pt(0, 0, 3), pt(0, 0, 7)), "世b界");
        assert_eq!(painted_on(&r, pt(0, 0, 4), pt(0, 0, 6), 0, 0), Some(3..8));
    }

    /// The highlight runs from the content's first column to the row's last
    /// glyph: never on the rail, never on the padding after the text.
    #[test]
    fn the_highlight_skips_the_rail_and_the_padding() {
        let r = rows(vec![vec![
            (railed("first   "), RAILED),
            (railed("second"), RAILED),
            (railed("    "), RAILED),
        ]]);
        let (start, end) = (pt(0, 0, 0), pt(0, 2, u16::MAX));
        assert_eq!(painted_on(&r, start, end, 0, 0), Some(2..7));
        assert_eq!(painted_on(&r, start, end, 0, 1), Some(2..8));
        assert_eq!(painted_on(&r, start, end, 0, 2), None, "a blank row");
        assert_eq!(painted_on(&r, pt(0, 1, 0), end, 0, 0), None, "outside");
    }

    /// Rows a renderer trimmed and padded (a code block on its background):
    /// the padding is left out of the join, and the spaces the trim took at
    /// the break are put back. A row the feed wrapped keeps its whitespace and
    /// is joined as it is.
    #[test]
    fn a_padded_rows_join_puts_the_break_back_and_drops_the_padding() {
        let glued = |n| RowFact {
            lead: 2,
            soft: true,
            glue: Some(n),
            tail: 0,
        };
        let r = rows(vec![vec![
            (railed("cargo test     "), RAILED),
            (railed("--bin x        "), glued(1)),
            (railed("--nocapture    "), glued(2)),
            (railed("tail "), RAILED),
            (railed("end"), SOFT),
        ]]);
        assert_eq!(
            text(&r, pt(0, 0, 0), pt(0, 4, u16::MAX)),
            "cargo test --bin x  --nocapture\ntail end"
        );
    }

    /// A table is copied as drawn — box-drawing characters and all (F4).
    #[test]
    fn a_table_copies_as_drawn() {
        let r = rows(vec![vec![
            (railed("┌───┬───┐"), RAILED),
            (railed("│ a │ b │"), RAILED),
            (railed("└───┴───┘"), RAILED),
        ]]);
        assert_eq!(
            text(&r, pt(0, 0, 0), pt(0, 2, u16::MAX)),
            "┌───┬───┐\n│ a │ b │\n└───┴───┘"
        );
    }

    #[test]
    fn a_double_click_takes_the_run_under_the_pointer() {
        // Columns: rail 0-1, `say` 2-4, ` ` 5, `hello,` 6-11, ` ` 12, a Cyrillic word 13-15.
        let r = rows(vec![vec![(railed("say hello, мир"), RAILED)]]);
        assert_eq!(word_bounds(&r, pt(0, 0, 7)), (pt(0, 0, 6), pt(0, 0, 10)));
        assert_eq!(word_bounds(&r, pt(0, 0, 11)), (pt(0, 0, 11), pt(0, 0, 11)));
        assert_eq!(word_bounds(&r, pt(0, 0, 14)), (pt(0, 0, 13), pt(0, 0, 15)));
        // Past the row's end: its last run; on the rail: its first.
        assert_eq!(word_bounds(&r, pt(0, 0, 40)), (pt(0, 0, 13), pt(0, 0, 15)));
        assert_eq!(word_bounds(&r, pt(0, 0, 0)), (pt(0, 0, 2), pt(0, 0, 4)));
        let empty = rows(vec![vec![(Line::from(""), BARE)]]);
        assert_eq!(word_bounds(&empty, pt(0, 0, 3)), (pt(0, 0, 3), pt(0, 0, 3)));
    }

    /// A row's text runs from past the rail to its last glyph; a blank row,
    /// or one of spaces, has none.
    #[test]
    fn a_rows_text_is_past_the_rail_and_before_the_padding() {
        let (line, blank) = (railed("  code   "), railed("    "));
        assert_eq!(content_cols(&line, RAILED), Some(2..8));
        assert_eq!(content_cols(&blank, RAILED), None);
        assert_eq!(content_cols(&Line::from(""), BARE), None);
    }

    /// A header's row ends on its message's number: the padding and the
    /// number are the row's tail — not painted, not copied, not a word a
    /// double click takes (docs/research/chat-branches.md §3.5).
    #[test]
    fn a_rows_tail_is_chrome_like_its_rail() {
        // Columns: rail 0-1, `ASSISTANT` 2-10, padding 11-16, `#12` 17-19.
        let numbered = RowFact { tail: 9, ..RAILED };
        let header = || railed("ASSISTANT      #12");
        let r = rows(vec![vec![
            (header(), numbered),
            (railed("the reply"), RAILED),
        ]]);
        assert_eq!(
            text(&r, pt(0, 0, 0), pt(0, 1, u16::MAX)),
            "ASSISTANT\nthe reply",
            "a copy across the header holds no number"
        );
        assert_eq!(text(&r, pt(0, 0, 14), pt(0, 0, u16::MAX)), "");
        assert_eq!(painted(&header(), numbered, 0, u16::MAX), Some(2..11));
        assert_eq!(painted(&header(), numbered, 12, u16::MAX), None);
        assert_eq!(content_cols(&header(), numbered), Some(2..11));
        // On the number, as past a row's end: the row's last run.
        assert_eq!(word_bounds(&r, pt(0, 0, 18)), (pt(0, 0, 2), pt(0, 0, 10)));
        // The same row with no tail named reads to its end — the control.
        assert_eq!(content_cols(&header(), RAILED), Some(2..20));
    }

    #[test]
    fn a_triple_click_takes_the_logical_line() {
        let r = rows(vec![vec![
            (railed("before"), RAILED),
            (railed("one two "), RAILED),
            (railed("three"), SOFT),
            (railed("after"), RAILED),
        ]]);
        let (start, end) = line_bounds(&r, pt(0, 2, 3));
        assert_eq!((start, end), (pt(0, 1, 0), pt(0, 2, u16::MAX)));
        assert_eq!(text(&r, start, end), "one two three");
    }

    /// A drag after a double click grows by words, either way from the word
    /// it started on, which stays selected.
    #[test]
    fn a_drag_grows_by_the_unit_of_its_press() {
        let r = rows(vec![vec![(railed("alpha beta gamma"), RAILED)]]);
        let beta = word_bounds(&r, pt(0, 0, 9));
        assert_eq!(beta, (pt(0, 0, 8), pt(0, 0, 11)));
        let right = grow(&r, SelectUnit::Word, beta, pt(0, 0, 14));
        assert_eq!(text(&r, right.0, right.1), "beta gamma");
        let left = grow(&r, SelectUnit::Word, beta, pt(0, 0, 3));
        assert_eq!(text(&r, left.0, left.1), "alpha beta");
        let inside = grow(&r, SelectUnit::Word, beta, pt(0, 0, 10));
        assert_eq!(inside, beta);
        // A press a character at a time: from the press to the pointer,
        // whichever way the pointer went.
        let point = (pt(0, 0, 9), pt(0, 0, 9));
        assert_eq!(
            grow(&r, SelectUnit::Char, point, pt(0, 0, 4)),
            (pt(0, 0, 4), pt(0, 0, 9))
        );
    }
}
