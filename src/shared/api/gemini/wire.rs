//! Serde-типы нативного протокола Google Gemini (`generateContent`/
//! `streamGenerateContent`) и трансляция доменного [`ChatRequest`] в его формат.
//! Отличия от OpenAI Chat Completions (см. ADR 0004, docs/research/gemini-native-client.md):
//! - системное сообщение — top-level `systemInstruction:{parts:[{text}]}`;
//! - история — `contents:[{role:"user"|"model", parts:[…]}]` (ролей `system`/`tool`
//!   нет: system → top-level, результат инструмента → часть `functionResponse` в
//!   `role:"user"`); соседние сообщения одной роли склеиваются;
//! - вызов инструмента — часть `{functionCall:{name, args}}` (`args` — ОБЪЕКТ, не строка;
//!   `id` у вызова отсутствует, парность `functionCall`↔`functionResponse` позиционная);
//! - лимит токенов — `generationConfig.maxOutputTokens` (включает токены мыслей!);
//! - reasoning — `generationConfig.thinkingConfig` (`thinkingLevel` для Gemini 3.x /
//!   `thinkingBudget` для 2.5 + `includeThoughts` для видимого резюме «мыслей»).
//!
//! Подписи мыслей (`thoughtSignature`) — сосед `functionCall`-части; переотправляются
//! на реплее истории (Gemini 3 иначе `400`), из [`ApiToolCall::thought_signature`](crate::shared::api::contract::ApiToolCall).
//! Событийный SSE (`?alt=sse`): строки `data: {…}` с частичным `GenerateContentResponse`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::entities::sampling::ReasoningEffort;
use crate::shared::api::contract::{ApiRole, ChatRequest};

// ---------- запрос ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenRequest {
    /// Системное сообщение (top-level, не в `contents`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Value>,
    pub contents: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<Value>,
}

/// Строит тело запроса `generateContent` из доменного [`ChatRequest`]. `model` нужен
/// лишь для выбора формата thinking-конфига (`thinkingLevel` у Gemini 3.x vs
/// `thinkingBudget` у 2.5) — в тело он не пишется (модель — в URL клиента).
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

/// Собирает `generationConfig` из семплинга (`skip` незаданных). `thinkingConfig`
/// добавляется, когда reasoning востребован (см. [`thinking_config`]).
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

/// `thinkingConfig` по семплингу и поколению модели. Gemini 3.x управляет глубиной
/// через `thinkingLevel` (`minimal/low/medium/high`), Gemini 2.5 — через
/// `thinkingBudget` (токены). `includeThoughts:true` включает видимое резюме «мыслей»
/// (сырой CoT API не отдаёт). `reasoning_budget==0` (импперсонация/авто-название)
/// глушит «мысли»: `thinkingBudget:0` (2.5 — выключить; 3.x — минимальный уровень,
/// полностью выключить нельзя; у 3 Pro минимум `low` — «minimal» не поддержан).
/// Возвращает `None`, когда reasoning не востребован
/// (модель использует thinking по умолчанию). Инференс поколения по имени модели —
/// см. docs/research/gemini-native-client.md §3, ловушка 1.
fn thinking_config(req: &ChatRequest, model: &str) -> Option<Value> {
    let s = &req.sampling;
    let thinking_on = s.thinking == Some(true);
    let force_off = s.reasoning_budget == Some(0);
    // Ничего не запрошено (thinking выкл, effort не задан, не глушим) — не шлём
    // thinkingConfig: пусть модель решает сама.
    if !thinking_on && s.reasoning_effort.is_none() && !force_off {
        return None;
    }
    let include = thinking_on && !force_off;
    let mut tc = Map::new();
    tc.insert("includeThoughts".into(), json!(include));

    if is_gemini_3(model) {
        // 3.x: thinkingLevel. Полностью выключить нельзя — force_off → минимальный уровень.
        let level = if force_off {
            Some("minimal")
        } else {
            s.reasoning_effort.and_then(effort_to_level)
        };
        // Gemini 3 **Pro** не поддерживает `thinkingLevel:"minimal"` (вернёт `400`
        // «Thinking level MINIMAL is not supported for this model») — минимум у него
        // `low`. Кламп «minimal → low» для Pro (зеркало `is_gemini_25_pro`, где 0→128):
        // касается и force_off (авто-название/импперсонация), и явного effort=Minimal.
        let level = level.map(|l| {
            if l == "minimal" && is_gemini_3_pro(model) {
                "low"
            } else {
                l
            }
        });
        // effort не задан (level None) при thinking on — уровень не шлём (дефолт модели),
        // остаётся includeThoughts.
        if let Some(level) = level {
            tc.insert("thinkingLevel".into(), json!(level));
        }
    } else {
        // 2.5 и прочие: thinkingBudget (токены).
        let budget = if force_off {
            // Gemini 2.5 Pro не умеет ВЫКЛЮЧАТЬ мысли (минимум 128) — `thinkingBudget:0`
            // вернул бы `400`; шлём минимум. Flash/Flash-Lite: `0` выключает.
            if is_gemini_25_pro(model) { 128 } else { 0 }
        } else {
            s.reasoning_effort.map(effort_to_budget).unwrap_or(-1) // -1 = динамически
        };
        tc.insert("thinkingBudget".into(), json!(budget));
    }
    Some(Value::Object(tc))
}

/// Поколение Gemini 3.x (использует `thinkingLevel`). Грубый инференс по имени модели.
fn is_gemini_3(model: &str) -> bool {
    model.contains("gemini-3")
}

/// Gemini 2.5 **Pro** — не умеет полностью выключать мысли (`thinkingBudget` минимум 128).
fn is_gemini_25_pro(model: &str) -> bool {
    model.contains("gemini-2.5-pro")
}

/// Gemini 3.x **Pro** — не поддерживает `thinkingLevel:"minimal"` (минимум `low`).
/// Например `gemini-3-pro-preview`, `gemini-3.1-pro-preview`.
fn is_gemini_3_pro(model: &str) -> bool {
    is_gemini_3(model) && model.contains("pro")
}

/// `reasoning_effort` → `thinkingLevel` (Gemini 3.x). `None` — уровень не шлём.
fn effort_to_level(e: ReasoningEffort) -> Option<&'static str> {
    match e {
        ReasoningEffort::None => None,
        ReasoningEffort::Minimal => Some("minimal"),
        ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        // xhigh у Gemini нет — приводим к ближайшему (high).
        ReasoningEffort::High | ReasoningEffort::XHigh => Some("high"),
    }
}

/// `reasoning_effort` → `thinkingBudget` (Gemini 2.5, токены; зеркало таблицы
/// OpenAI-compat). `None` глушит (0).
fn effort_to_budget(e: ReasoningEffort) -> i64 {
    match e {
        ReasoningEffort::None => 0,
        ReasoningEffort::Minimal | ReasoningEffort::Low => 1024,
        ReasoningEffort::Medium => 8192,
        ReasoningEffort::High | ReasoningEffort::XHigh => 24576,
    }
}

/// Транслирует историю в массив `contents`. Роли `user`/`model` (ассистент → `model`);
/// результат инструмента → часть `functionResponse` в `role:"user"`; system
/// пропускается (идёт top-level). Соседние сообщения одной роли склеиваются (Gemini
/// требует чередования user/model, а agentic-loop даёт несколько `tool` подряд).
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
            ApiRole::User => push("user", text_parts(&m.content)),
            ApiRole::Assistant => {
                let mut parts = text_parts(&m.content);
                for tc in &m.tool_calls {
                    // args — JSON-строка; Gemini строго требует ОБЪЕКТ. Не-объект → `{}`.
                    let args = serde_json::from_str::<Value>(&tc.arguments)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({}));
                    let mut part = json!({
                        "functionCall": { "name": tc.name, "args": args }
                    });
                    // Подпись мысли (Gemini 3) — сосед functionCall в той же части.
                    // Без неё Gemini 3 отвергает исторический вызов (`400`). См. §2.3.
                    if let Some(sig) = &tc.thought_signature
                        && let Some(obj) = part.as_object_mut()
                    {
                        obj.insert("thoughtSignature".into(), json!(sig));
                    }
                    parts.push(part);
                }
                push("model", parts);
            }
            // Результат инструмента → functionResponse внутри user. У нас нет имени
            // функции в tool-сообщении, зато есть `tool_call_id` = синтезированный
            // клиентом `"{name}-{index}"` (см. client.rs) — имя восстанавливаем из него
            // (Gemini сопоставляет по имени). `response` обязан быть объектом.
            ApiRole::Tool => {
                let name = tool_name_from_id(m.tool_call_id.as_deref());
                push(
                    "user",
                    vec![json!({
                        "functionResponse": {
                            "name": name,
                            "response": { "result": m.content },
                        }
                    })],
                );
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

/// Восстанавливает имя функции из синтезированного `tool_call_id` вида `"{name}-{index}"`
/// (клиент так формирует id, т.к. у нативного Gemini вызовы без id). Отрезаем хвост
/// `-<число>`; если формат иной — берём как есть.
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

/// Санитизация JSON-схемы инструмента под OpenAPI-подмножество Gemini: снимаем ключи,
/// которых Gemini не принимает на корне (`$schema`, `additionalProperties`). Фаза C
/// уточнит по живым схемам (вложенные `additionalProperties`, форматы). См. §3, ловушка 4.
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

// ---------- стриминговый ответ ----------

/// Частичный `GenerateContentResponse` (одна SSE-строка `data:`). Поля camelCase.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenResponse {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub usage_metadata: Option<UsageMetadata>,
    /// Обратная связь по промпту: заполняется, когда сам **запрос** заблокирован
    /// фильтром (тогда `candidates` пуст) — иначе пустой ответ выглядел бы как обычный
    /// `STOP` без объяснения. См. [`PromptFeedback`].
    #[serde(default)]
    pub prompt_feedback: Option<PromptFeedback>,
}

/// Обратная связь по промпту. `block_reason` (`SAFETY`/`OTHER`/…) присутствует, когда
/// запрос отклонён фильтром безопасности до генерации.
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

/// Часть содержимого. `thought:true` помечает текст резюме «мыслей»; `function_call`
/// — вызов инструмента; `thought_signature` (Фаза B) — подпись на части.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub thought: Option<bool>,
    #[serde(default)]
    pub function_call: Option<FunctionCall>,
    /// Подпись мысли на части (Gemini 3 `thoughtSignature`). Клиент кладёт её на вызов
    /// (`ToolCallDelta`→`ApiToolCall`) для переотправки при tool-use. См. §2.3.
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

/// Счётчик токенов (`usageMetadata`). `thoughtsTokenCount` — токены «мыслей» (входят
/// в биллинг вывода), ложатся в [`TokenUsage::reasoning_tokens`](crate::shared::api::contract::TokenUsage).
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
            system: Some("Ты — ассистент.".into()),
            messages,
            sampling: SamplingConfig {
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        }
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
        // Первый content — user с текстовой частью.
        assert_eq!(json["contents"][0]["role"], "user");
        assert_eq!(json["contents"][0]["parts"][0]["text"], "привет");
        // Без reasoning thinkingConfig не шлём.
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
        // reasoning_budget==0 (импперсонация/авто-название): 2.5 → thinkingBudget 0,
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
        // 2.5 Pro не умеет выключать мысли (минимум 128) — force_off → 128, не 0.
        let jpro = serde_json::to_value(build_request(&r, "gemini-2.5-pro")).unwrap();
        let tcpro = &jpro["generationConfig"]["thinkingConfig"];
        assert_eq!(tcpro["thinkingBudget"], 128);
        assert_eq!(tcpro["includeThoughts"], false);
        // 3.x Pro не поддерживает "minimal" — force_off клампится к "low".
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
        // $schema/additionalProperties вычищены (Gemini их не принимает).
        assert!(decl["parameters"].get("$schema").is_none());
        assert!(decl["parameters"].get("additionalProperties").is_none());
        assert_eq!(decl["parameters"]["properties"]["x"]["type"], "number");
    }

    #[test]
    fn tool_call_and_result_become_parts() {
        // assistant(functionCall) → model-content с частью functionCall;
        // tool → user-content с functionResponse (имя из id "calc-0").
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
        // Фаза B: подпись мысли (Gemini 3) переотправляется соседом functionCall.
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

        // Без подписи ключ не появляется.
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
        // Два functionResponse склеены в один user-content.
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
                "args должен быть объектом для {raw:?}, получили {args}"
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
