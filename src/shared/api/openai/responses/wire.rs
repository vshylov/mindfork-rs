//! Serde-типы протокола OpenAI Responses (`/v1/responses`) и трансляция доменного
//! [`ChatRequest`] в его формат. Отличия от Chat Completions (см. ADR 0004,
//! docs/research/openai-responses-client.md):
//! - системное сообщение — top-level `instructions` (не роль в `messages`);
//! - история — массив `input` из **элементов** (сообщения, `reasoning`,
//!   `function_call`, `function_call_output`), а не `messages` c `tool_calls`;
//! - результат инструмента — элемент `function_call_output` (нет роли `tool`);
//! - лимит токенов — `max_output_tokens` (включает reasoning-токены!);
//! - reasoning-элемент с `encrypted_content` возвращается **перед** своим
//!   `function_call` (stateless-режим `store:false`, аналог подписи thinking Anthropic).
//!
//! Событийный SSE: тег события — поле `type` внутри `data` (как у Anthropic).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::shared::api::contract::{ApiRole, ChatRequest};

// ---------- запрос ----------

#[derive(Debug, Serialize)]
pub struct RespRequest {
    pub model: String,
    /// Системное сообщение (top-level, не в `input`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub input: Vec<Value>,
    pub stream: bool,
    /// История у приложения своя → не просим сервер сохранять ответы (приватность,
    /// stateless). При `store:false` reasoning-элементы возвращаются во `input`.
    pub store: bool,
    /// `include: ["reasoning.encrypted_content"]` — просим зашифрованное рассуждение
    /// в reasoning-элементах (нужно для переотправки при tool-use). Шлём только когда
    /// reasoning включён (иначе бессмысленно/на не-reasoning моделях лишнее).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<&'static str>,
    /// Лимит токенов ответа (включает reasoning-токены — при скупом значении рассуждение
    /// съест бюджет, а `output_text` придёт пустым; см. docs/research/openai-responses-client.md).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<RespReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<RespText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<RespTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<&'static str>,
}

/// Конфиг рассуждений. `effort` — глубина (`none`/`minimal`/`low`/`medium`/`high`/
/// `xhigh`); `summary` — `auto` для видимого резюме «мыслей» (сырой CoT API не отдаёт).
#[derive(Debug, Serialize)]
pub struct RespReasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<&'static str>,
}

/// `text.verbosity` — многословность ответа.
#[derive(Debug, Serialize)]
pub struct RespText {
    pub verbosity: &'static str,
}

/// Function-tool в плоской форме Responses (`{type,name,description,parameters,strict}`).
/// `strict:false` — наши схемы не удовлетворяют требованиям строгого режима
/// (`additionalProperties:false` + все поля в `required`), а по умолчанию Responses
/// «пробует строгий» — отключаем явно.
#[derive(Debug, Serialize)]
pub struct RespTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub strict: bool,
}

/// Строит тело запроса Responses из доменного [`ChatRequest`]. `model` обязателен.
/// Из семплинга применяются `max_tokens`→`max_output_tokens`, `thinking`/
/// `reasoning_effort`→`reasoning`, `verbosity`→`text` — прочие поля Responses не имеет.
pub fn build_request(req: &ChatRequest, model: &str, stream: bool) -> RespRequest {
    let s = &req.sampling;
    // reasoning_budget==0 форсирует выключение «мыслей» (имп(ерсонация)/авто-название),
    // как reasoning_budget=0 у llama.cpp: effort=none, без summary.
    let force_off = s.reasoning_budget == Some(0);
    let want_summary = !force_off && s.thinking == Some(true);
    let effort = if force_off {
        Some("none")
    } else {
        s.reasoning_effort.map(|e| e.as_wire())
    };
    // summary: "detailed", а не "auto" — часть моделей на "auto" отдаёт ПУСТОЕ резюме,
    // а на "detailed" — текст (сообщения разработчиков; gpt-5.x поддерживает detailed).
    // ВАЖНО: резюме рассуждений приходит только организациям, прошедшим верификацию
    // (platform.openai.com/settings/organization/general) — иначе поток «мыслей» пуст
    // (или неверифицированной орг. приходит 400 на сам факт `reasoning.summary`).
    let reasoning = (want_summary || effort.is_some()).then_some(RespReasoning {
        effort,
        summary: want_summary.then_some("detailed"),
    });
    // Зашифрованное рассуждение нужно только когда reasoning действительно включён.
    let include = if reasoning.is_some() {
        vec!["reasoning.encrypted_content"]
    } else {
        Vec::new()
    };

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| RespTool {
                    kind: "function",
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                    strict: false,
                })
                .collect(),
        )
    };
    let tool_choice = tools.as_ref().map(|_| "auto");

    RespRequest {
        model: model.to_string(),
        instructions: req.system.clone(),
        input: build_input(req),
        stream,
        store: false,
        include,
        max_output_tokens: s.max_tokens,
        reasoning,
        text: s.verbosity.map(|v| RespText {
            verbosity: v.as_wire(),
        }),
        tools,
        tool_choice,
    }
}

/// Транслирует историю в массив `input` Responses. Reasoning-элемент (с `id`+
/// `encrypted_content`) ставится **перед** `function_call`-элементами того же
/// assistant-хода — Responses ждёт его непосредственно перед вызовом (см.
/// [`ThinkingBlock`](crate::shared::api::contract::ThinkingBlock)).
fn build_input(req: &ChatRequest) -> Vec<Value> {
    let mut items = Vec::new();
    for m in &req.messages {
        match m.role {
            // Системное сообщение идёт в top-level `instructions`.
            ApiRole::System => continue,
            ApiRole::User => items.push(json!({
                "type": "message", "role": "user", "content": m.content,
            })),
            ApiRole::Assistant => {
                // Reasoning-элемент текущего хода (только если есть id — его несёт
                // лишь OpenAI Responses; у прочих бэкендов thinking.id == None).
                if let Some(tb) = &m.thinking
                    && let Some(id) = &tb.id
                {
                    // `summary` — ОБЯЗАТЕЛЬНОЕ поле reasoning-элемента в Responses API
                    // (иначе 400 `Missing required parameter: 'input[N].summary'`). Шлём
                    // пустой массив: смысл несёт `encrypted_content`, а текст резюме не
                    // нужен для переотправки (и у неверифицированной орг. он пуст, §7a
                    // docs/research/openai-responses-client.md).
                    items.push(json!({
                        "type": "reasoning",
                        "id": id,
                        "summary": [],
                        "encrypted_content": tb.signature,
                    }));
                }
                if !m.content.is_empty() {
                    items.push(json!({
                        "type": "message", "role": "assistant", "content": m.content,
                    }));
                }
                for tc in &m.tool_calls {
                    // arguments — JSON-строка; безаргументный вызов → пустой объект.
                    let args = if tc.arguments.is_empty() {
                        "{}"
                    } else {
                        tc.arguments.as_str()
                    };
                    items.push(json!({
                        "type": "function_call",
                        "call_id": tc.id,
                        "name": tc.name,
                        "arguments": args,
                    }));
                }
            }
            ApiRole::Tool => items.push(json!({
                "type": "function_call_output",
                "call_id": m.tool_call_id.clone().unwrap_or_default(),
                "output": m.content,
            })),
        }
    }
    items
}

// ---------- стриминговые события ----------

/// Событие SSE Responses (тег — поле `type` в `data`). Неинтересные события
/// (`response.created`, `*.part.added`, `*.done` кроме `output_item.done`, `ping`)
/// попадают в [`RespEvent::Other`].
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RespEvent {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryDelta {
        #[serde(default)]
        delta: String,
    },
    /// Некоторые reasoning-модели/настройки стримят рассуждение этим событием, а не
    /// summary-событием — обрабатываем оба (оба → `ChatChunk::Thoughts`).
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        #[serde(default)]
        output_index: usize,
        item: RespItem,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone { item: RespItem },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionArgsDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.completed")]
    Completed { response: RespBody },
    #[serde(rename = "response.incomplete")]
    Incomplete {
        #[serde(default)]
        response: Option<RespBody>,
    },
    #[serde(rename = "response.failed")]
    Failed,
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        message: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// Элемент вывода (`output_item.added`/`done`). Интересны `function_call` (даёт
/// `call_id`+имя) и `reasoning` (в `done` несёт `encrypted_content`); прочее (текстовое
/// сообщение и т.п.) — [`RespItem::Other`].
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RespItem {
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default)]
        call_id: String,
        #[serde(default)]
        name: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        id: String,
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// Объект `response` в терминальных событиях (`completed`/`incomplete`). Нужен только
/// счётчик токенов — `status`/прочее игнорируем (причину завершения выводит клиент).
#[derive(Debug, Default, Deserialize)]
pub struct RespBody {
    #[serde(default)]
    pub usage: Option<RespUsage>,
}

/// Счётчик токенов ответа Responses (`input_tokens`/`output_tokens` +
/// `output_tokens_details.reasoning_tokens`).
#[derive(Debug, Default, Deserialize)]
pub struct RespUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub output_tokens_details: RespOutputTokensDetails,
}

/// Детализация токенов ответа (интересуют reasoning-токены «мыслей»).
#[derive(Debug, Default, Deserialize)]
pub struct RespOutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig, Verbosity};
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ThinkingBlock, ToolSchema};

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
    fn system_is_instructions_and_store_false() {
        let json = serde_json::to_value(build_request(
            &base_req(vec![ApiMessage::user("hi")]),
            "gpt-x",
            true,
        ))
        .unwrap();
        assert_eq!(json["model"], "gpt-x");
        assert_eq!(json["instructions"], "Ты — ассистент.");
        assert_eq!(json["store"], false);
        assert_eq!(json["max_output_tokens"], 256);
        assert!(json.get("max_tokens").is_none());
        // Первый input-элемент — user-сообщение со строковым content.
        assert_eq!(json["input"][0]["type"], "message");
        assert_eq!(json["input"][0]["role"], "user");
        assert_eq!(json["input"][0]["content"], "hi");
        // Без reasoning не просим encrypted_content и не шлём reasoning/text.
        assert!(json.get("include").is_none());
        assert!(json.get("reasoning").is_none());
        assert!(json.get("text").is_none());
    }

    #[test]
    fn reasoning_summary_and_effort_and_verbosity() {
        let mut r = base_req(vec![ApiMessage::user("посчитай")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_effort = Some(ReasoningEffort::XHigh);
        r.sampling.verbosity = Some(Verbosity::Low);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["reasoning"]["effort"], "xhigh");
        assert_eq!(json["reasoning"]["summary"], "detailed");
        assert_eq!(json["text"]["verbosity"], "low");
        // Reasoning включён → просим зашифрованное рассуждение.
        assert_eq!(json["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn reasoning_budget_zero_forces_off() {
        // thinking включён, но reasoning_budget=0 (импперсонация/авто-название) →
        // effort=none, без summary; include не шлём.
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_budget = Some(0);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["reasoning"]["effort"], "none");
        assert!(json["reasoning"].get("summary").is_none());
    }

    #[test]
    fn effort_without_summary_when_thinking_off() {
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.sampling.reasoning_effort = Some(ReasoningEffort::Low);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["reasoning"]["effort"], "low");
        assert!(json["reasoning"].get("summary").is_none());
        // effort задан (reasoning есть) → include присутствует.
        assert_eq!(json["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn tools_are_flat_with_strict_false() {
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.tools = vec![ToolSchema {
            name: "calc".into(),
            description: "Считает".into(),
            parameters: json!({"type":"object"}),
        }];
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["tools"][0]["type"], "function");
        assert_eq!(json["tools"][0]["name"], "calc");
        assert_eq!(json["tools"][0]["strict"], false);
        assert_eq!(json["tools"][0]["parameters"]["type"], "object");
        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn tool_call_and_result_become_items() {
        let r = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "call_1".into(),
                    name: "calc".into(),
                    arguments: "{\"x\":1}".into(),
                }],
            ),
            ApiMessage::tool("call_1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        // [0] user message, [1] function_call, [2] function_call_output.
        assert_eq!(json["input"][1]["type"], "function_call");
        assert_eq!(json["input"][1]["call_id"], "call_1");
        assert_eq!(json["input"][1]["name"], "calc");
        assert_eq!(json["input"][1]["arguments"], "{\"x\":1}");
        assert_eq!(json["input"][2]["type"], "function_call_output");
        assert_eq!(json["input"][2]["call_id"], "call_1");
        assert_eq!(json["input"][2]["output"], "2");
    }

    #[test]
    fn reasoning_item_precedes_function_call() {
        // assistant-ход с thinking (id+encrypted) → reasoning-элемент перед function_call.
        let r = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "call_1".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            )
            .with_thinking(Some(ThinkingBlock {
                text: "резюме".into(),
                signature: "gAAA-enc".into(),
                id: Some("rs_42".into()),
            })),
            ApiMessage::tool("call_1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        // [0] user, [1] reasoning (id+summary+encrypted), [2] function_call, [3] output.
        assert_eq!(json["input"][1]["type"], "reasoning");
        assert_eq!(json["input"][1]["id"], "rs_42");
        assert_eq!(json["input"][1]["encrypted_content"], "gAAA-enc");
        // `summary` обязателен для reasoning-элемента (иначе 400) — шлём пустой массив.
        assert_eq!(json["input"][1]["summary"], json!([]));
        assert_eq!(json["input"][2]["type"], "function_call");
    }

    #[test]
    fn thinking_without_id_omits_reasoning_item() {
        // thinking-блок без id (Anthropic-стиль) не даёт reasoning-элемента в Responses.
        let r = base_req(vec![
            ApiMessage::user("hi"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "call_1".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            )
            .with_thinking(Some(ThinkingBlock {
                text: "x".into(),
                signature: "sig".into(),
                id: None,
            })),
        ]);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["input"][1]["type"], "function_call");
    }

    #[test]
    fn parses_stream_events() {
        let td = r#"{"type":"response.output_text.delta","delta":"hi"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(td).unwrap(),
            RespEvent::OutputTextDelta { delta } if delta == "hi"
        ));
        let rd = r#"{"type":"response.reasoning_summary_text.delta","delta":"думаю"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(rd).unwrap(),
            RespEvent::ReasoningSummaryDelta { delta } if delta == "думаю"
        ));
        // Альтернативное событие рассуждения — тоже распознаётся.
        let rt = r#"{"type":"response.reasoning_text.delta","delta":"шаг"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(rt).unwrap(),
            RespEvent::ReasoningTextDelta { delta } if delta == "шаг"
        ));
        let added = r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"calc","arguments":""}}"#;
        match serde_json::from_str::<RespEvent>(added).unwrap() {
            RespEvent::OutputItemAdded {
                output_index,
                item: RespItem::FunctionCall { call_id, name },
            } => {
                assert_eq!(output_index, 1);
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "calc");
            }
            other => panic!("ожидался function_call added, получили {other:?}"),
        }
        let done = r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_9","encrypted_content":"ENC"}}"#;
        match serde_json::from_str::<RespEvent>(done).unwrap() {
            RespEvent::OutputItemDone {
                item:
                    RespItem::Reasoning {
                        id,
                        encrypted_content: Some(enc),
                    },
                ..
            } => {
                assert_eq!(id, "rs_9");
                assert_eq!(enc, "ENC");
            }
            other => panic!("ожидался reasoning done, получили {other:?}"),
        }
        let args = r#"{"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"x\":1}"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(args).unwrap(),
            RespEvent::FunctionArgsDelta { output_index, delta } if output_index == 1 && delta == "{\"x\":1}"
        ));
        let completed = r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":42,"output_tokens":7}}}"#;
        match serde_json::from_str::<RespEvent>(completed).unwrap() {
            RespEvent::Completed { response } => {
                let u = response.usage.unwrap();
                assert_eq!(u.input_tokens, 42);
                assert_eq!(u.output_tokens, 7);
            }
            other => panic!("ожидался completed, получили {other:?}"),
        }
        // Неинтересное событие → Other (не падаем).
        assert!(matches!(
            serde_json::from_str::<RespEvent>(r#"{"type":"response.created","response":{}}"#)
                .unwrap(),
            RespEvent::Other
        ));
        // Текстовое сообщение как output-элемент → RespItem::Other.
        let msg_added = r#"{"type":"response.output_item.added","output_index":2,"item":{"type":"message","role":"assistant","content":[]}}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(msg_added).unwrap(),
            RespEvent::OutputItemAdded {
                item: RespItem::Other,
                ..
            }
        ));
    }
}
