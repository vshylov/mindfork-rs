//! Экран чата (FSD "page"): компоновка виджетов, роутинг фокуса, горячие
//! клавиши и read-only-проекция состояния. См. spec §11.1, §11.7.
//!
//! Экран НЕ знает про `app`/каналы (FSD: зависимости только вниз). На нажатия
//! он возвращает [`ChatIntent`] — намерение, которое `app` транслирует в
//! `AppCommand`. События оркестратора `app` применяет, вызывая мутаторы экрана
//! (`set_*`, `push_*`, …) — экрану не нужен тип `AppEvent`.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::entities::message::Message;
use crate::entities::profile::{Profile, ProfileSummary};
use crate::features::rag_ingest::RagProgress;
use crate::features::spellcheck::SpellChecker;
use crate::shared::api::FinishReason;
use crate::shared::config::AppConfig;
use crate::shared::keys;
use crate::shared::server::ServerStatus;
use crate::shared::theme::Palette;
use crate::widgets::impersonation_preview;
use crate::widgets::input_box::InputBox;
use crate::widgets::message_feed::{FeedMessage, FeedRole, MessageFeed};
use crate::widgets::profile_list::{ProfileListAction, ProfileListState};
use crate::widgets::status_bar;

/// Высота прокрутки ленты на одно нажатие PageUp/PageDown (строк).
const PAGE_SCROLL: usize = 8;

/// Высота прокрутки ленты на одну «зарубку» колеса мыши (строк).
const WHEEL_SCROLL: usize = 3;

/// Задержка дебаунса спелл-чека: слово не флагуется, пока пользователь печатает.
const SPELL_DEBOUNCE: Duration = Duration::from_millis(300);

/// Кадры спиннера индикатора фоновой индексации RAG.
const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

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
    /// Написать сообщение от имени пользователя (имперсонация, `Ctrl+U`). `seed` —
    /// уже введённый текст (модель продолжит его). См. spec §11.8.
    Impersonate {
        seed: String,
    },
    /// Отменить текущую имперсонацию (`Esc` в предпросмотре).
    CancelImpersonation,
    /// Создать чат из профиля (`None` — профиль по умолчанию). `Ctrl+N` в чате
    /// (операции над конкретными чатами — переключение/клон/удаление/переименование
    /// — идут через экран списка чатов, [`ChatListIntent`](crate::screens::chat_list::ChatListIntent)).
    NewChat {
        profile_id: Option<Uuid>,
    },
    /// Скопировать всю переписку активного чата в буфер обмена (`F5`).
    CopyChat(Uuid),
    /// Индексировать файл/директорию в RAG (команда `/rag add <path> [-r]`).
    RagAdd {
        path: String,
        recursive: bool,
    },
    /// Удалить файл/директорию из RAG (команда `/rag remove <path>`).
    RagDelete {
        path: String,
    },
    /// Открыть экран настроек (`Ctrl+P`). `app` создаёт его из снимка настроек.
    OpenSettings,
    /// Открыть экран списка чатов (`Esc`). `app` создаёт его из снимка списка.
    OpenChatList,
    /// Включить/выключить захват мыши терминала для прокрутки колесом (`Ctrl+W`).
    /// `true` — колесо прокручивает ленту (выделение текста — с Shift); `false` —
    /// нативное выделение мышью. См. spec §11.3.
    SetMouseCapture(bool),
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

/// Состояние имперсонации (`Ctrl+U`): пока идёт написание реплики «за пользователя»,
/// поле ввода скрыто и показывается потоковый предпросмотр. См. spec §11.8.
struct ImpersonationState {
    generation_id: Uuid,
    /// Накопленный текст реплики (начинается с уже введённого текста-затравки).
    text: String,
    /// Счётчик тиков перерисовки для анимации спиннера.
    tick: usize,
    /// Генерация завершена (спиннер гаснет до применения/сброса).
    done: bool,
}

/// Баннер прогресса фоновой индексации RAG (`/rag add`). Живёт, пока идёт
/// индексация; завершение/ошибка гасят баннер и оставляют заметку в ленте.
struct RagBanner {
    /// Текущий текст индикатора (без спиннера).
    text: String,
    /// Счётчик тиков перерисовки для анимации спиннера.
    tick: usize,
}

/// Экран чата: всё состояние UI и его отрисовка.
pub struct ChatScreen {
    feed: Vec<FeedMessage>,
    feed_view: MessageFeed,
    active_chat: Option<Uuid>,
    title: String,
    chats: Vec<ChatSummary>,
    /// Снимок профилей (для оверлея выбора при создании чата).
    profiles: Vec<ProfileSummary>,
    /// Открытый оверлей выбора профиля.
    profile_overlay: Option<ProfileListState>,
    input: InputBox,
    status: ServerStatus,
    current_gen: Option<Uuid>,
    generating: bool,
    /// Счётчик токенов ответа текущей/последней генерации (показывается в
    /// статус-баре). Сбрасывается при старте новой генерации. См. spec §11.1.
    gen_tokens: u64,
    /// Токенов в переписке (промпте) — оценка клиента до ответа сервера, затем
    /// точное `usage.prompt_tokens`; `None`, пока неизвестно.
    gen_context: Option<u64>,
    /// Точное ли значение `gen_context` (из `usage` сервера). `false` — оценка,
    /// статус-бар помечает её `~`.
    gen_context_exact: bool,
    /// Спелл-чекер (загружается в фоне; `None`, пока не готов/нет словарей).
    spell: Option<SpellChecker>,
    /// Текст ввода изменился — нужна перепроверка орфографии (с дебаунсом).
    spell_dirty: bool,
    /// Черновик ввода изменился — нужно сохранить его в активном чате. Петля
    /// забирает текст (`take_dirty_draft`) и шлёт `SetDraft`. См. spec §11.7.
    draft_dirty: bool,
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
    /// Включён ли захват мыши для прокрутки колесом (тумблер `Ctrl+W`). По
    /// умолчанию выключен — работает нативное выделение текста мышью. См. spec §11.3.
    mouse_scroll: bool,
    /// Индикатор фоновой индексации RAG (`/rag add`); `None` — индексация не идёт.
    rag: Option<RagBanner>,
    /// Состояние имперсонации (`Ctrl+U`); `None` — не идёт. См. spec §11.8.
    impersonation: Option<ImpersonationState>,
    /// Был ли вызов инструмента после последнего текстового чанка стримящегося
    /// ответа: первый текст следующего раунда отделяется пустой строкой (`\n\n`),
    /// чтобы live-стрим совпадал с перезагрузкой (`FeedMessage::from_messages`).
    pending_text_sep: bool,
    /// То же для блока «мыслей» (разделитель `\n` между раундами).
    pending_thoughts_sep: bool,
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
            profiles: Vec::new(),
            profile_overlay: None,
            input: InputBox::new(),
            status: ServerStatus::Connecting,
            current_gen: None,
            generating: false,
            gen_tokens: 0,
            gen_context: None,
            gen_context_exact: false,
            spell: None,
            spell_dirty: false,
            draft_dirty: false,
            last_edit: None,
            suggest: None,
            settings_snapshot: None,
            show_help: false,
            palette: Palette::default(),
            mouse_scroll: false,
            rag: None,
            impersonation: None,
            pending_text_sep: false,
            pending_thoughts_sep: false,
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
        self.chats = chats;
    }

    pub fn set_profile_list(&mut self, profiles: Vec<ProfileSummary>) {
        self.profiles = profiles;
    }

    /// Снимок списка чатов — для создания экрана списка по `Esc` (`OpenChatList`).
    pub fn chat_summaries(&self) -> Vec<ChatSummary> {
        self.chats.clone()
    }

    /// Активный чат (метка в экране списка; `None`, пока чат не выбран).
    pub fn active_chat(&self) -> Option<Uuid> {
        self.active_chat
    }

    /// Текущая палитра темы — для отрисовки экрана списка чатов.
    pub fn palette(&self) -> Palette {
        self.palette
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

    pub fn activate_chat(&mut self, id: Uuid, title: String, messages: &[Message], draft: &str) {
        // Смена чата сбрасывает состояние генерации: «осиротевшие» чанки прежней
        // генерации не должны попадать в ленту нового чата.
        self.active_chat = Some(id);
        self.title = title;
        self.current_gen = None;
        self.generating = false;
        // Склейка раундов agentic-loop в один блок «Ассистент:» с инлайн tool-блоками.
        self.feed = FeedMessage::from_messages(messages);
        self.feed_view.scroll_to_bottom();
        // Загружаем сохранённый черновик чата в поле ввода (пустой у нового чата).
        // НЕ помечаем `draft_dirty` — иначе тут же отправили бы его обратно тем же
        // `SetDraft`; перепроверку орфографии запускаем напрямую.
        self.input.set_text(draft);
        self.spell_dirty = true;
        self.last_edit = None;
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
        self.gen_tokens = 0;
        self.gen_context = None;
        self.gen_context_exact = false;
        self.pending_text_sep = false;
        self.pending_thoughts_sep = false;
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
            // Вызов сделан после уже накопленного текста ответа — фиксируем позицию,
            // чтобы tool-блок встал на месте вызова, а не в «шапке».
            let text_offset = last.text.len();
            last.tools.push(crate::widgets::message_feed::FeedToolCall {
                name,
                arguments,
                result,
                text_offset,
            });
            // Текст/мысли следующего раунда отделяем разделителем (как при перезагрузке).
            self.pending_text_sep = true;
            self.pending_thoughts_sep = true;
            self.feed_view.scroll_to_bottom();
        }
    }

    pub fn push_chunk(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen == Some(generation_id)
            && let Some(last) = self.feed.last_mut()
        {
            // Первый текст раунда после вызова инструмента — с пустой строкой-
            // разделителем (совпадение с `FeedMessage::from_messages`).
            if self.pending_text_sep {
                self.pending_text_sep = false;
                if !last.text.is_empty() {
                    last.text.push_str("\n\n");
                }
            }
            last.text.push_str(text);
        }
    }

    pub fn push_thoughts(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen == Some(generation_id)
            && let Some(last) = self.feed.last_mut()
        {
            if self.pending_thoughts_sep {
                self.pending_thoughts_sep = false;
                if !last.thoughts.is_empty() {
                    last.thoughts.push('\n');
                }
            }
            last.thoughts.push_str(text);
        }
    }

    /// Обновляет счётчик токенов текущей генерации (live). Игнорирует устаревшие
    /// события (по `generation_id`). Контекст (переписку) обновляет только когда он
    /// задан (`Some`), запоминая, точное это число или оценка.
    pub fn set_token_usage(
        &mut self,
        generation_id: Uuid,
        completion: u64,
        context: Option<u64>,
        context_exact: bool,
    ) {
        if self.current_gen == Some(generation_id) {
            self.gen_tokens = completion;
            if let Some(c) = context {
                self.gen_context = Some(c);
                self.gen_context_exact = context_exact;
            }
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

    // ---------- имперсонация (Ctrl+U, spec §11.8) ----------

    /// Начинает имперсонацию: прячет поле ввода и показывает потоковый предпросмотр,
    /// затравленный уже введённым текстом (модель продолжит его).
    pub fn begin_impersonation(&mut self, generation_id: Uuid) {
        self.impersonation = Some(ImpersonationState {
            generation_id,
            text: self.input.text(),
            tick: 0,
            done: false,
        });
    }

    /// Дописывает дельту текста имперсонации в предпросмотр.
    pub fn push_impersonation_chunk(&mut self, generation_id: Uuid, text: &str) {
        if let Some(imp) = &mut self.impersonation
            && imp.generation_id == generation_id
        {
            imp.text.push_str(text);
        }
    }

    /// Завершает имперсонацию. При `Stop`/`Length` накопленный текст вставляется в
    /// поле ввода; при `Cancelled`/`Error` отбрасывается (поле ввода сохраняет
    /// исходный текст-затравку). См. spec §11.8.
    pub fn finish_impersonation(&mut self, generation_id: Uuid, reason: FinishReason) {
        match &self.impersonation {
            Some(imp) if imp.generation_id == generation_id => {}
            _ => return,
        }
        let imp = self.impersonation.take().unwrap();
        match reason {
            FinishReason::Stop | FinishReason::Length => {
                // set_text ставит курсор в конец вставленного текста.
                self.input.set_text(&imp.text);
                self.mark_input_changed();
            }
            _ => {}
        }
    }

    /// Идёт ли имперсонация (петля перерисовывает кадры для анимации спиннера).
    pub fn is_impersonating(&self) -> bool {
        self.impersonation.is_some()
    }

    /// Обновляет индикатор фоновой индексации RAG (`/rag add`). Старт/прогресс
    /// показывают баннер со спиннером; завершение/ошибка гасят его и оставляют
    /// итоговую заметку в ленте. См. spec §9.3.
    pub fn set_rag_progress(&mut self, progress: RagProgress) {
        match progress {
            RagProgress::Started { total } => {
                self.rag = Some(RagBanner {
                    text: format!("найдено файлов: {total}, начинаю индексацию…"),
                    tick: 0,
                });
            }
            RagProgress::Indexing {
                index,
                total,
                name,
                dir,
            } => {
                let location = if dir.is_empty() {
                    String::new()
                } else {
                    format!(" из {dir}")
                };
                let text = format!("индексация {name}{location} ({index}/{total})");
                match &mut self.rag {
                    Some(banner) => banner.text = text,
                    None => self.rag = Some(RagBanner { text, tick: 0 }),
                }
            }
            RagProgress::Finished {
                files,
                chunks,
                errors,
                cancelled,
            } => {
                self.rag = None;
                let mut msg = if cancelled {
                    format!("RAG: индексация прервана — фрагментов добавлено: {chunks}")
                } else {
                    format!("RAG: индексация завершена — файлов: {files}, фрагментов: {chunks}")
                };
                if errors > 0 {
                    msg.push_str(&format!(", с ошибками: {errors}"));
                }
                self.push_note(&msg);
            }
            RagProgress::Removed { chunks } => {
                let msg = if chunks == 0 {
                    "RAG: по указанному пути ничего не найдено в базе".to_string()
                } else {
                    format!("RAG: удалено фрагментов: {chunks}")
                };
                self.push_note(&msg);
            }
            RagProgress::Failed(err) => {
                self.rag = None;
                self.push_error(&format!("RAG: {err}"));
            }
        }
    }

    /// Идёт ли фоновая индексация RAG (петля перерисовывает кадры для анимации
    /// спиннера, пока это `true`).
    pub fn is_rag_active(&self) -> bool {
        self.rag.is_some()
    }

    /// Добавляет нейтральную заметку в ленту (напр. подтверждение операции списка
    /// чатов, когда экран списка уже закрыт — поздний ответ авто-названия/копии).
    pub fn push_note(&mut self, text: &str) {
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
        // Во время имперсонации поле ввода скрыто (показан предпросмотр): реагируем
        // только на отмену (`Esc`) и выход (`Ctrl+C`); прочие клавиши игнорируем.
        if self.impersonation.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && let KeyCode::Char(c) = key.code
                && keys::physical_char(c) == 'c'
            {
                return Some(ChatIntent::Quit);
            }
            if key.code == KeyCode::Esc {
                return Some(ChatIntent::CancelImpersonation);
            }
            return None;
        }
        if self.suggest.is_some() {
            self.handle_suggest_key(key);
            return None;
        }
        if self.profile_overlay.is_some() {
            return self.handle_profile_overlay_key(key);
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
                // Имперсонация: написать сообщение от лица пользователя (spec §11.8).
                // `seed` — уже введённый текст (модель продолжит его).
                'u' => {
                    return (!self.generating).then(|| ChatIntent::Impersonate {
                        seed: self.input.text(),
                    });
                }
                // Удалить весь текст ввода / вернуть удалённое (spec §11.5).
                // Повторное нажатие восстанавливает удалённое, если после него
                // ничего не вводилось.
                'k' => {
                    self.input.clear_or_restore();
                    self.mark_input_changed();
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
                // Тумблер прокрутки колесом ↔ выделения текста мышью (spec §11.3).
                // `Ctrl+M` для этого непригоден: терминал отдаёт его как Enter.
                'w' => {
                    self.mouse_scroll = !self.mouse_scroll;
                    return Some(ChatIntent::SetMouseCapture(self.mouse_scroll));
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
            // Копирование переписки активного чата в буфер обмена (как F5 в списке
            // чатов). Подтверждение/ошибка приходят заметкой в ленту (оверлея нет).
            (KeyCode::F(5), _) => self.active_chat.map(ChatIntent::CopyChat),
            // Прокрутка ленты (spec §11.3).
            (KeyCode::PageUp, _) => {
                self.feed_view.scroll_up(PAGE_SCROLL);
                None
            }
            (KeyCode::PageDown, _) => {
                self.feed_view.scroll_down(PAGE_SCROLL);
                None
            }
            // Esc открывает экран списка чатов (`app` создаёт его из снимка списка;
            // `Esc` там закрывает экран — переключение «список ↔ чат»). Во время
            // генерации Esc сперва отменяет её. Выход — `Ctrl+C`. См. spec §11.7.
            (KeyCode::Esc, _) => {
                if self.generating {
                    Some(ChatIntent::Cancel)
                } else {
                    Some(ChatIntent::OpenChatList)
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
                if text.trim().is_empty() {
                    return None;
                }
                // Slash-команда RAG (`/rag add …`) — не отправляется как сообщение и
                // работает независимо от генерации (фоновая индексация).
                if let Some(parsed) = crate::features::rag_command::parse(&text) {
                    use crate::features::rag_command::RagCommand;
                    self.input.clear();
                    self.mark_input_changed();
                    return match parsed {
                        Ok(RagCommand::Add { path, recursive }) => {
                            Some(ChatIntent::RagAdd { path, recursive })
                        }
                        Ok(RagCommand::Delete { path }) => Some(ChatIntent::RagDelete { path }),
                        Err(msg) => {
                            self.push_note(&format!("RAG: {msg}"));
                            None
                        }
                    };
                }
                if !self.generating {
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

    /// Вставляет текст из буфера обмена в поле ввода (событие `Event::Paste` —
    /// bracketed paste). Вставка идёт одним куском, переводы строк сохраняются как
    /// текст (а НЕ трактуются как Enter/отправка). Работает только в основном виде:
    /// при открытой справке/попапе/оверлее (их однострочные поля) — no-op. Вставка
    /// не отправляет сообщение даже с переносами внутри. См. spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        if self.show_help || self.suggest.is_some() || self.profile_overlay.is_some() {
            return;
        }
        if text.is_empty() {
            return;
        }
        self.input.insert_str(text);
        self.mark_input_changed();
    }

    /// Обрабатывает событие мыши: колесо прокручивает ленту чата. Работает только
    /// в основном виде — при открытом оверлее/попапе/справке прокрутка ленты под
    /// ними была бы неожиданной, поэтому это no-op. См. spec §11.3.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.show_help || self.suggest.is_some() || self.profile_overlay.is_some() {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.feed_view.scroll_up(WHEEL_SCROLL),
            MouseEventKind::ScrollDown => self.feed_view.scroll_down(WHEEL_SCROLL),
            _ => {}
        }
    }

    /// Помечает ввод изменённым (запускает дебаунс перепроверки орфографии и
    /// сохранение черновика в активном чате).
    fn mark_input_changed(&mut self) {
        self.spell_dirty = true;
        self.draft_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// Забирает изменённый черновик ввода для сохранения в активном чате (или
    /// `None`, если с прошлого раза не менялся). Петля шлёт его командой `SetDraft`;
    /// запись на диск в оркестраторе идёт с дебаунсом. См. spec §11.7.
    pub fn take_dirty_draft(&mut self) -> Option<String> {
        if !self.draft_dirty {
            return None;
        }
        self.draft_dirty = false;
        Some(self.input.text())
    }

    /// Перепроверяет орфографию ввода, если истёк дебаунс. Возвращает `true`, если
    /// подсветка ошибок была пересчитана (нужна перерисовка). Вызывается из петли
    /// каждый тик (она и обеспечивает пробуждение по истечении дебаунса — рендер
    /// сам по тикам уже не запускается). См. spec §11.5.
    pub fn maybe_recheck_spelling(&mut self) -> bool {
        let Some(spell) = &self.spell else {
            return false;
        };
        if !self.spell_dirty {
            return false;
        }
        // Команды (`/rag …`) и пути файлов орфографией не проверяем — снимаем
        // возможные подчёркивания (они подсвечиваются жёлтым целиком при рендере).
        if self.input_is_command() {
            self.input.set_misspelled(Vec::new());
            self.spell_dirty = false;
            return true;
        }
        if let Some(t) = self.last_edit
            && t.elapsed() < SPELL_DEBOUNCE
        {
            return false; // ещё печатает — не флагуем текущее слово
        }
        let ranges = self
            .input
            .line_strings()
            .iter()
            .map(|line| spell.misspellings(line))
            .collect();
        self.input.set_misspelled(ranges);
        self.spell_dirty = false;
        true
    }

    /// Является ли текущий ввод командой (`/rag …`). Такой текст подсвечивается
    /// жёлтым и не проверяется орфографией. См. spec §11.5.
    fn input_is_command(&self) -> bool {
        crate::features::rag_command::parse(&self.input.text()).is_some()
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
    /// Публичный: `app` вызывает его, когда `Ctrl+N` нажат в экране списка чатов
    /// (выбор профиля живёт здесь, в экране чата).
    pub fn request_new_chat(&mut self) -> Option<ChatIntent> {
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

    // ---------- отрисовка ----------

    pub fn render(&mut self, frame: &mut Frame) {
        let _ = self.maybe_recheck_spelling();

        // Анимация спиннера индикатора индексации (петля рисует каждый тик, пока
        // `is_rag_active()`); делитель замедляет смену кадров до приятного темпа.
        if let Some(banner) = &mut self.rag {
            banner.tick = banner.tick.wrapping_add(1);
        }
        // Анимация спиннера предпросмотра имперсонации (пока `is_impersonating()`).
        if let Some(imp) = &mut self.impersonation {
            imp.tick = imp.tick.wrapping_add(1);
        }

        // Высота ввода/предпросмотра растёт под содержимое с учётом переноса (1–6
        // рядов + рамка). Ширина внутренней области = ширина экрана минус рамки.
        let input_inner_w = frame.area().width.saturating_sub(2).max(1) as usize;
        let content_lines = match &self.impersonation {
            Some(imp) => visual_line_count(&imp.text, input_inner_w),
            None => self.input.visual_line_count(input_inner_w),
        };
        let input_h = (content_lines.clamp(1, 6) + 2) as u16;
        // Баннер индексации RAG занимает строку только когда активен (иначе 0 —
        // пустой прямоугольник, рендер в него безвреден).
        let banner_h: u16 = if self.rag.is_some() { 1 } else { 0 };
        let [feed_area, banner_area, input_area, status_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(banner_h),
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

        if let Some(banner) = &self.rag {
            let spinner = SPINNER[(banner.tick / 2) % SPINNER.len()];
            let line = Line::from(vec![
                Span::styled(format!("{spinner} RAG: "), self.palette.accent_style()),
                Span::from(banner.text.clone()),
            ]);
            frame.render_widget(line, banner_area);
        }

        status_bar::render(
            frame,
            status_area,
            &self.status,
            self.generating,
            self.gen_tokens,
            self.gen_context,
            self.gen_context_exact,
            self.mouse_scroll,
            &self.palette,
        );

        // Во время имперсонации поле ввода скрыто — на его месте потоковый
        // предпросмотр реплики. См. spec §11.8.
        if let Some(imp) = &self.impersonation {
            impersonation_preview::render(
                frame,
                input_area,
                &imp.text,
                imp.tick,
                imp.done,
                &self.palette,
            );
        } else {
            let input_title = if self.generating {
                "ввод · генерация… Esc отмена"
            } else {
                "ввод · Enter отправить · Shift+Enter перенос"
            };
            let focused = self.profile_overlay.is_none() && self.suggest.is_none();
            let command = self.input_is_command();
            self.input.render(
                frame,
                input_area,
                input_title,
                focused,
                &self.palette,
                command,
            );
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
    ("Ctrl+V", "вставить текст из буфера (многострочно)"),
    ("Esc", "список чатов · закрыть · отмена генерации"),
    ("Ctrl+N", "новый чат (выбор профиля)"),
    ("F5", "копировать переписку чата"),
    ("Ctrl+R", "перегенерировать ответ"),
    ("Ctrl+E", "удалить последний обмен (правка)"),
    ("Ctrl+U", "написать сообщение за пользователя"),
    ("Ctrl+K", "удалить весь текст ввода (повтор — вернуть)"),
    ("Ctrl+P", "экран настроек"),
    ("Ctrl+T", "свернуть/развернуть «мысли»"),
    ("Ctrl+G", "подсказки орфографии"),
    ("Ctrl+W", "колесо мыши ↔ выделение текста"),
    ("/rag add <путь> [-r]", "индексировать файлы в RAG"),
    ("/rag remove <путь>", "удалить файлы из RAG"),
    ("PageUp/PageDown", "прокрутка ленты"),
    ("F1 / ?", "эта справка"),
    ("Ctrl+C", "выход"),
];

/// Рисует оверлей помощи по центру экрана.
fn render_help(frame: &mut Frame) {
    let rows = (HELP_KEYS.len() as u16 + 2).min(frame.area().height);
    let key_width = HELP_KEYS
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let desc_width = HELP_KEYS
        .iter()
        .map(|(_, d)| d.chars().count())
        .max()
        .unwrap_or(0);
    // Ширина строки: "  " слева + поле клавиш + " " + описание + "  " справа + рамка (2).
    let width = (2 + key_width + 1 + desc_width + 2 + 2) as u16;
    let area = centered_rect(width, rows, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Горячие клавиши ")
        .title_bottom(Line::from(" любая клавиша — закрыть ").dim());
    let items: Vec<ListItem> = HELP_KEYS
        .iter()
        .map(|(k, d)| ListItem::new(Line::from(format!("  {k:<key_width$} {d}"))))
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

/// Число визуальных рядов, которые займёт `text` при переносе по ширине `width`
/// (для расчёта высоты предпросмотра имперсонации). Минимум 1.
fn visual_line_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    text.split('\n')
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            crate::shared::wrap::wrap_ranges(&chars, width).len().max(1)
        })
        .sum::<usize>()
        .max(1)
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
    fn live_stream_with_tool_matches_reload() {
        use crate::entities::message::{Message, ToolCallRecord};

        // Live: текст раунда 1 → вызов инструмента → текст раунда 2 (финал).
        let mut s = ChatScreen::new();
        let id = gen_id();
        s.begin_generation(id);
        s.push_chunk(id, "Ищу погоду.");
        s.push_tool_call(
            id,
            "web_search".into(),
            "{\"q\":\"погода\"}".into(),
            "ясно".into(),
        );
        s.push_chunk(id, "Сейчас ясно.");
        s.finish_generation(id, FinishReason::Stop);

        let live = s.feed.last().unwrap().clone();
        assert_eq!(live.text, "Ищу погоду.\n\nСейчас ясно.");
        assert_eq!(live.tools.len(), 1);
        assert_eq!(live.tools[0].text_offset, "Ищу погоду.".len());

        // Reload: те же раунды как доменные сообщения (assistant+tool / assistant).
        let mut r1 = Message::assistant("Ищу погоду.");
        r1.tool_calls = vec![ToolCallRecord {
            id: "c1".into(),
            name: "web_search".into(),
            arguments: serde_json::json!({"q": "погода"}),
            result: Some("ясно".into()),
        }];
        let tool_msg = {
            let mut m = Message::new(MessageRole::Tool, "ясно");
            m.tool_call_id = Some("c1".into());
            m.tool_name = Some("web_search".into());
            m
        };
        let r2 = Message::assistant("Сейчас ясно.");
        let reload = FeedMessage::from_messages(&[r1, tool_msg, r2]);

        // Один слитый блок ассистента, тот же текст и то же смещение вызова.
        assert_eq!(reload.len(), 1);
        assert_eq!(reload[0].text, live.text);
        assert_eq!(reload[0].tools.len(), 1);
        assert_eq!(reload[0].tools[0].text_offset, live.tools[0].text_offset);
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
        s.activate_chat(id, "Чат".into(), &messages, "");
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
    fn activate_chat_loads_draft_without_marking_dirty() {
        let mut s = ChatScreen::new();
        // Активация чата с сохранённым черновиком загружает его в поле ввода…
        s.activate_chat(gen_id(), "Чат".into(), &[], "недописанный текст");
        assert_eq!(s.input.text(), "недописанный текст");
        // …но не помечает черновик «грязным» (иначе тут же отправили бы его обратно).
        assert_eq!(s.take_dirty_draft(), None);
        // Переключение на чат без черновика очищает поле ввода.
        s.activate_chat(gen_id(), "Новый".into(), &[], "");
        assert!(s.input.is_empty());
        assert_eq!(s.take_dirty_draft(), None);
    }

    #[test]
    fn typing_marks_draft_dirty_and_take_returns_text_once() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "черновик");
        // Первый забор отдаёт набранный текст…
        assert_eq!(s.take_dirty_draft(), Some("черновик".into()));
        // …повторный — None, пока ввод снова не изменится.
        assert_eq!(s.take_dirty_draft(), None);
    }

    #[test]
    fn sending_clears_draft_to_empty() {
        let mut s = ChatScreen::new();
        s.set_server_status(ServerStatus::Ready);
        type_str(&mut s, "вопрос");
        let _ = s.take_dirty_draft(); // забрали черновик при наборе
        let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(intent, Some(ChatIntent::Send("вопрос".into())));
        // Отправка очистила поле — черновик стал пустым (UI отправит SetDraft("")).
        assert_eq!(s.take_dirty_draft(), Some(String::new()));
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
    fn ctrl_u_emits_impersonate_with_input_seed() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "начало");
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Some(ChatIntent::Impersonate {
                seed: "начало".into()
            })
        );
        // Во время генерации — подавляется.
        s.begin_generation(gen_id());
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn impersonation_stream_then_stop_commits_text_to_input() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "Я "); // затравка
        let id = gen_id();
        s.begin_impersonation(id);
        assert!(s.is_impersonating());
        s.push_impersonation_chunk(id, "хочу узнать про Rust");
        s.finish_impersonation(id, FinishReason::Stop);
        assert!(!s.is_impersonating());
        // Текст реплики (затравка + сгенерированное) — в поле ввода.
        assert_eq!(s.input.text(), "Я хочу узнать про Rust");
    }

    #[test]
    fn impersonation_cancel_keeps_seed_and_discards_generated() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "черновик");
        let id = gen_id();
        s.begin_impersonation(id);
        s.push_impersonation_chunk(id, " дополнение");
        // Esc во время имперсонации — намерение отмены.
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ChatIntent::CancelImpersonation)
        );
        // Отмена (Cancelled) отбрасывает сгенерированное — поле сохраняет затравку.
        s.finish_impersonation(id, FinishReason::Cancelled);
        assert!(!s.is_impersonating());
        assert_eq!(s.input.text(), "черновик");
    }

    #[test]
    fn keys_ignored_during_impersonation_except_cancel_quit() {
        let mut s = ChatScreen::new();
        s.begin_impersonation(gen_id());
        // Обычная клавиша не печатается в поле (поле скрыто).
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            None
        );
        // Ctrl+C всё ещё выходит.
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(ChatIntent::Quit)
        );
    }

    #[test]
    fn render_during_impersonation_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = ChatScreen::new();
        let id = gen_id();
        s.begin_impersonation(id);
        s.push_impersonation_chunk(id, "текст реплики");
        let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
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
    fn esc_opens_chat_list_else_cancels_generation() {
        let mut s = ChatScreen::new();
        // Без генерации Esc просит открыть экран списка чатов (его создаёт `app`).
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ChatIntent::OpenChatList)
        );
        // Во время генерации Esc сперва отменяет её.
        s.begin_generation(gen_id());
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ChatIntent::Cancel)
        );
    }

    #[test]
    fn ctrl_c_quits_from_chat() {
        let mut s = ChatScreen::new();
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(ChatIntent::Quit)
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
        // Ctrl+с (физ. C) — шорткаты обязаны срабатывать.
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
    }

    #[test]
    fn rename_chat_updates_title_bar_of_active_chat() {
        let mut s = ChatScreen::new();
        let id = gen_id();
        s.activate_chat(id, "Старое".into(), &[], "");
        s.rename_chat(id, "Новое".into());
        assert_eq!(s.title, "Новое");
        // Чужой чат не трогает шапку активного.
        s.rename_chat(gen_id(), "Постороннее".into());
        assert_eq!(s.title, "Новое");
    }

    #[test]
    fn f5_copies_active_chat_in_main_window() {
        let mut s = ChatScreen::new();
        // Без активного чата F5 — no-op.
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            None
        );
        let id = gen_id();
        s.activate_chat(id, "Чат".into(), &[], "");
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            Some(ChatIntent::CopyChat(id))
        );
    }

    #[test]
    fn late_list_op_results_fall_to_feed() {
        // Когда экран списка закрыт, поздние результаты операций списка (копия/
        // авто-название) `app` кладёт заметкой в ленту через push_note/push_error.
        let mut s = ChatScreen::new();
        s.push_note("Переписка скопирована в буфер обмена");
        s.push_error("не удалось");
        assert_eq!(
            s.feed.iter().filter(|m| m.role == FeedRole::Note).count(),
            2
        );
    }

    fn wheel(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_wheel_up_scrolls_feed_and_disables_follow() {
        let mut s = ChatScreen::new();
        assert!(s.feed_view.is_following());
        s.handle_mouse(wheel(MouseEventKind::ScrollUp));
        assert!(
            !s.feed_view.is_following(),
            "прокрутка вверх отключает следование за хвостом"
        );
    }

    #[test]
    fn ctrl_w_toggles_mouse_capture_intent() {
        let mut s = ChatScreen::new();
        // По умолчанию захват выключен → первое нажатие включает (true).
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            Some(ChatIntent::SetMouseCapture(true))
        );
        // Второе — выключает (false).
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            Some(ChatIntent::SetMouseCapture(false))
        );
        // Работает и при русской раскладке: Ctrl+ц (физ. W).
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('ц'), KeyModifiers::CONTROL)),
            Some(ChatIntent::SetMouseCapture(true))
        );
    }

    #[test]
    fn mouse_wheel_ignored_while_overlay_open() {
        let mut s = ChatScreen::new();
        s.show_help = true;
        s.handle_mouse(wheel(MouseEventKind::ScrollUp));
        assert!(
            s.feed_view.is_following(),
            "при открытой справке колесо не трогает ленту"
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
    fn rag_command_intercepted_on_enter() {
        let mut s = ChatScreen::new();
        s.set_server_status(ServerStatus::Ready);
        type_str(&mut s, "/rag add d:\\docs -r");
        let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            intent,
            Some(ChatIntent::RagAdd {
                path: "d:\\docs".into(),
                recursive: true,
            })
        );
        assert!(s.input.is_empty(), "поле очищено после команды");
    }

    #[test]
    fn invalid_rag_command_shows_note_and_does_not_send() {
        let mut s = ChatScreen::new();
        s.set_server_status(ServerStatus::Ready);
        type_str(&mut s, "/rag");
        let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(intent, None, "ошибочная команда не отправляется");
        assert!(s.feed.iter().any(|m| m.role == FeedRole::Note));
    }

    #[test]
    fn rag_progress_banner_lifecycle() {
        let mut s = ChatScreen::new();
        assert!(!s.is_rag_active());
        s.set_rag_progress(RagProgress::Started { total: 3 });
        assert!(s.is_rag_active());
        s.set_rag_progress(RagProgress::Indexing {
            index: 1,
            total: 3,
            name: "a.txt".into(),
            dir: "d:\\docs".into(),
        });
        assert!(s.is_rag_active());
        s.set_rag_progress(RagProgress::Finished {
            files: 3,
            chunks: 9,
            errors: 0,
            cancelled: false,
        });
        assert!(!s.is_rag_active(), "по завершении баннер гаснет");
        assert!(
            s.feed
                .iter()
                .any(|m| m.role == FeedRole::Note && m.text.contains("завершена"))
        );
    }

    #[test]
    fn command_input_is_not_spellchecked() {
        let mut s = ChatScreen::new();
        s.set_spellchecker(mk_checker());
        // Обычный текст с ошибкой → проверяется (есть подчёркивания).
        type_str(&mut s, "helo");
        s.last_edit = None; // снять дебаунс, чтобы перепроверка прошла сразу
        assert!(s.maybe_recheck_spelling());
        assert!(
            !s.input.misspelled_is_empty(),
            "обычный текст проверяется орфографией"
        );
        // Делаем из строки команду — орфография снимается.
        s.input.clear();
        type_str(&mut s, "/rag add helo");
        s.last_edit = None;
        assert!(s.input_is_command());
        assert!(s.maybe_recheck_spelling());
        assert!(
            s.input.misspelled_is_empty(),
            "команда не проверяется орфографией"
        );
    }

    #[test]
    fn rag_remove_command_intercepted_on_enter() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "/rag remove d:\\dir\\file.txt");
        let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            intent,
            Some(ChatIntent::RagDelete {
                path: "d:\\dir\\file.txt".into(),
            })
        );
        assert!(s.input.is_empty());
    }

    #[test]
    fn rag_removed_progress_pushes_note() {
        let mut s = ChatScreen::new();
        s.set_rag_progress(RagProgress::Removed { chunks: 5 });
        assert!(
            s.feed
                .iter()
                .any(|m| m.role == FeedRole::Note && m.text.contains("удалено фрагментов: 5"))
        );
        // Ноль — понятная заметка «ничего не найдено».
        s.set_rag_progress(RagProgress::Removed { chunks: 0 });
        assert!(
            s.feed
                .iter()
                .any(|m| m.role == FeedRole::Note && m.text.contains("ничего не найдено"))
        );
    }

    #[test]
    fn rag_failure_pushes_error_note() {
        let mut s = ChatScreen::new();
        s.set_rag_progress(RagProgress::Failed("эмбеддер недоступен".into()));
        assert!(!s.is_rag_active());
        assert!(
            s.feed
                .iter()
                .any(|m| m.role == FeedRole::Note && m.text.contains("эмбеддер недоступен"))
        );
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
