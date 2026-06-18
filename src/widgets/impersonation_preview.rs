//! Потоковый предпросмотр имперсонируемой реплики (`Ctrl+U`, spec §11.8).
//!
//! Во время написания сообщения «за пользователя» поле ввода прячется, а на его
//! месте показывается этот нередактируемый виджет: в него потоково печатается
//! генерируемый текст. По завершении (если не отменено) текст вставляется в поле
//! ввода. Внешне виджет похож на поле ввода (рамка + текст), но без курсора.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::shared::theme::Palette;

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
    let para = Paragraph::new(text.to_string())
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}
