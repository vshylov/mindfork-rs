//! Контракт обмена UI ↔ оркестратор: команды и события.
//! Однонаправленный поток данных, см. spec §4.4.

use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::entities::message::Message;
use crate::entities::profile::{Profile, ProfileSummary};
use crate::features::profiles::ProfileEdit;
pub use crate::features::rag_ingest::RagProgress;
use crate::shared::api::FinishReason;
use crate::shared::config::AppConfig;
pub use crate::shared::server::ServerStatus;

/// Команда от UI к оркестратору.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Отправить сообщение пользователя в активный чат и начать генерацию.
    SendMessage(String),
    /// Сохранить черновик поля ввода в активном чате (несохранённый текст). UI шлёт
    /// при изменении ввода; оркестратор пишет в файл чата с дебаунсом. См. spec §11.7.
    SetDraft(String),
    /// Перегенерировать последний ответ ассистента: удалить всё после последнего
    /// сообщения пользователя и запустить генерацию заново из того же запроса.
    RegenerateLast,
    /// Удалить последний обмен (ответ ассистента вместе с вызвавшим его сообщением
    /// пользователя); текст пользователя возвращается в поле ввода (`RestoreInput`).
    DeleteLastExchange,
    /// Отменить текущую генерацию.
    Cancel,
    /// Написать сообщение от имени пользователя (имперсонация, `Ctrl+U`). `seed` —
    /// уже введённый в поле текст (модель продолжит его; пусто — пишет с нуля).
    /// Результат стримится событиями `Impersonation*`. См. spec §11.8.
    Impersonate { seed: String },
    /// Отменить текущую имперсонацию.
    CancelImpersonation,
    /// Создать новый чат из профиля (по `id`; `None` — профиль по умолчанию).
    NewChat { profile_id: Option<Uuid> },
    /// Сделать чат активным (загрузить его в ленту).
    SwitchChat(Uuid),
    /// Переименовать чат.
    RenameChat { id: Uuid, title: String },
    /// Авто-название чата: модель читает переписку (или её часть) и придумывает
    /// заголовок. Запрос идёт фоновой задачей; результат — событие `ChatRenamed`.
    AutoRenameChat(Uuid),
    /// Клонировать чат (копия сообщений и настроек).
    CloneChat(Uuid),
    /// Скопировать всю переписку чата в буфер обмена. Оркестратор формирует текст
    /// (он владеет `Chat`) и эмитит `CopyToClipboard`; запись в буфер — в UI-слое.
    CopyChat(Uuid),
    /// Мягко удалить чат.
    DeleteChat(Uuid),
    /// Создать новый профиль (UI-секция — M8; команда нужна для операций/тестов).
    CreateProfile {
        name: String,
        system_message: String,
    },
    /// Мягко удалить профиль с каскадом на его чаты (notes/RAG исключаются).
    DeleteProfile(Uuid),
    /// Заменить конфигурацию целиком (экран настроек). Оркестратор сохраняет её,
    /// перезапускает сервер/реестр при необходимости и переэмитит. См. spec §11.6.
    /// `Box` — `AppConfig` крупный, не раздуваем enum.
    UpdateConfig(Box<AppConfig>),
    /// Применить правки профиля (экран настроек). `Box` — `ProfileEdit` несёт
    /// крупные поля (системное сообщение).
    UpdateProfile { id: Uuid, edit: Box<ProfileEdit> },
    /// Индексировать файл или директорию в базу знаний (RAG) активного профиля
    /// (команда `/rag add <path> [-r]`). Выполняется фоновой задачей; прогресс
    /// приходит событиями `RagProgress`. См. spec §9.3.
    RagAdd { path: String, recursive: bool },
    /// Удалить из базы знаний файл или директорию (со всем, что под ней) активного
    /// профиля (команда `/rag remove <path>`). Результат — событие `RagProgress`.
    RagDelete { path: String },
    /// Показать источники базы знаний активного профиля (команда `/rag list`).
    /// Результат — событие `RagProgress::Listed`.
    RagList,
    /// Реиндексировать базу знаний активного профиля (команда `/rag rebuild`):
    /// перечанковать и переэмбеддить сохранённые исходники. Выполняется фоновой
    /// задачей; прогресс — событиями `RagProgress`. См. spec §9.3.
    RagRebuild,
    /// Запросить снимок «модели себя» активного профиля (для экрана просмотра, `F3`).
    /// Оркестратор (владелец `Storage`) отвечает событием `SelfModelView`.
    RequestSelfModel,
    /// Завершить работу (оркестратор останавливается).
    Quit,
}

/// Событие от оркестратора к UI. UI обновляет read-only-проекцию только так.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Изменился статус сервера.
    ServerStatus(ServerStatus),
    /// Полный список видимых чатов (для оверлея). Шлётся при изменениях набора.
    ChatList(Vec<ChatSummary>),
    /// Заголовок чата изменился (ручное/авто-переименование). UI обновляет шапку
    /// ленты активного чата без перестроения (`ChatList` обновляет список).
    ChatRenamed { id: Uuid, title: String },
    /// Ошибка операции над списком чатов (авто-название/удаление/клон). Показывается
    /// в отдельной области оверлея списка (а не в ленте чата), если он открыт.
    ChatListError(String),
    /// Записать текст в буфер обмена (side-effect UI-слоя). Оркестратор сформировал
    /// переписку чата; runtime пишет в буфер и шлёт подтверждение/ошибку в оверлей.
    CopyToClipboard(String),
    /// Полный список видимых профилей (для оверлея выбора при создании чата).
    ProfileList(Vec<ProfileSummary>),
    /// Полный снимок настроек для экрана настроек (конфиг + полные профили).
    /// Шлётся при старте и после любой правки конфига/профилей. См. spec §11.6.
    Settings {
        config: Box<AppConfig>,
        profiles: Vec<Profile>,
    },
    /// Активный чат сменился — UI перестраивает ленту из его сообщений и загружает
    /// сохранённый черновик в поле ввода (`draft`; пустой у нового чата).
    ChatActivated {
        id: Uuid,
        title: String,
        messages: Vec<Message>,
        draft: String,
    },
    /// Сообщение пользователя принято (эхо для ленты).
    UserMessage(String),
    /// Вернуть текст в поле ввода (после удаления последнего обмена). Непустой
    /// существующий ввод не затирается — текст добавляется в его начало (UI).
    RestoreInput(String),
    /// Началась генерация ответа ассистента.
    GenerationStarted { generation_id: Uuid },
    /// Дельта основного текста ответа.
    Chunk { generation_id: Uuid, text: String },
    /// Дельта «мыслей» (CoT).
    Thoughts { generation_id: Uuid, text: String },
    /// Счётчик токенов текущей генерации (live). `completion` — сгенерировано
    /// токенов ответа (накопительно по раундам agentic-loop); `context` — токенов
    /// в промпте (вся переписка); `None` оставляет прежнее значение нетронутым.
    /// `context_exact` — точное ли это число из `usage` сервера (иначе клиентская
    /// оценка, UI помечает `~`). UI показывает «переписка + ответ» в статус-баре,
    /// см. spec §11.1.
    TokenUsage {
        generation_id: Uuid,
        completion: u64,
        context: Option<u64>,
        context_exact: bool,
    },
    /// Инструмент вызван и исполнен (для tool-блока в ленте). См. spec §6.3, §11.3.
    ToolCall {
        generation_id: Uuid,
        name: String,
        arguments: String,
        result: String,
    },
    /// Ассистент решил написать **ещё одно** сообщение (инструмент
    /// `send_followup_message`): UI завершает текущий пузырь и начинает новый,
    /// в который пойдёт текст следующего раунда. См. spec §9.3.
    AssistantContinue { generation_id: Uuid },
    /// Ассистент решил **переписать** текущее сообщение (инструмент
    /// `rewrite_current_message`): UI отбрасывает уже накопленный текст текущего
    /// пузыря; в него пойдёт переписанный ответ. См. spec §9.3.
    AssistantRewrite { generation_id: Uuid },
    /// Генерация завершена.
    Finished {
        generation_id: Uuid,
        reason: FinishReason,
    },
    /// Имперсонация началась: UI прячет поле ввода и показывает потоковый предпросмотр
    /// реплики (заполняется уже введённым текстом). См. spec §11.8.
    ImpersonationStarted { generation_id: Uuid },
    /// Дельта текста имперсонируемой реплики (в предпросмотр).
    ImpersonationChunk { generation_id: Uuid, text: String },
    /// Имперсонация завершена. При `Stop`/`Length` UI вставляет накопленный текст в
    /// поле ввода; при `Cancelled`/`Error` — отбрасывает (поле ввода не меняется).
    ImpersonationFinished {
        generation_id: Uuid,
        reason: FinishReason,
    },
    /// Прогресс фоновой индексации файлов в RAG (команда `/rag add`). См. spec §9.3.
    RagProgress(RagProgress),
    /// Снимок «модели себя» активного профиля (ответ на `RequestSelfModel`) для
    /// экрана просмотра (`F3`). `None` — модель ещё не создавалась. `Box` — крупный
    /// тип, не раздуваем enum. См. docs/self-model-mvp.md.
    SelfModelView(Box<Option<crate::entities::self_model::SelfModel>>),
    /// Ошибка (для показа в UI).
    Error(String),
}
