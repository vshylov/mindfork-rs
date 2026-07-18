//! Логотип mindfork ячейками терминала (см. docs/branding.md §5).
//!
//! Иконка бренда — пиксель-арт на сетке 16×16 из пяти прямоугольников, поэтому её
//! можно нарисовать **нативно**, без растровых картинок: одна ячейка терминала несёт
//! **два вертикальных пикселя** через половинные блоки `▀`/`▄`/`█`. Рисуется
//! прозрачный вариант (без подложки) — глиф ложится на фон терминала и одинаково
//! уместен в тёмной и светлой теме.
//!
//! Рядом с глифом рисуется **вордмарк** — слово `mindfork` собственным пиксельным
//! шрифтом (SVG-вордмарк для терминала непригоден: там текст переведён в кривые).
//! Вместе они образуют **горизонтальный лockup** — ту же композицию, что
//! `artwork/mindfork-wordmark.svg`: слово вдвое ниже глифа, его базовая линия — на
//! строку выше низа глифа.
//!
//! **Цвета — фирменные, не из палитры**: логотип не перекрашивается темой (решение
//! Р3 в docs/branding.md — `accent` интерфейса означает «активность», перекраска
//! сломала бы семантику). Единственное исключение — `mind` в вордмарке: в бренде это
//! «текст на тёмном»/«текст на светлом» (раздельные варианты `-dark`/`-light`), т.е.
//! цвет фона, поэтому здесь он берётся из палитры (`text`) — так один лockup работает
//! в обеих темах. Глифы `▀`/`▄`/`█` входят в WGL4, поэтому режим совместимости со
//! старым терминалом (conhost) отдельной замены не требует — как `█` скроллбара и
//! `▌` рейлов ролей.
//!
//! Единственный источник истины геометрии глифа —
//! `artwork/mindfork-icon-transparent.svg`; тест `glyph_matches_artwork_svg` сверяет
//! таблицу ниже с ним, чтобы код и ассет не разъехались молча.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Акцент бренда — ствол глифа (`#c25a27`).
const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);
/// Ветви глифа (`#5c6370`).
const GRAY: Color = Color::Rgb(0x5c, 0x63, 0x70);

/// Прямоугольники глифа на сетке 16×16: `(x, y, w, h, цвет)`.
const GLYPH: [(u8, u8, u8, u8, Color); 5] = [
    (7, 2, 2, 12, ORANGE),
    (11, 2, 2, 5, GRAY),
    (9, 5, 2, 2, GRAY),
    (3, 7, 2, 5, GRAY),
    (5, 10, 2, 2, GRAY),
];

/// Границы чернил на сетке (`x` 3…13, `y` 2…14) — рисуем только их, без пустых полей.
const INK_X: (u8, u8) = (3, 13);
const INK_Y: (u8, u8) = (2, 14);

/// Ширина логотипа в колонках терминала.
pub const LOGO_COLS: u16 = (INK_X.1 - INK_X.0) as u16;
/// Высота логотипа в строках: два пикселя на строку.
pub const LOGO_ROWS: u16 = (INK_Y.1 - INK_Y.0) as u16 / 2;

/// Пиксельный шрифт вордмарка: по строке на пиксельный ряд глифа (`#` — чернила).
///
/// Начертание повторяет вордмарк бренда (JetBrains Mono ExtraBold): строчные буквы,
/// выносные у `d`/`f`/`k` и **штрихи в 2 пикселя** — той же толщины, что бары
/// глифа-иконки (в её сетке они тоже по 2 единицы). Штрих в 1 пиксель дал бы светлое
/// начертание, спорящее с чанковым глифом, а буква с 2-пиксельным стволом (`i`)
/// смотрелась бы среди них вдвое жирнее.
///
/// Высота — **8 пикселей = 4 строки терминала**: бренд задаёт выносной элемент как
/// `0.5227 × S`, где `S` — размер иконки (16), т.е. ≈ 8 пикселей (а не половину
/// 12-пиксельных чернил глифа). Это же даёт x-высоту в 6 пикселей, куда 2-пиксельные
/// штрихи и просвет между ними укладываются ровно.
///
/// Отдельный компромисс — `f`. Между её крючком (на выносной высоте) и перекладиной
/// (на x-высоте) в пиксельном бюджете остаётся 0 рядов, поэтому 2-пиксельный крючок
/// слипался с перекладиной в сплошной блок. Крючок сделан в **1 пиксель**: он
/// попадает в верхнюю половину ячейки (`▀`), нижняя остаётся пустой — просвет виден,
/// а перекладина стоит на x-высоте, в одну строку с верхними барами `n`/`o`/`r`.
///
/// Пиксельный шрифт нужен собственный: в SVG-вордмарке текст переведён в кривые
/// (docs/branding.md §2), растеризовать их в терминале нечем.
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

/// Высота вордмарка в пикселях (чётная — половинные блоки ложатся ровно в строки).
const WORDMARK_PX_ROWS: usize = 8;
/// Разрядка между буквами, в колонках.
const WORDMARK_TRACKING: u16 = 1;
/// Сколько первых букв — «mind» (цвет текста); остальные — «fork» (акцент).
const MIND_LETTERS: usize = 4;

/// Высота вордмарка в строках терминала.
pub const WORDMARK_ROWS: u16 = WORDMARK_PX_ROWS as u16 / 2;
/// Ширина вордмарка в колонках терминала.
pub const WORDMARK_COLS: u16 = wordmark_cols();

/// Зазор «глиф → слово» в колонках (пропорция бренда: 0.36 × размер иконки за
/// вычетом пустого поля справа от чернил, docs/branding.md §5).
const LOCKUP_GAP: u16 = 3;
/// Строка лockup'а, с которой начинается слово: его базовая линия оказывается на
/// строку выше низа глифа (бренд — `0.7418 × S` от верха иконки), так же, как в
/// SVG-лockup'е.
const WORDMARK_TOP_ROW: u16 = 1;

/// Ширина горизонтального лockup'а (глиф + зазор + слово) в колонках.
pub const LOCKUP_COLS: u16 = LOGO_COLS + LOCKUP_GAP + WORDMARK_COLS;
/// Высота лockup'а в строках: слово ниже глифа, поэтому её задаёт глиф.
pub const LOCKUP_ROWS: u16 = LOGO_ROWS;

// Слово обязано умещаться по высоте глифа — иначе `lockup_lines` молча обрезала бы
// его нижние строки. Правка `WORDMARK_TOP_ROW`/шрифта роняет сборку, а не картинку.
const _: () = assert!(WORDMARK_TOP_ROW + WORDMARK_ROWS <= LOCKUP_ROWS);

/// Суммарная ширина слова: глифы плюс разрядка между ними.
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

/// Цвет пикселя `(x, y)` сетки, если он закрашен.
fn pixel(x: u8, y: u8) -> Option<Color> {
    GLYPH
        .iter()
        .find(|(gx, gy, gw, gh, _)| x >= *gx && x < gx + gw && y >= *gy && y < gy + gh)
        .map(|(_, _, _, _, color)| *color)
}

/// Логотип строками для вставки в любой виджет: `LOGO_ROWS` строк по `LOGO_COLS` ячеек.
///
/// Пара вертикальных пикселей кодируется одной ячейкой: обе половины одного цвета —
/// `█`; разные — `▀` (верхняя в `fg`, нижняя в `bg`); одна половина — `▀`/`▄` без
/// фона (так логотип не тащит за собой прямоугольник подложки).
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

/// Колонки слова: цвет и столбик из `WORDMARK_PX_ROWS` пикселей на каждую.
///
/// Буквы разделены пустой колонкой (разрядка), поэтому цвет — свойство колонки:
/// «mind» рисуется цветом `mind`, «fork» — фирменным акцентом.
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

/// Слово `mindfork` строками: `WORDMARK_ROWS` строк по `WORDMARK_COLS` ячеек.
///
/// `mind` берёт переданный цвет (в вызывающем — `palette.text`), `fork` — фирменный
/// акцент; кодирование пары пикселей то же, что у глифа. Фон не выставляется —
/// подложки у слова нет.
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

/// Горизонтальный лockup «глиф + слово» — `LOCKUP_ROWS` строк.
///
/// Слово ниже глифа, поэтому строки лockup'а вне его диапазона несут только глиф
/// (хвостовыми пробелами не добиваем — строки рисуются с выравниванием влево).
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

    /// Разбирает `<rect …/>` из SVG-иконки: `(x, y, w, h, fill)`.
    fn parse_rects(svg: &str) -> Vec<(u8, u8, u8, u8, String)> {
        fn attr(tag: &str, name: &str) -> String {
            let key = format!("{name}=\"");
            let start = tag.find(&key).expect("атрибут") + key.len();
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
            other => panic!("ожидался Rgb, получено {other:?}"),
        }
    }

    /// Гейт против расхождения кода и ассета: таблица `GLYPH` обязана совпадать с
    /// `artwork/mindfork-icon-transparent.svg` — единственным источником геометрии.
    #[test]
    fn glyph_matches_artwork_svg() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/artwork/mindfork-icon-transparent.svg"
        );
        let svg = std::fs::read_to_string(path).expect("иконка на месте");
        let from_svg = parse_rects(&svg);
        let from_code: Vec<_> = GLYPH
            .iter()
            .map(|(x, y, w, h, c)| (*x, *y, *w, *h, hex(*c)))
            .collect();
        assert_eq!(
            from_svg, from_code,
            "GLYPH разошёлся с artwork/mindfork-icon-transparent.svg — \
             обновите таблицу или ассет"
        );
    }

    /// Сетка 16×16 и границы чернил в SVG те же, что заложены в константах.
    #[test]
    fn ink_bounds_match_svg_viewbox() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/artwork/mindfork-icon-transparent.svg"
        );
        let svg = std::fs::read_to_string(path).expect("иконка на месте");
        assert!(svg.contains("viewBox=\"0 0 16 16\""), "сетка не 16×16");
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
        // Высота чернил чётная — иначе половинные блоки не лягут ровно в строки.
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
            // Каждая ячейка — ровно одна колонка.
            for span in &line.spans {
                assert_eq!(span.content.chars().count(), 1);
            }
        }
    }

    /// Ствол (оранжевый) идёт сплошняком через все строки — глиф не «рассыпался».
    #[test]
    fn orange_trunk_present_in_every_row() {
        for (i, line) in logo_lines().iter().enumerate() {
            let has_orange = line
                .spans
                .iter()
                .any(|s| s.style.fg == Some(ORANGE) || s.style.bg == Some(ORANGE));
            assert!(has_orange, "в строке {i} нет ствола");
        }
    }

    /// Используются только WGL4-безопасные глифы (режим совместимости с conhost).
    #[test]
    fn uses_only_wgl4_block_glyphs() {
        for line in logo_lines().into_iter().chain(lockup_lines(Color::White)) {
            for span in line.spans {
                let ch = span.content.chars().next().unwrap();
                assert!(
                    matches!(ch, '█' | '▀' | '▄' | ' '),
                    "неожиданный глиф {ch:?}"
                );
            }
        }
    }

    /// Шрифт вордмарка целостен: слово то самое, у каждой буквы прямоугольная
    /// матрица и есть хоть один пиксель (иначе буква «пропала» бы молча).
    #[test]
    fn wordmark_font_is_well_formed() {
        let word: String = WORDMARK.iter().map(|(c, _)| *c).collect();
        assert_eq!(word, "mindfork");
        for (ch, rows) in WORDMARK {
            let w = rows[0].len();
            assert!(w > 0, "буква {ch:?} нулевой ширины");
            for row in rows {
                assert_eq!(row.len(), w, "у буквы {ch:?} строки разной ширины");
                assert!(
                    row.bytes().all(|b| b == b'#' || b == b'.' || b == b' '),
                    "у буквы {ch:?} посторонний символ в матрице"
                );
            }
            assert!(rows.iter().any(|r| r.contains('#')), "буква {ch:?} пустая");
        }
    }

    /// Слово рисуется в объявленный размер, а `mind`/`fork` — разными цветами
    /// («mind» темозависим, «fork» — фирменный акцент).
    #[test]
    fn wordmark_has_expected_size_and_split() {
        const MIND: Color = Color::Rgb(0xe4, 0xe4, 0xe7);
        let lines = wordmark_lines(MIND);
        assert_eq!(lines.len(), WORDMARK_ROWS as usize);
        assert_eq!(WORDMARK_ROWS, 4);
        for line in &lines {
            assert_eq!(line.spans.len(), WORDMARK_COLS as usize);
        }
        // Ширина слова = сумма букв + разрядка между ними.
        let letters: u16 = WORDMARK.iter().map(|(_, r)| r[0].len() as u16).sum();
        assert_eq!(
            WORDMARK_COLS,
            letters + WORDMARK_TRACKING * (WORDMARK.len() as u16 - 1)
        );
        // «fork» начинается там, где кончается «mind» с его разрядкой.
        let mind_cols: usize = WORDMARK
            .iter()
            .take(MIND_LETTERS)
            .map(|(_, r)| r[0].len() + WORDMARK_TRACKING as usize)
            .sum();
        let row = &lines[1]; // строка x-высоты: чернила есть у всех букв
        assert!(
            row.spans[..mind_cols]
                .iter()
                .all(|s| s.style.fg != Some(ORANGE)),
            "«mind» не должен быть акцентным"
        );
        assert!(
            row.spans[mind_cols..]
                .iter()
                .any(|s| s.style.fg == Some(ORANGE)),
            "«fork» рисуется фирменным акцентом"
        );
    }

    /// Лockup — это глиф слева и слово справа: первые `LOGO_COLS` колонок совпадают
    /// с самостоятельным глифом байт-в-байт, слово не наезжает на него.
    #[test]
    fn lockup_places_wordmark_right_of_glyph() {
        let logo = logo_lines();
        let lockup = lockup_lines(Color::White);
        assert_eq!(lockup.len(), LOCKUP_ROWS as usize);
        for (row, (with_word, glyph)) in lockup.iter().zip(&logo).enumerate() {
            assert_eq!(
                with_word.spans[..LOGO_COLS as usize],
                glyph.spans[..],
                "строка {row}: глиф изменился"
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
                "строка {row}: слово там, где его не ждали (или наоборот)"
            );
        }
        assert_eq!(LOCKUP_COLS, LOGO_COLS + LOCKUP_GAP + WORDMARK_COLS);
    }
}
