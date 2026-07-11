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
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap;

/// Ширина колонки приглашения `❯ ` (в колонках) перед текстом ввода.
const PROMPT_W: u16 = 2;

/// Один визуальный ряд: `(логическая строка, начало, конец)` в индексах символов
/// строки (с учётом переноса по ширине). См. [`InputBox::visual_rows`].
type VisualRow = (usize, usize, usize);

/// Кэш визуальных рядов: `(ширина, ревизия содержимого, ряды)`. См.
/// [`InputBox::rows_cache`].
type RowCache = Option<(usize, u64, Vec<VisualRow>)>;

/// Потолок глубины стека отмены (единиц). Самые старые вытесняются.
const UNDO_CAP: usize = 200;

/// Снимок содержимого для отмены/повтора: строки + позиция курсора. См.
/// [`InputBox::record_undo`], docs/input-selection-undo-mouse.md §C.
#[derive(Clone)]
struct Snapshot {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

/// Класс правки для коалесинга отмены: подряд идущие правки одного класса (кроме
/// `Structural`) сливаются в одну единицу отмены; смена класса, навигация или
/// структурная правка начинают новую. См. [`InputBox::record_undo`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// Набор символов (коалесится; разрыв на пробеле — word-granular отмена).
    Insert,
    /// Удаление (`Backspace`/`Delete`/слово) — коалесится.
    Delete,
    /// Структурная правка (перевод строки, вставка, замена диапазона/выделения,
    /// очистка `Ctrl+K`) — всегда отдельная единица отмены.
    Structural,
}

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
    /// Стек отмены: снимки содержимого **до** правки (с коалесингом по классу правки,
    /// см. [`Self::record_undo`]). `Ctrl+Z` восстанавливает верхний. Потолок [`UNDO_CAP`].
    undo: Vec<Snapshot>,
    /// Стек повтора: снимки, снятые с `undo` при отмене; чистится любой новой правкой.
    /// `Ctrl+Y` восстанавливает верхний.
    redo: Vec<Snapshot>,
    /// Класс последней правки — для коалесинга единиц отмены. Сбрасывается навигацией/
    /// выделением/отменой (тогда следующая правка начинает новую единицу). См.
    /// [`Self::record_undo`].
    last_edit_kind: Option<EditKind>,
    /// Счётчик ревизии содержимого: инкрементируется при любой правке `lines`
    /// ([`Self::touch`]). Вместе с шириной — ключ кэша визуальных рядов: если ревизия
    /// и ширина не изменились, перенос не пересчитывается.
    revision: u64,
    /// Кэш визуальных рядов `(ширина, ревизия, ряды)`. Перенос (`visual_rows`, O(n)
    /// по символам) за кадр нужен 2–3 раза (высота через [`Self::content_rows`], сам
    /// [`Self::render`], навигация `↑/↓`), а меняется лишь при правке или смене ширины.
    /// Инвалидируется по `(width, revision)`. См. [`Self::rows_cached`].
    rows_cache: RowCache,
    /// Якорь выделения `(строка, столбец)` в индексах символов. `Some` — есть активное
    /// выделение `[anchor, cursor]` (нормализуется при использовании: начало = меньшая
    /// из позиций в лексикографическом порядке `(строка, столбец)`). `None` — выделения
    /// нет; курсор — существующие `(row, col)`. Движение с `Shift` ставит якорь и
    /// растит выделение, обычное движение — снимает; любая правка содержимого снимает
    /// (через [`Self::touch`]). См. [`Self::selection_span`], docs/input-selection-undo-mouse.md.
    anchor: Option<(usize, usize)>,
    /// Внутренняя область текста последней отрисовки (после рамки и колонки `❯`).
    /// Нужна маппингу клика мыши (экранные координаты → позиция в тексте): клик/драг
    /// левой кнопкой ставит курсор / растит выделение (при захвате мыши `Ctrl+W`).
    /// `None` до первого рендера. См. [`Self::place_cursor_at`], этап D плана.
    last_area: Option<Rect>,
}

impl Default for InputBox {
    fn default() -> Self {
        Self::new()
    }
}

/// Итог обработки клавиши полем ввода ([`InputBox::on_key`]): различает **правку
/// содержимого** и одно лишь **движение курсора/скролла**. Вызывающий по нему решает,
/// помечать ли ввод «грязным» (сохранение черновика + перепроверка орфографии).
/// Раньше `on_key` возвращал `bool` («обработана/нет»), и любое движение курсора зря
/// поднимало дебаунс орфографии и слало `SetDraft` на диск. См. spec §11.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Содержимое изменено.
    Edited,
    /// Курсор/скролл сдвинут, содержимое не менялось.
    Moved,
    /// Клавиша не обработана (вызывающий трактует её дальше).
    Ignored,
}

impl KeyOutcome {
    /// Клавиша обработана виджетом (вызывающий не трактует её дальше). Пока нужен
    /// только тестам — продакшн-вызовы ветвятся по [`Self::edited`]; оставлен как
    /// парный к нему элемент публичного API виджета.
    #[allow(dead_code)]
    pub fn handled(self) -> bool {
        !matches!(self, KeyOutcome::Ignored)
    }

    /// Правка изменила содержимое (нужны пометка «грязного» ввода / перепроверка
    /// орфографии); движение курсора и необработанная клавиша — нет.
    pub fn edited(self) -> bool {
        matches!(self, KeyOutcome::Edited)
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
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit_kind: None,
            revision: 0,
            rows_cache: None,
            anchor: None,
            last_area: None,
        }
    }

    /// Включает однострочный режим (горизонтальный скролл вместо переноса; `↑/↓` и
    /// перевод строки отключены). Обычно вызывается **до** [`Self::set_text`], но
    /// инвариант «одна логическая строка» держится и при включении на уже
    /// многострочном содержимом: строки схлопываются в одну через пробел, курсор
    /// клампится. Иначе [`Self::render_single_line`] взял бы `lines[0]`, а `col` мог
    /// указывать за её длину — паника на срезе. См. поле [`Self::single_line`].
    pub fn set_single_line(&mut self, on: bool) {
        self.single_line = on;
        if on && self.lines.len() > 1 {
            let mut merged: Vec<char> = Vec::new();
            for (idx, line) in self.lines.iter().enumerate() {
                if idx > 0 {
                    merged.push(' ');
                }
                merged.extend(line.iter().copied());
            }
            self.col = self.col.min(merged.len());
            self.lines = vec![merged];
            self.row = 0;
            self.scroll = 0;
            self.hscroll = 0;
            self.goal_col = None;
            self.touch();
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

    /// Первый непробельный символ значения (по логическим строкам), если есть. Дешёвая
    /// проверка «похоже на команду» без аллокации всего текста: вызывающий слой сперва
    /// смотрит на `Some('/')` и лишь тогда парсит полный [`Self::text`]. Останавливается
    /// на первом непробельном символе. См. `ChatScreen::input_is_command`.
    pub fn first_non_whitespace(&self) -> Option<char> {
        self.lines
            .iter()
            .flat_map(|l| l.iter())
            .copied()
            .find(|c| !c.is_whitespace())
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
    pub fn content_rows(&mut self, area_width: u16) -> usize {
        if self.single_line {
            return 1;
        }
        let text_w = area_width.saturating_sub(2).saturating_sub(PROMPT_W).max(1) as usize;
        self.rows_cached(text_w).len()
    }

    /// Очищает поле **программно** (отправка сообщения, сброс). В отличие от
    /// [`Self::clear_undoable`] — **чистит историю отмены** (после отправки/загрузки
    /// чужого текста `Ctrl+Z` не должен воскрешать прежний контекст).
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.misspelled.clear();
        self.undo.clear();
        self.redo.clear();
        self.last_edit_kind = None;
        self.touch();
    }

    /// Хоткей «удалить весь текст ввода» (`Ctrl+K`, spec §11.5). Записывает снимок в
    /// историю отмены и очищает содержимое, **не** трогая историю — так `Ctrl+Z`
    /// возвращает текст (общая модель отмены; прежняя toggle-семантика удалена, см.
    /// docs/input-selection-undo-mouse.md §C). Пустое поле — no-op.
    pub fn clear_undoable(&mut self) {
        if self.is_empty() {
            return;
        }
        self.record_undo(EditKind::Structural); // снимок непустого содержимого
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.misspelled.clear();
        self.touch(); // ревизия + снятие выделения (историю НЕ чистим)
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

    /// Синхронизирует диапазоны ошибок строки `row` с правкой её содержимого: в
    /// позиции `at` удалено `removed` и вставлено `inserted` символов. Диапазоны
    /// целиком слева от правки не трогаются; целиком справа — сдвигаются на дельту;
    /// пересекающие изменённый участок — сбрасываются (слово изменилось — пусть
    /// перепроверка его переоценит). Так подчёркивания держатся на месте между
    /// дебаунс-перепроверками, не «съезжая» на соседние слова (иначе стилем ошибки
    /// подсвечивался бы уже другой текст). См. spec §11.5.
    fn edit_misspelled(&mut self, row: usize, at: usize, removed: usize, inserted: usize) {
        let Some(ranges) = self.misspelled.get_mut(row) else {
            return;
        };
        let end = at + removed;
        let delta = inserted as isize - removed as isize;
        ranges.retain_mut(|(s, e)| {
            if *e <= at {
                true // целиком слева — без изменений
            } else if *s >= end {
                *s = (*s as isize + delta).max(0) as usize; // целиком справа — сдвиг
                *e = (*e as isize + delta).max(0) as usize;
                *e > *s
            } else {
                false // пересекает правку — сбросить
            }
        });
    }

    /// Нет ли отмеченных ошибок орфографии (для тестов вышестоящего слоя).
    #[cfg(test)]
    pub fn misspelled_is_empty(&self) -> bool {
        self.misspelled.iter().all(|r| r.is_empty())
    }

    /// Диапазоны ошибок орфографии строки `row` (для тестов синхронизации правок).
    #[cfg(test)]
    fn misspelled_ranges_for_test(&self, row: usize) -> Vec<(usize, usize)> {
        self.misspelled.get(row).cloned().unwrap_or_default()
    }

    /// Текущий горизонтальный скролл (в колонках) — для теста выравнивания по границе
    /// символа в однострочном режиме.
    #[cfg(test)]
    fn hscroll_for_test(&self) -> usize {
        self.hscroll
    }

    /// Заменяет диапазон символов `[start, end)` в строке `row` на `replacement`
    /// и ставит курсор за вставленным текстом (для применения подсказки).
    pub fn replace_range(&mut self, row: usize, start: usize, end: usize, replacement: &str) {
        if row >= self.lines.len() {
            return;
        }
        self.record_undo(EditKind::Structural); // до мутации (замена — отдельная единица)
        let line = &mut self.lines[row];
        let end = end.min(line.len());
        let start = start.min(end);
        let repl: Vec<char> = replacement.chars().collect();
        let repl_len = repl.len();
        line.splice(start..end, repl);
        self.edit_misspelled(row, start, end - start, repl_len);
        self.row = row;
        self.col = start + repl_len;
        self.goal_col = None;
        self.touch();
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
        // Программная замена текста (загрузка черновика чужого чата, restore_input) —
        // чистим историю отмены: `Ctrl+Z` не должен воскрешать чужой контекст.
        self.undo.clear();
        self.redo.clear();
        self.last_edit_kind = None;
        // Старые диапазоны ошибок относились к прежнему тексту — сбрасываем (иначе до
        // ближайшей перепроверки подчёркивания рисовались бы на новом содержимом).
        self.misspelled.clear();
        self.touch();
    }

    // ---------- редактирование ----------

    pub fn insert_char(&mut self, c: char) {
        self.record_undo(EditKind::Insert);
        self.remove_selection(); // ввод поверх выделения заменяет его (undo уже записан)
        self.lines[self.row].insert(self.col, c);
        self.edit_misspelled(self.row, self.col, 0, 1);
        self.col += 1;
        self.goal_col = None;
        self.touch();
        // Пробел завершает единицу отмены (word-granular): следующий набор — новая единица.
        if c.is_whitespace() {
            self.last_edit_kind = None;
        }
    }

    pub fn insert_newline(&mut self) {
        if self.single_line {
            return; // в однострочном режиме перевод строки запрещён
        }
        self.record_undo(EditKind::Structural);
        self.remove_selection(); // перевод строки поверх выделения заменяет его
        let tail = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, tail);
        // Синхронизируем подчёркивания: текущую строку сбрасываем (её хвост уехал на
        // новую), для новой строки вставляем пустой набор диапазонов.
        if self.row < self.misspelled.len() {
            self.misspelled[self.row].clear();
            self.misspelled.insert(self.row + 1, Vec::new());
        }
        self.row += 1;
        self.col = 0;
        self.goal_col = None;
        self.touch();
    }

    /// Вставляет произвольный текст в позицию курсора (вставка из буфера обмена).
    /// Переводы строк (`\n`) разбивают текущую логическую строку на новые; `\r`
    /// нормализуются (`\r\n`/`\r` → `\n`), `\t` разворачивается в пробелы. Курсор
    /// встаёт в конец вставленного. Один проход без посимвольной петли — поэтому
    /// большая вставка не тормозит (см. bracketed paste, spec §11.5).
    pub fn insert_str(&mut self, text: &str) {
        self.record_undo(EditKind::Structural);
        self.remove_selection(); // вставка поверх выделения заменяет его
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
        // Вставка (буфер обмена) меняет строки произвольно; проще сбросить все
        // подчёркивания — перепроверка (её всегда запускает `mark_input_changed`
        // после вставки) их перестроит. См. spec §11.5.
        self.misspelled.clear();
        self.touch();
    }

    pub fn backspace(&mut self) {
        // Нечего удалять (пустой префикс без выделения) — не пишем единицу отмены.
        if !self.has_selection() && self.col == 0 && self.row == 0 {
            return;
        }
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // при выделении Backspace удаляет его целиком
        }
        self.goal_col = None;
        if self.col > 0 {
            // Удаляем кластер целиком (`❤️`/`👍🏽` — несколько скаляров), а не один
            // скаляр — иначе остаётся осиротевший вариатор/модификатор. См. spec §11.5.
            let start = wrap::prev_boundary(&self.lines[self.row], self.col);
            self.lines[self.row].drain(start..self.col);
            self.edit_misspelled(self.row, start, self.col - start, 0);
            self.col = start;
            self.touch();
        } else if self.row > 0 {
            // склейка с предыдущей строкой
            let current = self.lines.remove(self.row);
            self.join_misspelled_into_prev(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(current);
            self.touch();
        }
    }

    pub fn delete(&mut self) {
        // Нечего удалять (курсор в самом конце без выделения) — не пишем единицу отмены.
        if !self.has_selection()
            && self.col >= self.lines[self.row].len()
            && self.row + 1 >= self.lines.len()
        {
            return;
        }
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // при выделении Delete удаляет его целиком
        }
        self.goal_col = None;
        if self.col < self.lines[self.row].len() {
            // Удаляем кластер целиком (зеркально `backspace`), а не один скаляр.
            let end = wrap::next_boundary(&self.lines[self.row], self.col);
            self.lines[self.row].drain(self.col..end);
            self.edit_misspelled(self.row, self.col, end - self.col, 0);
            self.touch();
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.join_misspelled_into_prev(self.row + 1);
            self.lines[self.row].extend(next);
            self.touch();
        }
    }

    /// Синхронизирует подчёркивания при склейке строк: строка `removed` уходит в
    /// предыдущую. Диапазоны обеих строк относятся к прежним координатам, поэтому
    /// проще сбросить их у остающейся строки (её содержимое меняется) и убрать запись
    /// уехавшей — перепроверка перестроит. `removed` — индекс строки, которую убрали
    /// из [`Self::lines`]; предыдущая строка — `removed - 1`.
    fn join_misspelled_into_prev(&mut self, removed: usize) {
        if removed < self.misspelled.len() {
            self.misspelled.remove(removed);
        }
        if removed > 0
            && let Some(r) = self.misspelled.get_mut(removed - 1)
        {
            r.clear();
        }
    }

    // ---------- выделение ----------

    /// Есть ли непустое выделение (якорь стоит и не совпал с курсором). Нужен
    /// консьюмерам для копирования/вырезания (docs/input-selection-undo-mouse.md §B).
    pub fn has_selection(&self) -> bool {
        matches!(self.anchor, Some(a) if a != (self.row, self.col))
    }

    /// Нормализованный диапазон выделения `(начало, конец)` в лексикографическом
    /// порядке `(строка, столбец)`. `None`, если выделения нет (якорь снят или совпал
    /// с курсором).
    fn selection_span(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let c = (self.row, self.col);
        if a == c {
            return None;
        }
        Some(if a <= c { (a, c) } else { (c, a) })
    }

    /// Текст выделения (строки через `\n`), либо `None`. Для копирования/вырезания
    /// (консьюмеры, docs/input-selection-undo-mouse.md §B).
    pub fn selected_text(&self) -> Option<String> {
        let ((sr, sc), (er, ec)) = self.selection_span()?;
        let mut out = String::new();
        if sr == er {
            out.extend(self.lines[sr][sc..ec].iter().copied());
        } else {
            out.extend(self.lines[sr][sc..].iter().copied());
            out.push('\n');
            for line in &self.lines[sr + 1..er] {
                out.extend(line.iter().copied());
                out.push('\n');
            }
            out.extend(self.lines[er][..ec].iter().copied());
        }
        Some(out)
    }

    /// Ставит якорь в текущий курсор, если его ещё нет (перед `Shift`-навигацией —
    /// начало выделения). Уже поставленный якорь не сдвигает (выделение растёт от него).
    fn set_anchor_if_none(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some((self.row, self.col));
        }
    }

    /// Снимает выделение (обычная навигация без `Shift`; консьюмеры — после
    /// копирования по `Ctrl+C`).
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Выделяет весь текст (`Ctrl+A`): якорь — начало, курсор — конец. Сбрасывает
    /// коалесинг отмены (правка после выделения — новая единица).
    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
        self.goal_col = None;
        self.last_edit_kind = None;
    }

    /// Удаляет выделение как **самостоятельное** действие (`Ctrl+X` вырезать): пишет
    /// снимок в историю отмены, затем удаляет. Возвращает, было ли что удалять.
    /// Внутренние мутаторы (`insert_char`/`backspace`/…) удаляют выделение через
    /// [`Self::remove_selection`] (без записи — они уже записали свой снимок).
    pub fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        self.record_undo(EditKind::Structural);
        self.remove_selection()
    }

    /// Удаляет выделенный текст, ставит курсор в его начало, снимает выделение.
    /// Возвращает, было ли что удалять. Многострочное выделение склеивает строки;
    /// подчёркивания орфографии синхронизируются (как при удалении/склейке, п.5).
    /// **Не пишет** снимок отмены — это делает вызывающий (см. [`Self::delete_selection`]).
    fn remove_selection(&mut self) -> bool {
        let Some(((sr, sc), (er, ec))) = self.selection_span() else {
            return false;
        };
        // Синхронизация `misspelled`: одна строка — сдвиг/сброс диапазонов; несколько —
        // убрать записи промежуточных/последней строк и сбросить запись первой.
        if sr == er {
            self.edit_misspelled(sr, sc, ec - sc, 0);
        } else {
            for _ in sr..er {
                if sr + 1 < self.misspelled.len() {
                    self.misspelled.remove(sr + 1);
                }
            }
            if let Some(m) = self.misspelled.get_mut(sr) {
                m.clear();
            }
        }
        // Склейка: `lines[sr][..sc]` + `lines[er][ec..]`, промежуточные строки убрать.
        let tail: Vec<char> = self.lines[er][ec..].to_vec();
        self.lines[sr].truncate(sc);
        self.lines[sr].extend(tail);
        self.lines.drain((sr + 1)..=er);
        self.row = sr;
        self.col = sc;
        self.goal_col = None;
        self.touch(); // снимает anchor + инвалидирует кэш
        true
    }

    // ---------- мышь ----------

    /// Ставит курсор по экранным координатам клика мыши `(mx, my)` (при захвате мыши
    /// `Ctrl+W`). Возвращает, попал ли клик во внутреннюю область текста последней
    /// отрисовки ([`Self::last_area`]). Курсор снапится к границе графемного кластера
    /// (клик по широкому/эмодзи-глифу не садит его в середину, п.1); клик ниже
    /// последнего ряда → конец текста, правее конца ряда → конец ряда. До первого
    /// рендера (`last_area == None`) или клик вне области — no-op (`false`). Выделения
    /// не трогает — им управляют [`Self::mouse_press`]/[`Self::mouse_drag`]. Сбрасывает
    /// коалесинг отмены (как навигация). См. этап D плана.
    fn place_cursor_at(&mut self, mx: u16, my: u16) -> bool {
        let Some(area) = self.last_area else {
            return false;
        };
        if mx < area.x || mx >= area.x + area.width || my < area.y || my >= area.y + area.height {
            return false;
        }
        self.goal_col = None;
        self.last_edit_kind = None; // клик рвёт коалесинг отмены (как навигация)
        let vcol = (mx - area.x) as usize;
        if self.single_line {
            // Однострочный: колонка от левого края + горизонтальный скролл, снап к границе.
            let line = &self.lines[0];
            let col = col_at_width(line, self.hscroll + vcol).min(line.len());
            self.col = wrap::snap_boundary(line, col);
            return true;
        }
        let vrow = (my - area.y) as usize + self.scroll;
        let vrows = self.rows_cached(self.last_width).to_vec();
        if vrow >= vrows.len() {
            // Ниже последнего ряда → конец текста.
            self.row = self.lines.len() - 1;
            self.col = self.lines[self.row].len();
            return true;
        }
        let (li, start, end) = vrows[vrow];
        // Внутри ряда: логический столбец по накопленной ширине; правее конца ряда
        // `col_for_visual` даёт конец ряда (с откатом на мягком переносе) + снап.
        self.col = col_for_visual(&self.lines[li], start, end, vcol, is_soft(&vrows, vrow));
        self.row = li;
        true
    }

    /// Ставит курсор по нажатию левой кнопки мыши и **начинает** выделение от этой
    /// точки (пустое — курсор перемещён, видимого выделения ещё нет; драг растит его).
    /// Возвращает, попал ли клик в область текста. См. [`Self::place_cursor_at`].
    pub fn mouse_press(&mut self, mx: u16, my: u16) -> bool {
        if self.place_cursor_at(mx, my) {
            self.anchor = Some((self.row, self.col));
            true
        } else {
            false
        }
    }

    /// Двигает курсор по драгу мыши, **сохраняя** якорь — выделение растёт от точки
    /// нажатия до текущей. Возвращает, попал ли драг в область текста.
    pub fn mouse_drag(&mut self, mx: u16, my: u16) -> bool {
        self.place_cursor_at(mx, my)
    }

    /// Внутренняя область текста последней отрисовки (для тестов проводки мыши —
    /// вычислить экранные координаты клика по полю).
    #[cfg(test)]
    pub(crate) fn last_area_for_test(&self) -> Option<Rect> {
        self.last_area
    }

    // ---------- отмена / повтор ----------

    /// Снимок текущего содержимого (строки + курсор) для истории отмены.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        }
    }

    /// Запоминает снимок **до** правки для отмены (вызывается в начале мутатора).
    /// Коалесинг: подряд идущие правки одного класса (кроме `Structural`) сливаются в
    /// одну единицу — снимок толкается лишь при смене класса / после навигации/выделения
    /// (там `last_edit_kind` сброшен) / для `Structural`. Любая правка чистит стек
    /// повтора. Потолок [`UNDO_CAP`] — старейшие вытесняются. См. §C плана.
    fn record_undo(&mut self, kind: EditKind) {
        let coalesce = self.last_edit_kind == Some(kind) && kind != EditKind::Structural;
        if !coalesce {
            self.undo.push(self.snapshot());
            if self.undo.len() > UNDO_CAP {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_edit_kind = Some(kind);
    }

    /// Восстанавливает содержимое из снимка (общее для отмены/повтора): строки+курсор,
    /// снятие выделения/подсветки, инвалидация кэша. Историю отмены не трогает.
    fn restore(&mut self, snap: Snapshot) {
        self.lines = snap.lines;
        self.row = snap.row;
        self.col = snap.col;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.misspelled.clear();
        self.last_edit_kind = None;
        self.touch(); // ревизия + снятие anchor
    }

    /// Отменяет последнюю единицу правки (`Ctrl+Z`). Возвращает, была ли отмена.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(prev);
        true
    }

    /// Повторяет отменённую правку (`Ctrl+Y`). Возвращает, был ли повтор.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    /// Диапазон выделения на визуальном ряду `[start, end)` логической строки `li` в
    /// **row-local** координатах (для подсветки фона в рендере), либо `None`.
    fn row_selection(&self, li: usize, start: usize, end: usize) -> Option<(usize, usize)> {
        let ((sr, sc), (er, ec)) = self.selection_span()?;
        if li < sr || li > er {
            return None;
        }
        let s = if li == sr { sc.max(start) } else { start };
        let e = if li == er { ec.min(end) } else { end };
        (s < e).then(|| (s - start, e - start))
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
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // при выделении Ctrl+Backspace удаляет его целиком
        }
        self.goal_col = None;
        if self.col == 0 {
            self.backspace(); // record_undo(Delete) коалесится — доп. снимка нет
            return;
        }
        let start = self.word_left_col();
        self.lines[self.row].drain(start..self.col);
        self.edit_misspelled(self.row, start, self.col - start, 0);
        self.col = start;
        self.touch();
    }

    /// Удаляет слово справа от курсора (`Ctrl+Delete`). В конце строки склеивает со
    /// строкой ниже (как обычный `Delete`).
    fn delete_word_right(&mut self) {
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // при выделении Ctrl+Delete удаляет его целиком
        }
        self.goal_col = None;
        if self.col >= self.lines[self.row].len() {
            self.delete(); // record_undo(Delete) коалесится — доп. снимка нет
            return;
        }
        let end = self.word_right_col();
        self.lines[self.row].drain(self.col..end);
        self.edit_misspelled(self.row, self.col, end - self.col, 0);
        self.touch();
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
        let vrows = self.rows_cached(self.last_width).to_vec();
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
        let vrows = self.rows_cached(self.last_width).to_vec();
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
        let vrows = self.rows_cached(self.last_width).to_vec();
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
        let vrows = self.rows_cached(self.last_width).to_vec();
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

    /// Обрабатывает клавишу редактирования. Возвращает [`KeyOutcome`]: `Edited`
    /// (содержимое изменено), `Moved` (сдвинут курсор/выделение) или `Ignored` (клавиша
    /// не обработана — вызывающий трактует её дальше). `Enter` НЕ обрабатывается
    /// (политику отправки/переноса задаёт вызывающий слой). Различие `Edited`/`Moved`
    /// нужно вызывающему, чтобы не помечать ввод «грязным» на голой навигации.
    ///
    /// **Выделение** (etape A, docs/input-selection-undo-mouse.md): `Shift`+навигация
    /// растит выделение (ставит якорь), обычная навигация — снимает; `Ctrl+A` выделяет
    /// всё; правка при активном выделении сперва удаляет его (ввод/`Backspace`/`Delete`
    /// поверх выделения заменяют/удаляют его целиком). Копирование/вырезание —
    /// side-effect вызывающего слоя (§B плана), виджет отдаёт [`Self::selected_text`].
    pub fn on_key(&mut self, key: KeyEvent) -> KeyOutcome {
        if key.kind != KeyEventKind::Press {
            return KeyOutcome::Ignored;
        }
        // Ctrl усиливает навигацию/удаление до уровня слова / всего текста
        // (`Ctrl+←/→` — по словам, `Ctrl+Backspace/Delete` — удалить слово,
        // `Ctrl+Home/End` — в начало/конец текста). См. spec §11.5.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Ctrl-шорткаты редактора: выделить всё / отмена / повтор / очистка
        // (раскладко-независимо через `physical_char`). Отмена/повтор возвращают
        // `Edited`, только если реально что-то изменили (иначе `Moved` — no-op).
        if ctrl && let KeyCode::Char(c) = key.code {
            match keys::physical_char(c) {
                'a' => {
                    self.select_all();
                    return KeyOutcome::Moved;
                }
                'z' => {
                    return if self.undo() {
                        KeyOutcome::Edited
                    } else {
                        KeyOutcome::Moved
                    };
                }
                'y' => {
                    return if self.redo() {
                        KeyOutcome::Edited
                    } else {
                        KeyOutcome::Moved
                    };
                }
                'k' => {
                    self.clear_undoable();
                    return KeyOutcome::Edited;
                }
                _ => {}
            }
        }

        // Навигация (в т.ч. `Ctrl`+слово / `Ctrl`+начало/конец): с `Shift` растим
        // выделение (ставим якорь до движения), без — снимаем. Любая навигация/
        // выделение завершает коалесинг отмены (правка после — новая единица).
        if let Some(mv) = navigation(key.code, ctrl) {
            if shift {
                self.set_anchor_if_none();
            } else {
                self.clear_selection();
            }
            self.last_edit_kind = None;
            mv(self);
            return KeyOutcome::Moved;
        }

        // Правка: замену/удаление выделения делают сами мутаторы (в начале —
        // `delete_selection`), поэтому вставка/`Shift+Enter`/эмодзи тоже её уважают.
        match key.code {
            KeyCode::Backspace if ctrl => {
                self.delete_word_left();
                KeyOutcome::Edited
            }
            KeyCode::Delete if ctrl => {
                self.delete_word_right();
                KeyOutcome::Edited
            }
            // Обычный ввод символа: Ctrl+символ не печатаем (это шорткат вышестоящего
            // слоя), иначе в поле попал бы управляющий символ.
            KeyCode::Char(c) if !ctrl => {
                self.insert_char(c);
                KeyOutcome::Edited
            }
            KeyCode::Backspace => {
                self.backspace();
                KeyOutcome::Edited
            }
            KeyCode::Delete => {
                self.delete();
                KeyOutcome::Edited
            }
            _ => KeyOutcome::Ignored,
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
        // Запоминаем для маппинга клика мыши (экран → позиция в тексте).
        self.last_area = Some(inner);

        if self.single_line {
            self.render_single_line(frame, inner, focused, palette, command);
            return;
        }

        let view_w = inner.width.max(1) as usize;
        let visible_rows = inner.height.max(1) as usize;
        // Запоминаем ширину для навигации `↑/↓` по визуальным рядам (см. `move_up`).
        self.last_width = view_w;

        // Визуальные ряды с учётом переноса; позиция курсора — через тот же перенос
        // (единый источник истины, иначе курсор разъедется с текстом). Берём копию
        // кэша: дальше мутируем `self` (scroll/курсор), поэтому заимствование держать
        // нельзя, а memcpy готового результата дешевле повторного O(n)-переноса.
        let vrows = self.rows_cached(view_w).to_vec();
        let (cursor_row, cursor_col) = self.cursor_visual(&vrows);
        self.adjust_scroll(cursor_row, vrows.len(), visible_rows);

        // В режиме команды весь текст красим в `warning` и не подчёркиваем ошибки;
        // выделение (фон) показываем в любом режиме.
        let base_fg = command.then_some(palette.warning);
        let lines: Vec<Line> = vrows
            .iter()
            .skip(self.scroll)
            .take(visible_rows)
            .map(|&(li, start, end)| {
                let sub = &self.lines[li][start..end];
                let mis = if command {
                    None
                } else {
                    self.misspelled
                        .get(li)
                        .map(|rs| clip_ranges(rs, start, end))
                };
                let sel = self.row_selection(li, start, end);
                styled_line(sub, mis.as_deref(), sel, base_fg, palette)
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
        // Выравниваем левый край скролла по границе символа. Без этого при широком
        // глифе (CJK/эмодзи) слева срез мог начаться «в середине» символа, а `hscroll`
        // (в колонках) не совпадал бы с реальной шириной скрытого префикса — курсор
        // рисовался бы на колонку правее фактической позиции. `col_at_width` округляет
        // старт вверх до границы, а `hscroll` приводим к ширине этого префикса
        // (гарантированно `≤ cursor_vw` → курсор остаётся видимым, см. spec §11.6).
        let start = col_at_width(line, self.hscroll);
        self.hscroll = wrap::display_width(&line[..start]);
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
        } else {
            let base_fg = command.then_some(palette.warning);
            let mis = if command {
                None
            } else {
                self.misspelled
                    .first()
                    .map(|rs| clip_ranges(rs, start, end))
            };
            let sel = self.row_selection(0, start, end);
            Text::from(styled_line(sub, mis.as_deref(), sel, base_fg, palette))
        };
        frame.render_widget(Paragraph::new(text), inner);

        if focused {
            let cursor_x = inner.x + (cursor_vw - self.hscroll) as u16;
            let x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
            frame.set_cursor_position((x, inner.y));
        }
    }

    /// Помечает содержимое изменённым — инвалидирует кэш визуальных рядов (следующий
    /// [`Self::rows_cached`] увидит несовпадение ревизии) и **снимает выделение**
    /// (после правки якорь указывал бы на устаревшие координаты). Зовётся всеми
    /// мутаторами `lines`; навигация его НЕ зовёт (она сама ведёт `anchor`).
    /// [`Self::delete_selection`] снимает `anchor` до `touch` — двойной сброс безвреден.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.anchor = None;
    }

    /// Визуальные ряды с кэшем по `(ширина, ревизия)` (см. [`Self::rows_cache`]).
    /// Пересчитывает перенос только при смене ширины или содержимого; иначе отдаёт
    /// заимствование в кэш. Потребители, которые дальше мутируют `self`, берут копию
    /// (`.to_vec()` — дешёвый memcpy результата против O(n)-переноса).
    fn rows_cached(&mut self, width: usize) -> &[VisualRow] {
        let fresh =
            matches!(&self.rows_cache, Some((w, r, _)) if *w == width && *r == self.revision);
        if !fresh {
            let rows = self.visual_rows(width);
            self.rows_cache = Some((width, self.revision, rows));
        }
        // Кэш только что заполнен/проверен — unwrap безопасен.
        &self.rows_cache.as_ref().unwrap().2
    }

    /// Визуальные ряды: для каждого — `(логическая строка, начало, конец)` в
    /// индексах символов этой строки (с учётом переноса по ширине `width`). Чистый
    /// пересчёт; кэшированный путь — [`Self::rows_cached`].
    fn visual_rows(&self, width: usize) -> Vec<VisualRow> {
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
    fn cursor_visual(&self, vrows: &[VisualRow]) -> (usize, usize) {
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

/// Метод движения курсора для клавиши навигации (или `None`, если клавиша — не
/// навигация). Выделено таблицей, чтобы логику выделения (`Shift` → якорь, обычное →
/// снять) применить единообразно ко всем направлениям без дублирования веток. `Ctrl`
/// усиливает `←/→` до слова, `Home/End` — до границ текста; `Ctrl+↑/↓` не задан
/// (падает в `None` → `Ignored`, как раньше).
fn navigation(code: KeyCode, ctrl: bool) -> Option<fn(&mut InputBox)> {
    Some(match (code, ctrl) {
        (KeyCode::Left, true) => InputBox::move_word_left,
        (KeyCode::Right, true) => InputBox::move_word_right,
        (KeyCode::Home, true) => InputBox::move_doc_start,
        (KeyCode::End, true) => InputBox::move_doc_end,
        (KeyCode::Left, false) => InputBox::move_left,
        (KeyCode::Right, false) => InputBox::move_right,
        (KeyCode::Up, false) => InputBox::move_up,
        (KeyCode::Down, false) => InputBox::move_down,
        (KeyCode::Home, false) => InputBox::move_home,
        (KeyCode::End, false) => InputBox::move_end,
        _ => return None,
    })
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
fn is_soft(vrows: &[VisualRow], idx: usize) -> bool {
    idx + 1 < vrows.len() && vrows[idx + 1].0 == vrows[idx].0
}

/// Логический столбец на ряду `[start, end)`, ближайший к целевой визуальной колонке
/// `target_vw` (в колонках) — для перехода `↑/↓` с сохранением колонки. На мягком
/// переносе не отдаём `end` (иначе курсор «уедет» в начало следующего ряда) —
/// откатываемся на символ назад, оставаясь на этом ряду. Результат снапится к границе
/// графемного кластера, чтобы курсор не садился между базой и вариатором эмодзи
/// (`❤️` = `❤`+U+FE0F): иначе последующий `insert`/`backspace` разорвал бы кластер
/// (осиротевший селектор). См. spec §11.5.
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
    // Снап вниз к границе кластера. Всегда `≥ start` (start — граница ряда), поэтому
    // курсор не покидает этот визуальный ряд.
    wrap::snap_boundary(line, col)
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

/// Строит строку, компонуя три оформления по символам: подчёркивание ошибок
/// орфографии (`misspelled`, `UNDERLINED` + цвет ошибки), фон выделения (`selection`,
/// `keycap_bg`) и базовый цвет команды (`base_fg`, весь текст). Диапазоны — row-local
/// `[start, end)` в символах; выделение и ошибки могут пересекаться (складываются:
/// подчёркнуто И на фоне). Быстрый путь — когда оформлять нечего.
fn styled_line(
    chars: &[char],
    misspelled: Option<&[(usize, usize)]>,
    selection: Option<(usize, usize)>,
    base_fg: Option<Color>,
    palette: &Palette,
) -> Line<'static> {
    let has_mis = misspelled.is_some_and(|r| !r.is_empty());
    let has_sel = selection.is_some_and(|(s, e)| e > s);
    if !has_mis && !has_sel && base_fg.is_none() {
        return Line::from(chars.iter().collect::<String>());
    }
    let n = chars.len();
    let base = base_fg.map(|c| Style::new().fg(c)).unwrap_or_default();
    let mut styles = vec![base; n];
    if let Some(ranges) = misspelled {
        let bad = Style::new().underlined().fg(palette.error);
        for &(s, e) in ranges {
            for st in styles.iter_mut().take(e.min(n)).skip(s.min(n)) {
                *st = st.patch(bad);
            }
        }
    }
    if let Some((s, e)) = selection {
        for st in styles.iter_mut().take(e.min(n)).skip(s.min(n)) {
            *st = st.bg(palette.keycap_bg);
        }
    }
    // Склеиваем соседние символы с одинаковым стилем в спаны.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut i = 0;
    while i < n {
        let st = styles[i];
        let mut buf = String::new();
        while i < n && styles[i] == st {
            buf.push(chars[i]);
            i += 1;
        }
        spans.push(Span::styled(buf, st));
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

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn ctrl_shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
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

    // ---------- этап C: отмена/повтор ----------

    #[test]
    fn undo_typing_run_is_one_unit_then_redo() {
        // Набор без пробелов — одна единица отмены; Ctrl+Z → пусто, Ctrl+Y → назад.
        let mut ib = InputBox::new();
        type_str(&mut ib, "hello");
        assert!(ib.undo());
        assert!(ib.is_empty());
        assert!(ib.redo());
        assert_eq!(ib.text(), "hello");
        // повтор исчерпан
        assert!(!ib.redo());
    }

    #[test]
    fn undo_breaks_on_whitespace_word_granular() {
        // Пробел завершает единицу: "ab cd" отменяется по словам ("ab " ← "").
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab cd");
        assert!(ib.undo());
        assert_eq!(ib.text(), "ab ");
        assert!(ib.undo());
        assert!(ib.is_empty());
    }

    #[test]
    fn navigation_breaks_undo_coalescing() {
        // Набор, стрелка, ещё набор → две единицы отмены (разрыв по навигации).
        let mut ib = InputBox::new();
        type_str(&mut ib, "abc");
        ib.on_key(k(KeyCode::Left)); // навигация сбрасывает коалесинг
        ib.insert_char('X'); // курсор был перед 'c' → "abXc"
        assert_eq!(ib.text(), "abXc");
        assert!(ib.undo());
        assert_eq!(ib.text(), "abc"); // отменён только 'X'
    }

    #[test]
    fn insert_str_is_separate_undo_unit() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab");
        ib.insert_str("XY"); // вставка — отдельная (Structural) единица
        assert_eq!(ib.text(), "abXY");
        assert!(ib.undo());
        assert_eq!(ib.text(), "ab"); // отменена только вставка
    }

    #[test]
    fn edit_clears_redo() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "abc");
        ib.undo(); // "" , redo has "abc"
        ib.insert_char('z'); // новая правка чистит redo
        assert!(!ib.redo());
        assert_eq!(ib.text(), "z");
    }

    #[test]
    fn set_text_clears_undo_history() {
        // Программная замена (загрузка чужого черновика) — Ctrl+Z не воскрешает.
        let mut ib = InputBox::new();
        type_str(&mut ib, "user text");
        ib.set_text("другой чат");
        assert!(!ib.undo());
        assert_eq!(ib.text(), "другой чат");
    }

    #[test]
    fn ctrl_k_clears_and_ctrl_z_restores() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "привет\nмир");
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('k'))), KeyOutcome::Edited);
        assert!(ib.is_empty());
        // Ctrl+Z возвращает удалённое (общая модель отмены, не toggle).
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('z'))), KeyOutcome::Edited);
        assert_eq!(ib.text(), "привет\nмир");
    }

    #[test]
    fn undo_redo_noop_returns_moved() {
        // Пустые стеки — Ctrl+Z/Ctrl+Y ничего не меняют (Moved, не Edited).
        let mut ib = InputBox::new();
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('z'))), KeyOutcome::Moved);
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('y'))), KeyOutcome::Moved);
    }

    #[test]
    fn undo_cap_evicts_oldest() {
        // Больше UNDO_CAP единиц — старейшие вытесняются (не паникует, стек ограничен).
        let mut ib = InputBox::new();
        for _ in 0..(UNDO_CAP + 20) {
            // Каждая вставка — Structural → отдельная единица.
            ib.insert_str("x");
        }
        assert_eq!(ib.undo.len(), UNDO_CAP);
    }

    #[test]
    fn undo_restores_selection_replacement() {
        // Ввод поверх выделения — одна единица: Ctrl+Z возвращает исходный текст.
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a'))); // выделить всё
        ib.insert_char('Z'); // заменить выделение
        assert_eq!(ib.text(), "Z");
        assert!(ib.undo());
        assert_eq!(ib.text(), "hello");
    }

    #[test]
    fn on_key_handles_editing_but_not_enter() {
        let mut ib = InputBox::new();
        assert!(ib.on_key(k(KeyCode::Char('x'))).handled());
        assert!(ib.on_key(k(KeyCode::Backspace)).handled());
        assert!(!ib.on_key(k(KeyCode::Enter)).handled());
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
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 3)); // "оди|н два три" — колонка 3 сохранена
        // ещё одно ↑ на верхнем визуальном ряду — без движения
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 3));
    }

    #[test]
    fn arrow_down_moves_within_wrapped_line() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        render_at(&mut ib, 8);
        ib.row = 0;
        ib.col = 3; // верхний визуальный ряд, колонка 3
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        // на нижний ряд "три" с сохранением колонки → конец строки (3 символа)
        assert_eq!(ib.cursor(), (0, 12));
        // ещё одно ↓ на нижнем визуальном ряду — без движения
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (0, 12));
    }

    #[test]
    fn arrow_up_down_cross_logical_lines_when_not_wrapped() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef"); // две короткие логические строки, без переноса
        render_at(&mut ib, 20);
        ib.row = 1;
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 2)); // перешли на предыдущую логическую строку
        assert!(ib.on_key(k(KeyCode::Down)).handled());
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
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (1, 1)); // "x" короче — курсор прижат к концу
        assert!(ib.on_key(k(KeyCode::Down)).handled());
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
        assert!(ib.on_key(k(KeyCode::Down)).handled()); // (1,1), goal=5
        assert!(ib.on_key(k(KeyCode::Left)).handled()); // горизонтальное движение сбрасывает goal
        assert!(ib.on_key(k(KeyCode::Down)).handled());
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
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 9)); // начало ряда "три", а не всей строки
        assert!(ib.on_key(k(KeyCode::End)).handled());
        assert_eq!(ib.cursor(), (0, 12)); // конец ряда "три" = конец строки
        // на верхнем ряду End встаёт на последнюю позицию ряда (мягкий перенос)
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::End)).handled());
        assert_eq!(ib.cursor(), (0, 8)); // конец "один два", не уезжает в начало "три"
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 0)); // начало верхнего ряда
    }

    #[test]
    fn arrow_up_falls_back_to_logical_before_render() {
        // до первого рендера ширина неизвестна (last_width == 0) → логический переход
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef");
        ib.row = 1;
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)).handled());
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
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 2));
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (0, 2));
    }

    #[test]
    fn single_line_home_end_span_whole_value() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("a long value");
        render_at(&mut ib, 4); // узкое поле — значение длиннее ширины
        ib.col = 5;
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.on_key(k(KeyCode::End)).handled());
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
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 9));
        // ещё раз → начало "два"
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 5));
        // ещё раз → начало "один"
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 0));
        // Ctrl+→ → за концом "один"
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.cursor(), (0, 4));
        // ещё раз → за концом "два"
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.cursor(), (0, 8));
    }

    #[test]
    fn ctrl_left_right_cross_logical_lines() {
        let mut ib = InputBox::new();
        ib.set_text("ab\ncd");
        ib.row = 1;
        ib.col = 0; // начало второй строки
        // Ctrl+← на границе строки → конец предыдущей
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 2));
        // Ctrl+→ из конца первой строки → начало следующей
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.cursor(), (1, 0));
    }

    #[test]
    fn ctrl_backspace_deletes_word_left() {
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // курсор в конце
        assert!(ib.on_key(ctrl(KeyCode::Backspace)).handled());
        assert_eq!(ib.text(), "один два ");
        assert_eq!(ib.cursor(), (0, 9));
        // в начале строки склеивает со строкой выше (как обычный Backspace)
        ib.set_text("ab\ncd");
        ib.row = 1;
        ib.col = 0;
        assert!(ib.on_key(ctrl(KeyCode::Backspace)).handled());
        assert_eq!(ib.text(), "abcd");
    }

    #[test]
    fn ctrl_delete_deletes_word_right() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        ib.col = 0;
        assert!(ib.on_key(ctrl(KeyCode::Delete)).handled());
        assert_eq!(ib.text(), " два три"); // удалено слово "один", пробел остался
        assert_eq!(ib.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_home_end_jump_to_document_bounds() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef\nghi");
        ib.row = 1;
        ib.col = 1;
        assert!(ib.on_key(ctrl(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.on_key(ctrl(KeyCode::End)).handled());
        assert_eq!(ib.cursor(), (2, 3));
    }

    #[test]
    fn ctrl_char_is_not_inserted() {
        // Ctrl+символ — шорткат вышестоящего слоя, в поле не печатается. Берём `j`
        // (нейтральный; `a` теперь «выделить всё», прочие Ctrl-буквы не обработаны).
        let mut ib = InputBox::new();
        assert!(!ib.on_key(ctrl(KeyCode::Char('j'))).handled());
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

    // ---------- п.1: снап курсора к границе кластера ----------

    #[test]
    fn col_for_visual_snaps_off_emoji_cluster() {
        // "❤️abc" = ❤(U+2764) + U+FE0F + a + b + c. Целевая колонка 1 попадает между
        // базой и селектором → снап к началу кластера (0), а не в его середину (иначе
        // insert/backspace разорвали бы ❤️). Колонка 2 — сразу за кластером (граница).
        let line: Vec<char> = "❤\u{FE0F}abc".chars().collect();
        assert_eq!(col_for_visual(&line, 0, line.len(), 1, false), 0);
        assert_eq!(col_for_visual(&line, 0, line.len(), 2, false), 2);
        // Мягкий перенос: конец ряда на VS16-кластере не оставляет курсор в середине.
        // ряд "❤️" [0,2): target за концом, soft → откат, затем снап к границе (0).
        assert_eq!(col_for_visual(&line, 0, 2, 10, true), 0);
    }

    #[test]
    fn arrow_up_lands_on_cluster_boundary() {
        // Верхний ряд начинается с ❤️; ↓ затем ↑ с goal-колонкой 1 (середина ❤️) не
        // сажает курсор между базой и селектором.
        let mut ib = InputBox::new();
        ib.set_text("❤\u{FE0F}xy\nz");
        render_at(&mut ib, 20);
        ib.row = 1;
        ib.col = 1; // визуальная колонка 1 на нижней строке "z"
        ib.goal_col = None;
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        // на верхней строке колонка-цель 1 попадает в ❤️ → курсор снапится к 0 или 2,
        // но НЕ встаёт между скалярами (индекс 1 = между ❤ и U+FE0F)
        assert_ne!(ib.cursor(), (0, 1), "курсор сел в середину кластера ❤️");
    }

    // ---------- п.3: жёсткий инвариант однострочного режима ----------

    #[test]
    fn single_line_after_multiline_content_merges_without_panic() {
        let mut ib = InputBox::new();
        ib.set_text("first\nsecond\nthird"); // многострочно, курсор в конце
        ib.set_single_line(true); // включаем ПОСЛЕ set_text — инвариант должен устоять
        assert_eq!(ib.line_count(), 1);
        assert_eq!(ib.text(), "first second third");
        assert_eq!(ib.cursor().0, 0);
        // рендер однострочного не паникует (col не за границей lines[0])
        render_at(&mut ib, 8);
    }

    // ---------- п.4: выравнивание hscroll по границе символа ----------

    #[test]
    fn single_line_hscroll_aligns_to_char_boundary() {
        // "世aBcd": ведущий CJK-глиф шириной 2. Курсор после "世a" (визуальная колонка
        // 3), узкое поле (view_w=3) → скролл. Без выравнивания hscroll оказался бы «в
        // середине» 世 (значение вне множества префиксных ширин) и курсор рисовался бы
        // на колонку правее. См. spec §11.6.
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("世aBcd");
        ib.col = 2; // после "世a"
        render_at(&mut ib, 3);
        let line: Vec<char> = "世aBcd".chars().collect();
        let boundary_widths: Vec<usize> = (0..=line.len())
            .map(|k| wrap::display_width(&line[..k]))
            .collect();
        assert!(
            boundary_widths.contains(&ib.hscroll_for_test()),
            "hscroll={} не совпал ни с одной префиксной шириной (не на границе символа)",
            ib.hscroll_for_test()
        );
    }

    // ---------- п.5: синхронизация подчёркиваний орфографии с правкой ----------

    #[test]
    fn misspelled_shifts_on_insert_before_word() {
        let mut ib = InputBox::new();
        ib.set_text("foo bar");
        ib.set_misspelled(vec![vec![(4, 7)]]); // помечено "bar"
        ib.row = 0;
        ib.col = 0;
        ib.insert_char('X'); // "Xfoo bar" — "bar" сдвинулось на [5,8)
        assert_eq!(ib.misspelled_ranges_for_test(0), vec![(5, 8)]);
    }

    #[test]
    fn misspelled_dropped_when_edited_inside_word() {
        let mut ib = InputBox::new();
        ib.set_text("foo bar");
        ib.set_misspelled(vec![vec![(4, 7)]]);
        ib.row = 0;
        ib.col = 5; // внутри "bar"
        ib.insert_char('X'); // правка внутри слова — диапазон сбрасывается
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
    }

    #[test]
    fn misspelled_shifts_left_on_delete_after_word() {
        let mut ib = InputBox::new();
        ib.set_text("X foo");
        ib.set_misspelled(vec![vec![(2, 5)]]); // помечено "foo"
        ib.row = 0;
        ib.col = 0;
        ib.delete(); // удалили 'X' в начале → "foo" теперь [1,4)? нет: " foo" [1,4)
        assert_eq!(ib.misspelled_ranges_for_test(0), vec![(1, 4)]);
    }

    #[test]
    fn misspelled_synced_on_newline_and_join() {
        let mut ib = InputBox::new();
        ib.set_text("foo bar");
        ib.set_misspelled(vec![vec![(0, 3), (4, 7)]]);
        ib.row = 0;
        ib.col = 3; // после "foo"
        ib.insert_newline(); // "foo" | " bar": текущая сброшена, новая пустая
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
        assert!(ib.misspelled_ranges_for_test(1).is_empty());
        // склейка обратно — согласованность сохраняется, без паники
        ib.row = 1;
        ib.col = 0;
        ib.backspace();
        assert_eq!(ib.line_count(), 1);
    }

    #[test]
    fn set_text_and_paste_clear_misspelled() {
        let mut ib = InputBox::new();
        ib.set_text("helo");
        ib.set_misspelled(vec![vec![(0, 4)]]);
        ib.set_text("совсем другой текст"); // старые диапазоны прежнего текста сброшены
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
        // вставка тоже сбрасывает подчёркивания (перепроверка их перестроит)
        ib.set_misspelled(vec![vec![(0, 6)]]);
        ib.insert_str("abc");
        assert!(ib.misspelled_is_empty());
    }

    // ---------- п.6: on_key различает правку и движение ----------

    #[test]
    fn on_key_distinguishes_edit_move_ignore() {
        let mut ib = InputBox::new();
        assert_eq!(ib.on_key(k(KeyCode::Char('a'))), KeyOutcome::Edited);
        assert_eq!(ib.on_key(k(KeyCode::Left)), KeyOutcome::Moved);
        assert_eq!(ib.on_key(k(KeyCode::Backspace)), KeyOutcome::Edited);
        assert_eq!(ib.on_key(k(KeyCode::Enter)), KeyOutcome::Ignored);
        assert_eq!(ib.on_key(ctrl(KeyCode::Left)), KeyOutcome::Moved);
        assert_eq!(ib.on_key(ctrl(KeyCode::Backspace)), KeyOutcome::Edited);
        // хелперы edited()/handled()
        assert!(KeyOutcome::Edited.edited());
        assert!(!KeyOutcome::Moved.edited());
        assert!(KeyOutcome::Moved.handled());
        assert!(!KeyOutcome::Ignored.handled());
    }

    // ---------- п.7: кэш визуальных рядов + дешёвый предохранитель ----------

    #[test]
    fn row_cache_invalidates_on_every_mutator() {
        // После правки кэш визуальных рядов обязан совпасть со свежим пересчётом —
        // иначе где-то забыт `touch()` (кэш вернул бы устаревший перенос, разъехавшись
        // с реальным содержимым: неверные курсор/скролл).
        fn check(setup: &str, mutate: impl FnOnce(&mut InputBox)) {
            const W: usize = 6;
            let mut ib = InputBox::new();
            ib.set_text(setup);
            let _ = ib.rows_cached(W); // заполняем кэш ДО правки
            mutate(&mut ib);
            let cached = ib.rows_cached(W).to_vec();
            let fresh = ib.visual_rows(W);
            assert_eq!(cached, fresh, "кэш визуальных рядов не инвалидировался");
        }
        check("abc", |ib| {
            ib.col = 3;
            ib.insert_char('d');
        });
        check("abc", |ib| ib.replace_range(0, 0, 3, "xy"));
        check("abc", |ib| {
            ib.col = 3;
            ib.backspace();
        });
        check("ab\ncd", |ib| {
            ib.row = 1;
            ib.col = 0;
            ib.backspace(); // склейка строк
        });
        check("abc", |ib| {
            ib.col = 0;
            ib.delete();
        });
        check("ab\ncd", |ib| {
            ib.row = 0;
            ib.col = 2;
            ib.delete(); // склейка строк
        });
        check("abc def", |ib| {
            ib.col = 7;
            ib.delete_word_left();
        });
        check("abc def", |ib| {
            ib.col = 0;
            ib.delete_word_right();
        });
        check("abc", |ib| {
            ib.col = 1;
            ib.insert_newline();
        });
        check("abc", |ib| ib.insert_str("X\nY"));
        check("abc", |ib| ib.set_text("zzzz"));
        check("abc", |ib| ib.clear());
    }

    #[test]
    fn navigation_preserves_revision_but_edit_bumps_it() {
        // Инвариант оптимизации: движение курсора не инвалидирует кэш (ревизия не
        // растёт), а правка — растит (кэш пересчитается).
        let mut ib = InputBox::new();
        ib.set_text("hello world");
        render_at(&mut ib, 20);
        let r0 = ib.revision;
        assert!(ib.on_key(k(KeyCode::Left)).handled());
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.revision, r0, "навигация не должна инвалидировать кэш");
        assert!(ib.on_key(k(KeyCode::Char('!'))).handled());
        assert!(ib.revision > r0, "правка должна инвалидировать кэш");
    }

    #[test]
    fn first_non_whitespace_finds_leading_glyph() {
        let mut ib = InputBox::new();
        assert_eq!(ib.first_non_whitespace(), None); // пусто
        ib.set_text("  /rag add x");
        assert_eq!(ib.first_non_whitespace(), Some('/'));
        ib.set_text("привет");
        assert_eq!(ib.first_non_whitespace(), Some('п'));
        ib.set_text("\n\n  x"); // ведущие пустые строки/пробелы
        assert_eq!(ib.first_non_whitespace(), Some('x'));
        ib.set_text("   "); // только пробелы
        assert_eq!(ib.first_non_whitespace(), None);
    }

    // ---------- этап A: выделение ----------

    #[test]
    fn shift_arrow_extends_selection_plain_arrow_collapses() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.col = 0;
        assert!(!ib.has_selection());
        // Shift+Right ×2 → выделено "he"
        ib.on_key(shift(KeyCode::Right));
        ib.on_key(shift(KeyCode::Right));
        assert!(ib.has_selection());
        assert_eq!(ib.selected_text().as_deref(), Some("he"));
        // обычный Right снимает выделение
        ib.on_key(k(KeyCode::Right));
        assert!(!ib.has_selection());
        assert_eq!(ib.selected_text(), None);
    }

    #[test]
    fn ctrl_a_selects_all() {
        let mut ib = InputBox::new();
        ib.set_text("line1\nline2");
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('a'))), KeyOutcome::Moved);
        assert!(ib.has_selection());
        assert_eq!(ib.selected_text().as_deref(), Some("line1\nline2"));
        assert_eq!(ib.cursor(), (1, 5));
    }

    #[test]
    fn ctrl_shift_right_selects_word() {
        let mut ib = InputBox::new();
        ib.set_text("one two");
        ib.col = 0;
        ib.on_key(ctrl_shift(KeyCode::Right)); // до конца "one"
        assert_eq!(ib.selected_text().as_deref(), Some("one"));
    }

    #[test]
    fn typing_replaces_selection() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a'))); // выделить всё
        assert_eq!(ib.on_key(k(KeyCode::Char('X'))), KeyOutcome::Edited);
        assert_eq!(ib.text(), "X");
        assert!(!ib.has_selection());
    }

    #[test]
    fn backspace_deletes_whole_selection() {
        let mut ib = InputBox::new();
        ib.set_text("abcdef");
        ib.col = 1;
        for _ in 0..3 {
            ib.on_key(shift(KeyCode::Right)); // выделено "bcd"
        }
        assert_eq!(ib.selected_text().as_deref(), Some("bcd"));
        ib.on_key(k(KeyCode::Backspace));
        assert_eq!(ib.text(), "aef");
        assert!(!ib.has_selection());
    }

    #[test]
    fn delete_multiline_selection_merges_and_syncs_misspelled() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef\nghi");
        ib.set_misspelled(vec![vec![(0, 3)], vec![(0, 3)], vec![(0, 3)]]);
        // выделение (0,1)..(2,2): "bc\ndef\ngh"
        ib.anchor = Some((0, 1));
        ib.row = 2;
        ib.col = 2;
        assert_eq!(ib.selected_text().as_deref(), Some("bc\ndef\ngh"));
        assert!(ib.delete_selection());
        assert_eq!(ib.text(), "ai"); // "a" + "i"
        assert_eq!(ib.cursor(), (0, 1));
        assert_eq!(ib.line_count(), 1);
        // подчёркивания синхронизированы: одна строка, первая сброшена
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
    }

    #[test]
    fn shift_nav_is_moved_edit_over_selection_is_edited() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        ib.col = 0;
        let r0 = ib.revision;
        assert_eq!(ib.on_key(shift(KeyCode::Right)), KeyOutcome::Moved);
        assert_eq!(
            ib.revision, r0,
            "расширение выделения не бампит ревизию (кэш цел)"
        );
        assert!(ib.has_selection());
        assert_eq!(ib.on_key(k(KeyCode::Char('Z'))), KeyOutcome::Edited);
        assert!(ib.revision > r0, "правка поверх выделения инвалидирует кэш");
    }

    #[test]
    fn render_highlights_selection_background() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a'))); // выделить всё
        let pal = Palette::default();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| ib.render(f, f.area(), "ввод", true, &pal, false))
            .unwrap();
        let buf = term.backend().buffer();
        let area = buf.area;
        let has_sel_bg = (area.left()..area.right()).any(|x| {
            (area.top()..area.bottom()).any(|y| buf[(x, y)].style().bg == Some(pal.keycap_bg))
        });
        assert!(has_sel_bg, "выделение не отрисовано фоном keycap_bg");
    }

    #[test]
    fn paste_replaces_selection() {
        // Вставка (мимо `on_key`) тоже заменяет выделение — удаление вынесено в сам
        // мутатор `insert_str`, поэтому paste/`Shift+Enter`/эмодзи уважают выделение.
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a')));
        assert!(ib.has_selection());
        ib.insert_str("XY");
        assert_eq!(ib.text(), "XY");
        assert!(!ib.has_selection());
    }

    // ---------- этап D: мышь (клик → курсор, драг → выделение) ----------

    // `render_at(ib, inner_w)` рисует во внутреннюю ширину `inner_w`: рамка добавляет
    // рамку (1 слева) + колонку приглашения `❯` (PROMPT_W), поэтому область текста
    // начинается в экранном `x = 1 + PROMPT_W = 3`, `y = 1`. Экранные координаты
    // клика по визуальной ячейке `(vrow, vcol)` — `(3 + vcol, 1 + vrow)`.
    const TX: u16 = 1 + PROMPT_W; // левый край области текста при render_at
    const TY: u16 = 1; // верхний край области текста

    #[test]
    fn place_cursor_at_maps_click_to_position() {
        let mut ib = InputBox::new();
        ib.set_text("hello world"); // одна логическая строка
        render_at(&mut ib, 20); // широко — без переноса
        // клик в середину строки → курсор туда
        assert!(ib.place_cursor_at(TX + 3, TY));
        assert_eq!(ib.cursor(), (0, 3));
        // клик правее конца текста (в пределах области) → конец ряда
        assert!(ib.place_cursor_at(TX + 15, TY));
        assert_eq!(ib.cursor(), (0, 11));
    }

    #[test]
    fn place_cursor_below_last_row_goes_to_text_end() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef");
        render_at(&mut ib, 20);
        // клик ниже последнего ряда (но в пределах высоты области) → конец текста
        assert!(ib.place_cursor_at(TX, TY + 5));
        assert_eq!(ib.cursor(), (1, 3));
    }

    #[test]
    fn place_cursor_snaps_to_cluster_boundary() {
        // Клик в середину VS16-кластера ❤️ (❤ + U+FE0F, ширина 2) снапится к границе
        // (0), а не садится между базой и селектором (иначе insert/backspace порвал бы).
        let mut ib = InputBox::new();
        ib.set_text("❤\u{FE0F}abc");
        render_at(&mut ib, 20);
        assert!(ib.place_cursor_at(TX + 1, TY)); // визуальная колонка 1 = середина ❤️
        assert_ne!(ib.cursor(), (0, 1), "курсор сел в середину кластера ❤️");
        assert_eq!(ib.cursor(), (0, 0));
    }

    #[test]
    fn place_cursor_outside_area_is_noop() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        ib.row = 0;
        ib.col = 2;
        // клик левее области текста (в колонке приглашения/рамке) — не двигает курсор
        assert!(!ib.place_cursor_at(0, TY));
        assert_eq!(ib.cursor(), (0, 2));
    }

    #[test]
    fn place_cursor_before_render_is_noop() {
        // До первого рендера last_area == None → клик игнорируется.
        let mut ib = InputBox::new();
        ib.set_text("hello");
        assert!(!ib.place_cursor_at(3, 1));
    }

    #[test]
    fn mouse_press_then_drag_builds_selection() {
        let mut ib = InputBox::new();
        ib.set_text("hello world");
        render_at(&mut ib, 20);
        // нажатие в колонке 0 — курсор туда, выделения ещё нет (пустое)
        assert!(ib.mouse_press(TX, TY));
        assert_eq!(ib.cursor(), (0, 0));
        assert!(!ib.has_selection());
        // драг до колонки 5 растит выделение "hello"
        assert!(ib.mouse_drag(TX + 5, TY));
        assert_eq!(ib.cursor(), (0, 5));
        assert!(ib.has_selection());
        assert_eq!(ib.selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn mouse_press_without_drag_is_empty_selection() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        assert!(ib.mouse_press(TX + 3, TY));
        assert_eq!(ib.cursor(), (0, 3));
        assert!(!ib.has_selection()); // клик без драга — курсор перемещён, выделения нет
    }

    #[test]
    fn mouse_press_outside_keeps_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        ib.row = 0;
        ib.col = 4;
        assert!(!ib.mouse_press(0, TY)); // вне области текста
        assert_eq!(ib.cursor(), (0, 4));
        assert!(!ib.has_selection());
    }

    #[test]
    fn mouse_drag_selects_across_wrapped_rows() {
        // Драг через мягкий перенос выделяет по логическим координатам.
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // ширина 8: "один два " | "три"
        render_at(&mut ib, 8);
        assert!(ib.mouse_press(TX, TY)); // начало верхнего ряда (0,0)
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.mouse_drag(TX + 1, TY + 1)); // нижний ряд "три", колонка 1
        assert_eq!(ib.cursor(), (0, 10)); // "три" начинается на индексе 9 → +1 = 10
        assert_eq!(ib.selected_text().as_deref(), Some("один два т"));
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
