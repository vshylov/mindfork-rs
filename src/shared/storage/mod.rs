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

/// Whether this launch is about to create `data.db` from scratch **next to chat
/// files that already exist** — the fingerprint of a data directory carried to
/// another machine (or restored by hand) without the database.
///
/// Must be called **before** [`Storage::open`], which creates the file: once it
/// exists, a database that was never there is indistinguishable from a first
/// launch, and the loss can no longer be named. Everything `data.db` holds —
/// notes, the self-model, the knowledge base and the chat-attachment index
/// (spec §5.2) — silently starts empty otherwise; the chats themselves are
/// whole, because they *are* the files that were copied.
///
/// A fresh install answers `false` (no chats either), so the notice this drives
/// only fires where something is actually missing.
pub fn db_missing_beside_chats(paths: &Paths) -> bool {
    if paths.data_db().exists() {
        return false;
    }
    // `chat_files` is the same stat-only walk the search index reconciles
    // against: `*.json` whose stem is a UUID, i.e. what this application counts
    // as a chat. An unreadable directory reads as "no chats" — this decides
    // whether to *say* something, and guessing on an I/O error would be worse
    // than staying quiet.
    JsonStore::new(paths.clone())
        .chat_files()
        .is_ok_and(|files| !files.is_empty())
}

/// The single storage facade. The sole writer is the orchestrator (spec §4.4).
pub struct Storage {
    json: JsonStore,
    db: Db,
    cache: CacheDb,
    /// What [`db_missing_beside_chats`] found at open time — see
    /// [`Storage::chats_without_db`].
    chats_without_db: bool,
}

impl Storage {
    /// Opens storage at the app's paths.
    pub fn open(paths: Paths) -> Result<Self> {
        // Before `Db::open`, which creates the file: this is the last moment a
        // missing database is still distinguishable from a first launch. See
        // [`db_missing_beside_chats`] and [`Storage::chats_without_db`].
        let chats_without_db = db_missing_beside_chats(&paths);
        if chats_without_db {
            tracing::warn!(
                root = %paths.root().display(),
                "data.db is missing next to existing chats — starting an empty database: \
                 notes, the self-model, the knowledge base and the attachment index begin empty"
            );
        }
        let db = Db::open(&paths.data_db())?;
        // The search index is opened alongside the real data, but it cannot
        // block startup over its *contents*: `CacheDb::open` wipes an unusable
        // file and starts empty (research §2). Only an I/O failure that also
        // prevents creating a fresh one reaches here.
        let cache = CacheDb::open(&paths.cache_db())?;
        let json = JsonStore::new(paths);
        Ok(Self {
            json,
            db,
            cache,
            chats_without_db,
        })
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
            // Nothing was opened from disk, so there is no such history to report.
            chats_without_db: false,
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

    /// Whether this process **created** `data.db` next to chat files that were
    /// already there (see [`db_missing_beside_chats`]). Read once at startup by
    /// the orchestrator, which turns it into the one note that tells the user
    /// why the assistant remembers nothing about conversations it can see.
    ///
    /// Carried on the facade rather than recomputed: after `open` the file
    /// exists, so the answer would be `false` from then on.
    pub fn chats_without_db(&self) -> bool {
        self.chats_without_db
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

    /// Chat files carried to another machine without `data.db`: the one moment
    /// the situation is still visible. Pinned in every combination, because what
    /// makes the predicate useful is as much what it stays quiet about (a fresh
    /// install) as what it reports.
    #[test]
    fn db_missing_beside_chats_only_fires_when_chats_outlived_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let json = JsonStore::new(paths.clone());

        // A fresh install: no database and no chats — nothing to report.
        assert!(!db_missing_beside_chats(&paths));

        // A file that is not a chat does not make one (the stem is not a UUID),
        // otherwise a stray file in `chats/` would raise the alarm on its own.
        let stray = paths.chats_dir().join("readme.json");
        std::fs::create_dir_all(paths.chats_dir()).unwrap();
        std::fs::write(&stray, b"{}").unwrap();
        assert!(!db_missing_beside_chats(&paths));
        std::fs::remove_file(&stray).unwrap();

        // The copied directory: chats on disk, no database.
        let profile = Profile::new("A", "s");
        json.save_chat(&Chat::from_profile(&profile, "c1")).unwrap();
        assert!(db_missing_beside_chats(&paths));

        // Opening storage creates the database — which is exactly why the check
        // has to run before it, and why it must go quiet afterwards.
        let _storage = Storage::open(paths.clone()).unwrap();
        assert!(!db_missing_beside_chats(&paths));
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
