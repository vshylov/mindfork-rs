//! Заметка ассистента (память о пользователе/контексте). Изолируется по
//! профилю. См. spec §5.1, §9.3, §10.3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Заметка, принадлежащая профилю.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Note {
    pub fn new(profile_id: Uuid, content: impl Into<String>, tags: Vec<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            profile_id,
            content: content.into(),
            tags,
            created_at: now,
            updated_at: now,
        }
    }
}
