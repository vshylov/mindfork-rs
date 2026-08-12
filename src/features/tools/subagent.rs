//! `call_subagent` tool (spec §9.3.2): get an **alternative opinion**.
//!
//! The main agent sets the subagent's system message and single user message
//! itself. The subagent is an **independent single-turn** request to the same
//! model via `ctx.engine`: **no chat history, no tools, no nesting** (a
//! recursion ban — the subagent isn't given any tool at all, including
//! `call_subagent`). Subject to token and time limits.

use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest};

use super::{Tool, ToolContext, ToolOutcome};

/// Default token limit for the subagent's reply (protection against long/looping ones).
const DEFAULT_SUBAGENT_MAX_TOKENS: usize = 1024;
/// Default time limit for a single subagent call.
const DEFAULT_SUBAGENT_TIMEOUT: Duration = Duration::from_secs(60);

/// `call_subagent` — an independent single-turn request for an alternative opinion.
/// Token/time limits are configurable (`config.tools`, spec §11.6).
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
    /// Creates the tool with the given token/time limits.
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
        "subagent request"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.call_subagent.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "system_message": {
                    "type": "string",
                    "description": loc.t("tool.call_subagent.param.system_message")
                },
                "message": {
                    "type": "string",
                    "description": loc.t("tool.call_subagent.param.message")
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
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.call_subagent.err.message_empty")))?
            .to_string();

        // A token limit on top of the effective sampling; NO tools and no history.
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
            tools: Vec::new(), // a nesting ban: no tools at all
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
                    // "Thoughts", tool deltas, and the subagent's token counter are ignored.
                    ChatChunk::Error { message, .. } => {
                        tracing::warn!(error = %message, "engine error in a sub-agent call");
                    }
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
            Ok(Ok(_)) => Ok(ToolOutcome::text(
                ctx.loc.t("tool.call_subagent.result.empty"),
            )),
            Ok(Err(err)) => Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.call_subagent.result.error",
                &[("err", &err.to_string())],
            ))),
            Err(_) => {
                cancel.cancel();
                Ok(ToolOutcome::text(
                    ctx.loc.t("tool.call_subagent.result.timeout"),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::contract::{ChatStream, EngineBackend, FinishReason};
    use crate::shared::api::{Embedder, mock::MockEmbedder};
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    /// An engine that remembers the last request and returns a fixed reply.
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
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(16));
        let (dir, _storage, ctx) =
            super::super::testkit::ctx_with_backends(Uuid::new_v4(), engine, embedder);
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

        // The subagent's request: the given system, a single user message, no tools.
        let req = backend.last.lock().unwrap().take().unwrap();
        assert_eq!(req.system.as_deref(), Some("Ты — критик."));
        assert_eq!(req.messages.len(), 1);
        assert!(req.tools.is_empty(), "the subagent must not be given tools");
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
