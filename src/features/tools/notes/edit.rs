//! Заметки — note_revise / note_supersede / note_merge (правка/замещение/слияние). Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/history/refactoring-god-objects.md, этап 4).

use super::*;

/// `note_revise` — переписывает существующую заметку на месте (ревизия). Ядро
/// интеграции: новое замещает старое, а не копится рядом почти-дублем.
pub struct NoteRevise;

#[async_trait::async_trait]
impl Tool for NoteRevise {
    fn id(&self) -> ToolId {
        NOTE_REVISE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "переписать заметку"
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

/// `note_supersede` — замещает заметку новой версией («шрам» сохраняется).
pub struct NoteSupersede;

#[async_trait::async_trait]
impl Tool for NoteSupersede {
    fn id(&self) -> ToolId {
        NOTE_SUPERSEDE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "заместить заметку"
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
        // Новая версия наследует теги замещаемой (в т.ч. @self — иначе self-заметка
        // при замещении «выпала» бы в пользовательскую выдачу).
        let tags = ctx
            .storage
            .db()
            .note_get(ctx.profile_id, old_id)?
            .map(|n| n.tags)
            .unwrap_or_default();
        let new_id = create_note(ctx, content, tags).await?;
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
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "слить заметки"
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
        // Объединённая заметка наследует union тегов исходных (в т.ч. @self —
        // слияние self-заметок остаётся self-заметкой, скрытой из recall).
        let mut tags: Vec<String> = Vec::new();
        for old in &active {
            if let Ok(Some(n)) = ctx.storage.db().note_get(ctx.profile_id, *old) {
                for t in n.tags {
                    if !tags.contains(&t) {
                        tags.push(t);
                    }
                }
            }
        }
        let new_id = create_note(ctx, content, tags).await?;
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
