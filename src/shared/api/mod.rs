//! Слой движка инференса (`shared/api`): контракт [`EngineBackend`], HTTP-клиент
//! к OpenAI-совместимому серверу и запуск managed `llama-server` (llama.cpp).
//! См. spec §6.

pub mod backend;
pub mod client;
pub mod server;
pub mod thoughts;
mod wire;

#[cfg(test)]
pub mod mock;

pub use backend::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, Embedder, EngineBackend, FinishReason,
    ToolCallAccumulator, ToolSchema, UnavailableEmbedder,
};
pub use client::OpenAiClient;
pub use server::{ManagedConfig, ServerHandle, wait_until_ready};
