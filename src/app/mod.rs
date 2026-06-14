//! Слой `app` (FSD): композиция приложения — петля TUI, оркестрация, события.
//! См. spec §4.2.

pub mod events;
pub mod runtime;

/// Запускает приложение (TUI-петлю). На M1+ здесь поднимается оркестратор
/// и мост tokio ↔ TUI.
pub fn run() -> anyhow::Result<()> {
    runtime::run()
}
