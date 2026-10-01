//! Google Gemini cloud backend over the **native** API (`generateContent`/
//! `streamGenerateContent`). A protocol separate from OpenAI-compatible Chat Completions
//! (ADR 0004, docs/research/gemini-native-client.md): "thought" summaries
//! (`thinkingConfig.includeThoughts`), reasoning depth (`thinkingLevel`/
//! `thinkingBudget`), `thoughtsTokenCount`.
//!
//! `wire` — building the request body and parsing SSE; `client` — the
//! [`EngineBackend`](crate::shared::api::contract::EngineBackend) implementation. Gemini
//! embeddings are fetched separately via the OpenAI-compatible endpoint (`OpenAiClient`),
//! see the supervisor.

mod client;
#[cfg(test)]
mod live_tests;
mod wire;

pub use client::GeminiClient;
// Model-generation inference, shared with the video client (`shared::video::gemini`):
// which thinking knob this model takes, and what a refusal of a level is answered
// with. One copy, not two.
#[cfg(test)]
pub(crate) use wire::LEVEL_REFUSED;
pub(crate) use wire::{is_gemini_3, is_gemini_3_pro, level_above, says_level_refused};
