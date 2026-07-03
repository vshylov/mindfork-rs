//! Инструменты «модели себя» (SelfModel) — MVP-зонд: чтение, рефлексия и
//! минимальное обновление представления агента о себе, целях и собеседнике.
//! Данные пер-профильные, в SQLite (как заметки) — инструменты-мутаторы пишут
//! **напрямую** через `ctx.storage` (не через `ChatEffect`). Изоляция по
//! `ctx.profile_id`. См. [docs/self-model-mvp.md](../../../docs/self-model-mvp.md).

use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;

use crate::entities::profile::ToolId;
use crate::entities::self_model::{GoalMatch, GoalStatus, SelfModel};

use super::{Tool, ToolContext, ToolOutcome};

pub const GET_SELF_MODEL_ID: &str = "get_self_model";
pub const REFLECT_ID: &str = "reflect";
pub const UPDATE_SELF_MODEL_ID: &str = "update_self_model";
pub const UPDATE_USER_MODEL_ID: &str = "update_user_model";
pub const ADD_INSIGHT_ID: &str = "add_insight";
pub const CONSOLIDATE_NARRATIVE_ID: &str = "consolidate_narrative";

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

/// `get_self_model` — текущее состояние модели себя (чтение).
pub struct GetSelfModel;

#[async_trait::async_trait]
impl Tool for GetSelfModel {
    fn id(&self) -> ToolId {
        GET_SELF_MODEL_ID.into()
    }
    fn description(&self) -> String {
        "Прочитать твою текущую «модель себя» целиком: описание себя, цели (с #id для \
         отметки выполненных/неактуальных), представление о собеседнике и наблюдения."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let m = load(ctx)?;
        Ok(ToolOutcome::text(m.render_full(Utc::now())))
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
        // Приписка о заполненности нарратива (если близко к потолку) — рубрика
        // становится приборной панелью, а не плакатом.
        let fill = m
            .narrative_fill_hint(ctx.self_model_params.max_narrative)
            .map(|h| format!("\n{h}"))
            .unwrap_or_default();
        let out = format!(
            "Текущая модель себя:\n{}\n\nВопросы для размышления:\n\
             - Что устойчивого я понял(а) о себе? Уточни update_self_model.summary — \
             интегрируй прежнее с новым, не переписывай с нуля.\n\
             - Цели: пройди по активным по #id — какие выполнены (complete_goals) или \
             неактуальны (abandon_goals)? появились ли новые (add_goals)?\n\
             - Что устойчивого узнал(а) о собеседнике? update_user_model правит списки по \
             частям (add_/remove_), не перетирая. Мимолётное (настроение, разовая \
             реакция) — в add_insight, не в модель собеседника.\n\
             - Заметил(а) ли противоречие/напряжение? Запиши прозой через add_insight.\n\
             - Не раздулся ли нарратив (дубли, устаревшее)? Подними устойчивое в summary/\
             черты, а сырые/дублирующие наблюдения вычисти через consolidate_narrative \
             (убрать по #id, опц. добавить одно сводное).{fill}\n\
             Меняй только то, что действительно изменилось; если менять нечего — ничего \
             не вызывай.",
            m.render_full(Utc::now())
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
        // Атомарно (под мьютексом БД), чтобы параллельная авто-рефлексия/`F3` не
        // затёрла запись гонкой load-modify-save.
        let max = ctx.self_model_params.max_narrative;
        // Вытесненные за потолок сегменты — чтобы явно сообщить, что ушло (иначе
        // FIFO молча теряет старейшее).
        let mut evicted: Vec<String> = Vec::new();
        let (model, _) = ctx.storage.db().self_model_update(ctx.profile_id, |m| {
            evicted = m
                .add_insight(text, max)
                .into_iter()
                .map(|s| s.text)
                .collect();
            true
        })?;
        let mut msg = format!(
            "Наблюдение записано (нарратив {}/{max}).",
            model.narrative.len()
        );
        if !evicted.is_empty() {
            let list = evicted
                .iter()
                .map(|t| format!("«{t}»"))
                .collect::<Vec<_>>()
                .join(", ");
            msg.push_str(&format!(
                " Вытеснены старейшие: {list}. Если в них было устойчивое — подними в \
                 summary/черты или сведи сводным через consolidate_narrative."
            ));
        }
        Ok(ToolOutcome::text(msg))
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
        "Обновить «модель себя»: уточнить описание себя (summary — интегрируй прежнее с \
         новым, а не переписывай с нуля), добавить цели (add_goals), отметить \
         выполненные (complete_goals) или неактуальные (abandon_goals) — по #id или \
         полному id из get_self_model. Веди цели: закрывай достигнутые, не только \
         ставь новые."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "Уточнённое описание себя (интегрирует прежнее с изменившимся)"},
                "add_goals": {"type": "array", "items": {"type": "string"}, "description": "Новые цели"},
                "complete_goals": {"type": "array", "items": {"type": "string"}, "description": "#id (или полный id) выполненных целей"},
                "abandon_goals": {"type": "array", "items": {"type": "string"}, "description": "#id (или полный id) неактуальных целей"}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Нерезолвленные ручки целей собираются внутри атомарной правки (захват по
        // `&mut`), запись — под одним захватом мьютекса (защита от гонки).
        let mut unresolved: Vec<String> = Vec::new();
        let params = ctx.self_model_params;
        let (model, changed) = ctx.storage.db().self_model_update(ctx.profile_id, |m| {
            let mut changed = false;
            if let Some(s) = args.get("summary").and_then(|v| v.as_str()) {
                let s = s.trim().to_string();
                if m.summary != s {
                    m.summary = s;
                    changed = true;
                }
            }
            for g in str_array(&args, "add_goals") {
                m.add_goal(g);
                changed = true;
            }
            // Цели закрываются по #id/полному id — резолвим ручку среди целей модели.
            for h in str_array(&args, "complete_goals") {
                match m.match_goal(&h) {
                    GoalMatch::One(id) => changed |= m.set_goal_status(id, GoalStatus::Completed),
                    GoalMatch::None => unresolved.push(h),
                    GoalMatch::Ambiguous => unresolved.push(format!("{h} (неоднозначно)")),
                }
            }
            for h in str_array(&args, "abandon_goals") {
                match m.match_goal(&h) {
                    GoalMatch::One(id) => changed |= m.set_goal_status(id, GoalStatus::Abandoned),
                    GoalMatch::None => unresolved.push(h),
                    GoalMatch::Ambiguous => unresolved.push(format!("{h} (неоднозначно)")),
                }
            }
            // Свёртка старых закрытых целей в нарратив-шрам (потолок закрытых целей):
            // структура не растёт бесконечно, а «биография» сохраняется.
            if m.fold_closed_goals(params.max_closed_goals, params.max_narrative) > 0 {
                changed = true;
            }
            changed
        })?;

        if !changed {
            let mut msg = String::from("Нечего обновлять (не передано ни одного изменения).");
            if !unresolved.is_empty() {
                msg.push_str(&format!("\nНе найдены цели: {}.", unresolved.join(", ")));
            }
            return Ok(ToolOutcome::text(msg));
        }
        let mut msg = format!("Модель себя обновлена.\n{}", model.render_full(Utc::now()));
        if !unresolved.is_empty() {
            msg.push_str(&format!("\n(Не найдены цели: {}.)", unresolved.join(", ")));
        }
        Ok(ToolOutcome::text(msg))
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
        "Обновить устойчивую, интегрированную модель собеседника (через все разговоры, \
         не снимок текущего настроения). Списки правятся ПО ЧАСТЯМ и не перетираются: \
         add_traits/remove_traits (черты), add_interests/remove_interests (интересы); \
         relationship_dynamic — как вы относитесь во времени. При удалении/замене черты \
         передай note — что и почему изменилось (уйдёт в нарратив как след ревизии, чтобы \
         модель себя помнила, что менялась). Мимолётное (сегодняшнее настроение, разовая \
         реакция) записывай в add_insight, а не сюда."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "add_traits": {"type": "array", "items": {"type": "string"}, "description": "Добавить устойчивые черты (дедуп; прежние сохраняются)"},
                "remove_traits": {"type": "array", "items": {"type": "string"}, "description": "Убрать неверные/устаревшие черты"},
                "add_interests": {"type": "array", "items": {"type": "string"}, "description": "Добавить интересы (дедуп; прежние сохраняются)"},
                "remove_interests": {"type": "array", "items": {"type": "string"}, "description": "Убрать неактуальные интересы"},
                "relationship_dynamic": {"type": "string", "description": "Как вы относитесь во времени (заменяет прежнее)"},
                "note": {"type": "string", "description": "Что и почему изменилось (при удалении/замене черт) — уходит в нарратив как след ревизии"}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Признаки удаления/наличия note собираем внутри атомарной правки (захват по
        // `&mut`); запись — под одним захватом мьютекса (защита от гонки).
        let mut removed_traits = false;
        let mut removed_interests = false;
        let mut has_note = false;
        let max = ctx.self_model_params.max_narrative;
        let (model, changed) = ctx.storage.db().self_model_update(ctx.profile_id, |m| {
            let mut changed = false;

            // Списки — merge (add/remove с дедупом), а не замена: правка не обнуляет
            // накопленное представление (частая беда «перетирания по настроению»).
            changed |= m.user_model.add_traits(str_array(&args, "add_traits"));
            removed_traits = m
                .user_model
                .remove_traits(&str_array(&args, "remove_traits"));
            changed |= removed_traits;
            changed |= m
                .user_model
                .add_interests(str_array(&args, "add_interests"));
            removed_interests = m
                .user_model
                .remove_interests(&str_array(&args, "remove_interests"));
            changed |= removed_interests;
            if let Some(s) = args.get("relationship_dynamic").and_then(|v| v.as_str()) {
                let s = s.trim().to_string();
                if m.user_model.relationship_dynamic != s {
                    m.user_model.relationship_dynamic = s;
                    changed = true;
                }
            }
            // Шрам ревизии: `note` (что и почему изменилось) уходит в нарратив, так
            // изменение мнения о собеседнике оставляет след, а не стирается бесследно
            // (черты плоские — «биография» их изменений живёт в нарративе).
            let note = args
                .get("note")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            has_note = note.is_some();
            if let Some(n) = note {
                m.add_insight(n, max);
                changed = true;
            }
            changed
        })?;

        if !changed {
            return Ok(ToolOutcome::text(
                "Нечего обновлять (не передано ни одного изменения).",
            ));
        }
        let mut msg = format!(
            "Модель собеседника обновлена.\n{}",
            model.render_full(Utc::now())
        );
        // Удаление/замена черты — пересмотр суждения. Причина не записана → напоминаем
        // оставить след в нарративе (шрам), а не стирать молча.
        if (removed_traits || removed_interests) && !has_note {
            msg.push_str(
                "\n(Ты убрал(а) черты/интересы без пояснения. Если это пересмотр мнения — \
                 передай note с тем, что и почему изменилось: он останется в нарративе как \
                 след, чтобы модель себя помнила, что менялась.)",
            );
        }
        // Если note добавлен и нарратив близок к потолку — напомнить о консолидации.
        if has_note && let Some(h) = model.narrative_fill_hint(max) {
            msg.push_str(&format!("\n({h})"));
        }
        Ok(ToolOutcome::text(msg))
    }
}

/// `consolidate_narrative` — консолидация нарратива против раздувания: убирает
/// перечисленные наблюдения (по #id) и опционально добавляет одно сводное вместо
/// них. Лечит «дрейф» накопления (список наблюдений раздувается, дубли/устаревшее
/// топят сигнал, а FIFO-потолок тихо роняет старое без интеграции). Зеркало
/// `note_merge`/`consolidate_notes` в идиоме модели себя, без графа. Пишет в БД.
pub struct ConsolidateNarrative;

#[async_trait::async_trait]
impl Tool for ConsolidateNarrative {
    fn id(&self) -> ToolId {
        CONSOLIDATE_NARRATIVE_ID.into()
    }
    fn description(&self) -> String {
        "Консолидировать нарратив (список наблюдений) против раздувания: убрать \
         устаревшие/дублирующие наблюдения по #id (remove) и, если несколько сворачиваются \
         в один вывод, добавить его вместо них (add). Сначала подними устойчивое в summary/\
         черты (update_self_model/update_user_model), потом вычисти сырое здесь — это \
         интеграция, а не потеря."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "remove": {"type": "array", "items": {"type": "string"}, "description": "#id (или полные id) убираемых наблюдений (из get_self_model)"},
                "add": {"type": "string", "description": "Одно сводное наблюдение вместо убранных (опц.)"}
            },
            "required": ["remove"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Сводное наблюдение вместо убранных (опц.) — извлекаем до правки, чтобы
        // проверять `add.is_none()` и после закрытия closure.
        let add = args
            .get("add")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // Нерезолвленные ручки и счётчик удалённого собираем внутри атомарной правки.
        let mut unresolved: Vec<String> = Vec::new();
        let mut removed = 0usize;
        let max = ctx.self_model_params.max_narrative;
        let (model, _changed) = ctx.storage.db().self_model_update(ctx.profile_id, |m| {
            // Резолвим ручки убираемых наблюдений (#id/полный id) среди нарратива.
            let mut to_remove: Vec<Uuid> = Vec::new();
            for h in str_array(&args, "remove") {
                match m.match_insight(&h) {
                    GoalMatch::One(id) => to_remove.push(id),
                    GoalMatch::None => unresolved.push(h),
                    GoalMatch::Ambiguous => unresolved.push(format!("{h} (неоднозначно)")),
                }
            }
            removed = m.remove_insights(&to_remove);
            if let Some(a) = add.as_deref() {
                m.add_insight(a, max);
            }
            removed > 0 || add.is_some()
        })?;

        if removed == 0 && add.is_none() {
            let mut msg = String::from("Нечего консолидировать.");
            if !unresolved.is_empty() {
                msg.push_str(&format!(
                    "\nНе найдены наблюдения: {}.",
                    unresolved.join(", ")
                ));
            }
            return Ok(ToolOutcome::text(msg));
        }
        let mut msg = format!(
            "Нарратив консолидирован (убрано: {removed}{}).\n{}",
            if add.is_some() {
                ", добавлено сводное"
            } else {
                ""
            },
            model.render_full(Utc::now())
        );
        if !unresolved.is_empty() {
            msg.push_str(&format!(
                "\n(Не найдены наблюдения: {}.)",
                unresolved.join(", ")
            ));
        }
        Ok(ToolOutcome::text(msg))
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
                    "add_traits": ["скептичный", "глубокий"],
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
    async fn update_user_model_merges_not_overwrites() {
        // Ключевой фикс: правка не перетирает прежнее (беда «по настроению»).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["добрый"]}))
            .await
            .unwrap();
        // Вторая правка в другом «настроении» — добавляет, а не заменяет.
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["прямолинейный"]}))
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.user_model.perceived_traits.len(), 2);
        assert!(
            stored
                .user_model
                .perceived_traits
                .contains(&"добрый".to_string())
        );
        // remove_traits убирает точечно.
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"remove_traits": ["добрый"]}))
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(
            stored.user_model.perceived_traits,
            vec!["прямолинейный".to_string()]
        );
    }

    #[tokio::test]
    async fn complete_goal_by_short_id() {
        // Модель ссылается на цель коротким #id из get_self_model.
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["цель"]}))
            .await
            .unwrap();
        let id = storage.db().self_model_get(profile).unwrap().unwrap().goals[0].id;
        let short = id.simple().to_string()[..6].to_string();

        UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"complete_goals": [format!("#{short}")]}),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.active_goals().count(), 0);

        // Несуществующий #id — понятный отчёт, не паника.
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"complete_goals": ["#zzzzzz"]}))
            .await
            .unwrap();
        assert!(out.result.contains("Не найдены цели"));
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
        assert!(got.result.contains("Наблюдения"));
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

    #[tokio::test]
    async fn removing_trait_with_note_leaves_scar_in_narrative() {
        // Ярус 2: ревизия черты с note оставляет след в нарративе (шрам).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({"add_traits": ["компетентный одиночка"]}),
            )
            .await
            .unwrap();
        UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "remove_traits": ["компетентный одиночка"],
                    "add_traits": ["в команде раскрывается при доверии"],
                    "note": "Пересмотрел: раньше видел одиночкой, но в команде при доверии он силён"
                }),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        // Черта заменена, а причина сохранена в нарративе (не стёрта бесследно).
        assert_eq!(
            stored.user_model.perceived_traits,
            vec!["в команде раскрывается при доверии".to_string()]
        );
        assert_eq!(stored.narrative.len(), 1);
        assert!(stored.narrative[0].text.contains("Пересмотрел"));
    }

    #[tokio::test]
    async fn removing_trait_without_note_nudges_for_scar() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["скептик"]}))
            .await
            .unwrap();
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"remove_traits": ["скептик"]}))
            .await
            .unwrap();
        assert!(out.result.contains("без пояснения"));
        assert!(out.result.contains("note"));
    }

    #[tokio::test]
    async fn consolidate_narrative_prunes_and_folds() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        for t in ["дубль А", "дубль Б", "важное"] {
            AddInsight
                .invoke(&ctx, serde_json::json!({ "text": t }))
                .await
                .unwrap();
        }
        let narrative = storage
            .db()
            .self_model_get(profile)
            .unwrap()
            .unwrap()
            .narrative;
        let a = narrative.iter().find(|n| n.text == "дубль А").unwrap().id;
        let b = narrative.iter().find(|n| n.text == "дубль Б").unwrap().id;

        // Свернуть два дубля в один сводный, «важное» не трогаем.
        let out = ConsolidateNarrative
            .invoke(
                &ctx,
                serde_json::json!({
                    "remove": [a.to_string(), b.to_string()],
                    "add": "сводное про дубли"
                }),
            )
            .await
            .unwrap();
        assert!(out.result.contains("убрано: 2"));
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.narrative.len(), 2); // важное + сводное
        assert!(stored.narrative.iter().any(|n| n.text == "важное"));
        assert!(
            stored
                .narrative
                .iter()
                .any(|n| n.text == "сводное про дубли")
        );
        assert!(!stored.narrative.iter().any(|n| n.text == "дубль А"));

        // Несуществующий #id — отчёт, не паника.
        let out = ConsolidateNarrative
            .invoke(&ctx, serde_json::json!({"remove": ["#zzzzzz"]}))
            .await
            .unwrap();
        assert!(out.result.contains("Не найдены наблюдения"));
    }

    #[tokio::test]
    async fn add_insight_reports_count_and_eviction() {
        use crate::entities::self_model::SelfModelParams;
        use crate::shared::config::SelfModelSettings;
        let profile = Uuid::new_v4();
        let (_d, _storage, mut ctx) = ctx_with_storage(profile);
        // Маленький потолок нарратива, чтобы поймать вытеснение.
        ctx.self_model_params = SelfModelParams::from_settings(&SelfModelSettings {
            max_narrative: 2,
            ..SelfModelSettings::default()
        });
        for t in ["первое", "второе"] {
            let out = AddInsight
                .invoke(&ctx, serde_json::json!({ "text": t }))
                .await
                .unwrap();
            assert!(out.result.contains("нарратив"));
            assert!(!out.result.contains("Вытеснены"));
        }
        // Третье вытесняет «первое» — результат явно об этом сообщает.
        let out = AddInsight
            .invoke(&ctx, serde_json::json!({ "text": "третье" }))
            .await
            .unwrap();
        assert!(out.result.contains("нарратив 2/2"));
        assert!(out.result.contains("Вытеснены старейшие"));
        assert!(out.result.contains("«первое»"));
    }

    #[tokio::test]
    async fn update_self_model_folds_old_closed_goals() {
        use crate::entities::self_model::SelfModelParams;
        use crate::shared::config::SelfModelSettings;
        let profile = Uuid::new_v4();
        let (_d, storage, mut ctx) = ctx_with_storage(profile);
        // Держим не более 2 закрытых целей.
        ctx.self_model_params = SelfModelParams::from_settings(&SelfModelSettings {
            max_closed_goals: 2,
            ..SelfModelSettings::default()
        });
        UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"add_goals": ["ц0", "ц1", "ц2", "ц3", "ц4"]}),
            )
            .await
            .unwrap();
        let ids: Vec<String> = storage
            .db()
            .self_model_get(profile)
            .unwrap()
            .unwrap()
            .goals
            .iter()
            .map(|g| g.id.to_string())
            .collect();
        // Закрываем все пять — свёртка оставит 2 самых свежих закрытых, 3 → в архив.
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"complete_goals": ids}))
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.goals.len(), 2); // потолок закрытых целей соблюдён
        assert_eq!(
            stored
                .narrative
                .iter()
                .filter(|s| s.text.starts_with("[архив цели]"))
                .count(),
            3
        );
    }
}
