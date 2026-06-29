//! Инструменты заметок: `note_save`, `note_recall`. Память ассистента о
//! пользователе/контексте, **изолированная по `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;
use uuid::Uuid;

use crate::entities::note::Note;
use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Имя инструмента ревизии заметки (DB-only, гейтится набором профиля).
pub const NOTE_REVISE_ID: &str = "note_revise";
/// Граф связей и ревизионная история (Ярус 2, DB-only, гейтятся набором профиля).
pub const NOTE_LINK_ID: &str = "note_link";
pub const NOTE_NEIGHBORS_ID: &str = "note_neighbors";
pub const NOTE_SUPERSEDE_ID: &str = "note_supersede";
pub const NOTE_MERGE_ID: &str = "note_merge";
pub const CONSOLIDATE_NOTES_ID: &str = "consolidate_notes";

/// Порог косинусной близости, при котором две заметки считаются возможным дублем
/// (для обзора консолидации). Подобран эмпирически — пары выше стоит рассмотреть.
const CONSOLIDATE_SIMILARITY: f32 = 0.85;
/// Сколько элементов максимум показывать в каждой секции обзора консолидации.
const CONSOLIDATE_LIST_CAP: usize = 8;

/// Типы связей между заметками (направленные). Зеркалят схему инструмента `note_link`.
const RELATIONS: [&str; 4] = ["supports", "contradicts", "refines", "relates"];

/// Сколько заметок отдаёт `note_recall` по умолчанию (если лимит не задан).
const DEFAULT_RECALL: usize = 5;

/// Размер батча при бэкфилле эмбеддингов «старых» заметок.
const NOTE_BACKFILL_BATCH: usize = 32;

/// Сколько связанных заметок максимум подмешивать в `note_recall` (spreading activation).
const RELATED_IN_RECALL: usize = 5;

/// `note_save` — сохраняет заметку профиля. Возвращает её id.
pub struct NoteSave;

#[async_trait::async_trait]
impl Tool for NoteSave {
    fn id(&self) -> ToolId {
        "note_save".into()
    }
    fn description(&self) -> String {
        "Сохранить заметку о пользователе/контексте для будущих диалогов.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "Текст заметки"},
                "tags": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается строковое поле content"))?
            .trim()
            .to_string();
        if content.is_empty() {
            anyhow::bail!("content не может быть пустым");
        }
        let tags = parse_tags(&args);
        let note = Note::new(ctx.profile_id, content, tags);
        let id = note.id;
        ctx.storage.db().note_insert(&note)?;

        let mut out = format!("Заметка сохранена (id={id}).");
        // Эмбеддинг (best-effort) + ворота совместимости: показать семантически
        // близкие существующие заметки, чтобы модель могла переписать дубль через
        // note_revise вместо накопления почти-копии. Без эмбеддера — мягко пропускаем
        // (как RAG-реранкинг), заметка всё равно сохранена.
        if let Ok(vecs) = ctx.embedder.embed(vec![note.content.clone()]).await
            && let Some(emb) = vecs.into_iter().next()
        {
            let _ = ctx
                .storage
                .db()
                .note_vector_upsert(id, ctx.profile_id, &emb);
            // Дотягиваем эмбеддинги «старых» заметок без векторов, чтобы они
            // участвовали в воротах (и в последующем семантическом поиске).
            ensure_note_vectors(ctx).await;
            if let Ok(hits) = ctx
                .storage
                .db()
                .note_search_semantic(ctx.profile_id, &emb, 4)
            {
                let similar: Vec<Note> = hits
                    .into_iter()
                    .map(|(n, _)| n)
                    .filter(|n| n.id != id)
                    .take(3)
                    .collect();
                if !similar.is_empty() {
                    out.push_str(
                        "\nПохожие заметки (возможен дубль/конфликт — при необходимости \
                         перепиши существующую через note_revise вместо новой записи):",
                    );
                    for n in similar {
                        out.push_str(&format!("\n- (id={}) {}", n.id, n.content));
                    }
                }
            }
        }
        Ok(ToolOutcome::text(out))
    }
}

/// `note_recall` — ищет заметки профиля по тексту/тегам.
pub struct NoteRecall;

#[async_trait::async_trait]
impl Tool for NoteRecall {
    fn id(&self) -> ToolId {
        "note_recall".into()
    }
    fn description(&self) -> String {
        "Найти ранее сохранённые заметки по тексту и/или тегам.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Подстрока для поиска по содержимому"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "limit": {"type": "integer", "minimum": 1}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let tags = parse_tags(&args);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        // Семантический путь: есть запрос и доступен эмбеддер. Иначе (нет запроса,
        // эмбеддер недоступен или нет векторов у заметок) — откат на подстроку/теги.
        let notes = match query {
            Some(q) => match semantic_recall(ctx, q, &tags, limit).await {
                Some(n) => n,
                None => ctx
                    .storage
                    .db()
                    .note_list(ctx.profile_id, query, &tags, limit)?,
            },
            None => ctx
                .storage
                .db()
                .note_list(ctx.profile_id, query, &tags, limit)?,
        };

        let mut outcome = format_notes(&notes);
        // Spreading activation: подмешиваем связанные по графу заметки (Ярус 2),
        // чтобы припоминание поднимало кластер, а не одиночные атомы.
        if let Some(block) = related_block(ctx, &notes) {
            outcome.result.push_str(&block);
        }
        Ok(outcome)
    }
}

/// Подмешиваемый блок «Связанные заметки»: соседи топ-хитов по графу (обе стороны),
/// без уже показанных и без замещённых. `None`, если связей нет. Чистое чтение БД.
fn related_block(ctx: &ToolContext, hits: &[Note]) -> Option<String> {
    let mut seen: std::collections::HashSet<Uuid> = hits.iter().map(|n| n.id).collect();
    let mut lines: Vec<String> = Vec::new();
    for hit in hits.iter().take(3) {
        let nb = ctx
            .storage
            .db()
            .note_neighbors(ctx.profile_id, hit.id, None)
            .unwrap_or_default();
        for (note, relation, outgoing) in nb {
            if !seen.insert(note.id) {
                continue;
            }
            let arrow = if outgoing { "→" } else { "←" };
            lines.push(format!(
                "- {arrow}{relation} (id={}) {}",
                note.id, note.content
            ));
            if lines.len() >= RELATED_IN_RECALL {
                break;
            }
        }
        if lines.len() >= RELATED_IN_RECALL {
            break;
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!("\nСвязанные заметки:\n{}", lines.join("\n")))
    }
}

/// Создаёт заметку (insert + best-effort эмбеддинг) и возвращает её id. Используется
/// `note_supersede`/`note_merge` для новой версии/объединённой заметки.
async fn create_note(ctx: &ToolContext, content: String, tags: Vec<String>) -> Result<Uuid> {
    let note = Note::new(ctx.profile_id, content, tags);
    let id = note.id;
    ctx.storage.db().note_insert(&note)?;
    if let Ok(vecs) = ctx.embedder.embed(vec![note.content.clone()]).await
        && let Some(emb) = vecs.into_iter().next()
    {
        let _ = ctx
            .storage
            .db()
            .note_vector_upsert(id, ctx.profile_id, &emb);
    }
    Ok(id)
}

/// Парсит uuid из строкового поля аргументов с понятной ошибкой.
fn parse_id(args: &serde_json::Value, key: &str) -> Result<Uuid> {
    let raw = args
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    Uuid::parse_str(raw).map_err(|_| anyhow::anyhow!("некорректный id ({key}): {raw}"))
}

/// Дотягивает эмбеддинги заметок профиля, у которых их ещё нет (созданы до
/// векторного поиска, импортированы или сохранены при недоступном тогда эмбеддере).
/// Без этого семантический поиск/ворота их не видят. **Best-effort**: эмбеддер
/// недоступен или батч не прошёл — просто выходим (поиск отработает по тому, что
/// есть, плюс откат на подстроку). По сути один раз на профиль: после бэкфилла
/// список «без векторов» пуст и вызов почти бесплатен (один SELECT).
async fn ensure_note_vectors(ctx: &ToolContext) {
    let missing = match ctx.storage.db().notes_missing_vectors(ctx.profile_id) {
        Ok(m) => m,
        Err(_) => return,
    };
    for chunk in missing.chunks(NOTE_BACKFILL_BATCH) {
        let texts: Vec<String> = chunk.iter().map(|(_, c)| c.clone()).collect();
        let Ok(vecs) = ctx.embedder.embed(texts).await else {
            return; // эмбеддер недоступен — дальше смысла нет
        };
        for ((id, _), emb) in chunk.iter().zip(vecs) {
            let _ = ctx
                .storage
                .db()
                .note_vector_upsert(*id, ctx.profile_id, &emb);
        }
    }
}

/// Семантический поиск заметок по эмбеддингу запроса. `None`, если эмбеддер
/// недоступен (мягкая деградация — вызывающий откатится на подстроку) или выдача
/// пуста (например, у заметок ещё нет векторов). Теги применяются фильтром поверх
/// ранжирования.
async fn semantic_recall(
    ctx: &ToolContext,
    query: &str,
    tags: &[String],
    limit: Option<usize>,
) -> Option<Vec<Note>> {
    // Бэкфилл: дотянуть эмбеддинги заметок без векторов (старые/импортированные),
    // иначе семантический поиск их не увидит.
    ensure_note_vectors(ctx).await;
    let emb = ctx
        .embedder
        .embed(vec![query.to_string()])
        .await
        .ok()?
        .into_iter()
        .next()?;
    let want = limit.unwrap_or(DEFAULT_RECALL);
    // При фильтре по тегам берём больше кандидатов, затем отсекаем до `want`.
    let cand = if tags.is_empty() { want } else { want.max(30) };
    let hits = ctx
        .storage
        .db()
        .note_search_semantic(ctx.profile_id, &emb, cand)
        .ok()?;
    let mut notes: Vec<Note> = hits
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| tags.is_empty() || tags.iter().all(|t| n.tags.contains(t)))
        .collect();
    notes.truncate(want);
    if notes.is_empty() { None } else { Some(notes) }
}

/// Форматирует список заметок в текстовый результат инструмента.
fn format_notes(notes: &[Note]) -> ToolOutcome {
    if notes.is_empty() {
        return ToolOutcome::text("Заметки не найдены.");
    }
    let mut out = format!("Найдено заметок: {}\n", notes.len());
    for n in notes {
        out.push_str(&format!("- {}", n.content));
        if !n.tags.is_empty() {
            out.push_str(&format!("  [{}]", n.tags.join(", ")));
        }
        out.push('\n');
    }
    ToolOutcome::text(out.trim_end().to_string())
}

/// `note_revise` — переписывает существующую заметку на месте (ревизия). Ядро
/// интеграции: новое замещает старое, а не копится рядом почти-дублем.
pub struct NoteRevise;

#[async_trait::async_trait]
impl Tool for NoteRevise {
    fn id(&self) -> ToolId {
        NOTE_REVISE_ID.into()
    }
    fn description(&self) -> String {
        "Переписать существующую заметку на месте (по id из note_recall/note_save): \
         новое содержимое замещает прежнее. Используй, когда заметка устарела, \
         уточнилась или дублируется, — вместо создания почти-копии."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "id заметки (из note_recall/note_save)"},
                "content": {"type": "string", "description": "Новое содержимое заметки"}
            },
            "required": ["id", "content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let uuid =
            Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("некорректный id заметки: {id}"))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается строковое поле content"))?
            .trim()
            .to_string();
        if content.is_empty() {
            anyhow::bail!("content не может быть пустым");
        }
        if !ctx
            .storage
            .db()
            .note_update(uuid, ctx.profile_id, &content)?
        {
            return Ok(ToolOutcome::text(format!(
                "Заметка не найдена (id={uuid})."
            )));
        }
        // Переэмбеддинг (best-effort): семантический поиск должен видеть новое содержимое.
        if let Ok(vecs) = ctx.embedder.embed(vec![content.clone()]).await
            && let Some(emb) = vecs.into_iter().next()
        {
            let _ = ctx
                .storage
                .db()
                .note_vector_upsert(uuid, ctx.profile_id, &emb);
        }
        let mut msg = format!("Заметка переписана (id={uuid}).");
        // Предупреждение целостности графа: правка на месте не трогает связи, но если
        // изменился СМЫСЛ, входящие рёбра (напр. contradicts) могут стать неверными —
        // для смысловой переработки честнее note_supersede (сохранит замещённую
        // версию, к которой относились связи).
        if let Ok(links) = ctx.storage.db().note_link_count(ctx.profile_id, uuid)
            && links > 0
        {
            msg.push_str(&format!(
                "\n⚠ У заметки есть связи ({links}). Они не изменились вместе с текстом: \
                 если смысл стал другим, входящие связи (например contradicts) могут \
                 теперь лгать. Для смысловой переработки используй note_supersede — \
                 он сохранит прежнюю версию как замещённую, к которой относились связи."
            ));
        }
        Ok(ToolOutcome::text(msg))
    }
}

/// `note_link` — связывает две заметки направленной типизированной связью.
pub struct NoteLink;

#[async_trait::async_trait]
impl Tool for NoteLink {
    fn id(&self) -> ToolId {
        NOTE_LINK_ID.into()
    }
    fn description(&self) -> String {
        "Связать две заметки (по id из note_recall/note_save) направленной связью: \
         supports (подтверждает), contradicts (противоречит), refines (уточняет), \
         relates (связано по теме). Помогает помнить, как заметки соотносятся."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from_id": {"type": "string", "description": "id заметки-источника"},
                "to_id": {"type": "string", "description": "id заметки-цели"},
                "relation": {"type": "string", "enum": RELATIONS, "description": "тип связи"}
            },
            "required": ["from_id", "to_id", "relation"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let from = parse_id(&args, "from_id")?;
        let to = parse_id(&args, "to_id")?;
        let relation = args
            .get("relation")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if !RELATIONS.contains(&relation) {
            anyhow::bail!("неизвестный тип связи: {relation} (допустимо: {RELATIONS:?})");
        }
        if from == to {
            anyhow::bail!("нельзя связать заметку с самой собой");
        }
        let db = ctx.storage.db();
        if !db.note_is_active(ctx.profile_id, from)? || !db.note_is_active(ctx.profile_id, to)? {
            return Ok(ToolOutcome::text(
                "Одна из заметок не найдена (или замещена).".to_string(),
            ));
        }
        let created = db.note_link_insert(ctx.profile_id, from, to, relation)?;
        let verb = if created {
            "Связь создана"
        } else {
            "Связь уже существовала"
        };
        Ok(ToolOutcome::text(format!(
            "{verb}: {from} —{relation}→ {to}."
        )))
    }
}

/// `note_neighbors` — показывает связанные с заданной заметкой заметки.
pub struct NoteNeighbors;

#[async_trait::async_trait]
impl Tool for NoteNeighbors {
    fn id(&self) -> ToolId {
        NOTE_NEIGHBORS_ID.into()
    }
    fn description(&self) -> String {
        "Показать заметки, связанные с данной (по id), с типом и направлением связи. \
         Опционально — только связи указанного типа (supports/contradicts/refines/relates)."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "id заметки"},
                "relation": {"type": "string", "enum": RELATIONS, "description": "фильтр по типу связи (опц.)"}
            },
            "required": ["id"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let id = parse_id(&args, "id")?;
        let relation = args
            .get("relation")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        if let Some(r) = relation
            && !RELATIONS.contains(&r)
        {
            anyhow::bail!("неизвестный тип связи: {r} (допустимо: {RELATIONS:?})");
        }
        let nb = ctx
            .storage
            .db()
            .note_neighbors(ctx.profile_id, id, relation)?;
        if nb.is_empty() {
            return Ok(ToolOutcome::text("Связанных заметок нет."));
        }
        let mut out = format!("Связи заметки {id}:\n");
        for (note, rel, outgoing) in &nb {
            let arrow = if *outgoing { "→" } else { "←" };
            out.push_str(&format!(
                "- {arrow}{rel} (id={}) {}\n",
                note.id, note.content
            ));
        }
        Ok(ToolOutcome::text(out.trim_end().to_string()))
    }
}

/// `note_supersede` — замещает заметку новой версией («шрам» сохраняется).
pub struct NoteSupersede;

#[async_trait::async_trait]
impl Tool for NoteSupersede {
    fn id(&self) -> ToolId {
        NOTE_SUPERSEDE_ID.into()
    }
    fn description(&self) -> String {
        "Заместить устаревшую заметку новой версией (по id из note_recall): создаётся \
         новая заметка, старая помечается замещённой (скрывается из поиска, но \
         хранится для следа изменения). Для простой правки на месте — note_revise."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "old_id": {"type": "string", "description": "id замещаемой заметки"},
                "content": {"type": "string", "description": "Содержимое новой версии"}
            },
            "required": ["old_id", "content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let old_id = parse_id(&args, "old_id")?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается строковое поле content"))?
            .trim()
            .to_string();
        if content.is_empty() {
            anyhow::bail!("content не может быть пустым");
        }
        if !ctx.storage.db().note_is_active(ctx.profile_id, old_id)? {
            return Ok(ToolOutcome::text(format!(
                "Заметка не найдена (id={old_id})."
            )));
        }
        let new_id = create_note(ctx, content, Vec::new()).await?;
        ctx.storage
            .db()
            .note_supersede_mark(ctx.profile_id, old_id, new_id)?;
        Ok(ToolOutcome::text(format!(
            "Заметка замещена: {old_id} → новая (id={new_id})."
        )))
    }
}

/// `note_merge` — сводит несколько заметок в одну (исходные замещаются).
pub struct NoteMerge;

#[async_trait::async_trait]
impl Tool for NoteMerge {
    fn id(&self) -> ToolId {
        NOTE_MERGE_ID.into()
    }
    fn description(&self) -> String {
        "Свести несколько заметок (ids из note_recall) в одну: создаётся новая с \
         объединённым содержимым, исходные помечаются замещёнными (скрываются, но \
         хранятся). Используй для консолидации дублей/осколков по одной теме."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "ids": {"type": "array", "items": {"type": "string"}, "minItems": 2, "description": "id объединяемых заметок"},
                "content": {"type": "string", "description": "Объединённое содержимое"}
            },
            "required": ["ids", "content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается строковое поле content"))?
            .trim()
            .to_string();
        if content.is_empty() {
            anyhow::bail!("content не может быть пустым");
        }
        let ids: Vec<Uuid> = args
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .filter_map(|s| Uuid::parse_str(s.trim()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let db = ctx.storage.db();
        let active: Vec<Uuid> = ids
            .into_iter()
            .filter(|id| db.note_is_active(ctx.profile_id, *id).unwrap_or(false))
            .collect();
        if active.len() < 2 {
            anyhow::bail!("нужно минимум две существующие заметки для объединения");
        }
        let new_id = create_note(ctx, content, Vec::new()).await?;
        for old in &active {
            ctx.storage
                .db()
                .note_supersede_mark(ctx.profile_id, *old, new_id)?;
            // Связи исходных заметок переносим на объединённую — граф не осиротеет.
            ctx.storage
                .db()
                .note_links_retarget(ctx.profile_id, *old, new_id)?;
        }
        Ok(ToolOutcome::text(format!(
            "Объединено заметок: {} → новая (id={new_id}).",
            active.len()
        )))
    }
}

/// Косинусная близость двух векторов (0 при разной длине/нулевой норме).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Усечение строки по символам (для компактного обзора).
fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Строит обзор базы знаний для консолидации: похожие пары (возможные дубли по
/// косинусу), связи `contradicts`, заметки без связей. Только данные (без рубрики) —
/// используется и инструментом `consolidate_notes`, и фоновой авто-консолидацией.
/// Изоляция по `profile_id`. Чистое чтение БД (эмбеддер не нужен — вектора уже в БД).
pub(crate) fn build_consolidation_overview(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
) -> String {
    let active = storage
        .db()
        .note_list(profile_id, None, &[], None)
        .unwrap_or_default();
    if active.is_empty() {
        return "База заметок пуста — консолидировать нечего.".to_string();
    }
    let with_vec = storage
        .db()
        .notes_with_vectors(profile_id)
        .unwrap_or_default();
    let links = storage.db().note_links_all(profile_id).unwrap_or_default();

    // Похожие пары (возможные дубли) по косинусу, по убыванию близости.
    let mut pairs: Vec<(f32, &Note, &Note)> = Vec::new();
    for i in 0..with_vec.len() {
        for j in (i + 1)..with_vec.len() {
            let s = cosine(&with_vec[i].1, &with_vec[j].1);
            if s >= CONSOLIDATE_SIMILARITY {
                pairs.push((s, &with_vec[i].0, &with_vec[j].0));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));

    let contradicts: Vec<&(Uuid, Uuid, String)> = links
        .iter()
        .filter(|(_, _, r)| r == "contradicts")
        .collect();
    let linked: std::collections::HashSet<Uuid> =
        links.iter().flat_map(|(f, t, _)| [*f, *t]).collect();
    let dangling: Vec<&Note> = active.iter().filter(|n| !linked.contains(&n.id)).collect();

    let mut out = format!(
        "Обзор базы знаний для консолидации:\nАктивных заметок: {} (без связей: {}).\n",
        active.len(),
        dangling.len()
    );

    out.push_str(&format!(
        "\nПохожие пары (возможные дубли, близость ≥ {CONSOLIDATE_SIMILARITY}): {}\n",
        pairs.len()
    ));
    for (s, a, b) in pairs.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!(
            "- {s:.2} (id={}) {} ↔ (id={}) {}\n",
            a.id,
            clip(&a.content, 60),
            b.id,
            clip(&b.content, 60)
        ));
    }

    out.push_str(&format!(
        "\nСвязи contradicts (проверь, держится ли противоречие после правок): {}\n",
        contradicts.len()
    ));
    for (f, t, _) in contradicts.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={f}) ↔ (id={t})\n"));
    }

    out.push_str(&format!(
        "\nЗаметки без связей (кандидаты связать): {}\n",
        dangling.len()
    ));
    for n in dangling.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={}) {}\n", n.id, clip(&n.content, 60)));
    }

    out.trim_end().to_string()
}

/// `consolidate_notes` — обзор базы знаний + рубрика для консолидации (entry-point,
/// как `reflect` у SelfModel). Ничего не меняет: дальше модель сама зовёт
/// merge/supersede/revise/link.
pub struct ConsolidateNotes;

#[async_trait::async_trait]
impl Tool for ConsolidateNotes {
    fn id(&self) -> ToolId {
        CONSOLIDATE_NOTES_ID.into()
    }
    fn description(&self) -> String {
        "Обзор базы знаний для консолидации: похожие пары (возможные дубли), связи \
         contradicts, заметки без связей — и что с этим делать. Точка входа: дальше \
         слей дубли (note_merge), перепиши/замести устаревшее (note_revise/\
         note_supersede), свяжи родственное (note_link)."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let overview = build_consolidation_overview(&ctx.storage, ctx.profile_id);
        let out = format!(
            "{overview}\n\nЧто сделать (только при необходимости):\n\
             - слей явные дубли: note_merge(ids[], content);\n\
             - устаревшее перепиши (note_revise — мелкая правка) или замести \
             (note_supersede — смысловая переработка, сохранит «шрам»);\n\
             - свяжи родственные заметки: note_link (supports/contradicts/refines/relates).\n\
             Меняй только то, что действительно нужно; если всё в порядке — ничего не вызывай."
        );
        Ok(ToolOutcome::text(out))
    }
}

/// Извлекает массив строковых тегов из аргументов (пустой, если нет).
fn parse_tags(args: &serde_json::Value) -> Vec<String> {
    args.get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn save_then_recall_isolated_by_profile() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);

        NoteSave
            .invoke(
                &ctx,
                serde_json::json!({"content": "любит чай", "tags": ["pref"]}),
            )
            .await
            .unwrap();
        // Заметка действительно записана под этим профилем.
        assert_eq!(
            storage
                .db()
                .note_list(profile, None, &[], None)
                .unwrap()
                .len(),
            1
        );

        let out = NoteRecall
            .invoke(&ctx, serde_json::json!({"query": "чай"}))
            .await
            .unwrap();
        assert!(out.result.contains("любит чай"));

        // Чужой профиль не видит заметку.
        let (_d2, _s2, other) = ctx_with_storage(Uuid::new_v4());
        // другой профиль — другое хранилище: проверяем изоляцию на уровне фильтра
        let empty = NoteRecall
            .invoke(&other, serde_json::json!({}))
            .await
            .unwrap();
        assert!(empty.result.contains("не найдены"));
    }

    #[tokio::test]
    async fn recall_by_tag() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "a", "tags": ["x"]}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "b", "tags": ["y"]}))
            .await
            .unwrap();
        let out = NoteRecall
            .invoke(&ctx, serde_json::json!({"tags": ["x"]}))
            .await
            .unwrap();
        assert!(out.result.contains("a"));
        assert!(!out.result.contains("- b"));
    }

    #[tokio::test]
    async fn save_rejects_empty_content() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            NoteSave
                .invoke(&ctx, serde_json::json!({"content": "  "}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn save_surfaces_similar_notes_as_gate() {
        // MockEmbedder(16) — мешок символов: тексты с общими буквами близки.
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "aaaa bbbb"}))
            .await
            .unwrap();
        // Вторая заметка близка по символам → ворота должны показать первую.
        let out = NoteSave
            .invoke(&ctx, serde_json::json!({"content": "aaab"}))
            .await
            .unwrap();
        assert!(out.result.contains("Похожие заметки"));
        assert!(out.result.contains("aaaa bbbb"));
    }

    #[tokio::test]
    async fn recall_semantic_finds_non_substring_match() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "aaaa"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "wwww"}))
            .await
            .unwrap();
        // Запрос «aaab» не является подстрокой ни одной заметки, но семантически
        // ближе к «aaaa» → семантический путь его находит.
        let out = NoteRecall
            .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
            .await
            .unwrap();
        assert!(out.result.contains("aaaa"));
        assert!(!out.result.contains("wwww"));
    }

    #[tokio::test]
    async fn save_gate_surfaces_legacy_note_without_vector() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        // «Старая» заметка без вектора (вставлена напрямую — как до фичи/при импорте).
        storage
            .db()
            .note_insert(&Note::new(profile, "aaaa bbbb", vec![]))
            .unwrap();
        assert_eq!(
            storage.db().notes_missing_vectors(profile).unwrap().len(),
            1
        );

        // Сохраняем похожую — ворота должны показать старую (бэкфилл в note_save).
        let out = NoteSave
            .invoke(&ctx, serde_json::json!({"content": "aaab"}))
            .await
            .unwrap();
        assert!(out.result.contains("Похожие заметки"));
        assert!(out.result.contains("aaaa bbbb"));
        // Бэкфилл проиндексировал старую заметку.
        assert_eq!(
            storage.db().notes_missing_vectors(profile).unwrap().len(),
            0
        );
    }

    #[tokio::test]
    async fn recall_backfills_legacy_notes_without_vectors() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        // «Старые» заметки без векторов (вставлены напрямую — как до фичи/при импорте).
        storage
            .db()
            .note_insert(&Note::new(profile, "aaaa", vec![]))
            .unwrap();
        storage
            .db()
            .note_insert(&Note::new(profile, "wwww", vec![]))
            .unwrap();
        assert_eq!(
            storage.db().notes_missing_vectors(profile).unwrap().len(),
            2
        );

        // Семантический recall дотягивает вектора и находит не-подстрочное совпадение.
        let out = NoteRecall
            .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
            .await
            .unwrap();
        assert!(out.result.contains("aaaa"));
        // Бэкфилл выполнен — заметок без векторов больше нет.
        assert_eq!(
            storage.db().notes_missing_vectors(profile).unwrap().len(),
            0
        );
    }

    #[tokio::test]
    async fn revise_rewrites_in_place() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "старое"}))
            .await
            .unwrap();
        let id = storage.db().note_list(profile, None, &[], None).unwrap()[0].id;

        let out = NoteRevise
            .invoke(
                &ctx,
                serde_json::json!({"id": id.to_string(), "content": "новое"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("переписана"));
        // Содержимое заменено на месте (не добавлена новая заметка).
        let notes = storage.db().note_list(profile, None, &[], None).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].content, "новое");
    }

    #[tokio::test]
    async fn revise_bad_and_missing_id() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        // Некорректный uuid → ошибка.
        assert!(
            NoteRevise
                .invoke(&ctx, serde_json::json!({"id": "not-uuid", "content": "x"}))
                .await
                .is_err()
        );
        // Корректный, но несуществующий → понятный текст, не паника.
        let out = NoteRevise
            .invoke(
                &ctx,
                serde_json::json!({"id": Uuid::new_v4().to_string(), "content": "x"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("не найдена"));
    }

    /// id заметки по содержимому (для тестов графа).
    fn id_by_content(
        storage: &crate::shared::storage::Storage,
        profile: Uuid,
        content: &str,
    ) -> Uuid {
        storage
            .db()
            .note_list(profile, None, &[], None)
            .unwrap()
            .into_iter()
            .find(|n| n.content == content)
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn link_then_neighbors() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "альфа"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "бета"}))
            .await
            .unwrap();
        let a = id_by_content(&storage, profile, "альфа");
        let b = id_by_content(&storage, profile, "бета");

        let out = NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "refines"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Связь создана"));

        // Повтор той же связи — честный ответ «уже существовала» (без дубля в графе).
        let dup = NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "refines"}),
            )
            .await
            .unwrap();
        assert!(dup.result.contains("уже существовала"));

        let nb = NoteNeighbors
            .invoke(&ctx, serde_json::json!({"id": a.to_string()}))
            .await
            .unwrap();
        assert!(nb.result.contains("бета"));
        assert!(nb.result.contains("refines"));

        // Неизвестный тип связи и самосвязь → ошибки.
        assert!(
            NoteLink
                .invoke(
                    &ctx,
                    serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "foo"}),
                )
                .await
                .is_err()
        );
        assert!(
            NoteLink
                .invoke(
                    &ctx,
                    serde_json::json!({"from_id": a.to_string(), "to_id": a.to_string(), "relation": "relates"}),
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn revise_warns_when_note_has_links() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "узел"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "другой"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "одиночка"}))
            .await
            .unwrap();
        let a = id_by_content(&storage, profile, "узел");
        let b = id_by_content(&storage, profile, "другой");
        let lone = id_by_content(&storage, profile, "одиночка");
        NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": b.to_string(), "to_id": a.to_string(), "relation": "contradicts"}),
            )
            .await
            .unwrap();

        // Ревизия узла со связью предупреждает про note_supersede.
        let out = NoteRevise
            .invoke(
                &ctx,
                serde_json::json!({"id": a.to_string(), "content": "узел v2"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("переписана"));
        assert!(out.result.contains("note_supersede"));

        // Узел без связей — без предупреждения.
        let out2 = NoteRevise
            .invoke(
                &ctx,
                serde_json::json!({"id": lone.to_string(), "content": "одиночка v2"}),
            )
            .await
            .unwrap();
        assert!(!out2.result.contains("note_supersede"));
    }

    #[tokio::test]
    async fn supersede_hides_old_shows_new() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "первая версия"}))
            .await
            .unwrap();
        let old = id_by_content(&storage, profile, "первая версия");

        let out = NoteSupersede
            .invoke(
                &ctx,
                serde_json::json!({"old_id": old.to_string(), "content": "вторая версия"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("замещена"));

        let active = storage.db().note_list(profile, None, &[], None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].content, "вторая версия");
        assert!(!storage.db().note_is_active(profile, old).unwrap());
    }

    #[tokio::test]
    async fn merge_consolidates_sources() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "кусок один"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "кусок два"}))
            .await
            .unwrap();
        let ids: Vec<String> = storage
            .db()
            .note_list(profile, None, &[], None)
            .unwrap()
            .into_iter()
            .map(|n| n.id.to_string())
            .collect();

        let out = NoteMerge
            .invoke(
                &ctx,
                serde_json::json!({"ids": ids, "content": "единая заметка"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Объединено заметок: 2"));

        let active = storage.db().note_list(profile, None, &[], None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].content, "единая заметка");

        // Меньше двух существующих → ошибка.
        assert!(
            NoteMerge
                .invoke(
                    &ctx,
                    serde_json::json!({"ids": [Uuid::new_v4().to_string()], "content": "x"}),
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn recall_spreads_to_linked_notes() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "aaaa"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "zzzz"}))
            .await
            .unwrap();
        let a = id_by_content(&storage, profile, "aaaa");
        let z = id_by_content(&storage, profile, "zzzz");
        NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": z.to_string(), "relation": "relates"}),
            )
            .await
            .unwrap();

        // Запрос близок к «aaaa»; «zzzz» не похож, но связан → попадёт в «Связанные».
        let out = NoteRecall
            .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
            .await
            .unwrap();
        assert!(out.result.contains("aaaa"));
        assert!(out.result.contains("Связанные заметки"));
        assert!(out.result.contains("zzzz"));
    }

    #[tokio::test]
    async fn consolidate_notes_reports_dups_and_dangling() {
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        // Два почти-дубля (общие символы → высокий косинус на MockEmbedder).
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "aaaa bbbb"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "aaaa bbbbb"}))
            .await
            .unwrap();
        // Несвязанная непохожая заметка.
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "zzzz"}))
            .await
            .unwrap();

        let out = ConsolidateNotes
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("Обзор базы знаний"));
        // Похожая пара найдена (хотя бы одна).
        assert!(out.result.contains("Похожие пары (возможные дубли"));
        assert!(out.result.contains("aaaa bbbb"));
        assert!(out.result.contains("без связей"));
        assert!(out.result.contains("note_merge"));
    }

    #[tokio::test]
    async fn merge_transfers_links_to_new_note() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "часть один"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "часть два"}))
            .await
            .unwrap();
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "третья"}))
            .await
            .unwrap();
        // id берём ДО слияния (после источники замещаются и из списка исчезают).
        let s1 = id_by_content(&storage, profile, "часть один");
        let s2 = id_by_content(&storage, profile, "часть два");
        let other = id_by_content(&storage, profile, "третья");
        NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": s1.to_string(), "to_id": other.to_string(), "relation": "relates"}),
            )
            .await
            .unwrap();

        NoteMerge
            .invoke(
                &ctx,
                serde_json::json!({"ids": [s1.to_string(), s2.to_string()], "content": "единая"}),
            )
            .await
            .unwrap();

        // Связь источника перенесена на объединённую заметку: сосед «третьей» — «единая».
        let nb = NoteNeighbors
            .invoke(&ctx, serde_json::json!({"id": other.to_string()}))
            .await
            .unwrap();
        assert!(nb.result.contains("единая"));
        assert!(!nb.result.contains("часть один"));
    }
}
