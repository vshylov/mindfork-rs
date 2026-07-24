//! Anthropic (Claude) inference backend — a separate Messages API protocol
//! (`/v1/messages`). Implements [`EngineBackend`](super::contract::EngineBackend)
//! via [`AnthropicClient`]. See ADR 0004, Phase 2.

pub mod client;
mod wire;

pub use client::AnthropicClient;
