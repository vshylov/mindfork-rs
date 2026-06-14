//! Документ базы знаний (RAG): фрагмент текста с эмбеддингом. Изолируется по
//! профилю. См. spec §5.1, §9.3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Фрагмент знаний с эмбеддингом, принадлежащий профилю.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagDocument {
    pub id: Uuid,
    pub profile_id: Uuid,
    /// Источник (имя файла/URL/произвольный).
    pub source: String,
    pub chunk_text: String,
    pub embedding: Vec<f32>,
    pub created_at: DateTime<Utc>,
}

/// Результат RAG-поиска (без эмбеддинга; с расстоянием).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagHit {
    pub id: Uuid,
    pub source: String,
    pub chunk_text: String,
    pub distance: f32,
}

impl RagDocument {
    pub fn new(
        profile_id: Uuid,
        source: impl Into<String>,
        chunk_text: impl Into<String>,
        embedding: Vec<f32>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            profile_id,
            source: source.into(),
            chunk_text: chunk_text.into(),
            embedding,
            created_at: Utc::now(),
        }
    }
}
