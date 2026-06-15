//! Перенос строк по словам для виджетов с собственным скроллом/курсором
//! (`message_feed`, `input_box`). См. ADR 0001: виджеты владеют отрисовкой, поэтому
//! переносим строки сами — заранее, до `Paragraph`, — чтобы число визуальных рядов
//! совпадало с числом строк (иначе ломаются математика скролла и позиция курсора).
//!
//! Ширина считается в терминальных колонках через `unicode-width` (кириллица/латиница
//! = 1, эмодзи/CJK = 2), а не в символах — иначе широкие символы «вылезают» за край.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Ширина символа в терминальных колонках (управляющие → 0).
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Суммарная ширина среза символов в колонках.
pub fn display_width(chars: &[char]) -> usize {
    chars.iter().map(|&c| char_width(c)).sum()
}

/// Делит логическую строку символов на визуальные ряды шириной не больше `width`.
///
/// Перенос предпочтительно по границам слов (после пробелов); слово длиннее ширины
/// рвётся по символам. В каждый ряд кладётся хотя бы один символ (защита от
/// зацикливания на символе шире `width`). Возвращает диапазоны индексов `[start, end)`
/// исходного среза, по одному на ряд. Для пустого среза — один пустой ряд `(0, 0)`
/// (сохраняет пустые строки как визуальные ряды).
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
        // Индекс последнего пробела, влезшего в ряд (кандидат на мягкий перенос).
        let mut last_ws: Option<usize> = None;
        let end = loop {
            if i >= n {
                break n;
            }
            let cw = char_width(chars[i]);
            // Первый символ ряда берём всегда, даже если он шире `width`.
            if w + cw > width && i > start {
                if chars[i].is_whitespace() {
                    // Ровно на границе слова: пробельный «хвост» можно «пролить» за
                    // край (он невидим) и оставить в этом ряду — тогда следующий ряд
                    // начнётся со слова, а не с пробела.
                    while i < n && chars[i].is_whitespace() {
                        i += 1;
                    }
                    break i;
                }
                break match last_ws {
                    // мягкий перенос после последнего пробела
                    Some(ws) => ws + 1,
                    // слово длиннее ширины — жёсткий разрыв по символам
                    None => i,
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

/// Переносит стилизованную строку на визуальные ряды шириной `width`, сохраняя стили
/// спанов (markdown-подсветка, подчёркивания спелл-чека) и стиль/выравнивание строки.
pub fn wrap_line(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    // Разворачиваем спаны в посимвольные (символ, стиль) — переносить удобнее так.
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
            // Пересобираем спаны ряда, склеивая соседние символы с одинаковым стилем.
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

    /// Удобный помощник: переносит строку и собирает текст каждого ряда.
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
        // "один два три" при ширине 8: оба слова влезают (4+1+3=8); пробел-разделитель
        // остаётся в конце ряда (невидим), следующий ряд начинается со слова.
        assert_eq!(wrap_text("один два три", 8), vec!["один два ", "три"]);
    }

    #[test]
    fn long_word_is_hard_broken() {
        // слово длиннее ширины рвётся по символам
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wide_chars_count_two_columns() {
        // эмодзи шириной 2: при ширине 3 влезает один + пробел
        let cs = chars("🔧🔧");
        // каждый эмодзи = 2 колонки, ширина 3 → по одному на ряд
        assert_eq!(wrap_ranges(&cs, 3), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn single_wide_char_never_loops() {
        // символ шире ширины всё равно кладётся (по одному в ряд)
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
        // "крас" — красная, " синий текст" — обычная; перенос по ширине 6
        let line = Line::from(vec![Span::raw("раз ").red(), Span::raw("два три")]);
        let rows = wrap_line(&line, 6);
        // собираем весь текст обратно — содержимое не теряется
        let joined: String = rows
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(joined, "раз два три");
        // в первом ряду сохранился красный спан
        assert!(rows[0].spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn wrap_line_keeps_empty_row_for_blank_line() {
        let line = Line::from("");
        let rows = wrap_line(&line, 10);
        assert_eq!(rows.len(), 1);
    }
}
