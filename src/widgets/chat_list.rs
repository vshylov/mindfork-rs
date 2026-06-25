//! Оверлей списка чатов: поиск-фильтр, две сортировки, переименование по месту,
//! создание/клонирование/удаление. Открывается/закрывается по `Esc`. См. spec §11.2.
//!
//! Виджет самодостаточен: хранит снимок списка и состояние ввода, на нажатия
//! отвечает [`ChatListAction`] (его исполняет `app` — единственный писатель).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::features::chat_search_sort::{SortMode, filter_and_sort};
use crate::features::rename_chat::sanitize_title;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::wrap;

/// Шаг постраничного перемещения выделения по `PageUp`/`PageDown`. Фиксированный,
/// так как фактическая высота списка известна только во время рендера.
const PAGE_STEP: usize = 10;

/// Действие, которое оверлей просит выполнить вышестоящий слой.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatListAction {
    /// Ничего (нажатие обработано внутри оверлея).
    None,
    /// Закрыть оверлей.
    Close,
    /// Выйти из приложения (`Ctrl+C`).
    Quit,
    /// Сделать чат активным (и закрыть оверлей).
    Switch(Uuid),
    /// Создать новый чат.
    New,
    /// Клонировать чат.
    Clone(Uuid),
    /// Скопировать всю переписку чата в буфер обмена.
    Copy(Uuid),
    /// Мягко удалить чат.
    Delete(Uuid),
    /// Переименовать чат.
    Rename { id: Uuid, title: String },
    /// Авто-название: модель читает переписку и придумывает заголовок.
    AutoRename(Uuid),
}

/// Режим ввода внутри оверлея.
enum Mode {
    /// Ввод в строку поиска.
    Search,
    /// Переименование выбранного чата по месту. Текст хранится посимвольно
    /// (`Vec<char>`), `cursor` — позиция курсора в символах (для перемещения
    /// стрелками/Home/End и вставки/удаления в середине).
    Rename {
        id: Uuid,
        buffer: Vec<char>,
        cursor: usize,
    },
}

/// Состояние оверлея списка чатов.
pub struct ChatListState {
    /// Снимок всех видимых чатов (обновляется из `AppEvent::ChatList`).
    all: Vec<ChatSummary>,
    query: String,
    sort: SortMode,
    /// Индекс выделения в текущем отфильтрованном списке.
    selected: usize,
    mode: Mode,
    /// Текущая ошибка операции (авто-название/удаление/клон) для отдельной области.
    /// Сбрасывается при следующем нажатии клавиши.
    error: Option<String>,
    /// Подтверждение операции (напр. «скопировано») для той же области, но как
    /// успех. Сбрасывается при следующем нажатии клавиши. Взаимоисключимо с `error`.
    notice: Option<String>,
}

impl ChatListState {
    /// Открывает оверлей со снимком списка; выделение — на активном чате.
    pub fn new(chats: Vec<ChatSummary>, active: Option<Uuid>) -> Self {
        let mut state = Self {
            all: chats,
            query: String::new(),
            sort: SortMode::default(),
            selected: 0,
            mode: Mode::Search,
            error: None,
            notice: None,
        };
        if let Some(active) = active {
            let visible = state.visible();
            if let Some(idx) = visible.iter().position(|c| c.id == active) {
                state.selected = idx;
            }
        }
        state
    }

    /// Обновляет снимок списка (после изменений набора чатов), сохраняя выделение
    /// по возможности на том же чате.
    pub fn set_chats(&mut self, chats: Vec<ChatSummary>) {
        let current = self.selected_id();
        self.all = chats;
        let visible = self.visible();
        self.selected = current
            .and_then(|id| visible.iter().position(|c| c.id == id))
            .unwrap_or(0);
        self.clamp_selection();
    }

    /// Текущий отфильтрованный/отсортированный список.
    fn visible(&self) -> Vec<ChatSummary> {
        filter_and_sort(&self.all, &self.query, self.sort)
    }

    /// Id выделенного чата (если список не пуст).
    pub fn selected_id(&self) -> Option<Uuid> {
        self.visible().get(self.selected).map(|c| c.id)
    }

    fn clamp_selection(&mut self) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// Устанавливает текст ошибки для отдельной области оверлея. Сбрасывается
    /// при следующем нажатии клавиши, так что не «висит» постоянно.
    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
        self.notice = None;
    }

    /// Устанавливает подтверждение операции (успех) для той же области. Сбрасывается
    /// при следующем нажатии клавиши.
    pub fn set_notice(&mut self, message: String) {
        self.notice = Some(message);
        self.error = None;
    }

    /// Обрабатывает нажатие клавиши, возвращая действие для исполнения.
    pub fn on_key(&mut self, key: KeyEvent) -> ChatListAction {
        if key.kind != KeyEventKind::Press {
            return ChatListAction::None;
        }
        // Любое нажатие убирает показанные ранее ошибку/подтверждение (не «висят»).
        self.error = None;
        self.notice = None;
        match &mut self.mode {
            Mode::Rename { .. } => self.on_key_rename(key),
            Mode::Search => self.on_key_search(key),
        }
    }

    fn on_key_search(&mut self, key: KeyEvent) -> ChatListAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl-шорткаты матчим по «физической» латинской клавише — работают при
        // любой раскладке (см. shared::keys). Любой другой Ctrl+символ глотаем,
        // чтобы он не попал в строку поиска.
        if ctrl && let KeyCode::Char(c) = key.code {
            return match keys::physical_char(c) {
                // Выход из приложения работает и из оверлея списка чатов.
                'c' => ChatListAction::Quit,
                'n' => ChatListAction::New,
                'd' => match self.selected_id() {
                    Some(id) => ChatListAction::Clone(id),
                    None => ChatListAction::None,
                },
                // Авто-название выделенного чата силами модели.
                'r' => match self.selected_id() {
                    Some(id) => ChatListAction::AutoRename(id),
                    None => ChatListAction::None,
                },
                _ => ChatListAction::None,
            };
        }
        match key.code {
            KeyCode::Esc => ChatListAction::Close,
            KeyCode::Enter => match self.selected_id() {
                Some(id) => ChatListAction::Switch(id),
                None => ChatListAction::None,
            },
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ChatListAction::None
            }
            KeyCode::Down => {
                let len = self.visible().len();
                if len > 0 {
                    self.selected = (self.selected + 1).min(len - 1);
                }
                ChatListAction::None
            }
            // Постраничное перемещение выделения: шаг на «страницу» (фиксированный,
            // т.к. высота списка известна лишь при рендере).
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(PAGE_STEP);
                ChatListAction::None
            }
            KeyCode::PageDown => {
                let len = self.visible().len();
                if len > 0 {
                    self.selected = (self.selected + PAGE_STEP).min(len - 1);
                }
                ChatListAction::None
            }
            // Прыжок к первому/последнему чату.
            KeyCode::Home => {
                self.selected = 0;
                ChatListAction::None
            }
            KeyCode::End => {
                self.selected = self.visible().len().saturating_sub(1);
                ChatListAction::None
            }
            KeyCode::Tab => {
                self.sort = self.sort.toggled();
                self.clamp_selection();
                ChatListAction::None
            }
            KeyCode::F(2) => {
                if let Some(chat) = self.visible().get(self.selected) {
                    let buffer: Vec<char> = chat.title.chars().collect();
                    let cursor = buffer.len();
                    self.mode = Mode::Rename {
                        id: chat.id,
                        buffer,
                        cursor,
                    };
                }
                ChatListAction::None
            }
            // Копирование всей переписки выделенного чата в буфер обмена.
            KeyCode::F(5) => match self.selected_id() {
                Some(id) => ChatListAction::Copy(id),
                None => ChatListAction::None,
            },
            KeyCode::Delete => match self.selected_id() {
                Some(id) => ChatListAction::Delete(id),
                None => ChatListAction::None,
            },
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
                ChatListAction::None
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.selected = 0;
                ChatListAction::None
            }
            _ => ChatListAction::None,
        }
    }

    fn on_key_rename(&mut self, key: KeyEvent) -> ChatListAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // Локально извлекаем поля, чтобы не держать заём `self.mode`.
        let Mode::Rename { id, buffer, cursor } = &mut self.mode else {
            return ChatListAction::None;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Search;
                ChatListAction::None
            }
            KeyCode::Enter => {
                let id = *id;
                let title = buffer.iter().collect::<String>();
                let action = match sanitize_title(&title) {
                    Some(title) => ChatListAction::Rename { id, title },
                    None => ChatListAction::None,
                };
                self.mode = Mode::Search;
                action
            }
            // Перемещение курсора (для правки имени в середине).
            KeyCode::Left => {
                *cursor = cursor.saturating_sub(1);
                ChatListAction::None
            }
            KeyCode::Right => {
                *cursor = (*cursor + 1).min(buffer.len());
                ChatListAction::None
            }
            KeyCode::Home => {
                *cursor = 0;
                ChatListAction::None
            }
            KeyCode::End => {
                *cursor = buffer.len();
                ChatListAction::None
            }
            KeyCode::Backspace => {
                if *cursor > 0 {
                    *cursor -= 1;
                    buffer.remove(*cursor);
                }
                ChatListAction::None
            }
            KeyCode::Delete => {
                if *cursor < buffer.len() {
                    buffer.remove(*cursor);
                }
                ChatListAction::None
            }
            // Вставка символа в позицию курсора (Ctrl/Alt-комбинации не печатаем).
            KeyCode::Char(c) if !ctrl && !alt => {
                buffer.insert(*cursor, c);
                *cursor += 1;
                ChatListAction::None
            }
            _ => ChatListAction::None,
        }
    }

    /// Рисует окно списка чатов на весь экран (`area`). `active` — текущий
    /// активный чат (метка). См. spec §11.2.
    pub fn render(&self, frame: &mut Frame, area: Rect, active: Option<Uuid>, palette: &Palette) {
        frame.render_widget(Clear, area);

        // Снизу — строка статуса с «клавишами» (как на экране чата): аккуратная
        // сетка хоткеев (число строк зависит от ширины). Считаем её заранее, чтобы
        // отвести под неё ровно нужную высоту.
        let status_lines = self.status_lines(palette, area.width as usize);
        let status_h = (status_lines.len() as u16).max(1);
        let [main_area, status_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(status_h)]).areas(area);

        // Панель со списком: скруглённая рамка, титул слева, число диалогов справа.
        let count = self.all.len();
        let block = palette.panel("▤ Чаты", true).title(
            Line::from(Span::styled(
                format!(" {count} диалог(ов) "),
                palette.muted_style(),
            ))
            .right_aligned(),
        );
        let inner = block.inner(main_area);
        frame.render_widget(block, main_area);

        // Внутри панели: строка поиска (в рамке, 3 ряда), список и — при наличии —
        // строка статуса операции (ошибка/подтверждение).
        let mut constraints = vec![Constraint::Length(3), Constraint::Min(1)];
        if self.error.is_some() || self.notice.is_some() {
            constraints.push(Constraint::Length(1));
        }
        let chunks = Layout::vertical(constraints).split(inner);
        let (search_area, list_area) = (chunks[0], chunks[1]);

        // --- строка поиска (в рамке, с клавишей `/` справа) ИЛИ поле
        // переименования (обычный ввод с настоящим курсором) ---
        match &self.mode {
            Mode::Search => self.render_search(frame, search_area, palette),
            Mode::Rename { buffer, cursor, .. } => {
                render_rename(frame, search_area, palette, buffer, *cursor)
            }
        }

        // --- список ---
        let visible = self.visible();
        let width = list_area.width as usize;
        let items: Vec<ListItem> = visible
            .iter()
            .enumerate()
            .map(|(i, c)| {
                ListItem::new(self.item_line(c, i == self.selected, active, palette, width))
            })
            .collect();
        // Выделение — мягкая подложка (как тинт в макете), а не инверсия всей строки;
        // зелёный рейл выделенной строки добавляется в `item_line`.
        let list = List::new(items).highlight_style(Style::new().bg(palette.keycap_bg));
        let mut list_state = ListState::default();
        if !visible.is_empty() {
            list_state.select(Some(self.selected.min(visible.len() - 1)));
        }
        frame.render_stateful_widget(list, list_area, &mut list_state);

        // --- область статуса операции: ошибка (красным) или подтверждение (успехом) ---
        if let Some(err) = &self.error {
            let line = Line::from(vec![
                Span::from("⚠ ").fg(palette.error),
                Span::from(err.clone()).fg(palette.error),
            ]);
            frame.render_widget(Paragraph::new(line), chunks[2]);
        } else if let Some(notice) = &self.notice {
            let line = Line::from(vec![
                Span::from("✓ ").fg(palette.success),
                Span::from(notice.clone()).fg(palette.success),
            ]);
            frame.render_widget(Paragraph::new(line), chunks[2]);
        }

        // --- строка статуса (хоткеи, как на экране чата) ---
        frame.render_widget(Paragraph::new(status_lines), status_area);
    }

    /// Рисует строку поиска в рамке; справа — «клавиша» `/` (приглашение фокуса).
    fn render_search(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(palette.border_style(true));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Поле и колонка под «клавишу» `/` справа.
        let [field, cap] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(3)]).areas(inner);

        let mut spans = vec![
            Span::styled("⌕ ", palette.muted_style()),
            Span::styled(self.query.clone(), Style::new().fg(palette.text)),
            Span::styled("▏", palette.muted_style()),
        ];
        if self.query.is_empty() {
            spans.push(Span::styled("Поиск по чатам…", palette.muted_style()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), field);
        frame.render_widget(Paragraph::new(Line::from(palette.keycap("/"))), cap);
    }

    /// Строка чата: цветной рейл выделенной строки, точка (зелёная у активного чата),
    /// заголовок слева и счётчик сообщений, прижатый к правому краю (`width` — ширина
    /// области списка). Длинный заголовок усекается с многоточием.
    fn item_line(
        &self,
        chat: &ChatSummary,
        is_selected: bool,
        active: Option<Uuid>,
        palette: &Palette,
        width: usize,
    ) -> Line<'static> {
        let is_active = active == Some(chat.id);
        // Рейл выделенной строки (2 колонки) + точка (2 колонки) = префикс.
        let rail = if is_selected {
            Span::styled("▌ ", Style::new().fg(palette.success))
        } else {
            Span::raw("  ")
        };
        let dot_color = if is_active {
            palette.success
        } else {
            palette.border
        };
        let title_style = if is_active {
            Style::new().fg(palette.text).bold()
        } else {
            Style::new().fg(palette.text)
        };

        let count = format!("{} сообщ.", chat.message_count);
        let count_w = display_width_str(&count);
        const PREFIX_W: usize = 4; // рейл (2) + точка «● » (2)
        const TRAIL: usize = 1; // правый отступ
        // Доступная ширина под заголовок (минимальный зазор 1 перед счётчиком).
        let max_title = width.saturating_sub(PREFIX_W + count_w + TRAIL + 1);
        let (title, title_w) = truncate_to_width(&chat.title, max_title);
        let gap = width
            .saturating_sub(PREFIX_W + title_w + count_w + TRAIL)
            .max(1);

        Line::from(vec![
            rail,
            Span::styled("● ", Style::new().fg(dot_color)),
            Span::styled(title, title_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(count, palette.muted_style()),
        ])
    }

    /// Строки статуса (хоткеи) внизу экрана — как на экране чата: «клавиши» на
    /// приглушённом фоне + приглушённые описания, выровненные в аккуратную сетку
    /// (столбцы совпадают по вертикали). Число строк подбирается под ширину `width`:
    /// берём максимум столбцов, влезающих в ширину (минимум строк). В режиме
    /// переименования — одна строка-подсказка.
    fn status_lines(&self, palette: &Palette, width: usize) -> Vec<Line<'static>> {
        if let Mode::Rename { .. } = self.mode {
            return vec![Line::from(vec![
                palette.keycap("Enter"),
                Span::styled(" сохранить   ", palette.muted_style()),
                palette.keycap("Esc"),
                Span::styled(" отмена", palette.muted_style()),
            ])];
        }

        // Пары «клавиша — описание — опасная ли» (порядок = чтение слева-направо,
        // сверху-вниз). `Tab` несёт текущий режим сортировки. Сетку выкладывает
        // общий хелпер `Palette::hotkey_grid` (тот же, что у статус-бара чата).
        let sort = self.sort.label();
        let sort_desc = format!("сортировка: {sort}");
        let items: [(&str, &str, bool); 11] = [
            ("↑↓ PgUp/Dn Home/End", "выбор", false),
            ("Enter", "открыть", false),
            ("F2", "переименовать", false),
            ("Ctrl+R", "авто-назв.", false),
            ("Ctrl+N", "новый", false),
            ("Ctrl+D", "копия", false),
            ("F5", "в буфер", false),
            ("Del", "удалить", true),
            ("Esc", "назад", false),
            ("Ctrl+C", "выход", false),
            ("Tab", sort_desc.as_str(), false),
        ];
        palette.hotkey_grid(&items, width)
    }
}

/// Рисует поле переименования в рамке: обычный однострочный ввод с **настоящим**
/// терминальным курсором (через `set_cursor_position`) и горизонтальным скроллом
/// (длинный заголовок «уезжает» влево, курсор всегда виден). Заменяет строку поиска,
/// пока идёт переименование. `buffer`/`cursor` — текст и позиция курсора (в символах).
fn render_rename(frame: &mut Frame, area: Rect, palette: &Palette, buffer: &[char], cursor: usize) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(palette.border_style(true))
        .title(Span::styled(" Переименование ", palette.muted_style()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let view_w = inner.width as usize;
    let cursor = cursor.min(buffer.len());
    let cursor_vw = wrap::display_width(&buffer[..cursor]);

    // Горизонтальный скролл: держим курсор в видимой области.
    let hscroll = cursor_vw.saturating_sub(view_w.saturating_sub(1));
    // Видимый срез [start, end) по колонкам [hscroll, hscroll + view_w).
    let mut start = 0;
    let mut w = 0;
    while start < buffer.len() && w < hscroll {
        w += wrap::char_width(buffer[start]);
        start += 1;
    }
    let mut end = start;
    let mut vis_w = 0;
    while end < buffer.len() {
        let cw = wrap::char_width(buffer[end]);
        if vis_w + cw > view_w {
            break;
        }
        vis_w += cw;
        end += 1;
    }
    let visible: String = buffer[start..end].iter().collect();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            visible,
            Style::new().fg(palette.text),
        ))),
        inner,
    );

    // Настоящий курсор (на ячейке, без «фантомного» столбца как у каретки).
    let cursor_x = inner.x + (cursor_vw - hscroll) as u16;
    let x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
    frame.set_cursor_position((x, inner.y));
}

/// Видимая ширина строки в колонках терминала.
fn display_width_str(s: &str) -> usize {
    wrap::display_width(&s.chars().collect::<Vec<_>>())
}

/// Усекает строку до ширины `max` колонок, добавляя «…» при усечении. Возвращает
/// усечённую строку и её фактическую ширину. При `max == 0` — пустая строка.
fn truncate_to_width(s: &str, max: usize) -> (String, usize) {
    let chars: Vec<char> = s.chars().collect();
    let full = wrap::display_width(&chars);
    if full <= max {
        return (s.to_string(), full);
    }
    if max == 0 {
        return (String::new(), 0);
    }
    // Оставляем место под «…» (1 колонка).
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut w = 0;
    for c in chars {
        let cw = wrap::char_width(c);
        if w + cw > budget {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push('…');
    (out, w + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn chat(title: &str) -> ChatSummary {
        ChatSummary {
            id: Uuid::new_v4(),
            title: title.to_string(),
            created_at: Utc::now(),
            modified_at: Utc::now(),
            message_count: 0,
        }
    }

    #[test]
    fn typing_filters_and_enter_switches() {
        let chats = vec![chat("Альфа"), chat("Бета")];
        let beta_id = chats[1].id;
        let mut s = ChatListState::new(chats, None);

        for c in "Бет".chars() {
            assert_eq!(s.on_key(key(KeyCode::Char(c))), ChatListAction::None);
        }
        // остался один чат — он же выделен
        assert_eq!(s.selected_id(), Some(beta_id));
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Switch(beta_id)
        );
    }

    #[test]
    fn esc_closes() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        assert_eq!(s.on_key(key(KeyCode::Esc)), ChatListAction::Close);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('c'))), ChatListAction::Quit);
        // И при кириллической раскладке (физ. C = Ctrl+с).
        assert_eq!(s.on_key(ctrl(KeyCode::Char('с'))), ChatListAction::Quit);
    }

    #[test]
    fn ctrl_n_and_ctrl_d_and_delete() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('n'))), ChatListAction::New);
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('d'))),
            ChatListAction::Clone(id)
        );
        assert_eq!(s.on_key(key(KeyCode::Delete)), ChatListAction::Delete(id));
    }

    #[test]
    fn ctrl_shortcuts_work_under_cyrillic_layout() {
        // Русская раскладка: Ctrl+т (физ. N) — новый, Ctrl+в (физ. D) — копия.
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('т'))), ChatListAction::New);
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('в'))),
            ChatListAction::Clone(id)
        );
        // Текст поиска кириллицей по-прежнему набирается (без Ctrl).
        s.on_key(key(KeyCode::Char('я')));
        assert_eq!(s.query, "я");
    }

    #[test]
    fn f5_requests_copy_of_selected() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.on_key(key(KeyCode::F(5))), ChatListAction::Copy(id));
    }

    #[test]
    fn notice_is_set_and_cleared_on_next_key() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        s.set_notice("скопировано".into());
        assert_eq!(s.notice.as_deref(), Some("скопировано"));
        // Установка ошибки гасит подтверждение и наоборот (взаимоисключимы).
        s.set_error("боль".into());
        assert!(s.notice.is_none());
        s.set_notice("ок".into());
        assert!(s.error.is_none());
        // Любое нажатие убирает подтверждение.
        s.on_key(key(KeyCode::Down));
        assert!(s.notice.is_none());
    }

    #[test]
    fn ctrl_r_requests_auto_rename_of_selected() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('r'))),
            ChatListAction::AutoRename(id)
        );
        // И при кириллической раскладке (физ. R = Ctrl+к).
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('к'))),
            ChatListAction::AutoRename(id)
        );
    }

    #[test]
    fn f2_enters_rename_and_enter_commits() {
        let chats = vec![chat("Старое")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);

        assert_eq!(s.on_key(key(KeyCode::F(2))), ChatListAction::None);
        // очищаем буфер и печатаем новое имя
        for _ in 0.."Старое".chars().count() {
            s.on_key(key(KeyCode::Backspace));
        }
        for c in "Новое".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "Новое".into()
            }
        );
    }

    #[test]
    fn rename_cursor_moves_and_edits_in_middle() {
        // Курсор в режиме переименования двигается стрелками/Home/End, правка идёт
        // в позицию курсора (а не только в конец).
        let chats = vec![chat("abc")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2))); // буфер "abc", курсор в конце (3)
        s.on_key(key(KeyCode::Home)); // курсор → 0
        s.on_key(key(KeyCode::Char('X'))); // вставка в начало → "Xabc", курсор 1
        s.on_key(key(KeyCode::End)); // курсор → 4 (конец)
        s.on_key(key(KeyCode::Char('Y'))); // вставка в конец → "XabcY", курсор 5
        s.on_key(key(KeyCode::Home)); // курсор → 0
        s.on_key(key(KeyCode::Right)); // курсор 1
        s.on_key(key(KeyCode::Right)); // курсор 2 (перед 'b')
        s.on_key(key(KeyCode::Delete)); // удалить 'b' в середине → "XacY"
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "XacY".into()
            }
        );
    }

    #[test]
    fn rename_esc_cancels_without_action() {
        let chats = vec![chat("Старое")];
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2)));
        assert_eq!(s.on_key(key(KeyCode::Esc)), ChatListAction::None);
        // вернулись в режим поиска: печать снова фильтрует
        s.on_key(key(KeyCode::Char('x')));
        assert_eq!(s.selected_id(), None); // 'x' не совпал ни с чем
    }

    #[test]
    fn page_up_down_move_selection_by_page() {
        // 25 чатов; PageDown сдвигает на PAGE_STEP, не выходя за конец, PageUp — назад.
        let chats: Vec<ChatSummary> = (0..25).map(|i| chat(&format!("чат {i}"))).collect();
        let mut s = ChatListState::new(chats, None);
        assert_eq!(s.selected, 0);
        s.on_key(key(KeyCode::PageDown));
        assert_eq!(s.selected, PAGE_STEP);
        s.on_key(key(KeyCode::PageDown));
        assert_eq!(s.selected, 2 * PAGE_STEP);
        // Третий PageDown упирается в последний элемент (24), а не уезжает за край.
        s.on_key(key(KeyCode::PageDown));
        assert_eq!(s.selected, 24);
        s.on_key(key(KeyCode::PageUp));
        assert_eq!(s.selected, 24 - PAGE_STEP);
        // PageUp из верхней части насыщается на 0 (без переполнения).
        s.on_key(key(KeyCode::PageUp));
        s.on_key(key(KeyCode::PageUp));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn home_end_jump_to_first_and_last() {
        let chats: Vec<ChatSummary> = (0..25).map(|i| chat(&format!("чат {i}"))).collect();
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::End));
        assert_eq!(s.selected, 24);
        s.on_key(key(KeyCode::Home));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn home_end_on_empty_list_is_noop() {
        let mut s = ChatListState::new(vec![], None);
        assert_eq!(s.on_key(key(KeyCode::End)), ChatListAction::None);
        assert_eq!(s.selected, 0);
        assert_eq!(s.on_key(key(KeyCode::Home)), ChatListAction::None);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn page_down_on_empty_list_is_noop() {
        let mut s = ChatListState::new(vec![], None);
        assert_eq!(s.on_key(key(KeyCode::PageDown)), ChatListAction::None);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn tab_toggles_sort_indicator() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        let before = s.sort;
        s.on_key(key(KeyCode::Tab));
        assert_ne!(s.sort, before);
    }

    #[test]
    fn error_is_set_and_cleared_on_next_key() {
        let mut s = ChatListState::new(vec![chat("A")], None);
        s.set_error("боль".into());
        assert_eq!(s.error.as_deref(), Some("боль"));
        // Любое нажатие убирает ошибку.
        s.on_key(key(KeyCode::Down));
        assert!(s.error.is_none());
    }

    #[test]
    fn render_with_error_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut state = ChatListState::new(vec![chat("Альфа")], None);
        state.set_error("Недостаточно сообщений для авто-названия".into());
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| state.render(f, f.area(), None, &Palette::default()))
                .unwrap();
        }
    }

    #[test]
    fn render_does_not_panic_on_small_and_normal_areas() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let state = ChatListState::new(vec![chat("Альфа"), chat("Бета")], None);
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| state.render(f, f.area(), None, &Palette::default()))
                .unwrap();
        }
    }

    #[test]
    fn set_chats_keeps_selection_on_same_chat() {
        let chats = vec![chat("A"), chat("B"), chat("C")];
        let b_id = chats[1].id;
        let mut s = ChatListState::new(chats.clone(), None);
        s.on_key(key(KeyCode::Down)); // выделить B (индекс 1 в порядке Modified ~ исходный)
        let sel = s.selected_id();
        // обновляем список тем же набором — выделение сохраняется на том же чате
        s.set_chats(chats);
        assert_eq!(s.selected_id(), sel);
        let _ = b_id;
    }
}
