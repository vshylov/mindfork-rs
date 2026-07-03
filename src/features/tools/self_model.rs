//! Инструменты «модели себя» (SelfModel) — MVP-зонд: чтение, рефлексия и
//! минимальное обновление представления агента о себе, целях и собеседнике.
//! Данные пер-профильные, в SQLite (как заметки) — инструменты-мутаторы пишут
//! **напрямую** через `ctx.storage` (не через `ChatEffect`). Изоляция по
//! `ctx.profile_id`. См. [docs/self-model-mvp.md](../../../docs/self-model-mvp.md).

use anyhow::Result;
use chrono::Utc;

use crate::entities::profile::ToolId;
use crate::entities::self_model::{GoalMatch, GoalStatus, NarrativeSegment, SelfModel};

use super::{Tool, ToolContext, ToolOutcome, notes};

pub const GET_SELF_MODEL_ID: &str = "get_self_model";
pub const REFLECT_ID: &str = "reflect";
pub const UPDATE_SELF_MODEL_ID: &str = "update_self_model";
pub const UPDATE_USER_MODEL_ID: &str = "update_user_model";
pub const ADD_INSIGHT_ID: &str = "add_insight";

/// Все id инструментов группы «модели себя» (для детекции правок в ходе).
/// `consolidate_narrative` удалён — наблюдения переехали в заметки (Ярус 1
/// «нарратив как заметки»), их консолидируют note-инструменты (note_revise/
/// note_supersede/note_merge). См. docs/narrative-as-notes.md.
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

/// Канонические правила ведения «модели себя» — **единственный источник** формулировок,
/// из которого собираются оба текста, инструктирующих модель: протокол ведения
/// (пассивная инъекция в промпт хода, [`maintenance_protocol`]) и системное сообщение
/// фоновой авто-рефлексии (`orchestrator::reflection`). Раньше эти правила
/// дублировались в двух местах и уже слегка разъехались; здесь они одни (этап 6
/// доводки). Интерактивная рубрика инструмента `reflect` намеренно **не** отсюда — она
/// иного жанра (вопросы, а не императив), но покрывает те же темы.
pub const POLICY_CORE: &str = "Когда что-то устойчивое изменилось — о тебе, о собеседнике или о твоих целях — \
     зафиксируй это инструментами: update_self_model (описание себя — интегрируй прежнее \
     с новым, не переписывай с нуля; веди цели по #id — закрывай выполненные и \
     неактуальные, а не только ставь новые), update_user_model (черты/интересы \
     собеседника — add_/remove_, не перетирая прежнее), add_insight (наблюдение или \
     противоречие прозой — оно сохранится как заметка «о себе»). Мимолётное (настроение, \
     разовая реакция) — в add_insight, не в модель собеседника. Если наблюдение почти \
     повторяет прежнее (add_insight покажет похожие) — перепиши то через note_revise или \
     замести note_supersede, а не плоди почти-дубль. Точность важнее угодливости: \
     фиксируй то, что верно, а не что польстит.";

/// Нейтральный к персоне «протокол ведения модели» — [`POLICY_CORE`] в обрамлении «ты
/// сам ведёшь эту модель». Подмешивается в системный промпт хода поверх любой персоны
/// профиля (см. `orchestrator::generation::inject_self_model`), делая использование
/// SelfModel-инструментов предсказуемым независимо от персоны.
pub fn maintenance_protocol() -> String {
    format!("(Ты сам ведёшь эту «модель себя». {POLICY_CORE})")
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
/// `render_full` + блок «Связанные наблюдения» (граф над наблюдениями, Ярус 2).
/// Наблюдения и их связи — из заметок.
fn render_self_read(ctx: &ToolContext, m: &SelfModel) -> String {
    let recent = recent_segments(ctx);
    let ids: Vec<uuid::Uuid> = recent.iter().map(|s| s.id).collect();
    let mut out = m.render_full(Utc::now(), &recent);
    if let Some(block) = notes::self_related_block(ctx, &ids) {
        out.push_str(&block);
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

/// Порог косинусной близости, при котором добавляемая черта считается почти-дублем
/// уже имеющейся (ворота `add_traits`, зеркало ворот `add_insight`). Консервативный —
/// предупреждать о перефразах («ценит краткость» ↔ «любит лаконичность»), а не о любой
/// смежной черте. То же значение, что у обзора консолидации заметок (эмпирически).
const TRAIT_SIMILARITY: f32 = 0.85;

/// Ворота почти-дублей черт (Шаг C): для КАЖДОЙ реально добавленной черты ищет
/// ближайшую среди ПРЕЖНИХ (существовавших до этой правки) выше порога
/// [`TRAIT_SIMILARITY`]. Возвращает пары (добавленная, близкая существующая),
/// новейшие первыми. Эмбеддит новые + прежние одним запросом; у черт нет хранимых
/// векторов (плоский `Vec<String>`), поэтому считаем на лету. Пусто при недоступном
/// эмбеддере или нестыковке числа векторов — **мягкая деградация**, прямое зеркало
/// ворот `add_insight`/`note_save`. См. docs/narrative-as-notes.md (Ярус 2, Шаг C).
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
             - Что устойчивого я понял(а) о себе? Уточни update_self_model.summary — \
             интегрируй прежнее с новым, не переписывай с нуля.\n\
             - Цели: пройди по активным по #id — какие выполнены (complete_goals) или \
             неактуальны (abandon_goals)? появились ли новые (add_goals)?\n\
             - Что устойчивого узнал(а) о собеседнике? update_user_model правит списки по \
             частям (add_/remove_), не перетирая. Мимолётное (настроение, разовая \
             реакция) — в add_insight, не в модель собеседника.\n\
             - Заметил(а) ли противоречие/напряжение? Запиши прозой через add_insight \
             (оно сохранится как заметка «о себе»).\n\
             - Есть ли среди наблюдений почти-дубли или устаревшее? Перепиши их через \
             note_revise или замести note_supersede по полному id (из get_self_model), \
             а не плоди почти-копии.\n\
             - Соотносятся ли наблюдения (противоречат, уточняют, об одном)? Свяжи их \
             note_link (contradicts/refines/relates) по полному id — память связной, а \
             не россыпью.\n\
             Меняй только то, что действительно изменилось; если менять нечего — ничего \
             не вызывай.",
            render_self_read(ctx, &m)
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
        // Наблюдение — это self-заметка (@self): получает эмбеддинг, семантический
        // поиск, граф и консолидацию наравне с обычными заметками, но скрыта из
        // пользовательского recall. См. docs/narrative-as-notes.md.
        let id =
            notes::create_note(ctx, text.clone(), vec![notes::SELF_NOTE_TAG.to_string()]).await?;
        let mut msg = format!("Наблюдение записано (id={id}).");
        // Ворота (ядро гипотезы Яруса 1): похожие существующие наблюдения — чтобы
        // переписать почти-дубль через note_revise/note_supersede, а не плодить копию.
        let similar = notes::self_note_similar(ctx, &text, id).await;
        if !similar.is_empty() {
            msg.push_str(
                "\nПохожие наблюдения (возможен дубль — при необходимости перепиши то \
                 через note_revise или замести note_supersede вместо новой записи):",
            );
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
        // Нерезолвленные ручки целей и шрамы свёрнутых закрытых целей собираются
        // внутри атомарной правки (захват по `&mut`); запись модели — под одним
        // захватом мьютекса, а шрамы → self-заметки уже после (внутри closure нет
        // доступа к storage/async).
        let mut unresolved: Vec<String> = Vec::new();
        let mut scars: Vec<String> = Vec::new();
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
            // Свёртка старых закрытых целей: fold возвращает шрамы (тексты) — их
            // запишем self-заметками после атомарной правки (потолок закрытых целей:
            // структура не растёт, «биография» сохраняется наблюдением).
            scars = m.fold_closed_goals(params.max_closed_goals);
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
            let mut msg = String::from("Нечего обновлять (не передано ни одного изменения).");
            if !unresolved.is_empty() {
                msg.push_str(&format!("\nНе найдены цели: {}.", unresolved.join(", ")));
            }
            return Ok(ToolOutcome::text(msg));
        }
        let mut msg = format!(
            "Модель себя обновлена.\n{}",
            model.render_full(Utc::now(), &recent_segments(ctx))
        );
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
         реакция) записывай в add_insight, а не сюда. Если добавляемая черта близка к уже \
         имеющейся, инструмент предупредит о почти-дубле — интегрируй их в одну \
         (remove_traits + одна точная), а не копи перефразы."
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
                "relationship_dynamic": {"type": "string", "description": "Как вы относитесь во времени (заменяет прежнее; при существенной смене передай note)"},
                "note": {"type": "string", "description": "Что и почему изменилось (при удалении/замене черт или смене динамики) — уходит в нарратив как след ревизии"}
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
            return Ok(ToolOutcome::text(
                "Нечего обновлять (не передано ни одного изменения).",
            ));
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

        let mut msg = format!(
            "Модель собеседника обновлена.\n{}",
            model.render_full(Utc::now(), &recent_segments(ctx))
        );
        // Ворота почти-дублей черт (Шаг C): близкая к добавленной уже существует —
        // подсказываем оставить одну через remove_traits, чтобы модель собеседника не
        // раздувалась перефразами (зеркало ворот add_insight над наблюдениями).
        if !dup_pairs.is_empty() {
            msg.push_str(
                "\nПохожие черты уже есть (возможен почти-дубль — при необходимости оставь \
                 одну через remove_traits, а не копи перефразы):",
            );
            for (added, existing) in &dup_pairs {
                msg.push_str(&format!("\n- «{added}» ≈ «{existing}»"));
            }
        }
        // Удаление черты/интереса или замена непустой динамики — пересмотр суждения.
        // Причина не записана → напоминаем оставить след наблюдением (шрам), а не менять
        // молча (смена динамики отношений — самый значимый пересмотр модели собеседника).
        if (removed_traits || removed_interests || replaced_dynamic) && !has_note {
            msg.push_str(
                "\n(Ты изменил(а) черты/интересы/динамику без пояснения. Если это пересмотр \
                 мнения — передай note с тем, что и почему изменилось: он останется \
                 наблюдением как след, чтобы модель себя помнила, что менялась.)",
            );
        }
        Ok(ToolOutcome::text(msg))
    }
}

// `consolidate_narrative` удалён: наблюдения переехали в заметки (Ярус 1), их
// консолидируют note-инструменты (note_revise/note_supersede/note_merge) — они
// сильнее (замещение со «шрамом», а не удаление по id). См. docs/narrative-as-notes.md.

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
    fn is_self_model_tool_recognizes_group() {
        assert!(is_self_model_tool(UPDATE_SELF_MODEL_ID));
        assert!(is_self_model_tool(ADD_INSIGHT_ID));
        assert!(is_self_model_tool(GET_SELF_MODEL_ID));
        assert!(!is_self_model_tool("note_save"));
        assert!(!is_self_model_tool("web_search"));
    }

    #[test]
    fn maintenance_protocol_wraps_policy_core() {
        let p = maintenance_protocol();
        // Обрамление «ты сам ведёшь» + весь POLICY_CORE (с ключевой фразой против лести).
        assert!(p.contains("Ты сам ведёшь эту «модель себя»"));
        assert!(p.contains(POLICY_CORE));
        assert!(p.contains("угодливости"));
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
        // Шаг C: добавление черты, близкой к уже имеющейся, поднимает ворота
        // почти-дубля (зеркало ворот add_insight). MockEmbedder(16) — мешок символов:
        // «aaaa bbbb» ↔ «aaab» близки (cosine ≈ 0.89 > порога 0.85).
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        // Первая черта — прежних нет, ворота молчат.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaaa bbbb"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Похожие черты"));
        // Вторая черта близка к первой → ворота предупреждают о почти-дубле.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaab"]}))
            .await
            .unwrap();
        assert!(out.result.contains("Похожие черты"));
        assert!(out.result.contains("aaab"));
        assert!(out.result.contains("aaaa bbbb"));
        assert!(out.result.contains("remove_traits"));
    }

    #[tokio::test]
    async fn add_trait_gate_silent_for_dissimilar() {
        // Непохожая черта не поднимает ворота (ложных срабатываний нет).
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
        assert!(!out.result.contains("Похожие черты"));
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
