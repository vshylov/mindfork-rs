//! The inference engine contract (`EngineBackend`) and its types. Hides the transport
//! (HTTP to xinfer) behind a trait — testability (mock/replay) and the ability
//! to swap the transport. See spec §6.1 and docs/xinfer-contract.md §8.

use std::pin::Pin;

use anyhow::Result;
use futures_util::Stream;
use tokio_util::sync::CancellationToken;

use crate::entities::sampling::SamplingConfig;

/// The role of a message in a request to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiRole {
    /// The system role. The system message is passed via [`ChatRequest::system`],
    /// so it's never constructed as a message role — kept for the enum's completeness.
    #[allow(dead_code)]
    System,
    User,
    Assistant,
    Tool,
}

impl ApiRole {
    pub fn as_wire(self) -> &'static str {
        match self {
            ApiRole::System => "system",
            ApiRole::User => "user",
            ApiRole::Assistant => "assistant",
            ApiRole::Tool => "tool",
        }
    }
}

/// A tool call by the assistant (in the history of an assistant message). See spec §9.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApiToolCall {
    pub id: String,
    pub name: String,
    /// Arguments as a JSON string (that's how the server returns/accepts them).
    pub arguments: String,
    /// The thought signature tied to the call (Gemini 3 `thoughtSignature`). Needed for
    /// resending on tool-use: Gemini 3 requires it on `functionCall` parts (otherwise
    /// `400`). Other backends — `None` (Anthropic/OpenAI have one signature per turn, carried
    /// via [`ThinkingRef`]). See docs/research/gemini-native-client.md §2.3.
    pub thought_signature: Option<String>,
}

/// A reasoning block (extended thinking / reasoning item) for resending in history.
/// Needed by providers that require the reasoning to be returned along with the tool call
/// in the same turn, otherwise the round's next request returns `400`/degrades:
/// - **Anthropic**: a thinking block with `signature` on an assistant turn with `tool_use`
///   (see [`anthropic::wire`](super::anthropic));
/// - **OpenAI Responses**: a reasoning item (`id` + `encrypted_content`) right before
///   its own `function_call` (see [`openai::responses`](super::openai)).
///
/// Other backends (llama.cpp Chat Completions) ignore the field. Not needed between turns
/// (reload/new request) — both providers only require it for the most
/// recent assistant turn, so it isn't persisted in the domain `Message`. `text` is what
/// the server sent (with `display:summarized` this is a summary), resent unchanged
/// (Anthropic); `id` is filled only by OpenAI Responses (Anthropic has `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingBlock {
    pub text: String,
    /// The signature (Anthropic `signature`) or encrypted reasoning
    /// (OpenAI `encrypted_content`) — resent unchanged.
    pub signature: String,
    /// The reasoning item's id (OpenAI `rs_…`); `None` for Anthropic.
    pub id: Option<String>,
}

/// A reference to reasoning from the stream: the reasoning item's id (OpenAI `rs_…`,
/// `None` for Anthropic) and the signature/encrypted content. Arrives as the chunk
/// [`ChatChunk::ThoughtsSignature`], accumulates over the turn, then attaches to the
/// assistant message as [`ThinkingBlock`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThinkingRef {
    pub id: Option<String>,
    pub signature: String,
}

/// An image passed to the model as part of a message's content (spec §9.10).
///
/// Provider-agnostic on purpose: every backend takes base64 plus a MIME type, and each
/// wraps them differently (`image_url` with a `data:` URI for Chat Completions and xAI,
/// an `image`/`source` block for Anthropic, `inline_data` for Gemini, `input_image` for
/// Responses). See docs/research/multimodal-images.md §2.2 for the four verified shapes.
///
/// `data` is an [`Arc<str>`] rather than a `String` because [`super::retry::RetryBackend`]
/// clones the whole [`ChatRequest`] per attempt: a megabyte of base64 copied on every
/// retry would be a real cost for no reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiImage {
    /// `image/png` or `image/jpeg` — normalized at attach time.
    pub mime: String,
    /// The payload, base64, without a `data:` prefix.
    pub data: std::sync::Arc<str>,
    /// A short text part emitted immediately **before** the image (`Image #1 — "a.png":`).
    /// Anthropic documents labelling images when several are present, and it is the only
    /// way a model can refer to one by the name the user sees in `/image list`. Built in
    /// the app layer, because it is prompt scaffold in the **profile** language (axis A)
    /// and the wire layer has no locale.
    pub label: Option<String>,
}

impl ApiImage {
    pub fn new(mime: impl Into<String>, data: &str, label: Option<String>) -> Self {
        Self {
            mime: mime.into(),
            data: std::sync::Arc::from(data),
            label,
        }
    }
}

/// A conversation message passed to the model.
#[derive(Debug, Clone)]
pub struct ApiMessage {
    pub role: ApiRole,
    pub content: String,
    /// Images carried by this message (spec §9.10). Empty for every message that has
    /// none, which is what keeps a text-only request byte-identical to what the app sent
    /// before the feature existed — see the wire builders.
    pub images: Vec<ApiImage>,
    /// The tool-call id (for the `Tool` role).
    pub tool_call_id: Option<String>,
    /// Tool calls (for the `Assistant` role that initiated a tool call).
    pub tool_calls: Vec<ApiToolCall>,
    /// A reasoning block (Anthropic extended thinking) for resending in the current
    /// agentic-loop turn. Set only on an assistant turn with `tool_use` (see
    /// [`ThinkingBlock`]); other backends ignore it. `None` by default.
    pub thinking: Option<ThinkingBlock>,
}

impl ApiMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            thinking: None,
            images: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            thinking: None,
            images: Vec::new(),
        }
    }

    /// An assistant turn with tool calls (content can be empty).
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ApiToolCall>) -> Self {
        Self {
            role: ApiRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls,
            thinking: None,
            images: Vec::new(),
        }
    }

    /// Attaches a thinking block (Anthropic/OpenAI Responses) to the message
    /// (builder-style).
    pub fn with_thinking(mut self, thinking: Option<ThinkingBlock>) -> Self {
        self.thinking = thinking;
        self
    }

    /// Attaches images to the message (builder-style). See [`ApiImage`].
    pub fn with_images(mut self, images: Vec<ApiImage>) -> Self {
        self.images = images;
        self
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            thinking: None,
            images: Vec::new(),
        }
    }
}

/// An OpenAI tool schema sent to the server (it returns valid
/// `tool_calls`). Tool implementations (`features/tools`) build it.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema of the parameters object.
    pub parameters: serde_json::Value,
}

/// A request for one generation turn.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    /// The system message (substituted first). See spec §6.2.
    pub system: Option<String>,
    /// The conversation (user/assistant/tool).
    pub messages: Vec<ApiMessage>,
    pub sampling: SamplingConfig,
    /// Schemas of the available tools (empty — no tool-calling). `tool_choice=auto`.
    pub tools: Vec<ToolSchema>,
    /// Continue the **trailing assistant message** in place instead of starting
    /// a new reply (`/continue`, spec §6.4). Only the OpenAI-compatible wire
    /// acts on it — llama.cpp continues such a tail by default and the explicit
    /// `continue_final_message` pair plus `enable_thinking:false` ride along
    /// for vLLM and thinking templates (research §2, §7.1); other wires ignore
    /// it, and [`ServerMode::supports_continuation`](crate::shared::config::ServerMode)
    /// keeps it from reaching them. `false` — a request byte-identical to
    /// before the field existed.
    pub continue_final: bool,
}

/// The reason generation finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    Cancelled,
    Error,
}

impl FinishReason {
    /// Maps the string `finish_reason` from the server response.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "tool_calls" => FinishReason::ToolCalls,
            _ => FinishReason::Stop,
        }
    }
}

/// The token counter from the server response (the `usage` field). The server sends it
/// as the stream's final chunk when `stream_options.include_usage=true` is requested
/// (see [`openai::wire`]). Fields may be zero if the server didn't return them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    /// Tokens in the prompt (the request's context size).
    pub prompt_tokens: u32,
    /// Tokens in the reply (generated by the model).
    pub completion_tokens: u32,
    /// Reasoning ("thoughts") tokens, already included in `completion_tokens`. Returned by
    /// reasoning providers (OpenAI Responses `output_tokens_details.reasoning_tokens`;
    /// OpenAI-compat/llama.cpp `completion_tokens_details.reasoning_tokens`). `0` —
    /// the provider doesn't separate them (Anthropic: "thoughts" are counted in `completion_tokens`).
    pub reasoning_tokens: u32,
}

/// A tool-call delta from the stream (accumulated by `index`). See spec §6.3.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolCallDelta {
    pub index: usize,
    /// The call's id (usually in the first delta).
    pub id: Option<String>,
    /// The function name (usually in the first delta).
    pub name: Option<String>,
    /// A delta of the arguments string (concatenated).
    pub arguments: String,
    /// The call's thought signature (Gemini 3 `thoughtSignature`). Only the Gemini client
    /// emits it (others give `None`); accumulates into [`ApiToolCall::thought_signature`].
    pub thought_signature: Option<String>,
}

/// An incremental fragment of the model's response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatChunk {
    /// A delta of the main text.
    Text(String),
    /// A delta of "thoughts" (reasoning).
    Thoughts(String),
    /// A reference to reasoning (Anthropic `signature_delta` / an OpenAI reasoning item).
    /// Needed to resend the thinking block on an assistant turn with a tool call
    /// (see [`ThinkingBlock`]). Backends without extended thinking don't emit it.
    ThoughtsSignature(ThinkingRef),
    /// A tool-call delta (the server parses `<tool_call>` itself).
    ToolCall(ToolCallDelta),
    /// The token counter (`usage`) — usually a separate chunk before finishing.
    Usage(TokenUsage),
    /// The turn failed **after** the stream was already open.
    ///
    /// This is the stream's error channel, and it exists because such a failure
    /// cannot be returned as `Err`: the caller already holds the stream. Every
    /// client yields it immediately before `Finished(`[`FinishReason::Error`]`)`,
    /// so a consumer that only watches `Finished` behaves exactly as it did
    /// before.
    ///
    /// Before it existed, an in-stream failure was a `tracing::warn!` and nothing
    /// more: a reply cut off mid-sentence was indistinguishable from a finished
    /// one — Anthropic's in-stream `error` event (its `529 overloaded_error`
    /// arrives this way), an OpenAI-shaped error object inside a `200` stream from
    /// llama.cpp, a Gemini error payload, a dropped connection. See
    /// docs/research/cloud-retry-backoff.md §1.2.
    ///
    /// `transient` says whether another attempt could succeed
    /// ([`stream_error_transient`](super::error::stream_error_transient)). Nothing
    /// reads it yet — the retry decorator (stage 2 of that research) is its
    /// consumer; it is carried now so the clients' classification lives in one
    /// place from the start.
    Error { message: String, transient: bool },
    /// A transient failure is being retried, and the next attempt starts in
    /// `delay`.
    ///
    /// Emitted by [`RetryBackend`](super::retry::RetryBackend) *before* it waits,
    /// so the UI can say what is happening while it happens rather than after —
    /// silence for up to [`RETRY_AFTER_CAP`](super::retry::RETRY_AFTER_CAP) is the
    /// door left open (docs/lessons.md §4). `attempt` is the one about to start and
    /// counts from 2; `max` is the total a turn gets.
    ///
    /// Never carries content, so a consumer may ignore it: the turn either
    /// continues normally afterwards or ends with [`ChatChunk::Error`].
    Retry {
        attempt: u32,
        max: u32,
        delay: std::time::Duration,
    },
    /// Generation finished.
    Finished(FinishReason),
}

impl ChatChunk {
    /// The pair of chunks that ends a failed stream: the reason, then the
    /// terminator.
    ///
    /// The **ordering is a contract**, which is why it lives here rather than
    /// being spelled out at each of the eight places a client gives up (four
    /// transport drops, four provider error payloads). A consumer that watches
    /// only [`Finished`](ChatChunk::Finished) must still see it — that is what
    /// keeps this addition backwards-compatible — and a client that yielded the
    /// error alone would leave the turn generating forever.
    pub fn failure(message: String, transient: bool) -> [ChatChunk; 2] {
        [
            ChatChunk::Error { message, transient },
            ChatChunk::Finished(FinishReason::Error),
        ]
    }
}

/// An accumulator of tool calls from streaming deltas (by `index`).
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: Vec<ApiToolCall>,
}

impl ToolCallAccumulator {
    pub fn push(&mut self, delta: ToolCallDelta) {
        while self.calls.len() <= delta.index {
            self.calls.push(ApiToolCall::default());
        }
        let call = &mut self.calls[delta.index];
        if let Some(id) = delta.id
            && !id.is_empty()
        {
            call.id = id;
        }
        if let Some(name) = delta.name
            && !name.is_empty()
        {
            call.name = name;
        }
        // The thought signature (Gemini) arrives together with the call — store it on it.
        if let Some(sig) = delta.thought_signature
            && !sig.is_empty()
        {
            call.thought_signature = Some(sig);
        }
        call.arguments.push_str(&delta.arguments);
    }

    /// Returns the assembled calls (dropping nameless "gaps").
    pub fn finish(self) -> Vec<ApiToolCall> {
        self.calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .collect()
    }
}

/// A stream of reply fragments.
pub type ChatStream = Pin<Box<dyn Stream<Item = ChatChunk> + Send>>;

/// The inference engine (chat). Implementations: an HTTP client to xinfer
/// ([`super::openai::OpenAiClient`]) and a mock for tests.
#[async_trait::async_trait]
pub trait EngineBackend: Send + Sync {
    /// A streaming single-turn request. Cancellation — via `cancel`.
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream>;

    /// The engine's own context window in tokens, when it can say — the budget
    /// history compression measures itself against (spec §6.7, sub-decision S1
    /// of docs/research/history-compression.md).
    ///
    /// "What window does this engine have" is engine knowledge, so it belongs on
    /// the engine contract (ADR 0004) rather than in the layer above: the
    /// orchestrator asks and stays provider-agnostic. The default is `None` —
    /// **"cannot say", never "unlimited"** — so a backend that has no way to
    /// answer leaves auto-compaction inactive rather than acting on a guess. Only
    /// [`super::openai::OpenAiClient`] overrides it (llama.cpp's `/props`); the
    /// cloud protocols have no such endpoint and model windows are free-form
    /// (§11), so there the answer comes from an explicit setting or not at all.
    ///
    /// Called rarely — once per applied engine, re-asked when readiness flips —
    /// so a network round trip here is not on any hot path.
    async fn context_budget(&self) -> Option<u32> {
        None
    }

    /// Whether this engine can accept images in a request (spec §9.10).
    ///
    /// Same shape and the same reasoning as [`context_budget`](Self::context_budget):
    /// capability is engine knowledge, so it is answered here rather than guessed by the
    /// layer above. The default is [`VisionSupport::Unknown`] — "cannot say", never
    /// "no" — because the honest answer for an arbitrary OpenAI-compatible server is that
    /// we do not know, and refusing an attach on that basis would be wrong for every
    /// vLLM/LM Studio user running a vision model.
    ///
    /// [`super::openai::OpenAiClient`] overrides it with llama.cpp's `/props`
    /// (`modalities.vision`), which is the same fetch `context_budget` already makes; the
    /// cloud backends answer [`VisionSupport::Supported`] statically, since every
    /// current-generation model on all four providers takes images and a legacy text-only
    /// model produces a clear provider error on send. A model-name allowlist is
    /// deliberately not used — it goes stale and then lies, the trap
    /// docs/research/grok-xai-provider.md recorded for reasoning detection.
    async fn vision(&self) -> VisionSupport {
        VisionSupport::Unknown
    }

    /// What the engine says it is running, when it can say — the name shown on
    /// the feed and recorded in a message's metadata snapshot (spec §11.3) for a
    /// server the user connected to without naming its model.
    ///
    /// The third question of the same shape as [`context_budget`](Self::context_budget)
    /// and [`vision`](Self::vision), and for the same reason: what model is
    /// loaded is engine knowledge, so it is answered here rather than guessed
    /// above. The default is `None` — **"cannot say", never a placeholder** — and
    /// a caller that gets it shows nothing, exactly as it did before this method
    /// existed.
    ///
    /// Only [`super::openai::OpenAiClient`] overrides it: the cloud backends are
    /// unreachable without a configured model name, so there is never a blank to
    /// fill. The answer is already display-shaped
    /// ([`gguf::display_id`](crate::shared::gguf::display_id)) — a bare
    /// `llama-server` answers with the whole `-m` path.
    ///
    /// Asked once per applied engine (and again when readiness flips), never per
    /// turn. See docs/research/external-model-name.md §4.
    async fn model_id(&self) -> Option<String> {
        None
    }
}

/// Whether an engine accepts image input. See [`EngineBackend::vision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VisionSupport {
    /// The engine cannot be asked (no `/props`, or it did not answer). Attaching is
    /// allowed with a neutral note: a send-time provider error is a better outcome than
    /// refusing a capability the server may well have.
    #[default]
    Unknown,
    Supported,
    /// The engine answered "no images" — the only case where an attach is refused, and
    /// the refusal names what would fix it (the `mmproj` setting, for a managed server).
    Unsupported,
}

/// What a text is being embedded *as*.
///
/// Some model families (e5 and relatives) expect the input to be marked with its
/// role, and score noticeably worse without it; others (bge-m3) expect bare text
/// and score *worse* with a marker. The marker itself is applied centrally by
/// [`crate::shared::embed_prefix::PrefixedEmbedder`] — a call site's only job is
/// to state, truthfully, which side of a comparison its text is on.
///
/// **There is deliberately no `Default`.** A silently defaulted role is the exact
/// failure this type exists to prevent: it is invisible, and on a compressed
/// model it costs up to 17% of the usable similarity range (see
/// docs/research/embedding-input-prefixes.md §2.3).
///
/// The rule for choosing, in one line: **ask what the vector will be compared
/// against.** Only a search query against a stored index is [`Query`]; anything
/// stored, and anything compared against something stored, is [`Passage`] — even
/// when it reads like a question. See the full per-site table in the research
/// doc §5.
///
/// [`Query`]: EmbedRole::Query
/// [`Passage`]: EmbedRole::Passage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbedRole {
    /// A search query, matched against stored passages. Asymmetric retrieval only.
    Query,
    /// Text that is stored as a vector, or is compared against stored vectors.
    ///
    /// This is also the role for every **symmetric** comparison (trait against
    /// trait, observation against observation): both sides must be embedded the
    /// same way, and the project's similarity gates are calibrated on
    /// passage-role text.
    Passage,
}

/// A source of embeddings for RAG. Per the M5 decision — a **dedicated** embedding server
/// (a separate process/port), so it's split off from [`EngineBackend`] (chat).
/// See docs/decisions/0002-embeddings-dedicated-server.md.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Returns embeddings for each input text (in the same order).
    ///
    /// Every text in one call shares `role`; a comparison between two roles
    /// therefore takes two calls (deliberate — batches in this codebase are
    /// homogeneous everywhere except web-search reranking).
    async fn embed(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>>;
}

/// A stub embedder: returns an error (the embedding server isn't configured). RAG is
/// unavailable in this mode, but the tool doesn't "crash" — the error goes to the model.
pub struct UnavailableEmbedder;

#[async_trait::async_trait]
impl Embedder for UnavailableEmbedder {
    async fn embed(&self, _texts: Vec<String>, _role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        anyhow::bail!("embedding server is not configured — RAG unavailable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(FinishReason::from_wire("stop"), FinishReason::Stop);
        assert_eq!(FinishReason::from_wire("length"), FinishReason::Length);
        assert_eq!(
            FinishReason::from_wire("tool_calls"),
            FinishReason::ToolCalls
        );
        assert_eq!(FinishReason::from_wire("weird"), FinishReason::Stop);
    }

    #[test]
    fn role_wire_strings() {
        assert_eq!(ApiRole::System.as_wire(), "system");
        assert_eq!(ApiRole::Tool.as_wire(), "tool");
    }

    #[test]
    fn accumulator_assembles_split_tool_call() {
        let mut acc = ToolCallAccumulator::default();
        acc.push(ToolCallDelta {
            thought_signature: None,
            index: 0,
            id: Some("call_1".into()),
            name: Some("note_save".into()),
            arguments: "{\"con".into(),
        });
        acc.push(ToolCallDelta {
            thought_signature: None,
            index: 0,
            id: None,
            name: None,
            arguments: "tent\":\"hi\"}".into(),
        });
        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "note_save");
        assert_eq!(calls[0].arguments, "{\"content\":\"hi\"}");
    }

    #[test]
    fn accumulator_carries_thought_signature() {
        // Gemini attaches the signature to the call delta — it lands on the assembled ApiToolCall.
        let mut acc = ToolCallAccumulator::default();
        acc.push(ToolCallDelta {
            index: 0,
            id: Some("calc-0".into()),
            name: Some("calc".into()),
            arguments: "{}".into(),
            thought_signature: Some("SIG".into()),
        });
        let calls = acc.finish();
        assert_eq!(calls[0].thought_signature.as_deref(), Some("SIG"));
    }

    #[test]
    fn accumulator_handles_two_parallel_calls() {
        let mut acc = ToolCallAccumulator::default();
        acc.push(ToolCallDelta {
            thought_signature: None,
            index: 0,
            id: Some("a".into()),
            name: Some("f".into()),
            arguments: "{}".into(),
        });
        acc.push(ToolCallDelta {
            thought_signature: None,
            index: 1,
            id: Some("b".into()),
            name: Some("g".into()),
            arguments: "{}".into(),
        });
        assert_eq!(acc.finish().len(), 2);
    }
}
