//! Экран чата (FSD "page"): компоновка виджетов, роутинг фокуса, горячие
//! клавиши и read-only-проекция состояния. См. spec §11.1, §11.7.
//!
//! Экран НЕ знает про `app`/каналы (FSD: зависимости только вниз). На нажатия
//! он возвращает [`ChatIntent`] — намерение, которое `app` транслирует в
//! `AppCommand`. События оркестратора `app` применяет, вызывая мутаторы экрана
//! (`set_*`, `push_*`, …) — экрану не нужен тип `AppEvent`.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::entities::message::Message;
use crate::shared::api::FinishReason;
use crate::shared::server::ServerStatus;
use crate::widgets::chat_list::{ChatListAction, ChatListState};
use crate::widgets::input_box::InputBox;
use crate::widgets::message_feed::{FeedMessage, FeedRole, MessageFeed};
use crate::widgets::status_bar;

/// Высота прокрутки ленты на одно нажатие PageUp/PageDown (строк).
const PAGE_SCROLL: usize = 8;

/// Намерение пользователя, которое исполняет `app` (транслирует в `AppCommand`).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatIntent {
    Quit,
    Send(String),
    Cancel,
    NewChat,
    SwitchChat(Uuid),
    CloneChat(Uuid),
    DeleteChat(Uuid),
    RenameChat { id: Uuid, title: String },
}

/// Экран чата: всё состояние UI и его отрисовка.
pub struct ChatScreen {
    feed: Vec<FeedMessage>,
    feed_view: MessageFeed,
    active_chat: Option<Uuid>,
    title: String,
    chats: Vec<ChatSummary>,
    overlay: Option<ChatListState>,
    input: InputBox,
    status: ServerStatus,
    current_gen: Option<Uuid>,
    generating: bool,
}

impl Default for ChatScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatScreen {
    pub fn new() -> Self {
        Self {
            feed: Vec::new(),
            feed_view: MessageFeed::new(),
            active_chat: None,
            title: String::new(),
            chats: Vec::new(),
            overlay: None,
            input: InputBox::new(),
            status: ServerStatus::Connecting,
            current_gen: None,
            generating: false,
        }
    }

    // ---------- проекция событий оркестратора (вызывается слоем `app`) ----------

    pub fn set_server_status(&mut self, status: ServerStatus) {
        self.status = status;
    }

    pub fn set_chat_list(&mut self, chats: Vec<ChatSummary>) {
        // Если оверлей открыт — синхронизируем его снимок (после
        // переименования/удаления/клонирования).
        if let Some(overlay) = &mut self.overlay {
            overlay.set_chats(chats.clone());
        }
        self.chats = chats;
    }

    pub fn activate_chat(&mut self, id: Uuid, title: String, messages: &[Message]) {
        // Смена чата сбрасывает состояние генерации: «осиротевшие» чанки прежней
        // генерации не должны попадать в ленту нового чата.
        self.active_chat = Some(id);
        self.title = title;
        self.current_gen = None;
        self.generating = false;
        self.feed = messages
            .iter()
            .filter_map(FeedMessage::from_message)
            .collect();
        self.feed_view.scroll_to_bottom();
    }

    pub fn push_user_message(&mut self, text: String) {
        self.feed.push(FeedMessage {
            role: FeedRole::User,
            text,
            thoughts: String::new(),
            streaming: false,
        });
        self.feed_view.scroll_to_bottom();
    }

    pub fn begin_generation(&mut self, generation_id: Uuid) {
        self.current_gen = Some(generation_id);
        self.generating = true;
        self.feed.push(FeedMessage {
            role: FeedRole::Assistant,
            text: String::new(),
            thoughts: String::new(),
            streaming: true,
        });
        self.feed_view.scroll_to_bottom();
    }

    pub fn push_chunk(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen == Some(generation_id)
            && let Some(last) = self.feed.last_mut()
        {
            last.text.push_str(text);
        }
    }

    pub fn push_thoughts(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen == Some(generation_id)
            && let Some(last) = self.feed.last_mut()
        {
            last.thoughts.push_str(text);
        }
    }

    pub fn finish_generation(&mut self, generation_id: Uuid, reason: FinishReason) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        if let Some(last) = self.feed.last_mut() {
            last.streaming = false;
        }
        self.generating = false;
        self.current_gen = None;
        if reason == FinishReason::Cancelled {
            self.push_note("(генерация отменена)");
        }
    }

    pub fn push_error(&mut self, message: &str) {
        self.push_note(&format!("⚠ {message}"));
    }

    fn push_note(&mut self, text: &str) {
        self.feed.push(FeedMessage::note(text));
        self.feed_view.scroll_to_bottom();
    }

    // ---------- ввод ----------

    /// Обрабатывает нажатие клавиши, возвращая намерение для `app` (или `None`,
    /// если клавиша обработана внутри экрана: ввод, скролл, навигация оверлея).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if self.overlay.is_some() {
            return self.handle_overlay_key(key);
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(ChatIntent::Quit),
            (KeyCode::Char('n'), KeyModifiers::CONTROL) => Some(ChatIntent::NewChat),
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.overlay = Some(ChatListState::new(self.chats.clone(), self.active_chat));
                None
            }
            // Прокрутка ленты и сворачивание «мыслей» (spec §11.3).
            (KeyCode::PageUp, _) => {
                self.feed_view.scroll_up(PAGE_SCROLL);
                None
            }
            (KeyCode::PageDown, _) => {
                self.feed_view.scroll_down(PAGE_SCROLL);
                None
            }
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                self.feed_view.toggle_thoughts();
                None
            }
            (KeyCode::Esc, _) => {
                if self.generating {
                    Some(ChatIntent::Cancel)
                } else {
                    Some(ChatIntent::Quit)
                }
            }
            // Shift+Enter — перенос строки; Enter — отправка (spec §11.7).
            (KeyCode::Enter, KeyModifiers::SHIFT) => {
                self.input.insert_newline();
                None
            }
            (KeyCode::Enter, _) => {
                let text = self.input.text();
                if !text.trim().is_empty() && !self.generating {
                    self.input.clear();
                    Some(ChatIntent::Send(text))
                } else {
                    None
                }
            }
            _ => {
                self.input.on_key(key);
                None
            }
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let overlay = self.overlay.as_mut()?;
        match overlay.on_key(key) {
            ChatListAction::None => None,
            ChatListAction::Close => {
                self.overlay = None;
                None
            }
            ChatListAction::Switch(id) => {
                self.overlay = None;
                Some(ChatIntent::SwitchChat(id))
            }
            ChatListAction::New => {
                self.overlay = None;
                Some(ChatIntent::NewChat)
            }
            ChatListAction::Clone(id) => {
                self.overlay = None;
                Some(ChatIntent::CloneChat(id))
            }
            // Удаление/переименование не закрывают оверлей: обновлённый список
            // прилетит как `set_chat_list` и синхронизирует снимок.
            ChatListAction::Delete(id) => Some(ChatIntent::DeleteChat(id)),
            ChatListAction::Rename { id, title } => Some(ChatIntent::RenameChat { id, title }),
        }
    }

    // ---------- отрисовка ----------

    pub fn render(&mut self, frame: &mut Frame) {
        // Высота ввода растёт под содержимое (1–6 строк + рамка).
        let input_h = (self.input.line_count().clamp(1, 6) + 2) as u16;
        let [feed_area, input_area, status_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(input_h),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let title = if self.title.is_empty() {
            "mindfork-rs".to_string()
        } else {
            self.title.clone()
        };
        self.feed_view.render(frame, feed_area, &title, &self.feed);

        status_bar::render(frame, status_area, &self.status, self.generating);

        let input_title = if self.generating {
            "ввод · генерация… Esc отмена"
        } else {
            "ввод · Enter отправить · Shift+Enter перенос"
        };
        let focused = self.overlay.is_none();
        self.input.render(frame, input_area, input_title, focused);

        if let Some(overlay) = &self.overlay {
            overlay.render(frame, frame.area(), self.active_chat);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::MessageRole;

    fn gen_id() -> Uuid {
        Uuid::new_v4()
    }

    #[test]
    fn streaming_sequence_builds_feed() {
        let mut s = ChatScreen::new();
        let id = gen_id();
        s.push_user_message("hi".into());
        s.begin_generation(id);
        s.push_thoughts(id, "hmm");
        s.push_chunk(id, "Hel");
        s.push_chunk(id, "lo");
        s.finish_generation(id, FinishReason::Stop);

        assert_eq!(s.feed.len(), 2);
        assert_eq!(s.feed[0].role, FeedRole::User);
        assert_eq!(s.feed[1].role, FeedRole::Assistant);
        assert_eq!(s.feed[1].text, "Hello");
        assert_eq!(s.feed[1].thoughts, "hmm");
        assert!(!s.feed[1].streaming);
        assert!(!s.generating);
    }

    #[test]
    fn ignores_chunks_from_stale_generation() {
        let mut s = ChatScreen::new();
        let current = gen_id();
        let stale = gen_id();
        s.begin_generation(current);
        s.push_chunk(stale, "ghost");
        s.push_chunk(current, "real");
        assert_eq!(s.feed[0].text, "real");
    }

    #[test]
    fn cancelled_finish_adds_note() {
        let mut s = ChatScreen::new();
        let id = gen_id();
        s.begin_generation(id);
        s.finish_generation(id, FinishReason::Cancelled);
        assert!(s.feed.iter().any(|i| i.role == FeedRole::Note));
        assert!(!s.generating);
    }

    #[test]
    fn activate_chat_rebuilds_feed_and_resets_gen() {
        let mut s = ChatScreen::new();
        s.begin_generation(gen_id()); // как будто шла генерация
        let id = gen_id();
        let messages = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("привет"),
            Message::assistant("здравствуйте"),
        ];
        s.activate_chat(id, "Чат".into(), &messages);
        assert_eq!(s.active_chat, Some(id));
        assert!(!s.generating);
        assert!(s.current_gen.is_none());
        // системное сообщение не попадает в ленту
        assert_eq!(s.feed.len(), 2);
    }

    #[test]
    fn enter_sends_when_idle_and_nonempty() {
        let mut s = ChatScreen::new();
        s.set_server_status(ServerStatus::Ready);
        for c in "привет".chars() {
            s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(intent, Some(ChatIntent::Send("привет".into())));
        // поле очищено
        assert!(
            s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .is_none()
        );
    }

    #[test]
    fn esc_cancels_while_generating_else_quits() {
        let mut s = ChatScreen::new();
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ChatIntent::Quit)
        );
        s.begin_generation(gen_id());
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ChatIntent::Cancel)
        );
    }

    #[test]
    fn ctrl_l_opens_overlay_and_routes_keys() {
        let mut s = ChatScreen::new();
        s.set_chat_list(vec![ChatSummary {
            id: gen_id(),
            title: "A".into(),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            message_count: 0,
        }]);
        assert!(s.overlay.is_none());
        s.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert!(s.overlay.is_some());
        // Esc внутри оверлея закрывает его, а не выходит из приложения
        let intent = s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(intent, None);
        assert!(s.overlay.is_none());
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = ChatScreen::new();
        s.push_user_message("привет".into());
        let id = gen_id();
        s.begin_generation(id);
        s.push_chunk(id, "# Ответ\n\nтекст");
        let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }
}
