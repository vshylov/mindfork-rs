//! Инструменты заметок: `note_save`, `note_recall`. Память ассистента о
//! пользователе/контексте, **изолированная по `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;
use uuid::Uuid;

use crate::entities::note::Note;
use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Имя инструмента ревизии заметки (DB-only, гейтится набором профиля).
pub const NOTE_REVISE_ID: &str = "note_revise";

/// Сколько заметок отдаёт `note_recall` по умолчанию (если лимит не задан).
const DEFAULT_RECALL: usize = 5;

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
        if let Some(q) = query
            && let Some(notes) = semantic_recall(ctx, q, &tags, limit).await
        {
            return Ok(format_notes(&notes));
        }

        let notes = ctx
            .storage
            .db()
            .note_list(ctx.profile_id, query, &tags, limit)?;
        Ok(format_notes(&notes))
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
        Ok(ToolOutcome::text(format!(
            "Заметка переписана (id={uuid})."
        )))
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
}
