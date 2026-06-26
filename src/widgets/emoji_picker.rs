//! Попап выбора эмодзи (`Ctrl+B` в окне чата): сетка популярных эмодзи, выбор
//! стрелками, вставка выбранного в поле ввода на месте курсора. См. spec §11.5.
//!
//! Виджет самодостаточен (FSD: `screens → widgets`): хранит только индекс
//! выделения, на нажатия отвечает [`EmojiPickerAction`] — его исполняет экран
//! чата (вставляет эмодзи через [`InputBox::insert_str`]). Сами эмодзи — это
//! статический список ниже; курсор/удаление по графемным кластерам уже корректны
//! в поле ввода (см. spec §11.5), так что многоскалярные эмодзи (`❤️`, `👍🏽`)
//! вставляются и редактируются без сюрпризов.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::shared::theme::Palette;

/// Самые ходовые эмодзи (4 ряда по [`COLS`]). Порядок — от «лиц» к жестам и
/// символам; список фиксирован (расширять — правкой здесь). Ширина сетки выбрана
/// так, чтобы нижняя подсказка попапа помещалась целиком.
const EMOJIS: &[&str] = &[
    "😀", "😃", "😄", "😁", "😆", "😅", "😂", "🤣", "😊", "😍", "😘", //
    "😎", "🤔", "😉", "🙂", "🥳", "😴", "😭", "😢", "😡", "🤯", "🙄", //
    "👍", "👎", "👏", "🙏", "🙌", "💪", "🤝", "👀", "🤷", "👌", "✌️", //
    "❤️", "🔥", "✨", "🎉", "💯", "✅", "❌", "⭐", "🚀", "💔", "🎯", //
];

/// Число эмодзи в ряду сетки.
const COLS: usize = 11;

/// Ширина ячейки эмодзи в колонках (` 😀 ` — пробел + эмодзи шириной 2 + пробел).
const CELL_W: u16 = 4;

/// Действие, которое попап просит выполнить экран чата.
#[derive(Debug, Clone, PartialEq)]
pub enum EmojiPickerAction {
    /// Нажатие обработано внутри попапа (перерисовать).
    None,
    /// Закрыть попап без вставки.
    Cancel,
    /// Вставить эмодзи в поле ввода и закрыть попап.
    Pick(String),
}

/// Состояние попапа выбора эмодзи.
pub struct EmojiPickerState {
    /// Индекс выделенного эмодзи в [`EMOJIS`].
    selected: usize,
}

impl Default for EmojiPickerState {
    fn default() -> Self {
        Self::new()
    }
}

impl EmojiPickerState {
    /// Открывает попап (выделение — на первом эмодзи).
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    /// Открывает попап с восстановленным выделением (индекс клампится в границы) —
    /// чтобы попап «помнил» последний выбранный эмодзи между открытиями.
    pub fn with_selected(idx: usize) -> Self {
        Self {
            selected: idx.min(EMOJIS.len() - 1),
        }
    }

    /// Текущий индекс выделения (экран сохраняет его, чтобы восстановить при
    /// следующем открытии через [`Self::with_selected`]).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Эмодзи под выделением (всегда валиден: список непуст и индекс клампится).
    fn selected_emoji(&self) -> String {
        EMOJIS[self.selected.min(EMOJIS.len() - 1)].to_string()
    }

    /// Обрабатывает нажатие клавиши, возвращая действие для исполнения. Навигация —
    /// стрелками по сетке; `Enter` вставляет, `Esc` закрывает.
    pub fn on_key(&mut self, key: KeyEvent) -> EmojiPickerAction {
        if key.kind != KeyEventKind::Press {
            return EmojiPickerAction::None;
        }
        match key.code {
            KeyCode::Esc => EmojiPickerAction::Cancel,
            KeyCode::Enter => EmojiPickerAction::Pick(self.selected_emoji()),
            KeyCode::Left => {
                self.selected = self.selected.saturating_sub(1);
                EmojiPickerAction::None
            }
            KeyCode::Right => {
                self.selected = (self.selected + 1).min(EMOJIS.len() - 1);
                EmojiPickerAction::None
            }
            KeyCode::Up => {
                if self.selected >= COLS {
                    self.selected -= COLS;
                }
                EmojiPickerAction::None
            }
            KeyCode::Down => {
                if self.selected + COLS < EMOJIS.len() {
                    self.selected += COLS;
                }
                EmojiPickerAction::None
            }
            _ => EmojiPickerAction::None,
        }
    }

    /// Рисует попап по центру `area`: сетка эмодзи, выделенная ячейка — reversed.
    pub fn render(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let rows = EMOJIS.len().div_ceil(COLS) as u16;
        let width = COLS as u16 * CELL_W + 2; // +2 — рамка
        let popup = centered_rect(width, rows + 2, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel("☺ Эмодзи", true)
            .title_bottom(Line::from(Span::styled(
                " ←↑↓→ выбор · Enter вставить · Esc отмена ",
                palette.muted_style(),
            )));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let selected = self.selected.min(EMOJIS.len() - 1);
        let lines: Vec<Line> = EMOJIS
            .chunks(COLS)
            .enumerate()
            .map(|(r, row)| {
                let spans: Vec<Span> = row
                    .iter()
                    .enumerate()
                    .map(|(c, emoji)| {
                        let idx = r * COLS + c;
                        // Пробелы вокруг эмодзи: глиф шириной 2, плюс по краю — поэтому
                        // ` {emoji} ` даёт ровную ячейку и «кнопочный» вид у выделенного.
                        let cell = format!(" {emoji} ");
                        if idx == selected {
                            // Тёмный фон выделения (как у выбранной строки в списке
                            // чатов) — не сливается с цветным глифом, в отличие от
                            // reversed (светлый фон затирал эмодзи).
                            Span::styled(cell, Style::new().fg(palette.text).bg(palette.keycap_bg))
                        } else {
                            Span::styled(cell, Style::new().fg(palette.text))
                        }
                    })
                    .collect();
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// Прямоугольник по центру `area` фиксированной ширины/высоты (с клампом).
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_picks_first_by_default() {
        let mut s = EmojiPickerState::new();
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            EmojiPickerAction::Pick(EMOJIS[0].to_string())
        );
    }

    #[test]
    fn arrows_navigate_grid() {
        let mut s = EmojiPickerState::new();
        // вправо → второй эмодзи в первом ряду
        assert_eq!(s.on_key(key(KeyCode::Right)), EmojiPickerAction::None);
        assert_eq!(s.selected, 1);
        // вниз → на ряд ниже (тот же столбец)
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected, 1 + COLS);
        // вверх → обратно
        s.on_key(key(KeyCode::Up));
        assert_eq!(s.selected, 1);
        // влево → к первому
        s.on_key(key(KeyCode::Left));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn navigation_clamps_at_bounds() {
        let mut s = EmojiPickerState::new();
        // влево/вверх с первой ячейки — без движения
        s.on_key(key(KeyCode::Left));
        s.on_key(key(KeyCode::Up));
        assert_eq!(s.selected, 0);
        // дойти до последней ячейки и попробовать дальше — кламп
        for _ in 0..EMOJIS.len() + 5 {
            s.on_key(key(KeyCode::Right));
        }
        assert_eq!(s.selected, EMOJIS.len() - 1);
        // вниз с последнего ряда — без движения
        let last = s.selected;
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected, last);
    }

    #[test]
    fn with_selected_restores_and_clamps() {
        // Восстанавливает индекс…
        assert_eq!(EmojiPickerState::with_selected(5).selected(), 5);
        // …и клампит вышедший за границы в последний элемент.
        assert_eq!(
            EmojiPickerState::with_selected(9999).selected(),
            EMOJIS.len() - 1
        );
    }

    #[test]
    fn esc_cancels() {
        let mut s = EmojiPickerState::new();
        assert_eq!(s.on_key(key(KeyCode::Esc)), EmojiPickerAction::Cancel);
    }

    #[test]
    fn grid_is_full_rows() {
        // Список ровно заполняет ряды по COLS — иначе сетка «рваная».
        assert_eq!(EMOJIS.len() % COLS, 0);
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let s = EmojiPickerState::new();
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f, f.area(), &Palette::default()))
                .unwrap();
        }
    }
}
