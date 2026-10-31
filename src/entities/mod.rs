//! The `entities` layer (FSD): domain types with no I/O. See spec §4.2, §5.1.

pub mod attachment;
pub mod chat;
pub mod chat_file;
pub mod followup;
pub mod live_turn;
pub mod message;
pub mod message_image;
pub mod note;
pub mod profile;
pub mod rag;
pub mod sampling;
pub mod self_model;
pub mod self_model_history;
// Fabula's domain is built and tested ahead of its first production consumer,
// the background story run (docs/fabula.md §4, PR 8); the allow goes with it.
#[allow(dead_code)]
pub mod story;
pub mod subagent;
pub mod task;
pub mod workspace;
