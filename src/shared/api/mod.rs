//! Inference engine layer (`shared/api`). A provider-agnostic contract
//! ([`EngineBackend`]/[`Embedder`] in [`contract`]) and its implementations, grouped
//! by family: [`openai`] (local/external `llama-server`, OpenAI cloud via
//! Responses), [`anthropic`] (Claude, Messages API), [`gemini`] (Gemini, native
//! generateContent), [`managed`] (launching a child `llama-server`).
//! See spec §6 and [ADR 0004](../../../docs/decisions/0004-engine-contract-multi-provider.md).

pub mod anthropic;
pub mod contract;
pub mod gemini;
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
pub use gemini::GeminiClient;
pub use managed::{ManagedConfig, ServerHandle, wait_until_ready};
pub use openai::{OpenAiClient, ResponsesClient};
