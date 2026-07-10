//! Бэкенды инференса семейства OpenAI. Два протокола:
//! - **Chat Completions** ([`OpenAiClient`], `wire`): локальный/external `llama-server`
//!   и облако Gemini (OpenAI-совместимый endpoint). Диалект — [`WireDialect`];
//! - **Responses API** ([`ResponsesClient`], `responses`): облако OpenAI
//!   (`platform.openai.com/v1/responses`) с резюме рассуждений, `reasoning.effort` и
//!   `text.verbosity`. См. ADR 0004, docs/research/openai-responses-client.md.
//!
//! Оба реализуют [`EngineBackend`](super::contract::EngineBackend);
//! [`Embedder`](super::contract::Embedder) — только у [`OpenAiClient`] (`/v1/embeddings`;
//! в Responses эмбеддингов нет).

pub mod client;
pub mod responses;
mod wire;

pub use client::OpenAiClient;
pub use responses::ResponsesClient;
pub use wire::WireDialect;
