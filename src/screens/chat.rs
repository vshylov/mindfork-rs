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
use crate::entities::profile::{Profile, ProfileSummary};
use crate::features::spellcheck::SpellChecker;
use crate::shared::api::FinishReason;
use crate::shared::config::AppConfig;
use crate::shared::keys;
use crate::shared::server::ServerStatus;
use crate::shared::theme::Palette;
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
    /// Перегенерировать последний ответ ассистента (`Ctrl+R`).
    RegenerateLast,
    /// Удалить последний обмен; текст пользователя вернётся в поле ввода (`Ctrl+E`).
    DeleteLastExchange,
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
    /// Авто-название чата силами модели (читает переписку, придумывает заголовок).
    AutoRenameChat(Uuid),
    /// Открыть экран настроек (`Ctrl+P`). `app` создаёт его из снимка настроек.
    OpenSettings,
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
    /// Последний снимок настроек (конфиг + полные профили) — для открытия экрана
    /// настроек по `Ctrl+P`. Заполняется событием `Settings`. См. spec §11.6.
    settings_snapshot: Option<(AppConfig, Vec<Profile>)>,
    /// Показан ли оверлей помощи по клавишам (`F1`/`?`). См. spec §11.7.
    show_help: bool,
    /// Активная палитра темы (из `config.interface.theme`). См. spec §11.6.
    palette: Palette,
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
            settings_snapshot: None,
            show_help: false,
            palette: Palette::default(),
        }
    }

    /// Сохраняет снимок настроек (для открытия экрана настроек по `Ctrl+P`) и
    /// обновляет палитру темы.
    pub fn set_settings(&mut self, config: AppConfig, profiles: Vec<Profile>) {
        self.palette = Palette::for_theme(config.interface.theme);
        self.settings_snapshot = Some((config, profiles));
    }

    /// Снимок настроек для создания экрана настроек (`None`, пока не получен).
    pub fn settings_snapshot(&self) -> Option<(AppConfig, Vec<Profile>)> {
        self.settings_snapshot.clone()
    }

    /// Текущие настройки спелл-чека `(включён, выбранные словари)` из последнего
    /// снимка настроек — для (пере)загрузки словарей в `app/runtime.rs`. См. spec §11.6.
    pub fn spell_config(&self) -> Option<(bool, &[String])> {
        self.settings_snapshot.as_ref().map(|(c, _)| {
            (
                c.interface.spellcheck_enabled,
                c.interface.selected_dictionaries.as_slice(),
            )
        })
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

    /// Показывает ошибку операции списка чатов. Если оверлей открыт — в его
    /// отдельной области (исчезает по нажатию клавиши, не засоряет ленту); иначе
    /// (оверлей уже закрыт, напр. поздний ответ авто-названия) — заметкой в ленте.
    pub fn set_overlay_error(&mut self, message: String) {
        match &mut self.overlay {
            Some(overlay) => overlay.set_error(message),
            None => self.push_error(&message),
        }
    }

    /// Обновляет заголовок чата в проекции (после ручного/авто-переименования).
    /// Меняет заголовок в шапке ленты, если это активный чат. Список и оверлей
    /// дополнительно обновляются событием `ChatList` (`set_chat_list`).
    pub fn rename_chat(&mut self, id: Uuid, title: String) {
        if self.active_chat == Some(id) {
            self.title = title.clone();
        }
        if let Some(c) = self.chats.iter_mut().find(|c| c.id == id) {
            c.title = title;
        }
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

    /// Возвращает текст в поле ввода после удаления последнего обмена. Если поле
    /// непустое — текст добавляется в его начало (существующий ввод не теряется).
    /// См. spec §11.7.
    pub fn restore_input(&mut self, text: String) {
        let existing = self.input.text();
        let combined = if existing.is_empty() {
            text
        } else {
            format!("{text}{existing}")
        };
        self.input.set_text(&combined);
        self.mark_input_changed();
    }

    pub fn push_user_message(&mut self, text: String) {
        self.feed.push(FeedMessage {
            role: FeedRole::User,
            text,
            thoughts: String::new(),
            tools: Vec::new(),
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
            tools: Vec::new(),
            streaming: true,
        });
        self.feed_view.scroll_to_bottom();
    }

    /// Добавляет tool-блок к текущему сообщению ассистента (live во время хода).
    pub fn push_tool_call(
        &mut self,
        generation_id: Uuid,
        name: String,
        arguments: String,
        result: String,
    ) {
        if self.current_gen == Some(generation_id)
            && let Some(last) = self.feed.last_mut()
        {
            last.tools.push(crate::widgets::message_feed::FeedToolCall {
                name,
                arguments,
                result,
            });
            self.feed_view.scroll_to_bottom();
        }
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
        // Оверлей помощи перехватывает ввод: любая клавиша закрывает его.
        if self.show_help {
            self.show_help = false;
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
        // Шорткаты с Ctrl матчим по «физической» латинской клавише — чтобы они
        // срабатывали при любой раскладке (русская ЙЦУКЕН даёт `Ctrl+д` вместо
        // `Ctrl+l`). См. shared::keys, spec §11.7.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = key.code
        {
            match keys::physical_char(c) {
                'c' => return Some(ChatIntent::Quit),
                // Экран настроек (Ctrl+P) — открывается, если снимок настроек получен.
                'p' => {
                    return self
                        .settings_snapshot
                        .is_some()
                        .then_some(ChatIntent::OpenSettings);
                }
                'n' => return self.request_new_chat(),
                // Перегенерация / удаление последнего обмена (только когда не идёт
                // генерация). См. spec §11.7.
                'r' => {
                    return (!self.generating).then_some(ChatIntent::RegenerateLast);
                }
                'e' => {
                    return (!self.generating).then_some(ChatIntent::DeleteLastExchange);
                }
                'l' => {
                    self.overlay = Some(ChatListState::new(self.chats.clone(), self.active_chat));
                    return None;
                }
                // Подсказки орфографии для слова под курсором (spec §11.5).
                'g' => {
                    self.open_suggestions();
                    return None;
                }
                // Сворачивание «мыслей» (spec §11.3).
                't' => {
                    self.feed_view.toggle_thoughts();
                    return None;
                }
                _ => {}
            }
        }
        match (key.code, key.modifiers) {
            // Помощь по клавишам: F1 всегда; `?` — только при пустом вводе (иначе
            // символ печатается). См. spec §11.7.
            (KeyCode::F(1), _) => {
                self.show_help = true;
                None
            }
            (KeyCode::Char('?'), KeyModifiers::NONE) if self.input.is_empty() => {
                self.show_help = true;
                None
            }
            // Прокрутка ленты (spec §11.3).
            (KeyCode::PageUp, _) => {
                self.feed_view.scroll_up(PAGE_SCROLL);
                None
            }
            (KeyCode::PageDown, _) => {
                self.feed_view.scroll_down(PAGE_SCROLL);
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
            // Удаление/переименование/авто-название не закрывают оверлей:
            // обновлённый список прилетит как `set_chat_list` и синхронизирует снимок.
            ChatListAction::Delete(id) => Some(ChatIntent::DeleteChat(id)),
            ChatListAction::Rename { id, title } => Some(ChatIntent::RenameChat { id, title }),
            ChatListAction::AutoRename(id) => Some(ChatIntent::AutoRenameChat(id)),
        }
    }

    // ---------- отрисовка ----------

    pub fn render(&mut self, frame: &mut Frame) {
        self.maybe_recheck_spelling();

        // Высота ввода растёт под содержимое с учётом переноса (1–6 рядов + рамка).
        // Ширина внутренней области = ширина экрана минус вертикальные рамки.
        let input_inner_w = frame.area().width.saturating_sub(2).max(1) as usize;
        let input_h = (self.input.visual_line_count(input_inner_w).clamp(1, 6) + 2) as u16;
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
        self.feed_view
            .render(frame, feed_area, &title, &self.feed, &self.palette);

        status_bar::render(
            frame,
            status_area,
            &self.status,
            self.generating,
            &self.palette,
        );

        let input_title = if self.generating {
            "ввод · генерация… Esc отмена"
        } else {
            "ввод · Enter отправить · Shift+Enter перенос"
        };
        let focused =
            self.overlay.is_none() && self.profile_overlay.is_none() && self.suggest.is_none();
        self.input
            .render(frame, input_area, input_title, focused, &self.palette);

        if let Some(overlay) = &self.overlay {
            overlay.render(frame, frame.area(), self.active_chat, &self.palette);
        }
        if let Some(overlay) = &self.profile_overlay {
            overlay.render(frame, frame.area());
        }
        if let Some(popup) = &self.suggest {
            render_suggest(frame, popup);
        }
        if self.show_help {
            render_help(frame);
        }
    }
}

/// Список горячих клавиш для оверлея помощи (`F1`/`?`). См. spec §11.7.
const HELP_KEYS: &[(&str, &str)] = &[
    ("Enter", "отправить сообщение"),
    ("Shift+Enter", "перенос строки"),
    ("Esc", "отмена генерации / закрыть"),
    ("Ctrl+L", "список чатов"),
    ("Ctrl+N", "новый чат (выбор профиля)"),
    ("Ctrl+R", "перегенерировать ответ"),
    ("Ctrl+E", "удалить последний обмен (правка)"),
    ("Ctrl+P", "экран настроек"),
    ("Ctrl+T", "свернуть/развернуть «мысли»"),
    ("Ctrl+G", "подсказки орфографии"),
    ("PageUp/PageDown", "прокрутка ленты"),
    ("F1 / ?", "эта справка"),
    ("Ctrl+C", "выход"),
];

/// Рисует оверлей помощи по центру экрана.
fn render_help(frame: &mut Frame) {
    let rows = (HELP_KEYS.len() as u16 + 2).min(frame.area().height);
    let area = centered_rect(54, rows, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Горячие клавиши ")
        .title_bottom(Line::from(" любая клавиша — закрыть ").dim());
    let items: Vec<ListItem> = HELP_KEYS
        .iter()
        .map(|(k, d)| ListItem::new(Line::from(format!("  {k:<16} {d}"))))
        .collect();
    frame.render_widget(List::new(items).block(block), area);
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
    fn restore_input_sets_when_empty_and_prepends_when_not() {
        let mut s = ChatScreen::new();
        // Пустое поле — просто заполняется.
        s.restore_input("вопрос".into());
        assert_eq!(s.input.text(), "вопрос");
        // Непустое — текст добавляется в начало, существующий ввод сохраняется.
        s.input.clear();
        type_str(&mut s, "хвост");
        s.restore_input("голова ".into());
        assert_eq!(s.input.text(), "голова хвост");
    }

    #[test]
    fn ctrl_r_and_e_emit_intents_when_idle() {
        let mut s = ChatScreen::new();
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            Some(ChatIntent::RegenerateLast)
        );
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            Some(ChatIntent::DeleteLastExchange)
        );
    }

    #[test]
    fn ctrl_r_and_e_suppressed_while_generating() {
        let mut s = ChatScreen::new();
        s.begin_generation(gen_id());
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            None
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
    fn f1_opens_and_any_key_closes_help() {
        let mut s = ChatScreen::new();
        assert!(!s.show_help);
        s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(s.show_help);
        // Любая клавиша закрывает справку и не делает ничего другого.
        let intent = s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(intent, None);
        assert!(!s.show_help);
        assert!(s.input.is_empty(), "ввод не печатался при закрытии справки");
    }

    #[test]
    fn question_mark_opens_help_only_when_input_empty() {
        let mut s = ChatScreen::new();
        // Пустой ввод → `?` открывает справку.
        s.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(s.show_help);
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)); // закрыть
        // Непустой ввод → `?` печатается, справка не открывается.
        type_str(&mut s, "abc");
        s.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(!s.show_help);
        assert_eq!(s.input.text(), "abc?");
    }

    #[test]
    fn settings_event_updates_theme_palette() {
        use crate::shared::config::Theme;
        let mut s = ChatScreen::new();
        assert_eq!(s.palette, Palette::for_theme(Theme::Auto));
        let mut cfg = AppConfig::default();
        cfg.interface.theme = Theme::Dark;
        s.set_settings(cfg, vec![]);
        assert_eq!(s.palette, Palette::for_theme(Theme::Dark));
    }

    #[test]
    fn ctrl_p_opens_settings_only_with_snapshot() {
        let mut s = ChatScreen::new();
        // Без снимка настроек — Ctrl+P ничего не делает.
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            None
        );
        s.set_settings(AppConfig::default(), vec![Profile::new("P", "sys")]);
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Some(ChatIntent::OpenSettings)
        );
    }

    #[test]
    fn ctrl_shortcuts_work_under_cyrillic_layout() {
        // При русской раскладке физические клавиши дают кириллицу: Ctrl+з (физ. P),
        // Ctrl+с (физ. C), Ctrl+д (физ. L) — шорткаты обязаны срабатывать.
        let mut s = ChatScreen::new();
        s.set_settings(AppConfig::default(), vec![Profile::new("P", "sys")]);
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('з'), KeyModifiers::CONTROL)),
            Some(ChatIntent::OpenSettings),
            "Ctrl+з (физ. P) открывает настройки"
        );
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('с'), KeyModifiers::CONTROL)),
            Some(ChatIntent::Quit),
            "Ctrl+с (физ. C) — выход"
        );
        // Ctrl+д (физ. L) открывает оверлей списка чатов (внутреннее действие).
        assert!(s.overlay.is_none());
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('д'), KeyModifiers::CONTROL)),
            None
        );
        assert!(s.overlay.is_some(), "Ctrl+д (физ. L) открыл список чатов");
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
    fn rename_chat_updates_title_bar_of_active_chat() {
        let mut s = ChatScreen::new();
        let id = gen_id();
        s.activate_chat(id, "Старое".into(), &[]);
        s.rename_chat(id, "Новое".into());
        assert_eq!(s.title, "Новое");
        // Чужой чат не трогает шапку активного.
        s.rename_chat(gen_id(), "Постороннее".into());
        assert_eq!(s.title, "Новое");
    }

    #[test]
    fn overlay_ctrl_r_routes_auto_rename_intent() {
        let mut s = ChatScreen::new();
        let id = gen_id();
        s.set_chat_list(vec![ChatSummary {
            id,
            title: "A".into(),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            message_count: 2,
        }]);
        s.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert!(s.overlay.is_some());
        let intent = s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(intent, Some(ChatIntent::AutoRenameChat(id)));
        // Оверлей остаётся открытым — список обновится событием ChatList.
        assert!(s.overlay.is_some());
    }

    #[test]
    fn overlay_error_goes_to_overlay_when_open_else_feed() {
        let mut s = ChatScreen::new();
        // Оверлей закрыт — ошибка падает заметкой в ленту.
        s.set_overlay_error("упс".into());
        assert!(s.feed.iter().any(|m| m.role == FeedRole::Note));
        let feed_before = s.feed.len();
        // Оверлей открыт — ошибка идёт в его область, лента не растёт.
        s.set_chat_list(vec![ChatSummary {
            id: gen_id(),
            title: "A".into(),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            message_count: 0,
        }]);
        s.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        s.set_overlay_error("в оверлей".into());
        assert_eq!(
            s.feed.len(),
            feed_before,
            "лента не пополняется при открытом оверлее"
        );
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
