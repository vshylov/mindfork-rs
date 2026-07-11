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
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::features::chat_search_sort::{SortMode, filter_and_sort};
use crate::features::rename_chat::sanitize_title;
use crate::features::spellcheck::SpellChecker;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap;
use crate::widgets::input_box::InputBox;

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
    /// Переименование выбранного чата по месту. Текст и курсор ведёт однострочный
    /// [`InputBox`] (`set_single_line`) — отсюда «бесплатно» доступны спелл-чек,
    /// пословная навигация/удаление (`Ctrl+←/→`, `Ctrl+Backspace/Delete`),
    /// `Ctrl+Home/End`, очистка/возврат (`Ctrl+K`) и вставка из буфера. `InputBox`
    /// крупный — боксируем (clippy::large_enum_variant). `spell_dirty` отмечает,
    /// что подсветку ошибок нужно пересчитать (см. [`Self::recheck_rename_spelling`]).
    Rename {
        id: Uuid,
        input: Box<InputBox>,
        spell_dirty: bool,
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
    /// по возможности на том же чате. Если выделенный чат исчез (напр. был удалён),
    /// выделение остаётся на **той же позиции** (следующий по списку чат, а при
    /// удалении последнего — новый последний), а не прыгает на первый элемент —
    /// как принято в списках.
    pub fn set_chats(&mut self, chats: Vec<ChatSummary>) {
        let current = self.selected_id();
        let prev_index = self.selected;
        self.all = chats;
        let visible = self.visible();
        self.selected = current
            .and_then(|id| visible.iter().position(|c| c.id == id))
            .unwrap_or(prev_index);
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
                    // Однострочный `InputBox` с текущим названием (курсор в конце).
                    // `set_single_line` — до `set_text` (инвариант одной строки).
                    let mut input = InputBox::new();
                    input.set_single_line(true);
                    input.set_text(&chat.title);
                    self.mode = Mode::Rename {
                        id: chat.id,
                        input: Box::new(input),
                        spell_dirty: true,
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
        let Mode::Rename {
            id,
            input,
            spell_dirty,
        } = &mut self.mode
        else {
            return ChatListAction::None;
        };
        // Очистка/возврат всего текста (`Ctrl+K`) — раскладко-независимо, как в чате.
        // Прочие Ctrl-комбинации (пословная навигация `Ctrl+←/→`, удаление слова
        // `Ctrl+Backspace/Delete`, `Ctrl+Home/End`) обрабатывает сам `InputBox` ниже;
        // незнакомые он глотает (Ctrl+символ в поле не печатается).
        if ctrl
            && let KeyCode::Char(c) = key.code
            && keys::physical_char(c) == 'k'
        {
            input.clear_or_restore();
            *spell_dirty = true;
            return ChatListAction::None;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Search;
                ChatListAction::None
            }
            KeyCode::Enter => {
                let id = *id;
                let title = input.text();
                let action = match sanitize_title(&title) {
                    Some(title) => ChatListAction::Rename { id, title },
                    None => ChatListAction::None,
                };
                self.mode = Mode::Search;
                action
            }
            // Всё прочее (печать, навигация по словам/символам, удаление, `Home/End`)
            // ведёт сам `InputBox`; на реальной правке помечаем подсветку ошибок на
            // пересчёт (движение курсора его не требует). См. [`KeyOutcome`].
            _ => {
                if input.on_key(key).edited() {
                    *spell_dirty = true;
                }
                ChatListAction::None
            }
        }
    }

    /// Вставляет текст из буфера обмена в поле переименования (если оно открыто).
    /// Переводы строк схлопываются в пробел (`InputBox` однострочный). Вне режима
    /// переименования — no-op (в строке поиска вставка не нужна).
    pub fn handle_paste(&mut self, text: &str) {
        if let Mode::Rename {
            input, spell_dirty, ..
        } = &mut self.mode
        {
            input.insert_str(text);
            *spell_dirty = true;
        }
    }

    /// Пересчитывает подсветку ошибок орфографии в поле переименования, если оно
    /// открыто и помечено «грязным» (после правки/вставки). Возвращает `true`, если
    /// подсветка была обновлена (нужна перерисовка). Чекер не владеется виджетом —
    /// его одалживает `app` (владелец живёт в экране чата). См. spec §11.5.
    pub fn recheck_rename_spelling(&mut self, spell: &SpellChecker) -> bool {
        let Mode::Rename {
            input, spell_dirty, ..
        } = &mut self.mode
        else {
            return false;
        };
        if !*spell_dirty {
            return false;
        }
        let ranges = vec![spell.misspellings(&input.text())];
        input.set_misspelled(ranges);
        *spell_dirty = false;
        true
    }

    /// Рисует окно списка чатов на весь экран (`area`). `active` — текущий
    /// активный чат (метка). `&mut self` — поле переименования рисует [`InputBox`]
    /// (ему нужен `&mut` для скролла/курсора). См. spec §11.2.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        active: Option<Uuid>,
        palette: &Palette,
    ) {
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
        let title = format!("{} Чаты", palette.glyphs().chats_icon);
        let block = palette.panel(title, true).title(
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
        // переименования (однострочный `InputBox`: спелл-чек, пословная навигация,
        // настоящий курсор, горизонтальный скролл) ---
        if let Mode::Rename { input, .. } = &mut self.mode {
            input.render(frame, search_area, "Переименование", true, palette, false);
        } else {
            self.render_search(frame, search_area, palette);
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

        // Скроллбар на правой рамке панели «Чаты» — когда чатов больше видимой
        // высоты списка. Бар занимает только ряды списка (строка поиска и статус
        // не затрагиваются); позиция — фактический offset списка после рендера.
        render_scrollbar(
            frame,
            Rect {
                x: main_area.x,
                y: list_area.y,
                width: main_area.width,
                height: list_area.height,
            },
            visible.len(),
            list_area.height as usize,
            list_state.offset(),
            true, // рамка панели — в фокусном цвете (panel(_, true))
            palette,
        );

        // --- область статуса операции: ошибка (красным) или подтверждение (успехом) ---
        if let Some(err) = &self.error {
            let line = Line::from(vec![
                Span::from(format!("{} ", palette.glyphs().warn)).fg(palette.error),
                Span::from(err.clone()).fg(palette.error),
            ]);
            frame.render_widget(Paragraph::new(line), chunks[2]);
        } else if let Some(notice) = &self.notice {
            let line = Line::from(vec![
                Span::from(format!("{} ", palette.glyphs().ok)).fg(palette.success),
                Span::from(notice.clone()).fg(palette.success),
            ]);
            frame.render_widget(Paragraph::new(line), chunks[2]);
        }

        // --- строка статуса (хоткеи, как на экране чата) ---
        frame.render_widget(Paragraph::new(status_lines), status_area);
    }

    /// Рисует строку поиска в рамке; справа — «клавиша» `/` (приглашение фокуса).
    fn render_search(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let glyphs = palette.glyphs();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(glyphs.border)
            .border_style(palette.border_style(true));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Поле и колонка под «клавишу» `/` справа.
        let [field, cap] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(3)]).areas(inner);

        let mut spans = vec![
            Span::styled(format!("{} ", glyphs.search), palette.muted_style()),
            Span::styled(self.query.clone(), Style::new().fg(palette.text)),
            Span::styled(glyphs.caret, palette.muted_style()),
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
    for i in 0..chars.len() {
        let cw = wrap::width_at(&chars, i);
        if w + cw > budget {
            break;
        }
        w += cw;
        out.push(chars[i]);
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
    fn rename_supports_input_box_word_navigation() {
        // Поле переименования — однострочный `InputBox`, поэтому `Ctrl+Backspace`
        // удаляет слово целиком (раньше посимвольный буфер этого не умел).
        let chats = vec![chat("один два")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2))); // "один два", курсор в конце
        s.on_key(ctrl(KeyCode::Backspace)); // удалить слово "два" → "один "
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "один".into() // хвостовой пробел снимает sanitize_title
            }
        );
    }

    #[test]
    fn rename_clear_and_restore_with_ctrl_k() {
        // `Ctrl+K` чистит поле, повторное нажатие возвращает текст (как в чате).
        let chats = vec![chat("Старое имя")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2)));
        s.on_key(ctrl(KeyCode::Char('k'))); // удалить весь текст
        s.on_key(ctrl(KeyCode::Char('k'))); // вернуть удалённое
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "Старое имя".into()
            }
        );
    }

    #[test]
    fn rename_spellcheck_underlines_misspelled_word() {
        use crate::features::spellcheck::SpellChecker;
        use std::collections::HashSet;
        let dict = spellbook::Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        let spell = SpellChecker::new(vec![dict], HashSet::new(), None);

        let mut s = ChatListState::new(vec![chat("helo")], None);
        s.on_key(key(KeyCode::F(2))); // входим в переименование, текст "helo"
        // Пересчёт орфографии помечает "helo" как ошибку (диапазон непуст).
        assert!(s.recheck_rename_spelling(&spell));
        match &s.mode {
            Mode::Rename {
                input, spell_dirty, ..
            } => {
                assert!(!*spell_dirty, "флаг сбрасывается после пересчёта");
                assert!(
                    !input.misspelled_is_empty(),
                    "ошибка должна быть подчёркнута"
                );
            }
            _ => panic!("ожидался режим переименования"),
        }
        // Вне режима переименования пересчёт — no-op.
        s.on_key(key(KeyCode::Esc));
        assert!(!s.recheck_rename_spelling(&spell));
    }

    #[test]
    fn rename_paste_inserts_into_field() {
        let chats = vec![chat("a")];
        let id = chats[0].id;
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::F(2))); // "a", курсор в конце
        s.handle_paste("bc"); // вставка из буфера обмена
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            ChatListAction::Rename {
                id,
                title: "abc".into()
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
    fn scrollbar_appears_only_when_list_overflows() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // Бегунок «█» на правой рамке панели — только когда чатов больше высоты.
        let has_thumb = |term: &Terminal<TestBackend>| {
            let buf = term.backend().buffer();
            let x = buf.area.right() - 1; // колонка рамки панели «Чаты»
            (buf.area.top()..buf.area.bottom()).any(|y| buf[(x, y)].symbol() == "█")
        };
        // Ширина 72 — сетка хоткеев внизу в 2 колонки (не съедает высоту списка).
        let mut term = Terminal::new(TestBackend::new(72, 24)).unwrap();
        let mut short = ChatListState::new(vec![chat("A"), chat("B")], None);
        term.draw(|f| short.render(f, f.area(), None, &Palette::default()))
            .unwrap();
        assert!(!has_thumb(&term), "короткий список — без бегунка");
        let chats: Vec<ChatSummary> = (0..40).map(|i| chat(&format!("Чат {i}"))).collect();
        let mut long = ChatListState::new(chats, None);
        term.draw(|f| long.render(f, f.area(), None, &Palette::default()))
            .unwrap();
        assert!(has_thumb(&term), "длинный список — с бегунком");
    }

    #[test]
    fn render_does_not_panic_on_small_and_normal_areas() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut state = ChatListState::new(vec![chat("Альфа"), chat("Бета")], None);
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

    #[test]
    fn deleting_selected_keeps_position_not_first() {
        // Удаление выделенного чата (переэмит списка без него) оставляет выделение
        // на той же позиции — под ним оказывается следующий по списку чат, а не
        // происходит прыжок на первый элемент.
        let chats = vec![chat("A"), chat("B"), chat("C")];
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::Down)); // выделение на позиции 1
        assert_eq!(s.selected, 1);
        // Порядок берём из фактического `visible` (сортировка может отличаться от
        // порядка ввода) — так тест не зависит от близких таймстампов.
        let victim_id = s.selected_id().unwrap();
        let next_id = s.visible()[2].id; // станет под позицией 1 после удаления

        let remaining: Vec<ChatSummary> = s
            .visible()
            .into_iter()
            .filter(|c| c.id != victim_id)
            .collect();
        s.set_chats(remaining);

        assert_eq!(s.selected, 1, "позиция выделения сохраняется");
        assert_eq!(
            s.selected_id(),
            Some(next_id),
            "под выделением — бывший следующий чат, а не первый"
        );
    }

    #[test]
    fn deleting_last_selected_clamps_to_new_last() {
        // Удаление выделенного последнего чата уводит выделение на новый последний
        // (а не на первый).
        let chats = vec![chat("A"), chat("B"), chat("C")];
        let mut s = ChatListState::new(chats, None);
        s.on_key(key(KeyCode::End)); // выделение на последнем (позиция 2)
        assert_eq!(s.selected, 2);
        let victim_id = s.selected_id().unwrap();
        let new_last_id = s.visible()[1].id; // станет новым последним

        let remaining: Vec<ChatSummary> = s
            .visible()
            .into_iter()
            .filter(|c| c.id != victim_id)
            .collect();
        s.set_chats(remaining);

        assert_eq!(s.selected, 1, "выделение прижато к новому последнему");
        assert_eq!(s.selected_id(), Some(new_last_id));
    }
}
