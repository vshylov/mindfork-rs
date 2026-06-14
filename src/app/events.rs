//! Контракт обмена UI ↔ оркестратор: команды и события.
//! Однонаправленный поток данных, см. spec §4.4.
//!
//! На этапе M0 объявлены минимально; наполняются на M1+
//! (SendMessage, Cancel, RegenerateLast, GenerationChunk, … — см. spec §4.4).

/// Команда от UI к оркестратору.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Завершить работу приложения.
    Quit,
}

/// Событие от оркестратора к UI (read-only-проекция обновляется только так).
#[derive(Debug, Clone)]
pub enum AppEvent {
    // Наполняется на M1+: GenerationStarted/Chunk/Thoughts/Finished/Cancelled, ChatUpdated, …
}
