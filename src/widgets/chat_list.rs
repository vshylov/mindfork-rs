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
use crate::shared::keys;
use crate::shared::theme::Palette;

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
    /// Переименование выбранного чата по месту.
    Rename { id: Uuid, buffer: String },
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
            KeyCode::Tab => {
                self.sort = self.sort.toggled();
                self.clamp_selection();
                ChatListAction::None
            }
            KeyCode::F(2) => {
                if let Some(chat) = self.visible().get(self.selected) {
                    self.mode = Mode::Rename {
                        id: chat.id,
                        buffer: chat.title.clone(),
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
        // Локально извлекаем поля, чтобы не держать заём `self.mode`.
        let Mode::Rename { id, buffer } = &mut self.mode else {
            return ChatListAction::None;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Search;
                ChatListAction::None
            }
            KeyCode::Enter => {
                let id = *id;
                let action = match sanitize_title(buffer) {
                    Some(title) => ChatListAction::Rename { id, title },
                    None => ChatListAction::None,
                };
                self.mode = Mode::Search;
                action
            }
            KeyCode::Backspace => {
                buffer.pop();
                ChatListAction::None
            }
            KeyCode::Char(c) => {
                buffer.push(c);
                ChatListAction::None
            }
            _ => ChatListAction::None,
        }
    }

    /// Рисует окно списка чатов на весь экран (`area`). `active` — текущий
    /// активный чат (метка). См. spec §11.2.
    pub fn render(&self, frame: &mut Frame, area: Rect, active: Option<Uuid>, palette: &Palette) {
        let popup = area;
        frame.render_widget(Clear, popup);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Чаты ")
            .title_bottom(self.help_line());
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        // Под список отдаём всё, кроме строки поиска и (если есть) строки статуса
        // (ошибка/подтверждение).
        let mut constraints = vec![Constraint::Length(1), Constraint::Min(1)];
        if self.error.is_some() || self.notice.is_some() {
            constraints.push(Constraint::Length(1));
        }
        let chunks = Layout::vertical(constraints).split(inner);
        let (search_area, list_area) = (chunks[0], chunks[1]);

        // --- строка поиска / переименования ---
        frame.render_widget(self.header_line(palette), search_area);

        // --- список ---
        let visible = self.visible();
        let items: Vec<ListItem> = visible
            .iter()
            .map(|c| ListItem::new(self.item_line(c, active, palette)))
            .collect();
        let list = List::new(items).highlight_style(Style::new().reversed());
        let mut list_state = ListState::default();
        if !visible.is_empty() {
            list_state.select(Some(self.selected.min(visible.len() - 1)));
        }
        frame.render_stateful_widget(list, list_area, &mut list_state);

        // --- область статуса: ошибка (красным) или подтверждение (успехом) ---
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
    }

    fn header_line(&self, palette: &Palette) -> Paragraph<'static> {
        match &self.mode {
            Mode::Search => Paragraph::new(Line::from(vec![
                Span::from("🔎 ").dim(),
                Span::from(self.query.clone()),
                Span::from("▏").dim(),
            ])),
            Mode::Rename { buffer, .. } => Paragraph::new(Line::from(vec![
                Span::from("✎ ").fg(palette.warning),
                Span::from(buffer.clone()),
                Span::from("▏").dim(),
            ])),
        }
    }

    fn item_line(
        &self,
        chat: &ChatSummary,
        active: Option<Uuid>,
        palette: &Palette,
    ) -> Line<'static> {
        let marker = if active == Some(chat.id) {
            "● "
        } else {
            "  "
        };
        Line::from(vec![
            Span::from(marker).fg(palette.success),
            Span::from(chat.title.clone()),
            Span::from(format!(" · {} сообщ.", chat.message_count)).dim(),
        ])
    }

    fn help_line(&self) -> Line<'static> {
        match self.mode {
            Mode::Search => Line::from(format!(
                " ↑↓ выбор · Enter открыть · Esc назад · F2 ⮞ · Ctrl+R авто-назв. · Ctrl+N новый · Ctrl+D копия · F5 в буфер · Del удалить · Ctrl+C выход · Tab сорт.: {} ",
                self.sort.label()
            ))
            .dim(),
            Mode::Rename { .. } => {
                Line::from(" переименование: Enter — ок · Esc — отмена ").dim()
            }
        }
    }
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
