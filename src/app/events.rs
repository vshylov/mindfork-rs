//! Контракт обмена UI ↔ оркестратор: команды и события.
//! Однонаправленный поток данных, см. spec §4.4.

use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::entities::message::Message;
use crate::entities::profile::ProfileSummary;
use crate::shared::api::FinishReason;
pub use crate::shared::server::ServerStatus;

/// Команда от UI к оркестратору.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Отправить сообщение пользователя в активный чат и начать генерацию.
    SendMessage(String),
    /// Отменить текущую генерацию.
    Cancel,
    /// Создать новый чат из профиля (по `id`; `None` — профиль по умолчанию).
    NewChat { profile_id: Option<Uuid> },
    /// Сделать чат активным (загрузить его в ленту).
    SwitchChat(Uuid),
    /// Переименовать чат.
    RenameChat { id: Uuid, title: String },
    /// Клонировать чат (копия сообщений и настроек).
    CloneChat(Uuid),
    /// Мягко удалить чат.
    DeleteChat(Uuid),
    /// Создать новый профиль (UI-секция — M8; команда нужна для операций/тестов).
    CreateProfile {
        name: String,
        system_message: String,
    },
    /// Мягко удалить профиль с каскадом на его чаты (notes/RAG исключаются).
    DeleteProfile(Uuid),
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
    /// Полный список видимых профилей (для оверлея выбора при создании чата).
    ProfileList(Vec<ProfileSummary>),
    /// Активный чат сменился — UI перестраивает ленту из его сообщений.
    ChatActivated {
        id: Uuid,
        title: String,
        messages: Vec<Message>,
    },
    /// Сообщение пользователя принято (эхо для ленты).
    UserMessage(String),
    /// Началась генерация ответа ассистента.
    GenerationStarted { generation_id: Uuid },
    /// Дельта основного текста ответа.
    Chunk { generation_id: Uuid, text: String },
    /// Дельта «мыслей» (CoT).
    Thoughts { generation_id: Uuid, text: String },
    /// Инструмент вызван и исполнен (для tool-блока в ленте). См. spec §6.3, §11.3.
    ToolCall {
        generation_id: Uuid,
        name: String,
        arguments: String,
        result: String,
    },
    /// Генерация завершена.
    Finished {
        generation_id: Uuid,
        reason: FinishReason,
    },
    /// Ошибка (для показа в UI).
    Error(String),
}
