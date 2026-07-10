//! Бэкенд облака OpenAI через **Responses API** (`POST /v1/responses`). Отдельный от
//! Chat Completions протокол того же вендора (ADR 0004,
//! docs/research/openai-responses-client.md): резюме рассуждений
//! (`reasoning.summary`), глубина рассуждения (`reasoning.effort`), многословность
//! (`text.verbosity`), reasoning-элементы с `encrypted_content` для tool-use.
//!
//! `wire` — сборка тела запроса и разбор событийного SSE; `client` — реализация
//! [`EngineBackend`](crate::shared::api::contract::EngineBackend).

mod client;
mod wire;

pub use client::ResponsesClient;
