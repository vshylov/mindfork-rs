//! Инструмент `call_subagent` (spec §9.3.2): получить **альтернативное мнение**.
//!
//! Основной агент сам задаёт саб-агенту системное сообщение и единственное
//! пользовательское сообщение. Саб-агент — **независимый одно-ходовый** запрос к
//! той же модели через `ctx.engine`: **без истории чата, без инструментов и без
//! вложенности** (запрет рекурсии — саб-агенту не передаётся ни один инструмент,
//! включая `call_subagent`). Подчиняется лимитам токенов и времени.

use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest};

use super::{Tool, ToolContext, ToolOutcome};

/// Лимит токенов ответа саб-агента по умолчанию (защита от длинных/зацикленных).
const DEFAULT_SUBAGENT_MAX_TOKENS: usize = 1024;
/// Лимит времени на один вызов саб-агента по умолчанию.
const DEFAULT_SUBAGENT_TIMEOUT: Duration = Duration::from_secs(60);

/// `call_subagent` — независимый одно-ходовый запрос для альтернативного мнения.
/// Лимиты токенов/времени настраиваются (`config.tools`, spec §11.6).
pub struct CallSubagent {
    max_tokens: usize,
    timeout: Duration,
}

impl Default for CallSubagent {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_SUBAGENT_MAX_TOKENS,
            timeout: DEFAULT_SUBAGENT_TIMEOUT,
        }
    }
}

impl CallSubagent {
    /// Создаёт инструмент с заданными лимитами токенов/времени.
    pub fn new(max_tokens: usize, timeout: Duration) -> Self {
        Self {
            max_tokens,
            timeout,
        }
    }
}

#[async_trait::async_trait]
impl Tool for CallSubagent {
    fn id(&self) -> ToolId {
        "call_subagent".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Subagent
    }
    fn ui_label(&self) -> &'static str {
        "запрос суб-агенту"
    }
    fn description(&self) -> String {
        "Спросить независимого саб-агента (с заданным ему системным сообщением) для \
         альтернативного мнения. У саб-агента нет истории этого чата и нет инструментов."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "system_message": {
                    "type": "string",
                    "description": "Роль/инструкция для саб-агента"
                },
                "message": {
                    "type": "string",
                    "description": "Единственное сообщение саб-агенту"
                }
            },
            "required": ["system_message", "message"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let system_message = args
            .get("system_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле message"))?
            .to_string();

        // Лимит токенов поверх действующего семплинга; БЕЗ инструментов и истории.
        let sampling = SamplingConfig {
            max_tokens: Some(
                ctx.effective_sampling
                    .max_tokens
                    .map_or(self.max_tokens, |m| m.min(self.max_tokens)),
            ),
            ..ctx.effective_sampling.clone()
        };
        let request = ChatRequest {
            system: (!system_message.is_empty()).then(|| system_message.clone()),
            messages: vec![ApiMessage::user(message)],
            sampling,
            tools: Vec::new(), // запрет вложенности: никаких инструментов
        };

        let cancel = CancellationToken::new();
        let engine = ctx.engine.clone();
        let collect = async {
            let mut stream = engine.chat_stream(request, cancel.clone()).await?;
            let mut text = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    ChatChunk::Finished(_) => break,
                    // «Мысли», tool-дельты и счётчик токенов саб-агента игнорируем.
                    ChatChunk::Thoughts(_)
                    | ChatChunk::ThoughtsSignature(_)
                    | ChatChunk::ToolCall(_)
                    | ChatChunk::Usage(_) => {}
                }
            }
            Ok::<String, anyhow::Error>(text)
        };

        match tokio::time::timeout(self.timeout, collect).await {
            Ok(Ok(text)) if !text.trim().is_empty() => Ok(ToolOutcome::text(text)),
            Ok(Ok(_)) => Ok(ToolOutcome::text("(саб-агент вернул пустой ответ)")),
            Ok(Err(err)) => Ok(ToolOutcome::text(format!("Ошибка саб-агента: {err}"))),
            Err(_) => {
                cancel.cancel();
                Ok(ToolOutcome::text("Саб-агент превысил лимит времени."))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::contract::{ChatStream, EngineBackend, FinishReason};
    use crate::shared::api::{Embedder, mock::MockEmbedder};
    use crate::shared::paths::Paths;
    use crate::shared::storage::Storage;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    /// Движок, запоминающий последний запрос и отдающий фиксированный ответ.
    struct CapturingBackend {
        last: Mutex<Option<ChatRequest>>,
        reply: String,
    }

    #[async_trait::async_trait]
    impl EngineBackend for CapturingBackend {
        async fn chat_stream(
            &self,
            req: ChatRequest,
            _cancel: CancellationToken,
        ) -> Result<ChatStream> {
            *self.last.lock().unwrap() = Some(req);
            let reply = self.reply.clone();
            let s = async_stream::stream! {
                yield ChatChunk::Text(reply);
                yield ChatChunk::Finished(FinishReason::Stop);
            };
            Ok(Box::pin(s))
        }
    }

    fn ctx_with_engine(engine: Arc<dyn EngineBackend>) -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(16));
        let ctx = ToolContext {
            profile_id: Uuid::new_v4(),
            chat_id: Uuid::new_v4(),
            system_message: "основной системный промпт".into(),
            effective_sampling: SamplingConfig::default(),
            last_user_message_at: None,
            storage,
            engine,
            embedder,
            chunk_params: crate::features::tools::rag::ChunkParams::default(),
            self_model_params: crate::entities::self_model::SelfModelParams::default(),
            recall_includes_self: false,
        };
        (dir, ctx)
    }

    #[tokio::test]
    async fn subagent_runs_without_tools_history_and_returns_text() {
        let backend = Arc::new(CapturingBackend {
            last: Mutex::new(None),
            reply: "альтернативное мнение".into(),
        });
        let (_d, ctx) = ctx_with_engine(backend.clone());

        let out = CallSubagent::default()
            .invoke(
                &ctx,
                serde_json::json!({
                    "system_message": "Ты — критик.",
                    "message": "Оцени идею X."
                }),
            )
            .await
            .unwrap();
        assert_eq!(out.result, "альтернативное мнение");
        assert!(out.effects.is_empty());

        // Запрос саб-агента: заданный system, единственное user-сообщение, без инструментов.
        let req = backend.last.lock().unwrap().take().unwrap();
        assert_eq!(req.system.as_deref(), Some("Ты — критик."));
        assert_eq!(req.messages.len(), 1);
        assert!(
            req.tools.is_empty(),
            "саб-агенту нельзя передавать инструменты"
        );
        assert!(req.sampling.max_tokens.unwrap() <= DEFAULT_SUBAGENT_MAX_TOKENS);
    }

    #[tokio::test]
    async fn subagent_rejects_empty_message() {
        let backend = Arc::new(CapturingBackend {
            last: Mutex::new(None),
            reply: String::new(),
        });
        let (_d, ctx) = ctx_with_engine(backend);
        assert!(
            CallSubagent::default()
                .invoke(
                    &ctx,
                    serde_json::json!({"system_message": "x", "message": "  "})
                )
                .await
                .is_err()
        );
    }
}
