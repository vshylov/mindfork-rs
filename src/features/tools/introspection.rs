//! Инструменты интроспекции: чтение/изменение семплинга и системного сообщения,
//! время последнего user-сообщения. См. spec §9.3 (`get/set_sampling`,
//! `get/set_system_message`, `get_last_user_message_time`).
//!
//! Мутирующие (`set_*`) **не** трогают `Chat`, а возвращают [`ChatEffect`];
//! применяет их оркестратор (spec §4.4.2). Изменения вступают в силу со
//! следующего хода/построения запроса (spec §6.6).

use anyhow::Result;
use chrono::Utc;

use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;

use super::{ChatEffect, Tool, ToolContext, ToolOutcome};

/// `get_sampling` — возвращает действующий семплинг (JSON).
pub struct GetSampling;

#[async_trait::async_trait]
impl Tool for GetSampling {
    fn id(&self) -> ToolId {
        "get_sampling".into()
    }
    fn description(&self) -> String {
        "Вернуть текущие параметры семплинга (temperature, top_k и т.д.).".into()
    }
    fn parameters(&self) -> serde_json::Value {
        empty_object()
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let json = serde_json::to_string(&ctx.effective_sampling)?;
        Ok(ToolOutcome::text(json))
    }
}

/// `set_sampling` — переопределяет семплинг чата (частично; со следующего хода).
pub struct SetSampling;

#[async_trait::async_trait]
impl Tool for SetSampling {
    fn id(&self) -> ToolId {
        "set_sampling".into()
    }
    fn description(&self) -> String {
        "Изменить параметры семплинга чата. Указанные поля переопределяют текущие; \
         применяется со следующего ответа."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "temperature": {"type": "number"},
                "top_k": {"type": "integer"},
                "top_p": {"type": "number"},
                "frequency_penalty": {"type": "number"},
                "presence_penalty": {"type": "number"},
                "max_tokens": {"type": "integer"},
                "thinking": {"type": "boolean"},
                "reasoning_effort": {"type": "string", "enum": ["none", "low", "medium", "high"]}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Разбираем частичный конфиг (все поля Option, отсутствующие = None).
        let patch: SamplingConfig = serde_json::from_value(args)
            .map_err(|e| anyhow::anyhow!("неверные аргументы set_sampling: {e}"))?;
        let merged = merge_sampling(&ctx.effective_sampling, &patch);
        let json = serde_json::to_string(&merged)?;
        Ok(ToolOutcome::with_effects(
            format!("Семплинг обновлён: {json}"),
            vec![ChatEffect::SetSamplingOverride(merged)],
        ))
    }
}

/// `get_system_message` — возвращает текущее системное сообщение чата.
pub struct GetSystemMessage;

#[async_trait::async_trait]
impl Tool for GetSystemMessage {
    fn id(&self) -> ToolId {
        "get_system_message".into()
    }
    fn description(&self) -> String {
        "Вернуть текущее системное сообщение чата.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        empty_object()
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        Ok(ToolOutcome::text(ctx.system_message.clone()))
    }
}

/// `set_system_message` — меняет системное сообщение (со следующего построения запроса).
pub struct SetSystemMessage;

#[async_trait::async_trait]
impl Tool for SetSystemMessage {
    fn id(&self) -> ToolId {
        "set_system_message".into()
    }
    fn description(&self) -> String {
        "Изменить системное сообщение чата. Применяется со следующего ответа.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"system_message": {"type": "string"}},
            "required": ["system_message"]
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let msg = args
            .get("system_message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается строковое поле system_message"))?
            .to_string();
        Ok(ToolOutcome::with_effects(
            "Системное сообщение обновлено.",
            vec![ChatEffect::SetSystemMessage(msg)],
        ))
    }
}

/// `get_last_user_message_time` — таймстемп последнего user-сообщения + прошедшее время.
pub struct GetLastUserMessageTime;

#[async_trait::async_trait]
impl Tool for GetLastUserMessageTime {
    fn id(&self) -> ToolId {
        "get_last_user_message_time".into()
    }
    fn description(&self) -> String {
        "Вернуть время последнего сообщения пользователя (ISO 8601) и сколько прошло.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        empty_object()
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let result = match ctx.last_user_message_at {
            None => "Пользователь ещё не отправлял сообщений в этом чате.".to_string(),
            Some(ts) => {
                let elapsed = Utc::now().signed_duration_since(ts);
                let secs = elapsed.num_seconds().max(0);
                format!(
                    "Последнее сообщение пользователя: {} ({} назад).",
                    ts.to_rfc3339(),
                    humanize(secs)
                )
            }
        };
        Ok(ToolOutcome::text(result))
    }
}

/// Сливает частичный патч поверх базового семплинга (заданные поля побеждают).
fn merge_sampling(base: &SamplingConfig, patch: &SamplingConfig) -> SamplingConfig {
    SamplingConfig {
        temperature: patch.temperature.or(base.temperature),
        top_k: patch.top_k.or(base.top_k),
        top_p: patch.top_p.or(base.top_p),
        frequency_penalty: patch.frequency_penalty.or(base.frequency_penalty),
        presence_penalty: patch.presence_penalty.or(base.presence_penalty),
        max_tokens: patch.max_tokens.or(base.max_tokens),
        thinking: patch.thinking.or(base.thinking),
        reasoning_effort: patch.reasoning_effort.or(base.reasoning_effort),
    }
}

/// JSON Schema пустого объекта параметров (инструмент без аргументов).
fn empty_object() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

/// Грубое человекочитаемое представление длительности в секундах.
fn humanize(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} с")
    } else if secs < 3600 {
        format!("{} мин", secs / 60)
    } else if secs < 86400 {
        format!("{} ч", secs / 3600)
    } else {
        format!("{} дн", secs / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use crate::entities::sampling::ReasoningEffort;
    use uuid::Uuid;

    #[tokio::test]
    async fn get_sampling_returns_effective() {
        let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
        ctx.effective_sampling = SamplingConfig {
            temperature: Some(0.7),
            ..Default::default()
        };
        let out = GetSampling
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        let parsed: SamplingConfig = serde_json::from_str(&out.result).unwrap();
        assert_eq!(parsed.temperature, Some(0.7));
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn set_sampling_merges_and_emits_effect() {
        let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
        ctx.effective_sampling = SamplingConfig {
            temperature: Some(0.1),
            max_tokens: Some(512),
            ..Default::default()
        };
        let out = SetSampling
            .invoke(
                &ctx,
                serde_json::json!({"temperature": 0.9, "reasoning_effort": "high"}),
            )
            .await
            .unwrap();
        match &out.effects[..] {
            [ChatEffect::SetSamplingOverride(s)] => {
                assert_eq!(s.temperature, Some(0.9)); // переопределено
                assert_eq!(s.max_tokens, Some(512)); // сохранено из базы
                assert_eq!(s.reasoning_effort, Some(ReasoningEffort::High));
            }
            other => panic!("ожидался SetSamplingOverride, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_system_message_emits_effect() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = SetSystemMessage
            .invoke(&ctx, serde_json::json!({"system_message": "новое"}))
            .await
            .unwrap();
        assert_eq!(
            out.effects,
            vec![ChatEffect::SetSystemMessage("новое".into())]
        );
    }

    #[tokio::test]
    async fn get_system_message_returns_snapshot() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = GetSystemMessage
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(out.result, "системное сообщение");
    }

    #[tokio::test]
    async fn last_user_message_time_handles_none_and_some() {
        let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
        let none = GetLastUserMessageTime
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(none.result.contains("ещё не отправлял"));

        ctx.last_user_message_at = Some(Utc::now());
        let some = GetLastUserMessageTime
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(some.result.contains("Последнее сообщение"));
    }
}
