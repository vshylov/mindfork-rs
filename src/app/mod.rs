//! Слой `app` (FSD): композиция приложения — петля TUI, оркестрация, события.
//! См. spec §4.2, §4.4.

pub mod events;
pub mod orchestrator;
pub mod runtime;
pub mod supervisor;
