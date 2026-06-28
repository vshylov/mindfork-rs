//! Инструменты «модели себя» (SelfModel) — MVP-зонд: чтение, рефлексия и
//! минимальное обновление представления агента о себе, целях и собеседнике.
//! Данные пер-профильные, в SQLite (как заметки) — инструменты-мутаторы пишут
//! **напрямую** через `ctx.storage` (не через `ChatEffect`). Изоляция по
//! `ctx.profile_id`. См. [docs/self-model-mvp.md](../../../docs/self-model-mvp.md).

use anyhow::Result;
use uuid::Uuid;

use crate::entities::profile::ToolId;
use crate::entities::self_model::{DEFAULT_PROMPT_CAP, GoalStatus, SelfModel};

use super::{Tool, ToolContext, ToolOutcome};

pub const GET_SELF_MODEL_ID: &str = "get_self_model";
pub const REFLECT_ID: &str = "reflect";
pub const UPDATE_SELF_MODEL_ID: &str = "update_self_model";
pub const UPDATE_USER_MODEL_ID: &str = "update_user_model";
pub const ADD_INSIGHT_ID: &str = "add_insight";

/// Загружает модель профиля из хранилища (или пустую, если ещё не создавалась).
/// Читаем из БД, а не из снимка `ctx.self_model`, чтобы видеть правки, сделанные
/// другими SelfModel-инструментами в этом же ходе.
fn load(ctx: &ToolContext) -> Result<SelfModel> {
    Ok(ctx
        .storage
        .db()
        .self_model_get(ctx.profile_id)?
        .unwrap_or_else(|| SelfModel::new(ctx.profile_id)))
}

/// Извлекает массив строк по ключу (пустой, если нет/не массив).
fn str_array(args: &serde_json::Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Рендер модели для результата инструмента (или явная пометка пустоты).
fn render_or_empty(m: &SelfModel) -> String {
    m.render_for_prompt(DEFAULT_PROMPT_CAP)
        .unwrap_or_else(|| "(модель себя пока пуста)".to_string())
}

/// `get_self_model` — текущее состояние модели себя (чтение).
pub struct GetSelfModel;

#[async_trait::async_trait]
impl Tool for GetSelfModel {
    fn id(&self) -> ToolId {
        GET_SELF_MODEL_ID.into()
    }
    fn description(&self) -> String {
        "Прочитать твою текущую «модель себя»: краткое описание себя, активные цели \
         и представление о собеседнике."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let m = load(ctx)?;
        Ok(ToolOutcome::text(render_or_empty(&m)))
    }
}

/// `reflect` — возвращает текущую модель и рубрику для размышления. Ничего не
/// пишет: это «точка входа» рефлексии, после которой модель сама зовёт
/// `update_self_model`/`update_user_model`, если есть что зафиксировать.
pub struct Reflect;

#[async_trait::async_trait]
impl Tool for Reflect {
    fn id(&self) -> ToolId {
        REFLECT_ID.into()
    }
    fn description(&self) -> String {
        "Поразмышлять над недавним разговором: получить текущую «модель себя» и \
         вопросы для саморефлексии. Если по итогам что-то изменилось — обнови \
         модель через update_self_model / update_user_model."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let m = load(ctx)?;
        let out = format!(
            "Текущая модель себя:\n{}\n\nВопросы для размышления:\n\
             - Что нового я понял(а) о себе в этом разговоре?\n\
             - Изменились ли мои цели — есть новые, выполненные или неактуальные?\n\
             - Что я узнал(а) о собеседнике (черты, интересы, динамика отношений)?\n\
             - Заметил(а) ли я противоречие/напряжение в себе или разговоре?\n\
             Если есть что зафиксировать — вызови update_self_model, update_user_model \
             и/или add_insight (для наблюдений и противоречий прозой).",
            render_or_empty(&m)
        );
        Ok(ToolOutcome::text(out))
    }
}

/// `add_insight` — добавляет короткое наблюдение/инсайт в нарратив (включая
/// замеченные противоречия — прозой, без отдельного типа). Пишет напрямую в БД.
pub struct AddInsight;

#[async_trait::async_trait]
impl Tool for AddInsight {
    fn id(&self) -> ToolId {
        ADD_INSIGHT_ID.into()
    }
    fn description(&self) -> String {
        "Записать короткое наблюдение/инсайт о себе, разговоре или собеседнике в свой \
         нарратив (историю «я во времени»). Сюда же — замеченные противоречия или \
         внутренние напряжения, простой прозой. Используй для того, что стоит \
         помнить со временем."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Короткое наблюдение/инсайт (1-2 предложения)"}
            },
            "required": ["text"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            anyhow::bail!("ожидается непустое поле text");
        }
        let mut m = load(ctx)?;
        m.add_insight(text);
        ctx.storage.db().self_model_upsert(&m)?;
        Ok(ToolOutcome::text("Наблюдение записано в нарратив."))
    }
}

/// `update_self_model` — правит описание себя и/или цели.
pub struct UpdateSelfModel;

#[async_trait::async_trait]
impl Tool for UpdateSelfModel {
    fn id(&self) -> ToolId {
        UPDATE_SELF_MODEL_ID.into()
    }
    fn description(&self) -> String {
        "Обновить «модель себя»: задать краткое описание себя (summary), добавить \
         новые цели (add_goals), отметить выполненные (complete_goals) или \
         неактуальные (abandon_goals по id из get_self_model)."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "Новое краткое описание себя (заменяет прежнее)"},
                "add_goals": {"type": "array", "items": {"type": "string"}, "description": "Новые цели"},
                "complete_goals": {"type": "array", "items": {"type": "string"}, "description": "id выполненных целей"},
                "abandon_goals": {"type": "array", "items": {"type": "string"}, "description": "id неактуальных целей"}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let mut m = load(ctx)?;
        let mut changed = false;

        if let Some(s) = args.get("summary").and_then(|v| v.as_str()) {
            m.summary = s.trim().to_string();
            changed = true;
        }
        for g in str_array(&args, "add_goals") {
            m.add_goal(g);
            changed = true;
        }
        for id in str_array(&args, "complete_goals") {
            if let Ok(uuid) = Uuid::parse_str(&id) {
                changed |= m.set_goal_status(uuid, GoalStatus::Completed);
            }
        }
        for id in str_array(&args, "abandon_goals") {
            if let Ok(uuid) = Uuid::parse_str(&id) {
                changed |= m.set_goal_status(uuid, GoalStatus::Abandoned);
            }
        }

        if !changed {
            return Ok(ToolOutcome::text(
                "Нечего обновлять (не передано ни одного изменения).",
            ));
        }
        ctx.storage.db().self_model_upsert(&m)?;
        Ok(ToolOutcome::text(format!(
            "Модель себя обновлена.\n{}",
            render_or_empty(&m)
        )))
    }
}

/// `update_user_model` — правит представление о собеседнике.
pub struct UpdateUserModel;

#[async_trait::async_trait]
impl Tool for UpdateUserModel {
    fn id(&self) -> ToolId {
        UPDATE_USER_MODEL_ID.into()
    }
    fn description(&self) -> String {
        "Обновить представление о собеседнике: воспринимаемые черты \
         (perceived_traits), текущие интересы (current_interests), динамику \
         отношений (relationship_dynamic). Списки заменяют прежние значения."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "perceived_traits": {"type": "array", "items": {"type": "string"}},
                "current_interests": {"type": "array", "items": {"type": "string"}},
                "relationship_dynamic": {"type": "string"}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let mut m = load(ctx)?;
        let mut changed = false;

        if args.get("perceived_traits").is_some() {
            m.user_model.perceived_traits = str_array(&args, "perceived_traits");
            changed = true;
        }
        if args.get("current_interests").is_some() {
            m.user_model.current_interests = str_array(&args, "current_interests");
            changed = true;
        }
        if let Some(s) = args.get("relationship_dynamic").and_then(|v| v.as_str()) {
            m.user_model.relationship_dynamic = s.trim().to_string();
            changed = true;
        }

        if !changed {
            return Ok(ToolOutcome::text(
                "Нечего обновлять (не передано ни одного поля).",
            ));
        }
        ctx.storage.db().self_model_upsert(&m)?;
        Ok(ToolOutcome::text(format!(
            "Модель собеседника обновлена.\n{}",
            render_or_empty(&m)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;

    #[tokio::test]
    async fn get_on_empty_reports_empty() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("пуста"));
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn update_summary_and_goal_persists() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);

        let out = UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"summary": "ценю ясность", "add_goals": ["помочь с проектом"]}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("обновлена"));

        // Записано в хранилище под этим профилем.
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.summary, "ценю ясность");
        assert_eq!(stored.active_goals().count(), 1);

        // get_self_model отражает запись.
        let got = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(got.result.contains("ценю ясность"));
        assert!(got.result.contains("помочь с проектом"));
    }

    #[tokio::test]
    async fn complete_goal_by_id() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["старая цель"]}))
            .await
            .unwrap();
        let id = storage.db().self_model_get(profile).unwrap().unwrap().goals[0].id;

        UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"complete_goals": [id.to_string()]}),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.active_goals().count(), 0);
    }

    #[tokio::test]
    async fn update_user_model_persists() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "perceived_traits": ["скептичный", "глубокий"],
                    "relationship_dynamic": "рабочие"
                }),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.user_model.perceived_traits.len(), 2);
        assert_eq!(stored.user_model.relationship_dynamic, "рабочие");
    }

    #[tokio::test]
    async fn update_with_nothing_is_noop() {
        let (_d, storage, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("Нечего обновлять"));
        // Ничего не записано.
        assert!(
            storage
                .db()
                .self_model_get(ctx.profile_id)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn reflect_returns_current_and_rubric() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = Reflect.invoke(&ctx, serde_json::json!({})).await.unwrap();
        assert!(out.result.contains("Вопросы для размышления"));
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn add_insight_persists_and_shows() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        AddInsight
            .invoke(
                &ctx,
                serde_json::json!({"text": "напряжение между краткостью и полнотой"}),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.narrative.len(), 1);

        let got = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(got.result.contains("Недавние наблюдения:"));
        assert!(got.result.contains("напряжение"));

        // Пустой text — ошибка, ничего не дописано.
        assert!(
            AddInsight
                .invoke(&ctx, serde_json::json!({"text": "  "}))
                .await
                .is_err()
        );
        assert_eq!(
            storage
                .db()
                .self_model_get(profile)
                .unwrap()
                .unwrap()
                .narrative
                .len(),
            1
        );
    }
}
