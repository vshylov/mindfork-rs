//! Экран списка чатов (FSD "page"): полноэкранный список с поиском, сортировкой,
//! переименованием по месту, созданием/клонированием/копированием/удалением.
//! Открывается из чата по `Esc`, закрывается `Esc`. См. spec §11.2.
//!
//! Тонкая обёртка над виджетом [`ChatListState`]: хранит контекст отрисовки
//! (активный чат — для метки, палитру темы) и переводит [`ChatListAction`] виджета
//! в [`ChatListIntent`] — намерение, которое `app` транслирует в `AppCommand` или
//! в управление экранами. Экран про `app`/каналы не знает (FSD, зависимости вниз) —
//! как `ChatScreen`/`SettingsScreen`.

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::features::spellcheck::SpellChecker;
use crate::shared::theme::Palette;
use crate::widgets::chat_list::{ChatListAction, ChatListState};

/// Намерение экрана списка чатов (транслируется `app`). Параллель к
/// [`ChatIntent`](crate::screens::chat::ChatIntent)/`SettingsIntent`.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatListIntent {
    /// Закрыть список, вернуться к чату (`Esc`).
    Close,
    /// Выйти из приложения (`Ctrl+C`).
    Quit,
    /// Сделать чат активным и вернуться к чату (`Enter`).
    Switch(Uuid),
    /// Создать новый чат (выбор профиля делегируется экрану чата) (`Ctrl+N`).
    NewChat,
    /// Клонировать чат и вернуться к чату (`Ctrl+D`).
    Clone(Uuid),
    /// Скопировать переписку чата в буфер обмена (список остаётся открытым) (`F5`).
    Copy(Uuid),
    /// Мягко удалить чат (список остаётся открытым) (`Del`).
    Delete(Uuid),
    /// Переименовать чат (список остаётся открытым) (`F2`).
    Rename { id: Uuid, title: String },
    /// Авто-название чата силами модели (список остаётся открытым) (`Ctrl+R`).
    AutoRename(Uuid),
}

/// Экран списка чатов: состояние виджета + контекст отрисовки.
pub struct ChatListScreen {
    state: ChatListState,
    /// Активный чат (метка `●` в списке). Обновляется при `ChatActivated`.
    active: Option<Uuid>,
    /// Палитра темы для отрисовки (обновляется при `Settings`).
    palette: Palette,
    /// Ждём активации только что созданного чата (`Ctrl+N`): список остаётся на
    /// экране до прихода его `ChatActivated`, чтобы не мигнуть прежним чатом
    /// перед новым. Гасится в `take_pending_new_chat`. См. spec §11.2.
    pending_new_chat: bool,
}

impl ChatListScreen {
    /// Открывает экран со снимком списка; выделение — на активном чате.
    pub fn new(chats: Vec<ChatSummary>, active: Option<Uuid>, palette: Palette) -> Self {
        Self {
            state: ChatListState::new(chats, active),
            active,
            palette,
            pending_new_chat: false,
        }
    }

    /// Помечает: создан новый чат, ждём его `ChatActivated`, чтобы переключиться
    /// на него атомарно (не показав прежний чат). См. `dispatch_chat_list`.
    pub fn set_pending_new_chat(&mut self) {
        self.pending_new_chat = true;
    }

    /// Забирает флаг ожидания нового чата (сбрасывая его). `true` — пришедшая
    /// активация относится к только что созданному чату, список пора закрыть.
    pub fn take_pending_new_chat(&mut self) -> bool {
        std::mem::take(&mut self.pending_new_chat)
    }

    /// Обновляет снимок списка (после изменения набора чатов) — событие
    /// `AppEvent::ChatList`. Сохраняет выделение на том же чате по возможности.
    pub fn set_chats(&mut self, chats: Vec<ChatSummary>) {
        self.state.set_chats(chats);
    }

    /// Обновляет метку активного чата (событие `AppEvent::ChatActivated`, напр.
    /// после удаления активного чата при открытом списке).
    pub fn set_active(&mut self, active: Option<Uuid>) {
        self.active = active;
    }

    /// Обновляет палитру темы (событие `AppEvent::Settings`).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Показывает ошибку операции списка в его области статуса (гаснет по нажатию).
    pub fn set_error(&mut self, message: String) {
        self.state.set_error(message);
    }

    /// Показывает подтверждение операции (успех) в его области статуса.
    pub fn set_notice(&mut self, message: String) {
        self.state.set_notice(message);
    }

    /// Обрабатывает нажатие клавиши, возвращая намерение для `app` (или `None`,
    /// если клавиша обработана внутри: навигация, ввод поиска/переименования).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChatListIntent> {
        match self.state.on_key(key) {
            ChatListAction::None => None,
            ChatListAction::Close => Some(ChatListIntent::Close),
            ChatListAction::Quit => Some(ChatListIntent::Quit),
            ChatListAction::Switch(id) => Some(ChatListIntent::Switch(id)),
            ChatListAction::New => Some(ChatListIntent::NewChat),
            ChatListAction::Clone(id) => Some(ChatListIntent::Clone(id)),
            ChatListAction::Copy(id) => Some(ChatListIntent::Copy(id)),
            ChatListAction::Delete(id) => Some(ChatListIntent::Delete(id)),
            ChatListAction::Rename { id, title } => Some(ChatListIntent::Rename { id, title }),
            ChatListAction::AutoRename(id) => Some(ChatListIntent::AutoRename(id)),
        }
    }

    /// Вставляет текст из буфера обмена в поле переименования (если открыто).
    /// Вне режима переименования — no-op. См. spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        self.state.handle_paste(text);
    }

    /// Перепроверяет орфографию в поле переименования (если открыто и изменилось).
    /// Возвращает `true`, если подсветка обновлена (нужна перерисовка). Чекер
    /// одалживается из экрана чата — владельца (`app` сводит это в петле).
    pub fn recheck_spelling(&mut self, spell: &SpellChecker) -> bool {
        self.state.recheck_rename_spelling(spell)
    }

    /// Рисует список во весь экран. `&mut self` — поле переименования рисует
    /// [`InputBox`](crate::widgets::input_box::InputBox), которому нужен `&mut`.
    pub fn render(&mut self, frame: &mut Frame) {
        self.state
            .render(frame, frame.area(), self.active, &self.palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn chat(title: &str) -> ChatSummary {
        ChatSummary {
            id: Uuid::new_v4(),
            title: title.to_string(),
            created_at: Utc::now(),
            modified_at: Utc::now(),
            message_count: 0,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn esc_maps_to_close() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default());
        assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(ChatListIntent::Close));
    }

    #[test]
    fn ctrl_c_maps_to_quit() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default());
        assert_eq!(
            s.handle_key(ctrl(KeyCode::Char('c'))),
            Some(ChatListIntent::Quit)
        );
    }

    #[test]
    fn enter_maps_to_switch_of_selected() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListScreen::new(chats, Some(id), Palette::default());
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(ChatListIntent::Switch(id))
        );
    }

    #[test]
    fn ctrl_n_maps_to_new_chat() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default());
        assert_eq!(
            s.handle_key(ctrl(KeyCode::Char('n'))),
            Some(ChatListIntent::NewChat)
        );
    }

    #[test]
    fn typing_filters_and_returns_none() {
        let mut s =
            ChatListScreen::new(vec![chat("Альфа"), chat("Бета")], None, Palette::default());
        // Печать символа в строку поиска — обработана внутри (нет намерения).
        assert_eq!(s.handle_key(key(KeyCode::Char('Б'))), None);
    }

    #[test]
    fn pending_new_chat_flag_set_and_taken_once() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default());
        assert!(!s.take_pending_new_chat());
        s.set_pending_new_chat();
        // Забирается ровно один раз (сбрасывается) — вторая активация закрыть не должна.
        assert!(s.take_pending_new_chat());
        assert!(!s.take_pending_new_chat());
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = ChatListScreen::new(vec![chat("Альфа")], None, Palette::default());
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }
}
