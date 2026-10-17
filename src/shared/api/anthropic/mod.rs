//! Anthropic (Claude) inference backend — a separate Messages API protocol
//! (`/v1/messages`). Implements [`EngineBackend`](super::contract::EngineBackend)
//! via [`AnthropicClient`]. See ADR 0004, Phase 2.

#[cfg(test)]
mod cache_probe_tests;
pub mod client;
#[cfg(test)]
mod live_tests;
mod wire;

pub(crate) use client::ANTHROPIC_VERSION;
pub use client::AnthropicClient;
