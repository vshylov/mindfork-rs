//! Markdown — table layout and rendering (TableBuilder + render_table). Part
//! of module [`super`]; split out of the markdown.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 6).

use super::*;

/// Cell accumulator between `Table…` events.
pub(super) struct TableBuilder {
    /// Column alignment (from `:---:` markup).
    pub(super) alignments: Vec<Alignment>,
    /// Header cells.
    pub(super) head: Vec<Vec<Span<'static>>>,
    /// Body rows (each — a vector of cell-spans).
    pub(super) rows: Vec<Vec<Vec<Span<'static>>>>,
    /// The row currently being collected.
    pub(super) current_row: Vec<Vec<Span<'static>>>,
    /// The cell currently being collected (between `Start/End(TableCell)`).
    pub(super) current_cell: Option<Vec<Span<'static>>>,
}

// ---------- table layout ----------

/// Minimum "readable" column width (columns).
pub(super) const MIN_COL: usize = 5;
/// Upper bound on the minimum: a long word is allowed to break rather than
/// inflating min.
pub(super) const MAX_MIN: usize = 12;

/// Renders a table into feed lines. Column widths are fit to `width` (see
/// [`fit_columns`]); cell content wraps by word. If even the readable minimum
/// doesn't fit the columns, the table is drawn at its natural width and
/// **clipped** on the right edge of the panel (horizontal clipping).
///
/// `row_separators` — draw a horizontal separator (`├─┼─┤`) **between** body
/// rows (a "grid" look, [`RenderOpts::table_row_separators`]); no separator
/// after the last row (the table's bottom already closes with `└─┴─┘`).
pub(super) fn render_table(
    tb: &TableBuilder,
    width: usize,
    palette: &Palette,
    row_separators: bool,
) -> Vec<Line<'static>> {
    let ncols = tb
        .alignments
        .len()
        .max(tb.head.len())
        .max(tb.rows.iter().map(Vec::len).max().unwrap_or(0));
    if ncols == 0 {
        return Vec::new();
    }
    let aligns: Vec<Alignment> = (0..ncols)
        .map(|j| tb.alignments.get(j).copied().unwrap_or(Alignment::None))
        .collect();

    // Natural and minimum width per column.
    let mut desired = vec![0usize; ncols];
    let mut minw = vec![0usize; ncols];
    let mut visit = |cell: &[Span<'static>], j: usize| {
        let w = cell_width(cell);
        desired[j] = desired[j].max(w);
        let word = longest_word(cell).clamp(MIN_COL, MAX_MIN).min(w.max(1));
        minw[j] = minw[j].max(word);
    };
    for (j, cell) in tb.head.iter().enumerate() {
        visit(cell, j);
    }
    for row in &tb.rows {
        for (j, cell) in row.iter().enumerate() {
            visit(cell, j);
        }
    }
    // An empty column still gets a readable minimum.
    for j in 0..ncols {
        minw[j] = minw[j].max(MIN_COL.min(desired[j].max(1)));
    }

    // Width available for content = width minus borders/padding: `│` on the
    // left + per column (padding space + content + padding space + `│`).
    let chrome = 3 * ncols + 1;
    let avail = width.saturating_sub(chrome).max(ncols);
    let (widths, clip) = match fit_columns(&desired, &minw, avail) {
        Some(w) => (w, false),
        None => (desired.clone(), true),
    };

    let mut out = Vec::new();
    out.push(border_line(&widths, Border::Top));
    out.extend(render_row(&tb.head, &widths, &aligns, palette, true));
    out.push(border_line(&widths, Border::Mid));
    for (i, row) in tb.rows.iter().enumerate() {
        if row_separators && i > 0 {
            out.push(border_line(&widths, Border::Mid));
        }
        out.extend(render_row(row, &widths, &aligns, palette, false));
    }
    out.push(border_line(&widths, Border::Bottom));
    if clip {
        for line in &mut out {
            clip_line(line, width);
        }
    }
    out
}

/// Fits column widths via "water-fill": when space is short, narrow columns
/// keep their natural width, the remainder splits evenly among the wide ones
/// (with a floor of `minw`). `None` — even the minimums don't fit (needs
/// clipping).
pub(super) fn fit_columns(desired: &[usize], minw: &[usize], avail: usize) -> Option<Vec<usize>> {
    let total_desired: usize = desired.iter().sum();
    if total_desired <= avail {
        return Some(desired.to_vec());
    }
    let total_min: usize = minw.iter().sum();
    if total_min > avail {
        return None;
    }
    let n = desired.len();
    let mut w = minw.to_vec();
    let mut extra = avail - total_min;
    while extra > 0 {
        let wanting: Vec<usize> = (0..n).filter(|&j| w[j] < desired[j]).collect();
        if wanting.is_empty() {
            break;
        }
        for j in wanting {
            if extra == 0 {
                break;
            }
            w[j] += 1;
            extra -= 1;
        }
    }
    Some(w)
}

/// Cell width in columns (sum of span widths).
pub(super) fn cell_width(cell: &[Span<'static>]) -> usize {
    cell.iter()
        .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
        .sum()
}

/// Width of the longest "word" (non-whitespace run) in a cell.
pub(super) fn longest_word(cell: &[Span<'static>]) -> usize {
    let text: String = cell.iter().map(|s| s.content.as_ref()).collect();
    text.split_whitespace()
        .map(|w| wrap::display_width(&w.chars().collect::<Vec<_>>()))
        .max()
        .unwrap_or(0)
}

/// Kind of table horizontal border.
#[derive(Clone, Copy)]
pub(super) enum Border {
    Top,
    Mid,
    Bottom,
}

/// Builds a border line from column widths.
pub(super) fn border_line(widths: &[usize], kind: Border) -> Line<'static> {
    let (left, junction, right) = match kind {
        Border::Top => ('┌', '┬', '┐'),
        Border::Mid => ('├', '┼', '┤'),
        Border::Bottom => ('└', '┴', '┘'),
    };
    let mut s = String::new();
    s.push(left);
    for (j, w) in widths.iter().enumerate() {
        for _ in 0..(w + 2) {
            s.push('─');
        }
        s.push(if j + 1 == widths.len() {
            right
        } else {
            junction
        });
    }
    Line::from(s).add_modifier(Modifier::DIM)
}

/// Renders a table row (wrapping cells by column widths) into visual rows.
/// The header — bold.
pub(super) fn render_row(
    cells: &[Vec<Span<'static>>],
    widths: &[usize],
    aligns: &[Alignment],
    palette: &Palette,
    header: bool,
) -> Vec<Line<'static>> {
    let ncols = widths.len();
    // Wrap each cell by its width → rows of spans.
    let wrapped: Vec<Vec<Line<'static>>> = (0..ncols)
        .map(|j| {
            let empty: Vec<Span<'static>> = Vec::new();
            let cell = cells.get(j).unwrap_or(&empty);
            let styled: Vec<Span<'static>> = if header {
                cell.iter()
                    .map(|s| {
                        Span::styled(
                            s.content.to_string(),
                            s.style.add_modifier(Modifier::BOLD).fg(palette.accent),
                        )
                    })
                    .collect()
            } else {
                cell.clone()
            };
            wrap::wrap_line(&Line::from(styled), widths[j])
                .into_iter()
                .map(trim_row_trailing_ws)
                .collect()
        })
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);

    let mut rows = Vec::with_capacity(height);
    for r in 0..height {
        let mut spans: Vec<Span<'static>> = vec![Span::styled("│", Style::new().dim())];
        for j in 0..ncols {
            spans.push(Span::raw(" "));
            let empty = Line::default();
            let cell_row = wrapped[j].get(r).unwrap_or(&empty);
            spans.extend(pad_cell(cell_row, widths[j], aligns[j]));
            spans.push(Span::raw(" "));
            spans.push(Span::styled("│", Style::new().dim()));
        }
        rows.push(Line::from(spans));
    }
    rows
}

/// Strips trailing spaces from a cell's visual row. `wrap::wrap_ranges`
/// "spills" a word-boundary space past the row's edge (invisible in the
/// regular feed), but in a table these spaces are counted by [`pad_cell`] and
/// inflate the line **wider than the column** — then the feed's re-wrap
/// (`message_feed`) breaks the border. Trailing spaces inside a cell carry no
/// meaning (padding is re-added anyway), so it's safe to trim them,
/// guaranteeing a row ≤ `widths[j]`.
pub(super) fn trim_row_trailing_ws(line: Line<'static>) -> Line<'static> {
    let mut spans = line.spans;
    while let Some(last) = spans.last() {
        let trimmed = last.content.trim_end();
        if trimmed.len() == last.content.len() {
            break;
        }
        if trimmed.is_empty() {
            spans.pop();
        } else {
            let style = last.style;
            *spans.last_mut().unwrap() = Span::styled(trimmed.to_string(), style);
            break;
        }
    }
    let mut out = Line::from(spans);
    out.style = line.style;
    out.alignment = line.alignment;
    out
}

/// Pads a cell row with spaces to `width`, respecting alignment.
pub(super) fn pad_cell(line: &Line<'static>, width: usize, align: Alignment) -> Vec<Span<'static>> {
    let content: usize = line
        .spans
        .iter()
        .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
        .sum();
    let pad = width.saturating_sub(content);
    let (left, right) = match align {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    let mut spans = Vec::new();
    if left > 0 {
        spans.push(Span::raw(" ".repeat(left)));
    }
    spans.extend(line.spans.iter().cloned());
    if right > 0 {
        spans.push(Span::raw(" ".repeat(right)));
    }
    spans
}

/// Clips a line to `width` columns (a wide table's clip), adding "…".
pub(super) fn clip_line(line: &mut Line<'static>, width: usize) {
    let total: usize = line
        .spans
        .iter()
        .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
        .sum();
    if total <= width {
        return;
    }
    let budget = width.saturating_sub(1); // room for "…"
    let mut acc = 0usize;
    let mut new_spans: Vec<Span<'static>> = Vec::new();
    for span in &line.spans {
        let chars: Vec<char> = span.content.chars().collect();
        let w = wrap::display_width(&chars);
        if acc + w <= budget {
            acc += w;
            new_spans.push(span.clone());
            continue;
        }
        // partially fits — cut by characters
        let mut buf = String::new();
        for i in 0..chars.len() {
            let cw = wrap::width_at(&chars, i);
            if acc + cw > budget {
                break;
            }
            acc += cw;
            buf.push(chars[i]);
        }
        if !buf.is_empty() {
            new_spans.push(Span::styled(buf, span.style));
        }
        break;
    }
    new_spans.push(Span::styled("…", Style::new().dim()));
    *line = Line::from(new_spans);
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::*;

    #[test]
    fn table_wraps_cell_content() {
        // a long cell wraps into several rows without exceeding the width
        let long = "\
| A | Особенности |
| :--- | :--- |
| x | Самая высокая скорость на практике сортировки |";
        let w = 40;
        assert!(max_line_width(long, w) <= w);
        // several body lines → the wrap happened
        let lines = render(long, w, &Palette::default()).lines.len();
        assert!(lines >= 6, "expected the cell to wrap into several rows");
    }

    #[test]
    fn table_renders_box_and_content() {
        let collected = rendered_text(TABLE_MD);
        assert!(collected.contains('┌') && collected.contains('┼') && collected.contains('└'));
        assert!(collected.contains("Алгоритм"));
        assert!(collected.contains("QuickSort"));
        assert!(collected.contains("MergeSort"));
    }

    #[test]
    fn table_fits_panel_width() {
        // given enough width, the table doesn't exceed it
        for w in [40usize, 60, 80, 120] {
            let max = max_line_width(TABLE_MD, w);
            assert!(max <= w, "width {max} exceeded the panel {w}");
        }
    }

    /// A table with wrapping cells (as in the screenshot) must never exceed
    /// the width, **at any** panel size — otherwise the feed's re-wrap
    /// (`message_feed`) would break the border. A regression on a "spilled"
    /// space at a word boundary.
    #[test]
    fn wrapping_table_never_exceeds_any_width() {
        const WIDE: &str = "\
| Подход | Как работает | Минус |
| :--- | :--- | :--- |
| Стандартный Transformer Chain-of-Thought (o1) | Фиксированный проход Input → Output. Модель пишет рассуждения текстом в скрытый чат | Одинаковые затраты ресурсов на всё. Дорого по токенам, медленно, ограничено длиной текста. |
| Ваша идея (Recurrent ACT) | Итерации в скрытом пространстве (latent space) | Сложность в обучении (нужны новые методы градиентного спуска). |";
        for w in 30usize..=140 {
            let max = max_line_width(WIDE, w);
            assert!(max <= w, "at width {w} the table row grew to {max}");
        }
    }

    #[test]
    fn wide_table_is_clipped_to_width() {
        // a narrow panel: the table is clipped, but doesn't exceed the edge
        let narrow = 24;
        let max = max_line_width(TABLE_MD, narrow);
        assert!(
            max <= narrow,
            "width {max} exceeded the narrow panel {narrow}"
        );
        let collected = rendered_text_w(TABLE_MD, narrow);
        assert!(collected.contains('…'), "expected a clip marker");
    }

    /// Render with the row-separators flag, joined into text (like `rendered_text_w`).
    fn rendered_with_separators(input: &str, width: usize) -> String {
        let opts = RenderOpts {
            table_row_separators: true,
            ..Default::default()
        };
        render_with(input, width, &Palette::default(), opts)
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

    /// Number of `├…┤` separator lines in the joined render.
    fn mid_border_count(text: &str) -> usize {
        text.lines().filter(|l| l.starts_with('├')).count()
    }

    /// By default (flag off) — the previous compact look: a single `├…┤`
    /// under the header, no separators between body rows.
    #[test]
    fn row_separators_off_by_default() {
        let collected = rendered_text(TABLE_MD);
        assert_eq!(
            mid_border_count(&collected),
            1,
            "expected only the header separator:\n{collected}"
        );
    }

    /// With the `table_row_separators` flag, `├…┤` appears between body rows
    /// (TABLE_MD has two rows → one separator between them + one under the
    /// header), and after the last row — still the bottom `└…┘`.
    #[test]
    fn row_separators_drawn_between_body_rows() {
        let collected = rendered_with_separators(TABLE_MD, 80);
        assert_eq!(
            mid_border_count(&collected),
            2,
            "expected the header separator + one inter-row one:\n{collected}"
        );
        assert!(
            collected.lines().last().unwrap().starts_with('└'),
            "the last row should be followed by the table's bottom, not a separator"
        );
        // The separator sits between row content, not right next to the bottom.
        let quick = collected.lines().position(|l| l.contains("QuickSort"));
        let merge = collected.lines().position(|l| l.contains("MergeSort"));
        let mid = collected
            .lines()
            .enumerate()
            .filter(|(_, l)| l.starts_with('├'))
            .map(|(i, _)| i)
            .last();
        let (quick, merge, mid) = (quick.unwrap(), merge.unwrap(), mid.unwrap());
        assert!(
            quick < mid && mid < merge,
            "the inter-row separator should sit between the table's rows"
        );
    }

    /// A table with a single body row: there's nowhere for an inter-row
    /// separator to come from — the look matches the flag disabled.
    #[test]
    fn row_separators_noop_for_single_row_table() {
        let single = "| A | B |\n| :--- | :--- |\n| x | y |";
        assert_eq!(mid_border_count(&rendered_with_separators(single, 80)), 1);
    }

    /// Separators don't break the width invariant: both when fitting columns
    /// and on a narrow panel (a clip with "…"), table rows stay ≤ the panel
    /// width.
    #[test]
    fn row_separators_respect_panel_width() {
        let opts = RenderOpts {
            table_row_separators: true,
            ..Default::default()
        };
        for w in [24usize, 40, 60, 80] {
            let text = render_with(TABLE_MD, w, &Palette::default(), opts);
            let max = text
                .lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
                        .sum::<usize>()
                })
                .max()
                .unwrap_or(0);
            assert!(max <= w, "at width {w} the row grew to {max}");
        }
    }
}
