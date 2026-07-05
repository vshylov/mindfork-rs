//! Собственный многострочный редактор ввода (см. ADR 0001: `tui-textarea`
//! несовместим с ratatui 0.30, а свой виджет даёт контроль над `Shift+Enter`,
//! скроллом и — позже — подсветкой ошибок спелл-чека). См. spec §11.5.
//!
//! Хранит строки как `Vec<Vec<char>>`: индекс курсора — это индекс символа,
//! без забот о границах UTF-8. Политику «`Enter` отправляет / `Shift+Enter`
//! переносит» решает вызывающий слой; виджет занимается только редактированием.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap;

/// Ширина колонки приглашения `❯ ` (в колонках) перед текстом ввода.
const PROMPT_W: u16 = 2;

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
    /// Однострочный режим: значение не переносится по словам, а **скроллится
    /// горизонтально**; `↑/↓` и перевод строки отключены; `Home/End` — к началу/
    /// концу логической строки. Для редактируемых полей настроек, где значение
    /// логически одна строка (URL, путь, число). По умолчанию выключен —
    /// чат-ввод многострочный. См. spec §11.6.
    single_line: bool,
    /// Горизонтальный скролл в колонках (только однострочный режим): первая видимая
    /// колонка. Держит курсор в видимой области по аналогии с вертикальным `scroll`.
    hscroll: usize,
    /// Ширина внутренней области последней отрисовки (в колонках). Нужна навигации
    /// `↑/↓`, чтобы ходить по **визуальным** рядам перенесённой строки, а не по
    /// логическим строкам (перенос считается только при рендере). `0` — рендера ещё
    /// не было: тогда `↑/↓` падают на логический переход. См. [`Self::move_up`].
    last_width: usize,
    /// «Целевая» визуальная колонка серии `↑/↓` (в колонках). Запоминается при первом
    /// вертикальном переходе и держится, пока курсор не сдвинут иначе — тогда серия
    /// `↑/↓` через короткие ряды сохраняет исходную колонку (как в больших редакторах).
    /// Любое горизонтальное движение/правка сбрасывает в `None`. См. [`Self::move_up`].
    goal_col: Option<usize>,
    /// Диапазоны слов с ошибками орфографии по логическим строкам (индекс строки
    /// → отсортированные непересекающиеся `[start, end)` в символах). Заполняет
    /// экран из спелл-чекера; виджет лишь подчёркивает. См. spec §11.5.
    misspelled: Vec<Vec<(usize, usize)>>,
    /// Буфер «отмены» хоткея «удалить весь текст» ([`Self::clear_or_restore`]):
    /// текст, удалённый последним нажатием, чтобы повторное нажатие его вернуло.
    /// `Some` только пока после удаления **ничего не вводилось** — любой ввод
    /// текста (`insert_*`/`replace_range`/`set_text`) инвалидирует буфер в `None`.
    /// См. spec §11.5.
    cleared: Option<String>,
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
            single_line: false,
            hscroll: 0,
            last_width: 0,
            goal_col: None,
            misspelled: Vec::new(),
            cleared: None,
        }
    }

    /// Включает однострочный режим (горизонтальный скролл вместо переноса; `↑/↓` и
    /// перевод строки отключены). Вызывать **до** [`Self::set_text`]. См. поле
    /// [`Self::single_line`].
    pub fn set_single_line(&mut self, on: bool) {
        self.single_line = on;
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

    /// Число логических строк. Высоту поля теперь считает [`Self::visual_line_count`]
    /// (учитывает перенос); метод оставлен как естественный аккомпанемент и для тестов.
    #[allow(dead_code)]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Число визуальных рядов при ширине `width` (с учётом переноса). Производный
    /// путь высоты поля теперь использует [`Self::content_rows`] (он сам вычитает
    /// рамку и колонку приглашения); метод оставлен для тестов как тонкая обёртка
    /// над [`Self::visual_rows`].
    #[cfg(test)]
    pub fn visual_line_count(&self, width: usize) -> usize {
        self.visual_rows(width).len()
    }

    /// Число визуальных рядов содержимого при отрисовке в область **внешней** ширины
    /// `area_width` (вместе с рамкой). Вычитает рамку (2) и колонку приглашения
    /// ([`PROMPT_W`]) — ровно ту же ширину текста, что использует [`Self::render`].
    /// Слой выше считает высоту поля по этому методу, чтобы она совпадала с реальным
    /// переносом: иначе расчёт высоты по «ширине минус рамка» завышал бы доступную
    /// ширину на [`PROMPT_W`] и поле не росло бы на один-два символа за границей
    /// переноса (курсор прижимался к краю, см. spec §11.5). В однострочном режиме
    /// перенос отключён — всегда один ряд.
    pub fn content_rows(&self, area_width: u16) -> usize {
        if self.single_line {
            return 1;
        }
        let text_w = area_width.saturating_sub(2).saturating_sub(PROMPT_W).max(1) as usize;
        self.visual_rows(text_w).len()
    }

    /// Очищает поле. Сбрасывает и буфер отмены [`Self::cleared`] (после отправки
    /// сообщения «вернуть удалённое» не должно воскрешать уже отправленный текст).
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.misspelled.clear();
        self.cleared = None;
    }

    /// Хоткей «удалить весь текст / вернуть удалённое» (`Ctrl+K`, spec §11.5).
    /// Если поле непусто — запоминает текст и очищает поле. Если поле пусто, а в
    /// буфере есть ранее удалённый текст (с тех пор ничего не вводилось) —
    /// восстанавливает его (курсор в конец). Любой ввод между нажатиями
    /// инвалидирует буфер (см. [`Self::cleared`]), поэтому восстановить можно лишь
    /// сразу после удаления.
    pub fn clear_or_restore(&mut self) {
        if !self.is_empty() {
            let text = self.text();
            self.clear(); // сбрасывает cleared в None
            self.cleared = Some(text);
        } else if let Some(text) = self.cleared.take() {
            self.set_text(&text); // set_text тоже сбросит cleared (уже None)
        }
    }

    /// Текст по логическим строкам (для спелл-чека построчно).
    pub fn line_strings(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Позиция курсора `(строка, столбец)` в индексах символов.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Устанавливает диапазоны ошибок орфографии (по строкам). См. [`Self::misspelled`].
    pub fn set_misspelled(&mut self, ranges: Vec<Vec<(usize, usize)>>) {
        self.misspelled = ranges;
    }

    /// Нет ли отмеченных ошибок орфографии (для тестов вышестоящего слоя).
    #[cfg(test)]
    pub fn misspelled_is_empty(&self) -> bool {
        self.misspelled.iter().all(|r| r.is_empty())
    }

    /// Заменяет диапазон символов `[start, end)` в строке `row` на `replacement`
    /// и ставит курсор за вставленным текстом (для применения подсказки).
    pub fn replace_range(&mut self, row: usize, start: usize, end: usize, replacement: &str) {
        let Some(line) = self.lines.get_mut(row) else {
            return;
        };
        let end = end.min(line.len());
        let start = start.min(end);
        let repl: Vec<char> = replacement.chars().collect();
        let repl_len = repl.len();
        line.splice(start..end, repl);
        self.row = row;
        self.col = start + repl_len;
        self.goal_col = None;
        self.cleared = None;
    }

    /// Заполняет поле текстом, ставит курсор в конец (для правки по месту, M3+).
    pub fn set_text(&mut self, text: &str) {
        // В однострочном режиме сохраняем инвариант «одна логическая строка»:
        // переводы строк схлопываем в пробел.
        let owned;
        let text = if self.single_line && text.contains('\n') {
            owned = text.replace('\n', " ");
            owned.as_str()
        } else {
            text
        };
        self.lines = if text.is_empty() {
            vec![Vec::new()]
        } else {
            text.split('\n').map(|l| l.chars().collect()).collect()
        };
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.cleared = None;
    }

    // ---------- редактирование ----------

    pub fn insert_char(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
        self.goal_col = None;
        self.cleared = None;
    }

    pub fn insert_newline(&mut self) {
        if self.single_line {
            return; // в однострочном режиме перевод строки запрещён
        }
        let tail = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
        self.goal_col = None;
        self.cleared = None;
    }

    /// Вставляет произвольный текст в позицию курсора (вставка из буфера обмена).
    /// Переводы строк (`\n`) разбивают текущую логическую строку на новые; `\r`
    /// нормализуются (`\r\n`/`\r` → `\n`), `\t` разворачивается в пробелы. Курсор
    /// встаёт в конец вставленного. Один проход без посимвольной петли — поэтому
    /// большая вставка не тормозит (см. bracketed paste, spec §11.5).
    pub fn insert_str(&mut self, text: &str) {
        // Хвост текущей строки после курсора — приклеим к последней вставленной.
        let tail: Vec<char> = self.lines[self.row].split_off(self.col);
        // В однострочном режиме переводы строк превращаем в пробелы (одна строка).
        let normalized = normalize_paste(text);
        let normalized = if self.single_line {
            normalized.replace('\n', " ")
        } else {
            normalized
        };
        let mut first = true;
        for segment in normalized.split('\n') {
            if first {
                first = false;
            } else {
                // Новый перевод строки: заводим следующую логическую строку.
                self.row += 1;
                self.lines.insert(self.row, Vec::new());
            }
            self.lines[self.row].extend(segment.chars());
        }
        self.col = self.lines[self.row].len();
        self.lines[self.row].extend(tail);
        self.goal_col = None;
        self.cleared = None;
    }

    pub fn backspace(&mut self) {
        self.goal_col = None;
        if self.col > 0 {
            // Удаляем кластер целиком (`❤️`/`👍🏽` — несколько скаляров), а не один
            // скаляр — иначе остаётся осиротевший вариатор/модификатор. См. spec §11.5.
            let start = wrap::prev_boundary(&self.lines[self.row], self.col);
            self.lines[self.row].drain(start..self.col);
            self.col = start;
        } else if self.row > 0 {
            // склейка с предыдущей строкой
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(current);
        }
    }

    pub fn delete(&mut self) {
        self.goal_col = None;
        if self.col < self.lines[self.row].len() {
            // Удаляем кластер целиком (зеркально `backspace`), а не один скаляр.
            let end = wrap::next_boundary(&self.lines[self.row], self.col);
            self.lines[self.row].drain(self.col..end);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
        }
    }

    // ---------- движение курсора ----------

    fn move_left(&mut self) {
        self.goal_col = None;
        if self.col > 0 {
            // По графемному кластеру, а не по скаляру (см. `backspace`/spec §11.5).
            self.col = wrap::prev_boundary(&self.lines[self.row], self.col);
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    fn move_right(&mut self) {
        self.goal_col = None;
        if self.col < self.lines[self.row].len() {
            self.col = wrap::next_boundary(&self.lines[self.row], self.col);
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// Влево на одно слово (`Ctrl+Left`): пропускает пробелы слева, затем символы
    /// слова — курсор встаёт в начало слова. В начале логической строки переходит
    /// в конец предыдущей (одно нажатие = одна граница, как в больших редакторах).
    fn move_word_left(&mut self) {
        self.goal_col = None;
        if self.col == 0 {
            if self.row > 0 {
                self.row -= 1;
                self.col = self.lines[self.row].len();
            }
            return;
        }
        self.col = self.word_left_col();
    }

    /// Вправо на одно слово (`Ctrl+Right`): пропускает пробелы справа, затем символы
    /// слова — курсор встаёт за концом слова. В конце логической строки переходит в
    /// начало следующей.
    fn move_word_right(&mut self) {
        self.goal_col = None;
        if self.col >= self.lines[self.row].len() {
            if self.row + 1 < self.lines.len() {
                self.row += 1;
                self.col = 0;
            }
            return;
        }
        self.col = self.word_right_col();
    }

    /// Граница слова слева от курсора **в пределах текущей строки** (для пословного
    /// движения и удаления): пропускает пробелы, затем символы слова. См.
    /// [`Self::move_word_left`].
    fn word_left_col(&self) -> usize {
        let line = &self.lines[self.row];
        let mut i = self.col;
        while i > 0 && line[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !line[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// Граница слова справа от курсора **в пределах текущей строки** (зеркально
    /// [`Self::word_left_col`]).
    fn word_right_col(&self) -> usize {
        let line = &self.lines[self.row];
        let mut i = self.col;
        while i < line.len() && line[i].is_whitespace() {
            i += 1;
        }
        while i < line.len() && !line[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Удаляет слово слева от курсора (`Ctrl+Backspace`). В начале строки склеивает
    /// со строкой выше (как обычный `Backspace`).
    fn delete_word_left(&mut self) {
        self.goal_col = None;
        self.cleared = None;
        if self.col == 0 {
            self.backspace();
            return;
        }
        let start = self.word_left_col();
        self.lines[self.row].drain(start..self.col);
        self.col = start;
    }

    /// Удаляет слово справа от курсора (`Ctrl+Delete`). В конце строки склеивает со
    /// строкой ниже (как обычный `Delete`).
    fn delete_word_right(&mut self) {
        self.goal_col = None;
        self.cleared = None;
        if self.col >= self.lines[self.row].len() {
            self.delete();
            return;
        }
        let end = self.word_right_col();
        self.lines[self.row].drain(self.col..end);
    }

    /// В самое начало текста (`Ctrl+Home`).
    fn move_doc_start(&mut self) {
        self.goal_col = None;
        self.row = 0;
        self.col = 0;
    }

    /// В самый конец текста (`Ctrl+End`).
    fn move_doc_end(&mut self) {
        self.goal_col = None;
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }

    /// Вверх по **визуальному** ряду: если логическая строка перенесена, `↑` идёт на
    /// предыдущий визуальный ряд той же строки, сохраняя колонку. Использует ширину
    /// последней отрисовки; до первого рендера (`last_width == 0`) — логический переход.
    fn move_up(&mut self) {
        if self.single_line {
            return; // однострочное поле — `↑` не двигает курсор
        }
        if self.last_width == 0 {
            self.goal_col = None;
            self.move_up_logical();
            return;
        }
        let vrows = self.visual_rows(self.last_width);
        let (vrow, vcol) = self.cursor_visual(&vrows);
        // Первый шаг серии запоминает колонку; дальше держим её (goal-column).
        let goal = *self.goal_col.get_or_insert(vcol);
        if vrow == 0 {
            return; // уже верхний визуальный ряд (goal сохранён для обратного ↓)
        }
        let (li, start, end) = vrows[vrow - 1];
        let col = col_for_visual(&self.lines[li], start, end, goal, is_soft(&vrows, vrow - 1));
        self.row = li;
        self.col = col;
    }

    /// Вниз по **визуальному** ряду (зеркально [`Self::move_up`]).
    fn move_down(&mut self) {
        if self.single_line {
            return; // однострочное поле — `↓` не двигает курсор
        }
        if self.last_width == 0 {
            self.goal_col = None;
            self.move_down_logical();
            return;
        }
        let vrows = self.visual_rows(self.last_width);
        let (vrow, vcol) = self.cursor_visual(&vrows);
        let goal = *self.goal_col.get_or_insert(vcol);
        if vrow + 1 >= vrows.len() {
            return; // уже нижний визуальный ряд
        }
        let (li, start, end) = vrows[vrow + 1];
        let col = col_for_visual(&self.lines[li], start, end, goal, is_soft(&vrows, vrow + 1));
        self.row = li;
        self.col = col;
    }

    /// `Home` — в начало текущего **визуального** ряда (не всей логической строки).
    /// До первого рендера — в начало логической строки.
    fn move_home(&mut self) {
        self.goal_col = None;
        if self.single_line {
            self.col = 0; // однострочное поле — к началу значения
            return;
        }
        if self.last_width == 0 {
            self.col = 0;
            return;
        }
        let vrows = self.visual_rows(self.last_width);
        let (vrow, _) = self.cursor_visual(&vrows);
        self.col = vrows[vrow].1;
    }

    /// `End` — в конец текущего **визуального** ряда. На мягком переносе встаёт на
    /// последнюю позицию этого ряда (не уезжает в начало следующего, см. `is_soft`).
    /// До первого рендера — в конец логической строки.
    fn move_end(&mut self) {
        self.goal_col = None;
        if self.single_line {
            self.col = self.lines[self.row].len(); // однострочное поле — к концу значения
            return;
        }
        if self.last_width == 0 {
            self.col = self.lines[self.row].len();
            return;
        }
        let vrows = self.visual_rows(self.last_width);
        let (vrow, _) = self.cursor_visual(&vrows);
        let (li, start, end) = vrows[vrow];
        self.col = col_for_visual(
            &self.lines[li],
            start,
            end,
            usize::MAX,
            is_soft(&vrows, vrow),
        );
    }

    /// Логический переход вверх/вниз (фолбэк до первого рендера, когда ширина и,
    /// значит, перенос ещё неизвестны).
    fn move_up_logical(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    fn move_down_logical(&mut self) {
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
        // Ctrl усиливает навигацию/удаление до уровня слова / всего текста
        // (`Ctrl+←/→` — по словам, `Ctrl+Backspace/Delete` — удалить слово,
        // `Ctrl+Home/End` — в начало/конец текста). См. spec §11.5.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Backspace if ctrl => {
                self.delete_word_left();
                true
            }
            KeyCode::Delete if ctrl => {
                self.delete_word_right();
                true
            }
            KeyCode::Left if ctrl => {
                self.move_word_left();
                true
            }
            KeyCode::Right if ctrl => {
                self.move_word_right();
                true
            }
            KeyCode::Home if ctrl => {
                self.move_doc_start();
                true
            }
            KeyCode::End if ctrl => {
                self.move_doc_end();
                true
            }
            // Обычный ввод символа: Ctrl+символ не печатаем (это шорткат вышестоящего
            // слоя), иначе в поле попал бы управляющий символ.
            KeyCode::Char(c) if !ctrl => {
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
                self.move_home();
                true
            }
            KeyCode::End => {
                self.move_end();
                true
            }
            _ => false,
        }
    }

    /// Рисует поле в `area` с рамкой и заголовком. При `focused` ставит курсор.
    /// При `command` весь текст подсвечивается цветом `warning` (это команда вроде
    /// `/rag …`), а подчёркивания орфографии не рисуются. См. spec §11.5.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        focused: bool,
        palette: &Palette,
        command: bool,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(palette.glyphs().border)
            .border_style(palette.border_style(focused))
            .title(Span::styled(format!(" {title} "), palette.muted_style()));
        let full_inner = block.inner(area);
        frame.render_widget(&block, area);

        // Колонка приглашения `❯` слева; текст рисуется правее.
        let prompt_style = if focused {
            Style::new().fg(palette.assistant)
        } else {
            palette.muted_style()
        };
        if full_inner.width > PROMPT_W {
            let prompt_area = Rect {
                height: 1,
                ..full_inner
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    palette.glyphs().prompt,
                    prompt_style,
                ))),
                prompt_area,
            );
        }
        // Внутренняя область под текст — без колонки приглашения.
        let inner = Rect {
            x: full_inner.x + PROMPT_W,
            width: full_inner.width.saturating_sub(PROMPT_W),
            ..full_inner
        };

        if self.single_line {
            self.render_single_line(frame, inner, focused, palette, command);
            return;
        }

        let view_w = inner.width.max(1) as usize;
        let visible_rows = inner.height.max(1) as usize;
        // Запоминаем ширину для навигации `↑/↓` по визуальным рядам (см. `move_up`).
        self.last_width = view_w;

        // Визуальные ряды с учётом переноса; позиция курсора — через тот же перенос
        // (единый источник истины, иначе курсор разъедется с текстом).
        let vrows = self.visual_rows(view_w);
        let (cursor_row, cursor_col) = self.cursor_visual(&vrows);
        self.adjust_scroll(cursor_row, vrows.len(), visible_rows);

        // В режиме команды весь текст красим в `warning` и не подчёркиваем ошибки.
        let cmd_style = command.then(|| Style::new().fg(palette.warning));
        let lines: Vec<Line> = vrows
            .iter()
            .skip(self.scroll)
            .take(visible_rows)
            .map(|&(li, start, end)| {
                let sub = &self.lines[li][start..end];
                if let Some(style) = cmd_style {
                    Line::styled(sub.iter().collect::<String>(), style)
                } else {
                    let ranges = self
                        .misspelled
                        .get(li)
                        .map(|rs| clip_ranges(rs, start, end));
                    styled_line(sub, ranges.as_deref(), palette)
                }
            })
            .collect();
        let placeholder = self.is_empty() && !focused;
        let text = if placeholder {
            Text::from(Line::from("введите сообщение…").dim())
        } else {
            Text::from(lines)
        };
        frame.render_widget(Paragraph::new(text), inner);

        // Скроллбар на правой рамке — когда визуальных рядов больше, чем видно
        // (поле выросло до потолка высоты и прокручивается). Цвет трека — как у
        // рамки поля (она зависит от фокуса).
        render_scrollbar(
            frame,
            area.inner(Margin::new(0, 1)),
            vrows.len(),
            visible_rows,
            self.scroll,
            focused,
            palette,
        );

        if focused {
            let cursor_y = inner.y + (cursor_row.saturating_sub(self.scroll)) as u16;
            let cursor_x = inner.x + cursor_col as u16;
            // не выходим за пределы внутренней области
            let x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
            let y = cursor_y.min(inner.y + inner.height.saturating_sub(1));
            frame.set_cursor_position((x, y));
        }
    }

    /// Рисует значение в однострочном режиме: без переноса, с горизонтальным
    /// скроллом — курсор всегда виден, длинное значение «уезжает» влево, а не
    /// заворачивается на невидимый ряд. `inner` — внутренняя область (уже без рамки).
    fn render_single_line(
        &mut self,
        frame: &mut Frame,
        inner: Rect,
        focused: bool,
        palette: &Palette,
        command: bool,
    ) {
        let view_w = inner.width.max(1) as usize;
        self.last_width = view_w;
        let line = &self.lines[0];
        let cursor_vw = wrap::display_width(&line[..self.col]);

        // Горизонтальный скролл держит курсор в видимой области.
        if cursor_vw < self.hscroll {
            self.hscroll = cursor_vw;
        } else if cursor_vw >= self.hscroll + view_w {
            self.hscroll = cursor_vw + 1 - view_w;
        }

        // Видимый срез [start, end) — от колонки `hscroll` на ширину `view_w`.
        let start = col_at_width(line, self.hscroll);
        let mut end = start;
        let mut w = 0;
        while end < line.len() {
            let cw = wrap::width_at(line, end);
            if w + cw > view_w {
                break;
            }
            w += cw;
            end += 1;
        }
        let sub = &line[start..end];

        let placeholder = self.is_empty() && !focused;
        let text = if placeholder {
            Text::from(Line::from("введите сообщение…").dim())
        } else if let Some(style) = command.then(|| Style::new().fg(palette.warning)) {
            Text::from(Line::styled(sub.iter().collect::<String>(), style))
        } else {
            let ranges = self
                .misspelled
                .first()
                .map(|rs| clip_ranges(rs, start, end));
            Text::from(styled_line(sub, ranges.as_deref(), palette))
        };
        frame.render_widget(Paragraph::new(text), inner);

        if focused {
            let cursor_x = inner.x + (cursor_vw - self.hscroll) as u16;
            let x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
            frame.set_cursor_position((x, inner.y));
        }
    }

    /// Визуальные ряды: для каждого — `(логическая строка, начало, конец)` в
    /// индексах символов этой строки (с учётом переноса по ширине `width`).
    fn visual_rows(&self, width: usize) -> Vec<(usize, usize, usize)> {
        let mut rows = Vec::new();
        for (li, chars) in self.lines.iter().enumerate() {
            for (start, end) in wrap::wrap_ranges(chars, width) {
                rows.push((li, start, end));
            }
        }
        rows
    }

    /// Позиция курсора в визуальных координатах `(индекс ряда, столбец-колонки)`.
    /// На мягком переносе (курсор в конце ряда, но не в конце логической строки)
    /// курсор уходит на начало следующего ряда.
    fn cursor_visual(&self, vrows: &[(usize, usize, usize)]) -> (usize, usize) {
        let mut last: Option<(usize, usize)> = None; // (индекс ряда, начало)
        for (idx, &(li, start, end)) in vrows.iter().enumerate() {
            if li != self.row {
                continue;
            }
            last = Some((idx, start));
            if self.col < end {
                let col = wrap::display_width(&self.lines[li][start..self.col]);
                return (idx, col);
            }
        }
        // курсор в самом конце логической строки — последний её ряд
        match last {
            Some((idx, start)) => (
                idx,
                wrap::display_width(&self.lines[self.row][start..self.col]),
            ),
            None => (0, 0),
        }
    }

    /// Держит курсор в видимой области (вертикальный скролл по визуальным рядам).
    fn adjust_scroll(&mut self, cursor_row: usize, total: usize, visible_rows: usize) {
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        } else if visible_rows > 0 && cursor_row >= self.scroll + visible_rows {
            self.scroll = cursor_row + 1 - visible_rows;
        }
        // не оставляем пустоту снизу, если рядов стало меньше (удаление/перенос)
        let max_scroll = total.saturating_sub(visible_rows);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
    }
}

/// Нормализует текст из буфера обмена перед вставкой: `\r\n`/`\r` → `\n`
/// (единый перевод строки), `\t` → пробелы. Прочие управляющие символы оставляем
/// как есть (терминал/рендер их отфильтруют).
fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
}

/// Индекс символа, на котором накопленная ширина строки достигает `target` колонок
/// (для горизонтального скролла однострочного поля). Значения `target` приходят из
/// префиксных ширин — границы символов совпадают, дробления широкого символа нет.
fn col_at_width(line: &[char], target: usize) -> usize {
    let mut w = 0;
    let mut i = 0;
    while i < line.len() && w < target {
        w += wrap::width_at(line, i);
        i += 1;
    }
    i
}

/// Визуальный ряд `idx` — мягкий перенос (не последний ряд своей логической строки),
/// т.е. следующий ряд принадлежит той же строке. Тогда позиция курсора `== end`
/// рисуется в начале следующего ряда — навигация это учитывает.
fn is_soft(vrows: &[(usize, usize, usize)], idx: usize) -> bool {
    idx + 1 < vrows.len() && vrows[idx + 1].0 == vrows[idx].0
}

/// Логический столбец на ряду `[start, end)`, ближайший к целевой визуальной колонке
/// `target_vw` (в колонках) — для перехода `↑/↓` с сохранением колонки. На мягком
/// переносе не отдаём `end` (иначе курсор «уедет» в начало следующего ряда) —
/// откатываемся на символ назад, оставаясь на этом ряду.
fn col_for_visual(line: &[char], start: usize, end: usize, target_vw: usize, soft: bool) -> usize {
    let mut w = 0;
    let mut col = start;
    while col < end {
        let cw = wrap::width_at(line, col);
        if w + cw > target_vw {
            break;
        }
        w += cw;
        col += 1;
    }
    if soft && col == end && end > start {
        col -= 1;
    }
    col
}

/// Пересекает диапазоны ошибок `[s, e)` логической строки с визуальным рядом
/// `[start, end)` и сдвигает в координаты ряда (для подчёркивания в [`styled_line`]).
fn clip_ranges(ranges: &[(usize, usize)], start: usize, end: usize) -> Vec<(usize, usize)> {
    ranges
        .iter()
        .filter_map(|&(s, e)| {
            let s = s.clamp(start, end);
            let e = e.clamp(start, end);
            (e > s).then_some((s - start, e - start))
        })
        .collect()
}

/// Строит строку, подчёркивая (`UNDERLINED`, цветом ошибки темы) диапазоны ошибок.
/// `ranges` — отсортированные непересекающиеся `[start, end)` в символах.
fn styled_line(
    chars: &[char],
    ranges: Option<&[(usize, usize)]>,
    palette: &Palette,
) -> Line<'static> {
    let ranges = match ranges {
        Some(r) if !r.is_empty() => r,
        _ => return Line::from(chars.iter().collect::<String>()),
    };
    let bad = Style::new().underlined().fg(palette.error);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut pos = 0;
    for &(start, end) in ranges {
        let start = start.min(chars.len());
        let end = end.min(chars.len());
        if start > pos {
            spans.push(Span::raw(chars[pos..start].iter().collect::<String>()));
        }
        if end > start {
            spans.push(Span::styled(
                chars[start..end].iter().collect::<String>(),
                bad,
            ));
        }
        pos = end.max(pos);
    }
    if pos < chars.len() {
        spans.push(Span::raw(chars[pos..].iter().collect::<String>()));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
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
    fn insert_str_multiline_at_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("aXd");
        ib.row = 0;
        ib.col = 1; // курсор между 'a' и 'X'
        ib.insert_str("b\nc");
        // 'a' + вставка("b\nc") + хвост("Xd")
        assert_eq!(ib.text(), "ab\ncXd");
        assert_eq!(ib.line_count(), 2);
        // курсор в конце вставленного, перед хвостом "Xd"
        assert_eq!(ib.cursor(), (1, 1));
    }

    #[test]
    fn insert_str_normalizes_newlines_and_tabs() {
        let mut ib = InputBox::new();
        ib.insert_str("a\r\nb\rc\td");
        assert_eq!(ib.text(), "a\nb\nc    d");
        assert_eq!(ib.line_count(), 3);
    }

    #[test]
    fn insert_str_single_line_keeps_one_row() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab");
        ib.insert_str("XY"); // курсор в конце
        assert_eq!(ib.text(), "abXY");
        assert_eq!(ib.line_count(), 1);
        assert_eq!(ib.cursor(), (0, 4));
    }

    #[test]
    fn insert_str_unicode() {
        let mut ib = InputBox::new();
        ib.insert_str("привет\nмир");
        assert_eq!(ib.text(), "привет\nмир");
        assert_eq!(ib.cursor(), (1, 3));
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
    fn clear_or_restore_clears_then_restores() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "привет\nмир");
        // первое нажатие — удаляет весь текст
        ib.clear_or_restore();
        assert!(ib.is_empty());
        // повторное нажатие на пустом поле — возвращает удалённое
        ib.clear_or_restore();
        assert_eq!(ib.text(), "привет\nмир");
        assert_eq!(ib.cursor(), (1, 3)); // курсор в конце восстановленного
    }

    #[test]
    fn restore_invalidated_after_typing() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "hello");
        ib.clear_or_restore(); // удалили, буфер = "hello"
        assert!(ib.is_empty());
        ib.insert_char('x'); // ввод инвалидирует буфер отмены
        ib.clear_or_restore(); // поле непусто → удаляет "x", не восстанавливает "hello"
        assert!(ib.is_empty());
        ib.clear_or_restore(); // теперь вернётся именно "x"
        assert_eq!(ib.text(), "x");
    }

    #[test]
    fn restore_invalidated_after_paste() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "draft");
        ib.clear_or_restore();
        ib.insert_str("pasted"); // вставка тоже инвалидирует буфер
        ib.clear_or_restore(); // удалит "pasted"
        ib.clear_or_restore(); // вернёт "pasted", а не "draft"
        assert_eq!(ib.text(), "pasted");
    }

    #[test]
    fn restore_survives_cursor_moves_on_empty_field() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "abc");
        ib.clear_or_restore();
        // движения курсора по пустому полю не вводят текст → буфер цел
        ib.on_key(k(KeyCode::Left));
        ib.on_key(k(KeyCode::Home));
        ib.clear_or_restore();
        assert_eq!(ib.text(), "abc");
    }

    #[test]
    fn clear_or_restore_on_empty_without_buffer_is_noop() {
        let mut ib = InputBox::new();
        ib.clear_or_restore(); // нечего удалять и нечего возвращать
        assert!(ib.is_empty());
    }

    #[test]
    fn plain_clear_drops_undo_buffer() {
        // После явного clear() (например, при отправке) восстановить нельзя.
        let mut ib = InputBox::new();
        type_str(&mut ib, "sent");
        ib.clear_or_restore(); // буфер = "sent"
        ib.clear(); // отправка/команда чистит поле и буфер отмены
        ib.clear_or_restore(); // нечего возвращать
        assert!(ib.is_empty());
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
    fn line_strings_and_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("abc\nde");
        assert_eq!(ib.line_strings(), vec!["abc".to_string(), "de".to_string()]);
        assert_eq!(ib.cursor(), (1, 2)); // курсор в конце последней строки
    }

    #[test]
    fn replace_range_swaps_word_and_moves_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("helo world");
        ib.replace_range(0, 0, 4, "hello");
        assert_eq!(ib.text(), "hello world");
        assert_eq!(ib.cursor(), (0, 5));
    }

    #[test]
    fn replace_range_unicode() {
        let mut ib = InputBox::new();
        ib.set_text("превед мир");
        ib.replace_range(0, 0, 6, "привет");
        assert_eq!(ib.text(), "привет мир");
    }

    #[test]
    fn render_with_misspelled_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("helo world\nпревед");
        ib.set_misspelled(vec![vec![(0, 4)], vec![(0, 6)]]);
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), false))
            .unwrap();
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("строка 1\nстрока 2\nстрока 3");
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), false))
            .unwrap();
    }

    #[test]
    fn render_command_mode_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("/rag add d:\\dir -r");
        // даже при наличии «ошибок» в режиме команды подчёркивания не рисуются
        ib.set_misspelled(vec![vec![(0, 4)]]);
        let mut term = Terminal::new(TestBackend::new(24, 3)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), true))
            .unwrap();
    }

    #[test]
    fn content_rows_matches_render_text_width() {
        // `content_rows(area_width)` должен считать перенос по ТОЙ ЖЕ ширине текста,
        // что и `render` (минус рамка 2 и колонка приглашения PROMPT_W), иначе высота
        // поля расходится с реальным переносом (поле не растёт на 1–2 символа за
        // границей). Внешняя ширина 14 → ширина текста = 14 − 2 − PROMPT_W = 10.
        let area_width: u16 = 14;
        let text_w = (area_width - 2 - PROMPT_W) as usize; // 10
        let mut ib = InputBox::new();
        // Слово ровно на один символ длиннее ширины текста → render переносит на 2 ряда.
        let word = "a".repeat(text_w + 1);
        type_str(&mut ib, &word);
        // Рендерим во внешнюю область этой ширины — last_width станет = реальной ширине.
        render_at(&mut ib, area_width - 2 - PROMPT_W);
        let rendered_rows = ib.visual_rows(ib.last_width).len();
        assert_eq!(ib.content_rows(area_width), rendered_rows);
        assert!(
            ib.content_rows(area_width) > 1,
            "поле должно вырасти до двух рядов на символе за границей переноса"
        );
    }

    #[test]
    fn content_rows_single_line_is_one() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("очень длинное значение не помещающееся в узкое поле");
        assert_eq!(ib.content_rows(12), 1);
    }

    #[test]
    fn long_line_counts_as_multiple_visual_rows() {
        let mut ib = InputBox::new();
        // одна логическая строка длиннее ширины → несколько визуальных рядов
        type_str(&mut ib, "один два три четыре");
        assert_eq!(ib.line_count(), 1);
        assert!(ib.visual_line_count(8) > 1);
    }

    #[test]
    fn cursor_moves_and_deletes_by_grapheme_cluster() {
        let mut ib = InputBox::new();
        ib.insert_str("a❤\u{FE0F}👍🏽");
        // a(1) + ❤️(2 скаляра) + 👍🏽(2 скаляра) = 5 символов, курсор в конце
        assert_eq!(ib.cursor(), (0, 5));
        // ← один раз проходит весь кластер 👍🏽 (на 2 скаляра назад)
        ib.move_left();
        assert_eq!(ib.cursor(), (0, 3));
        // ещё один ← проходит весь ❤️ (тоже 2 скаляра), без остановки в середине
        ib.move_left();
        assert_eq!(ib.cursor(), (0, 1));
        ib.move_left();
        assert_eq!(ib.cursor(), (0, 0));
        // Backspace с конца удаляет кластер целиком (не оставляет осиротевший скаляр)
        ib.move_doc_end();
        ib.backspace(); // удаляет 👍🏽 целиком
        assert_eq!(ib.text(), "a❤\u{FE0F}");
        ib.backspace(); // удаляет ❤️ целиком
        assert_eq!(ib.text(), "a");
    }

    #[test]
    fn delete_forward_removes_whole_cluster() {
        let mut ib = InputBox::new();
        ib.insert_str("❤\u{FE0F}👍🏽b");
        ib.move_doc_start();
        ib.delete(); // удаляет ❤️ целиком, не оставляя U+FE0F
        assert_eq!(ib.text(), "👍🏽b");
        ib.delete(); // удаляет 👍🏽 целиком
        assert_eq!(ib.text(), "b");
        ib.delete();
        assert_eq!(ib.text(), "");
        assert!(ib.is_empty());
    }

    #[test]
    fn cursor_visual_accounts_for_emoji_cluster_width() {
        // ❤️ (❤ + U+FE0F) терминал рисует шириной 2 → курсор за кластером в колонке 2,
        // а не 1 (иначе он «садился» в середину эмодзи, см. spec §11.5).
        let mut ib = InputBox::new();
        ib.insert_str("❤\u{FE0F}");
        let vrows = ib.visual_rows(40);
        let (row, col) = ib.cursor_visual(&vrows);
        assert_eq!((row, col), (0, 2));
        // Следующий символ продолжает с колонки 2 — текст после эмодзи не сдвинут.
        ib.insert_char('a');
        let vrows = ib.visual_rows(40);
        assert_eq!(ib.cursor_visual(&vrows), (0, 3));
    }

    #[test]
    fn cursor_maps_onto_wrapped_row() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "один два три"); // курсор в конце (col=12)
        let vrows = ib.visual_rows(8);
        // "один два" | "три" → курсор на втором ряду, столбец 3 ("три")
        let (row, col) = ib.cursor_visual(&vrows);
        assert_eq!((row, col), (1, 3));
    }

    #[test]
    fn cursor_at_soft_break_moves_to_next_row_start() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        // курсор сразу после "один два " (индекс 9) — начало слова "три"
        ib.row = 0;
        ib.col = 9;
        let vrows = ib.visual_rows(8);
        let (row, col) = ib.cursor_visual(&vrows);
        assert_eq!((row, col), (1, 0));
    }

    /// Рендерит поле во внутреннюю ширину `inner_w` (рамка добавляет 2 колонки,
    /// колонка приглашения `❯` — ещё `PROMPT_W`), чтобы выставить `last_width` для
    /// навигации `↑/↓` по визуальным рядам.
    fn render_at(ib: &mut InputBox, inner_w: u16) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(inner_w + 2 + PROMPT_W, 8)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), false))
            .unwrap();
    }

    #[test]
    fn col_for_visual_clamps_off_soft_break() {
        let line: Vec<char> = "abcd".chars().collect();
        // На мягком переносе целевая колонка за концом ряда откатывается на символ
        // назад (иначе курсор уехал бы в начало следующего ряда).
        assert_eq!(col_for_visual(&line, 0, 4, 10, true), 3);
        // На жёстком конце логической строки клампа нет.
        assert_eq!(col_for_visual(&line, 0, 4, 10, false), 4);
        // Колонка внутри ряда — обычный поиск по ширине.
        assert_eq!(col_for_visual(&line, 0, 4, 2, true), 2);
    }

    #[test]
    fn arrow_up_moves_within_wrapped_line() {
        let mut ib = InputBox::new();
        // одна логическая строка, переносится на два ряда: "один два " | "три"
        ib.set_text("один два три"); // курсор в конце (row=0, col=12)
        render_at(&mut ib, 8);
        // ↑ переводит на предыдущий визуальный ряд той же строки (не уходит выше)
        assert!(ib.on_key(k(KeyCode::Up)));
        assert_eq!(ib.cursor(), (0, 3)); // "оди|н два три" — колонка 3 сохранена
        // ещё одно ↑ на верхнем визуальном ряду — без движения
        assert!(ib.on_key(k(KeyCode::Up)));
        assert_eq!(ib.cursor(), (0, 3));
    }

    #[test]
    fn arrow_down_moves_within_wrapped_line() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        render_at(&mut ib, 8);
        ib.row = 0;
        ib.col = 3; // верхний визуальный ряд, колонка 3
        assert!(ib.on_key(k(KeyCode::Down)));
        // на нижний ряд "три" с сохранением колонки → конец строки (3 символа)
        assert_eq!(ib.cursor(), (0, 12));
        // ещё одно ↓ на нижнем визуальном ряду — без движения
        assert!(ib.on_key(k(KeyCode::Down)));
        assert_eq!(ib.cursor(), (0, 12));
    }

    #[test]
    fn arrow_up_down_cross_logical_lines_when_not_wrapped() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef"); // две короткие логические строки, без переноса
        render_at(&mut ib, 20);
        ib.row = 1;
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)));
        assert_eq!(ib.cursor(), (0, 2)); // перешли на предыдущую логическую строку
        assert!(ib.on_key(k(KeyCode::Down)));
        assert_eq!(ib.cursor(), (1, 2));
    }

    #[test]
    fn goal_column_preserved_through_short_row() {
        // Серия ↓ через короткую строку держит исходную колонку (goal-column).
        let mut ib = InputBox::new();
        ib.set_text("abcdef\nx\nabcdef");
        render_at(&mut ib, 20); // широко — без переноса, по логическим строкам
        ib.row = 0;
        ib.col = 5; // колонка 5 на первой строке
        ib.goal_col = None; // прямое присвоение col выше не сбрасывает goal
        assert!(ib.on_key(k(KeyCode::Down)));
        assert_eq!(ib.cursor(), (1, 1)); // "x" короче — курсор прижат к концу
        assert!(ib.on_key(k(KeyCode::Down)));
        assert_eq!(ib.cursor(), (2, 5)); // колонка 5 восстановлена, не осталась 1
    }

    #[test]
    fn horizontal_move_resets_goal_column() {
        let mut ib = InputBox::new();
        ib.set_text("abcdef\nx\nabcdef");
        render_at(&mut ib, 20);
        ib.row = 0;
        ib.col = 5;
        ib.goal_col = None;
        assert!(ib.on_key(k(KeyCode::Down))); // (1,1), goal=5
        assert!(ib.on_key(k(KeyCode::Left))); // горизонтальное движение сбрасывает goal
        assert!(ib.on_key(k(KeyCode::Down)));
        // без goal колонка берётся из текущей (0) → начало третьей строки
        assert_eq!(ib.cursor(), (2, 0));
    }

    #[test]
    fn home_end_act_on_visual_row() {
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // ширина 8: "один два " | "три"
        render_at(&mut ib, 8);
        // курсор в середине нижнего визуального ряда "три"
        ib.row = 0;
        ib.col = 10;
        assert!(ib.on_key(k(KeyCode::Home)));
        assert_eq!(ib.cursor(), (0, 9)); // начало ряда "три", а не всей строки
        assert!(ib.on_key(k(KeyCode::End)));
        assert_eq!(ib.cursor(), (0, 12)); // конец ряда "три" = конец строки
        // на верхнем ряду End встаёт на последнюю позицию ряда (мягкий перенос)
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::End)));
        assert_eq!(ib.cursor(), (0, 8)); // конец "один два", не уезжает в начало "три"
        assert!(ib.on_key(k(KeyCode::Home)));
        assert_eq!(ib.cursor(), (0, 0)); // начало верхнего ряда
    }

    #[test]
    fn arrow_up_falls_back_to_logical_before_render() {
        // до первого рендера ширина неизвестна (last_width == 0) → логический переход
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef");
        ib.row = 1;
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)));
        assert_eq!(ib.cursor(), (0, 2));
    }

    #[test]
    fn single_line_disables_newline_and_collapses_paste() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("ab\ncd"); // перевод строки схлопывается в пробел
        assert_eq!(ib.text(), "ab cd");
        assert_eq!(ib.line_count(), 1);
        ib.insert_newline(); // no-op
        assert_eq!(ib.line_count(), 1);
        ib.insert_str("x\ny"); // вставка тоже одной строкой
        assert_eq!(ib.line_count(), 1);
        assert!(ib.text().contains("x y"));
    }

    #[test]
    fn single_line_arrows_up_down_are_noop() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("hello");
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)));
        assert_eq!(ib.cursor(), (0, 2));
        assert!(ib.on_key(k(KeyCode::Down)));
        assert_eq!(ib.cursor(), (0, 2));
    }

    #[test]
    fn single_line_home_end_span_whole_value() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("a long value");
        render_at(&mut ib, 4); // узкое поле — значение длиннее ширины
        ib.col = 5;
        assert!(ib.on_key(k(KeyCode::Home)));
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.on_key(k(KeyCode::End)));
        assert_eq!(ib.cursor(), (0, 12)); // конец всего значения, не визуального ряда
    }

    #[test]
    fn single_line_renders_long_value_without_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("/very/long/path/to/a/gguf/model/that/does/not/fit.gguf");
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), false))
            .unwrap();
    }

    #[test]
    fn col_at_width_lands_on_char_boundary() {
        let line: Vec<char> = "abcdef".chars().collect();
        assert_eq!(col_at_width(&line, 0), 0);
        assert_eq!(col_at_width(&line, 3), 3);
        assert_eq!(col_at_width(&line, 100), 6); // за концом — вся строка
    }

    #[test]
    fn ctrl_left_right_move_by_word() {
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // курсор в конце (col=12)
        // Ctrl+← → начало слова "три"
        assert!(ib.on_key(ctrl(KeyCode::Left)));
        assert_eq!(ib.cursor(), (0, 9));
        // ещё раз → начало "два"
        assert!(ib.on_key(ctrl(KeyCode::Left)));
        assert_eq!(ib.cursor(), (0, 5));
        // ещё раз → начало "один"
        assert!(ib.on_key(ctrl(KeyCode::Left)));
        assert_eq!(ib.cursor(), (0, 0));
        // Ctrl+→ → за концом "один"
        assert!(ib.on_key(ctrl(KeyCode::Right)));
        assert_eq!(ib.cursor(), (0, 4));
        // ещё раз → за концом "два"
        assert!(ib.on_key(ctrl(KeyCode::Right)));
        assert_eq!(ib.cursor(), (0, 8));
    }

    #[test]
    fn ctrl_left_right_cross_logical_lines() {
        let mut ib = InputBox::new();
        ib.set_text("ab\ncd");
        ib.row = 1;
        ib.col = 0; // начало второй строки
        // Ctrl+← на границе строки → конец предыдущей
        assert!(ib.on_key(ctrl(KeyCode::Left)));
        assert_eq!(ib.cursor(), (0, 2));
        // Ctrl+→ из конца первой строки → начало следующей
        assert!(ib.on_key(ctrl(KeyCode::Right)));
        assert_eq!(ib.cursor(), (1, 0));
    }

    #[test]
    fn ctrl_backspace_deletes_word_left() {
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // курсор в конце
        assert!(ib.on_key(ctrl(KeyCode::Backspace)));
        assert_eq!(ib.text(), "один два ");
        assert_eq!(ib.cursor(), (0, 9));
        // в начале строки склеивает со строкой выше (как обычный Backspace)
        ib.set_text("ab\ncd");
        ib.row = 1;
        ib.col = 0;
        assert!(ib.on_key(ctrl(KeyCode::Backspace)));
        assert_eq!(ib.text(), "abcd");
    }

    #[test]
    fn ctrl_delete_deletes_word_right() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        ib.col = 0;
        assert!(ib.on_key(ctrl(KeyCode::Delete)));
        assert_eq!(ib.text(), " два три"); // удалено слово "один", пробел остался
        assert_eq!(ib.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_home_end_jump_to_document_bounds() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef\nghi");
        ib.row = 1;
        ib.col = 1;
        assert!(ib.on_key(ctrl(KeyCode::Home)));
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.on_key(ctrl(KeyCode::End)));
        assert_eq!(ib.cursor(), (2, 3));
    }

    #[test]
    fn ctrl_char_is_not_inserted() {
        // Ctrl+символ — шорткат вышестоящего слоя, в поле не печатается.
        let mut ib = InputBox::new();
        assert!(!ib.on_key(ctrl(KeyCode::Char('a'))));
        assert!(ib.is_empty());
    }

    #[test]
    fn render_wrapped_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("очень длинная строка которая точно не влезает в узкое поле ввода");
        ib.set_misspelled(vec![vec![(0, 5)]]);
        let mut term = Terminal::new(TestBackend::new(12, 4)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), false))
            .unwrap();
    }

    #[test]
    fn scrollbar_appears_only_when_input_scrolls() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // Бегунок «█» на правой рамке — только когда рядов больше видимой высоты.
        let right_col = |term: &Terminal<TestBackend>| -> Vec<String> {
            let buf = term.backend().buffer();
            let area = buf.area;
            (area.top()..area.bottom())
                .map(|y| buf[(area.right() - 1, y)].symbol().to_string())
                .collect()
        };
        let mut ib = InputBox::new();
        ib.set_text("a\nb"); // 2 ряда во внутренней высоте 2 — помещается
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), false))
            .unwrap();
        assert!(
            !right_col(&term).iter().any(|s| s == "█"),
            "помещающийся текст — без бегунка"
        );
        ib.set_text("1\n2\n3\n4\n5\n6"); // 6 рядов, видно 2 — прокрутка
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &Palette::default(), false))
            .unwrap();
        assert!(
            right_col(&term).iter().any(|s| s == "█"),
            "прокручиваемое поле — с бегунком"
        );
    }
}
