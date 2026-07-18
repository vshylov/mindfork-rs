//! Логотип mindfork ячейками терминала (см. docs/branding.md §5).
//!
//! Иконка бренда — пиксель-арт на сетке 16×16 из пяти прямоугольников, поэтому её
//! можно нарисовать **нативно**, без растровых картинок: одна ячейка терминала несёт
//! **два вертикальных пикселя** через половинные блоки `▀`/`▄`/`█`. Рисуется
//! прозрачный вариант (без подложки) — глиф ложится на фон терминала и одинаково
//! уместен в тёмной и светлой теме.
//!
//! **Цвета — фирменные, не из палитры**: логотип не перекрашивается темой (решение
//! Р3 в docs/branding.md — `accent` интерфейса означает «активность», перекраска
//! сломала бы семантику). Глифы `▀`/`▄`/`█` входят в WGL4, поэтому режим
//! совместимости со старым терминалом (conhost) отдельной замены не требует — как
//! `█` скроллбара и `▌` рейлов ролей.
//!
//! Единственный источник истины геометрии — `artwork/mindfork-icon-transparent.svg`;
//! тест `glyph_matches_artwork_svg` сверяет таблицу ниже с ним, чтобы код и ассет не
//! разъехались молча.

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
        for line in logo_lines() {
            for span in line.spans {
                let ch = span.content.chars().next().unwrap();
                assert!(
                    matches!(ch, '█' | '▀' | '▄' | ' '),
                    "неожиданный глиф {ch:?}"
                );
            }
        }
    }
}
