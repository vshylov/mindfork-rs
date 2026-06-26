//! Бэкенд инференса для OpenAI-протокола: локальный/external `llama-server`, а также
//! облако OpenAI и Gemini (OpenAI-совместимый endpoint). Реализует
//! [`EngineBackend`](super::contract::EngineBackend) и [`Embedder`](super::contract::Embedder)
//! через [`OpenAiClient`]. Диалект тела запроса — [`WireDialect`]. См. ADR 0004.

pub mod client;
mod wire;

pub use client::OpenAiClient;
pub use wire::WireDialect;
