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
// Fabula's domain is reached from production through the background story
// run (docs/fabula.md §4, PR 8); what the run does not reach yet — the canon's
// import and checks and the answer's resolution (`story_create`,
// `story_continue`, PR 9), the replay and the harnesses' scenes (M3) — is
// built and tested ahead of those consumers. The allow goes with the last of them.
#[allow(dead_code)]
pub mod story;
pub mod subagent;
pub mod task;
pub mod workspace;
