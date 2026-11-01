//! Fabula, the story engine (docs/fabula.md; docs/research/fabula-engine.md;
//! ADR 0014). Built a pull request at a time:
//!
//! - [`port`] — the model port, [`LlmPort`](port::LlmPort), and the one
//!   policy every answer goes through: a cut retried once, an invalid reply
//!   repaired once, a refusal final (spec §8).
//! - [`engine`] — the port over the chat's own `EngineBackend`, recording
//!   every attempt in the story's call log.
//! - [`replay`] — the port over that call log, keyed by address, with drift
//!   reported (spec §9).
//! - [`schema`] — the replies' schemas, derived from their types, and the
//!   validator that is the contract; [`dialect`] — the dialect's own check.
//! - [`scene`] — one scene played by script: M0's simulation, until M1's
//!   World Master (docs/fabula.md §4, PR 4).

pub mod dialect;
pub mod engine;
pub mod port;
pub mod replay;
pub mod scene;
pub mod schema;

#[cfg(test)]
mod scene_live_tests;
#[cfg(test)]
mod schema_live_tests;
