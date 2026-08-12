//! Inference engine layer (`shared/api`). A provider-agnostic contract
//! ([`EngineBackend`]/[`Embedder`] in [`contract`]) and its implementations, grouped
//! by family: [`openai`] (local/external `llama-server`, OpenAI cloud via
//! Responses), [`anthropic`] (Claude, Messages API), [`gemini`] (Gemini, native
//! generateContent), [`managed`] (launching a child `llama-server`).
//! See spec §6 and [ADR 0004](../../../docs/decisions/0004-engine-contract-multi-provider.md).

pub mod anthropic;
pub mod contract;
pub mod error;
pub mod gemini;
pub mod http;
pub mod managed;
pub mod openai;
pub mod retry;
pub mod thoughts;

// Production since the demo mode (`mindfork demo`) boots on it; tests were
// the original and remain the main consumer.
pub mod mock;

pub use anthropic::AnthropicClient;
pub use contract::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EmbedRole, Embedder, EngineBackend,
    FinishReason, ThinkingBlock, ThinkingRef, ToolCallAccumulator, ToolSchema, UnavailableEmbedder,
};
pub use gemini::GeminiClient;
pub use managed::{ManagedConfig, ServerHandle, wait_until_ready};
pub use openai::{OpenAiClient, ResponsesClient};

/// A client to a live OpenAI-compatible server for the `#[ignore]` smokes,
/// named by a pair of env variables: the URL and an **optional** Bearer key.
///
/// `None` when the URL variable is unset — the smoke skips, as before. An unset
/// or empty key variable sends no `Authorization` header, i.e. byte-for-byte the
/// previous behaviour against a local `llama-server`; setting it lets the same
/// smokes run against an authenticated server (a hosted endpoint, a proxy). See
/// [docs/history/remote-e2e-hf.md](../../../docs/history/remote-e2e-hf.md) §5.
#[cfg(test)]
pub(crate) fn live_client(url_var: &str, key_var: &str) -> Option<OpenAiClient> {
    let url = std::env::var(url_var).ok()?;
    Some(OpenAiClient::new(url).with_api_key(std::env::var(key_var).ok()))
}
