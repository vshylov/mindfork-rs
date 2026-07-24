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
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, Embedder, EngineBackend, FinishReason, TokenUsage,
    ToolCallDelta,
};
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
}

impl OpenAiClient {
    /// A client to a local/external OpenAI-compatible server: no key, no model
    /// name (llama.cpp extensions are sent as-is).
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            http: reqwest::Client::new(),
            base_url,
            api_key: None,
            model: None,
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
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("probing {url}"))?;
        if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            bail!("server is still loading the model (503)");
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EngineBackend for OpenAiClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        let body = wire::build_chat_request(&req, true, self.model.as_deref());
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .auth(self.http.post(&url))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        // Don't swallow the error body: llama.cpp/OpenAI servers put the reason in JSON
        // (`{"error":{"message":...}}`); without it "error status" is useless. Log it
        // to a file and surface it in the error text (truncating long bodies). See §7.
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.trim().chars().take(500).collect();
            tracing::warn!(%status, body = %detail, "engine returned an error status");
            if detail.is_empty() {
                anyhow::bail!("engine returned status {status}");
            }
            anyhow::bail!("engine returned status {status}: {detail}");
        }

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // A "thoughts" splitter in case of an inline <think> in content.
            let mut parser = ThoughtsParser::new();
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
                                yield ChatChunk::Finished(FinishReason::Stop);
                                break;
                            }
                            Some(Err(err)) => {
                                tracing::warn!(error = %err, "SSE stream error");
                                yield ChatChunk::Finished(FinishReason::Error);
                                break;
                            }
                            Some(Ok(event)) => {
                                if event.data == "[DONE]" {
                                    for piece in parser.finish() { yield piece_to_chunk(piece); }
                                    yield ChatChunk::Finished(FinishReason::Stop);
                                    break;
                                }
                                match serde_json::from_str::<wire::ChatCompletionChunk>(&event.data) {
                                    Ok(chunk) => {
                                        // The token counter (include_usage) arrives as a separate
                                        // chunk (with an empty choices) — emit it before parsing choice.
                                        if let Some(u) = chunk.usage {
                                            yield ChatChunk::Usage(TokenUsage {
                                                prompt_tokens: u.prompt_tokens,
                                                completion_tokens: u.completion_tokens,
                                                reasoning_tokens: u.completion_tokens_details.reasoning_tokens,
                                            });
                                        }
                                        let Some(choice) = chunk.choices.into_iter().next() else { continue };
                                        if let Some(r) = choice.delta.reasoning_content
                                            && !r.is_empty()
                                        {
                                            yield ChatChunk::Thoughts(r);
                                        }
                                        if let Some(c) = choice.delta.content
                                            && !c.is_empty()
                                        {
                                            for piece in parser.push(&c) { yield piece_to_chunk(piece); }
                                        }
                                        if let Some(tool_calls) = choice.delta.tool_calls {
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
                                            yield ChatChunk::Finished(FinishReason::from_wire(&reason));
                                            break;
                                        }
                                    }
                                    Err(err) => {
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
}

#[async_trait::async_trait]
impl Embedder for OpenAiClient {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
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
            .with_context(|| format!("POST {url}"))?;
        // Don't swallow the error body (like chat_stream): llama-server puts the reason in
        // JSON (e.g. "input is too large to process. increase the physical batch
        // size" for a too-long chunk) — without it "error status" is useless.
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.trim().chars().take(500).collect();
            tracing::warn!(%status, body = %detail, "embeddings request returned an error status");
            if detail.is_empty() {
                anyhow::bail!("embedder returned status {status}");
            }
            anyhow::bail!("embedder returned status {status}: {detail}");
        }
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
    let trimmed = base_url.trim_end_matches('/');
    let root = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    format!("{root}/health")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::env::var("MINDFORK_ENGINE_URL")
            .ok()
            .map(OpenAiClient::new)
    }

    async fn collect(stream: ChatStream) -> (String, String, Option<FinishReason>) {
        let mut text = String::new();
        let mut thoughts = String::new();
        let mut finish = None;
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::ThoughtsSignature(_) | ChatChunk::ToolCall(_) | ChatChunk::Usage(_) => {}
                ChatChunk::Finished(r) => {
                    finish = Some(r);
                    break;
                }
            }
        }
        (text, thoughts, finish)
    }

    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            system: Some("You are a helpful assistant.".into()),
            messages: vec![ApiMessage::user("Reply with exactly: pong")],
            sampling: SamplingConfig {
                max_tokens: Some(64),
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

    /// Asks the model to print the literal EOS text and then say `DONE` —
    /// generation shouldn't cut off (stopped by token-id on the server, the `stop` field
    /// isn't sent; docs/xinfer-contract.md §5). `DONE` may arrive in the text or in
    /// "thoughts" (a reasoning model), so both streams are checked.
    async fn assert_no_self_terminate(client: &OpenAiClient, eos_text: &str) {
        let req = ChatRequest {
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
    /// `max_tokens` is generous: a reasoning model "thinks" before the call.
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
            system: None,
            messages: vec![ApiMessage::user(
                "Call get_weather for Paris. Respond only with the tool call.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(512),
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
    /// nothing to do with accepting the sampling fields (see CLAUDE.md, the
    /// reasoning-budget trap). `max_tokens` is generous so generation is visible.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn accepts_creative_sampling_extensions() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
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
    /// must **call** `send_followup_message` per the instruction — `finish_reason=
    /// "tool_calls"` and the name is parsed. This is the feature's key unknown (will
    /// the model understand the schema/description). Schemas are taken straight from the `Tool`
    /// implementations (real descriptions). `max_tokens` is generous — Gemma may "think" before the call.
    #[tokio::test]
    #[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
    async fn control_tools_are_callable() {
        use crate::features::tools::Tool;
        use crate::features::tools::control::{RewriteCurrentMessage, SendFollowupMessage};
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let req = ChatRequest {
            system: Some(
                "Ты — дружелюбный ассистент. Ответь на сообщение пользователя \
                 короткой первой репликой, а затем ОБЯЗАТЕЛЬНО вызови инструмент \
                 send_followup_message, чтобы добавить вторую реплику с подробностями."
                    .into(),
            ),
            messages: vec![ApiMessage::user("Расскажи интересный факт о космосе.")],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                ..Default::default()
            },
            tools: {
                let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
                vec![
                    SendFollowupMessage.schema(loc),
                    RewriteCurrentMessage.schema(loc),
                ]
            },
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
            calls.iter().any(|c| c.name == "send_followup_message"),
            "the model did not call send_followup_message: finish={finish:?} calls={calls:?}"
        );
    }
}
