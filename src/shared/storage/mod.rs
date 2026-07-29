//! Storage layer (`shared/storage`): JSON (config/chats/profiles) + SQLite
//! (notes/RAG). The [`Storage`] facade combines both and coordinates cascade
//! soft delete. See spec §5.2, §4.4.2.

pub mod cache;
pub mod db;
pub mod json;
pub mod schema;

use anyhow::Result;
use uuid::Uuid;

use crate::shared::paths::Paths;

// Ahead of its consumer, like the module itself (see `cache/mod.rs`): this
// re-export becomes the path `Storage` uses for its third member. Drop the
// `allow` when it does.
#[allow(unused_imports)]
pub use cache::CacheDb;
pub use db::Db;
pub use json::JsonStore;

/// The single storage facade. The sole writer is the orchestrator (spec §4.4).
pub struct Storage {
    json: JsonStore,
    db: Db,
}

impl Storage {
    /// Opens storage at the app's paths.
    pub fn open(paths: Paths) -> Result<Self> {
        let db = Db::open(&paths.data_db())?;
        let json = JsonStore::new(paths);
        Ok(Self { json, db })
    }

    /// JSON repository (config/profiles/chats).
    pub fn json(&self) -> &JsonStore {
        &self.json
    }

    /// SQLite repository (notes/RAG).
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Soft-deletes a profile with a cascade to its chats. Notes/RAG are not
    /// physically deleted — they're excluded from results because the profile
    /// is hidden (filtering by visible profiles happens at the query side).
    /// See spec §12.3.
    pub fn hide_profile_cascade(&self, profile_id: Uuid) -> Result<bool> {
        let found = self.json.hide_profile(profile_id)?;
        if found {
            self.json.hide_chats_of_profile(profile_id)?;
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::chat::Chat;
    use crate::entities::profile::Profile;

    #[test]
    fn notes_and_rag_isolated_by_profile_via_facade() {
        use crate::entities::note::Note;
        use crate::entities::rag::RagDocument;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        storage
            .db()
            .note_insert(&Note::new(a, "секрет A", vec![]))
            .unwrap();
        storage
            .db()
            .note_insert(&Note::new(b, "секрет B", vec![]))
            .unwrap();
        storage
            .db()
            .rag_insert(&RagDocument::new(b, "b", "док B", vec![1.0, 0.0]))
            .unwrap();
        storage
            .db()
            .rag_insert(&RagDocument::new(a, "a", "док A", vec![1.0, 0.0]))
            .unwrap();

        // A chat of profile A sees only A's data — nothing from B "leaks in".
        let a_notes = storage.db().note_list(a, None, &[], None).unwrap();
        assert_eq!(a_notes.len(), 1);
        assert!(a_notes.iter().all(|n| n.profile_id == a));
        let a_hits = storage.db().rag_search(a, &[1.0, 0.0], 5).unwrap();
        assert_eq!(a_hits.len(), 1);
        assert_eq!(a_hits[0].chunk_text, "док A");
    }

    #[test]
    fn hide_profile_cascades_to_chats() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Paths::with_root(dir.path())).unwrap();

        let p = Profile::new("A", "s");
        storage.json().upsert_profile(&p).unwrap();
        storage
            .json()
            .save_chat(&Chat::from_profile(&p, "c1"))
            .unwrap();
        storage
            .json()
            .save_chat(&Chat::from_profile(&p, "c2"))
            .unwrap();

        assert!(storage.hide_profile_cascade(p.id).unwrap());
        assert!(storage.json().load_profiles().unwrap()[0].is_hidden);
        assert!(
            storage
                .json()
                .load_chats()
                .unwrap()
                .iter()
                .all(|c| c.is_hidden)
        );
    }
}
