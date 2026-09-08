//! Inference backends of the OpenAI family. Two protocols:
//! - **Chat Completions** ([`OpenAiClient`], `wire`): local/external `llama-server`
//!   (and OpenAI-compatible proxies/embeddings). Sampling is sent as-is (no dialect —
//!   the clouds moved to their own protocols, see ADR 0004);
//! - **Responses API** ([`ResponsesClient`], `responses`): OpenAI cloud
//!   (`platform.openai.com/v1/responses`) with reasoning summaries, `reasoning.effort`,
//!   and `text.verbosity`. See ADR 0004, docs/research/openai-responses-client.md.
//!
//! Both implement [`EngineBackend`](super::contract::EngineBackend);
//! [`Embedder`](super::contract::Embedder) — only [`OpenAiClient`] (`/v1/embeddings`;
//! Responses has no embeddings). Gemini cloud uses the native
//! [`GeminiClient`](super::gemini::GeminiClient), not this client.

pub mod client;
pub mod responses;
mod wire;

pub use client::OpenAiClient;
pub use responses::ResponsesClient;
pub use wire::tools_json;
