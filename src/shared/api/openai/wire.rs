//! Serde types for the xinfer HTTP protocol (`/v1/chat/completions`, `/v1/embeddings`)
//! and building the request body. Matches docs/xinfer-contract.md §3, §6 exactly.
//!
//! Invariant: the `stop` field is NOT sent (anti-self-cutoff on EOS text —
//! see spec §7, docs/xinfer-contract.md §5).

use serde::{Deserialize, Serialize};

use crate::entities::sampling::ReasoningEffort;
use crate::shared::api::contract::{ApiMessage, ApiRole, ChatRequest};

// The only consumer of this client is the local/external llama.cpp `llama-server`
// (managed/external). The clouds moved to their own protocols: OpenAI → Responses
// ([`ResponsesClient`](super::ResponsesClient)), Gemini → native
// [`GeminiClient`](crate::shared::api::gemini::GeminiClient), Claude → Anthropic
// Messages. So there's no longer a sampling dialect/filter — send everything set
// (llama.cpp ignores unknown fields). See ADR 0004.

// ---------- chat request ----------

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    /// The model name. Mandatory for the cloud; for `llama-server` it's ignored (it takes the
    /// loaded model), so it's sent only when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<WireMessage>,
    pub stream: bool,
    /// Stream options: ask the server to send a final `usage` with the token counter
    /// (`include_usage`). Sent only while streaming (see [`StreamOptions`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynatemp_range: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynatemp_exponent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    // llama.cpp `llama-server` extensions (see [`SamplingConfig`]); a strict
    // third-party OpenAI server ignores or rejects them — so they're sent only
    // when the user sets them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_n_sigma: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_target: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_decay: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_last_n: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_multiplier: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_base: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_allowed_length: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_penalty_last_n: Option<i64>,
    /// DRY breakers (an array of strings); only a non-empty list is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_sequence_breakers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtc_probability: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtc_threshold: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirostat: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirostat_tau: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirostat_eta: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// The sampler order (an array of names); only a non-empty list is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samplers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
    /// The "thoughts" budget (llama.cpp): `0` disables thinking. See [`SamplingConfig`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_budget: Option<i64>,
    /// Extra variables for the Jinja chat template (llama.cpp `chat_template_kwargs`).
    /// Used for `{"enable_thinking": false}` — different templates disable
    /// "thoughts" differently (built-in formats read `reasoning_budget`, many
    /// Jinja templates — `enable_thinking`), so both signals are sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<serde_json::Value>,
    /// Continue the trailing assistant message instead of opening a new one
    /// (`/continue`, spec §6.4). The explicit opt-in vLLM requires; llama.cpp
    /// continues by default and ignores the pair on builds that predate it
    /// (measured on b10659 — research §7.1). Sent only with
    /// [`ChatRequest::continue_final`], together with `add_generation_prompt:
    /// false` — the two are one setting on every server that knows them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_final_message: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_generation_prompt: Option<bool>,
    /// Tool schemas (absent if tool-calling isn't used).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<&'static str>,
}

/// OpenAI stream options (`stream_options`). `include_usage=true` makes the server
/// send a final chunk with a `usage` block (the token counter) — otherwise it isn't
/// in the stream. llama.cpp `llama-server` supports this.
#[derive(Debug, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Serialize)]
pub struct WireMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<WireContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
}

/// The `content` of a wire message: a bare string, or an array of content parts.
///
/// A message with no images serializes as a **plain string**, byte-identical to what this
/// client sent before images existed. That is deliberate and load-bearing rather than
/// cosmetic: llama.cpp reuses its prefix cache on a matching rendered prompt, and Gemma's
/// chat template handles the string and the parts array in two different branches — so a
/// blanket switch to parts would re-prefill every existing conversation and change the
/// prompt of every text-only turn on every provider. Parts appear only when there is an
/// image to carry. Pinned by `a_text_only_request_is_unchanged_by_the_image_support`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum WireContent {
    Text(String),
    Parts(Vec<serde_json::Value>),
}

/// Builds a message's `content`: images (each preceded by its label, when it has one) and
/// the text, in a role-dependent order.
///
/// A **user** turn puts its images first: Anthropic documents that ordering as the
/// better-performing one and no other provider cares, so keeping one order across all four
/// backends means a prompt behaves the same wherever it is sent. A **tool** result inverts
/// it — the text is the tool's actual answer and the image only illustrates it, and
/// Anthropic's images-first advice is about a user's request rather than a tool's output
/// (docs/research/mcp-tool-images.md §2.2).
///
/// A `role:"tool"` message takes the very same content-parts array a user message does —
/// verified live on both llama.cpp and grok-4.5, the two servers this client talks to.
fn wire_content(m: &ApiMessage) -> WireContent {
    if m.images.is_empty() {
        // For an assistant turn with tool_calls the content can legitimately be empty.
        return WireContent::Text(m.content.clone());
    }
    let text_part = |text: &str| serde_json::json!({ "type": "text", "text": text });
    // A tool result leads with its text; every other role leads with its images.
    let text_leads = m.role == ApiRole::Tool;
    let mut parts = Vec::with_capacity(m.images.len() * 2 + 1);
    if text_leads && !m.content.is_empty() {
        parts.push(text_part(&m.content));
    }
    for image in &m.images {
        if let Some(label) = &image.label {
            parts.push(text_part(label));
        }
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{};base64,{}", image.mime, image.data) },
        }));
    }
    if !text_leads && !m.content.is_empty() {
        parts.push(text_part(&m.content));
    }
    WireContent::Parts(parts)
}

/// The OpenAI wrapper for a tool schema (`{type:"function", function:{...}}`).
#[derive(Debug, Serialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunction,
}

#[derive(Debug, Serialize)]
pub struct WireFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call in a history assistant message.
#[derive(Debug, Serialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunctionCall,
}

#[derive(Debug, Serialize)]
pub struct WireFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Builds the chat request body from the domain [`ChatRequest`]. `model` is substituted into
/// the `model` field (for an external proxy if desired; for llama-server `None` works —
/// the server takes the loaded model). Everything set in sampling is sent (llama.cpp
/// ignores what it doesn't know). `omit_effort_none` — drop a `reasoning_effort` of
/// `"none"` rather than send it (xAI rejects the value; see
/// [`OpenAiClient::with_effort_none_omitted`](super::OpenAiClient::with_effort_none_omitted)).
pub fn build_chat_request(
    req: &ChatRequest,
    stream: bool,
    model: Option<&str>,
    omit_effort_none: bool,
) -> ChatCompletionRequest {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if let Some(system) = &req.system {
        messages.push(WireMessage {
            role: "system",
            content: Some(WireContent::Text(system.clone())),
            tool_call_id: None,
            tool_calls: None,
        });
    }
    for m in &req.messages {
        let tool_calls = if m.tool_calls.is_empty() {
            None
        } else {
            Some(
                m.tool_calls
                    .iter()
                    .map(|tc| WireToolCall {
                        id: tc.id.clone(),
                        kind: "function",
                        function: WireFunctionCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                    })
                    .collect(),
            )
        };
        messages.push(WireMessage {
            role: m.role.as_wire(),
            // For an assistant with tool_calls, content can be empty.
            content: Some(wire_content(m)),
            tool_call_id: m.tool_call_id.clone(),
            tool_calls,
        });
    }

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| WireTool {
                    kind: "function",
                    function: WireFunction {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    },
                })
                .collect(),
        )
    };
    let tool_choice = tools.as_ref().map(|_| "auto");

    let s = &req.sampling;
    // The request to disable "thoughts" (reasoning_budget=0) is also sent via
    // chat_template_kwargs.enable_thinking=false: llama.cpp's built-in formats read
    // reasoning_budget, while models' Jinja templates read enable_thinking; send both.
    // A continuation request sends the same kwarg: resuming a visible reply must
    // not re-open reasoning, and on #21889-era llama.cpp builds it is what lifts
    // the "prefill is incompatible with enable_thinking" rejection (research §7.1).
    let chat_template_kwargs = (s.reasoning_budget == Some(0) || req.continue_final)
        .then(|| serde_json::json!({ "enable_thinking": false }));
    // List fields (DRY breakers, sampler order): an empty list isn't sent —
    // otherwise the server would interpret it as "no breakers"/"disable all samplers".
    let non_empty = |v: &Option<Vec<String>>| v.clone().filter(|x| !x.is_empty());
    ChatCompletionRequest {
        model: model.map(str::to_string),
        messages,
        stream,
        // The token counter is only needed during a streaming generation turn.
        stream_options: stream.then_some(StreamOptions {
            include_usage: true,
        }),
        temperature: s.temperature,
        dynatemp_range: s.dynatemp_range,
        dynatemp_exponent: s.dynatemp_exponent,
        max_tokens: s.max_tokens,
        top_k: s.top_k,
        top_p: s.top_p,
        min_p: s.min_p,
        top_n_sigma: s.top_n_sigma,
        typical_p: s.typical_p,
        adaptive_target: s.adaptive_target,
        adaptive_decay: s.adaptive_decay,
        frequency_penalty: s.frequency_penalty,
        presence_penalty: s.presence_penalty,
        repeat_penalty: s.repeat_penalty,
        repeat_last_n: s.repeat_last_n,
        dry_multiplier: s.dry_multiplier,
        dry_base: s.dry_base,
        dry_allowed_length: s.dry_allowed_length,
        dry_penalty_last_n: s.dry_penalty_last_n,
        dry_sequence_breakers: non_empty(&s.dry_sequence_breakers),
        xtc_probability: s.xtc_probability,
        xtc_threshold: s.xtc_threshold,
        mirostat: s.mirostat,
        mirostat_tau: s.mirostat_tau,
        mirostat_eta: s.mirostat_eta,
        seed: s.seed,
        samplers: non_empty(&s.samplers),
        thinking: s.thinking,
        reasoning_effort: s
            .reasoning_effort
            .filter(|r| !(omit_effort_none && *r == ReasoningEffort::None))
            .map(|r| r.as_wire()),
        reasoning_budget: s.reasoning_budget,
        chat_template_kwargs,
        continue_final_message: req.continue_final.then_some(true),
        add_generation_prompt: req.continue_final.then_some(false),
        tools,
        tool_choice,
    }
}

// ---------- streaming response ----------

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub choices: Vec<ChatChoiceChunk>,
    /// The token counter: sent as the final chunk when
    /// `stream_options.include_usage=true` (such a chunk's `choices` is usually empty).
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// An error object delivered **inside** an already-open `200` SSE stream, instead
/// of a chunk.
///
/// llama.cpp does this (ggml-org/llama.cpp#14566), and OpenAI-compatible proxies
/// inherit the shape: the payload is the ordinary `{"error":{…}}` envelope, so it
/// fails to deserialize as a [`ChatCompletionChunk`] and used to be dropped with a
/// log line — leaving the turn to end as an ordinary `Stop`.
#[derive(Debug, Deserialize)]
struct StreamErrorEnvelope {
    error: StreamErrorBody,
}

#[derive(Debug, Default, Deserialize)]
struct StreamErrorBody {
    #[serde(default)]
    message: String,
    /// The provider's error name (`server_error`, `exceed_context_size_error`, …).
    #[serde(default, rename = "type")]
    name: String,
    /// Either an HTTP status (llama.cpp sends a number) or a string code
    /// (OpenAI sends `"context_length_exceeded"`).
    #[serde(default)]
    code: Option<serde_json::Value>,
}

/// An in-stream error, parsed: what to show and whether another attempt could
/// succeed.
pub struct StreamError {
    pub message: String,
    pub name: String,
    pub transient: bool,
}

/// Reads an SSE `data:` payload as an error envelope.
///
/// `None` when it is not one — which is the common case, since this is only tried
/// after a chunk failed to parse. An envelope carrying neither a name nor a
/// message is also `None`: it would produce a note that says nothing, which is
/// the defect this path exists to fix.
pub fn parse_stream_error(data: &str) -> Option<StreamError> {
    let env: StreamErrorEnvelope = serde_json::from_str(data).ok()?;
    let body = env.error;
    if body.name.trim().is_empty() && body.message.trim().is_empty() {
        return None;
    }
    let status = body.code.as_ref().and_then(|c| c.as_u64()).and_then(|c| {
        // A status is the only numeric code we can interpret; anything else
        // (a millisecond field, an id) must not be read as one.
        u16::try_from(c).ok().filter(|s| (100..=599).contains(s))
    });
    let transient = crate::shared::api::error::stream_error_transient(&body.name, status);
    Some(StreamError {
        message: crate::shared::api::error::stream_error_text(&body.name, &body.message),
        name: body.name,
        transient,
    })
}

/// The `usage` block of the server response (the token counter). `completion_tokens_details.
/// reasoning_tokens` is returned by OpenAI-compat/llama.cpp servers with a reasoning model
/// (included in `completion_tokens`); absent → `0`.
#[derive(Debug, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub completion_tokens_details: CompletionTokensDetails,
}

/// Token breakdown of the Chat Completions response (only reasoning tokens matter).
#[derive(Debug, Default, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub struct ChatChoiceChunk {
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct DeltaToolCall {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DeltaFunction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

// ---------- embeddings ----------

#[derive(Debug, Serialize)]
pub struct EmbeddingRequest {
    /// The embedding model's name. Mandatory for the cloud (OpenAI/Gemini); for
    /// `llama-server` it's ignored — sent only when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub input: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct EmbeddingResponse {
    #[serde(default)]
    pub data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
pub struct EmbeddingData {
    pub embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::contract::ApiMessage;

    #[test]
    fn omits_stop_and_none_fields() {
        let req = ChatRequest {
            continue_final: false,
            system: Some("sys".into()),
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let body = build_chat_request(&req, true, None, false);
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("stop").is_none(), "stop must never be sent");
        assert!(json.get("temperature").is_none());
        assert_eq!(json["stream"], true);
        // Streaming → ask for usage to be sent (the token counter).
        assert_eq!(json["stream_options"]["include_usage"], true);
        // system must go as the first message
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][1]["role"], "user");
        assert_eq!(json["messages"][1]["content"], "hi");
    }

    fn image(mime: &str, data: &str, label: Option<&str>) -> crate::shared::api::ApiImage {
        crate::shared::api::ApiImage {
            mime: mime.to_string(),
            data: std::sync::Arc::from(data),
            label: label.map(str::to_string),
        }
    }

    /// The guarantee the whole design rests on: adding image support must not change a
    /// single byte of a request that carries no images. llama.cpp reuses its prefix cache
    /// on a matching rendered prompt, and Gemma's chat template routes a string and a
    /// parts array through different branches — so a blanket switch to parts would
    /// re-prefill every existing conversation and silently change every text-only turn on
    /// every provider.
    #[test]
    fn a_text_only_request_is_unchanged_by_the_image_support() {
        let req = ChatRequest {
            continue_final: false,
            system: Some("be brief".into()),
            messages: vec![
                ApiMessage::user("hi"),
                ApiMessage::assistant("hello"),
                ApiMessage::tool("call-1", "42"),
            ],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        // Every content is a bare JSON string, exactly as before content parts existed.
        for i in 0..4 {
            assert!(
                json["messages"][i]["content"].is_string(),
                "message {i} must serialize its content as a string, got {:?}",
                json["messages"][i]["content"]
            );
        }
        assert_eq!(json["messages"][0]["content"], "be brief");
        assert_eq!(json["messages"][1]["content"], "hi");
        assert_eq!(json["messages"][3]["content"], "42");
    }

    #[test]
    fn images_become_content_parts_ahead_of_the_text() {
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("what is this?").with_images(vec![image(
                "image/png",
                "QUJD",
                Some("Image #1 — \"a.png\":"),
            )])],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        let parts = json["messages"][0]["content"].as_array().unwrap();
        // Label, then image, then the user's own text — the order Anthropic documents
        // and the one we keep across all four backends.
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "Image #1 — \"a.png\":");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(
            parts[1]["image_url"]["url"], "data:image/png;base64,QUJD",
            "the payload must be a data URI, which is what llama.cpp and xAI both accept"
        );
        assert_eq!(parts[2]["type"], "text");
        assert_eq!(parts[2]["text"], "what is this?");
    }

    #[test]
    fn several_images_keep_their_order_and_a_labelless_one_emits_no_text_part() {
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("").with_images(vec![
                image("image/png", "AAA", None),
                image("image/jpeg", "BBB", None),
            ])],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        let parts = json["messages"][0]["content"].as_array().unwrap();
        // Two images, no labels, and no empty trailing text part: an image-only message
        // is a legitimate request ("look at this"), and a blank text part is noise the
        // template would still render.
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["image_url"]["url"], "data:image/png;base64,AAA");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/jpeg;base64,BBB");
    }

    /// The same guarantee, on the tool side: a tool result that carries no image must
    /// still serialize as a bare string — not a one-element parts array. This is the
    /// shape every stored conversation replays, so a change here would re-prefill the
    /// llama.cpp prefix cache for every chat that ever called a tool.
    #[test]
    fn a_tool_result_without_images_is_unchanged() {
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::tool("call-1", "42")],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        assert_eq!(json["messages"][0]["role"], "tool");
        assert!(
            json["messages"][0]["content"].is_string(),
            "a tool result with no images must stay a string, got {:?}",
            json["messages"][0]["content"]
        );
        assert_eq!(json["messages"][0]["content"], "42");
    }

    /// A tool result that produced a screenshot (an MCP image result, spec §9.10):
    /// the same content-parts array a user message uses, but with the tool's own text
    /// **first** — it is the answer, and the image illustrates it.
    #[test]
    fn a_tool_result_image_follows_the_result_text() {
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::tool("call-1", "screenshot taken").with_images(vec![image(
                    "image/png",
                    "QUJD",
                    Some("Image #1 — \"shot.png\":"),
                )]),
            ],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        assert_eq!(json["messages"][0]["role"], "tool");
        assert_eq!(json["messages"][0]["tool_call_id"], "call-1");
        let parts = json["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        // Result text, then the image's label, then the image itself.
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "screenshot taken");
        assert_eq!(parts[1]["type"], "text");
        assert_eq!(parts[1]["text"], "Image #1 — \"shot.png\":");
        assert_eq!(parts[2]["type"], "image_url");
        assert_eq!(
            parts[2]["image_url"]["url"], "data:image/png;base64,QUJD",
            "a tool result carries the payload as the same data URI a user image does"
        );
    }

    /// A tool that returns *only* an image (no prose) must not grow an empty text part —
    /// the mirror of the user-side rule.
    #[test]
    fn an_image_only_tool_result_carries_no_empty_text_part() {
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::tool("call-1", "").with_images(vec![image(
                "image/jpeg",
                "QQ==",
                None,
            )])],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        let parts = json["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["image_url"]["url"], "data:image/jpeg;base64,QQ==");
    }

    #[test]
    fn maps_supported_sampling_fields() {
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                temperature: Some(0.8),
                dynatemp_range: Some(0.4),
                dynatemp_exponent: Some(1.0),
                top_k: Some(40),
                top_p: Some(0.95),
                min_p: Some(0.03),
                top_n_sigma: Some(1.5),
                adaptive_target: Some(0.1),
                adaptive_decay: Some(0.9),
                frequency_penalty: Some(0.1),
                presence_penalty: Some(0.2),
                repeat_penalty: Some(1.0),
                dry_multiplier: Some(0.8),
                dry_base: Some(1.75),
                dry_allowed_length: Some(2),
                xtc_probability: Some(0.3),
                xtc_threshold: Some(0.15),
                seed: Some(-1),
                max_tokens: Some(256),
                thinking: Some(true),
                reasoning_effort: Some(ReasoningEffort::High),
                reasoning_budget: Some(0),
                ..Default::default()
            },
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        // f32→f64 widening makes exact comparison unreliable — compare approximately.
        let approx = |v: &serde_json::Value, want: f64| (v.as_f64().unwrap() - want).abs() < 1e-6;
        assert!(approx(&json["temperature"], 0.8));
        assert!(approx(&json["dynatemp_range"], 0.4));
        assert!(approx(&json["dynatemp_exponent"], 1.0));
        assert_eq!(json["top_k"], 40);
        assert!(approx(&json["top_p"], 0.95));
        assert!(approx(&json["min_p"], 0.03));
        assert!(approx(&json["top_n_sigma"], 1.5));
        assert!(approx(&json["adaptive_target"], 0.1));
        assert!(approx(&json["adaptive_decay"], 0.9));
        assert!(approx(&json["frequency_penalty"], 0.1));
        assert!(approx(&json["presence_penalty"], 0.2));
        assert!(approx(&json["repeat_penalty"], 1.0));
        assert!(approx(&json["dry_multiplier"], 0.8));
        assert!(approx(&json["dry_base"], 1.75));
        assert_eq!(json["dry_allowed_length"], 2);
        assert!(approx(&json["xtc_probability"], 0.3));
        assert!(approx(&json["xtc_threshold"], 0.15));
        assert_eq!(json["seed"], -1);
        assert_eq!(json["max_tokens"], 256);
        assert_eq!(json["thinking"], true);
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["reasoning_budget"], 0);
        // reasoning_budget=0 is also signaled for Jinja templates.
        assert_eq!(json["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(json["stream"], false);
        // Without streaming, usage isn't requested.
        assert!(json.get("stream_options").is_none());
        // Unset extensions aren't serialized.
        assert!(json.get("typical_p").is_none());
        assert!(json.get("mirostat").is_none());
    }

    #[test]
    fn list_fields_sent_as_arrays_and_empty_omitted() {
        // Non-empty lists → JSON arrays.
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                dry_sequence_breakers: Some(vec!["\n".into(), ":".into()]),
                samplers: Some(vec!["penalties".into(), "temperature".into()]),
                ..Default::default()
            },
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        assert_eq!(json["dry_sequence_breakers"][0], "\n");
        assert_eq!(json["dry_sequence_breakers"][1], ":");
        assert_eq!(json["samplers"][0], "penalties");
        assert_eq!(json["samplers"][1], "temperature");

        // Empty lists are NOT sent (otherwise the server would take them as "disable everything").
        let req_empty = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                dry_sequence_breakers: Some(vec![]),
                samplers: Some(vec![]),
                ..Default::default()
            },
            tools: vec![],
        };
        let json_empty =
            serde_json::to_value(build_chat_request(&req_empty, false, None, false)).unwrap();
        assert!(json_empty.get("dry_sequence_breakers").is_none());
        assert!(json_empty.get("samplers").is_none());
    }

    #[test]
    fn model_is_sent_when_some_and_omitted_when_none() {
        // For an external proxy, the model name is set; for llama-server (None) — it isn't.
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let with_model =
            serde_json::to_value(build_chat_request(&req, true, Some("some-model"), false))
                .unwrap();
        assert_eq!(with_model["model"], "some-model");
        let no_model = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert!(no_model.get("model").is_none());
    }

    /// `reasoning_effort: "none"` is how the orchestrator says "don't think" on its
    /// auxiliary turns (title, compaction, impersonation) — llama.cpp obeys it, xAI
    /// answers `400`. With the flag on, the field is omitted rather than sent, and
    /// **only** that value is affected: an explicit `low`/`high` still goes out.
    #[test]
    fn effort_none_is_omitted_only_when_asked() {
        let req = |e: ReasoningEffort| ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                reasoning_effort: Some(e),
                ..Default::default()
            },
            tools: vec![],
        };
        let json = |e: ReasoningEffort, omit: bool| {
            serde_json::to_value(build_chat_request(&req(e), true, None, omit)).unwrap()
        };
        // Default (llama.cpp): "none" is a meaningful value, keep sending it.
        assert_eq!(
            json(ReasoningEffort::None, false)["reasoning_effort"],
            "none"
        );
        // Grok: dropped entirely — the model reasons at its default depth.
        assert!(
            json(ReasoningEffort::None, true)
                .get("reasoning_effort")
                .is_none()
        );
        // Every other level is untouched by the flag.
        assert_eq!(json(ReasoningEffort::Low, true)["reasoning_effort"], "low");
        assert_eq!(
            json(ReasoningEffort::XHigh, true)["reasoning_effort"],
            "xhigh"
        );
    }

    #[test]
    fn parses_streaming_chunk() {
        let raw = r#"{"choices":[{"delta":{"content":"hello","reasoning_content":"hmm"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        let c = &chunk.choices[0];
        assert_eq!(c.delta.content.as_deref(), Some("hello"));
        assert_eq!(c.delta.reasoning_content.as_deref(), Some("hmm"));
        assert!(c.finish_reason.is_none());
    }

    #[test]
    fn parses_usage_chunk() {
        // The final include_usage chunk: choices is empty, usage is present.
        let raw = r#"{"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":7,"total_tokens":49}}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert!(chunk.choices.is_empty());
        let u = chunk.usage.unwrap();
        assert_eq!(u.prompt_tokens, 42);
        assert_eq!(u.completion_tokens, 7);
    }

    #[test]
    fn chunk_without_usage_is_none() {
        let raw = r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn parses_finish_chunk() {
        let raw = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
    }

    #[test]
    fn parses_tool_call_delta() {
        let raw = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"note_save","arguments":"{\"x\":1}"}}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        let tc = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].index, 0);
        assert_eq!(tc[0].id.as_deref(), Some("c1"));
        let f = tc[0].function.as_ref().unwrap();
        assert_eq!(f.name.as_deref(), Some("note_save"));
        assert_eq!(f.arguments.as_deref(), Some("{\"x\":1}"));
    }

    #[test]
    fn builds_tools_and_tool_choice() {
        use crate::shared::api::contract::ToolSchema;
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig::default(),
            tools: vec![ToolSchema {
                name: "note_save".into(),
                description: "Сохранить заметку".into(),
                parameters: serde_json::json!({"type":"object"}),
            }],
        };
        let json = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert_eq!(json["tool_choice"], "auto");
        assert_eq!(json["tools"][0]["type"], "function");
        assert_eq!(json["tools"][0]["function"]["name"], "note_save");
    }

    #[test]
    fn serializes_assistant_tool_calls_in_history() {
        use crate::shared::api::contract::ApiToolCall;
        let req = ChatRequest {
            continue_final: false,
            system: None,
            messages: vec![
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        thought_signature: None,
                        id: "c1".into(),
                        name: "f".into(),
                        arguments: "{}".into(),
                    }],
                ),
                ApiMessage::tool("c1", "result"),
            ],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert_eq!(json["messages"][0]["tool_calls"][0]["id"], "c1");
        assert_eq!(
            json["messages"][0]["tool_calls"][0]["function"]["name"],
            "f"
        );
        assert_eq!(json["messages"][1]["role"], "tool");
        assert_eq!(json["messages"][1]["tool_call_id"], "c1");
        // A request without tools must not contain tool_choice.
        assert!(json.get("tool_choice").is_none());
    }
}

/// Error objects delivered inside an already-open `200` stream
/// (ggml-org/llama.cpp#14566) — see [`parse_stream_error`].
#[cfg(test)]
mod stream_error_tests {
    use super::*;

    #[test]
    fn a_llama_cpp_server_error_is_transient() {
        // llama.cpp puts the HTTP status in `code` as a number.
        let data = r#"{"error":{"code":500,"message":"failed to decode","type":"server_error"}}"#;
        let e = parse_stream_error(data).expect("an error envelope must be recognized");
        assert!(e.transient);
        assert!(e.message.contains("failed to decode"));
        assert_eq!(e.name, "server_error");
    }

    /// The trap: a context overflow also arrives as an `*_error` type, and retrying
    /// it would burn attempts on a request that can never fit.
    #[test]
    fn a_context_overflow_is_not_transient() {
        let data = r#"{"error":{"code":400,"message":"the request exceeds the available context size","type":"exceed_context_size_error"}}"#;
        let e = parse_stream_error(data).unwrap();
        assert!(!e.transient);
        // And the text still reaches the overflow classifier, which picks the advice.
        assert!(crate::features::compaction::is_context_overflow(&e.message));
    }

    #[test]
    fn a_string_code_does_not_read_as_a_status() {
        let data = r#"{"error":{"code":"context_length_exceeded","message":"too long","type":"invalid_request_error"}}"#;
        let e = parse_stream_error(data).unwrap();
        assert!(!e.transient, "a 4xx-class name must not be retried");
    }

    #[test]
    fn a_rate_limit_name_is_transient_without_any_code() {
        let data = r#"{"error":{"message":"slow down","type":"rate_limit_exceeded"}}"#;
        assert!(parse_stream_error(data).unwrap().transient);
    }

    /// Everything that is not an error envelope must stay `None`, or an ordinary
    /// parse hiccup would end the turn.
    #[test]
    fn non_errors_are_not_mistaken_for_errors() {
        for data in [
            r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
            r#"{"usage":{"prompt_tokens":1,"completion_tokens":2}}"#,
            "not json at all",
            // An envelope that names nothing would produce a note saying nothing.
            r#"{"error":{}}"#,
            r#"{"error":{"message":"   ","type":""}}"#,
        ] {
            assert!(parse_stream_error(data).is_none(), "{data}");
        }
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::*;
    use crate::shared::api::contract::ApiMessage;

    /// `/continue` (spec §6.4): the flag adds exactly the three continuation
    /// fields — the explicit llama.cpp/vLLM pair and the thinking suppression
    /// (research §2, §7.1) — and without it the body is byte-identical to
    /// before the feature existed, the images-feature discipline.
    #[test]
    fn continuation_adds_its_fields_and_absence_changes_nothing() {
        let mut req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user("q"), ApiMessage::assistant("part")],
            sampling: Default::default(),
            tools: vec![],
            continue_final: false,
        };
        let off = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert!(off.get("continue_final_message").is_none());
        assert!(off.get("add_generation_prompt").is_none());
        assert!(off.get("chat_template_kwargs").is_none());

        req.continue_final = true;
        let on = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert_eq!(on["continue_final_message"], true);
        assert_eq!(on["add_generation_prompt"], false);
        assert_eq!(on["chat_template_kwargs"]["enable_thinking"], false);

        // ...and nothing else moves with the flag.
        let (mut a, mut b) = (off, on);
        for k in [
            "continue_final_message",
            "add_generation_prompt",
            "chat_template_kwargs",
        ] {
            a.as_object_mut().unwrap().remove(k);
            b.as_object_mut().unwrap().remove(k);
        }
        assert_eq!(a, b, "the flag must touch nothing but its three fields");
    }
}
