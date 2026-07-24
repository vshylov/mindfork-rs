//! A knowledge-base document (RAG): a text fragment with an embedding. Isolated
//! per profile. See spec §5.1, §9.3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A knowledge fragment with an embedding, belonging to a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagDocument {
    pub id: Uuid,
    pub profile_id: Uuid,
    /// The source (a file name/URL/arbitrary label).
    pub source: String,
    pub chunk_text: String,
    pub embedding: Vec<f32>,
    pub created_at: DateTime<Utc>,
}

/// A RAG search result (no embedding; carries the distance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagHit {
    pub id: Uuid,
    pub source: String,
    pub chunk_text: String,
    pub distance: f32,
}

/// A per-source summary in the profile's knowledge base (for `/rag list`): how many
/// chunks are indexed and the date (of the source's earliest chunk).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagSourceInfo {
    pub source: String,
    pub chunks: usize,
    pub created_at: DateTime<Utc>,
}

/// The saved raw text of an indexed source (for `/rag rebuild`). Stored separately
/// from the chunks so reindexing (changing the chunk size/overlap or the embedding
/// model) doesn't require the source file on disk. See spec §9.3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagStoredSource {
    pub source: String,
    pub content: String,
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
