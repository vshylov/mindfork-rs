//! Инструменты «модели себя» (SelfModel) — MVP-зонд: чтение, рефлексия и
//! минимальное обновление представления агента о себе, целях и собеседнике.
//! Данные пер-профильные, в SQLite (как заметки) — инструменты-мутаторы пишут
//! **напрямую** через `ctx.storage` (не через `ChatEffect`). Изоляция по
//! `ctx.profile_id`. См. [docs/history/self-model-mvp.md](../../../docs/history/self-model-mvp.md).

use anyhow::Result;
use chrono::Utc;

use crate::entities::profile::ToolId;
use crate::entities::self_model::{GoalMatch, GoalStatus, NarrativeSegment, SelfModel};
use crate::shared::i18n::Locale;

use super::{Tool, ToolContext, ToolOutcome, notes};

pub const GET_SELF_MODEL_ID: &str = "get_self_model";
pub const REFLECT_ID: &str = "reflect";
pub const UPDATE_SELF_MODEL_ID: &str = "update_self_model";
pub const UPDATE_USER_MODEL_ID: &str = "update_user_model";
pub const ADD_INSIGHT_ID: &str = "add_insight";

/// Все id инструментов группы «модели себя» (для детекции правок в ходе).
/// `consolidate_narrative` удалён — наблюдения переехали в заметки (Ярус 1
/// «нарратив как заметки»), их консолидируют note-инструменты (note_revise/
/// note_supersede/note_merge). См. docs/history/narrative-as-notes.md.
pub const ALL_IDS: &[&str] = &[
    GET_SELF_MODEL_ID,
    REFLECT_ID,
    UPDATE_SELF_MODEL_ID,
    UPDATE_USER_MODEL_ID,
    ADD_INSIGHT_ID,
];

/// Относится ли инструмент к группе «модели себя» (для сигнала `SelfModelChanged`
/// после хода, где модель что-то правила через свои инструменты).
pub fn is_self_model_tool(name: &str) -> bool {
    ALL_IDS.contains(&name)
}

/// Канонические правила ведения «модели себя» на языке служебного каркаса (`loc`) —
/// ключ `selfmodel.policy_core`. **Единственный источник** формулировок, из которого
/// собираются протокол ведения ([`maintenance_protocol`]) и системное сообщение
/// авто-рефлексии (`orchestrator::reflection`). Интерактивная рубрика `reflect` —
/// намеренно не отсюда (иной жанр). Локализация — ось A, см. docs/history/i18n.md.
pub fn policy_core(loc: &Locale) -> &str {
    loc.t("selfmodel.policy_core")
}

/// Нейтральный к персоне «протокол ведения модели» — [`policy_core`] в обрамлении «ты
/// сам ведёшь эту модель». Подмешивается в системный промпт хода поверх любой персоны
/// профиля (см. `orchestrator::generation::inject_self_model`), делая использование
/// SelfModel-инструментов предсказуемым независимо от персоны.
pub fn maintenance_protocol(loc: &Locale) -> String {
    loc.tf(
        "selfmodel.maintenance_wrapper",
        &[("core", policy_core(loc))],
    )
}

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

/// Свежие наблюдения профиля (self-заметки) как сегменты нарратива — для полного
/// чтения (`render_full`). Наблюдения переехали в заметки (`@self`), поэтому рендер
/// «модели себя» получает их параметром. Новейшие первыми, до `max_narrative`.
fn recent_segments(ctx: &ToolContext) -> Vec<NarrativeSegment> {
    notes::self_notes_recent(
        &ctx.storage,
        ctx.profile_id,
        ctx.self_model_params.max_narrative,
    )
    .into_iter()
    .map(|n| NarrativeSegment {
        id: n.id,
        text: n.content,
        created_at: n.created_at,
    })
    .collect()
}

/// Полное чтение «модели себя» для инструментов (`get_self_model`/`reflect`/эхо):
/// `render_full` + блок «Связанные наблюдения» (граф над наблюдениями, Ярус 2) + блок
/// «Ссылки на источники» (наблюдение опирается на RAG-источник, Ярус 3, Путь 3).
/// Наблюдения, их связи и ссылки — из заметок.
fn render_self_read(ctx: &ToolContext, m: &SelfModel) -> String {
    let recent = recent_segments(ctx);
    let ids: Vec<uuid::Uuid> = recent.iter().map(|s| s.id).collect();
    let mut out = m.render_full(Utc::now(), &recent, ctx.loc);
    if let Some(block) = notes::self_related_block(ctx, &ids) {
        out.push_str(&block);
    }
    if let Some(block) = notes::cited_sources_block(ctx, &ids) {
        out.push_str(&block);
    }
    // Мягкие ворота размера описания (этап 2): если summary разрослось — подсказка
    // сократить. Видна в get_self_model/reflect и авто-рефлексии (та начинает с
    // get_self_model). См. docs/summary-as-snapshot.md.
    if let Some(hint) = m.summary_fill_hint(ctx.self_model_params.summary_target_chars, ctx.loc) {
        out.push_str("\n\n");
        out.push_str(&hint);
    }
    out
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

/// Порог косинусной близости, при котором добавляемая черта считается **родственной**
/// уже имеющейся (ворота `add_traits`). Откалиброван на живом bge-m3 (Шаг C): у него
/// короткие черты сжаты в узкую полосу, и настоящие перефразы дают ~0.73–0.83
/// («любит лаконичность» ↔ «ценит краткость» = 0.77, «скептичный» ↔ «критично» = 0.73),
/// а не-родственные — ниже (кофе↔альпинизм = 0.69, программист↔готовка = 0.58). Порог
/// 0.72 разделяет их. **Важно:** bge-m3 сближает по *измерению/теме*, не по направлению
/// смысла, поэтому в полосу попадают и антонимы («любит краткость» ↔ «любит длинные
/// объяснения» = 0.71) — но это фича ворот: родственную черту стоит показать, чтобы
/// модель решила, **дубль это (слить) или противоречие (записать наблюдением)**. См.
/// docs/history/narrative-as-notes.md (Ярус 2, Шаг C).
const TRAIT_SIMILARITY: f32 = 0.72;

/// Ворота родственных черт (Шаг C): для КАЖДОЙ реально добавленной черты ищет
/// ближайшую среди ПРЕЖНИХ (существовавших до этой правки) выше порога
/// [`TRAIT_SIMILARITY`]. Возвращает пары (добавленная, близкая существующая),
/// новейшие первыми. Эмбеддит новые + прежние одним запросом; у черт нет хранимых
/// векторов (плоский `Vec<String>`), поэтому считаем на лету. Пусто при недоступном
/// эмбеддере или нестыковке числа векторов — **мягкая деградация**, прямое зеркало
/// ворот `add_insight`/`note_save`. См. docs/history/narrative-as-notes.md (Ярус 2, Шаг C).
async fn near_duplicate_traits(
    ctx: &ToolContext,
    added: &[String],
    existing_before: &[String],
) -> Vec<(String, String)> {
    if added.is_empty() || existing_before.is_empty() {
        return Vec::new();
    }
    // Один запрос: сначала добавленные, затем прежние — чтобы разбить по границе.
    let texts: Vec<String> = added.iter().chain(existing_before).cloned().collect();
    let Ok(vecs) = ctx.embedder.embed(texts).await else {
        return Vec::new();
    };
    if vecs.len() != added.len() + existing_before.len() {
        return Vec::new();
    }
    let (added_vecs, existing_vecs) = vecs.split_at(added.len());
    let mut out = Vec::new();
    for (i, a) in added.iter().enumerate() {
        // Ближайшая прежняя черта выше порога (одна на добавленную — не шумим).
        let mut best: Option<(f32, usize)> = None;
        for (j, _) in existing_before.iter().enumerate() {
            let s = notes::cosine(&added_vecs[i], &existing_vecs[j]);
            if s >= TRAIT_SIMILARITY && best.map(|(bs, _)| s > bs).unwrap_or(true) {
                best = Some((s, j));
            }
        }
        if let Some((_, j)) = best {
            out.push((a.clone(), existing_before[j].clone()));
        }
    }
    out
}

/// `get_self_model` — текущее состояние модели себя (чтение).
pub struct GetSelfModel;

#[async_trait::async_trait]
impl Tool for GetSelfModel {
    fn id(&self) -> ToolId {
        GET_SELF_MODEL_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "показать модель себя"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.get_self_model.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let m = load(ctx)?;
        Ok(ToolOutcome::text(render_self_read(ctx, &m)))
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
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "саморефлексия"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.reflect.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let m = load(ctx)?;
        // Обзор наблюдений для консолидации (похожие пары / contradicts / без связей) —
        // конкретные данные под рубрику ниже. Пусто, если наблюдений < 2.
        let mut overview =
            notes::build_self_consolidation_overview(&ctx.storage, ctx.profile_id, ctx.loc)
                .unwrap_or_default();
        // A2: семантическое совпадение абзацев описания себя (summary) с наблюдениями —
        // эмбеддинг абзацев на лету (у summary нет хранимых векторов). См.
        // docs/self-model-consolidation.md §A2.
        if let Some(section) = notes::summary_observation_overlaps(
            &ctx.storage,
            ctx.embedder.as_ref(),
            ctx.profile_id,
            ctx.loc,
        )
        .await
        {
            if overview.is_empty() {
                overview = section;
            } else {
                overview.push_str("\n\n");
                overview.push_str(&section);
            }
        }
        let overview = if overview.is_empty() {
            String::new()
        } else {
            format!("\n\n{overview}")
        };
        let out = format!(
            "{}\n{}{overview}\n\n{}",
            ctx.loc.t("tool.reflect.rubric.header"),
            render_self_read(ctx, &m),
            ctx.loc.t("tool.reflect.rubric.questions"),
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
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "добавить наблюдение"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.add_insight.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": loc.t("tool.add_insight.param.text")}
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
            anyhow::bail!(ctx.loc.t("tool.add_insight.err.text_empty"));
        }
        // Наблюдение — это self-заметка (@self): получает эмбеддинг, семантический
        // поиск, граф и консолидацию наравне с обычными заметками, но скрыта из
        // пользовательского recall. См. docs/history/narrative-as-notes.md.
        let id =
            notes::create_note(ctx, text.clone(), vec![notes::SELF_NOTE_TAG.to_string()]).await?;
        let mut msg = ctx.loc.tf(
            "tool.add_insight.result.recorded",
            &[("id", &id.to_string())],
        );
        // Ворота (ядро гипотезы Яруса 1): похожие существующие наблюдения — чтобы
        // переписать почти-дубль через note_revise/note_supersede, а не плодить копию.
        let similar = notes::self_note_similar(ctx, &text, id).await;
        if !similar.is_empty() {
            msg.push('\n');
            msg.push_str(ctx.loc.t("tool.add_insight.gate.similar"));
            for n in similar {
                msg.push_str(&format!("\n- (id={}) {}", n.id, n.content));
            }
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
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "обновить модель себя"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.update_self_model.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": loc.t("tool.update_self_model.param.summary")},
                "add_goals": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_self_model.param.add_goals")},
                "complete_goals": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_self_model.param.complete_goals")},
                "abandon_goals": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_self_model.param.abandon_goals")}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Нерезолвленные ручки целей и шрамы свёрнутых закрытых целей собираются
        // внутри атомарной правки (захват по `&mut`); запись модели — под одним
        // захватом мьютекса, а шрамы → self-заметки уже после (внутри closure нет
        // доступа к storage/async).
        let mut unresolved: Vec<String> = Vec::new();
        let mut scars: Vec<String> = Vec::new();
        // Менялось ли описание себя — для строки размера в эхе (ворота размера, этап 2).
        let mut summary_changed = false;
        // Дельты правки для компактного эха (этап 4): что реально добавлено/закрыто —
        // вместо полного render_full (тот остаётся у get_self_model). Цели называем
        // #id — теми же ручками, по которым их потом закрывать.
        let mut added_goals: Vec<(String, uuid::Uuid)> = Vec::new();
        let mut completed: Vec<uuid::Uuid> = Vec::new();
        let mut abandoned: Vec<uuid::Uuid> = Vec::new();
        let params = ctx.self_model_params;
        let loc = ctx.loc; // &'static — копируем, чтобы не заимствовать ctx в closure
        let (model, changed) =
            ctx.storage.db().self_model_update(ctx.profile_id, |m| {
                let mut changed = false;
                if let Some(s) = args.get("summary").and_then(|v| v.as_str()) {
                    let s = s.trim().to_string();
                    if m.summary != s {
                        m.summary = s;
                        changed = true;
                        summary_changed = true;
                    }
                }
                for g in str_array(&args, "add_goals") {
                    let before = m.goals.len();
                    m.add_goal(g);
                    // add_goal игнорирует пустые — фиксируем только реально добавленное.
                    if m.goals.len() != before {
                        let goal = m.goals.last().expect("только что добавлена");
                        added_goals.push((goal.description.clone(), goal.id));
                        changed = true;
                    }
                }
                // Цели закрываются по #id/полному id — резолвим ручку среди целей модели.
                for h in str_array(&args, "complete_goals") {
                    match m.match_goal(&h) {
                        GoalMatch::One(id) => {
                            if m.set_goal_status(id, GoalStatus::Completed) {
                                completed.push(id);
                                changed = true;
                            }
                        }
                        GoalMatch::None => unresolved.push(h),
                        GoalMatch::Ambiguous => unresolved
                            .push(loc.tf("tool.update_self_model.ambiguous", &[("h", &h)])),
                    }
                }
                for h in str_array(&args, "abandon_goals") {
                    match m.match_goal(&h) {
                        GoalMatch::One(id) => {
                            if m.set_goal_status(id, GoalStatus::Abandoned) {
                                abandoned.push(id);
                                changed = true;
                            }
                        }
                        GoalMatch::None => unresolved.push(h),
                        GoalMatch::Ambiguous => unresolved
                            .push(loc.tf("tool.update_self_model.ambiguous", &[("h", &h)])),
                    }
                }
                // Свёртка старых закрытых целей: fold возвращает шрамы (тексты) — их
                // запишем self-заметками после атомарной правки (потолок закрытых целей:
                // структура не растёт, «биография» сохраняется наблюдением).
                scars = m.fold_closed_goals(params.max_closed_goals, loc);
                if !scars.is_empty() {
                    changed = true;
                }
                changed
            })?;

        // Шрамы свёрнутых закрытых целей → self-заметки (наблюдения). Best-effort.
        for scar in &scars {
            let _ =
                notes::create_note(ctx, scar.clone(), vec![notes::SELF_NOTE_TAG.to_string()]).await;
        }

        if !changed {
            let mut msg = ctx.loc.t("selfmodel.result.nothing").to_string();
            if !unresolved.is_empty() {
                msg.push('\n');
                msg.push_str(&ctx.loc.tf(
                    "tool.update_self_model.goals_not_found",
                    &[("list", &unresolved.join(", "))],
                ));
            }
            return Ok(ToolOutcome::text(msg));
        }
        // Дельта-эхо (этап 4): только изменённое, без полного render_full (полное чтение
        // — у get_self_model). Экономит токены и не «заякоривает» модель на жанре эссе.
        use crate::entities::self_model::short_id;
        let mut msg = ctx
            .loc
            .t("tool.update_self_model.result.updated")
            .to_string();
        // Обратная связь о размере описания (этап 2): всегда при правке summary, чтобы
        // модель видела рост даже до превышения ориентира. См. docs/summary-as-snapshot.md.
        if summary_changed {
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_self_model.size",
                &[
                    ("n", &model.summary.chars().count().to_string()),
                    ("target", &params.summary_target_chars.to_string()),
                ],
            ));
        }
        if !added_goals.is_empty() {
            let list: Vec<String> = added_goals
                .iter()
                .map(|(d, id)| format!("#{} {d}", short_id(id)))
                .collect();
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_self_model.added_goals",
                &[("list", &list.join("; "))],
            ));
        }
        if !completed.is_empty() {
            let ids: Vec<String> = completed
                .iter()
                .map(|id| format!("#{}", short_id(id)))
                .collect();
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_self_model.completed",
                &[("list", &ids.join(", "))],
            ));
        }
        if !abandoned.is_empty() {
            let ids: Vec<String> = abandoned
                .iter()
                .map(|id| format!("#{}", short_id(id)))
                .collect();
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_self_model.abandoned",
                &[("list", &ids.join(", "))],
            ));
        }
        if !scars.is_empty() {
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_self_model.folded",
                &[("n", &scars.len().to_string())],
            ));
        }
        if !unresolved.is_empty() {
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_self_model.goals_not_found_paren",
                &[("list", &unresolved.join(", "))],
            ));
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
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "обновить собеседника"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.update_user_model.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "add_traits": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.add_traits")},
                "remove_traits": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.remove_traits")},
                "add_interests": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.add_interests")},
                "remove_interests": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.remove_interests")},
                "relationship_dynamic": {"type": "string", "description": loc.t("tool.update_user_model.param.relationship_dynamic")},
                "note": {"type": "string", "description": loc.t("tool.update_user_model.param.note")}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Признаки удаления/наличия note собираем внутри атомарной правки (захват по
        // `&mut`); запись — под одним захватом мьютекса (защита от гонки).
        let mut removed_traits = false;
        let mut removed_interests = false;
        let mut replaced_dynamic = false;
        let mut has_note = false;
        // Шрам ревизии (`note`) собираем внутри closure, но записываем **после** —
        // как self-заметку (наблюдение), а не в блоб модели (async/storage вне closure).
        let mut note_scar: Option<String> = None;
        // Ворота почти-дублей черт (Шаг C): нужен снимок черт ДО добавления —
        // собираем внутри атомарной правки (захват по `&mut`), эмбеддинг — после.
        let requested_traits = str_array(&args, "add_traits");
        let mut existing_before_traits: Vec<String> = Vec::new();
        let (model, changed) = ctx.storage.db().self_model_update(ctx.profile_id, |m| {
            let mut changed = false;

            // Списки — merge (add/remove с дедупом), а не замена: правка не обнуляет
            // накопленное представление (частая беда «перетирания по настроению»).
            existing_before_traits = m.user_model.perceived_traits.clone();
            changed |= m.user_model.add_traits(requested_traits.clone());
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
                    // Замена НЕПУСТОЙ динамики — существенный пересмотр (в отличие от
                    // первичного заполнения); просим оставить след (шрам), как у черт.
                    replaced_dynamic = !m.user_model.relationship_dynamic.trim().is_empty();
                    m.user_model.relationship_dynamic = s;
                    changed = true;
                }
            }
            // Шрам ревизии: `note` (что и почему изменилось) сохраняем — уйдёт
            // наблюдением-заметкой «о себе», так изменение мнения о собеседнике
            // оставляет след (черты плоские — «биография» их изменений в наблюдениях).
            note_scar = args
                .get("note")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            has_note = note_scar.is_some();
            changed
        })?;

        // Шрам → self-заметка (наблюдение). Best-effort. `note` сам по себе изменением
        // модели не считается (это отдельная заметка), но делает вызов результативным.
        if let Some(scar) = &note_scar {
            let _ =
                notes::create_note(ctx, scar.clone(), vec![notes::SELF_NOTE_TAG.to_string()]).await;
        }

        if !changed && !has_note {
            return Ok(ToolOutcome::text(ctx.loc.t("selfmodel.result.nothing")));
        }
        // Реально добавленные черты (новые после дедупа, без внутрибатчевых повторов) —
        // сравниваем их с прежними воротами почти-дублей (эмбеддинг только если есть
        // что сравнивать; при недоступном эмбеддере — мягко пусто).
        let mut added_traits: Vec<String> = Vec::new();
        for t in &requested_traits {
            let lc = t.to_lowercase();
            let known = existing_before_traits
                .iter()
                .any(|x| x.to_lowercase() == lc)
                || added_traits.iter().any(|x| x.to_lowercase() == lc);
            if !known {
                added_traits.push(t.clone());
            }
        }
        let dup_pairs = near_duplicate_traits(ctx, &added_traits, &existing_before_traits).await;

        // Дельта-эхо (этап 4): компактные итоговые списки модели собеседника вместо
        // полного render_full (полное чтение — у get_self_model). Списки коротки по
        // построению (merge с дедупом), поэтому показываем их целиком.
        let mut msg = ctx
            .loc
            .t("tool.update_user_model.result.updated")
            .to_string();
        let u = &model.user_model;
        if !u.perceived_traits.is_empty() {
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_user_model.traits",
                &[("list", &u.perceived_traits.join(", "))],
            ));
        }
        if !u.current_interests.is_empty() {
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_user_model.interests",
                &[("list", &u.current_interests.join(", "))],
            ));
        }
        if !u.relationship_dynamic.trim().is_empty() {
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.update_user_model.relationship",
                &[("dyn", u.relationship_dynamic.trim())],
            ));
        }
        // Подтверждение шрама ревизии (если передан note) — виден его текст.
        if let Some(scar) = &note_scar {
            msg.push('\n');
            msg.push_str(
                &ctx.loc
                    .tf("tool.update_user_model.scar_saved", &[("scar", scar)]),
            );
        }
        // Ворота родственных черт (Шаг C): близкая по теме черта уже существует.
        // bge-m3 сближает черты по измерению (перефразы И антонимы), поэтому просим
        // модель РЕШИТЬ: это дубль (слить через remove_traits) или противоречие
        // (записать наблюдением add_insight) — зеркало ворот add_insight, но над
        // плоским списком черт (интеграция вместо накопления).
        if !dup_pairs.is_empty() {
            msg.push('\n');
            msg.push_str(ctx.loc.t("tool.update_user_model.gate.related"));
            for (added, existing) in &dup_pairs {
                msg.push_str(&format!("\n- «{added}» ≈ «{existing}»"));
            }
        }
        // Удаление черты/интереса или замена непустой динамики — пересмотр суждения.
        // Причина не записана → напоминаем оставить след наблюдением (шрам), а не менять
        // молча (смена динамики отношений — самый значимый пересмотр модели собеседника).
        if (removed_traits || removed_interests || replaced_dynamic) && !has_note {
            msg.push('\n');
            msg.push_str(ctx.loc.t("tool.update_user_model.nudge_note"));
        }
        Ok(ToolOutcome::text(msg))
    }
}

// `consolidate_narrative` удалён: наблюдения переехали в заметки (Ярус 1), их
// консолидируют note-инструменты (note_revise/note_supersede/note_merge) — они
// сильнее (замещение со «шрамом», а не удаление по id). См. docs/history/narrative-as-notes.md.

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    /// Self-заметки (наблюдения) профиля — новейшие первыми. Наблюдения переехали в
    /// заметки (@self), поэтому проверяем их там, а не в блобе модели.
    fn self_notes(
        storage: &crate::shared::storage::Storage,
        profile: Uuid,
    ) -> Vec<crate::entities::note::Note> {
        storage
            .db()
            .note_list(profile, None, &[notes::SELF_NOTE_TAG.to_string()], None)
            .unwrap()
    }

    #[test]
    fn self_model_tool_descriptions_are_localized() {
        // Каждый инструмент «модели себя» возвращает РАЗНЫЙ текст на ru/en (ловит
        // забытый `_loc`), en — без кириллицы. §3.5 docs/history/i18n.md.
        use crate::shared::i18n::{Lang, locale};
        let (r, e) = (locale(Lang::Ru), locale(Lang::En));
        let no_cyr = |s: &str| {
            !s.chars()
                .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
        };
        let pairs: Vec<(String, String)> = vec![
            (GetSelfModel.description(r), GetSelfModel.description(e)),
            (Reflect.description(r), Reflect.description(e)),
            (AddInsight.description(r), AddInsight.description(e)),
            (
                UpdateSelfModel.description(r),
                UpdateSelfModel.description(e),
            ),
            (
                UpdateUserModel.description(r),
                UpdateUserModel.description(e),
            ),
        ];
        for (ru_d, en_d) in pairs {
            assert_ne!(ru_d, en_d, "описание не локализовано: {ru_d}");
            assert!(no_cyr(&en_d), "кириллица в en-описании: {en_d}");
        }
    }

    #[tokio::test]
    async fn add_insight_result_localized_for_all_langs() {
        // Подтверждение записи наблюдения рендерится на каждом вшитом языке.
        use crate::shared::i18n::{Lang, locale};
        for &lang in Lang::ALL {
            let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
            ctx.loc = locale(lang);
            let out = AddInsight
                .invoke(&ctx, serde_json::json!({"text": "hello observation"}))
                .await
                .unwrap();
            // Результат начинается с локализованного шаблона «записано (id=…)».
            let prefix = locale(lang)
                .t("tool.add_insight.result.recorded")
                .split("{id}")
                .next()
                .unwrap();
            assert!(out.result.starts_with(prefix), "{lang:?}: {}", out.result);
        }
    }

    #[test]
    fn is_self_model_tool_recognizes_group() {
        assert!(is_self_model_tool(UPDATE_SELF_MODEL_ID));
        assert!(is_self_model_tool(ADD_INSIGHT_ID));
        assert!(is_self_model_tool(GET_SELF_MODEL_ID));
        assert!(!is_self_model_tool("note_save"));
        assert!(!is_self_model_tool("web_search"));
    }

    /// Референсная локаль (ru) — ассерты на русские подстроки пинят ru-бандл.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn maintenance_protocol_wraps_policy_core() {
        let p = maintenance_protocol(ru());
        // Обрамление «ты сам ведёшь» + весь POLICY_CORE (с ключевой фразой против лести).
        assert!(p.contains("Ты сам ведёшь эту «модель себя»"));
        assert!(p.contains(policy_core(ru())));
        assert!(p.contains("угодливости"));
    }

    /// Per-language (§3.5): протокол ведения оборачивает `policy_core` того же языка на
    /// каждом вшитом языке; плейсхолдер `{core}` подставлен.
    #[test]
    fn maintenance_protocol_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let p = maintenance_protocol(l);
            assert!(
                p.contains(policy_core(l)),
                "{lang:?}: policy_core не встроен"
            );
            assert!(!p.contains("{core}"), "{lang:?}: плейсхолдер не подставлен");
        }
    }

    #[test]
    fn policy_core_routes_events_to_insights() {
        // Этап 1 (summary — снимок, не летопись): жанровая граница проведена по оси
        // «состояние → summary, событие-вывод → add_insight (даже устойчивое)».
        let core = policy_core(ru());
        assert!(core.contains("снимок"));
        assert!(core.contains("СОКРАЩАЙ"));
        assert!(core.contains("ДАЖЕ ЕСЛИ"));
        // Явный шаг «прочти целиком перед правкой» (защита от правки с усечённого вида).
        assert!(core.contains("прочти его целиком через get_self_model"));
    }

    #[test]
    fn update_self_model_description_routes_events_to_insights() {
        // Описание инструмента направляет событийные выводы в add_insight, а summary
        // держит компактным снимком.
        let d =
            UpdateSelfModel.description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru));
        assert!(d.contains("снимок"));
        assert!(d.contains("add_insight"));
    }

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
    async fn update_summary_echo_shows_size() {
        // Этап 2: эхо правки summary всегда несёт строку размера (обратная связь о росте).
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"summary": "ценю ясность"}))
            .await
            .unwrap();
        assert!(out.result.contains("Описание:"));
        assert!(out.result.contains("симв."));
        // Правка без summary (только цель) — строки размера нет.
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["цель"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Описание:"));
    }

    #[tokio::test]
    async fn update_self_model_echo_is_delta_not_full() {
        // Этап 4: эхо правки несёт дельты (#id добавленной цели), но НЕ полный текст
        // summary (полное чтение — только у get_self_model).
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "summary": "УНИКАЛЬНЫЙ_МАРКЕР_ОПИСАНИЯ",
                    "add_goals": ["новая цель"]
                }),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Добавлены цели:"));
        assert!(out.result.contains("новая цель"));
        assert!(out.result.contains('#'));
        assert!(out.result.contains("Описание:")); // строка размера (этап 2)
        // Текст summary в эхо не попадает (нет «заякоривания» на жанре эссе).
        assert!(!out.result.contains("УНИКАЛЬНЫЙ_МАРКЕР_ОПИСАНИЯ"));
    }

    #[tokio::test]
    async fn update_self_model_echo_shows_closed_goal_id() {
        // Этап 4: закрытие цели отражается в эхе её #id (та же ручка, что и для закрытия).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["цель"]}))
            .await
            .unwrap();
        let id = storage.db().self_model_get(profile).unwrap().unwrap().goals[0].id;
        let short = id.simple().to_string()[..6].to_string();
        let out = UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"complete_goals": [format!("#{short}")]}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Закрыты выполненными:"));
        assert!(out.result.contains(&format!("#{short}")));
    }

    #[tokio::test]
    async fn update_user_model_echo_shows_final_lists() {
        // Этап 4: эхо показывает компактные итоговые списки, не полный render_full.
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "add_traits": ["скептик"],
                    "add_interests": ["Rust"],
                    "relationship_dynamic": "рабочие"
                }),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Черты теперь: скептик"));
        assert!(out.result.contains("Интересы теперь: Rust"));
        assert!(out.result.contains("Отношения: рабочие"));
    }

    #[tokio::test]
    async fn get_self_model_surfaces_summary_fill_hint_over_target() {
        // Этап 2: раздутое описание (сверх ориентира) поднимает подсказку в чтении.
        use crate::entities::self_model::SelfModelParams;
        use crate::shared::config::SelfModelSettings;
        let profile = Uuid::new_v4();
        let (_d, _s, mut ctx) = ctx_with_storage(profile);
        // Ориентир 5 санитизируется до пола 200 — описание берём длиннее 200 символов.
        ctx.self_model_params = SelfModelParams::from_settings(&SelfModelSettings {
            summary_target_chars: 5,
            ..SelfModelSettings::default()
        });
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"summary": "я".repeat(250)}))
            .await
            .unwrap();
        let out = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("Описание себя разрослось"));
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
        // Этап 1: рубрика спрашивает про разрастание описания себя.
        assert!(out.result.contains("Не разрослось ли описание себя"));
        assert!(out.effects.is_empty());
        // Без наблюдений (< 2) обзор self-консолидации не подмешивается.
        assert!(!out.result.contains("Обзор наблюдений"));
    }

    #[tokio::test]
    async fn reflect_includes_self_consolidation_overview() {
        // Ярус 3: при ≥2 наблюдениях reflect подмешивает обзор self-консолидации
        // (конкретные похожие пары / связи / без связей под рубрику).
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaaa bbbb"}))
            .await
            .unwrap();
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaab"}))
            .await
            .unwrap();
        let out = Reflect.invoke(&ctx, serde_json::json!({})).await.unwrap();
        assert!(out.result.contains("Обзор наблюдений"));
        assert!(out.result.contains("Похожие пары"));
    }

    #[tokio::test]
    async fn add_insight_creates_self_note_and_shows() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        let out = AddInsight
            .invoke(
                &ctx,
                serde_json::json!({"text": "напряжение между краткостью и полнотой"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Наблюдение записано"));
        // Записано как self-заметка (@self), а не в блоб модели (его нет).
        assert_eq!(self_notes(&storage, profile).len(), 1);
        assert!(storage.db().self_model_get(profile).unwrap().is_none());

        // get_self_model показывает наблюдение (собранное из заметок).
        let got = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(got.result.contains("Наблюдения"));
        assert!(got.result.contains("напряжение"));

        // Пустой text — ошибка, ничего не создано.
        assert!(
            AddInsight
                .invoke(&ctx, serde_json::json!({"text": "  "}))
                .await
                .is_err()
        );
        assert_eq!(self_notes(&storage, profile).len(), 1);
    }

    #[tokio::test]
    async fn add_insight_gate_surfaces_similar_observation() {
        // Ядро гипотезы Яруса 1: почти-дубль наблюдения показывает ворота (похожую
        // существующую self-заметку) с подсказкой переписать через note_revise.
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaaa bbbb"}))
            .await
            .unwrap();
        let out = AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaab"}))
            .await
            .unwrap();
        assert!(out.result.contains("Похожие наблюдения"));
        assert!(out.result.contains("aaaa bbbb"));
        assert!(out.result.contains("note_revise"));
    }

    #[tokio::test]
    async fn get_self_model_surfaces_linked_observations() {
        // Ярус 2: get_self_model показывает связи между наблюдениями (граф).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "ценю краткость"}))
            .await
            .unwrap();
        AddInsight
            .invoke(
                &ctx,
                serde_json::json!({"text": "иногда бываю многословен"}),
            )
            .await
            .unwrap();
        let ns = self_notes(&storage, profile);
        storage
            .db()
            .note_link_insert(profile, ns[0].id, ns[1].id, "contradicts")
            .unwrap();

        let out = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("Связи наблюдений"));
        assert!(out.result.contains("contradicts"));
    }

    #[tokio::test]
    async fn add_trait_gate_surfaces_near_duplicate() {
        // Шаг C: добавление черты, родственной уже имеющейся, поднимает ворота
        // (зеркало ворот add_insight). MockEmbedder(16) — мешок символов: «aaaa bbbb»
        // ↔ «aaab» близки (cosine ≈ 0.89 > порога 0.72).
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        // Первая черта — прежних нет, ворота молчат.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaaa bbbb"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Родственные черты"));
        // Вторая черта близка к первой → ворота показывают родственную черту.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaab"]}))
            .await
            .unwrap();
        assert!(out.result.contains("Родственные черты"));
        assert!(out.result.contains("aaab"));
        assert!(out.result.contains("aaaa bbbb"));
        assert!(out.result.contains("remove_traits"));
    }

    #[tokio::test]
    async fn add_trait_gate_silent_for_dissimilar() {
        // Неродственная черта не поднимает ворота (ложных срабатываний нет).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaaa bbbb"]}))
            .await
            .unwrap();
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["wwww"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Родственные черты"));
        // Обе черты сохранены (ворота ничего не блокируют — только предупреждают).
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.user_model.perceived_traits.len(), 2);
    }

    #[tokio::test]
    async fn removing_trait_with_note_leaves_scar_as_self_note() {
        // Ревизия черты с note оставляет след — теперь self-заметкой (наблюдением).
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
        // Черта заменена, а причина сохранена self-заметкой (не в блобе, не стёрта).
        assert_eq!(
            stored.user_model.perceived_traits,
            vec!["в команде раскрывается при доверии".to_string()]
        );
        assert!(stored.narrative.is_empty());
        let n = self_notes(&storage, profile);
        assert_eq!(n.len(), 1);
        assert!(n[0].content.contains("Пересмотрел"));
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
    async fn replacing_nonempty_dynamic_without_note_nudges() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        // Первичное заполнение динамики — БЕЗ напоминания (это не пересмотр).
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({"relationship_dynamic": "доверительные"}),
            )
            .await
            .unwrap();
        assert!(!out.result.contains("без пояснения"));
        // Замена непустой динамики без note — напоминание про шрам.
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({"relationship_dynamic": "натянутые"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("без пояснения"));
        // С note — напоминания нет, а причина уходит наблюдением (виден в эхо).
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "relationship_dynamic": "снова тёплые",
                    "note": "помирились после спора"
                }),
            )
            .await
            .unwrap();
        assert!(!out.result.contains("без пояснения"));
        assert!(out.result.contains("помирились после спора"));
    }

    #[tokio::test]
    async fn update_self_model_folds_old_closed_goals_into_notes() {
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
        // Закрываем все пять — свёртка оставит 2 самых свежих закрытых, 3 → в архив
        // (теперь self-заметками, не в блобе).
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"complete_goals": ids}))
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.goals.len(), 2); // потолок закрытых целей соблюдён
        assert!(stored.narrative.is_empty());
        // Три свёрнутых цели — self-заметки «[архив цели]».
        let archived = self_notes(&storage, profile)
            .into_iter()
            .filter(|n| n.content.starts_with("[архив цели]"))
            .count();
        assert_eq!(archived, 3);
    }
}
