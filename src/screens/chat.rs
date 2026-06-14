//! Экран чата (FSD "page"): компоновка виджетов, роутинг фокуса, горячие
//! клавиши и read-only-проекция состояния. См. spec §11.1, §11.7.
//!
//! Экран НЕ знает про `app`/каналы (FSD: зависимости только вниз). На нажатия
//! он возвращает [`ChatIntent`] — намерение, которое `app` транслирует в
//! `AppCommand`. События оркестратора `app` применяет, вызывая мутаторы экрана
//! (`set_*`, `push_*`, …) — экрану не нужен тип `AppEvent`.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::entities::message::Message;
use crate::entities::profile::ProfileSummary;
use crate::features::spellcheck::SpellChecker;
use crate::shared::api::FinishReason;
use crate::shared::server::ServerStatus;
use crate::widgets::chat_list::{ChatListAction, ChatListState};
use crate::widgets::input_box::InputBox;
use crate::widgets::message_feed::{FeedMessage, FeedRole, MessageFeed};
use crate::widgets::profile_list::{ProfileListAction, ProfileListState};
use crate::widgets::status_bar;

/// Высота прокрутки ленты на одно нажатие PageUp/PageDown (строк).
const PAGE_SCROLL: usize = 8;

/// Задержка дебаунса спелл-чека: слово не флагуется, пока пользователь печатает.
const SPELL_DEBOUNCE: Duration = Duration::from_millis(300);

/// Намерение пользователя, которое исполняет `app` (транслирует в `AppCommand`).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatIntent {
    Quit,
    Send(String),
    Cancel,
    /// Создать чат из профиля (`None` — профиль по умолчанию).
    NewChat {
        profile_id: Option<Uuid>,
    },
    SwitchChat(Uuid),
    CloneChat(Uuid),
    DeleteChat(Uuid),
    RenameChat {
        id: Uuid,
        title: String,
    },
}

/// Пункт попапа подсказок орфографии.
#[derive(Debug, Clone, PartialEq)]
enum SuggestItem {
    /// Заменить слово на вариант.
    Replace(String),
    /// Добавить слово в персональный словарь.
    AddToDictionary,
}

/// Попап подсказок орфографии для слова под курсором. См. spec §11.5.
struct SuggestPopup {
    word: String,
    row: usize,
    start: usize,
    end: usize,
    items: Vec<SuggestItem>,
    selected: usize,
}

/// Экран чата: всё состояние UI и его отрисовка.
pub struct ChatScreen {
    feed: Vec<FeedMessage>,
    feed_view: MessageFeed,
    active_chat: Option<Uuid>,
    title: String,
    chats: Vec<ChatSummary>,
    overlay: Option<ChatListState>,
    /// Снимок профилей (для оверлея выбора при создании чата).
    profiles: Vec<ProfileSummary>,
    /// Открытый оверлей выбора профиля.
    profile_overlay: Option<ProfileListState>,
    input: InputBox,
    status: ServerStatus,
    current_gen: Option<Uuid>,
    generating: bool,
    /// Спелл-чекер (загружается в фоне; `None`, пока не готов/нет словарей).
    spell: Option<SpellChecker>,
    /// Текст ввода изменился — нужна перепроверка орфографии (с дебаунсом).
    spell_dirty: bool,
    /// Момент последнего изменения ввода (для дебаунса).
    last_edit: Option<Instant>,
    /// Открытый попап подсказок орфографии.
    suggest: Option<SuggestPopup>,
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
            profiles: Vec::new(),
            profile_overlay: None,
            input: InputBox::new(),
            status: ServerStatus::Connecting,
            current_gen: None,
            generating: false,
            spell: None,
            spell_dirty: false,
            last_edit: None,
            suggest: None,
        }
    }

    /// Устанавливает спелл-чекер (после фоновой загрузки словарей) и планирует
    /// перепроверку текущего ввода.
    pub fn set_spellchecker(&mut self, checker: SpellChecker) {
        self.spell = Some(checker);
        self.spell_dirty = true;
        self.last_edit = None; // перепроверить немедленно
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

    pub fn set_profile_list(&mut self, profiles: Vec<ProfileSummary>) {
        self.profiles = profiles;
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
        if self.suggest.is_some() {
            self.handle_suggest_key(key);
            return None;
        }
        if self.profile_overlay.is_some() {
            return self.handle_profile_overlay_key(key);
        }
        if self.overlay.is_some() {
            return self.handle_overlay_key(key);
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(ChatIntent::Quit),
            (KeyCode::Char('n'), KeyModifiers::CONTROL) => self.request_new_chat(),
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.overlay = Some(ChatListState::new(self.chats.clone(), self.active_chat));
                None
            }
            // Подсказки орфографии для слова под курсором (spec §11.5).
            (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
                self.open_suggestions();
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
                self.mark_input_changed();
                None
            }
            (KeyCode::Enter, _) => {
                let text = self.input.text();
                if !text.trim().is_empty() && !self.generating {
                    self.input.clear();
                    self.mark_input_changed();
                    Some(ChatIntent::Send(text))
                } else {
                    None
                }
            }
            _ => {
                if self.input.on_key(key) {
                    self.mark_input_changed();
                }
                None
            }
        }
    }

    /// Помечает ввод изменённым (запускает дебаунс перепроверки орфографии).
    fn mark_input_changed(&mut self) {
        self.spell_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// Перепроверяет орфографию ввода, если истёк дебаунс. Вызывается из `render`.
    fn maybe_recheck_spelling(&mut self) {
        let Some(spell) = &self.spell else { return };
        if !self.spell_dirty {
            return;
        }
        if let Some(t) = self.last_edit
            && t.elapsed() < SPELL_DEBOUNCE
        {
            return; // ещё печатает — не флагуем текущее слово
        }
        let ranges = self
            .input
            .line_strings()
            .iter()
            .map(|line| spell.misspellings(line))
            .collect();
        self.input.set_misspelled(ranges);
        self.spell_dirty = false;
    }

    /// Открывает попап подсказок для слова с ошибкой под курсором (если есть).
    fn open_suggestions(&mut self) {
        let Some(spell) = &self.spell else { return };
        let (row, col) = self.input.cursor();
        let lines = self.input.line_strings();
        let Some(line) = lines.get(row) else { return };
        let Some(word) = spell.misspelled_word_at(line, col) else {
            return;
        };
        let mut items: Vec<SuggestItem> = spell
            .suggest(&word.text)
            .into_iter()
            .map(SuggestItem::Replace)
            .collect();
        items.push(SuggestItem::AddToDictionary);
        self.suggest = Some(SuggestPopup {
            word: word.text,
            row,
            start: word.start,
            end: word.end,
            items,
            selected: 0,
        });
    }

    fn handle_suggest_key(&mut self, key: KeyEvent) {
        let Some(popup) = &mut self.suggest else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.suggest = None,
            KeyCode::Up => popup.selected = popup.selected.saturating_sub(1),
            KeyCode::Down => {
                popup.selected = (popup.selected + 1).min(popup.items.len().saturating_sub(1));
            }
            KeyCode::Enter => self.apply_suggestion(),
            _ => {}
        }
    }

    /// Применяет выбранный пункт попапа подсказок и закрывает его.
    fn apply_suggestion(&mut self) {
        let Some(popup) = self.suggest.take() else {
            return;
        };
        match popup.items.get(popup.selected) {
            Some(SuggestItem::Replace(word)) => {
                self.input
                    .replace_range(popup.row, popup.start, popup.end, word);
            }
            Some(SuggestItem::AddToDictionary) => {
                if let Some(spell) = &mut self.spell
                    && let Err(err) = spell.add_to_personal(&popup.word)
                {
                    tracing::warn!(error = %err, "не удалось дописать персональный словарь");
                }
            }
            None => {}
        }
        self.mark_input_changed();
    }

    /// Запрашивает создание чата: при >1 профиле открывает оверлей выбора,
    /// иначе сразу создаёт из единственного/дефолтного профиля (spec §10).
    fn request_new_chat(&mut self) -> Option<ChatIntent> {
        if self.profiles.len() > 1 {
            self.profile_overlay = Some(ProfileListState::new(self.profiles.clone()));
            None
        } else {
            Some(ChatIntent::NewChat {
                profile_id: self.profiles.first().map(|p| p.id),
            })
        }
    }

    fn handle_profile_overlay_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let overlay = self.profile_overlay.as_mut()?;
        match overlay.on_key(key) {
            ProfileListAction::None => None,
            ProfileListAction::Cancel => {
                self.profile_overlay = None;
                None
            }
            ProfileListAction::Pick(id) => {
                self.profile_overlay = None;
                Some(ChatIntent::NewChat {
                    profile_id: Some(id),
                })
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
                self.request_new_chat()
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
        self.maybe_recheck_spelling();

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
        let focused =
            self.overlay.is_none() && self.profile_overlay.is_none() && self.suggest.is_none();
        self.input.render(frame, input_area, input_title, focused);

        if let Some(overlay) = &self.overlay {
            overlay.render(frame, frame.area(), self.active_chat);
        }
        if let Some(overlay) = &self.profile_overlay {
            overlay.render(frame, frame.area());
        }
        if let Some(popup) = &self.suggest {
            render_suggest(frame, popup);
        }
    }
}

/// Рисует попап подсказок орфографии по центру экрана.
fn render_suggest(frame: &mut Frame, popup: &SuggestPopup) {
    let rows = (popup.items.len() as u16 + 2).min(frame.area().height);
    let area = centered_rect(40, rows, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", popup.word))
        .title_bottom(Line::from(" Enter — применить · Esc — отмена ").dim());
    let items: Vec<ListItem> = popup
        .items
        .iter()
        .map(|item| {
            ListItem::new(match item {
                SuggestItem::Replace(word) => Line::from(word.clone()),
                SuggestItem::AddToDictionary => Line::from("➕ Добавить в словарь").italic(),
            })
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().reversed());
    let mut state = ListState::default();
    state.select(Some(
        popup.selected.min(popup.items.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(list, area, &mut state);
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

    fn mk_checker() -> SpellChecker {
        let dict = spellbook::Dictionary::new("SET UTF-8\n", "2\nhello\nworld\n").unwrap();
        SpellChecker::new(vec![dict], std::collections::HashSet::new(), None)
    }

    fn type_str(s: &mut ChatScreen, text: &str) {
        for c in text.chars() {
            s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn ctrl_g_opens_suggestions_for_misspelled_word() {
        let mut s = ChatScreen::new();
        s.set_spellchecker(mk_checker());
        type_str(&mut s, "helo"); // курсор в конце слова с ошибкой
        s.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(s.suggest.is_some());
        // последний пункт — «добавить в словарь»
        let items = &s.suggest.as_ref().unwrap().items;
        assert_eq!(items.last(), Some(&SuggestItem::AddToDictionary));
    }

    #[test]
    fn applying_suggestion_replaces_word() {
        let mut s = ChatScreen::new();
        s.set_spellchecker(mk_checker());
        type_str(&mut s, "helo");
        s.open_suggestions();
        // первый пункт — подсказка «hello»; Enter применяет
        assert_eq!(
            s.suggest.as_ref().unwrap().items.first(),
            Some(&SuggestItem::Replace("hello".into()))
        );
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(s.suggest.is_none());
        assert_eq!(s.input.text(), "hello");
    }

    #[test]
    fn add_to_dictionary_clears_the_error() {
        let mut s = ChatScreen::new();
        s.set_spellchecker(mk_checker());
        type_str(&mut s, "helo");
        s.open_suggestions();
        let last = s.suggest.as_ref().unwrap().items.len() - 1;
        // переходим на «добавить в словарь» и применяем
        for _ in 0..last {
            s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(s.suggest.is_none());
        // слово теперь в персональном словаре — больше не ошибка
        assert!(
            s.spell
                .as_ref()
                .unwrap()
                .misspelled_word_at("helo", 2)
                .is_none()
        );
    }

    #[test]
    fn esc_closes_suggestions_without_quitting() {
        let mut s = ChatScreen::new();
        s.set_spellchecker(mk_checker());
        type_str(&mut s, "helo");
        s.open_suggestions();
        assert!(s.suggest.is_some());
        let intent = s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(intent, None);
        assert!(s.suggest.is_none());
    }

    #[test]
    fn render_with_suggestions_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = ChatScreen::new();
        s.set_spellchecker(mk_checker());
        type_str(&mut s, "helo");
        s.open_suggestions();
        let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
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
