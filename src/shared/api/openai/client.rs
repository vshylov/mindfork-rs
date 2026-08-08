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
    ChatChunk, ChatRequest, ChatStream, EmbedRole, Embedder, EngineBackend, FinishReason,
    TokenUsage, ToolCallDelta,
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
                                tracing::warn!(error = %err, "SSE stream error");
                                yield ChatChunk::Finished(FinishReason::Error);
                                break;
                            }
                            Some(Ok(event)) => {
                                if event.data == "[DONE]" {
                                    for piece in parser.finish() { yield piece_to_chunk(piece); }
                                    yield ChatChunk::Finished(finish.unwrap_or(FinishReason::Stop));
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
    /// "cannot say". Logged at debug, since not answering is normal here.
    async fn context_budget(&self) -> Option<u32> {
        let url = props_url(&self.base_url);
        let resp = match self.auth(self.http.get(&url)).send().await {
            Ok(r) if r.status().is_success() => r,
            other => {
                tracing::debug!(%url, ok = other.is_ok(), "no context budget from /props");
                return None;
            }
        };
        let props: Props = match resp.json().await {
            Ok(p) => p,
            Err(err) => {
                tracing::debug!(%url, error = %err, "/props did not parse");
                return None;
            }
        };
        // A zero would be a nonsense window; treat it as "cannot say" rather than
        // as a budget every prompt exceeds.
        props
            .default_generation_settings
            .and_then(|g| g.n_ctx)
            .filter(|&n| n > 0)
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
}

#[derive(serde::Deserialize)]
struct PropsGeneration {
    n_ctx: Option<u32>,
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

    async fn collect(url: String) -> Vec<ChatChunk> {
        let client = OpenAiClient::new(url);
        let req = ChatRequest {
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
