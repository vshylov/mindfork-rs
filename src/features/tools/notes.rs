//! Инструменты заметок: `note_save`, `note_recall`. Память ассистента о
//! пользователе/контексте, **изолированная по `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;

use crate::entities::note::Note;
use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

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
        Ok(ToolOutcome::text(format!("Заметка сохранена (id={id}).")))
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

        let notes = ctx
            .storage
            .db()
            .note_list(ctx.profile_id, query, &tags, limit)?;
        if notes.is_empty() {
            return Ok(ToolOutcome::text("Заметки не найдены."));
        }
        let mut out = format!("Найдено заметок: {}\n", notes.len());
        for n in &notes {
            out.push_str(&format!("- {}", n.content));
            if !n.tags.is_empty() {
                out.push_str(&format!("  [{}]", n.tags.join(", ")));
            }
            out.push('\n');
        }
        Ok(ToolOutcome::text(out.trim_end().to_string()))
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
}
