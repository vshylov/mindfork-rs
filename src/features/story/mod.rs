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
//! - [`scene`] — one scene played by script: M0's simulation, kept for Gate
//!   G0's fixture.
//! - [`simulation`] — M1's: a scene as K branches of rounds, the characters
//!   proposing and the World Master ruling ([`contracts`], [`prompts`]); the
//!   secrets followed by their planted tokens ([`leaks`]).

pub mod contracts;
pub mod dialect;
pub mod engine;
pub mod leaks;
pub mod port;
pub mod prompts;
pub mod replay;
pub mod scene;
pub mod schema;
pub mod simulation;

#[cfg(test)]
mod scene_live_tests;
#[cfg(test)]
mod schema_live_tests;
#[cfg(test)]
mod simulation_live_tests;
