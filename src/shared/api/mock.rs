//! Mock-реализация [`EngineBackend`] для тестов оркестратора (без сервера/сети).

use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::contract::{ChatChunk, ChatRequest, ChatStream, Embedder, EngineBackend, FinishReason};

/// Скриптованный движок: проигрывает заранее заданные фрагменты. Поддерживает
/// **последовательность** скриптов по вызовам (для раундов agentic-loop).
pub struct MockBackend {
    /// Очередь скриптов: один на каждый вызов `chat_stream`.
    scripts: Mutex<VecDeque<Vec<ChatChunk>>>,
    /// После проигрывания скрипта ждать отмены и завершить `Cancelled`
    /// (имитация длинной генерации).
    wait_for_cancel: bool,
}

impl MockBackend {
    /// Движок, проигрывающий фрагменты и завершающийся ими (`script` должен
    /// содержать терминальный [`ChatChunk::Finished`]). Один и тот же скрипт на
    /// каждый вызов.
    pub fn scripted(script: Vec<ChatChunk>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from([script])),
            wait_for_cancel: false,
        }
    }

    /// Движок, отдающий разные скрипты по очереди (по одному на вызов
    /// `chat_stream`) — для проверки раундов agentic-loop. После исчерпания
    /// очереди отдаёт пустой ход (Finished Stop).
    pub fn sequence(scripts: Vec<Vec<ChatChunk>>) -> Self {
        Self {
            scripts: Mutex::new(VecDeque::from(scripts)),
            wait_for_cancel: false,
        }
    }

    /// Движок, который проигрывает `prefix`, затем «висит» до отмены.
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
        // Для scripted очередь не исчерпываем (повторяем последний скрипт);
        // для sequence — берём следующий по порядку.
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
