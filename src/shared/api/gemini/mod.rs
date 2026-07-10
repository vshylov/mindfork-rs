//! Бэкенд облака Google Gemini через **нативный** API (`generateContent`/
//! `streamGenerateContent`). Отдельный от OpenAI-совместимого Chat Completions протокол
//! (ADR 0004, docs/research/gemini-native-client.md): резюме «мыслей»
//! (`thinkingConfig.includeThoughts`), глубина рассуждений (`thinkingLevel`/
//! `thinkingBudget`), `thoughtsTokenCount`.
//!
//! `wire` — сборка тела запроса и разбор SSE; `client` — реализация
//! [`EngineBackend`](crate::shared::api::contract::EngineBackend). Эмбеддинги Gemini
//! берутся отдельно через OpenAI-совместимый endpoint (`OpenAiClient`), см. супервайзер.

mod client;
mod wire;

pub use client::GeminiClient;
