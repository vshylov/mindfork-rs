//! Markdown — раскладка и отрисовка таблиц (TableBuilder + render_table). Часть модуля [`super`]; разбито из монолита
//! markdown.rs (см. docs/refactoring-god-objects.md, этап 6).

use super::*;

/// Накопитель ячеек таблицы между событиями `Table…`.
pub(super) struct TableBuilder {
    /// Выравнивание по столбцам (из разметки `:---:`).
    pub(super) alignments: Vec<Alignment>,
    /// Ячейки заголовка.
    pub(super) head: Vec<Vec<Span<'static>>>,
    /// Строки тела (каждая — вектор ячеек-спанов).
    pub(super) rows: Vec<Vec<Vec<Span<'static>>>>,
    /// Текущая собираемая строка.
    pub(super) current_row: Vec<Vec<Span<'static>>>,
    /// Текущая собираемая ячейка (между `Start/End(TableCell)`).
    pub(super) current_cell: Option<Vec<Span<'static>>>,
}

// ---------- раскладка таблиц ----------

/// Минимальная «читаемая» ширина столбца (колонок).
pub(super) const MIN_COL: usize = 5;
/// Верхняя граница минимума: длинное слово допускаем разрывать, не раздувая min.
pub(super) const MAX_MIN: usize = 12;

/// Рендерит таблицу в строки ленты. Ширина столбцов подбирается под `width`
/// (см. [`fit_columns`]); содержимое ячеек переносится по словам. Если столбцам
/// не хватает даже читаемого минимума — таблица рисуется в естественной ширине и
/// **обрезается** по правому краю панели (горизонтальный клип).
pub(super) fn render_table(
    tb: &TableBuilder,
    width: usize,
    palette: &Palette,
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

    // Натуральная и минимальная ширина по столбцам.
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
    // Пустой столбец всё равно получает читаемый минимум.
    for j in 0..ncols {
        minw[j] = minw[j].max(MIN_COL.min(desired[j].max(1)));
    }

    // Доступная ширина под содержимое = ширина минус рамки/паддинги:
    // `│` слева + на столбец (паддинг-пробел + содержимое + паддинг-пробел + `│`).
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
    for row in &tb.rows {
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

/// Подбирает ширины столбцов «водоналивом»: при нехватке места узкие столбцы
/// получают свою натуральную ширину, остаток равномерно делится между широкими
/// (с полом `minw`). `None` — даже минимумы не вмещаются (нужен клип).
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

/// Ширина ячейки в колонках (сумма ширин спанов).
pub(super) fn cell_width(cell: &[Span<'static>]) -> usize {
    cell.iter()
        .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
        .sum()
}

/// Ширина самого длинного «слова» (непробельной последовательности) в ячейке.
pub(super) fn longest_word(cell: &[Span<'static>]) -> usize {
    let text: String = cell.iter().map(|s| s.content.as_ref()).collect();
    text.split_whitespace()
        .map(|w| wrap::display_width(&w.chars().collect::<Vec<_>>()))
        .max()
        .unwrap_or(0)
}

/// Вид горизонтальной границы таблицы.
#[derive(Clone, Copy)]
pub(super) enum Border {
    Top,
    Mid,
    Bottom,
}

/// Строит строку-границу по ширинам столбцов.
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

/// Рендерит строку таблицы (с переносом ячеек по ширинам столбцов) в визуальные
/// ряды. Заголовок — жирным.
pub(super) fn render_row(
    cells: &[Vec<Span<'static>>],
    widths: &[usize],
    aligns: &[Alignment],
    palette: &Palette,
    header: bool,
) -> Vec<Line<'static>> {
    let ncols = widths.len();
    // Переносим каждую ячейку по её ширине → ряды спанов.
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

/// Убирает хвостовые пробелы из визуального ряда ячейки. `wrap::wrap_ranges`
/// «проливает» пробел на границе слова за край ряда (в обычной ленте он невидим),
/// но в таблице эти пробелы учитываются в [`pad_cell`] и раздувают строку **шире
/// столбца** — тогда повторный перенос ленты (`message_feed`) разрывает рамку.
/// Внутри ячейки хвостовые пробелы незначимы (паддинг добавляется заново), поэтому
/// их безопасно срезать, гарантируя ряд ≤ `widths[j]`.
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

/// Дополняет ряд ячейки пробелами до ширины `width` с учётом выравнивания.
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

/// Обрезает строку по `width` колонкам (клип широкой таблицы), добавляя «…».
pub(super) fn clip_line(line: &mut Line<'static>, width: usize) {
    let total: usize = line
        .spans
        .iter()
        .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
        .sum();
    if total <= width {
        return;
    }
    let budget = width.saturating_sub(1); // место под «…»
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
        // частично влезает — режем по символам
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
        // длинная ячейка переносится в несколько рядов, не вылезая за ширину
        let long = "\
| A | Особенности |
| :--- | :--- |
| x | Самая высокая скорость на практике сортировки |";
        let w = 40;
        assert!(max_line_width(long, w) <= w);
        // несколько строк тела → перенос произошёл
        let lines = render(long, w, &Palette::default()).lines.len();
        assert!(lines >= 6, "ожидался перенос ячейки в несколько рядов");
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
        // при достаточной ширине таблица не превышает её
        for w in [40usize, 60, 80, 120] {
            let max = max_line_width(TABLE_MD, w);
            assert!(max <= w, "ширина {max} превысила панель {w}");
        }
    }

    /// Таблица с переносом ячеек (как на скриншоте) не должна превышать ширину
    /// **ни при каком** размере панели — иначе повторный перенос в `message_feed`
    /// разорвал бы рамку. Регрессия на «пролитый» пробел на границе слова.
    #[test]
    fn wrapping_table_never_exceeds_any_width() {
        const WIDE: &str = "\
| Подход | Как работает | Минус |
| :--- | :--- | :--- |
| Стандартный Transformer Chain-of-Thought (o1) | Фиксированный проход Input → Output. Модель пишет рассуждения текстом в скрытый чат | Одинаковые затраты ресурсов на всё. Дорого по токенам, медленно, ограничено длиной текста. |
| Ваша идея (Recurrent ACT) | Итерации в скрытом пространстве (latent space) | Сложность в обучении (нужны новые методы градиентного спуска). |";
        for w in 30usize..=140 {
            let max = max_line_width(WIDE, w);
            assert!(max <= w, "при ширине {w} строка таблицы вышла на {max}");
        }
    }

    #[test]
    fn wide_table_is_clipped_to_width() {
        // узкая панель: таблица обрезается, но не вылезает за край
        let narrow = 24;
        let max = max_line_width(TABLE_MD, narrow);
        assert!(
            max <= narrow,
            "ширина {max} превысила узкую панель {narrow}"
        );
        let collected = rendered_text_w(TABLE_MD, narrow);
        assert!(collected.contains('…'), "ожидался маркер обрезки");
    }
}
