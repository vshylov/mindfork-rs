//! Перенос строк по словам для виджетов с собственным скроллом/курсором
//! (`message_feed`, `input_box`). См. ADR 0001: виджеты владеют отрисовкой, поэтому
//! переносим строки сами — заранее, до `Paragraph`, — чтобы число визуальных рядов
//! совпадало с числом строк (иначе ломаются математика скролла и позиция курсора).
//!
//! Ширина считается в терминальных колонках через `unicode-width` (кириллица/латиница
//! = 1, эмодзи/CJK = 2), а не в символах — иначе широкие символы «вылезают» за край.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Селектор «эмодзи-представления» (U+FE0F): превращает текстовый по умолчанию символ
/// (напр. ❤ U+2764, по `unicode-width` ширина 1) в эмодзи, который терминал рисует
/// шириной 2 колонки. См. [`width_at`].
const EMOJI_VS: char = '\u{FE0F}';

/// Ширина символа в терминальных колонках по `unicode-width` (управляющие → 0).
/// **Контекст-независима** — для эмодзи-кластеров (база + вариатор/модификатор)
/// используйте [`width_at`]/[`display_width`], которые учитывают соседей.
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Модификатор тона кожи (U+1F3FB..U+1F3FF): присоединяется к предыдущему эмодзи и
/// **не добавляет** колонок (база уже шириной 2). Сам по себе `unicode-width` считает
/// его шириной 2, поэтому без поправки `👍🏽` мерилось бы как 4 колонки вместо 2.
fn is_emoji_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Ширина символа `chars[i]` в колонках **с учётом эмодзи-кластера**. Каузально
/// (смотрит только на предыдущий символ), поэтому накопительная ширина любого
/// префикса корректна — это нужно и переносу, и позиционированию курсора:
/// - селектор эмодзи-представления U+FE0F добавляет недостающую колонку к
///   предшествующему «текстовому» символу (`❤` ширины 1 + U+FE0F = 2 колонки);
/// - модификатор тона кожи не добавляет колонок (база-эмодзи уже шириной 2).
///
/// Терминал (Windows Terminal и пр.) рисует такие кластеры шириной 2; без поправки
/// курсор «разъезжался» с текстом на эмодзи (см. spec §11.5).
pub fn width_at(chars: &[char], i: usize) -> usize {
    let c = chars[i];
    if is_emoji_modifier(c) {
        return 0;
    }
    if c == EMOJI_VS {
        let prev = if i > 0 { char_width(chars[i - 1]) } else { 0 };
        // Доводим предшествующий символ до ширины 2 (для уже-широкого добавит 0).
        return 2usize.saturating_sub(prev);
    }
    char_width(c)
}

/// Суммарная ширина среза символов в колонках — с учётом эмодзи-кластеров
/// (см. [`width_at`]).
pub fn display_width(chars: &[char]) -> usize {
    (0..chars.len()).map(|i| width_at(chars, i)).sum()
}

/// Граница графемного кластера **слева** от позиции `col` (индекс символа): начало
/// кластера, в котором/перед которым стоит курсор. Курсор/удаление должны ходить по
/// кластерам, а не по скалярам Unicode — иначе `❤️` (`❤`+U+FE0F), `👍🏽` (эмодзи +
/// модификатор тона), ZWJ-последовательности и флаги проходятся/удаляются по половинке
/// (курсор садится в середину эмодзи, Backspace оставляет «осиротевший» вариатор). UAX
/// #29 (extended grapheme clusters) через `unicode-segmentation`. См. spec §11.5.
pub fn prev_boundary(chars: &[char], col: usize) -> usize {
    cluster_boundaries(chars)
        .into_iter()
        .rfind(|&b| b < col)
        .unwrap_or(0)
}

/// Граница графемного кластера **справа** от позиции `col` (зеркально
/// [`prev_boundary`]) — конец кластера, на котором стоит курсор.
pub fn next_boundary(chars: &[char], col: usize) -> usize {
    cluster_boundaries(chars)
        .into_iter()
        .find(|&b| b > col)
        .unwrap_or(chars.len())
}

/// Границы графемных кластеров в срезе (индексы символов), включая `0` и `len`.
/// Между соседними границами — один кластер (возможно из нескольких скаляров).
fn cluster_boundaries(chars: &[char]) -> Vec<usize> {
    let s: String = chars.iter().collect();
    let mut bounds = Vec::with_capacity(chars.len() + 1);
    bounds.push(0);
    let mut acc = 0;
    for g in s.graphemes(true) {
        acc += g.chars().count();
        bounds.push(acc);
    }
    bounds
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
            let cw = width_at(chars, i);
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
    fn emoji_presentation_selector_makes_width_two() {
        // ❤ (U+2764, по unicode-width ширина 1) + U+FE0F → терминал рисует 2 колонки.
        let cs = chars("❤\u{FE0F}");
        assert_eq!(display_width(&cs), 2);
        // Без селектора остаётся текстовым (ширина 1).
        assert_eq!(display_width(&chars("❤")), 1);
        // Накопительная ширина каузальна: префикс из одного ❤ = 1, с селектором = 2.
        assert_eq!(width_at(&cs, 0), 1);
        assert_eq!(width_at(&cs, 1), 1);
    }

    #[test]
    fn skin_tone_modifier_adds_no_columns() {
        // 👍 (ширина 2) + модификатор тона кожи 🏽 → единый эмодзи шириной 2, не 4.
        assert_eq!(display_width(&chars("👍🏽")), 2);
        // Эмодзи из supplementary-плоскости без модификатора — обычные 2 колонки.
        assert_eq!(display_width(&chars("😊")), 2);
    }

    #[test]
    fn selector_after_wide_emoji_adds_nothing() {
        // Уже-широкий эмодзи + U+FE0F (избыточный селектор) остаётся шириной 2.
        let cs = chars("😊\u{FE0F}");
        assert_eq!(display_width(&cs), 2);
    }

    #[test]
    fn grapheme_boundaries_group_emoji_clusters() {
        // "a❤️b👍🏽" : a | ❤+U+FE0F | b | 👍+модификатор → границы 0,1,3,4,6.
        let cs = chars("a❤\u{FE0F}b👍🏽");
        assert_eq!(cs.len(), 6);
        // слева от конца (6) — начало кластера 👍🏽 (4)
        assert_eq!(prev_boundary(&cs, 6), 4);
        // слева от 4 — 'b' (3)
        assert_eq!(prev_boundary(&cs, 3), 1);
        // справа от 1 — конец ❤️ (3)
        assert_eq!(next_boundary(&cs, 1), 3);
        // одиночный скаляр-эмодзи 😊 — обычная граница ±1
        let e = chars("😊x");
        assert_eq!(next_boundary(&e, 0), 1);
        assert_eq!(prev_boundary(&e, 1), 0);
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
