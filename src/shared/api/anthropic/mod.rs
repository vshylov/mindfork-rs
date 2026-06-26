//! Бэкенд инференса Anthropic (Claude) — отдельный протокол Messages API
//! (`/v1/messages`). Реализует [`EngineBackend`](super::contract::EngineBackend)
//! через [`AnthropicClient`]. См. ADR 0004, Фаза 2.

pub mod client;
mod wire;

pub use client::AnthropicClient;
