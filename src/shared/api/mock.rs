//! Mock implementation of [`EngineBackend`] for orchestrator tests (no server/network).

use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::contract::{
    ChatChunk, ChatRequest, ChatStream, EmbedRole, Embedder, EngineBackend, FinishReason,
};

/// A scripted engine: plays back preset fragments. Supports a **sequence** of
/// scripts across calls (for agentic-loop rounds).
pub struct MockBackend {
    /// A queue of scripts: one per `chat_stream` call.
    scripts: Mutex<VecDeque<Vec<ChatChunk>>>,
    /// After playing the script, wait for cancellation and finish `Cancelled`
    /// (simulates a long generation).
    wait_for_cancel: bool,
}

impl MockBackend {
    /// An engine that plays back fragments and finishes with them (`script` must
    /// contain a terminal [`ChatChunk::Finished`]). The same script on every call.
    pub fn scripted(script: Vec<ChatChunk>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from([script])),
            wait_for_cancel: false,
        }
    }

    /// An engine that hands out different scripts in order (one per `chat_stream`
    /// call) — for testing agentic-loop rounds. Once the queue is exhausted, gives an
    /// empty turn (Finished Stop).
    pub fn sequence(scripts: Vec<Vec<ChatChunk>>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from(scripts)),
            wait_for_cancel: false,
        }
    }

    /// An engine that plays back `prefix`, then "hangs" until cancelled.
    pub fn cancellable(prefix: Vec<ChatChunk>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from([prefix])),
            wait_for_cancel: true,
        }
    }
}

#[async_trait::async_trait]
impl EngineBackend for MockBackend {
    async fn chat_stream(
        &self,
        _req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream> {
        // For scripted, don't drain the queue (repeat the last script);
        // for sequence — take the next one in order.
        let script = {
            let mut q = self.scripts.lock().unwrap();
            if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                q.front().cloned().unwrap_or_default()
            }
        };
        let wait = self.wait_for_cancel;
        let s = async_stream::stream! {
            for chunk in script {
                if cancel.is_cancelled() {
                    yield ChatChunk::Finished(FinishReason::Cancelled);
                    return;
                }
                yield chunk;
            }
            if wait {
                cancel.cancelled().await;
                yield ChatChunk::Finished(FinishReason::Cancelled);
            }
        };
        Ok(Box::pin(s))
    }
}

/// A deterministic in-process embedder for tests: bag-of-chars in `dim`
/// dimensions with L2 normalization (similar texts give similar vectors).
pub struct MockEmbedder {
    dim: usize,
}

impl MockEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[async_trait::async_trait]
impl Embedder for MockEmbedder {
    // Role-blind on purpose: the marker is applied above by `PrefixedEmbedder`,
    // so a test that wires this directly sees exactly the text it passed in.
    async fn embed(&self, texts: Vec<String>, _role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| embed_text(t, self.dim)).collect())
    }
}

fn embed_text(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; dim];
    for ch in text.to_lowercase().chars().filter(|c| c.is_alphanumeric()) {
        v[(ch as usize) % dim] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}
