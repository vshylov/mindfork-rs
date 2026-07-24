//! The `app` layer (FSD): application composition — the TUI loop, orchestration, events.
//! See spec §4.2, §4.4.

pub mod events;
pub mod gen_state;
pub mod orchestrator;
pub mod runtime;
pub mod supervisor;
