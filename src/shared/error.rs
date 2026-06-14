//! Базовые типы ошибок инфраструктурных модулей `shared`.
//!
//! Прикладные слои (`app`, `screens`, `widgets`, `features`) используют
//! `anyhow::Result`; библиотечные модули `shared` — этот `thiserror`-тип.

use thiserror::Error;

/// Ошибки инфраструктурного слоя.
#[derive(Debug, Error)]
pub enum SharedError {
    /// Ошибка ввода-вывода (файлы конфигурации/чатов/логов).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Уже запущен другой экземпляр приложения.
    #[error("another instance of mindfork-rs is already running")]
    AlreadyRunning,

    /// Прочая ошибка инфраструктуры.
    #[error("{0}")]
    Other(String),
}

/// Удобный `Result` для модулей `shared`.
pub type Result<T> = std::result::Result<T, SharedError>;
