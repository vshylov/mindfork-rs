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
use crate::entities::sampling::{SamplingConfig, supported_sampling_fields};
use crate::shared::config::CloudProvider;

use super::{ChatEffect, Tool, ToolContext, ToolOutcome};

/// Имя инструмента чтения семплинга.
pub const GET_SAMPLING_ID: &str = "get_sampling";
/// Имя инструмента изменения семплинга.
pub const SET_SAMPLING_ID: &str = "set_sampling";

/// `get_sampling` — возвращает действующий семплинг (JSON), ограниченный полями,
/// доступными в текущем режиме движка (`provider`). См. [`supported_sampling_fields`].
pub struct GetSampling {
    /// Облачный провайдер chat-движка (`None` — локальный/external: доступны все поля).
    provider: Option<CloudProvider>,
}

impl GetSampling {
    pub fn new(provider: Option<CloudProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl Tool for GetSampling {
    fn id(&self) -> ToolId {
        GET_SAMPLING_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Introspection
    }
    fn ui_label(&self) -> &'static str {
        "показать семплинг"
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        format!(
            "Вернуть текущие параметры семплинга. {}",
            scope_note(self.provider)
        )
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        empty_object()
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        // Показываем только поля, доступные в текущем режиме (остальные движок
        // всё равно не принял бы), чтобы модель не пыталась их менять.
        let filtered = filter_to_supported(&ctx.effective_sampling, self.provider)?;
        Ok(ToolOutcome::text(serde_json::to_string(&filtered)?))
    }
}

/// `set_sampling` — переопределяет семплинг чата (частично; со следующего хода).
/// Доступные поля ограничены текущим режимом движка.
pub struct SetSampling {
    /// Облачный провайдер chat-движка (`None` — локальный/external: доступны все поля).
    provider: Option<CloudProvider>,
}

impl SetSampling {
    pub fn new(provider: Option<CloudProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl Tool for SetSampling {
    fn id(&self) -> ToolId {
        SET_SAMPLING_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Introspection
    }
    fn ui_label(&self) -> &'static str {
        "изменить семплинг"
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        format!(
            "Изменить параметры семплинга чата. Указанные поля переопределяют текущие; \
             применяется со следующего ответа. {}",
            scope_note(self.provider)
        )
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        // Схема несёт только поля, принимаемые движком текущего режима.
        let supported = supported_sampling_fields(self.provider);
        let mut props = serde_json::Map::new();
        for (name, schema) in field_schemas() {
            if supported.contains(&name) {
                props.insert(name.to_string(), schema);
            }
        }
        serde_json::json!({ "type": "object", "properties": props })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let supported = supported_sampling_fields(self.provider);
        // Отбрасываем поля, недоступные в текущем режиме (движок их не примет),
        // и сообщаем об этом модели, а не молча применяем неподдержанное.
        let mut obj = match args {
            serde_json::Value::Object(map) => map,
            serde_json::Value::Null => serde_json::Map::new(),
            other => {
                anyhow::bail!("неверные аргументы set_sampling: ожидался объект, получено {other}")
            }
        };
        let mut dropped: Vec<String> = obj
            .keys()
            .filter(|k| !supported.contains(&k.as_str()))
            .cloned()
            .collect();
        dropped.sort();
        obj.retain(|k, _| supported.contains(&k.as_str()));

        // Разбираем частичный конфиг (все поля Option, отсутствующие = None).
        let patch: SamplingConfig = serde_json::from_value(serde_json::Value::Object(obj))
            .map_err(|e| anyhow::anyhow!("неверные аргументы set_sampling: {e}"))?;
        let merged = merge_sampling(&ctx.effective_sampling, &patch);
        // В результат кладём только поля, доступные в текущем режиме — иначе в ленту
        // (и модели) уезжает полный конфиг с десятками `null`, сбивая с толку.
        let json = serde_json::to_string(&filter_to_supported(&merged, self.provider)?)?;
        let mut result = format!("Семплинг обновлён: {json}");
        if !dropped.is_empty() {
            result.push_str(&format!(
                ". Проигнорированы недоступные в текущем режиме поля: {}",
                dropped.join(", ")
            ));
        }
        Ok(ToolOutcome::with_effects(
            result,
            vec![ChatEffect::SetSamplingOverride(Box::new(merged))],
        ))
    }
}

/// Подсказка модели о доступном наборе полей в текущем режиме.
fn scope_note(provider: Option<CloudProvider>) -> String {
    match provider {
        None => "Доступны все параметры (temperature, top_k, min_p и т.д.).".into(),
        Some(_) => format!(
            "В текущем режиме доступны только: {}.",
            supported_sampling_fields(provider).join(", ")
        ),
    }
}

/// Сериализует семплинг, оставляя только поля, доступные в текущем режиме.
fn filter_to_supported(
    sampling: &SamplingConfig,
    provider: Option<CloudProvider>,
) -> Result<serde_json::Value> {
    let supported = supported_sampling_fields(provider);
    let value = serde_json::to_value(sampling)?;
    let serde_json::Value::Object(mut map) = value else {
        return Ok(value);
    };
    map.retain(|k, _| supported.contains(&k.as_str()));
    Ok(serde_json::Value::Object(map))
}

/// JSON-схемы значений всех настраиваемых полей семплинга (имя → схема). Порядок
/// совпадает с [`crate::entities::sampling::SETTABLE_SAMPLING_FIELDS`].
fn field_schemas() -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::json;
    let number = || json!({"type": "number"});
    let integer = || json!({"type": "integer"});
    let string_list = || json!({"type": "array", "items": {"type": "string"}});
    vec![
        ("temperature", number()),
        ("dynatemp_range", number()),
        ("dynatemp_exponent", number()),
        ("top_k", integer()),
        ("top_p", number()),
        ("min_p", number()),
        ("top_n_sigma", number()),
        ("typical_p", number()),
        ("adaptive_target", number()),
        ("adaptive_decay", number()),
        ("frequency_penalty", number()),
        ("presence_penalty", number()),
        ("repeat_penalty", number()),
        ("repeat_last_n", integer()),
        ("dry_multiplier", number()),
        ("dry_base", number()),
        ("dry_allowed_length", integer()),
        ("dry_penalty_last_n", integer()),
        ("dry_sequence_breakers", string_list()),
        ("xtc_probability", number()),
        ("xtc_threshold", number()),
        ("mirostat", integer()),
        ("mirostat_tau", number()),
        ("mirostat_eta", number()),
        ("max_tokens", integer()),
        ("seed", integer()),
        ("samplers", string_list()),
        ("thinking", json!({"type": "boolean"})),
        (
            "reasoning_effort",
            json!({"type": "string", "enum": ["none", "minimal", "low", "medium", "high", "xhigh"]}),
        ),
        (
            "verbosity",
            json!({"type": "string", "enum": ["low", "medium", "high"]}),
        ),
    ]
}

/// `get_system_message` — возвращает текущее системное сообщение чата.
pub struct GetSystemMessage;

#[async_trait::async_trait]
impl Tool for GetSystemMessage {
    fn id(&self) -> ToolId {
        "get_system_message".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Introspection
    }
    fn ui_label(&self) -> &'static str {
        "показать сис. сообщение"
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Вернуть текущее системное сообщение чата.".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
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
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Introspection
    }
    fn ui_label(&self) -> &'static str {
        "изменить сис. сообщение"
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Изменить системное сообщение чата. Применяется со следующего ответа.".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
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
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Introspection
    }
    fn ui_label(&self) -> &'static str {
        "время посл. сообщения"
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Вернуть время последнего сообщения пользователя (ISO 8601) и сколько прошло.".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
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
        dynatemp_range: patch.dynatemp_range.or(base.dynatemp_range),
        dynatemp_exponent: patch.dynatemp_exponent.or(base.dynatemp_exponent),
        top_k: patch.top_k.or(base.top_k),
        top_p: patch.top_p.or(base.top_p),
        min_p: patch.min_p.or(base.min_p),
        top_n_sigma: patch.top_n_sigma.or(base.top_n_sigma),
        typical_p: patch.typical_p.or(base.typical_p),
        adaptive_target: patch.adaptive_target.or(base.adaptive_target),
        adaptive_decay: patch.adaptive_decay.or(base.adaptive_decay),
        frequency_penalty: patch.frequency_penalty.or(base.frequency_penalty),
        presence_penalty: patch.presence_penalty.or(base.presence_penalty),
        repeat_penalty: patch.repeat_penalty.or(base.repeat_penalty),
        repeat_last_n: patch.repeat_last_n.or(base.repeat_last_n),
        dry_multiplier: patch.dry_multiplier.or(base.dry_multiplier),
        dry_base: patch.dry_base.or(base.dry_base),
        dry_allowed_length: patch.dry_allowed_length.or(base.dry_allowed_length),
        dry_penalty_last_n: patch.dry_penalty_last_n.or(base.dry_penalty_last_n),
        dry_sequence_breakers: patch
            .dry_sequence_breakers
            .clone()
            .or_else(|| base.dry_sequence_breakers.clone()),
        xtc_probability: patch.xtc_probability.or(base.xtc_probability),
        xtc_threshold: patch.xtc_threshold.or(base.xtc_threshold),
        mirostat: patch.mirostat.or(base.mirostat),
        mirostat_tau: patch.mirostat_tau.or(base.mirostat_tau),
        mirostat_eta: patch.mirostat_eta.or(base.mirostat_eta),
        max_tokens: patch.max_tokens.or(base.max_tokens),
        seed: patch.seed.or(base.seed),
        samplers: patch.samplers.clone().or_else(|| base.samplers.clone()),
        thinking: patch.thinking.or(base.thinking),
        reasoning_effort: patch.reasoning_effort.or(base.reasoning_effort),
        reasoning_budget: patch.reasoning_budget.or(base.reasoning_budget),
        verbosity: patch.verbosity.or(base.verbosity),
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
        let out = GetSampling::new(None)
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
        let out = SetSampling::new(None)
            .invoke(
                &ctx,
                serde_json::json!({
                    "temperature": 0.9,
                    "reasoning_effort": "high",
                    "min_p": 0.03,
                    "dry_multiplier": 0.8,
                    "dynatemp_range": 0.4,
                    "samplers": ["penalties", "temperature"],
                    "seed": -1
                }),
            )
            .await
            .unwrap();
        match &out.effects[..] {
            [ChatEffect::SetSamplingOverride(s)] => {
                assert_eq!(s.temperature, Some(0.9)); // переопределено
                assert_eq!(s.max_tokens, Some(512)); // сохранено из базы
                assert_eq!(s.reasoning_effort, Some(ReasoningEffort::High));
                // Новые расширения llama.cpp тоже мёржатся.
                assert_eq!(s.min_p, Some(0.03));
                assert_eq!(s.dry_multiplier, Some(0.8));
                assert_eq!(s.dynatemp_range, Some(0.4));
                assert_eq!(
                    s.samplers.as_deref(),
                    Some(&["penalties".to_string(), "temperature".to_string()][..])
                );
                assert_eq!(s.seed, Some(-1));
            }
            other => panic!("ожидался SetSamplingOverride, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_sampling_cloud_hides_unsupported_fields() {
        let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
        ctx.effective_sampling = SamplingConfig {
            temperature: Some(0.7),
            top_k: Some(40),
            min_p: Some(0.05),
            max_tokens: Some(256),
            ..Default::default()
        };
        // Gemini (нативный): top_k доступен, а min_p (расширение llama.cpp) — нет.
        let out = GetSampling::new(Some(CloudProvider::Gemini))
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.result).unwrap();
        assert!(v.get("temperature").is_some());
        assert!(v.get("max_tokens").is_some());
        assert!(v.get("top_k").is_some());
        assert!(v.get("min_p").is_none());

        // OpenAI: недоступна и temperature (GPT 5.5/5.6 её отвергают).
        let out = GetSampling::new(Some(CloudProvider::OpenAi))
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.result).unwrap();
        assert!(v.get("max_tokens").is_some());
        assert!(v.get("temperature").is_none());
        assert!(v.get("top_k").is_none());
    }

    #[tokio::test]
    async fn set_sampling_cloud_drops_unsupported_fields() {
        let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
        ctx.effective_sampling = SamplingConfig::default();
        // Claude: доступен только max_tokens; temperature/top_k должны быть отброшены.
        let out = SetSampling::new(Some(CloudProvider::Claude))
            .invoke(
                &ctx,
                serde_json::json!({"max_tokens": 1024, "temperature": 0.9, "top_k": 40}),
            )
            .await
            .unwrap();
        match &out.effects[..] {
            [ChatEffect::SetSamplingOverride(s)] => {
                assert_eq!(s.max_tokens, Some(1024));
                assert_eq!(s.temperature, None);
                assert_eq!(s.top_k, None);
            }
            other => panic!("ожидался SetSamplingOverride, got {other:?}"),
        }
        // Об отброшенных полях модель уведомляется.
        assert!(out.result.contains("temperature"));
        assert!(out.result.contains("top_k"));
        // Но в JSON результата нет полного дампа конфига с десятками `null`-полей —
        // только доступные в режиме (для Claude это max_tokens + reasoning).
        assert!(!out.result.contains("dynatemp_range"));
        assert!(!out.result.contains("reasoning_budget"));
        assert!(out.result.contains("max_tokens"));
    }

    #[test]
    fn set_sampling_schema_reflects_mode() {
        // Локально — полная схема (все настраиваемые поля).
        let local = SetSampling::new(None)
            .parameters(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru));
        let local_props = local["properties"].as_object().unwrap();
        assert_eq!(
            local_props.len(),
            crate::entities::sampling::SETTABLE_SAMPLING_FIELDS.len()
        );
        assert!(local_props.contains_key("top_k"));
        // Claude — max_tokens + reasoning (thinking/reasoning_effort), но не расширения.
        let claude = SetSampling::new(Some(CloudProvider::Claude))
            .parameters(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru));
        let claude_props = claude["properties"].as_object().unwrap();
        assert!(claude_props.contains_key("max_tokens"));
        assert!(claude_props.contains_key("thinking"));
        assert!(claude_props.contains_key("reasoning_effort"));
        assert!(!claude_props.contains_key("top_k"));
    }

    #[test]
    fn field_schemas_cover_all_settable_fields() {
        let names: Vec<&str> = field_schemas().into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names.as_slice(),
            crate::entities::sampling::SETTABLE_SAMPLING_FIELDS
        );
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
