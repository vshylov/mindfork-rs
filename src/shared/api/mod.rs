//! Слой движка инференса (`shared/api`). Провайдеро-агностичный контракт
//! ([`EngineBackend`]/[`Embedder`] в [`contract`]) и его реализации, сгруппированные
//! по семействам: [`openai`] (локальный/external `llama-server`, облако OpenAI/Gemini),
//! [`anthropic`] (Claude, Messages API), [`managed`] (запуск дочернего `llama-server`).
//! См. spec §6 и [ADR 0004](../../../docs/decisions/0004-engine-contract-multi-provider.md).

pub mod anthropic;
pub mod contract;
pub mod managed;
pub mod openai;
pub mod thoughts;

#[cfg(test)]
pub mod mock;

pub use anthropic::AnthropicClient;
pub use contract::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, Embedder, EngineBackend, FinishReason,
    ThinkingBlock, ThinkingRef, ToolCallAccumulator, ToolSchema, UnavailableEmbedder,
};
pub use managed::{ManagedConfig, ServerHandle, wait_until_ready};
pub use openai::{OpenAiClient, ResponsesClient, WireDialect};
