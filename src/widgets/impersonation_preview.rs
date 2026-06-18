//! Потоковый предпросмотр имперсонируемой реплики (`Ctrl+U`, spec §11.8).
//!
//! Во время написания сообщения «за пользователя» поле ввода прячется, а на его
//! месте показывается этот нередактируемый виджет: в него потоково печатается
//! генерируемый текст. По завершении (если не отменено) текст вставляется в поле
//! ввода. Внешне виджет похож на поле ввода (рамка + текст), но без курсора.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::shared::theme::Palette;
use crate::shared::wrap::wrap_ranges;

/// Кадры спиннера «идёт генерация» (как у индикатора RAG).
const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Рисует предпросмотр имперсонации в `area`. `text` — накопленный текст реплики,
/// `tick` — счётчик кадров для анимации спиннера, `done` — генерация завершена
/// (спиннер гаснет, подсказка меняется).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    tick: usize,
    done: bool,
    palette: &Palette,
) {
    let hint = if done {
        " имперсонация · готово ".to_string()
    } else {
        let spinner = SPINNER[(tick / 2) % SPINNER.len()];
        format!(" {spinner} имперсонация · Esc отмена ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(palette.accent_style())
        .title(Line::from(hint).style(palette.accent_style()));

    // Переносим текст по словам сами (как в `message_feed`/`input_box`), чтобы знать
    // точное число визуальных рядов и прокрутить ленту к хвосту — иначе при длинной
    // реплике виден только её начало, а дописываемый конец уходит за нижнюю границу.
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let mut rows: Vec<Line<'static>> = Vec::new();
    for logical in text.split('\n') {
        let chars: Vec<char> = logical.chars().collect();
        for (s, e) in wrap_ranges(&chars, inner_w) {
            rows.push(Line::from(chars[s..e].iter().collect::<String>()));
        }
    }
    // Скролл к хвосту: показываем последние `inner_h` рядов.
    let scroll = (rows.len().saturating_sub(inner_h)) as u16;

    let para = Paragraph::new(Text::from(rows))
        .block(block)
        .scroll((scroll, 0));
    frame.render_widget(para, area);
}
