//! Storage layer (`shared/storage`): JSON (config/chats/profiles) + SQLite
//! (notes/RAG). The [`Storage`] facade combines both and coordinates cascade
//! soft delete. See spec §5.2, §4.4.2.

pub mod cache;
mod chat_steps;
pub mod db;
pub mod json;
pub mod schema;

use anyhow::Result;
use uuid::Uuid;

use crate::shared::paths::Paths;

pub use cache::CacheDb;
pub use db::Db;
pub use json::JsonStore;

/// The single storage facade. The sole writer is the orchestrator (spec §4.4).
pub struct Storage {
    json: JsonStore,
    db: Db,
    cache: CacheDb,
}

impl Storage {
    /// Opens storage at the app's paths.
    pub fn open(paths: Paths) -> Result<Self> {
        let db = Db::open(&paths.data_db())?;
        // The search index is opened alongside the real data, but it cannot
        // block startup over its *contents*: `CacheDb::open` wipes an unusable
        // file and starts empty (research §2). Only an I/O failure that also
        // prevents creating a fresh one reaches here.
        let cache = CacheDb::open(&paths.cache_db())?;
        let json = JsonStore::new(paths);
        Ok(Self { json, db, cache })
    }

    /// Like [`Storage::open`], but with both SQLite halves in memory — for
    /// tests that need a working store rather than a durable one.
    ///
    /// The JSON repository still uses `paths`, so anything that reads or writes
    /// config/profiles/chats behaves exactly as before; only `data.db` and
    /// `cache.db` stop being files. That is deliberate: the two SQLite halves
    /// are what a tool test actually exercises, and they are also what costs —
    /// every write is an fsync, which is why the tests that go through this are
    /// the ones a slow disk punishes hardest (measured on the Windows CI
    /// runner: notes/RAG tests run 8–19x slower than locally, against a 3.9x
    /// median for the suite).
    ///
    /// Not suitable for anything that reopens storage or asserts on the files
    /// themselves — an in-memory database dies with its connection. Backup,
    /// migration and compaction tests therefore keep using [`Storage::open`].
    #[cfg(test)]
    pub fn open_in_memory(paths: Paths) -> Result<Self> {
        Ok(Self {
            json: JsonStore::new(paths),
            db: Db::open_in_memory()?,
            cache: CacheDb::open_in_memory()?,
        })
    }

    /// JSON repository (config/profiles/chats).
    pub fn json(&self) -> &JsonStore {
        &self.json
    }

    /// SQLite repository (notes/RAG).
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The disposable search cache (`cache.db`): a full-text index over chat
    /// content, kept in step with the chat files by
    /// [`app::orchestrator::search`](crate::app::orchestrator). Derived data —
    /// see the [`cache`] module doc.
    pub fn cache(&self) -> &CacheDb {
        &self.cache
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

    /// The in-memory facade must behave like the real one *and* touch no disk.
    /// The second half is the whole point and is invisible from a passing test,
    /// so it is pinned here. (That the tool testkit actually *uses* this is a
    /// separate claim, pinned by `tool_context_storage_touches_no_disk` in
    /// `features::tools` — a test cannot see which constructor its caller
    /// picked.)
    #[test]
    fn in_memory_storage_works_but_writes_no_database_files() {
        use crate::entities::note::Note;
        use crate::entities::rag::RagDocument;

        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let storage = Storage::open_in_memory(paths.clone()).unwrap();
        let profile = Uuid::new_v4();

        // Both SQLite halves are real: schema applied, sqlite-vec registered.
        storage
            .db()
            .note_insert(&Note::new(profile, "заметка", vec![]))
            .unwrap();
        storage
            .db()
            .rag_insert(&RagDocument::new(profile, "s", "документ", vec![1.0, 0.0]))
            .unwrap();
        assert_eq!(
            storage
                .db()
                .note_list(profile, None, &[], None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            storage
                .db()
                .rag_search(profile, &[1.0, 0.0], 5)
                .unwrap()
                .len(),
            1
        );

        assert!(!paths.data_db().exists(), "data.db must not be created");
        assert!(!paths.cache_db().exists(), "cache.db must not be created");
    }

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
