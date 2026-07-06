//! Заметки — note_link / note_neighbors (типизированный граф связей). Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/refactoring-god-objects.md, этап 4).

use super::*;

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
