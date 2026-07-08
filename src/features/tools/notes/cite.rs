//! Заметки — note_cite_source (ссылка заметки на источник RAG). Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/history/refactoring-god-objects.md, этап 4).

use super::*;

/// `note_cite_source` — связывает заметку с RAG-источником (Ярус 3, Путь 3: связывание
/// органов памяти). Ссылка ведётся на **имя источника** (стабильно к переиндексации, в
/// отличие от id чанков). Источник должен реально существовать в базе знаний профиля.
pub struct NoteCiteSource;

#[async_trait::async_trait]
impl Tool for NoteCiteSource {
    fn id(&self) -> ToolId {
        NOTE_CITE_SOURCE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "сослаться на источник"
    }
    fn description(&self) -> String {
        "Связать заметку (по id из note_recall/note_save) с источником из базы знаний \
         (RAG): указывает, что заметка/наблюдение опирается на этот источник. Имя \
         источника — как в выдаче rag_search (в квадратных скобках). Потом при \
         припоминании заметки виден её источник, а поиск rag_search показывает \
         ссылающиеся заметки."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "note_id": {"type": "string", "description": "id заметки (из note_recall/note_save)"},
                "source": {"type": "string", "description": "имя источника из базы знаний (как в rag_search)"}
            },
            "required": ["note_id", "source"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let note_id = parse_id(&args, "note_id")?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if source.is_empty() {
            anyhow::bail!("ожидается непустое поле source");
        }
        let db = ctx.storage.db();
        if !db.note_is_active(ctx.profile_id, note_id)? {
            return Ok(ToolOutcome::text(format!(
                "Заметка не найдена (или замещена) (id={note_id})."
            )));
        }
        if !db.rag_source_exists(ctx.profile_id, &source)? {
            return Ok(ToolOutcome::text(format!(
                "Источник «{source}» не найден в базе знаний. Проверь имя (как в rag_search)."
            )));
        }
        let created = db.note_cite_source_insert(ctx.profile_id, note_id, &source)?;
        let verb = if created {
            "Связь с источником создана"
        } else {
            "Связь с источником уже существовала"
        };
        Ok(ToolOutcome::text(format!(
            "{verb}: заметка {note_id} → источник «{source}»."
        )))
    }
}
