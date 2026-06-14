//! Контракт обмена UI ↔ оркестратор: команды и события.
//! Однонаправленный поток данных, см. spec §4.4.

use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::entities::message::Message;
use crate::shared::api::FinishReason;

/// Команда от UI к оркестратору.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Отправить сообщение пользователя в активный чат и начать генерацию.
    SendMessage(String),
    /// Отменить текущую генерацию.
    Cancel,
    /// Создать новый чат (по профилю по умолчанию; выбор профиля — M4).
    NewChat,
    /// Сделать чат активным (загрузить его в ленту).
    SwitchChat(Uuid),
    /// Переименовать чат.
    RenameChat { id: Uuid, title: String },
    /// Клонировать чат (копия сообщений и настроек).
    CloneChat(Uuid),
    /// Мягко удалить чат.
    DeleteChat(Uuid),
    /// Завершить работу (оркестратор останавливается).
    Quit,
}

/// Статус соединения с сервером инференса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    /// Сервер не настроен (нет URL/бинарника).
    NotConfigured,
    /// Идёт подключение/запуск.
    Connecting,
    /// Готов к работе.
    Ready,
    /// Недоступен (с описанием причины).
    Disconnected(String),
}

/// Событие от оркестратора к UI. UI обновляет read-only-проекцию только так.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Изменился статус сервера.
    ServerStatus(ServerStatus),
    /// Полный список видимых чатов (для оверлея). Шлётся при изменениях набора.
    ChatList(Vec<ChatSummary>),
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
    /// Генерация завершена.
    Finished {
        generation_id: Uuid,
        reason: FinishReason,
    },
    /// Ошибка (для показа в UI).
    Error(String),
}
