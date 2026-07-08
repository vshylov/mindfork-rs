//! Заметки — note_save + запись заметки, ворота похожести, эмбеддинг. Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/refactoring-god-objects.md, этап 4).

use super::*;

/// `note_save` — сохраняет заметку профиля. Возвращает её id.
pub struct NoteSave;

#[async_trait::async_trait]
impl Tool for NoteSave {
    fn id(&self) -> ToolId {
        "note_save".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "сохранить заметку"
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
            ensure_note_vectors(&ctx.storage, ctx.embedder.as_ref(), ctx.profile_id).await;
            if let Ok(hits) = ctx
                .storage
                .db()
                .note_search_semantic(ctx.profile_id, &emb, 4)
            {
                let similar: Vec<Note> = hits
                    .into_iter()
                    .map(|(n, _)| n)
                    // Исключаем только что созданную и self-заметки (@self) — обычная
                    // запись не должна натыкаться на наблюдения «модели себя».
                    .filter(|n| n.id != id && !is_self_note(n))
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

/// Создаёт заметку (insert + best-effort эмбеддинг) и возвращает её id. Используется
/// `note_supersede`/`note_merge` для новой версии/объединённой заметки, а также
/// инструментами «модели себя» для self-заметок (тег [`SELF_NOTE_TAG`]).
pub(crate) async fn create_note(
    ctx: &ToolContext,
    content: String,
    tags: Vec<String>,
) -> Result<Uuid> {
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

/// Семантически близкие self-заметки к `content` — ворота инструмента `add_insight`:
/// дотягивает вектора self-заметок (бэкфилл), эмбеддит запрос, ищет среди
/// self-заметок, исключает `exclude`. Пусто при недоступном эмбеддере (мягкая
/// деградация) — прямое зеркало ворот `note_save`, но над памятью «о себе».
pub(crate) async fn self_note_similar(
    ctx: &ToolContext,
    content: &str,
    exclude: Uuid,
) -> Vec<Note> {
    ensure_note_vectors(&ctx.storage, ctx.embedder.as_ref(), ctx.profile_id).await;
    let Ok(vecs) = ctx.embedder.embed(vec![content.to_string()]).await else {
        return Vec::new();
    };
    let Some(emb) = vecs.into_iter().next() else {
        return Vec::new();
    };
    let Ok(hits) = ctx
        .storage
        .db()
        .note_search_semantic(ctx.profile_id, &emb, 8)
    else {
        return Vec::new();
    };
    hits.into_iter()
        .map(|(n, _)| n)
        .filter(|n| n.id != exclude && is_self_note(n))
        .take(3)
        .collect()
}

/// Дотягивает эмбеддинги заметок профиля, у которых их ещё нет (созданы до
/// векторного поиска, импортированы или сохранены при недоступном тогда эмбеддере).
/// Без этого семантический поиск/ворота их не видят. **Best-effort**: эмбеддер
/// недоступен или батч не прошёл — просто выходим (поиск отработает по тому, что
/// есть, плюс откат на подстроку). По сути один раз на профиль: после бэкфилла
/// список «без векторов» пуст и вызов почти бесплатен (один SELECT).
pub(crate) async fn ensure_note_vectors(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
) {
    let missing = match storage.db().notes_missing_vectors(profile_id) {
        Ok(m) => m,
        Err(_) => return,
    };
    for chunk in missing.chunks(NOTE_BACKFILL_BATCH) {
        let texts: Vec<String> = chunk.iter().map(|(_, c)| c.clone()).collect();
        let Ok(vecs) = embedder.embed(texts).await else {
            return; // эмбеддер недоступен — дальше смысла нет
        };
        for ((id, _), emb) in chunk.iter().zip(vecs) {
            let _ = storage.db().note_vector_upsert(*id, profile_id, &emb);
        }
    }
}
