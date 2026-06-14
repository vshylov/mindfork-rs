//! Контракт обмена UI ↔ оркестратор: команды и события.
//! Однонаправленный поток данных, см. spec §4.4.

use uuid::Uuid;

use crate::shared::api::FinishReason;

/// Команда от UI к оркестратору.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Отправить сообщение пользователя и начать генерацию.
    SendMessage(String),
    /// Отменить текущую генерацию.
    Cancel,
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
