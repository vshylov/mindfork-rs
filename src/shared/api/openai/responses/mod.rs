//! OpenAI cloud backend over the **Responses API** (`POST /v1/responses`). A protocol
//! separate from Chat Completions, same vendor (ADR 0004,
//! docs/research/openai-responses-client.md): reasoning summaries
//! (`reasoning.summary`), reasoning depth (`reasoning.effort`), verbosity
//! (`text.verbosity`), reasoning items with `encrypted_content` for tool-use.
//!
//! `wire` — building the request body and parsing event-based SSE; `client` — the
//! [`EngineBackend`](crate::shared::api::contract::EngineBackend) implementation.

mod client;
mod wire;

pub use client::ResponsesClient;
