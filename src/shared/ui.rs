//! Слой `shared` (FSD): мелкие переиспользуемые помощники отрисовки TUI,
//! не зависящие от верхних слоёв.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::shared::theme::Palette;

/// Притеняет весь экран (модификатор `DIM` на все ячейки буфера), чтобы попап
/// поверх не сливался с фоном. Вызывать **перед** `Clear`+рендером попапа:
/// `Clear` затем сбрасывает ячейки попапа к дефолтному (не приглушённому) стилю,
/// так что притеняется только фон, а сам попап остаётся ярким.
///
/// Эффект `DIM` терминало-зависим (Windows Terminal поддерживает, conhost
/// Windows 10 — нет), поэтому в режиме совместимости (`palette.compat`, spec
/// §11.6) фон притеняется **цветом**: fg всех ячеек → `palette.muted` (плюс
/// снимается `BOLD` — в 16-цветном маппинге он даёт «яркий» вариант и свёл бы
/// притенение на нет).
pub fn dim_background(frame: &mut Frame, palette: &Palette) {
    let area = frame.area();
    let compat = palette.compat;
    let muted = palette.muted;
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if compat {
                cell.fg = muted;
                cell.modifier.remove(Modifier::BOLD);
            } else {
                cell.modifier |= Modifier::DIM;
            }
        }
    }
}

/// Рисует вертикальный скроллбар в **правой колонке** `area`, когда содержимое
/// не помещается по высоте (`total > viewport`); иначе — no-op (бар не рисуется,
/// чтобы не шуметь на коротком содержимом). `total` — всего рядов содержимого,
/// `viewport` — видимых, `position` — первый видимый ряд (само содержимое
/// прокручивает вызывающий; бар — чистая индикация).
///
/// Панели с рамкой передают область с вертикальным отступом 1
/// (`area.inner(Margin::new(0, 1))`): бар ложится **на правую линию рамки**, не
/// трогая её углы и не отнимая ширину у содержимого. `focused` — в каком цвете
/// нарисована рамка под баром (обычная/в фокусе): и трек, и бегунок рисуются
/// этим цветом, так что скроллбар сливается с рамкой, отличаясь лишь заливкой
/// `█` (бегунок) против тонкого `│` (трек). Бегунок следует за цветом рамки —
/// смена палитры рамки автоматически перекрашивает и его.
pub fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    total: usize,
    viewport: usize,
    position: usize,
    focused: bool,
    palette: &Palette,
) {
    if total <= viewport || area.width == 0 || area.height == 0 {
        return;
    }
    // `content_length` — число ПОЗИЦИЙ прокрутки (total − viewport + 1), а не
    // рядов: ratatui кладёт низ бегунка в конец трека на позиции
    // `content_length − 1`, т.е. ровно при полной прокрутке (total − viewport).
    // С `content_length = total` бегунок не доходил бы до низа.
    let mut state = ScrollbarState::new(total - viewport + 1)
        .position(position)
        .viewport_content_length(viewport);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .track_style(palette.border_style(focused))
        .thumb_style(palette.border_style(focused));
    frame.render_stateful_widget(bar, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Символы правой колонки буфера (там живёт скроллбар).
    fn right_column(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| buf[(area.right() - 1, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn scrollbar_renders_thumb_only_on_overflow() {
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(10, 6)).unwrap();
        // Содержимое помещается (total <= viewport) → бар не рисуется.
        term.draw(|f| render_scrollbar(f, f.area(), 6, 6, 0, false, &palette))
            .unwrap();
        assert!(right_column(&term).iter().all(|s| s != "█" && s != "│"));
        // Переполнение → в правой колонке появляются бегунок и трек.
        term.draw(|f| render_scrollbar(f, f.area(), 24, 6, 0, false, &palette))
            .unwrap();
        let col = right_column(&term);
        assert!(col.iter().any(|s| s == "█"), "нет бегунка: {col:?}");
        assert!(col.iter().any(|s| s == "│"), "нет трека: {col:?}");
    }

    #[test]
    fn scrollbar_thumb_tracks_position() {
        // В начале бегунок у верха, при полной прокрутке — у низа.
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(4, 8)).unwrap();
        let thumb_rows = |term: &Terminal<TestBackend>| -> Vec<usize> {
            right_column(term)
                .iter()
                .enumerate()
                .filter(|(_, s)| *s == "█")
                .map(|(i, _)| i)
                .collect()
        };
        term.draw(|f| render_scrollbar(f, f.area(), 32, 8, 0, false, &palette))
            .unwrap();
        let top = thumb_rows(&term);
        assert_eq!(top.first(), Some(&0), "в начале бегунок у верха: {top:?}");
        // Полная прокрутка: position = total - viewport.
        term.draw(|f| render_scrollbar(f, f.area(), 32, 8, 24, false, &palette))
            .unwrap();
        let bottom = thumb_rows(&term);
        assert_eq!(
            bottom.last(),
            Some(&7),
            "при полной прокрутке бегунок у низа: {bottom:?}"
        );
    }

    #[test]
    fn scrollbar_thumb_uses_border_color() {
        // Бегунок рисуется цветом рамки (как трек), а не текстом. В Auto-палитре
        // `border` (DarkGray) и `text` (Reset) различны, так что проверка значима.
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(4, 8)).unwrap();
        term.draw(|f| render_scrollbar(f, f.area(), 32, 8, 0, false, &palette))
            .unwrap();
        let buf = term.backend().buffer();
        let area = buf.area;
        let thumb_fg = (area.top()..area.bottom())
            .map(|y| &buf[(area.right() - 1, y)])
            .find(|c| c.symbol() == "█")
            .map(|c| c.fg);
        assert_eq!(thumb_fg, Some(palette.border), "бегунок — цветом рамки");
    }

    #[test]
    fn scrollbar_zero_area_is_noop() {
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(4, 4)).unwrap();
        term.draw(|f| {
            let zero = Rect::new(0, 0, 0, 0);
            render_scrollbar(f, zero, 10, 2, 0, false, &palette);
        })
        .unwrap();
    }

    #[test]
    fn dim_background_uses_color_in_compat_mode() {
        let mut term = Terminal::new(TestBackend::new(4, 2)).unwrap();
        // Обычный режим — модификатор DIM на ячейках.
        term.draw(|f| {
            dim_background(f, &Palette::default());
            assert!(f.buffer_mut()[(0, 0)].modifier.contains(Modifier::DIM));
        })
        .unwrap();
        // Режим совместимости — приглушённый цвет вместо DIM (conhost его не умеет).
        let compat = Palette::default().with_compat(true);
        term.draw(|f| {
            dim_background(f, &compat);
            let cell = f.buffer_mut()[(0, 0)].clone();
            assert_eq!(cell.fg, compat.muted);
            assert!(!cell.modifier.contains(Modifier::DIM));
        })
        .unwrap();
    }
}
