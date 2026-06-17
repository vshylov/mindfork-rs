//! Инструменты базы знаний (RAG): `rag_add`, `rag_search`. Чанкинг → эмбеддинг
//! (выделенный сервер, ADR 0002) → запись/kNN в sqlite-vec, **изоляция по
//! `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::entities::rag::RagDocument;

use super::{Tool, ToolContext, ToolOutcome};

/// Максимальный размер чанка в символах (грубая нарезка длинных абзацев).
const MAX_CHUNK_CHARS: usize = 800;
/// Топ-K по умолчанию для поиска.
const DEFAULT_TOP_K: usize = 5;

/// `rag_add` — добавляет текст в базу знаний (чанкинг + эмбеддинг). Возвращает
/// число записанных чанков.
pub struct RagAdd;

#[async_trait::async_trait]
impl Tool for RagAdd {
    fn id(&self) -> ToolId {
        "rag_add".into()
    }
    fn description(&self) -> String {
        "Добавить текст в базу знаний для последующего семантического поиска.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"},
                "source": {"type": "string", "description": "Источник (имя/URL)"}
            },
            "required": ["text"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("ожидается строковое поле text"))?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("(без источника)")
            .to_string();

        let chunks = chunk_text(text);
        if chunks.is_empty() {
            anyhow::bail!("text не содержит контента для индексации");
        }
        let embeddings = ctx.embedder.embed(chunks.clone()).await?;
        if embeddings.len() != chunks.len() {
            anyhow::bail!("эмбеддер вернул неверное число векторов");
        }
        for (chunk, embedding) in chunks.iter().zip(embeddings) {
            let doc = RagDocument::new(ctx.profile_id, &source, chunk, embedding);
            ctx.storage.db().rag_insert(&doc)?;
        }
        Ok(ToolOutcome::text(format!(
            "Добавлено чанков: {}.",
            chunks.len()
        )))
    }
}

/// `rag_search` — семантический поиск по базе знаний профиля.
pub struct RagSearch;

#[async_trait::async_trait]
impl Tool for RagSearch {
    fn id(&self) -> ToolId {
        "rag_search".into()
    }
    fn description(&self) -> String {
        "Найти релевантные фрагменты в базе знаний по смысловому запросу.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "top_k": {"type": "integer", "minimum": 1}
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле query"))?;
        let k = args
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_TOP_K);

        let mut embeddings = ctx.embedder.embed(vec![query.to_string()]).await?;
        let query_vec = embeddings
            .pop()
            .ok_or_else(|| anyhow::anyhow!("эмбеддер не вернул вектор запроса"))?;
        let hits = ctx.storage.db().rag_search(ctx.profile_id, &query_vec, k)?;
        if hits.is_empty() {
            return Ok(ToolOutcome::text("В базе знаний ничего не найдено."));
        }
        let mut out = format!("Найдено фрагментов: {}\n", hits.len());
        for h in &hits {
            out.push_str(&format!("- [{}] {}\n", h.source, h.chunk_text));
        }
        Ok(ToolOutcome::text(out.trim_end().to_string()))
    }
}

/// Нарезает текст на чанки: по абзацам (двойной перевод строки), длинные абзацы
/// дробятся окнами по `MAX_CHUNK_CHARS` символов. Пустые отбрасываются.
/// `pub(crate)` — переиспользуется фоновой индексацией файлов (`/rag add`).
pub(crate) fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    for paragraph in text.split("\n\n") {
        let trimmed = paragraph.trim();
        if trimmed.is_empty() {
            continue;
        }
        let chars: Vec<char> = trimmed.chars().collect();
        if chars.len() <= MAX_CHUNK_CHARS {
            chunks.push(trimmed.to_string());
        } else {
            for window in chars.chunks(MAX_CHUNK_CHARS) {
                chunks.push(window.iter().collect());
            }
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    #[test]
    fn chunking_splits_paragraphs_and_long_text() {
        let text = format!("первый абзац\n\nвторой абзац\n\n{}", "x".repeat(2000));
        let chunks = chunk_text(&text);
        // 2 коротких абзаца + 3 окна по 800 из 2000 длинного.
        assert_eq!(chunks.len(), 2 + 3);
        assert_eq!(chunks[0], "первый абзац");
    }

    #[tokio::test]
    async fn add_then_search_returns_relevant_chunk() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        RagAdd
            .invoke(
                &ctx,
                serde_json::json!({
                    "text": "кошки любят рыбу\n\nсобаки любят кости",
                    "source": "факты"
                }),
            )
            .await
            .unwrap();

        let out = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки рыба", "top_k": 1}))
            .await
            .unwrap();
        assert!(
            out.result.contains("кошки любят рыбу"),
            "got: {}",
            out.result
        );
        assert!(out.result.contains("факты"));
    }

    #[tokio::test]
    async fn search_isolated_by_profile() {
        // Документы профиля A не должны находиться при поиске профиля B
        // (разные профили в одном хранилище).
        let dir = tempfile::tempdir().unwrap();
        let storage = std::sync::Arc::new(
            crate::shared::storage::Storage::open(crate::shared::paths::Paths::with_root(
                dir.path(),
            ))
            .unwrap(),
        );
        let engine: std::sync::Arc<dyn crate::shared::api::EngineBackend> =
            std::sync::Arc::new(crate::shared::api::mock::MockBackend::scripted(vec![]));
        let embedder: std::sync::Arc<dyn crate::shared::api::Embedder> =
            std::sync::Arc::new(crate::shared::api::mock::MockEmbedder::new(16));

        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mk = |pid| super::ToolContext {
            profile_id: pid,
            chat_id: Uuid::new_v4(),
            system_message: String::new(),
            effective_sampling: Default::default(),
            last_user_message_at: None,
            storage: storage.clone(),
            engine: engine.clone(),
            embedder: embedder.clone(),
        };

        RagAdd
            .invoke(&mk(a), serde_json::json!({"text": "секрет профиля A"}))
            .await
            .unwrap();
        let out = RagSearch
            .invoke(&mk(b), serde_json::json!({"query": "секрет профиля A"}))
            .await
            .unwrap();
        assert!(out.result.contains("ничего не найдено"));
    }
}
