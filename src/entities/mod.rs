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
// Fabula's domain is reached from production through the chat's story tools and
// the background story run (docs/fabula.md §4, PRs 8–9); what they do not
// reach yet — the replay, the harnesses' scenes, the leak audit and the
// export (M3) — is built and tested ahead of those consumers. The allow goes
// with the last of them.
#[allow(dead_code)]
pub mod story;
pub mod subagent;
pub mod task;
pub mod workspace;
