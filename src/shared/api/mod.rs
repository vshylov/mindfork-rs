//! Слой движка инференса (`shared/api`): контракт [`EngineBackend`], HTTP-клиент
//! к серверу xinfer и его супервайзер. См. spec §6, docs/xinfer-contract.md.

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
pub use client::XinferClient;
pub use server::{ManagedConfig, ServerHandle, wait_until_ready};
