//! The model port over the chat's own `EngineBackend` (docs/fabula.md §3;
//! docs/research/fabula-engine.md §8, §14 *Integration work*): every attempt
//! goes to the model and into the call log, under its address.
//!
//! - **Schema-constrained where the engine can, the fallback elsewhere.** A
//!   JSON call sends its schema as `ChatRequest::output_schema` when the
//!   backend says it enforces one (`EngineBackend::enforces_output_schema`,
//!   asked once per adapter), and the call log records `schema`; otherwise it
//!   records `fallback` (spec §8). The prompt carries the schema either way
//!   ([`super::port::system_with_schema`]), the reply is read leniently and
//!   validated, and an invalid one is repaired once.
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
    ApiMessage, ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, OutputSchema,
    Served, TokenUsage,
};
use crate::shared::storage::story::{CallStatus, CallUsage, Mechanism, NewCall, RunLogWriter};

/// The run log's capability, shared between the adapter, which records the
/// calls, and the chapter's loop, which records the steps (spec §9).
pub type SharedLog = Arc<Mutex<RunLogWriter>>;

/// The port over a live engine.
pub struct EngineAdapter {
    backend: Arc<dyn EngineBackend>,
    log: SharedLog,
    model: String,
    sampling: SamplingConfig,
    /// A role's sampling in place of `sampling`: the characters' with
    /// thinking off, the World Master's with it on (spec §8, *Thinking is a
    /// role's setting*).
    roles: Vec<(Role, SamplingConfig)>,
    cancel: CancellationToken,
    /// The backend's answer to `enforces_output_schema`, asked once.
    enforces: tokio::sync::OnceCell<bool>,
    /// Never send the schema to the provider, whatever it can do.
    fallback_only: bool,
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
        Self::sharing(backend, Arc::new(Mutex::new(log)), model, sampling, cancel)
    }

    /// The same over a run log the caller shares: the chapter's loop
    /// records its steps through it while the adapter records the calls.
    pub fn sharing(
        backend: Arc<dyn EngineBackend>,
        log: SharedLog,
        model: impl Into<String>,
        sampling: SamplingConfig,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            backend,
            log,
            model: model.into(),
            sampling,
            roles: Vec::new(),
            cancel,
            enforces: tokio::sync::OnceCell::new(),
            fallback_only: false,
        }
    }

    /// The same adapter on the fallback mechanism alone: for an engine that
    /// says it constrains and must not be asked to — a managed `llama-server`
    /// with a draft model, whose grammar crashes under `json_schema` (docs/
    /// fabula.md §5) — and for measuring one mechanism against the other.
    pub fn fallback_only(mut self) -> Self {
        self.fallback_only = true;
        self
    }

    /// The same adapter with `sampling` for `role`'s calls.
    pub fn with_role_sampling(mut self, role: Role, sampling: SamplingConfig) -> Self {
        self.roles.retain(|(r, _)| *r != role);
        self.roles.push((role, sampling));
        self
    }

    /// The sampling `role`'s calls go out with.
    fn sampling_for(&self, role: Role) -> &SamplingConfig {
        self.roles
            .iter()
            .find(|(r, _)| *r == role)
            .map_or(&self.sampling, |(_, s)| s)
    }

    /// Whether a JSON call goes out schema-constrained.
    async fn constrained(&self) -> bool {
        !self.fallback_only
            && *self
                .enforces
                .get_or_init(|| self.backend.enforces_output_schema())
                .await
    }

    /// Gives the run log's capability back; none while another holder
    /// shares it.
    pub fn into_log(self) -> Option<RunLogWriter> {
        Arc::try_unwrap(self.log).ok().map(Mutex::into_inner)
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
        let constrained = self.constrained().await;
        let output_schema = constrained.then(|| {
            OutputSchema::new(role.as_str(), schema.to_json())
                .expect("a printed schema is one JSON value")
        });
        let mut live = Live {
            adapter: self,
            call,
            role,
            mechanism: if constrained {
                Mechanism::Schema
            } else {
                Mechanism::Fallback
            },
            output_schema,
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
            output_schema: None,
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
    /// Sent with every attempt when the provider constrains the decoding.
    output_schema: Option<OutputSchema>,
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
            sampling: adapter.sampling_for(self.role).clone(),
            output_schema: self.output_schema.clone(),
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
        // What the provider counted of the thinking, where it counts it apart:
        // a thought it does not show is still billed (docs/journal/
        // stories.md, PR 5's finding on Gemini 3.x).
        response["reasoning_tokens"] = json!(reply.usage.map(|u| u.reasoning_tokens));
        response["served_by"] = json!(reply.served.as_ref().and_then(|s| s.provider.clone()));
        let record = NewCall {
            run: self.call.run.clone(),
            address: self.call.address.clone(),
            attempt: attempt.number,
            role: self.role.as_str().to_owned(),
            model: adapter.model.clone(),
            params: serde_json::to_value(adapter.sampling_for(self.role)).unwrap_or(Value::Null),
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
