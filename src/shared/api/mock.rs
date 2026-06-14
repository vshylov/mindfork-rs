//! Mock-реализация [`EngineBackend`] для тестов оркестратора (без сервера/сети).

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::backend::{ChatChunk, ChatRequest, ChatStream, Embedder, EngineBackend, FinishReason};

/// Скриптованный движок: проигрывает заранее заданные фрагменты.
pub struct MockBackend {
    script: Vec<ChatChunk>,
    /// После проигрывания `script` ждать отмены и завершить `Cancelled`
    /// (имитация длинной генерации).
    wait_for_cancel: bool,
}

impl MockBackend {
    /// Движок, проигрывающий фрагменты и завершающийся ими (`script` должен
    /// содержать терминальный [`ChatChunk::Finished`]).
    pub fn scripted(script: Vec<ChatChunk>) -> Self {
        Self {
            script,
            wait_for_cancel: false,
        }
    }

    /// Движок, который проигрывает `prefix`, затем «висит» до отмены.
    pub fn cancellable(prefix: Vec<ChatChunk>) -> Self {
        Self {
            script: prefix,
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
        let script = self.script.clone();
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

/// Детерминированный in-process эмбеддер для тестов: bag-of-chars в `dim`
/// измерений с L2-нормализацией (близкие тексты дают близкие векторы).
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
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
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
