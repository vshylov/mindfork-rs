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

/// A conversation message passed to the model.
#[derive(Debug, Clone)]
pub struct ApiMessage {
    pub role: ApiRole,
    pub content: String,
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
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            thinking: None,
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
        }
    }

    /// Attaches a thinking block (Anthropic/OpenAI Responses) to the message
    /// (builder-style).
    pub fn with_thinking(mut self, thinking: Option<ThinkingBlock>) -> Self {
        self.thinking = thinking;
        self
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            thinking: None,
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
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// The system message (substituted first). See spec §6.2.
    pub system: Option<String>,
    /// The conversation (user/assistant/tool).
    pub messages: Vec<ApiMessage>,
    pub sampling: SamplingConfig,
    /// Schemas of the available tools (empty — no tool-calling). `tool_choice=auto`.
    pub tools: Vec<ToolSchema>,
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
    /// Generation finished.
    Finished(FinishReason),
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
}

/// A source of embeddings for RAG. Per the M5 decision — a **dedicated** embedding server
/// (a separate process/port), so it's split off from [`EngineBackend`] (chat).
/// See docs/decisions/0002-embeddings-dedicated-server.md.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Returns embeddings for each input text (in the same order).
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
}

/// A stub embedder: returns an error (the embedding server isn't configured). RAG is
/// unavailable in this mode, but the tool doesn't "crash" — the error goes to the model.
pub struct UnavailableEmbedder;

#[async_trait::async_trait]
impl Embedder for UnavailableEmbedder {
    async fn embed(&self, _texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        anyhow::bail!("embedding-сервер не настроен — RAG недоступен")
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
