//! The model port over the chat's own `EngineBackend` (docs/fabula.md §3;
//! docs/research/fabula-engine.md §8, §14 *Integration work*): every attempt
//! goes to the model and into the call log, under its address.
//!
//! - **The fallback mechanism only, for now.** `ChatRequest` has no field for
//!   an output schema until PR 3 of docs/fabula.md §4, so a JSON call puts the
//!   schema into the prompt, reads the reply leniently and repairs it once
//!   (spec §8, *The fallback*). The call log says so: `mechanism` is
//!   `fallback`.
//! - **Every attempt is recorded**, the failed ones too, with what the
//!   provider reported of its cost: what replay serves, what M1 measures and
//!   what T1's budget counts.
//! - **The answer's text, not its thoughts.** A thinking channel is the
//!   model's own, and only its length reaches the log.
//! - **The backend's own decorators stay.** A transient failure that
//!   `RetryBackend` absorbs never reaches the policy; one it gives up on does,
//!   and the policy tries once more (spec §8, *What still breaks a reply*).

use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::port::{
    Answer, Attempt, Attempts, CallAddress, LlmPort, PortError, Prompt, Role, prompt_sha256,
    run_json, run_text,
};
use super::schema::Schema;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, Served, TokenUsage,
};
use crate::shared::storage::story::{CallStatus, CallUsage, Mechanism, NewCall, RunLogWriter};

/// The port over a live engine.
pub struct EngineAdapter {
    backend: Arc<dyn EngineBackend>,
    log: Mutex<RunLogWriter>,
    model: String,
    sampling: SamplingConfig,
    cancel: CancellationToken,
}

impl EngineAdapter {
    /// An adapter that sends every call to `backend` with `sampling`, records
    /// it through `log` under the name `model`, and stops when `cancel` fires.
    pub fn new(
        backend: Arc<dyn EngineBackend>,
        log: RunLogWriter,
        model: impl Into<String>,
        sampling: SamplingConfig,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            backend,
            log: Mutex::new(log),
            model: model.into(),
            sampling,
            cancel,
        }
    }

    /// Gives the run log's capability back.
    pub fn into_log(self) -> RunLogWriter {
        self.log.into_inner()
    }
}

#[async_trait::async_trait]
impl LlmPort for EngineAdapter {
    async fn json(
        &self,
        call: &CallAddress,
        role: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Result<Value, PortError> {
        let mut live = Live {
            adapter: self,
            call,
            role,
            mechanism: Mechanism::Fallback,
            prompt_sha256: prompt_sha256(prompt, Some(schema)),
        };
        run_json(&mut live, &call.address, schema, prompt).await
    }

    async fn text(
        &self,
        call: &CallAddress,
        role: Role,
        prompt: &Prompt,
    ) -> Result<String, PortError> {
        let mut live = Live {
            adapter: self,
            call,
            role,
            mechanism: Mechanism::Text,
            prompt_sha256: prompt_sha256(prompt, None),
        };
        run_text(&mut live, &call.address, prompt).await
    }
}

/// One call's attempts against the engine.
struct Live<'a> {
    adapter: &'a EngineAdapter,
    call: &'a CallAddress,
    role: Role,
    mechanism: Mechanism,
    prompt_sha256: String,
}

#[async_trait::async_trait]
impl Attempts for Live<'_> {
    async fn attempt(&mut self, attempt: &Attempt) -> Result<Answer, PortError> {
        let adapter = self.adapter;
        let request = ChatRequest {
            system: Some(attempt.system.clone()),
            messages: attempt
                .turns
                .iter()
                .map(|t| match t.role {
                    "assistant" => ApiMessage::assistant(t.content.clone()),
                    _ => ApiMessage::user(t.content.clone()),
                })
                .collect(),
            sampling: adapter.sampling.clone(),
            ..ChatRequest::default()
        };
        let started = Instant::now();
        let stream = adapter
            .backend
            .chat_stream(request, adapter.cancel.child_token())
            .await;
        let reply = collect(stream).await;
        let latency_ms = u64::try_from(started.elapsed().as_millis()).ok();
        let answer = reply.answer();
        tracing::info!(
            address = %self.call.address,
            attempt = attempt.number,
            status = ?answer.status,
            chars = answer.text.len(),
            thought_chars = reply.thought_chars,
            "story call"
        );
        let mut response = answer.to_json();
        response["thought_chars"] = json!(reply.thought_chars);
        response["served_by"] = json!(reply.served.as_ref().and_then(|s| s.provider.clone()));
        let record = NewCall {
            run: self.call.run.clone(),
            address: self.call.address.clone(),
            attempt: attempt.number,
            role: self.role.as_str().to_owned(),
            model: adapter.model.clone(),
            params: serde_json::to_value(&adapter.sampling).unwrap_or(Value::Null),
            mechanism: self.mechanism,
            request: attempt.to_json(),
            prompt_sha256: self.prompt_sha256.clone(),
            response: Some(response),
            status: answer.status,
            usage: reply.usage(),
            latency_ms,
        };
        adapter.log.lock().await.record_call(record).await?;
        Ok(answer)
    }
}

/// What one streamed reply brought.
#[derive(Default)]
struct Collected {
    text: String,
    thought_chars: usize,
    finish: Option<FinishReason>,
    error: Option<(String, bool)>,
    usage: Option<TokenUsage>,
    served: Option<Served>,
}

async fn collect(stream: anyhow::Result<ChatStream>) -> Collected {
    let mut reply = Collected::default();
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(e) => {
            // The decorators below the adapter retried what could be retried.
            reply.error = Some((format!("{e:#}"), false));
            return reply;
        }
    };
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => reply.text.push_str(&t),
            ChatChunk::Thoughts(t) => reply.thought_chars += t.chars().count(),
            ChatChunk::Usage(u) => reply.usage = Some(u),
            ChatChunk::Served(s) => reply.served = Some(s),
            ChatChunk::Error { message, transient } => reply.error = Some((message, transient)),
            ChatChunk::Finished(reason) => {
                reply.finish = Some(reason);
                break;
            }
            ChatChunk::ThoughtsSignature(_) | ChatChunk::ToolCall(_) | ChatChunk::Retry { .. } => {}
        }
    }
    reply
}

impl Collected {
    fn answer(&self) -> Answer {
        let failed = |status, error: &str, transient| (status, Some(error.to_owned()), transient);
        let (status, error, transient) = match (&self.error, self.finish) {
            (Some((message, transient)), _) => {
                (CallStatus::Error, Some(message.clone()), *transient)
            }
            (None, Some(FinishReason::Stop | FinishReason::ToolCalls)) => {
                (CallStatus::Ok, None, false)
            }
            (None, Some(FinishReason::Length)) => (CallStatus::Cut, None, false),
            (None, Some(FinishReason::Filtered)) => (CallStatus::Refused, None, false),
            (None, Some(FinishReason::Cancelled)) => failed(CallStatus::Error, "cancelled", false),
            (None, Some(FinishReason::Error)) => failed(
                CallStatus::Error,
                "the provider ended the reply with an error",
                true,
            ),
            (None, None) => failed(
                CallStatus::Error,
                "the stream ended before the reply finished",
                true,
            ),
        };
        Answer {
            status,
            text: self.text.clone(),
            error,
            transient,
            cancelled: self.finish == Some(FinishReason::Cancelled),
        }
    }

    fn usage(&self) -> CallUsage {
        CallUsage {
            input_tokens: self.usage.map(|u| u64::from(u.prompt_tokens)),
            output_tokens: self.usage.map(|u| u64::from(u.completion_tokens)),
            cached_tokens: self.usage.and_then(|u| u.cached_tokens).map(u64::from),
            cost_usd: self
                .served
                .as_ref()
                .and_then(|s| s.cost_nanos)
                .map(|n| n as f64 / 1e9),
        }
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
