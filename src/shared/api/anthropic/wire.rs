//! Serde-типы протокола Anthropic Messages (`/v1/messages`) и трансляция доменного
//! [`ChatRequest`] в его формат. Отличия от OpenAI (см. ADR 0004, Фаза 2):
//! - системное сообщение — **top-level** поле `system` (не роль в `messages`);
//! - роли только `user`/`assistant`; результаты инструментов — блоки `tool_result`
//!   **внутри user-сообщения** (роль `tool` отсутствует);
//! - вызовы инструментов ассистента — блоки `tool_use` в его `content`;
//! - `max_tokens` **обязателен**; схема инструмента — `input_schema` (не `parameters`).
//!
//! Соседние сообщения одной роли склеиваются (Anthropic требует чередования
//! user/assistant; наш agentic-loop даёт несколько подряд `tool` → один user-блок).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entities::sampling::ReasoningEffort;
use crate::shared::api::contract::{ApiRole, ChatRequest};

/// `max_tokens` по умолчанию, если в семплинге не задан (Anthropic требует поле).
pub const DEFAULT_MAX_TOKENS: u64 = 4096;

// ---------- запрос ----------

#[derive(Debug, Serialize)]
pub struct AntRequest {
    pub model: String,
    pub max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AntMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AntTool>>,
    /// Extended thinking. Современные Claude (Opus 4.6+/Sonnet 4.6/Fable 5) принимают
    /// только `{type:"adaptive"}` — старое `budget_tokens` отвергают `400`. `display:
    /// "summarized"` нужен, чтобы текст «мыслей» приходил непустым (дефолт `omitted`).
    /// Шлём только когда reasoning включён. См. CLAUDE.md (CoT для Claude).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AntThinking>,
    /// Глубина рассуждений (`output_config.effort`, GA). Шлём только при thinking и
    /// заданном `reasoning_effort` (кроме `none`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<AntOutputConfig>,
}

/// Конфиг extended thinking Anthropic. `type` всегда `adaptive` (единственный
/// «вкл»-режим у моделей 4.6+); `display` — `summarized` для видимого CoT.
#[derive(Debug, Serialize)]
pub struct AntThinking {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub display: &'static str,
}

/// `output_config` Anthropic: уровень усилия (`low`/`medium`/`high`).
#[derive(Debug, Serialize)]
pub struct AntOutputConfig {
    pub effort: &'static str,
}

#[derive(Debug, Serialize)]
pub struct AntMessage {
    pub role: &'static str,
    pub content: Vec<AntBlock>,
}

/// Блок содержимого сообщения. Сериализуется с внутренним тегом `type`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntBlock {
    Text {
        text: String,
    },
    /// Блок рассуждений (extended thinking) для переотправки. Должен идти **первым**
    /// в assistant-ходе с `tool_use`; подпись обязательна (иначе `400`). См.
    /// [`ThinkingBlock`](crate::shared::api::contract::ThinkingBlock).
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
pub struct AntTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Строит тело запроса Anthropic из доменного [`ChatRequest`]. `model` обязателен.
/// Из семплинга шлём **только `max_tokens`** (обязательное поле): новейшие модели
/// Claude (4.x) «зафиксировали» сэмплинг и отвергают `temperature`/`top_p`/`top_k`
/// как deprecated (HTTP 400), поэтому эти поля не отправляются вовсе. UI помечает их
/// как неподдержанные у Claude. См. ADR 0004 и CLAUDE.md (Фаза 2).
pub fn build_request(req: &ChatRequest, model: &str, stream: bool) -> AntRequest {
    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| AntTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                })
                .collect(),
        )
    };
    // Extended thinking включаем по флагу `thinking` сэмплинга. Бюджет/`reasoning_budget`
    // не используем — модели 4.x отвергают `budget_tokens`; глубину задаёт `effort`.
    let thinking = (req.sampling.thinking == Some(true)).then_some(AntThinking {
        kind: "adaptive",
        display: "summarized",
    });
    let output_config = thinking.as_ref().and_then(|_| {
        req.sampling
            .reasoning_effort
            .and_then(ant_effort)
            .map(|effort| AntOutputConfig { effort })
    });
    AntRequest {
        model: model.to_string(),
        max_tokens: req
            .sampling
            .max_tokens
            .map(|m| m as u64)
            .unwrap_or(DEFAULT_MAX_TOKENS),
        system: req.system.clone(),
        messages: build_messages(req),
        stream,
        tools,
        thinking,
        output_config,
    }
}

/// Уровень усилия → значение `output_config.effort` Anthropic (`low`/`medium`/`high`).
/// `None` (в т.ч. `ReasoningEffort::None`) — поле не отправляется.
fn ant_effort(e: ReasoningEffort) -> Option<&'static str> {
    match e {
        ReasoningEffort::None => None,
        // Anthropic понимает только low/medium/high — крайние ступени OpenAI (minimal/
        // xhigh) приводим к ближайшей поддержанной.
        ReasoningEffort::Minimal | ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        ReasoningEffort::High | ReasoningEffort::XHigh => Some("high"),
    }
}

/// Транслирует историю в сообщения Anthropic, склеивая соседние одной роли.
fn build_messages(req: &ChatRequest) -> Vec<AntMessage> {
    let mut out: Vec<AntMessage> = Vec::new();
    for m in &req.messages {
        let (role, blocks): (&'static str, Vec<AntBlock>) = match m.role {
            ApiRole::User => ("user", text_blocks(&m.content)),
            ApiRole::Assistant => {
                let mut blocks = Vec::new();
                // Thinking-блок (с подписью) обязан идти ПЕРВЫМ в assistant-ходе с
                // tool_use — иначе Anthropic вернёт 400. Ставится только в текущем
                // ходе agentic-loop (см. ApiMessage::with_thinking).
                if let Some(tb) = &m.thinking {
                    blocks.push(AntBlock::Thinking {
                        thinking: tb.text.clone(),
                        signature: tb.signature.clone(),
                    });
                }
                blocks.extend(text_blocks(&m.content));
                for tc in &m.tool_calls {
                    // Аргументы у нас — JSON-строка; Anthropic СТРОГО требует, чтобы
                    // `input` был ОБЪЕКТОМ. Безаргументный вызов мог сохраниться в
                    // истории как `null` (старые чаты), и `"null"`/массив/скаляр
                    // распарсились бы в не-объект → Anthropic 400 («tool_use.input:
                    // Input should be an object»). Любой не-объект приводим к `{}`.
                    let input = serde_json::from_str::<Value>(&tc.arguments)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| serde_json::json!({}));
                    blocks.push(AntBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input,
                    });
                }
                ("assistant", blocks)
            }
            // Результат инструмента → блок tool_result внутри user-сообщения.
            ApiRole::Tool => (
                "user",
                vec![AntBlock::ToolResult {
                    tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                    content: m.content.clone(),
                }],
            ),
            // Системное сообщение идёт top-level полем `system`, не в messages.
            ApiRole::System => continue,
        };
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == role => last.content.extend(blocks),
            _ => out.push(AntMessage {
                role,
                content: blocks,
            }),
        }
    }
    out
}

fn text_blocks(content: &str) -> Vec<AntBlock> {
    if content.is_empty() {
        Vec::new()
    } else {
        vec![AntBlock::Text {
            text: content.to_string(),
        }]
    }
}

// ---------- стриминговые события ----------

/// Событие SSE-стрима Anthropic (тег — поле `type` в `data`). Неинтересные события
/// (`ping`, `content_block_stop`, `message_stop`, `error`) попадают в [`AntStreamEvent::Other`].
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntStreamEvent {
    MessageStart {
        message: AntStartMessage,
    },
    ContentBlockStart {
        index: usize,
        content_block: AntStartBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: AntDelta,
    },
    MessageDelta {
        delta: AntStopDelta,
        #[serde(default)]
        usage: Option<AntUsage>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct AntStartMessage {
    #[serde(default)]
    pub usage: Option<AntUsage>,
}

/// Начало блока содержимого. Интересует только `tool_use` (даёт id+имя инструмента).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntStartBlock {
    ToolUse {
        id: String,
        name: String,
    },
    #[serde(other)]
    Other,
}

/// Дельта блока содержимого.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    /// Подпись блока «мыслей» (приходит в конце thinking-блока). Нужна для
    /// переотправки thinking при tool-use в том же ходе.
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
pub struct AntStopDelta {
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AntUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ThinkingBlock, ToolSchema};

    fn req(messages: Vec<ApiMessage>) -> ChatRequest {
        ChatRequest {
            system: Some("Ты — ассистент.".into()),
            messages,
            sampling: SamplingConfig {
                temperature: Some(0.7),
                top_k: Some(40),
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        }
    }

    #[test]
    fn system_is_top_level_and_sampling_mapped() {
        let body = build_request(&req(vec![ApiMessage::user("привет")]), "claude-x", true);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "claude-x");
        assert_eq!(json["system"], "Ты — ассистент.");
        assert_eq!(json["max_tokens"], 256);
        // temperature/top_p/top_k Claude 4.x не принимает — их не шлём вовсе.
        assert!(json.get("temperature").is_none());
        assert!(json.get("top_p").is_none());
        assert!(json.get("top_k").is_none());
        // Сообщение — user с текстовым блоком.
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"][0]["type"], "text");
        assert_eq!(json["messages"][0]["content"][0]["text"], "привет");
    }

    #[test]
    fn max_tokens_defaults_when_absent() {
        let mut r = req(vec![ApiMessage::user("hi")]);
        r.sampling.max_tokens = None;
        let json = serde_json::to_value(build_request(&r, "claude-x", false)).unwrap();
        assert_eq!(json["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn tool_calls_become_tool_use_and_results_merge_into_user() {
        // assistant(tool_use) → tool → tool: два результата склеиваются в один user.
        let r = req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "t1".into(),
                    name: "calc".into(),
                    arguments: "{\"x\":1}".into(),
                }],
            ),
            ApiMessage::tool("t1", "2"),
            ApiMessage::tool("t1", "доп"),
        ]);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        // [0] user, [1] assistant(tool_use), [2] user(2× tool_result).
        assert_eq!(json["messages"][1]["role"], "assistant");
        assert_eq!(json["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(json["messages"][1]["content"][0]["name"], "calc");
        assert_eq!(json["messages"][1]["content"][0]["input"]["x"], 1);
        assert_eq!(json["messages"][2]["role"], "user");
        assert_eq!(json["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(json["messages"][2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(json["messages"][2]["content"][1]["content"], "доп");
    }

    #[test]
    fn tool_use_input_coerced_to_object_when_not_object() {
        // Безаргументный вызов в истории мог сохраниться как `null` → строка "null".
        // Anthropic строго требует объект — приводим к `{}` (иначе 400).
        for raw in ["null", "", "[1,2]", "42", "\"x\""] {
            let r = req(vec![
                ApiMessage::user("введи"),
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        thought_signature: None,
                        id: "t1".into(),
                        name: "get_sampling".into(),
                        arguments: raw.into(),
                    }],
                ),
            ]);
            let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
            let input = &json["messages"][1]["content"][0]["input"];
            assert!(
                input.is_object(),
                "input должен быть объектом для arguments={raw:?}, получили {input}"
            );
        }
    }

    #[test]
    fn tools_use_input_schema() {
        let mut r = req(vec![ApiMessage::user("hi")]);
        r.tools = vec![ToolSchema {
            name: "calc".into(),
            description: "Считает".into(),
            parameters: serde_json::json!({"type":"object"}),
        }];
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["tools"][0]["name"], "calc");
        assert_eq!(json["tools"][0]["input_schema"]["type"], "object");
        assert!(json["tools"][0].get("parameters").is_none());
    }

    #[test]
    fn parses_stream_events() {
        let start =
            r#"{"type":"message_start","message":{"usage":{"input_tokens":25,"output_tokens":1}}}"#;
        match serde_json::from_str::<AntStreamEvent>(start).unwrap() {
            AntStreamEvent::MessageStart { message } => {
                assert_eq!(message.usage.unwrap().input_tokens, 25)
            }
            other => panic!("ожидался MessageStart, получили {other:?}"),
        }
        let td =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        match serde_json::from_str::<AntStreamEvent>(td).unwrap() {
            AntStreamEvent::ContentBlockDelta {
                delta: AntDelta::TextDelta { text },
                ..
            } => assert_eq!(text, "hi"),
            other => panic!("ожидался TextDelta, получили {other:?}"),
        }
        let tu = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu1","name":"calc","input":{}}}"#;
        match serde_json::from_str::<AntStreamEvent>(tu).unwrap() {
            AntStreamEvent::ContentBlockStart {
                index,
                content_block: AntStartBlock::ToolUse { id, name },
            } => {
                assert_eq!(index, 1);
                assert_eq!(id, "tu1");
                assert_eq!(name, "calc");
            }
            other => panic!("ожидался ToolUse start, получили {other:?}"),
        }
        let md = r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}"#;
        match serde_json::from_str::<AntStreamEvent>(md).unwrap() {
            AntStreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("tool_use"));
                assert_eq!(usage.unwrap().output_tokens, 15);
            }
            other => panic!("ожидался MessageDelta, получили {other:?}"),
        }
        // ping/прочее — Other (не падаем).
        assert!(matches!(
            serde_json::from_str::<AntStreamEvent>(r#"{"type":"ping"}"#).unwrap(),
            AntStreamEvent::Other
        ));
    }

    #[test]
    fn thinking_off_by_default() {
        // Дефолтный сэмплинг (thinking=None) — поле thinking не отправляется.
        let json = serde_json::to_value(build_request(
            &req(vec![ApiMessage::user("привет")]),
            "claude-x",
            true,
        ))
        .unwrap();
        assert!(json.get("thinking").is_none());
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn thinking_enabled_sends_adaptive_with_effort() {
        // thinking=true → adaptive+summarized; reasoning_effort → output_config.effort.
        // budget_tokens (reasoning_budget) Claude 4.x отвергает — НЕ шлём.
        let mut r = req(vec![ApiMessage::user("посчитай")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_effort = Some(ReasoningEffort::High);
        r.sampling.reasoning_budget = Some(0);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["thinking"]["type"], "adaptive");
        assert_eq!(json["thinking"]["display"], "summarized");
        assert_eq!(json["output_config"]["effort"], "high");
        // reasoning_budget наружу не уходит (нет ключа budget_tokens).
        assert!(json.get("budget_tokens").is_none());
        assert!(json["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn thinking_effort_none_omits_output_config() {
        // thinking включён, но effort не задан → output_config не шлётся.
        let mut r = req(vec![ApiMessage::user("hi")]);
        r.sampling.thinking = Some(true);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["thinking"]["type"], "adaptive");
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn thinking_block_prepended_to_assistant_tool_use() {
        // assistant-ход с tool_use + thinking-блоком: первым идёт thinking (с подписью),
        // затем tool_use. Anthropic требует именно такой порядок (иначе 400).
        let r = req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "t1".into(),
                    name: "calc".into(),
                    arguments: "{\"x\":1}".into(),
                }],
            )
            .with_thinking(Some(ThinkingBlock {
                text: "надо сложить".into(),
                signature: "sig-abc".into(),
                id: None,
            })),
            ApiMessage::tool("t1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        let blocks = &json["messages"][1]["content"];
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "надо сложить");
        assert_eq!(blocks[0]["signature"], "sig-abc");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["name"], "calc");
    }

    #[test]
    fn assistant_without_thinking_has_no_thinking_block() {
        // Без thinking-блока (обычный/исторический ход) — thinking-блока в выводе нет.
        let r = req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "t1".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            ),
            ApiMessage::tool("t1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["messages"][1]["content"][0]["type"], "tool_use");
    }

    #[test]
    fn parses_signature_delta() {
        let sd = r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-xyz"}}"#;
        match serde_json::from_str::<AntStreamEvent>(sd).unwrap() {
            AntStreamEvent::ContentBlockDelta {
                delta: AntDelta::SignatureDelta { signature },
                ..
            } => assert_eq!(signature, "sig-xyz"),
            other => panic!("ожидался SignatureDelta, получили {other:?}"),
        }
    }
}
