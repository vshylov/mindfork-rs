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
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::entities::message::Message;
use crate::entities::profile::{Profile, ProfileSummary};
use crate::features::rag_ingest::RagProgress;
use crate::features::spellcheck::SpellChecker;
use crate::shared::api::FinishReason;
use crate::shared::config::AppConfig;
use crate::shared::i18n::{Locale, locale};
use crate::shared::keys;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::theme::Palette;
use crate::shared::ui::{dim_background, render_scrollbar};
use crate::widgets::emoji_picker::{EmojiPickerAction, EmojiPickerState};
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
    /// Показать источники базы знаний (команда `/rag list`).
    RagList,
    /// Реиндексировать базу знаний (команда `/rag rebuild`).
    RagRebuild,
    /// Открыть экран настроек (`Ctrl+P`). `app` создаёт его из снимка настроек.
    OpenSettings,
    /// Открыть экран списка чатов (`Esc`). `app` создаёт его из снимка списка.
    OpenChatList,
    /// Открыть экран просмотра «модели себя» (`F3`). `app` запрашивает снимок у
    /// оркестратора (`RequestSelfModel`) и создаёт экран из события `SelfModelView`.
    OpenSelfModel,
    /// Включить/выключить захват мыши терминала для прокрутки колесом (`Ctrl+W`).
    /// `true` — колесо прокручивает ленту (выделение текста — с Shift); `false` —
    /// нативное выделение мышью. См. spec §11.3.
    SetMouseCapture(bool),
    /// Записать текст в системный буфер обмена (`Ctrl+C` копировать / `Ctrl+X`
    /// вырезать выделение поля ввода). Side-effect UI-слоя — `runtime` пишет через
    /// `arboard` (не идёт в оркестратор: текст уже у UI). См. docs/history/input-selection-undo-mouse.md §B.
    CopyToClipboard(String),
}

/// Необратимая операция, требующая подтверждения в модальном попапе (`Ctrl+R`/
/// `Ctrl+E`, когда включена настройка `interface.confirm_destructive_keys`).
/// См. spec §11.7.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConfirmAction {
    /// Перегенерировать последний ответ (`Ctrl+R`).
    Regenerate,
    /// Удалить последний обмен (`Ctrl+E`).
    DeleteExchange,
}

impl ConfirmAction {
    /// Намерение, которое подтверждает эта операция.
    fn intent(self) -> ChatIntent {
        match self {
            ConfirmAction::Regenerate => ChatIntent::RegenerateLast,
            ConfirmAction::DeleteExchange => ChatIntent::DeleteLastExchange,
        }
    }

    /// Текст-вопрос попапа подтверждения (локализованный).
    fn prompt(self, loc: &'static Locale) -> &'static str {
        match self {
            ConfirmAction::Regenerate => loc.t("ui.confirm.regenerate"),
            ConfirmAction::DeleteExchange => loc.t("ui.confirm.delete_exchange"),
        }
    }
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
    /// Снимок статусов всех серверов (чат/эмбеддинги/имперсонация) для строки
    /// статуса. См. spec §11.1.
    statuses: ServerStatuses,
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
    /// Reasoning-токены («мысли») текущей/последней генерации из `usage` (`0` — нет/
    /// провайдер не разделяет). Статус-бар показывает их отдельно при `> 0`.
    gen_reasoning: u32,
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
    /// Открытый попап выбора эмодзи (`Ctrl+B`). См. spec §11.5.
    emoji: Option<EmojiPickerState>,
    /// Открытый модальный попап подтверждения необратимой операции (`Ctrl+R`/
    /// `Ctrl+E`); `None` — попап закрыт. См. spec §11.7.
    confirm: Option<ConfirmAction>,
    /// Спрашивать ли подтверждение перед `Ctrl+R`/`Ctrl+E` (из
    /// `interface.confirm_destructive_keys`; обновляется событием `Settings`).
    confirm_destructive: bool,
    /// Индекс последнего выделения в попапе эмодзи — восстанавливается при следующем
    /// открытии (попап «помнит» выбор).
    emoji_last: usize,
    /// Последний снимок настроек (конфиг + полные профили + id профилей с
    /// заблокированным языком каркаса) — для открытия экрана настроек по `Ctrl+P`.
    /// Заполняется событием `Settings`. См. spec §11.6, docs/i18n.md.
    settings_snapshot: Option<(AppConfig, Vec<Profile>, Vec<uuid::Uuid>)>,
    /// Показан ли оверлей помощи по клавишам (`F1`/`?`). См. spec §11.7.
    show_help: bool,
    /// Прокрутка оверлея помощи (первый видимый ряд списка клавиш) — для коротких
    /// терминалов, где весь список не помещается. Сбрасывается при открытии;
    /// клампится к максимуму в `render_help` (высота попапа известна там).
    help_scroll: usize,
    /// Активная палитра темы (из `config.interface.theme`). См. spec §11.6.
    palette: Palette,
    /// Локаль интерфейса (из `config.interface.language`, ось B — docs/i18n-ui.md).
    /// `&'static` — вшитый бандл; обновляется вместе с палитрой в `set_settings`.
    loc: &'static Locale,
    /// Включён ли захват мыши для прокрутки колесом (тумблер `Ctrl+W`). По
    /// умолчанию выключен — работает нативное выделение текста мышью. См. spec §11.3.
    mouse_scroll: bool,
    /// Идёт ли фоновая авто-рефлексия «модели себя» (тихий индикатор в статус-баре).
    reflecting: bool,
    /// Идёт ли фоновая авто-консолидация заметок («сон»; тихий индикатор).
    consolidating: bool,
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
            statuses: ServerStatuses {
                chat: ServerStatus::Connecting,
                embed: ServerStatus::NotConfigured,
                impersonation: ServerStatus::NotConfigured,
            },
            current_gen: None,
            generating: false,
            gen_tokens: 0,
            gen_context: None,
            gen_context_exact: false,
            gen_reasoning: 0,
            spell: None,
            spell_dirty: false,
            draft_dirty: false,
            last_edit: None,
            suggest: None,
            emoji: None,
            emoji_last: 0,
            confirm: None,
            confirm_destructive: false,
            settings_snapshot: None,
            show_help: false,
            help_scroll: 0,
            palette: Palette::default(),
            loc: locale(crate::shared::i18n::Lang::default()),
            mouse_scroll: false,
            reflecting: false,
            consolidating: false,
            rag: None,
            impersonation: None,
            pending_text_sep: false,
            pending_thoughts_sep: false,
        }
    }

    /// Сохраняет снимок настроек (для открытия экрана настроек по `Ctrl+P`) и
    /// обновляет палитру темы (вместе с режимом совместимости терминала) и
    /// параметры отрисовки ленты (разделители строк таблиц).
    pub fn set_settings(
        &mut self,
        config: AppConfig,
        profiles: Vec<Profile>,
        language_locked: Vec<uuid::Uuid>,
    ) {
        self.palette = Palette::for_theme(config.interface.theme)
            .with_compat(config.interface.terminal_compat);
        self.loc = locale(config.interface.language);
        self.confirm_destructive = config.interface.confirm_destructive_keys;
        self.feed_view
            .set_table_row_separators(config.interface.table_row_separators);
        self.settings_snapshot = Some((config, profiles, language_locked));
    }

    /// Снимок настроек для создания экрана настроек (`None`, пока не получен).
    pub fn settings_snapshot(&self) -> Option<(AppConfig, Vec<Profile>, Vec<uuid::Uuid>)> {
        self.settings_snapshot.clone()
    }

    /// Текущие настройки спелл-чека `(включён, выбранные словари)` из последнего
    /// снимка настроек — для (пере)загрузки словарей в `app/runtime.rs`. См. spec §11.6.
    pub fn spell_config(&self) -> Option<(bool, &[String])> {
        self.settings_snapshot.as_ref().map(|(c, _, _)| {
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

    /// Текущий спелл-чекер (или `None`, пока словари не загружены). Чекер живёт
    /// здесь, в экране чата; `app` одалживает его экрану списка чатов для подсветки
    /// ошибок в поле переименования (`F2`). См. spec §11.5.
    pub fn spellchecker(&self) -> Option<&SpellChecker> {
        self.spell.as_ref()
    }

    // ---------- проекция событий оркестратора (вызывается слоем `app`) ----------

    pub fn set_server_status(&mut self, statuses: ServerStatuses) {
        self.statuses = statuses;
    }

    /// Текущий снимок статусов серверов — чтобы `app` передал его открываемому
    /// экрану настроек (чипы статусов в секции «Модель/сервер»). См. spec §11.6.
    pub fn server_statuses(&self) -> ServerStatuses {
        self.statuses.clone()
    }

    /// Ставит/снимает флаг активной авто-рефлексии (тихий индикатор в статус-баре).
    /// Вид фоновой задачи различает `app` (маппинг `AppEvent::BackgroundTask`) — так
    /// `screens` не зависит от контракта `app` (FSD).
    pub fn set_reflecting(&mut self, active: bool) {
        self.reflecting = active;
    }

    /// Ставит/снимает флаг активной авто-консолидации заметок («сон»).
    pub fn set_consolidating(&mut self, active: bool) {
        self.consolidating = active;
    }

    /// Метка активных фоновых задач для статус-бара (`None` — ничего не идёт).
    pub(super) fn background_hint(&self) -> Option<String> {
        match (self.reflecting, self.consolidating) {
            (true, true) => Some(self.loc.t("ui.chat.bg.both").to_string()),
            (true, false) => Some(self.loc.t("ui.chat.bg.reflect").to_string()),
            (false, true) => Some(self.loc.t("ui.chat.bg.consolidate").to_string()),
            (false, false) => None,
        }
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

    /// Текущая локаль интерфейса — для отрисовки overlay-экранов (список чатов /
    /// модель себя) и broadcast при смене языка. См. docs/i18n-ui.md §3.3.
    pub fn loc(&self) -> &'static Locale {
        self.loc
    }
}

// ---------- подмодули (разбор god-object: docs/history/refactoring-god-objects.md, этап 2) ----------

mod feed;
mod impersonation;
mod input;
mod popups;
mod rag;
mod render;

#[cfg(test)]
mod tests;
