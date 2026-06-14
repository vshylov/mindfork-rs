//! Собственный многострочный редактор ввода (см. ADR 0001: `tui-textarea`
//! несовместим с ratatui 0.30, а свой виджет даёт контроль над `Shift+Enter`,
//! скроллом и — позже — подсветкой ошибок спелл-чека). См. spec §11.5.
//!
//! Хранит строки как `Vec<Vec<char>>`: индекс курсора — это индекс символа,
//! без забот о границах UTF-8. Политику «`Enter` отправляет / `Shift+Enter`
//! переносит» решает вызывающий слой; виджет занимается только редактированием.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Многострочное поле ввода с курсором.
pub struct InputBox {
    /// Логические строки (символы). Всегда непусто (минимум одна строка).
    lines: Vec<Vec<char>>,
    /// Строка курсора.
    row: usize,
    /// Столбец курсора (индекс символа в строке; может равняться длине строки).
    col: usize,
    /// Первая видимая строка (вертикальный скролл).
    scroll: usize,
}

impl Default for InputBox {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBox {
    pub fn new() -> Self {
        Self {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            scroll: 0,
        }
    }

    /// Текст поля (строки через `\n`).
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Пусто ли поле (одна пустая строка).
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Число логических строк.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Очищает поле.
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.scroll = 0;
    }

    /// Заполняет поле текстом, ставит курсор в конец (для правки по месту, M3+).
    pub fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![Vec::new()]
        } else {
            text.split('\n').map(|l| l.chars().collect()).collect()
        };
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
        self.scroll = 0;
    }

    // ---------- редактирование ----------

    pub fn insert_char(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
    }

    pub fn insert_newline(&mut self) {
        let tail = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            self.lines[self.row].remove(self.col);
        } else if self.row > 0 {
            // склейка с предыдущей строкой
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(current);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
        }
    }

    // ---------- движение курсора ----------

    fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    fn move_right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    /// Обрабатывает клавишу редактирования. Возвращает `true`, если клавиша
    /// обработана (вызывающий не трактует её дальше). `Enter` НЕ обрабатывается
    /// (политику отправки/переноса задаёт вызывающий слой).
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        match key.code {
            KeyCode::Char(c) => {
                self.insert_char(c);
                true
            }
            KeyCode::Backspace => {
                self.backspace();
                true
            }
            KeyCode::Delete => {
                self.delete();
                true
            }
            KeyCode::Left => {
                self.move_left();
                true
            }
            KeyCode::Right => {
                self.move_right();
                true
            }
            KeyCode::Up => {
                self.move_up();
                true
            }
            KeyCode::Down => {
                self.move_down();
                true
            }
            KeyCode::Home => {
                self.col = 0;
                true
            }
            KeyCode::End => {
                self.col = self.lines[self.row].len();
                true
            }
            _ => false,
        }
    }

    /// Рисует поле в `area` с рамкой и заголовком. При `focused` ставит курсор.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {title} "));
        let inner = block.inner(area);
        frame.render_widget(&block, area);

        let visible_rows = inner.height.max(1) as usize;
        self.adjust_scroll(visible_rows);

        let lines: Vec<Line> = self
            .lines
            .iter()
            .skip(self.scroll)
            .take(visible_rows)
            .map(|l| Line::from(l.iter().collect::<String>()))
            .collect();
        let placeholder = self.is_empty() && !focused;
        let text = if placeholder {
            Text::from(Line::from("введите сообщение…").dim())
        } else {
            Text::from(lines)
        };
        frame.render_widget(Paragraph::new(text), inner);

        if focused {
            let cursor_y = inner.y + (self.row.saturating_sub(self.scroll)) as u16;
            let cursor_x = inner.x + self.col as u16;
            // не выходим за пределы внутренней области
            let x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
            let y = cursor_y.min(inner.y + inner.height.saturating_sub(1));
            frame.set_cursor_position((x, y));
        }
    }

    /// Держит курсор в видимой области (вертикальный скролл).
    fn adjust_scroll(&mut self, visible_rows: usize) {
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if visible_rows > 0 && self.row >= self.scroll + visible_rows {
            self.scroll = self.row + 1 - visible_rows;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(ib: &mut InputBox, s: &str) {
        for c in s.chars() {
            ib.insert_char(c);
        }
    }

    #[test]
    fn new_is_empty() {
        let ib = InputBox::new();
        assert!(ib.is_empty());
        assert_eq!(ib.text(), "");
    }

    #[test]
    fn typing_and_text() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "привет");
        assert!(!ib.is_empty());
        assert_eq!(ib.text(), "привет");
    }

    #[test]
    fn newline_splits_at_cursor() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "abcd");
        ib.move_left();
        ib.move_left(); // курсор между b и c
        ib.insert_newline();
        assert_eq!(ib.text(), "ab\ncd");
        assert_eq!(ib.line_count(), 2);
    }

    #[test]
    fn backspace_joins_lines() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab");
        ib.insert_newline();
        type_str(&mut ib, "cd");
        // курсор в начале второй строки? нет — в конце "cd". Идём в начало строки.
        ib.col = 0;
        ib.backspace();
        assert_eq!(ib.text(), "abcd");
        assert_eq!(ib.line_count(), 1);
    }

    #[test]
    fn delete_at_eol_joins_next() {
        let mut ib = InputBox::new();
        ib.set_text("ab\ncd");
        ib.row = 0;
        ib.col = 2; // конец первой строки
        ib.delete();
        assert_eq!(ib.text(), "abcd");
    }

    #[test]
    fn unicode_cursor_is_char_based() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ёжик");
        ib.backspace(); // удалить 'к'
        assert_eq!(ib.text(), "ёжи");
        ib.move_left();
        ib.insert_char('!'); // курсор был на позиции 2 → между ж и и
        assert_eq!(ib.text(), "ёж!и");
    }

    #[test]
    fn clear_resets() {
        let mut ib = InputBox::new();
        ib.set_text("hello\nworld");
        ib.clear();
        assert!(ib.is_empty());
        assert_eq!(ib.line_count(), 1);
    }

    #[test]
    fn on_key_handles_editing_but_not_enter() {
        let mut ib = InputBox::new();
        assert!(ib.on_key(k(KeyCode::Char('x'))));
        assert!(ib.on_key(k(KeyCode::Backspace)));
        assert!(!ib.on_key(k(KeyCode::Enter)));
        assert!(ib.is_empty());
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("строка 1\nстрока 2\nстрока 3");
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true)).unwrap();
    }
}
