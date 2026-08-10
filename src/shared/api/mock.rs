//! Mock implementation of [`EngineBackend`] for orchestrator tests and the
//! demo mode (`mindfork demo`) — no server, no network.

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
    /// Rotate scripts instead of consuming them (`cycling`): after the last
    /// one, the first plays again — the demo engine never runs dry.
    cycle: bool,
    /// Pause between chunks during playback, in milliseconds. `0` (tests)
    /// plays back instantly; the demo mode uses a small delay so streaming is
    /// visible rather than instantaneous.
    delay_ms: u64,
}

impl MockBackend {
    /// An engine that plays back fragments and finishes with them (`script` must
    /// contain a terminal [`ChatChunk::Finished`]). The same script on every call.
    /// Test-only, like `sequence` and `cancellable`: the demo mode ships
    /// `cycling`, and a constructor nothing outside tests calls should not
    /// ride in the release binary.
    #[cfg(test)]
    pub fn scripted(script: Vec<ChatChunk>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from([script])),
            wait_for_cancel: false,
            cycle: false,
            delay_ms: 0,
        }
    }

    /// An engine that hands out different scripts in order (one per `chat_stream`
    /// call) — for testing agentic-loop rounds. Once the queue is exhausted, gives an
    /// empty turn (Finished Stop).
    #[cfg(test)]
    pub fn sequence(scripts: Vec<Vec<ChatChunk>>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from(scripts)),
            wait_for_cancel: false,
            cycle: false,
            delay_ms: 0,
        }
    }

    /// An engine that plays back `prefix`, then "hangs" until cancelled.
    #[cfg(test)]
    pub fn cancellable(prefix: Vec<ChatChunk>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from([prefix])),
            wait_for_cancel: true,
            cycle: false,
            delay_ms: 0,
        }
    }

    /// The demo mode's engine: cycles through `scripts` endlessly (after the
    /// last, the first again) with `delay_ms` between chunks — every reply
    /// differs, none runs out, and streaming stays visible.
    pub fn cycling(scripts: Vec<Vec<ChatChunk>>, delay_ms: u64) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from(scripts)),
            wait_for_cancel: false,
            cycle: true,
            delay_ms,
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
        // For scripted, don't drain the queue (repeat the last script); for
        // sequence — take the next one in order; for cycling — rotate.
        let script = {
            let mut q = self.scripts.lock().unwrap();
            if self.cycle {
                let s = q.pop_front().unwrap_or_default();
                q.push_back(s.clone());
                s
            } else if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                q.front().cloned().unwrap_or_default()
            }
        };
        let wait = self.wait_for_cancel;
        let delay = self.delay_ms;
        let s = async_stream::stream! {
            for chunk in script {
                if cancel.is_cancelled() {
                    yield ChatChunk::Finished(FinishReason::Cancelled);
                    return;
                }
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
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
