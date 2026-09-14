//! An HTTP client to an **OpenAI-compatible** server (llama.cpp `llama-server`, vLLM,
//! LM Studio, …), implementing [`EngineBackend`]. Streaming via SSE
//! (`/v1/chat/completions`), embeddings (`/v1/embeddings`). The protocol is OpenAI
//! (originally checked against docs/xinfer-contract.md; llama.cpp speaks the same).

use anyhow::{Context, Result, bail};
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire;
use crate::entities::sampling::ReasoningEffort;
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EmbedRole, Embedder, EngineBackend, FinishReason,
    ModelCapabilities, TokenUsage, ToolCallDelta, VisionSupport,
};
use crate::shared::api::error::{self, SUBJECT_EMBEDDER, SUBJECT_ENGINE};
use crate::shared::api::http;
use crate::shared::api::thoughts::{Piece, ThoughtsParser};

/// A client to an OpenAI-compatible inference server (local/external `llama-server`,
/// vLLM, LM Studio…; optionally a proxy with a Bearer key). Clouds now use
/// their own protocols (OpenAI → Responses, Gemini → native, Claude → Anthropic), so
/// there's no longer a body dialect — sampling is sent as-is (see ADR 0004). Also a source of
/// embeddings (`/v1/embeddings`) for local/cloud RAG.
pub struct OpenAiClient {
    http: reqwest::Client,
    /// The base URL with a `/v1` suffix, e.g. `http://127.0.0.1:8000/v1`.
    base_url: String,
    /// The API key for Bearer authentication (proxy/cloud embeddings). `None` — no
    /// header.
    api_key: Option<String>,
    /// The model name; substituted into the request body if set (cloud embeddings/
    /// a multi-model proxy require it, `llama-server` ignores it). The domain
    /// [`ChatRequest`] doesn't carry a model — it's a property of the backend.
    model: Option<String>,
    /// Send **no** `reasoning_effort` when the request asks for
    /// [`ReasoningEffort::None`](crate::entities::sampling::ReasoningEffort::None),
    /// instead of sending the literal `"none"`. See
    /// [`Self::with_effort_none_omitted`].
    omit_effort_none: bool,
    /// The same thing, learned at runtime: this server has **refused** a request
    /// to disable reasoning, so stop asking.
    ///
    /// Set by [`Self::chat_stream`] when a `400` says so, and never cleared — a
    /// model that must reason does not stop mid-session, and a server swapped
    /// behind the URL gets a new client anyway (`supervisor::apply`). An
    /// `AtomicBool` rather than a builder flag because this is discovered, not
    /// configured: nothing at construction time can know it
    /// (docs/research/openrouter-external.md §5, F2(a)).
    reasoning_off_refused: std::sync::atomic::AtomicBool,
}

impl OpenAiClient {
    /// A client to a local/external OpenAI-compatible server: no key, no model
    /// name (llama.cpp extensions are sent as-is).
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            http: http::engine_client(),
            base_url,
            api_key: None,
            model: None,
            omit_effort_none: false,
            reasoning_off_refused: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Sets the API key (Bearer). Builder-style.
    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.is_empty());
        self
    }

    /// Sets the model name (for cloud embeddings/a multi-model server).
    /// Builder-style.
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model.filter(|m| !m.is_empty());
        self
    }

    /// Omit `reasoning_effort` instead of sending `"none"`. Builder-style.
    ///
    /// `llama-server` reads `"none"` as "don't think", and the orchestrator relies
    /// on that for its auxiliary turns — title generation, compaction and
    /// impersonation all set [`ReasoningEffort::None`](crate::entities::sampling::ReasoningEffort::None)
    /// deliberately. xAI rejects the *value* outright ("This model does not support
    /// `reasoning_effort` value `none`"), so on a Grok backend those three
    /// background turns would fail with a `400` while ordinary chat kept working —
    /// a confusing failure to diagnose. Omitting the field is the same thing the
    /// Anthropic and Gemini wires already do (`ant_effort`/`gem_effort` map
    /// `None => None`); Grok simply reasons at its default depth instead.
    /// See docs/research/grok-xai-provider.md §2.3.
    pub fn with_effort_none_omitted(mut self, omit: bool) -> Self {
        self.omit_effort_none = omit;
        self
    }

    /// Adds a Bearer header if a key is set.
    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => rb.bearer_auth(key),
            None => rb,
        }
    }

    /// Checks the server's **readiness** for inference (not just "the port is open").
    ///
    /// llama.cpp binds the HTTP port right away, but while the model is loading (~seconds for
    /// large GGUFs) it responds `503 Loading model` on inference endpoints. So
    /// checking just for a response isn't enough — otherwise the `Ready` status would be set before
    /// readiness, and the very first request would fail with 503 (see §7). We try `/health`
    /// (at the root, outside `/v1`): `503` — still loading (not ready), `200` — ready, `404`
    /// (a server without `/health`) — treated as "alive and not loading" (ready).
    pub async fn probe(&self) -> Result<()> {
        let url = health_url(&self.base_url);
        // The key matters here: an authenticated server answers `/health` with
        // 401 without it, and 401 is not 503, so the probe would report "ready"
        // no matter what the key was — the supervisor smokes would pass even
        // with a broken one. No key set (a local `llama-server`) → no header,
        // exactly as before.
        let resp = self
            .auth(self.http.get(&url))
            .send()
            .await
            .with_context(|| format!("probing {url}"))?;
        if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            bail!("server is still loading the model (503)");
        }
        Ok(())
    }

    /// Fetches llama.cpp's `/props` — the server's own description of what it is
    /// running. One helper for both capability questions ([`EngineBackend::context_budget`]
    /// and [`EngineBackend::vision`]), so a server is asked in one shape and either
    /// answer can be added without a second endpoint.
    ///
    /// Any failure — a server without the endpoint (vLLM, LM Studio, a cloud proxy),
    /// a network error, a body that doesn't parse — is `None`, i.e. "cannot say".
    /// Logged at debug, since not answering is normal here.
    async fn props(&self) -> Option<Props> {
        let url = props_url(&self.base_url);
        let resp = match self.auth(self.http.get(&url)).send().await {
            Ok(r) if r.status().is_success() => r,
            other => {
                tracing::debug!(%url, ok = other.is_ok(), "no answer from /props");
                return None;
            }
        };
        match resp.json().await {
            Ok(p) => Some(p),
            Err(err) => {
                tracing::debug!(%url, error = %err, "/props did not parse");
                None
            }
        }
    }

    /// The catalogue entry for **the model this client is configured for**, or
    /// `None` when there is nothing to look up or nothing to find.
    ///
    /// Deliberately keyed on `self.model` and nothing else. A blank field means a
    /// single-model server, where `/props` already answers and the catalogue has
    /// no per-model row worth guessing at; a name that is not in the list means
    /// the endpoint routes differently than we assume, and inventing a neighbour
    /// would be worse than silence
    /// ([external-model-name.md](../../../../docs/research/external-model-name.md) §3
    /// makes the same call about the model's name).
    async fn catalogue_entry(&self) -> Option<wire::ModelEntry> {
        let wanted = self.model.as_deref()?;
        let url = format!("{}/models", self.base_url);
        let resp = match self.auth(self.http.get(&url)).send().await {
            Ok(r) if r.status().is_success() => r,
            other => {
                tracing::debug!(%url, ok = other.is_ok(), "no answer from /models");
                return None;
            }
        };
        let list: wire::ModelList = match resp.json().await {
            Ok(list) => list,
            Err(err) => {
                tracing::debug!(%url, error = %err, "/models did not parse");
                return None;
            }
        };
        list.data.into_iter().find(|m| m.id == wanted)
    }

    /// Fetches `GET /v1/models` and returns the ids it lists.
    ///
    /// The standard OpenAI-compatible catalogue endpoint — every server family
    /// this mode can point at answers it, unlike `/props`, which is llama.cpp's
    /// own. Any failure is `None` ("cannot say"), like [`Self::props`].
    async fn listed_models(&self) -> Option<Vec<String>> {
        let url = format!("{}/models", self.base_url);
        let resp = match self.auth(self.http.get(&url)).send().await {
            Ok(r) if r.status().is_success() => r,
            other => {
                tracing::debug!(%url, ok = other.is_ok(), "no answer from /models");
                return None;
            }
        };
        match resp.json::<wire::ModelList>().await {
            Ok(list) => Some(
                list.data
                    .into_iter()
                    .map(|m| m.id)
                    .filter(|id| !id.is_empty())
                    .collect(),
            ),
            Err(err) => {
                tracing::debug!(%url, error = %err, "/models did not parse");
                None
            }
        }
    }
}

impl OpenAiClient {
    /// Whether a `reasoning_effort` of `"none"` is dropped rather than sent:
    /// because the backend was built that way (xAI, [`Self::with_effort_none_omitted`])
    /// **or** because this server has already refused such a request.
    fn omit_effort_none(&self) -> bool {
        self.omit_effort_none
            || self
                .reasoning_off_refused
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// One attempt at the chat request. `Ok(None)` — the cancellation token
    /// fired before a response arrived (the caller answers with a cancelled
    /// stream, never an error).
    async fn send_chat(
        &self,
        req: &ChatRequest,
        omit_effort_none: bool,
        cancel: &CancellationToken,
    ) -> Result<Option<reqwest::Response>, error::EngineError> {
        let body = wire::build_chat_request(req, true, self.model.as_deref(), omit_effort_none);
        let url = format!("{}/chat/completions", self.base_url);
        // Cancellable: until this moved inside the token's reach, `Esc` could not
        // interrupt a request that had not yet produced a stream.
        let Some(response) =
            http::send_cancellable(self.auth(self.http.post(&url)).json(&body), cancel).await?
        else {
            return Ok(None);
        };
        // The error body is not swallowed (llama.cpp/OpenAI servers put the reason
        // in JSON); status, `Retry-After` and the text all come back typed — which
        // is also what lets the refusal below be recognised at all.
        error::check_status(SUBJECT_ENGINE, response)
            .await
            .map(Some)
    }

    /// Does this failure mean "this endpoint cannot turn reasoning off", for a
    /// request that asked it to?
    ///
    /// Measured on OpenRouter (docs/research/openrouter-external.md §8.1, M4):
    /// `reasoning_effort: "none"` against `deepseek/deepseek-r1` is answered
    /// `400 {"error":{"message":"Reasoning is mandatory for this endpoint and
    /// cannot be disabled."}}`. The silent turns — the title, the compaction roll
    /// and impersonation — are the only ones that ask, so on such a model they
    /// were the only ones failing, while ordinary chat worked: the same shape xAI
    /// produces by rejecting the *value* ([`Self::with_effort_none_omitted`]).
    ///
    /// Three conditions, and each rules out a way of being wrong:
    ///
    /// - the turn **asked** to mute reasoning, and the field was actually sent
    ///   (`omit_effort_none()` false) — otherwise re-sending changes nothing;
    /// - the status is `400`: a refusal of the request as written, not a rate
    ///   limit or an outage the retry decorator owns;
    /// - the message names reasoning **and** its disabling. Deliberately a pair of
    ///   substrings rather than the sentence: providers reword. A false positive
    ///   costs exactly one extra round trip — the second attempt meets the same
    ///   error and it surfaces unchanged — so the loose match is the safe side.
    fn should_stop_asking(&self, req: &ChatRequest, err: &error::EngineError) -> bool {
        if self.omit_effort_none()
            || req.sampling.reasoning_effort != Some(ReasoningEffort::None)
            || err.status != Some(400)
        {
            return false;
        }
        let message = err.message.to_lowercase();
        message.contains("reasoning")
            && (message.contains("disable") || message.contains("mandatory"))
    }
}

#[async_trait::async_trait]
impl EngineBackend for OpenAiClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let response = match self.send_chat(&req, self.omit_effort_none(), &cancel).await {
            Ok(Some(response)) => response,
            Ok(None) => return Ok(http::cancelled_stream()),
            // The one refusal worth answering rather than reporting: the server
            // says reasoning cannot be turned off here, and the turn *asked* for
            // it to be. Ask again without the field, once, and remember the
            // answer for this server — see `is_reasoning_off_refused`.
            Err(err) if self.should_stop_asking(&req, &err) => {
                tracing::info!(
                    reason = %err.message,
                    "the engine refuses to disable reasoning; re-sending without \
                     reasoning_effort, and not asking again on this server"
                );
                self.reasoning_off_refused
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                match self.send_chat(&req, true, &cancel).await {
                    Ok(Some(response)) => response,
                    Ok(None) => return Ok(http::cancelled_stream()),
                    Err(err) => return Err(err.into()),
                }
            }
            Err(err) => return Err(err.into()),
        };

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // A "thoughts" splitter in case of an inline <think> in content.
            let mut parser = ThoughtsParser::new();
            // The reason the model stopped, once a chunk has reported one. Held
            // until the stream terminates so a trailing `usage` chunk is not lost
            // (see the `finish_reason` arm below).
            let mut finish: Option<FinishReason> = None;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        yield ChatChunk::Finished(FinishReason::Cancelled);
                        break;
                    }
                    next = events.next() => {
                        match next {
                            None => {
                                for piece in parser.finish() { yield piece_to_chunk(piece); }
                                // A stream that ended without `[DONE]` still ends the
                                // turn — with the reason the model gave, if it gave one.
                                yield ChatChunk::Finished(finish.unwrap_or(FinishReason::Stop));
                                break;
                            }
                            Some(Err(err)) => {
                                // A dropped connection is the canonical retryable
                                // failure — and, until it was surfaced, the canonical
                                // silent truncation.
                                let message = error::chain_text(&err);
                                tracing::warn!(error = %message, "SSE stream error");
                                for chunk in ChatChunk::failure(message, true) { yield chunk; }
                                break;
                            }
                            Some(Ok(event)) => {
                                if event.data == "[DONE]" {
                                    for piece in parser.finish() { yield piece_to_chunk(piece); }
                                    yield ChatChunk::Finished(finish.unwrap_or(FinishReason::Stop));
                                    break;
                                }
                                // llama.cpp (and OpenAI-compatible proxies) can put an
                                // error object into an already-open 200 stream instead
                                // of a chunk (ggml-org/llama.cpp#14566). Asked *before*
                                // the chunk parse: every field of a chunk has a default,
                                // so `{"error":{…}}` deserializes as a chunk with no
                                // choices and no usage and used to be skipped in
                                // silence — the server's "Context size has been
                                // exceeded." to two colliding streams landed both as
                                // finished turns with a cut-off reply (measured,
                                // docs/research/admission-by-budget.md §7).
                                if let Some(e) = wire::parse_stream_error(&event.data) {
                                    tracing::warn!(
                                        name = %e.name,
                                        transient = e.transient,
                                        message = %e.message,
                                        "engine reported an error inside the stream"
                                    );
                                    for chunk in ChatChunk::failure(e.message, e.transient) { yield chunk; }
                                    break;
                                }
                                match serde_json::from_str::<wire::ChatCompletionChunk>(&event.data) {
                                    Ok(chunk) => {
                                        // The token counter (include_usage) arrives as a separate
                                        // chunk (with an empty choices) — emit it before parsing choice.
                                        if let Some(u) = chunk.usage {
                                            // llama.cpp's `timings` ride the same chunk: the
                                            // prefill's processed tokens and milliseconds
                                            // (docs/research/slow-prefill-detection.md §3.1).
                                            let prefill = chunk.timings.as_ref().map(|t| crate::shared::api::contract::Prefill {
                                                tokens: t.prompt_n,
                                                ms: t.prompt_ms.round().max(0.0) as u32,
                                            });
                                            yield ChatChunk::Usage(TokenUsage {
                                                prompt_tokens: u.prompt_tokens,
                                                completion_tokens: u.completion_tokens,
                                                reasoning_tokens: u.completion_tokens_details.reasoning_tokens,
                                                prefill,
                                            });
                                        }
                                        let Some(choice) = chunk.choices.into_iter().next() else { continue };
                                        let mut delta = choice.delta;
                                        // `reasoning_content` or a gateway's `reasoning`, one of
                                        // the two (wire::Delta::thoughts).
                                        if let Some(r) = delta.thoughts()
                                            && !r.is_empty()
                                        {
                                            yield ChatChunk::Thoughts(r);
                                        }
                                        if let Some(c) = delta.content
                                            && !c.is_empty()
                                        {
                                            for piece in parser.push(&c) { yield piece_to_chunk(piece); }
                                        }
                                        if let Some(tool_calls) = delta.tool_calls {
                                            for tc in tool_calls {
                                                let (name, arguments) = match tc.function {
                                                    Some(f) => (f.name, f.arguments.unwrap_or_default()),
                                                    None => (None, String::new()),
                                                };
                                                yield ChatChunk::ToolCall(ToolCallDelta { thought_signature: None,
                                                    index: tc.index,
                                                    id: tc.id,
                                                    name,
                                                    arguments,
                                                });
                                            }
                                        }
                                        if let Some(reason) = choice.finish_reason {
                                            for piece in parser.finish() { yield piece_to_chunk(piece); }
                                            // Record it and keep reading rather than
                                            // finishing here: llama.cpp sends the
                                            // `include_usage` chunk **after** this one
                                            // (measured — `choices` empty, then
                                            // `[DONE]`), so breaking now threw the exact
                                            // token counts away every single time. The
                                            // stream's own terminator ends us below, and
                                            // the cancel arm still bounds the wait.
                                            finish = Some(FinishReason::from_wire(&reason));
                                        }
                                    }
                                    Err(err) => {
                                        // Not a chunk and not an error envelope (asked
                                        // above): logged, and the stream goes on.
                                        tracing::warn!(error = %err, data = %event.data, "failed to parse SSE chunk");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    /// Reads llama.cpp's `/props` → `default_generation_settings.n_ctx`.
    ///
    /// **The figure is used as given, never divided by `total_slots`** — measured
    /// (§9a M2 of the research): with no `-np` flag the server sets
    /// `n_parallel = 4, kv_unified = true` and does *not* divide `n_ctx`, so
    /// dividing would be wrong by 4x; where slots do divide the context, the
    /// field already reports the per-slot figure.
    ///
    /// Any failure — a server without the endpoint (vLLM, LM Studio, a cloud
    /// proxy), a network error, a body that doesn't parse — is `None`, i.e.
    /// "cannot say" (see [`Self::props`]).
    async fn context_budget(&self) -> Option<u32> {
        // A zero would be a nonsense window; treat it as "cannot say" rather than
        // as a budget every prompt exceeds.
        self.props()
            .await?
            .default_generation_settings
            .and_then(|g| g.n_ctx)
            .filter(|&n| n > 0)
    }

    /// Reads the endpoint's catalogue for the configured model
    /// (`GET /v1/models`), when it carries anything worth reading.
    ///
    /// The two keys are OpenRouter's and every gateway that copies it; a
    /// llama.cpp answers the same endpoint with neither, which lands as the same
    /// `None` this method returns when the model field is blank or the fetch
    /// fails. An **empty** `supported_parameters` is read as silence rather than
    /// as "takes nothing": a catalogue that lists no parameters is not claiming
    /// the model refuses them all, and treating it as a claim would hide every
    /// sampling field on a server that simply says little.
    async fn model_capabilities(&self) -> Option<ModelCapabilities> {
        let entry = self.catalogue_entry().await?;
        let caps = ModelCapabilities {
            context_length: entry.context_length.filter(|&n| n > 0),
            sampling_fields: entry
                .supported_parameters
                .filter(|v| !v.is_empty())
                .map(|v| v.into()),
        };
        // Nothing worth carrying is the same as no answer, so the layer above has
        // one shape to handle rather than two.
        (caps != ModelCapabilities::default()).then_some(caps)
    }

    /// Reads llama.cpp's `/props` → `total_slots`: the number of server slots,
    /// i.e. requests it serves at once. Measured on b10791: `4` for a server
    /// launched with `-np 4`, and `4` for one launched without `-np` — since
    /// December 2025 llama.cpp's auto default is four slots over one unified
    /// KV pool (docs/research/parallel-subagents.md §2.4). A zero is nonsense
    /// and reads as "cannot say", like a zero window above.
    async fn parallel_slots(&self) -> Option<u32> {
        self.props().await?.total_slots.filter(|&n| n > 0)
    }

    /// Reads llama.cpp's `/props` → `modalities.vision` (measured on b10322).
    ///
    /// The same fetch [`Self::context_budget`] makes, read one field further along —
    /// not a second endpoint. A server that answers without a `modalities` key is
    /// [`VisionSupport::Unknown`], **not** `Unsupported`: everything that is not
    /// llama.cpp (vLLM, LM Studio, a proxy) omits the key, and answering "no" there
    /// would refuse a working vision setup on the strength of a field the server
    /// never claimed to speak.
    async fn vision(&self) -> VisionSupport {
        match self.props().await.and_then(|p| p.modalities?.vision) {
            Some(true) => VisionSupport::Supported,
            Some(false) => VisionSupport::Unsupported,
            None => VisionSupport::Unknown,
        }
    }

    /// Asks the server what it is running: `GET /v1/models` when it lists
    /// **exactly one** model, then llama.cpp's `/props` (`model_alias`, falling
    /// back to `model_path`).
    ///
    /// **The catalogue goes first** because it is the standard endpoint and,
    /// measured on b9769, the *public* one: an authenticated `llama-server`
    /// answers `/v1/models` with `200` and no key at all, while `/props` is
    /// `401`. A list of **several** models is deliberately not guessed at — on
    /// such an endpoint (llama.cpp's router mode, LM Studio, Ollama, LiteLLM,
    /// OpenRouter) the request's own `model` field is what picks one, so the
    /// name is in settings already or the setup does not work at all; inventing
    /// an answer here would name a model that did not reply.
    ///
    /// See docs/research/external-model-name.md §3.
    async fn model_id(&self) -> Option<String> {
        if let Some(only) = self
            .listed_models()
            .await
            .filter(|ids| ids.len() == 1)
            .and_then(|ids| ids.into_iter().next())
        {
            return Some(crate::shared::gguf::display_id(&only));
        }
        let props = self.props().await?;
        let raw = props
            .model_alias
            .into_iter()
            .chain(props.model_path)
            .find(|s| !s.trim().is_empty())?;
        Some(crate::shared::gguf::display_id(&raw))
    }
}

#[async_trait::async_trait]
impl Embedder for OpenAiClient {
    // The role carries no wire meaning: the OpenAI embeddings API takes plain
    // text. Where a model wants its input marked, that is done above by
    // `PrefixedEmbedder` (docs/research/embedding-input-prefixes.md §6.2).
    async fn embed(&self, texts: Vec<String>, _role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = wire::EmbeddingRequest {
            model: self.model.clone(),
            input: texts,
        };
        let response = self
            .auth(self.http.post(&url))
            .json(&body)
            .send()
            .await
            // No `.with_context(|| format!("POST {url}"))` here on purpose:
            // `anyhow` prints only the outermost context, so that wrapper used to
            // replace the reason ("connection refused") with the bare URL.
            .map_err(|err| error::EngineError::transport(&err))?;
        // Don't swallow the error body (like chat_stream): llama-server puts the reason in
        // JSON (e.g. "input is too large to process. increase the physical batch
        // size" for a too-long chunk) — without it "error status" is useless.
        let response = error::check_status(SUBJECT_EMBEDDER, response).await?;
        let resp: wire::EmbeddingResponse = response
            .json()
            .await
            .context("decoding embeddings response")?;
        Ok(resp.data.into_iter().map(|d| d.embedding).collect())
    }
}

/// The URL of the `/health` readiness endpoint, from the base URL. `/health` lives at
/// the server root (outside `/v1`), so the `/v1` suffix is stripped.
fn health_url(base_url: &str) -> String {
    server_root_url(base_url, "health")
}

/// `/props` — llama.cpp's own description of the running server (§9a M1). Like
/// `/health` it lives at the root, outside `/v1`.
fn props_url(base_url: &str) -> String {
    server_root_url(base_url, "props")
}

/// A path at the server root (outside the `/v1` prefix the OpenAI surface uses).
fn server_root_url(base_url: &str, path: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let root = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    format!("{root}/{path}")
}

/// The part of llama.cpp's `/props` we read. Every other field is ignored, so a
/// server that answers with more (or a future version with fewer) still parses.
#[derive(serde::Deserialize)]
struct Props {
    default_generation_settings: Option<PropsGeneration>,
    /// The number of server slots (`-np`, or llama.cpp's auto default of four).
    /// Absent on everything that is not llama.cpp.
    total_slots: Option<u32>,
    /// What the server calls the loaded model: `--alias` when one was given,
    /// otherwise the model's own name, otherwise the `-m` path (llama.cpp
    /// `server.cpp`). Absent on everything that is not llama.cpp.
    model_alias: Option<String>,
    /// The `-m` path. Read only when `model_alias` says nothing, so a server
    /// that reports one field but not the other still answers.
    model_path: Option<String>,
    /// What the loaded model accepts besides text. Measured on llama.cpp b10322:
    /// `"modalities": {"vision": true, "video": true, "audio": false}`. Absent on
    /// every server that is not llama.cpp — hence `Option`, and hence
    /// [`VisionSupport::Unknown`] rather than "no".
    modalities: Option<PropsModalities>,
}

#[derive(serde::Deserialize)]
struct PropsGeneration {
    n_ctx: Option<u32>,
}

#[derive(serde::Deserialize)]
struct PropsModalities {
    vision: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A var that is always set (so the URL/key resolve) vs one that never is.
    /// Deliberately reads existing variables instead of setting any: `set_var` is
    /// `unsafe` in edition 2024, and mutating the environment races every other
    /// test in the binary. Same trick as the supervisor's `api_key_env` tests.
    const SET: &str = "PATH";
    const UNSET: &str = "MINDFORK_DEFINITELY_UNSET_VAR_LIVE_CLIENT";

    #[test]
    fn live_client_needs_a_url_and_takes_the_key_only_when_set() {
        use crate::shared::api::live_client;

        assert!(
            live_client(UNSET, SET).is_none(),
            "no URL -> the smoke skips"
        );

        let no_key = live_client(SET, UNSET).expect("URL is set");
        assert!(
            no_key.api_key.is_none(),
            "an unset key must send no Authorization header — this is the local \
             llama-server path and it must stay byte-for-byte as before"
        );

        let with_key = live_client(SET, SET).expect("URL is set");
        assert!(with_key.api_key.is_some(), "a set key is carried");

        // …and the model stays unset unless its own variable names one: a
        // single-model `llama-server` must keep receiving a request with no
        // `model` key at all (docs/research/external-model-name.md §2.3).
        assert!(
            no_key.model.is_none() && with_key.model.is_none(),
            "no MINDFORK_*_MODEL variable is set in a unit-test environment"
        );
    }

    /// `probe()` must carry the key: an authenticated server answers `/health`
    /// with 401 without it, and 401 is not 503, so the probe would report ready
    /// regardless of the key — the supervisor smokes would pass with a broken one.
    #[tokio::test]
    async fn probe_sends_the_authorization_header_when_a_key_is_set() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).unwrap();
            // Drain the request before answering: writing first turns the close
            // into an RST that discards the response.
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase()
        });

        OpenAiClient::new(format!("http://{addr}/v1"))
            .with_api_key(Some("s3cret".into()))
            .probe()
            .await
            .expect("stub answers 200");

        let request = seen.join().unwrap();
        assert!(
            request.contains("authorization: bearer s3cret"),
            "probe must authenticate; got:\n{request}"
        );
        assert!(request.contains("get /health"), "and hit /health");
    }

    /// Serves one SSE response made of the given `data:` payloads.
    fn sse_server(events: &'static [&'static str]) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf);
            let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
        });
        format!("http://{addr}/v1")
    }

    /// Serves one SSE response whose body is written **verbatim** — for the
    /// lines [`sse_server`] cannot express, such as a comment
    /// (`: OPENROUTER PROCESSING`), which carries no `data:` field.
    fn sse_raw_server(body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
        });
        format!("http://{addr}/v1")
    }

    /// One stubbed HTTP exchange per connection: the status to answer with and
    /// the body to answer it with. Named because the arms below hold a table of
    /// them, and the tuple-in-an-array spelled out inline is what clippy calls a
    /// very complex type — rightly.
    type Script = &'static [(u16, &'static str)];

    /// Serves `replies` in order, one per connection, and hands back the request
    /// **bodies** it saw — which is what the refusal tests are really about: not
    /// only that the turn recovers, but that the second request no longer carries
    /// the field the server refused.
    fn scripted_server(replies: Script) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, payload) in replies {
                let (mut sock, _) = listener.accept().unwrap();
                seen.push(read_request_body(&mut sock));
                let (reason, kind) = match status {
                    200 => ("OK", "text/event-stream"),
                    503 => ("Service Unavailable", "application/json"),
                    _ => ("Bad Request", "application/json"),
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
            seen
        });
        (format!("http://{addr}/v1"), handle)
    }

    /// Reads one HTTP request off the socket and returns its body. Reads until
    /// `Content-Length` is satisfied rather than taking one `read`: a body split
    /// across segments would otherwise make these tests flaky rather than wrong.
    fn read_request_body(sock: &mut std::net::TcpStream) -> String {
        use std::io::Read;
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = match sock.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            raw.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&raw);
            let Some((head, body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let want: usize = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse().ok())?
                })
                .unwrap_or(0);
            if body.len() >= want {
                return body.to_string();
            }
        }
        String::new()
    }

    /// The `400` OpenRouter answers a request to mute reasoning with, on a model
    /// that always reasons (docs/research/openrouter-external.md §8.1, M4).
    const REASONING_MANDATORY: &str = r#"{"error":{"message":"Reasoning is mandatory for this endpoint and cannot be disabled.","code":400}}"#;

    /// A one-chunk SSE reply, as the server sends it after the retry succeeds.
    const ONE_CHUNK_SSE: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

    /// A turn that asks for reasoning to be off — the shape `title.rs`, the
    /// compaction roll and impersonation all send.
    fn muted_turn() -> ChatRequest {
        ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![crate::shared::api::ApiMessage::user("hi".to_string())],
            sampling: crate::entities::sampling::SamplingConfig {
                reasoning_effort: Some(ReasoningEffort::None),
                ..Default::default()
            },
            tools: Vec::new(),
        }
    }

    async fn collect_turn(client: &OpenAiClient, req: ChatRequest) -> Vec<ChatChunk> {
        let mut s = client
            .chat_stream(req, CancellationToken::new())
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(c) = s.next().await {
            out.push(c);
        }
        out
    }

    async fn collect(url: String) -> Vec<ChatChunk> {
        let client = OpenAiClient::new(url);
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![crate::shared::api::ApiMessage::user("hi".to_string())],
            sampling: Default::default(),
            tools: Vec::new(),
        };
        let mut s = client
            .chat_stream(req, CancellationToken::new())
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(c) = s.next().await {
            out.push(c);
        }
        out
    }

    /// llama.cpp sends the `include_usage` chunk **after** the one carrying
    /// `finish_reason` — measured against the live server, `choices` empty, then
    /// `[DONE]`. Finishing on `finish_reason` therefore discarded the exact token
    /// counts on every single turn, which is why the status bar never left the
    /// `~` estimate and why automatic compaction had nothing to trigger on.
    #[tokio::test]
    async fn a_usage_chunk_after_finish_reason_is_not_lost() {
        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":1234,"completion_tokens":7,"total_tokens":1241}}"#,
            "[DONE]",
        ]);
        let chunks = collect(url).await;
        let usage = chunks.iter().find_map(|c| match c {
            ChatChunk::Usage(u) => Some(*u),
            _ => None,
        });
        let usage = usage.expect("the trailing usage chunk must survive");
        assert_eq!(usage.prompt_tokens, 1234);
        assert_eq!(usage.completion_tokens, 7);

        // …and the turn still ends, with the reason the model actually gave —
        // held from the earlier chunk rather than replaced by a default `Stop`.
        assert!(
            matches!(chunks.last(), Some(ChatChunk::Finished(FinishReason::Stop))),
            "{chunks:?}"
        );
        assert_eq!(
            chunks
                .iter()
                .filter(|c| matches!(c, ChatChunk::Finished(_)))
                .count(),
            1,
            "exactly one terminator: {chunks:?}"
        );
    }

    /// The reason must survive the wait: a length cut-off that came back as a
    /// plain `Stop` would make the loop treat a truncated reply as a complete one.
    #[tokio::test]
    async fn the_reported_reason_survives_the_trailing_chunk() {
        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"content":"a"},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":9,"total_tokens":18}}"#,
            "[DONE]",
        ]);
        let chunks = collect(url).await;
        assert!(
            matches!(
                chunks.last(),
                Some(ChatChunk::Finished(FinishReason::Length))
            ),
            "{chunks:?}"
        );
    }

    /// The defect this recovery exists for, end to end: a server that refuses to
    /// disable reasoning answers the silent turns' request with a `400`
    /// (measured on `deepseek/deepseek-r1`, research §8.1 M4), so the title, the
    /// compaction roll and impersonation all failed on such a model while
    /// ordinary chat kept working. The turn must now ask again without the field
    /// — and the second request is the assertion that matters, not just the text.
    #[tokio::test]
    async fn a_refusal_to_disable_reasoning_is_answered_by_asking_again_without_it() {
        let (url, seen) = scripted_server(&[(400, REASONING_MANDATORY), (200, ONE_CHUNK_SSE)]);
        let client = OpenAiClient::new(url);
        let chunks = collect_turn(&client, muted_turn()).await;
        assert!(
            matches!(&chunks[0], ChatChunk::Text(t) if t == "hi"),
            "the turn must recover, not fail: {chunks:?}"
        );

        let bodies = seen.join().unwrap();
        assert_eq!(bodies.len(), 2, "exactly one retry");
        assert!(
            bodies[0].contains(r#""reasoning_effort":"none""#),
            "the first attempt asks, as before: {}",
            bodies[0]
        );
        assert!(
            !bodies[1].contains("reasoning_effort"),
            "the second must not ask again: {}",
            bodies[1]
        );
    }

    /// And it is remembered: the *next* turn on the same backend does not spend a
    /// round trip rediscovering it. A model that must reason does not stop
    /// mid-session, so the memo costs one refusal per server rather than one per
    /// silent turn.
    #[tokio::test]
    async fn the_refusal_is_remembered_for_the_next_turn() {
        let (url, seen) = scripted_server(&[
            (400, REASONING_MANDATORY),
            (200, ONE_CHUNK_SSE),
            (200, ONE_CHUNK_SSE),
        ]);
        let client = OpenAiClient::new(url);
        collect_turn(&client, muted_turn()).await;
        collect_turn(&client, muted_turn()).await;

        let bodies = seen.join().unwrap();
        assert_eq!(bodies.len(), 3, "the second turn must not be refused again");
        assert!(
            !bodies[2].contains("reasoning_effort"),
            "the second turn asks nothing: {}",
            bodies[2]
        );
    }

    /// The three arms that must **not** recover. Each script ends with a reply
    /// that would succeed, so a wrong retry does not merely make an extra
    /// request — it turns the turn **green**, and the `expect_err` catches it.
    /// That shape is deliberate: counting requests at the stub is not a test
    /// here, because a second request against a spent script is refused by the
    /// OS and still arrives as "an error" — measured, the count-based version of
    /// these arms survived removing the status check below.
    ///
    /// - a `400` about something else is a plain failure, its message intact;
    /// - the same *message* under a transient status belongs to the retry
    ///   decorator (spec §6.8): answering a `503` by dropping a sampling field
    ///   would file an outage as a capability;
    /// - a turn that never asked to mute reasoning has nothing to re-send — an
    ///   identical body would only buy the same refusal twice.
    #[tokio::test]
    async fn the_recovery_does_not_fire_on_anything_else() {
        const UNRELATED: &str = r#"{"error":{"message":"model not found","code":400}}"#;
        let cases: [(Script, bool, &str); 3] = [
            (
                &[(400, UNRELATED), (200, ONE_CHUNK_SSE)],
                true,
                "model not found",
            ),
            (
                &[(503, REASONING_MANDATORY), (200, ONE_CHUNK_SSE)],
                true,
                "",
            ),
            (
                &[(400, REASONING_MANDATORY), (200, ONE_CHUNK_SSE)],
                false,
                "",
            ),
        ];
        for (script, asks_to_mute, expected) in cases {
            let (url, _seen) = scripted_server(script);
            let client = OpenAiClient::new(url);
            let req = if asks_to_mute {
                muted_turn()
            } else {
                ChatRequest {
                    sampling: Default::default(),
                    ..muted_turn()
                }
            };
            let err = client
                .chat_stream(req, CancellationToken::new())
                .await
                // The stream is not `Debug`, and its *absence* is the assertion:
                // a recovery would have been answered by the script's `200`.
                .map(|_| ())
                .expect_err("this failure must reach the caller unrecovered");
            assert!(
                err.to_string().contains(expected),
                "{err} (expected to carry {expected:?})"
            );
        }
    }

    /// A **gateway** streams its reasoning as `delta.reasoning`, not
    /// `delta.reasoning_content` (OpenRouter, and the clients that copy it).
    /// Serde drops an unknown field in silence, so before the field existed this
    /// stream reached the feed as a reply with no thoughts at all — and the
    /// `<think>` fallback could not rescue it, since the gateway has already
    /// lifted the reasoning out of `content`
    /// (docs/research/openrouter-external.md §5, F1).
    #[tokio::test]
    async fn a_gateways_reasoning_field_becomes_thoughts() {
        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"reasoning":"weighing it"},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"42"},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        let chunks = collect(url).await;
        assert!(
            matches!(&chunks[0], ChatChunk::Thoughts(t) if t == "weighing it"),
            "{chunks:?}"
        );
        assert!(
            matches!(&chunks[1], ChatChunk::Text(t) if t == "42"),
            "{chunks:?}"
        );
    }

    /// A server that sends **both** names sends one trace twice, so exactly one
    /// of them may be read — otherwise every thought would appear doubled in the
    /// feed. `reasoning_content` wins: it is what llama.cpp, vLLM, DeepSeek and
    /// xAI emit natively, while `reasoning` is the gateway's spelling of the
    /// same text. The two fixtures deliberately differ — with one string under
    /// both keys, swapping the precedence would pass unnoticed (lessons §2).
    #[tokio::test]
    async fn reasoning_content_wins_when_a_server_sends_both() {
        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"native","reasoning":"mirrored"},"finish_reason":null}]}"#,
            "[DONE]",
        ]);
        let chunks = collect(url).await;
        let thoughts: Vec<&String> = chunks
            .iter()
            .filter_map(|c| match c {
                ChatChunk::Thoughts(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(thoughts, vec!["native"], "one trace, once: {chunks:?}");
    }

    /// OpenRouter parks an SSE **comment** (`: OPENROUTER PROCESSING`) in the
    /// stream while it waits for the provider, to keep the connection from
    /// timing out. A comment carries no `data:`, so the SSE parser dispatches no
    /// event for it at all — the turn must not see a chunk, a parse warning or
    /// an early terminator (docs/research/openrouter-external.md §4.1).
    #[tokio::test]
    async fn a_keep_alive_comment_does_not_disturb_the_stream() {
        let url = sse_raw_server(concat!(
            ": OPENROUTER PROCESSING\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
            ": OPENROUTER PROCESSING\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        ));
        let chunks = collect(url).await;
        assert!(
            matches!(&chunks[0], ChatChunk::Text(t) if t == "hi"),
            "{chunks:?}"
        );
        assert!(
            matches!(chunks.last(), Some(ChatChunk::Finished(FinishReason::Stop))),
            "{chunks:?}"
        );
        assert_eq!(chunks.len(), 2, "the comments added nothing: {chunks:?}");
    }

    /// llama.cpp's error object inside an open stream — the shape of its
    /// "Context size has been exceeded." to every slot of an overfilled pool
    /// (docs/research/admission-by-budget.md §3.2) — ends the turn as an
    /// **error**, transient (a 500), with the text kept. Every field of a
    /// chunk has a default, so the envelope *parses* as an empty chunk; it
    /// has to be recognised before that parse, or the turn ends as a finished
    /// reply cut mid-word, which is what the live control arm found.
    #[tokio::test]
    async fn an_error_object_inside_the_stream_ends_the_turn_as_an_error() {
        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"content":"KELV"},"finish_reason":null}]}"#,
            r#"{"error":{"code":500,"message":"Context size has been exceeded.","type":"server_error"}}"#,
        ]);
        let chunks = collect(url).await;
        assert!(
            matches!(&chunks[0], ChatChunk::Text(t) if t == "KELV"),
            "{chunks:?}"
        );
        assert!(
            matches!(
                &chunks[1],
                ChatChunk::Error { message, transient: true }
                    if message.contains("Context size has been exceeded")
            ),
            "{chunks:?}"
        );
        assert!(
            matches!(
                chunks.last(),
                Some(ChatChunk::Finished(FinishReason::Error))
            ),
            "{chunks:?}"
        );
        assert_eq!(chunks.len(), 3, "nothing after the failure: {chunks:?}");
    }

    /// A server that ends the body without `[DONE]` still ends the turn, and with
    /// the reason it gave.
    #[tokio::test]
    async fn a_stream_that_ends_without_done_still_reports_its_reason() {
        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"content":"a"},"finish_reason":"length"}]}"#,
        ]);
        let chunks = collect(url).await;
        assert!(
            matches!(
                chunks.last(),
                Some(ChatChunk::Finished(FinishReason::Length))
            ),
            "{chunks:?}"
        );
    }

    #[test]
    fn props_url_sits_at_the_server_root_like_health() {
        assert_eq!(
            props_url("http://127.0.0.1:8000/v1"),
            "http://127.0.0.1:8000/props"
        );
        assert_eq!(props_url("http://host:9/v1/"), "http://host:9/props");
        assert_eq!(props_url("http://host:9"), "http://host:9/props");
    }

    /// Answers one request with the given status and body, then reports the path
    /// that was asked for.
    fn one_shot_server(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            // Drain before answering: writing first turns the close into an RST
            // that discards the response.
            let n = sock.read(&mut buf).unwrap();
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        (format!("http://{addr}/v1"), handle)
    }

    /// A catalogue in the shape OpenRouter actually answers, trimmed to the two
    /// keys this reads and carrying a neighbour so the lookup has something to
    /// get wrong ([openrouter-external.md](../../../../docs/research/openrouter-external.md) §8.1, M5).
    const CATALOGUE_BODY: &str = r#"{"data":[
        {"id":"vendor/other","context_length":8192,"supported_parameters":["temperature"]},
        {"id":"deepseek/deepseek-r1","context_length":64000,
         "supported_parameters":["max_tokens","reasoning","repetition_penalty","seed","temperature","top_k","top_p"]}
    ]}"#;

    /// The catalogue answers for **the configured model**, and for no other: the
    /// window and the field list come back off the entry whose id matches, which
    /// is what the compaction trigger and the sampling offer are then built on
    /// (docs/gateway-capabilities.md §3).
    #[tokio::test]
    async fn the_catalogue_answers_for_the_configured_model() {
        let (url, seen) = one_shot_server("200 OK", CATALOGUE_BODY);
        let caps = OpenAiClient::new(url)
            .with_model(Some("deepseek/deepseek-r1".into()))
            .model_capabilities()
            .await
            .expect("the catalogue lists that model");
        assert_eq!(caps.context_length, Some(64000));
        let fields = caps.sampling_fields.expect("it lists parameters too");
        assert!(fields.iter().any(|f| f == "repetition_penalty"));
        assert!(
            !fields.iter().any(|f| f == "min_p"),
            "the neighbour's list must not leak in: {fields:?}"
        );
        assert!(seen.join().unwrap().contains("/v1/models"));
    }

    /// Silence, in each of its three shapes, is never a claim: a blank model
    /// field has nothing to look up and must not even ask, a model the catalogue
    /// does not list is not a neighbour's row, and a server that answers without
    /// the keys (every llama.cpp) says nothing. All three land as `None`, which
    /// downstream means "behave exactly as before this existed".
    #[tokio::test]
    async fn silence_is_never_a_claim() {
        // A blank model: no request at all — the port is closed, so a request
        // would fail loudly rather than pass.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            drop(l);
            format!("http://127.0.0.1:{port}/v1")
        };
        assert!(OpenAiClient::new(dead).model_capabilities().await.is_none());

        let (url, _seen) = one_shot_server("200 OK", CATALOGUE_BODY);
        assert!(
            OpenAiClient::new(url)
                .with_model(Some("vendor/absent".into()))
                .model_capabilities()
                .await
                .is_none(),
            "a model the catalogue does not list is not a neighbour's row"
        );

        // llama.cpp's own answer: ids and nothing else.
        let (url, _seen) = one_shot_server("200 OK", r#"{"data":[{"id":"gemma-4"}]}"#);
        assert!(
            OpenAiClient::new(url)
                .with_model(Some("gemma-4".into()))
                .model_capabilities()
                .await
                .is_none(),
            "an entry without either key carries nothing to say"
        );
    }

    /// The measured shape of a real `/props` (§9a M1), trimmed to what is read.
    const PROPS_BODY: &str = r#"{"default_generation_settings":{"n_ctx":16384,"n_predict":-1},
        "total_slots":4,"build_info":"b9867-152d337fa"}"#;

    #[tokio::test]
    async fn context_budget_reads_n_ctx_as_given() {
        let (url, server) = one_shot_server("200 OK", PROPS_BODY);
        let budget = OpenAiClient::new(url).context_budget().await;
        // **Not** divided by `total_slots`, which is 4 here: with no `-np` flag
        // llama.cpp reports 4 slots over an undivided context, so dividing would
        // be wrong by 4x (§9a M2). Where slots do divide it, the field already
        // reports the per-slot figure.
        assert_eq!(budget, Some(16384));
        assert!(
            server.join().unwrap().starts_with("GET /props "),
            "asked at the server root, outside /v1"
        );
    }

    /// The slot count comes from the same `/props` body, read as given; a
    /// server without the endpoint answers "cannot say", never a guess.
    #[tokio::test]
    async fn parallel_slots_reads_total_slots() {
        let (url, server) = one_shot_server("200 OK", PROPS_BODY);
        assert_eq!(OpenAiClient::new(url).parallel_slots().await, Some(4));
        assert!(server.join().unwrap().starts_with("GET /props "));

        let (url, _server) = one_shot_server("404 Not Found", "{}");
        assert_eq!(OpenAiClient::new(url).parallel_slots().await, None);
        let (url, _server) = one_shot_server("200 OK", r#"{"total_slots":0}"#);
        assert_eq!(OpenAiClient::new(url).parallel_slots().await, None);
    }

    /// Every way of not knowing is `None` — "cannot say", never a guess that
    /// would make the trigger measure against a fiction.
    #[tokio::test]
    async fn anything_but_a_real_answer_is_unknown() {
        for (status, body) in [
            // A server without the endpoint: vLLM, LM Studio, a cloud proxy.
            ("404 Not Found", "{}"),
            ("500 Internal Server Error", "{}"),
            // Answers, but says nothing we can use.
            ("200 OK", "{}"),
            ("200 OK", r#"{"default_generation_settings":{}}"#),
            // A zero window is nonsense, not a budget every prompt exceeds.
            ("200 OK", r#"{"default_generation_settings":{"n_ctx":0}}"#),
            ("200 OK", "not json at all"),
        ] {
            let (url, server) = one_shot_server(status, body);
            assert_eq!(
                OpenAiClient::new(url).context_budget().await,
                None,
                "status={status} body={body}"
            );
            let _ = server.join();
        }
    }

    /// The measured shape of `/props` on a vision build (llama.cpp b10322): the
    /// `modalities` object sits next to `default_generation_settings`, which is why
    /// one fetch answers both questions.
    #[tokio::test]
    async fn vision_true_in_props_is_supported() {
        let (url, server) = one_shot_server(
            "200 OK",
            r#"{"default_generation_settings":{"n_ctx":16384},
                "modalities":{"vision":true,"video":true,"audio":false}}"#,
        );
        assert_eq!(
            OpenAiClient::new(url).vision().await,
            VisionSupport::Supported
        );
        assert!(
            server.join().unwrap().starts_with("GET /props "),
            "asked at the server root, outside /v1"
        );
    }

    /// A model loaded without `--mmproj` says so explicitly — and that *is* an
    /// answer, so it must not be softened into "cannot say".
    #[tokio::test]
    async fn vision_false_in_props_is_unsupported() {
        let (url, server) = one_shot_server(
            "200 OK",
            r#"{"modalities":{"vision":false,"video":false,"audio":false}}"#,
        );
        assert_eq!(
            OpenAiClient::new(url).vision().await,
            VisionSupport::Unsupported
        );
        let _ = server.join();
    }

    /// Every way of not being told is `Unknown`, never `Unsupported`: an arbitrary
    /// OpenAI-compatible server (vLLM, LM Studio, a proxy) has no `modalities` key
    /// at all, and reading its silence as "no" would refuse a working vision setup.
    #[tokio::test]
    async fn a_server_that_does_not_report_modalities_is_unknown() {
        for (status, body) in [
            // No `/props` endpoint at all.
            ("404 Not Found", "{}"),
            ("500 Internal Server Error", "{}"),
            // Answers `/props`, but says nothing about modalities.
            (
                "200 OK",
                r#"{"default_generation_settings":{"n_ctx":16384}}"#,
            ),
            // The key is there but the field is not (a future/older build).
            ("200 OK", r#"{"modalities":{"audio":false}}"#),
            ("200 OK", "not json at all"),
        ] {
            let (url, server) = one_shot_server(status, body);
            assert_eq!(
                OpenAiClient::new(url).vision().await,
                VisionSupport::Unknown,
                "status={status} body={body}"
            );
            let _ = server.join();
        }
    }

    /// The projector question must not disturb the context-window answer: both are
    /// read from the same body, and `context_budget` keeps its old behaviour on a
    /// `/props` that now also carries `modalities`.
    #[tokio::test]
    async fn modalities_do_not_disturb_the_context_budget() {
        let (url, server) = one_shot_server(
            "200 OK",
            r#"{"default_generation_settings":{"n_ctx":16384},"total_slots":4,
                "modalities":{"vision":true}}"#,
        );
        assert_eq!(OpenAiClient::new(url).context_budget().await, Some(16384));
        let _ = server.join();
    }

    /// An unreachable host must not hang or panic — it simply cannot say.
    #[tokio::test]
    async fn an_unreachable_server_is_unknown_too() {
        // Bind and drop: the port is then almost certainly free and refusing.
        let addr = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let client = OpenAiClient::new(format!("http://{addr}/v1"));
        assert_eq!(client.context_budget().await, None);
    }

    #[test]
    fn health_url_strips_v1_suffix() {
        assert_eq!(
            health_url("http://127.0.0.1:8000/v1"),
            "http://127.0.0.1:8000/health"
        );
        // No /v1 — just append /health.
        assert_eq!(health_url("http://host:9"), "http://host:9/health");
        // A stray slash isn't doubled.
        assert_eq!(health_url("http://host:9/v1/"), "http://host:9/health");
    }

    /// A server that answers `n` requests, choosing the body by the path asked
    /// for, and reports the paths in the order they arrived. An unlisted path is
    /// a `404` — which is what a non-llama.cpp server does with `/props`.
    ///
    /// Each response closes its connection, so the two requests `model_id` can
    /// make arrive as two accepts rather than one pooled pipeline.
    fn path_server(
        n: usize,
        routes: &'static [(&'static str, &'static str, &'static str)],
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for _ in 0..n {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let read = sock.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..read]).to_string();
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, s, b)| (*s, *b))
                    .unwrap_or(("404 Not Found", "{}"));
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).unwrap();
                asked.push(path);
            }
            asked
        });
        (format!("http://{addr}/v1"), handle)
    }

    /// The measured shape of `/v1/models` on llama.cpp b9769: `data[0].id` is the
    /// model's name, and the catalogue is asked **first** — it is the standard
    /// endpoint and, unlike `/props`, it answers without an API key.
    #[tokio::test]
    async fn model_id_takes_a_single_listed_model_without_asking_props() {
        let (url, server) = path_server(
            1,
            &[(
                "/v1/models",
                "200 OK",
                r#"{"object":"list","data":[{"id":"gemma-3-4b-it","object":"model"}]}"#,
            )],
        );
        assert_eq!(
            OpenAiClient::new(url).model_id().await.as_deref(),
            Some("gemma-3-4b-it")
        );
        assert_eq!(
            server.join().unwrap(),
            vec!["/v1/models".to_string()],
            "one answer is enough — /props must not be asked as well"
        );
    }

    /// What an un-aliased `llama-server` actually reports (measured on b10659,
    /// the live stack): the `-m` path, backslashes and drive letter included. It
    /// reaches the header as a model name, not as a file.
    #[tokio::test]
    async fn model_id_normalizes_a_reported_gguf_path() {
        let (url, server) = path_server(
            1,
            &[(
                "/v1/models",
                "200 OK",
                r#"{"data":[{"id":"D:\\LLM\\GGUF\\gemma-4-31B_q4_0-it.gguf"}]}"#,
            )],
        );
        assert_eq!(
            OpenAiClient::new(url).model_id().await.as_deref(),
            Some("gemma-4-31B_q4_0-it")
        );
        let _ = server.join();
    }

    /// Several models listed — llama.cpp's router mode, LM Studio, Ollama, a
    /// gateway. Which one answers is decided by the request's own `model` field,
    /// so the catalogue cannot say; `/props` still can, and on a router it
    /// deliberately does not (`model_alias` is the dummy `"llama-server"`, which
    /// this test does not simulate — see the next one for the silent case).
    #[tokio::test]
    async fn several_listed_models_fall_through_to_props() {
        let (url, server) = path_server(
            2,
            &[
                (
                    "/v1/models",
                    "200 OK",
                    r#"{"data":[{"id":"qwen-3.6-27b"},{"id":"gemma-4-31b"}]}"#,
                ),
                (
                    "/props",
                    "200 OK",
                    r#"{"model_alias":"gemma-4-31b","model_path":"/models/gemma-4-31b.gguf"}"#,
                ),
            ],
        );
        assert_eq!(
            OpenAiClient::new(url).model_id().await.as_deref(),
            Some("gemma-4-31b")
        );
        assert_eq!(
            server.join().unwrap(),
            vec!["/v1/models".to_string(), "/props".to_string()]
        );
    }

    /// A build that reports the path but not the alias still answers — the two
    /// fields are read in order, not as a pair.
    #[tokio::test]
    async fn an_empty_alias_falls_through_to_the_model_path() {
        let (url, server) = path_server(
            2,
            &[(
                "/props",
                "200 OK",
                r#"{"model_alias":"","model_path":"/models/bge-m3-Q8_0.gguf"}"#,
            )],
        );
        assert_eq!(
            OpenAiClient::new(url).model_id().await.as_deref(),
            Some("bge-m3-Q8_0")
        );
        let _ = server.join();
    }

    /// Every way of not being told is `None` — "cannot say", never a placeholder.
    /// The caption and the metadata then show nothing, exactly as they did before
    /// the engine was ever asked.
    #[tokio::test]
    async fn a_server_that_names_no_model_says_nothing() {
        for routes in [
            // Neither endpoint exists: a cloud proxy, or a gateway that serves
            // only completions.
            &[][..],
            // A catalogue with no entries at all, and no `/props`.
            &[("/v1/models", "200 OK", r#"{"data":[]}"#)][..],
            // llama.cpp's router mode: several models and a `/props` that
            // describes the router rather than a model.
            &[
                (
                    "/v1/models",
                    "200 OK",
                    r#"{"data":[{"id":"a"},{"id":"b"}]}"#,
                ),
                ("/props", "200 OK", r#"{"model_alias":"","model_path":""}"#),
            ][..],
            // Answers, but not with JSON.
            &[("/v1/models", "200 OK", "not json at all")][..],
        ] {
            let (url, server) = path_server(2, routes);
            assert_eq!(
                OpenAiClient::new(url).model_id().await,
                None,
                "routes={routes:?}"
            );
            let _ = server.join();
        }
    }
    /// The usage chunk's `timings` become the usage's `prefill` — processed
    /// tokens and milliseconds — and a chunk without them leaves it `None`
    /// (docs/research/slow-prefill-detection.md §3.1).
    #[tokio::test]
    async fn timings_become_the_usage_prefill() {
        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":1250,"completion_tokens":7,"total_tokens":1257},"timings":{"cache_n":50,"prompt_n":1200,"prompt_ms":13333.4}}"#,
            "[DONE]",
        ]);
        let chunks = collect(url).await;
        let usage = chunks
            .iter()
            .find_map(|c| match c {
                ChatChunk::Usage(u) => Some(*u),
                _ => None,
            })
            .expect("usage");
        assert_eq!(
            usage.prefill,
            Some(crate::shared::api::contract::Prefill {
                tokens: 1200,
                ms: 13333
            })
        );

        let url = sse_server(&[
            r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":9,"total_tokens":18}}"#,
            "[DONE]",
        ]);
        let chunks = collect(url).await;
        let usage = chunks
            .iter()
            .find_map(|c| match c {
                ChatChunk::Usage(u) => Some(*u),
                _ => None,
            })
            .expect("usage");
        assert_eq!(usage.prefill, None, "no timings: another server's chunk");
    }
}

fn piece_to_chunk(piece: Piece) -> ChatChunk {
    match piece {
        Piece::Text(t) => ChatChunk::Text(t),
        Piece::Thoughts(t) => ChatChunk::Thoughts(t),
    }
}

/// A manual smoke set against a real OpenAI-compatible server (llama.cpp
/// `llama-server` etc.). Marked `#[ignore]` — doesn't run in CI.
/// Run: set `MINDFORK_ENGINE_URL=http://127.0.0.1:8000/v1` and
/// `cargo test -- --ignored`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::api::contract::{ApiMessage, ToolCallAccumulator, ToolSchema};
    use futures_util::StreamExt;

    fn client_from_env() -> Option<OpenAiClient> {
        crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")
    }

    /// A conversation whose **tool result** carries an image (spec §9.10): the model asks
    /// for a screenshot, the tool returns one, and the user asks what is on it.
    /// `with_image = false` is the control arm.
    fn screenshot_turn(with_image: bool) -> ChatRequest {
        screenshot_turn_with(with_image, "Screenshot taken.")
    }

    /// The same turn with the tool result's text spelled out: the withheld-image arm
    /// sends the statement the MCP adapter appends when `tools.mcp_images` is off
    /// (`tool.mcp.images_off`), where the plain control arm sends nothing at all.
    fn screenshot_turn_with(with_image: bool, result_text: &str) -> ChatRequest {
        let tool = ApiMessage::tool("call-1", result_text);
        let tool = if with_image {
            tool.with_images(vec![crate::shared::api::ApiImage::new(
                "image/png",
                &crate::shared::api::green_circle_png_base64(),
                None,
            )])
        } else {
            tool
        };
        ChatRequest {
            continue_final: false,
            system: None,
            // The shape a real turn has: the question is asked up front and the tool
            // result is the last message, so the model answers from it. See the Gemini
            // twin of this smoke for why a trailing user message is not just unrealistic
            // but actively misleading there.
            messages: vec![
                ApiMessage::user(crate::shared::api::TOOL_VISION_PROMPT),
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![crate::shared::api::ApiToolCall {
                        id: "call-1".into(),
                        name: "take_screenshot".into(),
                        arguments: "{}".into(),
                        thought_signature: None,
                    }],
                ),
                tool,
            ],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![ToolSchema {
                name: "take_screenshot".into(),
                description: "Take a screenshot of the screen.".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }],
        }
    }

    /// An image inside a **tool result** reaches a vision-capable llama.cpp — the path an
    /// MCP screenshot tool takes (spec §9.10, docs/research/mcp-tool-images.md).
    ///
    /// Two arms on purpose. The control, with the identical conversation minus the image,
    /// must **not** describe the fixture: measured, a blind model answers this question
    /// confidently anyway, and a one-armed version of this test passed against a feature
    /// that was doing nothing.
    #[tokio::test]
    #[ignore = "requires a vision-capable OpenAI-compatible server (MINDFORK_ENGINE_URL + --mmproj)"]
    async fn tool_result_image_is_seen_live() {
        if crate::shared::api::live_text_only() {
            eprintln!("skip: MINDFORK_LIVE_TEXT_ONLY — this stack has no vision projector");
            return;
        }
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let (control, _, _) = collect(
            client
                .chat_stream(screenshot_turn(false), Default::default())
                .await
                .unwrap(),
        )
        .await;
        eprintln!("control (no image): {control}");
        crate::shared::api::assert_sees_green_circle(&control, false, "control");

        let (answer, _, _) = collect(
            client
                .chat_stream(screenshot_turn(true), Default::default())
                .await
                .unwrap(),
        )
        .await;
        eprintln!("tool-result image: {answer}");
        crate::shared::api::assert_sees_green_circle(&answer, true, "with the image");
    }

    /// The blind arm once more, this time carrying the sentence the MCP adapter appends
    /// when `tools.mcp_images` withheld the image (`tool.mcp.images_off`).
    ///
    /// **Why the wording is directive and not descriptive.** Measured on Gemma 4 31B
    /// (2026-09-12, five runs an arm, the fixture's own 2048-token budget): saying
    /// nothing, the model described a screenshot it never received **5/5** — and the
    /// descriptive wording the `loop.images_*` family uses ("You have not seen them.")
    /// also went 5/5. Only adding "do not describe what they show; say that you cannot
    /// see them" moved it, to 0/5. So the sentence that ships here is deliberately not
    /// its siblings' shape, and this smoke is what says so. (A 256-token budget makes
    /// this unreadable: a thinking model runs out inside its thoughts and returns empty
    /// content, which looks like a decline and is not one.)
    ///
    /// No projector is needed — nothing here sends an image.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL"]
    async fn a_withheld_tool_image_is_not_described_live() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        // The control, printed rather than asserted: this is the answer the sentence has
        // to displace, and asserting a hallucination would only pin today's guess.
        let (control, _, _) = collect(
            client
                .chat_stream(screenshot_turn(false), Default::default())
                .await
                .unwrap(),
        )
        .await;
        eprintln!("control (nothing said): {control}");

        use crate::shared::i18n::{Lang, locale};
        let said = locale(Lang::En).tf("tool.mcp.images_off", &[("n", "1")]);
        let text = format!("Screenshot taken.\n{said}");
        let (answer, _, _) = collect(
            client
                .chat_stream(screenshot_turn_with(false, &text), Default::default())
                .await
                .unwrap(),
        )
        .await;
        eprintln!("withheld, and said so: {answer}");
        let low = answer.to_lowercase();
        // Two-sided on purpose. "Does not say green" would pass on a *wrong* guess —
        // which is exactly what the control produces — so the criterion is that no
        // colour is claimed at all, and that the model says outright it cannot see.
        let colours = [
            "blue", "green", "red", "white", "black", "yellow", "purple", "grey", "gray",
        ];
        assert!(
            !colours.iter().any(|c| low.contains(c)),
            "a withheld image must not be described: {answer:?}"
        );
        assert!(
            low.contains("cannot see") || low.contains("can't see") || low.contains("unable to"),
            "the model has to say it cannot see the image: {answer:?}"
        );
    }

    /// The other shape of the same family: `python_exec`'s note is a **suffix on one
    /// file's line** inside the result's `files:` section, not a bracketed line of its
    /// own — so it gets its own arm rather than inheriting the MCP one's verdict.
    ///
    /// The measured outcome differs too, and the assertion says so. With the shipped
    /// suffix the model mostly answered by **calling the tool again** (7 of 9 runs,
    /// `finish_reason: tool_calls`) — not a hallucination, but a wasted round, since the
    /// second call's image is withheld for the same reason. With the directive clause it
    /// declines outright 4/5 and re-calls 1/5. Either is acceptable; inventing an answer
    /// about the chart is not, and that is what this asserts.
    ///
    /// The result is assembled by the producers rather than typed: the console half by
    /// `present::format_console`, the section from `keep_outputs`' own lines and join. It
    /// was typed in the uncounted `stdout:` shape, which no call has returned since the
    /// console's sections were counted — so the smoke measured a result the model no longer
    /// gets, and would have gone on doing so through any later change to the shape.
    /// Re-measured on the counted shape (Gemma 4 31B, 2026-09-13, five runs an arm): with
    /// the note a plain refusal 5/5; without it, an invented answer 3/5 and a re-call 2/5.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL"]
    async fn a_withheld_chart_is_not_described_live() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        use crate::features::tools::present::format_console;
        use crate::shared::i18n::{Lang, locale};
        let loc = locale(Lang::En);
        // What a call that printed `saved` and left one chart returns with images off: the
        // console, a blank line, then `files:` over the section's lines (`keep_outputs`).
        let result = format!(
            "{}\n\nfiles:\n{}\n{}{}",
            format_console(None, "saved", "", true, Some(0), loc),
            loc.tf("tool.python_exec.files.saved_in", &[("dir", "/chat/files")]),
            loc.tf(
                "tool.python_exec.files.item",
                &[
                    ("name", "chart.png"),
                    ("size", "24.1 KB"),
                    ("mime", "image/png"),
                ],
            ),
            loc.t("tool.python_exec.files.not_shown_off"),
        );
        assert!(
            result.starts_with("stdout (1 line):\nsaved\n\nfiles:\n"),
            "the smoke has to send the shape a call returns: {result}"
        );
        let (answer, _, finish) = collect(
            client
                .chat_stream(chart_turn(&result), Default::default())
                .await
                .unwrap(),
        )
        .await;
        eprintln!("withheld chart ({finish:?}): {answer}");
        if finish == Some(FinishReason::ToolCalls) {
            // It went back to the tool instead of answering — it did not invent anything.
            return;
        }
        let low = answer.to_lowercase();
        assert!(
            low.contains("cannot see")
                || low.contains("can't see")
                || low.contains("cannot tell")
                || low.contains("unable to"),
            "a chart it was never shown must not be answered for: {answer:?}"
        );
    }

    /// A `python_exec` turn whose result is `result`: the question asks something only
    /// the rendered image could answer, so any substantive answer is a claim to have
    /// seen it.
    fn chart_turn(result: &str) -> ChatRequest {
        ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::user(
                    "Plot the monthly totals and save the chart, then tell me: does the \
                     legend overlap the plotted line? Answer briefly.",
                ),
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![crate::shared::api::ApiToolCall {
                        id: "call-1".into(),
                        name: "python_exec".into(),
                        arguments: "{}".into(),
                        thought_signature: None,
                    }],
                ),
                ApiMessage::tool("call-1", result),
            ],
            sampling: SamplingConfig {
                max_tokens: Some(8192),
                ..Default::default()
            },
            tools: vec![ToolSchema {
                name: "python_exec".into(),
                description: "Run Python.".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }],
        }
    }

    pub(super) async fn collect(stream: ChatStream) -> (String, String, Option<FinishReason>) {
        let mut text = String::new();
        let mut thoughts = String::new();
        let mut finish = None;
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::ThoughtsSignature(_) | ChatChunk::ToolCall(_) | ChatChunk::Usage(_) => {}
                // Say why: a smoke whose engine failed mid-stream would otherwise
                // assert on empty text with nothing in the output explaining it.
                ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
                ChatChunk::Retry {
                    attempt,
                    max,
                    delay,
                } => {
                    eprintln!("retrying {attempt}/{max} in {delay:?}")
                }
                ChatChunk::Finished(r) => {
                    finish = Some(r);
                    break;
                }
            }
        }
        (text, thoughts, finish)
    }

    /// The base engine smoke: a stream arrives, it carries text, and the finish
    /// reason is sane.
    ///
    /// `max_tokens` is deliberately generous and thinking is left **on**, because
    /// on a reasoning model the two share one budget and the reply is emitted
    /// last. Measured on `Qwen3.6-27B` q4_K_M: this trivial prompt costs 357–949
    /// characters of `reasoning_content` before the `pong` — so the original 64
    /// were spent entirely on thinking (empty text, `finish=Length`), and even
    /// 256 sits inside the worst case's noise. 1024 keeps ~4x margin over the
    /// widest run observed while the smoke still exercises *both* streams, which
    /// is what a base smoke is for; `emits_thoughts_for_reasoning_model` covers
    /// the thoughts channel on its own.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: Some("You are a helpful assistant.".into()),
            messages: vec![ApiMessage::user("Reply with exactly: pong")],
            sampling: SamplingConfig {
                max_tokens: Some(1024),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, _thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        assert!(!text.is_empty(), "expected non-empty response");
        assert!(matches!(
            finish,
            Some(FinishReason::Stop | FinishReason::Length)
        ));
    }

    /// A model too large for one file, end to end: the server reports the **part**
    /// it was pointed at, and what a header may show is the *model*.
    ///
    /// Weights over ~50 GB ship as `<name>-00001-of-00002.gguf`, …; llama.cpp is
    /// handed the first part and reads the rest itself, and an un-aliased
    /// `llama-server` then reports that part's whole path as its model id. That
    /// is the single input [`crate::shared::gguf::display_id`] exists for, and
    /// until `gpt-oss-120b` joined the live gate it had only ever been given
    /// fixtures.
    ///
    /// Both halves are asserted here because either alone is worthless: that the
    /// server really did report a part (otherwise the stack is not what the run
    /// declared, and the check is vacuous), and that `model_id` hands back a name
    /// with the directory, the extension **and** the part number gone.
    ///
    /// Declared, not detected — `MINDFORK_LIVE_SPLIT_MODEL=1` — and it **fails
    /// rather than skips** when the declaration turns out to be false
    /// (docs/research/e2e-gpt-oss-120b.md §6, T2).
    ///
    /// Measured on `unsloth/gpt-oss-120b-GGUF` Q8_0 (two parts, 63.39 GB):
    /// `/repository/Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf` in,
    /// `gpt-oss-120b-Q8_0` out.
    #[tokio::test]
    #[ignore = "requires a live server holding a split GGUF (MINDFORK_LIVE_SPLIT_MODEL=1)"]
    async fn a_split_model_is_named_by_the_model_not_the_part_live() {
        if !crate::shared::api::live_split_model() {
            eprintln!("skip: MINDFORK_LIVE_SPLIT_MODEL not set");
            return;
        }
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let listed = client
            .listed_models()
            .await
            .expect("a live server must answer GET /v1/models");
        eprintln!("listed models: {listed:?}");
        let raw = listed
            .first()
            .expect("GET /v1/models listed nothing to check");
        let shard = crate::shared::gguf::parse_shard(raw).unwrap_or_else(|| {
            panic!(
                "MINDFORK_LIVE_SPLIT_MODEL was declared, but the server reports a \
                 plain file: {raw} — this stack cannot test what the run says it tests"
            )
        });
        assert_eq!(
            shard.index, 1,
            "llama.cpp must be given the first part: {raw}"
        );
        assert!(
            shard.total > 1,
            "a split model has more than one part: {raw}"
        );

        let name = client
            .model_id()
            .await
            .expect("the same server that lists a model must name it");
        eprintln!("split model reported as {raw:?}, shown as {name:?}");
        assert!(
            crate::shared::gguf::parse_shard(&format!("{name}{}", crate::shared::gguf::EXT))
                .is_none(),
            "the part number survived into the name a header shows: {name}"
        );
        assert!(
            !name.contains('/')
                && !name.contains('\\')
                && !name.ends_with(crate::shared::gguf::EXT),
            "a path reached the header instead of a model name: {name}"
        );
    }

    /// A real rejection from a real server, checked as a **typed** error rather
    /// than as prose.
    ///
    /// This is the path every non-2xx takes (`check_status`), and the properties it
    /// has to hold are the ones the layers above decide on: the status survives,
    /// the server's own body survives (so the overflow classifier still fires and
    /// the user gets the `/compact` advice instead of a generic wrapper), and a
    /// `400` is **not** reported as worth retrying — the coming retry decorator
    /// would otherwise spend its whole budget on a prompt that can never fit
    /// (docs/research/cloud-retry-backoff.md §5).
    ///
    /// The prompt is deliberately far larger than any context this project runs
    /// against: llama.cpp answers `400` *before* the prefill starts (measured —
    /// spec §6.7), so the size costs nothing and the smoke cannot silently pass by
    /// fitting.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn an_oversized_prompt_is_a_typed_non_transient_error() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("word ".repeat(120_000))],
            sampling: SamplingConfig {
                max_tokens: Some(16),
                ..Default::default()
            },
            tools: vec![],
        };
        // `expect_err` would need `ChatStream: Debug`, which a boxed stream isn't.
        let err = match client.chat_stream(req, Default::default()).await {
            Ok(_) => panic!("a prompt this size cannot fit any context window"),
            Err(err) => err,
        };
        let typed = err
            .downcast_ref::<crate::shared::api::error::EngineError>()
            .expect("the status must reach the caller as a typed error");
        eprintln!(
            "status={:?} transient={} retry_after={:?}\nmessage={}",
            typed.status,
            typed.is_transient(),
            typed.retry_after,
            typed.message
        );
        assert_eq!(typed.status, Some(400), "the status must survive");
        assert!(
            !typed.is_transient(),
            "an oversized prompt must not be reported as retryable"
        );
        assert!(
            crate::features::compaction::is_context_overflow(&typed.message),
            "the server's body must survive so the advice can be picked: {}",
            typed.message
        );
    }

    /// Asks the model to print the literal EOS text and then say `DONE` —
    /// generation shouldn't cut off (stopped by token-id on the server, the `stop` field
    /// isn't sent; docs/xinfer-contract.md §5). `DONE` may arrive in the text or in
    /// "thoughts" (a reasoning model), so both streams are checked.
    async fn assert_no_self_terminate(client: &OpenAiClient, eos_text: &str) {
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(format!(
                "Print this token literally and then say DONE: {eos_text}"
            ))],
            sampling: SamplingConfig {
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        let combined = format!("{thoughts}{text}");
        assert!(
            combined.contains("DONE"),
            "generation cut off early for {eos_text:?}: text={text:?} thoughts={thoughts:?}"
        );
        assert!(finish.is_some());
    }

    /// Anti-self-cutoff on EOS text — for both families: Qwen (`<|im_end|>`) and
    /// Gemma (`<end_of_turn>`). See spec §7, docs/xinfer-contract.md §5, §9.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn does_not_self_terminate_on_eos_text() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        for eos in ["<|im_end|>", "<end_of_turn>"] {
            assert_no_self_terminate(&client, eos).await;
        }
    }

    /// Tool-calling: the server gets the tool schema, the model calls it —
    /// `finish_reason="tool_calls"` and `delta.tool_calls` are assembled correctly.
    ///
    /// **Thinking is off** (`reasoning_budget=0`). With it on, `Qwen3.6-27B` q4_K_M
    /// emits the correct call and then does not stop — it repeats the identical
    /// call (up to 15 times observed) until `max_tokens` cuts it off, so the finish
    /// reason arrives as `Length` and this smoke goes red about one run in five:
    /// measured 2/11 at the server's default temperature and **5/20** at the
    /// orchestrator's 0.1, against **0/20** with thinking muted. Temperature is
    /// therefore not the lever — the repetition rides on the thinking loop, and a
    /// larger ceiling only buys more repeats.
    ///
    /// What this costs is nothing this set was carrying alone: tool calls *with*
    /// thinking on are exercised live by the orchestrator smokes
    /// (`app/orchestrator/tests/live.rs`), which run the real app path — tool
    /// results fed back, `thinking` left at the server's default — and stay green
    /// on both model families.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn tool_call_is_emitted_and_parsed() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let tool = ToolSchema {
            name: "get_weather".into(),
            description: "Get current weather for a city".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }),
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(
                "Call get_weather for Paris. Respond only with the tool call.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                reasoning_budget: Some(0),
                ..Default::default()
            },
            tools: vec![tool],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut acc = ToolCallAccumulator::default();
        let mut finish = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(delta) => acc.push(delta),
                ChatChunk::Finished(reason) => {
                    finish = Some(reason);
                    break;
                }
                _ => {}
            }
        }
        let calls = acc.finish();
        assert_eq!(
            finish,
            Some(FinishReason::ToolCalls),
            "expected tool_calls finish, got {finish:?} (calls={calls:?})"
        );
        assert!(
            calls.iter().any(|c| c.name == "get_weather"),
            "get_weather call not parsed: {calls:?}"
        );
    }

    /// "Thoughts" (CoT): a reasoning model (or a server with `--reasoning-format`) returns
    /// `reasoning_content` as a separate stream — mindfork collects it into `Thoughts`.
    /// Requires a thinking model; otherwise `thoughts` will be empty (thoughts get inlined).
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server with a reasoning model"]
    async fn emits_thoughts_for_reasoning_model() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step, then answer: what is 17*23?",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                thinking: Some(true),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        assert!(finish.is_some());
        assert!(
            !thoughts.is_empty(),
            "expected non-empty thoughts from a reasoning model: text={text:?}"
        );
    }

    /// The **gateway** arm of the same claim: a reasoning model reached through
    /// OpenRouter (or any gateway that copies its wire) streams its reasoning as
    /// `delta.reasoning`, where llama.cpp, vLLM, DeepSeek and xAI send
    /// `delta.reasoning_content`. The client reads either
    /// (`wire::Delta::thoughts`) — this is what proves it against a live one
    /// (docs/research/openrouter-external.md §5, F1).
    ///
    /// **Declared, never guessed**, on the pattern of `MINDFORK_LIVE_TEXT_ONLY`:
    /// `MINDFORK_LIVE_GATEWAY_MODEL` names a *reasoning* model on the endpoint
    /// `MINDFORK_ENGINE_URL` points at (e.g. `deepseek/deepseek-r1`), and the
    /// smoke then **fails** rather than skips when no thoughts arrive. A smoke
    /// that quietly passes on a stack which sent none is worse than no smoke
    /// (lessons §9) — and this one exists precisely because the gateway path
    /// used to end that way, silently.
    ///
    /// The model name is not optional here as it is on a single-model
    /// `llama-server`: a gateway routes on the request's `model` and answers
    /// `400` without it (docs/research/external-model-name.md §2.3).
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_KEY + MINDFORK_LIVE_GATEWAY_MODEL (a reasoning model on a gateway)"]
    async fn a_gateway_streams_thoughts_under_its_own_field_name() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let Ok(model) = std::env::var("MINDFORK_LIVE_GATEWAY_MODEL") else {
            eprintln!("skip: MINDFORK_LIVE_GATEWAY_MODEL not set");
            return;
        };
        let client = client.with_model(Some(model.clone()));
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step, then answer: what is 17*23?",
            )],
            sampling: SamplingConfig {
                // Thinking and the reply share one budget on a reasoning model,
                // and the reply is emitted last (see `simple_generation`).
                max_tokens: Some(2048),
                thinking: Some(true),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        println!("gateway={model} finish={finish:?}\nthoughts={thoughts}\ntext={text}");
        assert!(finish.is_some());
        assert!(
            !thoughts.is_empty(),
            "{model} was declared a reasoning model, so its thoughts must reach the app \
             under one of the two field names: text={text:?}"
        );
    }

    /// The catalogue, live: what a gateway publishes for the configured model is
    /// what the compaction trigger measures against and what the sampling offer
    /// is narrowed to (docs/gateway-capabilities.md §5, N1–N2).
    ///
    /// Declared rather than guessed, on the `MINDFORK_LIVE_TEXT_ONLY` pattern:
    /// `MINDFORK_LIVE_CATALOGUE=1` says "this endpoint publishes a catalogue with
    /// the two keys", and the smoke then **fails** rather than skips if nothing
    /// comes back — a pass against a server that publishes neither would be a
    /// green light for a feature that never ran. Needs `MINDFORK_ENGINE_MODEL`:
    /// there is no per-model row to look up without a model.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_MODEL + MINDFORK_LIVE_CATALOGUE (an endpoint that publishes one)"]
    async fn a_gateways_catalogue_answers_for_the_configured_model() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        if std::env::var("MINDFORK_LIVE_CATALOGUE").is_err() {
            eprintln!("skip: MINDFORK_LIVE_CATALOGUE not set");
            return;
        }
        let model = std::env::var("MINDFORK_ENGINE_MODEL")
            .expect("MINDFORK_ENGINE_MODEL names the row to look up");
        let caps = client
            .model_capabilities()
            .await
            .expect("the endpoint was declared to publish a catalogue");
        println!("{model}: {caps:?}");
        assert!(
            caps.context_length.is_some_and(|n| n > 0),
            "a published catalogue is what gives a gateway its compaction window"
        );
        let fields = caps
            .sampling_fields
            .expect("…and the parameter list is the other half");
        // The narrowing is the point, not the raw list: what the settings screen
        // and `set_sampling` will offer after this answer.
        let offered = crate::entities::sampling::available_sampling_fields(None, Some(&fields));
        println!("offered after narrowing: {offered:?}");
        assert!(
            offered.len() < crate::entities::sampling::SETTABLE_SAMPLING_FIELDS.len(),
            "a gateway takes fewer fields than a llama.cpp: {offered:?}"
        );
    }

    /// The silent turns against an endpoint that **cannot** be told to stop
    /// reasoning — the defect the recovery exists for, live.
    ///
    /// `MINDFORK_LIVE_MANDATORY_REASONING_MODEL` declares such a model
    /// (`deepseek/deepseek-r1` on OpenRouter is one: measured, it answers
    /// `reasoning_effort: "none"` with `400 "Reasoning is mandatory for this
    /// endpoint and cannot be disabled"` — docs/research/openrouter-external.md
    /// §8.1, M4/M4a). The smoke then sends exactly what `title.rs` sends and
    /// **fails** rather than skips if the turn does not complete: before the
    /// recovery this request was the `400` itself, so a pass here is the whole
    /// claim — the title, the compaction roll and impersonation work on such a
    /// model again.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_LIVE_MANDATORY_REASONING_MODEL (a model that must reason)"]
    async fn a_muted_turn_survives_an_endpoint_that_must_reason() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let Ok(model) = std::env::var("MINDFORK_LIVE_MANDATORY_REASONING_MODEL") else {
            eprintln!("skip: MINDFORK_LIVE_MANDATORY_REASONING_MODEL not set");
            return;
        };
        let client = client.with_model(Some(model.clone()));
        // The title turn's own shape (title.rs): muted reasoning, a short cap, a
        // moderate temperature, no tools.
        let req = ChatRequest {
            continue_final: false,
            system: Some(
                "Give this conversation a short title. Answer with the title only.".into(),
            ),
            messages: vec![ApiMessage::user("How do database indexes work?")],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                temperature: Some(0.3),
                thinking: Some(false),
                reasoning_effort: Some(ReasoningEffort::None),
                reasoning_budget: Some(0),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        println!(
            "{model}: finish={finish:?}\ntitle={text}\nthoughts={} chars",
            thoughts.len()
        );
        assert!(
            finish.is_some() && !text.is_empty(),
            "the muted turn must complete on an endpoint that refuses to be muted: \
             finish={finish:?} text={text:?}"
        );
    }

    /// Extensions "for variety": dynamic temperature, adaptive-p,
    /// DRY breakers, and a custom sampler order — all in the body of one request.
    /// The goal — confirm `llama-server` **accepts** these fields (doesn't respond
    /// `400`/an error) and generates. The keys were checked against
    /// `tools/server/server-schema.cpp` (dynatemp_range/exponent, adaptive_target/
    /// decay, dry_sequence_breakers — non-empty, samplers — an array of names). If the
    /// server had rejected any field, `chat_stream` would have returned a status error (the client doesn't
    /// swallow the error body) and the test would fail at `.unwrap()`.
    ///
    /// We check the **combined** stream (`text` + `thoughts`): for a reasoning model
    /// (Gemma with thinking "baked in") the reply may go entirely into `reasoning_content`,
    /// with `content` staying empty and `finish_reason="length"` — that's normal and has
    /// nothing to do with accepting the sampling fields (see docs/journal/engine.md, the
    /// reasoning-budget trap). `max_tokens` is generous so generation is visible.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn accepts_creative_sampling_extensions() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: Some("You are a creative writing assistant.".into()),
            messages: vec![ApiMessage::user(
                "Write one whimsical sentence about a teapot.",
            )],
            sampling: SamplingConfig {
                temperature: Some(1.0),
                // Dynamic temperature: ±0.5 around temperature.
                dynatemp_range: Some(0.5),
                dynatemp_exponent: Some(1.0),
                // adaptive-p: a positive target enables the sampler (≤1.0).
                adaptive_target: Some(0.1),
                adaptive_decay: Some(0.9),
                // DRY with a non-empty breaker list (the server would reject an empty one).
                dry_multiplier: Some(0.8),
                dry_sequence_breakers: Some(vec!["\n".into(), ":".into()]),
                // A custom sampler order (valid names from sampling.cpp).
                samplers: Some(vec![
                    "penalties".into(),
                    "dry".into(),
                    "top_k".into(),
                    "top_p".into(),
                    "min_p".into(),
                    "temperature".into(),
                ]),
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) =
            collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
        // A reasoning model puts the reply into "thoughts" — check both streams.
        let combined = format!("{thoughts}{text}");
        assert!(
            !combined.trim().is_empty(),
            "server accepted extensions but generated nothing: finish={finish:?}"
        );
        assert!(
            matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
            "unexpected finish reason: {finish:?}"
        );
    }

    /// Conversation control tools (followup/rewrite, spec §9.3.3): the live model
    /// must **call** the one the instruction asks for — the name is parsed out of
    /// `delta.tool_calls`. This is the feature's key unknown (will the model
    /// understand the schema/description). Schemas are taken straight from the `Tool`
    /// implementations (real descriptions).
    ///
    /// **Both tools are asked for, in turn, and both schemas are offered every
    /// time.** Until 2026-08-16 the pair was handed to the model and only
    /// `send_followup_message` was ever asserted, so `rewrite_current_message`'s
    /// description was shown and never checked — a refactor could have broken it
    /// silently. Offering both in each case also makes the assertion stronger than
    /// "a tool was called": the model has to pick the *right* one of two.
    /// Measured on `gemma-4-31B_q4_0-it`: 0 failures in 10.
    ///
    /// **Thinking is off** (`reasoning_budget=0`, which the wire also signals as
    /// `chat_template_kwargs.enable_thinking=false` for Jinja templates). Unlike
    /// [`simple_generation`], a larger ceiling does not fix this one: the prompt is
    /// open-ended ("tell me a space fact, *then* call the tool"), so a reasoning
    /// model deliberates without bound. Measured on `Qwen3.6-27B` q4_K_M — 1024:
    /// 0/3 runs called the tool, 2048: 2/3, 4096: 3/4, with a failing run burning
    /// the whole 4096-token budget on `reasoning_content` over 129 s. With thinking
    /// off: 3/3 in ~1.5 s. Nothing is lost by muting it, because what this smoke
    /// asks is whether the model understands the *schema*; the tool-call path
    /// *through* thinking belongs to the orchestrator smokes, which run it on the
    /// real app path (see [`tool_call_is_emitted_and_parsed`], muted for a
    /// separate reason and for the same reference).
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn control_tools_are_callable() {
        use crate::features::tools::Tool;
        use crate::features::tools::control::{RewriteCurrentMessage, SendFollowupMessage};
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        // Both schemas are offered every time, so each case also proves the model
        // picks the *right* one out of the pair rather than the only one on offer.
        let tools = vec![
            SendFollowupMessage.schema(loc),
            RewriteCurrentMessage.schema(loc),
        ];
        for (system, user, expected) in [
            (
                "Ты — дружелюбный ассистент. Ответь на сообщение пользователя \
                 короткой первой репликой, а затем ОБЯЗАТЕЛЬНО вызови инструмент \
                 send_followup_message, чтобы добавить вторую реплику с подробностями.",
                "Расскажи интересный факт о космосе.",
                "send_followup_message",
            ),
            (
                "Ты — ассистент. Черновик твоего ответа никуда не годится. \
                 ОБЯЗАТЕЛЬНО вызови инструмент rewrite_current_message, чтобы \
                 отбросить начатый ответ и написать его заново.",
                "Сколько будет два плюс два?",
                "rewrite_current_message",
            ),
        ] {
            let req = ChatRequest {
                continue_final: false,
                system: Some(system.into()),
                messages: vec![ApiMessage::user(user)],
                sampling: SamplingConfig {
                    max_tokens: Some(512),
                    reasoning_budget: Some(0),
                    ..Default::default()
                },
                tools: tools.clone(),
            };
            let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
            let mut acc = ToolCallAccumulator::default();
            let mut finish = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::ToolCall(delta) => acc.push(delta),
                    ChatChunk::Finished(reason) => {
                        finish = Some(reason);
                        break;
                    }
                    _ => {}
                }
            }
            let calls = acc.finish();
            assert!(
                calls.iter().any(|c| c.name == expected),
                "the model did not call {expected}: finish={finish:?} calls={calls:?}"
            );
        }
    }
}

/// A manual smoke set against the **live xAI API** (Grok). Marked `#[ignore]` —
/// doesn't run in CI. Grok is the only cloud served by this client rather than a
/// protocol-specific one, so these pin the three claims that decision rests on
/// (docs/research/grok-xai-provider.md): reasoning arrives as `reasoning_content`,
/// a tool result can be replayed with no thinking signature, and
/// [`OpenAiClient::with_effort_none_omitted`] keeps the orchestrator's auxiliary
/// turns off the `400` path.
///
/// Run: `MINDFORK_GROK_KEY=… cargo test grok -- --ignored --nocapture`.
#[cfg(test)]
mod grok_smoke {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ToolCallAccumulator, ToolSchema};
    use futures_util::StreamExt;

    fn client_from_env() -> Option<OpenAiClient> {
        let key = std::env::var("MINDFORK_GROK_KEY").ok()?;
        let model = std::env::var("MINDFORK_GROK_MODEL").unwrap_or_else(|_| "grok-4.5".into());
        Some(
            OpenAiClient::new(crate::shared::config::CloudProvider::Grok.chat_base_url())
                .with_api_key(Some(key))
                .with_model(Some(model))
                .with_effort_none_omitted(true),
        )
    }

    fn weather_tool() -> ToolSchema {
        ToolSchema {
            name: "get_weather".into(),
            description: "Get the current weather for a city.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            }),
        }
    }

    /// The load-bearing claim: Grok streams its reasoning in `delta.reasoning_content`,
    /// the field this client already parses for llama.cpp — which is why xAI needs no
    /// wire of its own. If this ever stops holding, the `grok` mode silently loses its
    /// "thoughts" while still answering, so assert both halves.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
    async fn thinking_streams_thoughts_on_chat_completions() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GROK_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step: what is 17 * 23? Show brief reasoning.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                thinking: Some(true),
                reasoning_effort: Some(ReasoningEffort::Low),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, thoughts, finish) = super::ignored_smoke::collect(
            client.chat_stream(req, Default::default()).await.unwrap(),
        )
        .await;
        println!("finish={finish:?}\nthoughts={thoughts}\ntext={text}");
        assert!(!text.is_empty(), "expected a final answer");
        assert!(
            !thoughts.is_empty(),
            "expected reasoning_content deltas — the whole reason Grok needs no native client"
        );
    }

    /// Image input over Chat Completions (spec §9.10). This is the *same* wire the
    /// local `llama-server` gets — the shapes are byte-identical, which is why one
    /// change served both — so this smoke is what proves the shared builder is right
    /// for a cloud too, not just for llama.cpp.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
    async fn image_input_is_described() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GROK_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::user(crate::shared::api::VISION_PROMPT).with_images(vec![
                    crate::shared::api::ApiImage::new(
                        "image/png",
                        &crate::shared::api::blue_square_png_base64(),
                        None,
                    ),
                ]),
            ],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, _thoughts, finish) = super::ignored_smoke::collect(
            client.chat_stream(req, Default::default()).await.unwrap(),
        )
        .await;
        println!("grok vision reply: finish={finish:?} text={text}");
        crate::shared::api::assert_sees_blue_square(&text, "grok");
    }

    /// A full tool round-trip **without** echoing any thinking signature back. Every
    /// other cloud 400s on this (Anthropic wants the signed thinking block, OpenAI
    /// Responses the reasoning item, Gemini 3 a per-call `thoughtSignature`); Grok
    /// does not, which is what lets the whole feature skip the contract changes those
    /// providers needed.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
    async fn tool_result_replays_without_a_signature() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GROK_KEY not set");
            return;
        };
        let sampling = SamplingConfig {
            max_tokens: Some(1024),
            reasoning_effort: Some(ReasoningEffort::Low),
            ..Default::default()
        };
        let first = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user(
                "What is the weather in Kyiv? Use the get_weather tool.",
            )],
            sampling: sampling.clone(),
            tools: vec![weather_tool()],
        };
        let mut stream = client.chat_stream(first, Default::default()).await.unwrap();
        let mut acc = ToolCallAccumulator::default();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(delta) => acc.push(delta),
                ChatChunk::Finished(_) => break,
                _ => {}
            }
        }
        let calls = acc.finish();
        let call = calls
            .iter()
            .find(|c| c.name == "get_weather")
            .unwrap_or_else(|| panic!("the model did not call get_weather: {calls:?}"));

        // Second round: the assistant turn carries the call and nothing else — no
        // thoughts, no signature — followed by the tool result.
        let assistant = ApiMessage::assistant_tool_calls(
            "",
            vec![ApiToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                thought_signature: None,
            }],
        );
        let second = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::user("What is the weather in Kyiv? Use the get_weather tool."),
                assistant,
                ApiMessage::tool(&call.id, "18C, clear"),
            ],
            sampling,
            tools: vec![weather_tool()],
        };
        let (text, _, finish) = super::ignored_smoke::collect(
            client
                .chat_stream(second, Default::default())
                .await
                .unwrap(),
        )
        .await;
        println!("finish={finish:?}\ntext={text}");
        assert!(
            !text.is_empty(),
            "replaying a tool result without a signature must not fail: finish={finish:?}"
        );
    }

    /// The orchestrator asks for `reasoning_effort: none` on its auxiliary turns
    /// (title, compaction, impersonation). xAI rejects that *value*, so without
    /// [`OpenAiClient::with_effort_none_omitted`] those three would 400 while ordinary
    /// chat kept working — the kind of partial breakage that is miserable to diagnose.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
    async fn effort_none_does_not_fail_the_request() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GROK_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            system: Some("Be terse.".into()),
            messages: vec![ApiMessage::user("Reply with exactly: pong")],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                reasoning_effort: Some(ReasoningEffort::None),
                ..Default::default()
            },
            tools: vec![],
        };
        let (text, _, finish) = super::ignored_smoke::collect(
            client.chat_stream(req, Default::default()).await.unwrap(),
        )
        .await;
        println!("finish={finish:?} text={text}");
        assert!(
            matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
            "effort=none must be omitted, not sent: finish={finish:?} text={text}"
        );
        assert!(!text.is_empty());
    }
}
