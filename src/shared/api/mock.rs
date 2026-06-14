//! Mock-реализация [`EngineBackend`] для тестов оркестратора (без сервера/сети).

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::backend::{ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason};

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

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        Ok(texts.into_iter().map(|_| vec![0.0_f32; 4]).collect())
    }
}
