//! Inference backends of the OpenAI family. Two protocols:
//! - **Chat Completions** ([`OpenAiClient`], `wire`): local/external `llama-server`
//!   (and OpenAI-compatible proxies/embeddings), where sampling is sent as-is; xAI,
//!   which takes the same body; and the OpenRouter gateway, in a dialect of its own
//!   (docs/research/openrouter-mode.md). The other clouds moved to their own
//!   protocols, see ADR 0004;
//! - **Responses API** ([`ResponsesClient`], `responses`): OpenAI cloud
//!   (`platform.openai.com/v1/responses`) with reasoning summaries, `reasoning.effort`,
//!   and `text.verbosity`. See ADR 0004, docs/research/openai-responses-client.md.
//!
//! Both implement [`EngineBackend`](super::contract::EngineBackend);
//! [`Embedder`](super::contract::Embedder) — only [`OpenAiClient`] (`/v1/embeddings`;
//! Responses has no embeddings). Gemini cloud uses the native
//! [`GeminiClient`](super::gemini::GeminiClient), not this client.

pub mod client;
#[cfg(test)]
mod gateway_live_tests;
#[cfg(test)]
mod gateway_tests;
pub mod responses;
mod wire;
#[cfg(test)]
mod xai_live_tests;
#[cfg(test)]
mod xai_tests;

pub(crate) use client::attributed;
pub use client::{KeyVerdict, OpenAiClient};
pub use responses::ResponsesClient;
pub(crate) use wire::ModelEnvelope;
pub use wire::tools_json;
