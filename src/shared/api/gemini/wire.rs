//! Serde types for the native Google Gemini protocol (`generateContent`/
//! `streamGenerateContent`) and translating the domain [`ChatRequest`] into its format.
//! Differences from OpenAI Chat Completions (see ADR 0004, docs/research/gemini-native-client.md):
//! - the system message — top-level `systemInstruction:{parts:[{text}]}`;
//! - history — `contents:[{role:"user"|"model", parts:[…]}]` (there are no
//!   `system`/`tool` roles: system → top-level, a tool result → a `functionResponse` part in
//!   `role:"user"`); adjacent messages of the same role are merged;
//! - a tool call — a `{functionCall:{name, args}}` part (`args` is an OBJECT, not a string;
//!   a call has no `id`, `functionCall`↔`functionResponse` matching is positional);
//! - the token limit — `generationConfig.maxOutputTokens` (includes thought tokens!);
//! - reasoning — `generationConfig.thinkingConfig` (`thinkingLevel` for Gemini 3.x /
//!   `thinkingBudget` for 2.5 + `includeThoughts` for a visible "thoughts" summary).
//!
//! Thought signatures (`thoughtSignature`) — a neighbor of the `functionCall` part; resent
//! on a history replay (otherwise Gemini 3 gives `400`), from [`ApiToolCall::thought_signature`](crate::shared::api::contract::ApiToolCall).
//! Event-based SSE (`?alt=sse`): `data: {…}` lines with a partial `GenerateContentResponse`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{ApiImage, ApiMessage, ApiRole, ChatRequest};

// ---------- request ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenRequest {
    /// The system message (top-level, not in `contents`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Value>,
    pub contents: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<Value>,
}

/// Builds the `generateContent` request body from the domain [`ChatRequest`]. `model` is only
/// needed to pick the thinking-config format (`thinkingLevel` for Gemini 3.x vs
/// `thinkingBudget` for 2.5) — it isn't written into the body (the model is in the client's URL).
pub fn build_request(req: &ChatRequest, model: &str) -> GenRequest {
    let system_instruction = req
        .system
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| json!({ "parts": [{ "text": s }] }));

    let tools = if req.tools.is_empty() {
        None
    } else {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": sanitize_schema(&t.parameters),
                })
            })
            .collect();
        Some(vec![json!({ "functionDeclarations": decls })])
    };

    GenRequest {
        system_instruction,
        contents: build_contents(req),
        tools,
        generation_config: generation_config(req, model),
    }
}

/// Assembles `generationConfig` from sampling (`skip`s unset fields). `thinkingConfig`
/// is added when reasoning is wanted (see [`thinking_config`]).
fn generation_config(req: &ChatRequest, model: &str) -> Option<Value> {
    let s = &req.sampling;
    let mut cfg = Map::new();
    if let Some(v) = s.max_tokens {
        cfg.insert("maxOutputTokens".into(), json!(v));
    }
    if let Some(v) = s.temperature {
        cfg.insert("temperature".into(), json!(v));
    }
    if let Some(v) = s.top_p {
        cfg.insert("topP".into(), json!(v));
    }
    if let Some(v) = s.top_k {
        cfg.insert("topK".into(), json!(v));
    }
    if let Some(v) = s.seed {
        cfg.insert("seed".into(), json!(v));
    }
    if let Some(v) = s.frequency_penalty {
        cfg.insert("frequencyPenalty".into(), json!(v));
    }
    if let Some(v) = s.presence_penalty {
        cfg.insert("presencePenalty".into(), json!(v));
    }
    if let Some(tc) = thinking_config(req, model) {
        cfg.insert("thinkingConfig".into(), tc);
    }
    (!cfg.is_empty()).then_some(Value::Object(cfg))
}

/// `thinkingConfig` based on sampling and the model's generation. Gemini 3.x controls depth
/// via `thinkingLevel` (`minimal/low/medium/high`), Gemini 2.5 — via
/// `thinkingBudget` (tokens). `includeThoughts:true` enables a visible "thoughts" summary
/// (the API doesn't return raw CoT). `reasoning_budget==0` (impersonation/auto-title)
/// mutes "thoughts": `thinkingBudget:0` (2.5 — disable it; 3.x — the minimal level,
/// can't be fully disabled; 3 Pro's minimum is `low` — "minimal" isn't supported).
/// Returns `None` when reasoning isn't wanted
/// (the model uses thinking by default). The generation is inferred from the model name —
/// see docs/research/gemini-native-client.md §3, pitfall 1.
fn thinking_config(req: &ChatRequest, model: &str) -> Option<Value> {
    let s = &req.sampling;
    let thinking_on = s.thinking == Some(true);
    let force_off = s.reasoning_budget == Some(0);
    // Nothing was requested (thinking off, effort unset, not muting) — don't send
    // thinkingConfig: let the model decide for itself.
    if !thinking_on && s.reasoning_effort.is_none() && !force_off {
        return None;
    }
    let include = thinking_on && !force_off;
    let mut tc = Map::new();
    tc.insert("includeThoughts".into(), json!(include));

    if is_gemini_3(model) {
        // effort isn't set (level None) with thinking on — the level isn't sent (the model's default),
        // includeThoughts remains.
        if let Some(level) = gemini3_level(s, model, force_off) {
            tc.insert("thinkingLevel".into(), json!(level));
        }
    } else {
        tc.insert(
            "thinkingBudget".into(),
            json!(gemini25_budget(s, model, force_off)),
        );
    }
    Some(Value::Object(tc))
}

/// 3.x: thinkingLevel. Can't be fully disabled — force_off → the minimal level.
fn gemini3_level(s: &SamplingConfig, model: &str, force_off: bool) -> Option<&'static str> {
    let level = if force_off {
        Some("minimal")
    } else {
        s.reasoning_effort.and_then(effort_to_level)
    };
    // Gemini 3 **Pro** doesn't support `thinkingLevel:"minimal"` (returns `400`
    // "Thinking level MINIMAL is not supported for this model") — its minimum is
    // `low`. Clamp "minimal → low" for Pro (mirroring `is_gemini_25_pro`, where 0→128):
    // applies both to force_off (auto-title/impersonation) and an explicit effort=Minimal.
    level.map(|l| {
        if l == "minimal" && is_gemini_3_pro(model) {
            "low"
        } else {
            l
        }
    })
}

/// 2.5 and others: thinkingBudget (tokens).
fn gemini25_budget(s: &SamplingConfig, model: &str, force_off: bool) -> i64 {
    if force_off {
        // Gemini 2.5 Pro can't DISABLE thoughts (minimum 128) — `thinkingBudget:0`
        // would return `400`; send the minimum. Flash/Flash-Lite: `0` disables it.
        if is_gemini_25_pro(model) { 128 } else { 0 }
    } else {
        s.reasoning_effort.map(effort_to_budget).unwrap_or(-1) // -1 = dynamic
    }
}

/// The Gemini 3.x generation (uses `thinkingLevel`). A rough inference from the model name.
/// `pub(crate)` because the video client (`shared::video::gemini`) mutes thinking
/// the same way and must not carry a second copy of this heuristic.
pub(crate) fn is_gemini_3(model: &str) -> bool {
    model.contains("gemini-3")
}

/// Gemini 2.5 **Pro** — can't fully disable thoughts (`thinkingBudget` minimum 128).
fn is_gemini_25_pro(model: &str) -> bool {
    model.contains("gemini-2.5-pro")
}

/// Gemini 3.x **Pro** — doesn't support `thinkingLevel:"minimal"` (minimum `low`).
/// E.g. `gemini-3-pro-preview`, `gemini-3.1-pro-preview`.
pub(crate) fn is_gemini_3_pro(model: &str) -> bool {
    is_gemini_3(model) && model.contains("pro")
}

/// `reasoning_effort` → `thinkingLevel` (Gemini 3.x). `None` — the level isn't sent.
fn effort_to_level(e: ReasoningEffort) -> Option<&'static str> {
    match e {
        ReasoningEffort::None => None,
        ReasoningEffort::Minimal => Some("minimal"),
        ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        // Gemini has no xhigh — map it to the nearest one (high).
        ReasoningEffort::High | ReasoningEffort::XHigh => Some("high"),
    }
}

/// `reasoning_effort` → `thinkingBudget` (Gemini 2.5, tokens; mirrors the
/// OpenAI-compat table). `None` mutes it (0).
fn effort_to_budget(e: ReasoningEffort) -> i64 {
    match e {
        ReasoningEffort::None => 0,
        ReasoningEffort::Minimal | ReasoningEffort::Low => 1024,
        ReasoningEffort::Medium => 8192,
        ReasoningEffort::High | ReasoningEffort::XHigh => 24576,
    }
}

/// Translates history into the `contents` array. Roles `user`/`model` (assistant → `model`);
/// a tool result → a `functionResponse` part in `role:"user"`; system
/// is skipped (goes top-level). Adjacent messages of the same role are merged (Gemini
/// requires user/model alternation, while the agentic-loop gives several `tool` messages in a row).
fn build_contents(req: &ChatRequest) -> Vec<Value> {
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    let mut push = |role: &'static str, parts: Vec<Value>| {
        if parts.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((last_role, last_parts)) if *last_role == role => last_parts.extend(parts),
            _ => out.push((role, parts)),
        }
    };

    for m in &req.messages {
        match m.role {
            ApiRole::System => continue,
            ApiRole::User => push("user", user_parts(m)),
            ApiRole::Assistant => {
                let mut parts = text_parts(&m.content);
                for tc in &m.tool_calls {
                    // args — a JSON string; Gemini strictly requires an OBJECT. A non-object → `{}`.
                    let args = serde_json::from_str::<Value>(&tc.arguments)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({}));
                    let mut part = json!({
                        "functionCall": { "name": tc.name, "args": args }
                    });
                    // The thought signature (Gemini 3) — a neighbor of functionCall in the same part.
                    // Without it Gemini 3 rejects the historical call (`400`). See §2.3.
                    if let Some(sig) = &tc.thought_signature
                        && let Some(obj) = part.as_object_mut()
                    {
                        obj.insert("thoughtSignature".into(), json!(sig));
                    }
                    parts.push(part);
                }
                push("model", parts);
            }
            // A tool result → functionResponse inside a user. There's no function
            // name in the tool message, but there's `tool_call_id` = the client-synthesized
            // `"{name}-{index}"` (see client.rs) — the name is recovered from it
            // (Gemini matches by name). `response` must be an object.
            // Images a tool returned do NOT go inside the functionResponse: a multimodal
            // functionResponse is a hard `400` ("Multimodal function responses are not
            // supported for this model"). They are emitted as ordinary parts right after
            // it, in the same `role:"user"` content — which is where Gemini already puts a
            // tool result anyway, so the model sees the picture next to the answer it
            // belongs to. Chosen statically per provider, never by sending and catching the
            // 400 (fork F1-A, docs/research/mcp-tool-images.md).
            ApiRole::Tool => {
                let name = tool_name_from_id(m.tool_call_id.as_deref());
                let mut parts = vec![json!({
                    "functionResponse": {
                        "name": name,
                        "response": { "result": m.content },
                    }
                })];
                parts.extend(image_parts(&m.images));
                push("user", parts);
            }
        }
    }
    out.into_iter()
        .map(|(role, parts)| json!({ "role": role, "parts": parts }))
        .collect()
}

fn text_parts(content: &str) -> Vec<Value> {
    if content.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "text": content })]
    }
}

/// The parts of a user turn: images (each behind its label) first, then the text.
///
/// `inline_data` carries the payload in the request itself. Gemini also offers the Files
/// API, and its docs recommend it for large or reused media — but that is a second
/// service with its own lifecycle, and inline data is what the 20 MB request ceiling
/// comfortably fits once images are downscaled at attach time (spec §9.10). The
/// precedent in this codebase is `shared/video/gemini.rs`, which pushes a `file_data`
/// part the same way.
fn user_parts(m: &ApiMessage) -> Vec<Value> {
    if m.images.is_empty() {
        return text_parts(&m.content);
    }
    let mut parts = image_parts(&m.images);
    parts.extend(text_parts(&m.content));
    parts
}

/// `inline_data` parts, each preceded by its label when it has one. Shared by the user turn
/// and the tool result (fork F1-A) so an image looks the same wherever it entered the chat.
fn image_parts(images: &[ApiImage]) -> Vec<Value> {
    let mut parts = Vec::with_capacity(images.len() * 2);
    for image in images {
        if let Some(label) = &image.label {
            parts.push(json!({ "text": label }));
        }
        parts.push(json!({
            "inline_data": { "mime_type": image.mime, "data": image.data.as_ref() }
        }));
    }
    parts
}

/// Recovers the function name from a synthesized `tool_call_id` of the shape `"{name}-{index}"`
/// (the client forms the id this way, since native Gemini calls have no id). The
/// `-<number>` tail is stripped; if the format differs — taken as-is.
fn tool_name_from_id(id: Option<&str>) -> String {
    let Some(id) = id else {
        return String::new();
    };
    match id.rsplit_once('-') {
        Some((name, idx)) if !name.is_empty() && idx.chars().all(|c| c.is_ascii_digit()) => {
            name.to_string()
        }
        _ => id.to_string(),
    }
}

/// Sanitizes a tool's JSON schema to Gemini's OpenAPI subset: strips keys
/// Gemini doesn't accept at the root (`$schema`, `additionalProperties`). Phase C
/// will refine this against live schemas (nested `additionalProperties`, formats). See §3, pitfall 4.
fn sanitize_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if k == "$schema" || k == "additionalProperties" {
                    continue;
                }
                out.insert(k.clone(), sanitize_schema(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_schema).collect()),
        other => other.clone(),
    }
}

// ---------- streaming response ----------

/// A partial `GenerateContentResponse` (one SSE `data:` line). camelCase fields.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenResponse {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub usage_metadata: Option<UsageMetadata>,
    /// An error, reported with the **same** shape as a non-streaming one
    /// (documented). Every other field here defaults, so such an event used to
    /// deserialize as a perfectly valid empty chunk and end the turn as a silent
    /// `Stop` — see docs/research/cloud-retry-backoff.md §1.2.
    #[serde(default)]
    pub error: Option<GenError>,
    /// Prompt feedback: populated when the **request** itself is blocked
    /// by the filter (then `candidates` is empty) — otherwise an empty reply would look like an ordinary
    /// `STOP` with no explanation. See [`PromptFeedback`].
    #[serde(default)]
    pub prompt_feedback: Option<PromptFeedback>,
}

/// A Gemini error payload: the HTTP-ish `code`, prose `message`, and the canonical
/// `status` name (`UNAVAILABLE`/`RESOURCE_EXHAUSTED`/`INTERNAL`/…), which is what
/// says whether another attempt could succeed.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenError {
    #[serde(default)]
    pub code: Option<u16>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub status: String,
}

/// Prompt feedback. `block_reason` (`SAFETY`/`OTHER`/…) is present when the
/// request was rejected by the safety filter before generation.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptFeedback {
    #[serde(default)]
    pub block_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    #[serde(default)]
    pub content: Option<Content>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Content {
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// A content part. `thought:true` marks the "thoughts" summary text; `function_call`
/// — a tool call; `thought_signature` (Phase B) — a signature on the part.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub thought: Option<bool>,
    #[serde(default)]
    pub function_call: Option<FunctionCall>,
    /// A thought signature on the part (Gemini 3 `thoughtSignature`). The client attaches it to the call
    /// (`ToolCallDelta`→`ApiToolCall`) for resending on tool-use. See §2.3.
    #[serde(default)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FunctionCall {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

/// The token counter (`usageMetadata`). `thoughtsTokenCount` — "thoughts" tokens (counted
/// toward output billing), land in [`TokenUsage::reasoning_tokens`](crate::shared::api::contract::TokenUsage).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    #[serde(default)]
    pub prompt_token_count: u32,
    #[serde(default)]
    pub candidates_token_count: u32,
    #[serde(default)]
    pub thoughts_token_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ToolSchema};

    fn base_req(messages: Vec<ApiMessage>) -> ChatRequest {
        ChatRequest {
            continue_final: false,
            system: Some("Ты — ассистент.".into()),
            messages,
            sampling: SamplingConfig {
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        }
    }

    /// The mirror of the Chat Completions guarantee (spec §9.10): a user turn with no
    /// images keeps exactly the one text part it always had.
    #[test]
    fn a_text_only_user_turn_is_unchanged_by_the_image_support() {
        let json = serde_json::to_value(build_request(
            &base_req(vec![ApiMessage::user("привет")]),
            "gemini-2.5-flash",
        ))
        .unwrap();
        let parts = json["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["text"], "привет");
    }

    #[test]
    fn images_become_inline_data_parts_ahead_of_the_text() {
        let msg =
            ApiMessage::user("what is this?").with_images(vec![crate::shared::api::ApiImage::new(
                "image/png",
                "QUJD",
                Some("Image #1 — \"a.png\":".into()),
            )]);
        let json =
            serde_json::to_value(build_request(&base_req(vec![msg]), "gemini-2.5-flash")).unwrap();
        let parts = json["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["text"], "Image #1 — \"a.png\":");
        assert_eq!(parts[1]["inline_data"]["mime_type"], "image/png");
        assert_eq!(
            parts[1]["inline_data"]["data"], "QUJD",
            "bare base64, not a data: URI — Gemini takes the payload raw"
        );
        assert_eq!(parts[2]["text"], "what is this?");
    }

    /// Two images in one turn keep their order and each keeps its own label, so
    /// "the second picture" means the same thing to the model and to the user.
    #[test]
    fn several_images_keep_their_order_and_labels() {
        let msg = ApiMessage::user("").with_images(vec![
            crate::shared::api::ApiImage::new("image/png", "AAA", Some("Image #1:".into())),
            crate::shared::api::ApiImage::new("image/jpeg", "BBB", Some("Image #2:".into())),
        ]);
        let json =
            serde_json::to_value(build_request(&base_req(vec![msg]), "gemini-2.5-flash")).unwrap();
        let parts = json["contents"][0]["parts"].as_array().unwrap();
        // Four parts, not five: an image-only message adds no trailing empty text.
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0]["text"], "Image #1:");
        assert_eq!(parts[1]["inline_data"]["data"], "AAA");
        assert_eq!(parts[2]["text"], "Image #2:");
        assert_eq!(parts[3]["inline_data"]["mime_type"], "image/jpeg");
    }

    /// The tool-side mirror: a result with no images is exactly the one plain
    /// `functionResponse` part it always was — nothing added, nothing reshaped.
    #[test]
    fn a_tool_result_without_images_is_a_plain_function_response() {
        let r = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "calc-0".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            ),
            ApiMessage::tool("calc-0", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        let parts = json["contents"][2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        let fr = &parts[0]["functionResponse"];
        assert_eq!(fr["name"], "calc");
        assert_eq!(fr["response"], json!({ "result": "2" }));
    }

    /// Fork F1-A: Gemini answers a **multimodal `functionResponse` with a hard 400**
    /// ("Multimodal function responses are not supported for this model"). So the
    /// `functionResponse` stays text-only and the images follow it as ordinary parts of
    /// the same `role:"user"` content — where a tool result already lives here.
    #[test]
    fn a_tool_result_image_follows_the_function_response_instead_of_entering_it() {
        let r = base_req(vec![
            ApiMessage::user("сними скриншот"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "screenshot-0".into(),
                    name: "screenshot".into(),
                    arguments: "{}".into(),
                }],
            ),
            ApiMessage::tool("screenshot-0", "screenshot taken").with_images(vec![
                crate::shared::api::ApiImage::new(
                    "image/png",
                    "QUJD",
                    Some("Image #1 — \"shot.png\":".into()),
                ),
            ]),
        ]);
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        let content = &json["contents"][2];
        assert_eq!(content["role"], "user");
        let parts = content["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        // [0] the functionResponse — text only. This is the 400 the fork avoids: no
        // inline_data, no parts, nothing but the textual result.
        let fr = &parts[0]["functionResponse"];
        assert_eq!(fr["name"], "screenshot");
        assert_eq!(
            fr["response"],
            json!({ "result": "screenshot taken" }),
            "a multimodal functionResponse is a hard 400 — the image must never land inside it"
        );
        assert!(fr.get("parts").is_none());
        // [1..] the image, labelled, as a sibling part of the same user content.
        assert_eq!(parts[1]["text"], "Image #1 — \"shot.png\":");
        assert_eq!(parts[2]["inline_data"]["mime_type"], "image/png");
        assert_eq!(
            parts[2]["inline_data"]["data"], "QUJD",
            "bare base64, not a data: URI — Gemini takes the payload raw"
        );
    }

    /// A tool that returns only an image still emits its `functionResponse` (Gemini
    /// matches call↔response positionally, so dropping it would break the turn), with an
    /// empty `result` and the image beside it.
    #[test]
    fn an_image_only_tool_result_still_emits_its_function_response() {
        let r = base_req(vec![ApiMessage::tool("screenshot-0", "").with_images(
            vec![crate::shared::api::ApiImage::new(
                "image/jpeg",
                "QQ==",
                None,
            )],
        )]);
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        let parts = json["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["functionResponse"]["name"], "screenshot");
        assert_eq!(parts[0]["functionResponse"]["response"]["result"], "");
        assert_eq!(parts[1]["inline_data"]["data"], "QQ==");
    }

    #[test]
    fn system_is_top_level_and_generation_config_mapped() {
        let mut r = base_req(vec![ApiMessage::user("привет")]);
        r.sampling.temperature = Some(0.7);
        r.sampling.top_k = Some(40);
        r.sampling.top_p = Some(0.95);
        r.sampling.seed = Some(-1);
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        assert_eq!(
            json["systemInstruction"]["parts"][0]["text"],
            "Ты — ассистент."
        );
        assert_eq!(json["generationConfig"]["maxOutputTokens"], 256);
        assert_eq!(json["generationConfig"]["topK"], 40);
        assert_eq!(json["generationConfig"]["seed"], -1);
        assert!(json["generationConfig"].get("temperature").is_some());
        assert!(json["generationConfig"].get("topP").is_some());
        // The first content — a user with a text part.
        assert_eq!(json["contents"][0]["role"], "user");
        assert_eq!(json["contents"][0]["parts"][0]["text"], "привет");
        // Without reasoning, thinkingConfig isn't sent.
        assert!(json["generationConfig"].get("thinkingConfig").is_none());
    }

    #[test]
    fn thinking_budget_for_gemini_25() {
        let mut r = base_req(vec![ApiMessage::user("посчитай")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_effort = Some(ReasoningEffort::Medium);
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        let tc = &json["generationConfig"]["thinkingConfig"];
        assert_eq!(tc["includeThoughts"], true);
        assert_eq!(tc["thinkingBudget"], 8192);
        assert!(tc.get("thinkingLevel").is_none());
    }

    #[test]
    fn thinking_level_for_gemini_3() {
        let mut r = base_req(vec![ApiMessage::user("посчитай")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_effort = Some(ReasoningEffort::High);
        let json = serde_json::to_value(build_request(&r, "gemini-3-pro")).unwrap();
        let tc = &json["generationConfig"]["thinkingConfig"];
        assert_eq!(tc["includeThoughts"], true);
        assert_eq!(tc["thinkingLevel"], "high");
        assert!(tc.get("thinkingBudget").is_none());
    }

    #[test]
    fn reasoning_budget_zero_forces_off() {
        // reasoning_budget==0 (impersonation/auto-title): 2.5 → thinkingBudget 0,
        // includeThoughts false; 3.x → thinkingLevel minimal.
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_budget = Some(0);
        let j25 = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        let tc25 = &j25["generationConfig"]["thinkingConfig"];
        assert_eq!(tc25["thinkingBudget"], 0);
        assert_eq!(tc25["includeThoughts"], false);
        let j3 = serde_json::to_value(build_request(&r, "gemini-3-flash")).unwrap();
        let tc3 = &j3["generationConfig"]["thinkingConfig"];
        assert_eq!(tc3["thinkingLevel"], "minimal");
        assert_eq!(tc3["includeThoughts"], false);
        // 2.5 Pro can't disable thoughts (minimum 128) — force_off → 128, not 0.
        let jpro = serde_json::to_value(build_request(&r, "gemini-2.5-pro")).unwrap();
        let tcpro = &jpro["generationConfig"]["thinkingConfig"];
        assert_eq!(tcpro["thinkingBudget"], 128);
        assert_eq!(tcpro["includeThoughts"], false);
        // 3.x Pro doesn't support "minimal" — force_off clamps to "low".
        let j3pro = serde_json::to_value(build_request(&r, "gemini-3.1-pro-preview")).unwrap();
        let tc3pro = &j3pro["generationConfig"]["thinkingConfig"];
        assert_eq!(tc3pro["thinkingLevel"], "low");
        assert_eq!(tc3pro["includeThoughts"], false);
    }

    #[test]
    fn parses_prompt_feedback_block_reason() {
        let raw = r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#;
        let r: GenResponse = serde_json::from_str(raw).unwrap();
        assert!(r.candidates.is_empty());
        assert_eq!(
            r.prompt_feedback.and_then(|f| f.block_reason).as_deref(),
            Some("SAFETY")
        );
    }

    #[test]
    fn tools_use_function_declarations_and_strip_schema_keys() {
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.tools = vec![ToolSchema {
            name: "calc".into(),
            description: "Считает".into(),
            parameters: json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "additionalProperties": false,
                "properties": { "x": { "type": "number" } },
            }),
        }];
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        let decl = &json["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "calc");
        assert_eq!(decl["parameters"]["type"], "object");
        // $schema/additionalProperties are stripped (Gemini doesn't accept them).
        assert!(decl["parameters"].get("$schema").is_none());
        assert!(decl["parameters"].get("additionalProperties").is_none());
        assert_eq!(decl["parameters"]["properties"]["x"]["type"], "number");
    }

    #[test]
    fn tool_call_and_result_become_parts() {
        // assistant(functionCall) → model content with a functionCall part;
        // tool → user content with functionResponse (name from id "calc-0").
        let r = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "calc-0".into(),
                    name: "calc".into(),
                    arguments: "{\"x\":1}".into(),
                }],
            ),
            ApiMessage::tool("calc-0", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        // [0] user, [1] model(functionCall), [2] user(functionResponse).
        assert_eq!(json["contents"][1]["role"], "model");
        assert_eq!(
            json["contents"][1]["parts"][0]["functionCall"]["name"],
            "calc"
        );
        assert_eq!(
            json["contents"][1]["parts"][0]["functionCall"]["args"]["x"],
            1
        );
        assert_eq!(json["contents"][2]["role"], "user");
        let fr = &json["contents"][2]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "calc");
        assert_eq!(fr["response"]["result"], "2");
    }

    #[test]
    fn function_call_emits_thought_signature_when_present() {
        // Phase B: the thought signature (Gemini 3) is resent as a neighbor of functionCall.
        let with_sig = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    id: "calc-0".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                    thought_signature: Some("SIG-XYZ".into()),
                }],
            ),
        ]);
        let json = serde_json::to_value(build_request(&with_sig, "gemini-3-pro")).unwrap();
        let part = &json["contents"][1]["parts"][0];
        assert_eq!(part["functionCall"]["name"], "calc");
        assert_eq!(part["thoughtSignature"], "SIG-XYZ");

        // Without a signature, the key doesn't appear.
        let no_sig = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    id: "calc-0".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                    thought_signature: None,
                }],
            ),
        ]);
        let json = serde_json::to_value(build_request(&no_sig, "gemini-3-pro")).unwrap();
        assert!(
            json["contents"][1]["parts"][0]
                .get("thoughtSignature")
                .is_none()
        );
    }

    #[test]
    fn adjacent_tool_results_merge_into_one_user_content() {
        let r = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "calc-0".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            ),
            ApiMessage::tool("calc-0", "2"),
            ApiMessage::tool("calc-1", "3"),
        ]);
        let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
        // Two functionResponses are merged into one user content.
        assert_eq!(json["contents"][2]["role"], "user");
        assert_eq!(
            json["contents"][2]["parts"][0]["functionResponse"]["name"],
            "calc"
        );
        assert_eq!(
            json["contents"][2]["parts"][1]["functionResponse"]["response"]["result"],
            "3"
        );
    }

    #[test]
    fn tool_call_non_object_args_coerced() {
        for raw in ["null", "", "[1,2]", "42"] {
            let r = base_req(vec![
                ApiMessage::user("hi"),
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        thought_signature: None,
                        id: "f-0".into(),
                        name: "f".into(),
                        arguments: raw.into(),
                    }],
                ),
            ]);
            let json = serde_json::to_value(build_request(&r, "gemini-2.5-flash")).unwrap();
            let args = &json["contents"][1]["parts"][0]["functionCall"]["args"];
            assert!(
                args.is_object(),
                "args must be an object for {raw:?}, got {args}"
            );
        }
    }

    #[test]
    fn parses_streaming_response() {
        let raw = r#"{"candidates":[{"content":{"parts":[{"text":"привет"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"thoughtsTokenCount":0}}"#;
        let r: GenResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            r.candidates[0].content.as_ref().unwrap().parts[0]
                .text
                .as_deref(),
            Some("привет")
        );
        assert_eq!(r.usage_metadata.as_ref().unwrap().prompt_token_count, 10);
        assert_eq!(r.usage_metadata.as_ref().unwrap().candidates_token_count, 3);
    }

    #[test]
    fn parses_thought_part() {
        let raw = r#"{"candidates":[{"content":{"parts":[{"text":"думаю…","thought":true}]}}]}"#;
        let r: GenResponse = serde_json::from_str(raw).unwrap();
        let p = &r.candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(p.thought, Some(true));
        assert_eq!(p.text.as_deref(), Some("думаю…"));
    }

    #[test]
    fn parses_function_call_part_and_finish() {
        let raw = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"calc","args":{"x":1}},"thoughtSignature":"SIG"}]},"finishReason":"STOP"}]}"#;
        let r: GenResponse = serde_json::from_str(raw).unwrap();
        let p = &r.candidates[0].content.as_ref().unwrap().parts[0];
        let fc = p.function_call.as_ref().unwrap();
        assert_eq!(fc.name, "calc");
        assert_eq!(fc.args["x"], 1);
        assert_eq!(p.thought_signature.as_deref(), Some("SIG"));
        assert_eq!(r.candidates[0].finish_reason.as_deref(), Some("STOP"));
    }
}

/// An error payload inside an open stream. The trap being pinned: every field of
/// [`GenResponse`] defaults, so before `error` existed such a payload parsed as a
/// perfectly valid empty chunk and the turn ended as a silent `Stop`.
#[cfg(test)]
mod stream_error_tests {
    use super::*;

    #[test]
    fn an_error_payload_is_recognized_and_classified() {
        let data =
            r#"{"error":{"code":503,"message":"The model is overloaded.","status":"UNAVAILABLE"}}"#;
        let resp: GenResponse = serde_json::from_str(data).unwrap();
        let err = resp.error.expect("the error payload must be read");
        assert_eq!(err.code, Some(503));
        assert_eq!(err.status, "UNAVAILABLE");
        assert!(crate::shared::api::error::stream_error_transient(
            &err.status,
            err.code
        ));
        // And it is not mistaken for content.
        assert!(resp.candidates.is_empty());
    }

    #[test]
    fn a_quota_error_is_transient_but_an_argument_error_is_not() {
        let quota = r#"{"error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED"}}"#;
        let bad = r#"{"error":{"code":400,"message":"bad","status":"INVALID_ARGUMENT"}}"#;
        for (data, expected) in [(quota, true), (bad, false)] {
            let err = serde_json::from_str::<GenResponse>(data)
                .unwrap()
                .error
                .unwrap();
            assert_eq!(
                crate::shared::api::error::stream_error_transient(&err.status, err.code),
                expected,
                "{data}"
            );
        }
    }

    /// An ordinary chunk must not grow an error out of nowhere.
    #[test]
    fn a_normal_chunk_carries_no_error() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"hi"}],"role":"model"}}]}"#;
        assert!(
            serde_json::from_str::<GenResponse>(data)
                .unwrap()
                .error
                .is_none()
        );
    }
}
