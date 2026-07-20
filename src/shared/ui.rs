//! Слой `shared` (FSD): мелкие переиспользуемые помощники отрисовки TUI,
//! не зависящие от верхних слоёв.

use ratatui::Frame;
use ratatui::buffer::Buffer;
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

/// Готовит буфер к **полной перерисовке** следующего кадра: делает каждую ячейку
/// заведомо отличной от того, что нарисует кадр, чтобы поячеечный diff `ratatui`
/// переписал экран целиком — включая пробелы в пустых местах — и стёр «висячие»
/// артефакты терминала.
///
/// Вызывается на буфере, который затем уходит в задний через `swap_buffers()`
/// **без вывода на экран**: сам маркер на терминал не попадает, он лишь база для
/// diff'а. Обычной очисткой (`terminal.clear()`) не пользуемся — она шлёт `ESC[2J`,
/// и экран на миг гаснет (мигание).
///
/// **Почему пробел + `HIDDEN`, а не символ-заглушка.** Раньше маркером был символ
/// `"\0"`, но это ломало ряды с VS16-эмодзи (`🗂️`, `❤️`): для такого кластера
/// `ratatui` **дополнительно шлёт его хвостовую ячейку** (их обход терминалов, не
/// очищающих вторую половину широкого глифа), причём **только если её символ
/// изменился** — а `"\0"` менял его всегда. Бэкенд `crossterm` при этом ведёт
/// позицию по номеру ячейки, без учёта ширины глифа (`x == last.x + 1` → без
/// `MoveTo`), поэтому такой хвост печатался колонкой правее и сдвигал остаток ряда:
/// у следующего широкого глифа затиралась правая половина (терминал гасил его
/// целиком), рамка панели уезжала наружу.
///
/// Пробел совпадает с содержимым хвостовой ячейки (её `ratatui` сбрасывает в
/// дефолт), поэтому такие ячейки в diff не попадают — и сдвига не возникает. Всё
/// остальное отличается модификатором [`SENTINEL_MODIFIER`], которого в интерфейсе
/// не бывает (закреплено тестом), так что полнота перерисовки не страдает.
pub fn prime_full_redraw(buf: &mut Buffer) {
    for cell in buf.content.iter_mut() {
        cell.set_symbol(" ");
        cell.modifier = SENTINEL_MODIFIER;
    }
}

/// Модификатор-маркер для [`prime_full_redraw`]: в интерфейсе не используется
/// (палитра и виджеты обходятся `DIM`/`BOLD`/`ITALIC`/`UNDERLINED`/`REVERSED`),
/// поэтому ни одна ячейка реального кадра с ним не совпадёт.
const SENTINEL_MODIFIER: Modifier = Modifier::HIDDEN;

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
    fn prime_full_redraw_repaints_all_but_wide_glyph_tails() {
        // Гейт на устройство сентинела (регрессия «ряд съезжает вправо на VS16»).
        // Кадр: VS16-эмодзи с текстом после него — как в ленте.
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Paragraph, Widget};
        use unicode_width::UnicodeWidthStr;

        let area = Rect::new(0, 0, 20, 2);
        let mut frame = Buffer::empty(area);
        Paragraph::new(Line::from(vec![Span::styled(
            "a 🗂\u{FE0F} хвост",
            Style::new().fg(ratatui::style::Color::Blue),
        )]))
        .render(area, &mut frame);

        let mut sentinel = frame.clone();
        prime_full_redraw(&mut sentinel);
        let updates = sentinel.diff(&frame);

        // (1) Ни одно обновление не целится во вторую половину широкого глифа —
        // иначе бэкенд напечатал бы его без `MoveTo` и сдвинул остаток ряда.
        for &(x, y, _) in &updates {
            if x == 0 {
                continue;
            }
            let left = frame[(x - 1, y)].symbol();
            assert!(
                left.width() < 2,
                "обновление в ({x},{y}) целится во вторую половину глифа {left:?}"
            );
        }

        // (2) Перерисовано всё остальное: непокрытыми остаются ровно хвостовые
        // половины широких глифов (их закрывает сам глиф) — дыр в перерисовке нет.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if updates.iter().any(|&(ux, uy, _)| (ux, uy) == (x, y)) {
                    continue;
                }
                let is_tail = x > 0 && frame[(x - 1, y)].symbol().width() == 2;
                assert!(is_tail, "ячейка ({x},{y}) не перерисована и не хвостовая");
            }
        }
    }

    #[test]
    fn screen_switch_emits_vs16_tail_without_full_redraw() {
        // Почему смена экрана обязана идти полной перерисовкой (`app/runtime`).
        //
        // VS16-кластер (`❤️` = U+2764 U+FE0F) ratatui сопровождает хвостовой ячейкой,
        // которую шлёт, КОГДА ЕЁ СИМВОЛ ИЗМЕНИЛСЯ. Внутри одного экрана правки ленты
        // хвост не трогают (там был пробел — пробел и остался), а вот на возврате с
        // другого экрана на его месте стоял чужой символ → хвост уходит в терминал.
        // Бэкенд же ведёт позицию по номеру ячейки, без учёта ширины глифа
        // (открытый ratatui#2651): после широкого глифа `MoveTo` не эмитится, и хвост
        // печатается колонкой правее — остаток ряда едет вправо. Симптом: лишний
        // пробел после `❤️` при возврате из списка чатов / `F3`, пропадающий по
        // прокрутке (она заказывает ту же полную перерисовку).
        //
        // Сентинел делает символ хвоста пробелом, т.е. равным тому, что нарисует кадр,
        // — хвост в diff не попадает и ряд не съезжает.
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Paragraph, Widget};

        let area = Rect::new(0, 0, 20, 1);
        let mut feed = Buffer::empty(area);
        Paragraph::new(Line::from(vec![Span::styled(
            "a \u{2764}\u{FE0F} tail",
            Style::new(),
        )]))
        .render(area, &mut feed);
        let gx = (0..area.width)
            .find(|&x| feed[(x, 0)].symbol().contains('\u{FE0F}'))
            .expect("VS16-глиф не найден в кадре");

        // Возврат с другого экрана: на месте хвоста стоял чужой символ.
        let mut other = Buffer::empty(area);
        Paragraph::new(Line::from("chat list content!!")).render(area, &mut other);
        assert!(
            other
                .diff(&feed)
                .iter()
                .any(|&(x, y, _)| (x, y) == (gx + 1, 0)),
            "смена экрана шлёт хвост VS16 — без полной перерисовки ряд съедет"
        );

        // Правка внутри того же экрана хвост не шлёт (потому баг и был виден только
        // на переключении, а не при обычной работе с лентой).
        let mut same = feed.clone();
        same[(area.width - 1, 0)].set_symbol("Z");
        assert!(
            !same
                .diff(&feed)
                .iter()
                .any(|&(x, y, _)| (x, y) == (gx + 1, 0)),
            "правка внутри экрана хвост VS16 не трогает"
        );

        // Полная перерисовка: хвост не эмитится → сдвига нет.
        let mut sentinel = feed.clone();
        prime_full_redraw(&mut sentinel);
        assert!(
            !sentinel
                .diff(&feed)
                .iter()
                .any(|&(x, y, _)| (x, y) == (gx + 1, 0)),
            "сентинел не должен слать хвост VS16 (иначе сам же сдвинет ряд)"
        );
    }

    #[test]
    fn sentinel_modifier_is_unused_by_ui() {
        // Маркер обязан не встречаться в реальных кадрах, иначе совпавшая ячейка не
        // попадёт в diff и останется непрокрашенной. Палитра обходится
        // DIM/BOLD/ITALIC/UNDERLINED/REVERSED — проверяем, что маркер не из них.
        for used in [
            Modifier::DIM,
            Modifier::BOLD,
            Modifier::ITALIC,
            Modifier::UNDERLINED,
            Modifier::REVERSED,
        ] {
            assert!(
                !SENTINEL_MODIFIER.intersects(used),
                "маркер сентинела пересекается с используемым в UI {used:?}"
            );
        }
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
