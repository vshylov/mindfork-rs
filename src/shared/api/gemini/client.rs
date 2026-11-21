//! An HTTP client to the native Google Gemini API (`POST …:streamGenerateContent?alt=sse`),
//! implementing [`EngineBackend`]. A protocol separate from the OpenAI-compatible Chat
//! Completions path (`OpenAiClient` + the Gemini dialect, now replaced by this client): "thought"
//! summaries (`thinkingConfig.includeThoughts`), depth (`thinkingLevel`/
//! `thinkingBudget`), `thoughtsTokenCount`. See ADR 0004, docs/research/gemini-native-client.md.
//!
//! This client gives no embeddings — RAG in Gemini mode takes a separate source
//! (the OpenAI-compatible `…/v1beta/openai/embeddings`, see the supervisor), like Anthropic.

use anyhow::Result;
use async_stream::stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::wire::{self, GenResponse};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, ToolCallDelta, VisionSupport,
};
use crate::shared::api::effort::{self, EffortMemo, EffortWire};
use crate::shared::api::error::{self, EngineError, SUBJECT_GEMINI};
use crate::shared::api::http;

/// A client to the native Gemini API.
pub struct GeminiClient {
    http: reqwest::Client,
    /// The base URL with a `/v1beta` suffix (the client appends
    /// `/models/{model}:streamGenerateContent`), e.g.
    /// `https://generativelanguage.googleapis.com/v1beta`.
    base_url: String,
    api_key: String,
    model: String,
    /// The thinking levels this model has refused, and what it took in each
    /// one's place. Learned from the refusal, like the Responses client's
    /// memo: a muted turn asks 3.x for `minimal`, and which models have that
    /// level is not something a name tells (docs/research/effort-tiers.md §3).
    efforts: EffortMemo,
}

impl GeminiClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        // The model name may arrive with a `models/` prefix — the path already carries it.
        let model = model
            .into()
            .trim_start_matches("models/")
            .trim()
            .to_string();
        Self {
            http: http::engine_client(),
            base_url,
            api_key: api_key.into(),
            model,
            efforts: EffortMemo::default(),
        }
    }

    /// What this model has been found to take in place of the level `wish` —
    /// unset while it has not refused it. For the tests and smokes, which must
    /// tell a turn that survived a refusal from one that never met it.
    #[cfg(test)]
    pub(super) fn learned(&self, wish: &str) -> Option<Option<&'static str>> {
        self.efforts.instead_of(wish)
    }
}

impl EffortWire for GeminiClient {
    type Body = wire::GenRequest;

    fn model(&self) -> &str {
        &self.model
    }

    fn efforts(&self) -> &EffortMemo {
        &self.efforts
    }

    async fn send(
        &self,
        body: &wire::GenRequest,
        cancel: &CancellationToken,
    ) -> Result<Option<reqwest::Response>, EngineError> {
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url, self.model
        );
        let request = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .json(body);
        let Some(response) = http::send_cancellable(request, cancel).await? else {
            return Ok(None);
        };
        error::check_status(SUBJECT_GEMINI, response)
            .await
            .map(Some)
    }

    /// A refused level becomes the one above it — `minimal`, which a muted turn
    /// asks for, becomes `low` — where the refusal names it, and no level at all
    /// where it does not ([`wire::instead_of_refused`]).
    fn answer(&self, wish: &'static str, refused: &EngineError) -> Option<Option<&'static str>> {
        if !wire::refuses_thinking_level(refused) {
            return None;
        }
        wire::instead_of_refused(wish, &refused.message)
    }
}

/// Gemini's string `finishReason` → the domain reason. The presence of tool calls
/// (`saw_tool_call`) gives `ToolCalls` even on `STOP` (Gemini returns `STOP` with
/// `functionCall` parts), and over a filter reason too — the calls are what the model
/// asked for. `MAX_TOKENS` → `Length`; a filter reason ([`is_filter_reason`]) →
/// `Filtered`; anything else — `Stop`, so as not to fail ([`is_block_reason`]
/// recognizes the other blocking reasons and surfaces a note).
fn map_finish(reason: &str, saw_tool_call: bool) -> FinishReason {
    match reason {
        "MAX_TOKENS" => FinishReason::Length,
        _ if saw_tool_call => FinishReason::ToolCalls,
        _ if is_filter_reason(reason) => FinishReason::Filtered,
        _ => FinishReason::Stop,
    }
}

/// A finish reason saying the provider's content filter stopped the reply. It ends
/// the turn as [`FinishReason::Filtered`], whose note the feed shows in the
/// interface's language — it used to be an English sentence yielded as reply text,
/// stored in the chat and sent back to the model as its own words
/// ([docs/research/content-filter-finish.md](../../../../docs/research/content-filter-finish.md), fork C3).
fn is_filter_reason(reason: &str) -> bool {
    matches!(
        reason,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY"
    )
}

/// A blocking finish reason that is **not** a filter: a malformed call, or `OTHER`.
/// Such a reply arrives empty — without an explanation the user would see a silently
/// empty turn, so it's surfaced as a note (see [`block_note`]).
fn is_block_reason(reason: &str) -> bool {
    matches!(reason, "MALFORMED_FUNCTION_CALL" | "OTHER")
}

/// A note to the user about the block (goes into the feed as reply text, so an empty turn
/// is explainable).
fn block_note(reason: &str) -> String {
    format!("\n⚠ Gemini did not produce a response (reason: {reason}).")
}

/// A finish reason saying Gemini discarded the reply its model generated as
/// malformed. It comes after the thoughts and with no text: measured on
/// `gemini-4-neon` (2026-11-21), 9 replays in 10 of one round after a tool call,
/// each 3500–4600 thought tokens and then nothing, in a chat whose history held
/// an earlier reply that had leaked its reasoning into the text
/// (docs/journal/engine.md, "a reply Gemini discards as malformed"). Not a
/// filter, and not one of the [`is_block_reason`]s, whose note goes into the
/// reply text: this one is reported through the stream's error channel, so the
/// note is the feed's, in the interface's language, and nothing of ours is
/// stored as the model's words.
fn is_malformed_response(reason: &str) -> bool {
    reason == "MALFORMED_RESPONSE"
}

/// The error a malformed reply is reported with — the `{err}` of the feed's
/// note about a reply cut short.
fn malformed_text(reason: &str) -> String {
    format!("Gemini discarded the reply it generated as malformed ({reason})")
}

#[async_trait::async_trait]
impl EngineBackend for GeminiClient {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        // A level the model does not have is asked again as the one above it —
        // the muted turns' `minimal` on a 3.x Flash that starts at `low`.
        let mut body = wire::build_request(&req, &self.model);
        let Some(response) = effort::send_asking_again(self, &mut body, &cancel).await? else {
            return Ok(http::cancelled_stream());
        };

        let mut events = response.bytes_stream().eventsource();

        let s = stream! {
            // The ordinal index of the tool call (Gemini has no call_id — synthesize a
            // stable id `"{name}-{index}"`; functionResponse matching goes by it).
            let mut tool_index = 0usize;
            let mut saw_tool_call = false;
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
                                yield ChatChunk::Finished(FinishReason::Stop);
                                break;
                            }
                            Some(Err(err)) => {
                                let message = error::chain_text(&err);
                                tracing::warn!(error = %message, "SSE stream error (gemini)");
                                for chunk in ChatChunk::failure(message, true) { yield chunk; }
                                break;
                            }
                            Some(Ok(event)) => {
                                if event.data == "[DONE]" {
                                    yield ChatChunk::Finished(FinishReason::Stop);
                                    break;
                                }
                                let resp: GenResponse = match serde_json::from_str(&event.data) {
                                    Ok(r) => r,
                                    Err(err) => {
                                        tracing::warn!(error = %err, data = %event.data, "failed to parse gemini SSE chunk");
                                        continue;
                                    }
                                };
                                // An error reported inside the stream. Checked before
                                // anything else: every other field of GenResponse
                                // defaults, so this payload is otherwise a valid
                                // empty chunk and the turn ended as a silent Stop.
                                if let Some(e) = resp.error {
                                    let transient = error::stream_error_transient(&e.status, e.code);
                                    tracing::warn!(
                                        status = %e.status,
                                        code = ?e.code,
                                        transient,
                                        message = %e.message,
                                        "gemini reported an error inside the stream"
                                    );
                                    let message = error::stream_error_text(&e.status, &e.message);
                                    for chunk in ChatChunk::failure(message, transient) { yield chunk; }
                                    break;
                                }
                                // The prompt was blocked by the filter (candidates is empty) —
                                // ended as filtered, otherwise it would look like an empty STOP.
                                if let Some(reason) = resp
                                    .prompt_feedback
                                    .and_then(|f| f.block_reason)
                                    .filter(|r| !r.is_empty())
                                {
                                    tracing::warn!(reason = %reason, "gemini blocked the prompt");
                                    yield ChatChunk::Finished(FinishReason::Filtered);
                                    break;
                                }
                                let candidate = resp.candidates.into_iter().next();
                                if let Some(cand) = &candidate
                                    && let Some(content) = &cand.content
                                {
                                    for part in &content.parts {
                                        if let Some(fc) = &part.function_call {
                                            saw_tool_call = true;
                                            yield ChatChunk::ToolCall(ToolCallDelta {
                                                index: tool_index,
                                                id: Some(format!("{}-{}", fc.name, tool_index)),
                                                name: Some(fc.name.clone()),
                                                arguments: fc.args.to_string(),
                                                // The thought signature (Gemini 3) — on the functionCall part.
                                                thought_signature: part.thought_signature.clone(),
                                            });
                                            tool_index += 1;
                                            continue;
                                        }
                                        // A reply without calls carries its signature on
                                        // the last part — in a stream, often an empty text
                                        // part (`{"text":"","thoughtSignature":…}`, measured
                                        // 2026-11-20 on Gemini 4), which the empty-text
                                        // check below would drop with it.
                                        if let Some(sig) = &part.thought_signature {
                                            yield ChatChunk::TextSignature(sig.clone());
                                        }
                                        if let Some(text) = &part.text {
                                            if text.is_empty() {
                                                continue;
                                            }
                                            if part.thought == Some(true) {
                                                yield ChatChunk::Thoughts(text.clone());
                                            } else {
                                                yield ChatChunk::Text(text.clone());
                                            }
                                        }
                                    }
                                }
                                if let Some(u) = resp.usage_metadata {
                                    yield ChatChunk::Usage(u.token_usage());
                                }
                                if let Some(reason) = candidate.and_then(|c| c.finish_reason) {
                                    // A blocking reason that is not a filter (a malformed call,
                                    // OTHER) — surfaced as a note, otherwise an empty turn would
                                    // go unexplained. A filter reason ends as `Filtered` below.
                                    if is_filter_reason(&reason) {
                                        tracing::warn!(reason = %reason, "gemini stopped the reply with a filter reason");
                                    }
                                    // A reply Gemini threw away as malformed ends as a
                                    // failure — the feed's note, the thoughts kept — and
                                    // not as a stop: it arrives with no text, and read as
                                    // a stop it was an empty turn with no word of why.
                                    if is_malformed_response(&reason) && !saw_tool_call {
                                        tracing::warn!(reason = %reason, "gemini discarded the reply as malformed");
                                        for chunk in ChatChunk::failure(malformed_text(&reason), false) { yield chunk; }
                                        break;
                                    }
                                    if is_block_reason(&reason) && !saw_tool_call {
                                        tracing::warn!(reason = %reason, "gemini stopped with a block reason");
                                        yield ChatChunk::Text(block_note(&reason));
                                    }
                                    yield ChatChunk::Finished(map_finish(&reason, saw_tool_call));
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    /// Gemini takes images on every current model, so the answer is static.
    ///
    /// Deliberately **not** a model-name allowlist: a hardcoded list of vision
    /// models goes stale the week after it is written and then lies confidently —
    /// the trap docs/research/grok-xai-provider.md recorded for reasoning detection.
    /// A genuinely text-only model returns a clear provider error on send, which is
    /// a far better failure than refusing an attach on a guess.
    async fn vision(&self) -> VisionSupport {
        VisionSupport::Supported
    }

    /// `responseJsonSchema` constrains the reply by grammar on every model
    /// measured, Gemini 2.5 to 3.8 and the Gemma 4 the API serves
    /// ([`wire::GenConfig::response_json_schema`]).
    async fn enforces_output_schema(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::sse_stub::{self, collect, serve};

    /// The story engine sends its schemas to Gemini (docs/history/fabula.md §4, PR 5).
    #[tokio::test]
    async fn the_gemini_client_enforces_a_schema() {
        let client = GeminiClient::new("http://127.0.0.1:9", "k", "gemini-x");
        assert!(client.enforces_output_schema().await);
    }

    #[test]
    fn finish_mapping() {
        assert_eq!(map_finish("STOP", false), FinishReason::Stop);
        assert_eq!(map_finish("STOP", true), FinishReason::ToolCalls);
        assert_eq!(map_finish("MAX_TOKENS", false), FinishReason::Length);
        assert_eq!(map_finish("SAFETY", false), FinishReason::Filtered);
        assert_eq!(map_finish("RECITATION", false), FinishReason::Filtered);
        // The calls the model produced win over a filter reason after them.
        assert_eq!(map_finish("SAFETY", true), FinishReason::ToolCalls);
        assert_eq!(map_finish("OTHER", false), FinishReason::Stop);
    }

    #[test]
    fn strips_models_prefix_from_model() {
        let c = GeminiClient::new("https://x/v1beta", "k", "models/gemini-2.5-flash");
        assert_eq!(c.model, "gemini-2.5-flash");
    }

    #[test]
    fn block_reasons_recognized_and_noted() {
        assert!(is_block_reason("MALFORMED_FUNCTION_CALL"));
        assert!(is_block_reason("OTHER"));
        // A filter reason is not a block reason: it has a finish reason of its own.
        assert!(!is_block_reason("SAFETY"));
        assert!(!is_block_reason("STOP"));
        assert!(!is_block_reason("MAX_TOKENS"));
        // The note carries the reason.
        assert!(block_note("OTHER").contains("OTHER"));
    }

    #[test]
    fn filter_reasons_are_told_from_the_other_blocks() {
        for r in [
            "SAFETY",
            "RECITATION",
            "BLOCKLIST",
            "PROHIBITED_CONTENT",
            "SPII",
            "IMAGE_SAFETY",
        ] {
            assert!(is_filter_reason(r), "{r}");
            assert!(!is_block_reason(r), "{r}");
        }
        for r in ["STOP", "MAX_TOKENS", "MALFORMED_FUNCTION_CALL", "OTHER"] {
            assert!(!is_filter_reason(r), "{r}");
        }
    }

    /// A reply the filter stopped keeps what arrived, gains no text of ours, and ends
    /// as filtered — the note is the feed's, in the interface's language.
    #[tokio::test]
    async fn a_safety_stop_ends_the_turn_as_filtered_with_no_text_of_ours() {
        let base = serve(&[
            r#"{"candidates":[{"content":{"parts":[{"text":"Once upon"}],"role":"model"}}]}"#,
            r#"{"candidates":[{"content":{"parts":[],"role":"model"},"finishReason":"SAFETY"}]}"#,
        ]);
        let client = GeminiClient::new(base, "k", "gemini-x");
        let chunks = collect(
            client
                .chat_stream(sse_stub::hello(), Default::default())
                .await
                .unwrap(),
        )
        .await;
        let texts: Vec<_> = chunks
            .iter()
            .filter_map(|c| match c {
                ChatChunk::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Once upon"], "{chunks:?}");
        assert_eq!(
            chunks.last(),
            Some(&ChatChunk::Finished(FinishReason::Filtered))
        );
    }

    /// A refused prompt — no candidates, a `blockReason` — ends the same way.
    #[tokio::test]
    async fn a_blocked_prompt_ends_the_turn_as_filtered() {
        let base = serve(&[r#"{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT"}}"#]);
        let client = GeminiClient::new(base, "k", "gemini-x");
        let chunks = collect(
            client
                .chat_stream(sse_stub::hello(), Default::default())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            chunks,
            [ChatChunk::Finished(FinishReason::Filtered)],
            "no reply text, only the reason"
        );
    }

    /// A block that is not a filter keeps the note it had.
    #[tokio::test]
    async fn a_malformed_call_still_gets_its_note() {
        let base = serve(&[
            r#"{"candidates":[{"content":{"parts":[],"role":"model"},"finishReason":"MALFORMED_FUNCTION_CALL"}]}"#,
        ]);
        let client = GeminiClient::new(base, "k", "gemini-x");
        let chunks = collect(
            client
                .chat_stream(sse_stub::hello(), Default::default())
                .await
                .unwrap(),
        )
        .await;
        assert!(
            chunks.contains(&ChatChunk::Text(block_note("MALFORMED_FUNCTION_CALL"))),
            "{chunks:?}"
        );
        assert_eq!(
            chunks.last(),
            Some(&ChatChunk::Finished(FinishReason::Stop))
        );
    }

    /// A reply Gemini discarded as malformed — its thoughts, then an empty part
    /// with the signature and `MALFORMED_RESPONSE`, the shape `gemini-4-neon`
    /// sent on 2026-11-21 — ends as a failure, not a stop: the thoughts are kept,
    /// no text of ours is made the model's, and the error names the reason.
    #[tokio::test]
    async fn a_malformed_response_ends_as_a_failure_with_its_thoughts() {
        assert!(!is_filter_reason("MALFORMED_RESPONSE"));
        assert!(
            !is_block_reason("MALFORMED_RESPONSE"),
            "no note in the text"
        );
        let base = serve(&[
            r#"{"candidates":[{"content":{"parts":[{"text":"Weighing the bug report","thought":true}],"role":"model"}}]}"#,
            r#"{"candidates":[{"content":{"parts":[{"text":"","thoughtSignature":"SIG"}],"role":"model"},"finishReason":"MALFORMED_RESPONSE"}]}"#,
        ]);
        let client = GeminiClient::new(base, "k", "gemini-4-neon");
        let chunks = collect(
            client
                .chat_stream(sse_stub::hello(), Default::default())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            chunks,
            [
                ChatChunk::Thoughts("Weighing the bug report".into()),
                ChatChunk::TextSignature("SIG".into()),
                ChatChunk::Error {
                    message: malformed_text("MALFORMED_RESPONSE"),
                    transient: false,
                },
                ChatChunk::Finished(FinishReason::Error),
            ]
        );
        assert!(malformed_text("MALFORMED_RESPONSE").contains("MALFORMED_RESPONSE"));
    }

    /// Calls the model made before the reason still win, as they do over a
    /// filter: the round ran them, and the reason changes nothing they asked for.
    #[tokio::test]
    async fn a_malformed_response_after_a_call_keeps_the_call() {
        let base = serve(&[
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"calc","args":{}},"thoughtSignature":"SIG"}],"role":"model"},"finishReason":"MALFORMED_RESPONSE"}]}"#,
        ]);
        let client = GeminiClient::new(base, "k", "gemini-4-neon");
        let chunks = collect(
            client
                .chat_stream(sse_stub::hello(), Default::default())
                .await
                .unwrap(),
        )
        .await;
        assert!(
            !chunks.iter().any(|c| matches!(c, ChatChunk::Error { .. })),
            "{chunks:?}"
        );
        assert_eq!(
            chunks.last(),
            Some(&ChatChunk::Finished(FinishReason::ToolCalls))
        );
    }

    /// A whole reply: one word, and the reason it stopped.
    const REPLY: sse_stub::Canned = (
        "200 OK",
        "text/event-stream",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Indexes\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}]}

",
    );

    /// The `thinkingLevel` of each request the stub was sent, in order.
    fn levels_sent(bodies: &[String]) -> Vec<serde_json::Value> {
        bodies
            .iter()
            .map(|b| {
                let body: serde_json::Value =
                    serde_json::from_str(b).unwrap_or_else(|e| panic!("{e}: {b:?}"));
                body["generationConfig"]["thinkingConfig"]["thinkingLevel"].clone()
            })
            .collect()
    }

    /// The defect: `gemini-3.8-flash` answers a muted turn's `minimal` with a
    /// `400`, and nothing in its name says so — so the title, the roll and
    /// impersonation failed there while ordinary chat worked. The turn completes
    /// on the second request, at the level above, and the next muted turn asks
    /// for that at once.
    #[tokio::test]
    async fn a_refused_minimal_level_is_asked_again_as_low() {
        let (url, stub) = sse_stub::serve_in_turn(&[
            (sse_stub::BAD_REQUEST, sse_stub::JSON, wire::LEVEL_REFUSED),
            REPLY,
            REPLY,
        ]);
        let client = GeminiClient::new(url, "k", "gemini-3.8-flash");
        for _ in 0..2 {
            let chunks = collect(
                client
                    .chat_stream(sse_stub::muted(), CancellationToken::new())
                    .await
                    .expect("the muted turn survives the refusal"),
            )
            .await;
            assert!(
                chunks.contains(&ChatChunk::Text("Indexes".into())),
                "{chunks:?}"
            );
        }
        assert_eq!(
            levels_sent(&stub.join().unwrap()),
            ["minimal", "low", "low"]
        );
        assert_eq!(client.learned("minimal"), Some(Some("low")));
    }

    /// Any other `400` is reported as it came, after one request: the stub has
    /// a single answer, so a second request would meet a closed port and the
    /// error would be a transport one.
    #[tokio::test]
    async fn another_refusal_of_a_muted_turn_is_reported_as_it_came() {
        let (url, stub) = sse_stub::serve_in_turn(&[(
            sse_stub::BAD_REQUEST,
            sse_stub::JSON,
            r#"{"error":{"code":400,"message":"The input token count exceeds the maximum number of tokens allowed.","status":"INVALID_ARGUMENT"}}"#,
        )]);
        let client = GeminiClient::new(url, "k", "gemini-3.8-flash");
        let err = client
            .chat_stream(sse_stub::muted(), CancellationToken::new())
            .await
            .err()
            .expect("a 400 about the input is an error");
        assert!(err.to_string().contains("input token count"), "{err}");
        assert_eq!(stub.join().unwrap().len(), 1);
        assert_eq!(client.learned("minimal"), None);
    }

    /// A refused level with none above it has nothing to become.
    #[tokio::test]
    async fn a_refused_top_level_is_reported_as_it_came() {
        let (url, stub) = sse_stub::serve_in_turn(&[(
            sse_stub::BAD_REQUEST,
            sse_stub::JSON,
            wire::LEVEL_REFUSED,
        )]);
        let client = GeminiClient::new(url, "k", "gemini-3.8-flash");
        let mut req = sse_stub::hello();
        req.sampling.reasoning_effort = Some(crate::entities::sampling::ReasoningEffort::High);
        let err = client
            .chat_stream(req, CancellationToken::new())
            .await
            .err()
            .expect("the refusal is the answer");
        assert!(err.to_string().contains("Thinking level"), "{err}");
        assert_eq!(stub.join().unwrap().len(), 1);
        assert_eq!(client.learned("high"), None);
    }

    /// Gemma 4's answer to `low` and `medium` (2026-11-20): no level named.
    const LEVEL_REFUSED_UNNAMED: &str = r#"{"error":{"code":400,"message":"Thinking level is not supported for this model.","status":"INVALID_ARGUMENT"}}"#;

    /// A refusal that names no level is answered by asking with none — the
    /// model's own depth, thoughts still on — not with a further guess, and the
    /// next turn at that effort asks so at once.
    #[tokio::test]
    async fn a_refusal_naming_no_level_drops_the_level() {
        let (url, stub) = sse_stub::serve_in_turn(&[
            (sse_stub::BAD_REQUEST, sse_stub::JSON, LEVEL_REFUSED_UNNAMED),
            REPLY,
            REPLY,
        ]);
        let client = GeminiClient::new(url, "k", "gemma-4-31b-it");
        let mut req = sse_stub::hello();
        req.sampling.thinking = Some(true);
        req.sampling.reasoning_effort = Some(crate::entities::sampling::ReasoningEffort::Low);
        for _ in 0..2 {
            let chunks = collect(
                client
                    .chat_stream(req.clone(), CancellationToken::new())
                    .await
                    .expect("the turn survives the refusal"),
            )
            .await;
            assert!(
                chunks.contains(&ChatChunk::Text("Indexes".into())),
                "{chunks:?}"
            );
        }
        let bodies = stub.join().unwrap();
        assert_eq!(
            levels_sent(&bodies),
            [
                serde_json::json!("low"),
                serde_json::Value::Null,
                serde_json::Value::Null
            ]
        );
        let last: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
        assert_eq!(
            last["generationConfig"]["thinkingConfig"]["includeThoughts"], true,
            "the thoughts stay on: {last}"
        );
        assert_eq!(client.learned("low"), Some(None));
    }

    /// The defect this guards: every Gemini 4 model fell to the 2.5 branch and
    /// a muted turn was sent `thinkingBudget: 0`, which `gemini-4-neon` takes and
    /// thinks through anyway (181 thought tokens, 2026-11-20). It is a model of
    /// levels: `minimal`, refused, then `low`.
    #[tokio::test]
    async fn a_muted_turn_on_gemini_4_asks_for_levels() {
        let (url, stub) = sse_stub::serve_in_turn(&[
            (sse_stub::BAD_REQUEST, sse_stub::JSON, wire::LEVEL_REFUSED),
            REPLY,
        ]);
        let client = GeminiClient::new(url, "k", "gemini-4-neon");
        collect(
            client
                .chat_stream(sse_stub::muted(), CancellationToken::new())
                .await
                .expect("the muted turn survives the refusal"),
        )
        .await;
        let bodies = stub.join().unwrap();
        assert_eq!(levels_sent(&bodies), ["minimal", "low"]);
        for b in &bodies {
            assert!(!b.contains("thinkingBudget"), "{b}");
        }
    }

    /// A reply without calls carries its signature on a trailing empty text part
    /// (the stream of `gemini-4-neon`, 2026-11-20): the signature is handed on,
    /// the empty text is not, and a call's signature stays on the call.
    #[tokio::test]
    async fn a_replys_signature_on_a_trailing_empty_part_is_handed_on() {
        let base = serve(&[
            r#"{"candidates":[{"content":{"parts":[{"text":"Weighing it","thought":true}],"role":"model"}}]}"#,
            r#"{"candidates":[{"content":{"parts":[{"text":"391"}],"role":"model"}}]}"#,
            r#"{"candidates":[{"content":{"parts":[{"text":"","thoughtSignature":"SIG-TEXT"}],"role":"model"},"finishReason":"STOP"}]}"#,
        ]);
        let client = GeminiClient::new(base, "k", "gemini-4-neon");
        let chunks = collect(
            client
                .chat_stream(sse_stub::hello(), Default::default())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            chunks,
            [
                ChatChunk::Thoughts("Weighing it".into()),
                ChatChunk::Text("391".into()),
                ChatChunk::TextSignature("SIG-TEXT".into()),
                ChatChunk::Finished(FinishReason::Stop),
            ]
        );

        let base = serve(&[
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"calc","args":{}},"thoughtSignature":"SIG-CALL"}],"role":"model"},"finishReason":"STOP"}]}"#,
        ]);
        let client = GeminiClient::new(base, "k", "gemini-4-neon");
        let chunks = collect(
            client
                .chat_stream(sse_stub::hello(), Default::default())
                .await
                .unwrap(),
        )
        .await;
        assert!(
            !chunks
                .iter()
                .any(|c| matches!(c, ChatChunk::TextSignature(_))),
            "{chunks:?}"
        );
        assert!(
            chunks.iter().any(|c| matches!(
                c,
                ChatChunk::ToolCall(d) if d.thought_signature.as_deref() == Some("SIG-CALL")
            )),
            "{chunks:?}"
        );
    }
}

/// A manual smoke against the real Gemini API. Marked `#[ignore]` — not in CI.
/// Run: `MINDFORK_GEMINI_KEY=… cargo test gemini -- --ignored --nocapture`.
#[cfg(test)]
mod ignored_smoke {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::ToolCallAccumulator;
    use crate::shared::api::contract::{ApiMessage, ToolSchema};

    pub(super) fn client_from_env() -> Option<GeminiClient> {
        let key = std::env::var("MINDFORK_GEMINI_KEY").ok()?;
        let model =
            std::env::var("MINDFORK_GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
        Some(GeminiClient::new(
            "https://generativelanguage.googleapis.com/v1beta",
            key,
            model,
        ))
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn simple_generation() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            output_schema: None,
            system: Some("You are a helpful assistant.".into()),
            messages: vec![ApiMessage::user("Reply with exactly: pong")],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut text = String::new();
        let mut finish = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Finished(r) => {
                    finish = Some(r);
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert!(!text.is_empty(), "expected non-empty response");
        assert!(matches!(
            finish,
            Some(FinishReason::Stop | FinishReason::Length)
        ));
    }

    /// The **fallback** an image in a tool result takes on Gemini, and the only place it
    /// is exercised end to end (fork F1-A of docs/research/mcp-tool-images.md).
    ///
    /// Gemini is the one provider that refuses a multimodal `functionResponse` — measured,
    /// a hard `400` "Multimodal function responses are not supported for this model" —
    /// so its builder emits the image as user parts *after* the response instead. This
    /// test is what proves that detour still reaches the model, and that the request is
    /// accepted at all: a regression putting the image back inside the `functionResponse`
    /// would fail here with that 400 rather than silently degrade.
    ///
    /// Control arm included, for the reason recorded in §2.1 of that document.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn tool_result_image_takes_the_user_part_fallback() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let turn = |with_image: bool| {
            let tool = ApiMessage::tool("take_screenshot-0", "Screenshot taken.");
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
                output_schema: None,
                system: None,
                // The shape a real turn has: the question is asked **up front**, and the
                // tool result is the last message — the model answers from it. A trailing
                // user message would be unrealistic *and* wrong here: Gemini puts a tool
                // result in a `user` content and adjacent same-role contents merge, so the
                // question would end up sharing one content with the `functionResponse` —
                // measured, Gemini then returns an empty candidate (`STOP`, zero
                // completion tokens) while still billing the image.
                messages: vec![
                    ApiMessage::user(crate::shared::api::TOOL_VISION_PROMPT),
                    ApiMessage::assistant_tool_calls(
                        "",
                        vec![crate::shared::api::ApiToolCall {
                            id: "take_screenshot-0".into(),
                            name: "take_screenshot".into(),
                            arguments: "{}".into(),
                            // The call is made up here, not the model's: from Gemini 3
                            // on, an unsigned one is a `400` ("Function call is missing
                            // a thought_signature"), and this is the value Google
                            // documents for a call injected into the history (measured
                            // 2026-11-20 on `gemini-4-neon`; 2.5 ignores it).
                            thought_signature: Some("skip_thought_signature_validator".into()),
                        }],
                    ),
                    tool,
                ],
                sampling: SamplingConfig {
                    max_tokens: Some(2048),
                    // Thinking muted, or there is no answer to assert on: 2.5-flash
                    // thinks by default and the budget is **shared** with the reply, so
                    // both arms come back empty and the failure reads as "the image did
                    // not arrive" (docs/lessons.md §3 — measured here first).
                    reasoning_budget: Some(0),
                    ..Default::default()
                },
                tools: vec![crate::shared::api::ToolSchema {
                    name: "take_screenshot".into(),
                    description: "Take a screenshot of the screen.".into(),
                    parameters: serde_json::json!({ "type": "object", "properties": {} }),
                }],
            }
        };
        let read = async |req| {
            let mut stream: ChatStream = client.chat_stream(req, Default::default()).await.unwrap();
            let mut text = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
                    ChatChunk::Finished(_) => break,
                    _ => {}
                }
            }
            text
        };

        let control = read(turn(false)).await;
        eprintln!("gemini control (no image): {control}");
        crate::shared::api::assert_sees_green_circle(&control, false, "control");

        let answer = read(turn(true)).await;
        eprintln!("gemini tool-result image: {answer}");
        crate::shared::api::assert_sees_green_circle(&answer, true, "with the image");
    }

    /// Image input (spec §9.10): an `inline_data` part reaches the model and is
    /// described. Verified live before the wire was written — see
    /// docs/research/multimodal-images.md §2.2.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn image_input_is_described() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            output_schema: None,
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
            // 2.5-flash thinks by default and the budget is shared with the reply, so a
            // small cap here would return an empty answer rather than a wrong one.
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
                ChatChunk::Finished(_) => break,
                _ => {}
            }
        }
        eprintln!("gemini vision reply: {text}");
        crate::shared::api::assert_sees_blue_square(&text, "gemini");
    }

    /// Reasoning summary: with `thinking=true`, "thoughts" (Thoughts) and the reply arrive.
    /// `max_tokens` is generous — thought tokens eat into the reply budget.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn thinking_streams_thoughts() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let req = ChatRequest {
            continue_final: false,
            output_schema: None,
            system: None,
            messages: vec![ApiMessage::user(
                "Think step by step: what is 17 * 23? Show brief reasoning.",
            )],
            sampling: SamplingConfig {
                max_tokens: Some(4096),
                thinking: Some(true),
                reasoning_effort: Some(ReasoningEffort::Medium),
                ..Default::default()
            },
            tools: vec![],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut thoughts = String::new();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Finished(_) => break,
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert!(
            !text.is_empty(),
            "expected final answer, thoughts={thoughts:?}"
        );
    }

    /// One tool round: the model calls the tool (without resending the signature — Phase B).
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn single_tool_call() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let tool = ToolSchema {
            name: "get_weather".into(),
            description: "Get the current weather for a city.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            }),
        };
        let req = ChatRequest {
            continue_final: false,
            output_schema: None,
            system: None,
            messages: vec![ApiMessage::user("Call get_weather for Paris.")],
            sampling: SamplingConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
            tools: vec![tool],
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut acc = ToolCallAccumulator::default();
        let mut reason = FinishReason::Stop;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Finished(r) => {
                    reason = r;
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert_eq!(
            reason,
            FinishReason::ToolCalls,
            "model should call the tool"
        );
        let calls = acc.finish();
        assert!(!calls.is_empty(), "expected a tool call");
        assert_eq!(calls[0].name, "get_weather");
    }

    /// Phase B: a tool-use round-trip resending the thought signature. On **Gemini 3**
    /// (`MINDFORK_GEMINI_MODEL=gemini-3-*`) the first round gives a call + `thoughtSignature`;
    /// the second resends it on `functionCall` + the result — Gemini must not return
    /// `400` ("missing thought_signature"). On 2.5 the signature is optional (the round also
    /// goes through). Checks: the signature arrived and resending doesn't break the round.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API), Gemini 3 for signatures"]
    async fn tool_use_round_trips_signature() {
        let Some(client) = client_from_env() else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let tool = ToolSchema {
            name: "get_weather".into(),
            description: "Get the current weather for a city.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            }),
        };
        let sampling = SamplingConfig {
            max_tokens: Some(4096),
            thinking: Some(true),
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        };
        let prompt = "Reason briefly which of Paris or Berlin is the capital of France, \
             then call get_weather for that city.";
        let round1 = ChatRequest {
            continue_final: false,
            output_schema: None,
            system: None,
            messages: vec![ApiMessage::user(prompt)],
            sampling: sampling.clone(),
            tools: vec![tool.clone()],
        };
        let mut stream = client
            .chat_stream(round1, Default::default())
            .await
            .unwrap();
        let mut acc = ToolCallAccumulator::default();
        let mut reason = FinishReason::Stop;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Finished(r) => {
                    reason = r;
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert_eq!(
            reason,
            FinishReason::ToolCalls,
            "model should call the tool"
        );
        let calls = acc.finish();
        assert!(!calls.is_empty(), "expected a tool call");
        let call = calls[0].clone();
        eprintln!(
            "thought_signature present: {}",
            call.thought_signature.is_some()
        );

        // Second round: assistant(functionCall with the signature) + the result.
        let round2 = ChatRequest {
            continue_final: false,
            output_schema: None,
            system: None,
            messages: vec![
                ApiMessage::user(prompt),
                ApiMessage::assistant_tool_calls("", vec![call.clone()]),
                ApiMessage::tool(&call.id, "18°C, sunny"),
            ],
            sampling,
            tools: vec![tool],
        };
        let mut stream = client
            .chat_stream(round2, Default::default())
            .await
            .unwrap();
        let mut text = String::new();
        let mut finish = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Finished(r) => {
                    finish = Some(r);
                    break;
                }
                ChatChunk::Error { message, .. } => {
                    eprintln!("engine error: {message}");
                }
                _ => {}
            }
        }
        assert!(
            matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
            "second round must succeed (no 400), got {finish:?}"
        );
        assert!(
            !text.is_empty(),
            "expected a final answer after tool result"
        );
    }
}

#[cfg(test)]
#[path = "client/tests/thinking_live.rs"]
mod thinking_live;
