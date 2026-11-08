//! Fabula, the story engine (docs/history/fabula.md; docs/research/fabula-engine.md;
//! ADR 0014). M3's judges preferred one writer in one session to the
//! simulation the engine was first built around, which is gone (docs/history/fabula.md
//! §6, T28):
//!
//! - [`port`] — the model port, [`LlmPort`](port::LlmPort), and the one
//!   policy every answer goes through: a cut retried once, an invalid reply
//!   repaired once, a refusal final (spec §8).
//! - [`engine`] — the port over the chat's own `EngineBackend`, recording
//!   every attempt in the story's call log.
//! - [`schema`] — the replies' schemas and the validator that is the
//!   contract; [`dialect`] — the dialect's own check.
//! - [`single`] — B0, one writer in one session; [`single_story`] — the
//!   story tools' engine (docs/history/fabula.md §6, T25–T27): a session a chapter,
//!   with the pauses between; [`author_prompts`] — the canon and the rules of
//!   the prose as the writer reads them.
//! - [`run`] — a story's run as the app starts it; [`board`] — the runs out;
//!   [`conversation`] — what the chat tools do to a story's files;
//!   [`answer`] — the person's reply read.
//! - [`eval`] — M3's hidden runner, B0 alone now: the frozen premises, the
//!   batch; [`judge`] — its blind judging.

pub mod answer;
pub mod author_prompts;
pub mod board;
pub mod conversation;
pub mod dialect;
pub mod engine;
pub mod eval;
pub mod export;
pub mod judge;
pub mod port;
pub mod run;
pub mod schema;
pub mod single;
pub mod single_story;

#[cfg(test)]
pub(crate) mod live_route;
#[cfg(test)]
mod run_live_tests;
