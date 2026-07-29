//! The disposable search cache (`cache.db`) — a full-text index over chat
//! message text. See [docs/research/chat-content-search.md](../../../../docs/research/chat-content-search.md),
//! §2 (why a second database) and §7a (this schema).
//!
//! **This is derived data, and that changes the rules.** `data.db` holds notes,
//! the self-model and RAG — irreplaceable user content, hence the
//! schema-versioning machinery of ADR 0006 (steps in transactions, downgrade
//! guards, pre-migration backups). A search index needs **none of it**: a
//! version mismatch, an unreadable file or a corrupt schema is answered by
//! *deleting the file and starting empty* (~350 ms to rebuild on the real
//! corpus), never by a migration step and never by failing. So [`CacheDb::open`]
//! self-heals instead of bailing — a disposable index must not be able to block
//! startup. `features/backup.rs` uses an allowlist, so this file is excluded
//! from archives with no code change, and a restore correctly lands without an
//! index and rebuilds it.
//!
//! **Two shape decisions from the probe (§7a):**
//!
//! - *External-content FTS5*, not a standalone FTS table. A standalone table can
//!   only carry `chat_id` as `UNINDEXED`, so "re-index this one chat" means a
//!   full scan to find its old rows; an external-content table keeps the metadata
//!   in a real table with a real index. The price is triggers on write, which is
//!   the right trade — deletes happen on every incremental re-index, while a full
//!   rebuild is rare and runs in the background.
//! - *Diff at message level*, not chat level. A chat is saved every ~800 ms while
//!   a reply streams; re-indexing a large chat wholesale would cost ~385 ms per
//!   save. Since history is append-only apart from truncation, a diff over
//!   `(message_id, text_hash)` reduces a streaming save to **one** row deleted and
//!   re-inserted. See [`CacheDb::index_chat`].
//!
//! **FSD.** Query escaping (research §4) is pure logic and lives in `features`,
//! which `shared` may not depend on. So [`CacheDb::search_chats`] takes an
//! **already-escaped** FTS5 query, and `app` — which may use both — calls
//! `features::chat_search::to_fts_query` and passes the result down.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

/// Schema version of the disposable cache database (`PRAGMA user_version`). A
/// mismatch in either direction is answered by wiping the file — unlike
/// `data.db` there is nothing here worth migrating, so this constant is bumped
/// freely whenever the schema changes.
pub const CACHE_SCHEMA: u32 = 1;

/// One message as it goes into the index. `role`/`ts` are unused by stage 1's
/// chat-list filter; they are stored because stage 2's message-level screen
/// shows them, and adding them later would mean a full rebuild.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedMessage {
    /// The message's own id (stable across saves — the diff key).
    pub id: Uuid,
    /// `user` / `assistant` / ….
    pub role: String,
    /// Timestamp, RFC 3339 (stored as text — the index never sorts by it).
    pub ts: String,
    /// The indexed text. Stage 1 indexes `message.text` only (fork F3).
    pub text: String,
}

/// One matching message, as [`CacheDb::search_messages`] returns it: everything
/// the message-level search screen shows, straight out of the index (the chats
/// themselves are not read — that is the point of the cache).
#[derive(Debug, Clone, PartialEq)]
pub struct MessageHit {
    pub chat_id: Uuid,
    pub message_id: Uuid,
    pub role: String,
    /// Timestamp, RFC 3339 — as stored (see [`IndexedMessage::ts`]).
    pub ts: String,
    /// The message's full text; the snippet is built from it in `features`.
    pub text: String,
}

/// SQLite full-text index over chat content. Disposable — see the module doc.
pub struct CacheDb {
    conn: Mutex<Connection>,
}

impl CacheDb {
    /// Opens the cache at `path`, creating it if absent.
    ///
    /// Never fails because of the cache's *contents*: an unreadable file, a
    /// corrupt schema or a `user_version` that is not [`CACHE_SCHEMA`] (in
    /// either direction) is answered by deleting the file and starting empty,
    /// logged at `info`. Only an I/O failure that also prevents creating a fresh
    /// database is reported as an error.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match Self::try_open(path) {
            Ok(db) => Ok(db),
            Err(err) => {
                tracing::info!(
                    path = %path.display(),
                    error = %format!("{err:#}"),
                    "cache.db is unusable — deleting it and starting empty (derived data, rebuilt in the background)"
                );
                remove_db_files(path);
                Self::try_open(path).with_context(|| format!("recreating {}", path.display()))
            }
        }
    }

    /// Opens an in-memory cache (for tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn try_open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Bookkeeping for the startup reconciliation (research §3): `chat_id →
    /// (mtime_ms, size)` of the chat file as it was when indexed. A chat whose
    /// file differs — or is absent from this map — needs re-indexing.
    pub fn indexed_state(&self) -> Result<HashMap<Uuid, (i64, u64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT chat_id, mtime_ms, size FROM indexed_chats")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    parse_uuid(r.get::<_, String>(0)?),
                    (r.get::<_, i64>(1)?, r.get::<_, i64>(2)? as u64),
                ))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        Ok(rows)
    }

    /// Brings one chat's index in line with `messages`, and records the file
    /// state it was built from.
    ///
    /// Diffs at **message level** on `(message_id, text_hash)`: a row whose text
    /// is unchanged is left untouched, so a streaming save that appends to the
    /// last message rewrites exactly one row instead of the whole chat. A
    /// changed message is deleted and re-inserted rather than updated — that
    /// keeps the FTS index in step through the `AFTER DELETE`/`AFTER INSERT`
    /// triggers alone (see [`baseline_ddl`]).
    ///
    /// `role`/`ts` are not part of the diff key: they are fixed when a message
    /// is created, so text is the only field that can change under a stable id.
    ///
    /// One transaction, so the index is never observed half-updated — and the
    /// reconciliation can write chat by chat while search stays usable.
    pub fn index_chat(
        &self,
        chat_id: Uuid,
        mtime_ms: i64,
        size: u64,
        messages: &[IndexedMessage],
    ) -> Result<()> {
        self.index_chat_guarded(chat_id, None, mtime_ms, size, messages)
            .map(|_| ())
    }

    /// [`index_chat`](Self::index_chat), but only if the chat's recorded file
    /// state is still `expected` — i.e. **nobody has indexed it since the caller
    /// looked**. Returns whether the write happened.
    ///
    /// This exists because the index has two writers with very different
    /// freshness: the post-save hook, which always holds the current chat, and
    /// the startup reconciliation, which reads a chat and may only get round to
    /// writing it hundreds of milliseconds later. Without the guard the
    /// reconciliation's older snapshot can land *after* a live save and wipe it
    /// — a chat silently missing from search until the next launch, and exactly
    /// the common case of "launch the app and immediately keep typing".
    ///
    /// The comparison is against the bookkeeping row, which is the index's own
    /// record of what it holds — an exact token, unlike mtime, which two writes
    /// in the same millisecond would tie on.
    pub fn index_chat_if_unchanged(
        &self,
        chat_id: Uuid,
        expected: Option<(i64, u64)>,
        mtime_ms: i64,
        size: u64,
        messages: &[IndexedMessage],
    ) -> Result<bool> {
        self.index_chat_guarded(chat_id, Some(expected), mtime_ms, size, messages)
    }

    /// The shared body. `guard: None` writes unconditionally; `Some(expected)`
    /// writes only when the recorded state still matches — checked **inside the
    /// transaction**, so the check and the write cannot be separated.
    fn index_chat_guarded(
        &self,
        chat_id: Uuid,
        guard: Option<Option<(i64, u64)>>,
        mtime_ms: i64,
        size: u64,
        messages: &[IndexedMessage],
    ) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let chat = chat_id.to_string();

        if let Some(expected) = guard {
            let current: Option<(i64, u64)> = tx
                .query_row(
                    "SELECT mtime_ms, size FROM indexed_chats WHERE chat_id = ?1",
                    params![chat],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64)),
                )
                .optional()?;
            if current != expected {
                return Ok(false);
            }
        }

        // What is indexed for this chat right now: message_id → (rowid, hash).
        let existing: HashMap<String, (i64, i64)> = {
            let mut stmt =
                tx.prepare("SELECT message_id, id, text_hash FROM messages WHERE chat_id = ?1")?;
            stmt.query_map(params![chat], |r| {
                Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get(2)?)))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?
        };

        let mut seen: HashSet<String> = HashSet::with_capacity(messages.len());
        for msg in messages {
            let message_id = msg.id.to_string();
            // A malformed chat with a repeated message id would otherwise violate
            // UNIQUE(chat_id, message_id) and leave it unindexed entirely. This is
            // derived data — tolerate the input and keep the first occurrence.
            if !seen.insert(message_id.clone()) {
                continue;
            }
            let hash = text_hash(&msg.text);
            match existing.get(&message_id) {
                Some((_, old)) if *old == hash => continue, // unchanged — leave the row alone
                Some((rowid, _)) => {
                    tx.execute("DELETE FROM messages WHERE id = ?1", params![rowid])?;
                }
                None => {}
            }
            tx.execute(
                "INSERT INTO messages(chat_id, message_id, text_hash, role, ts, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![chat, message_id, hash, msg.role, msg.ts, msg.text],
            )?;
        }

        // Messages that disappeared (history truncation: Ctrl+E / regenerate).
        for (message_id, (rowid, _)) in &existing {
            if !seen.contains(message_id) {
                tx.execute("DELETE FROM messages WHERE id = ?1", params![rowid])?;
            }
        }

        tx.execute(
            "INSERT INTO indexed_chats(chat_id, mtime_ms, size) VALUES (?1, ?2, ?3)
             ON CONFLICT(chat_id) DO UPDATE SET mtime_ms = excluded.mtime_ms, size = excluded.size",
            params![chat, mtime_ms, size as i64],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Drops a chat from the index entirely — its file is gone, or the chat was
    /// hidden. Also forgets the reconciliation bookkeeping, so it is re-indexed
    /// from scratch should the file come back.
    pub fn forget_chat(&self, chat_id: Uuid) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let chat = chat_id.to_string();
        tx.execute("DELETE FROM messages WHERE chat_id = ?1", params![chat])?;
        tx.execute(
            "DELETE FROM indexed_chats WHERE chat_id = ?1",
            params![chat],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Chats having at least one message matching an **already-escaped** FTS5
    /// query (built by `features::chat_search::to_fts_query` — see the module
    /// doc on FSD). Order is unspecified: stage 1 *filters* the chat list and
    /// the user's existing sort orders it, because trigram's `bm25` is weak
    /// (research §5).
    ///
    /// A malformed query surfaces as an `Err` (FTS5 reports a syntax error)
    /// rather than a panic; the caller is expected to have escaped it, and to
    /// have dropped tokens shorter than trigram's 3-character floor.
    pub fn search_chats(&self, fts_query: &str) -> Result<Vec<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT m.chat_id
             FROM messages_fts f
             JOIN messages m ON m.id = f.rowid
             WHERE messages_fts MATCH ?1",
        )?;
        // FTS5 reports a syntax error when the cursor is filtered, which is
        // either of these two calls depending on the fault — so the context goes
        // around both.
        let ids = (|| -> rusqlite::Result<Vec<Uuid>> {
            stmt.query_map(params![fts_query], |r| {
                Ok(parse_uuid(r.get::<_, String>(0)?))
            })?
            .collect()
        })()
        .with_context(|| format!("full-text query {fts_query:?}"))?;
        Ok(ids)
    }

    /// Individual messages matching an **already-escaped** FTS5 query, at most
    /// `limit` of them (see the module doc on FSD, and [`Self::search_chats`]).
    ///
    /// Ordered by chat, then by insertion, so the caller can group in a single
    /// pass. Within a chat that order is only *approximately* chat order — a
    /// message whose text changed is deleted and re-inserted, taking a fresh
    /// rowid — so the orchestrator, which owns the chats, re-orders the hits
    /// against the real message list.
    ///
    /// When the query matches more than `limit` messages the cut falls by chat
    /// id, which is arbitrary with respect to the order the screen shows. That
    /// is why [`Self::count_matching_messages`] exists: the screen says
    /// "showing N of M" rather than silently truncating. On the measured corpus
    /// the worst case is 163 hits against a cap of 200, so this is a safety
    /// valve rather than an everyday path (docs/history/chat-search-stage2.md §2).
    pub fn search_messages(&self, fts_query: &str, limit: usize) -> Result<Vec<MessageHit>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.chat_id, m.message_id, m.role, m.ts, m.text
             FROM messages_fts f
             JOIN messages m ON m.id = f.rowid
             WHERE messages_fts MATCH ?1
             ORDER BY m.chat_id, m.id
             LIMIT ?2",
        )?;
        let hits = (|| -> rusqlite::Result<Vec<MessageHit>> {
            stmt.query_map(params![fts_query, limit as i64], |r| {
                Ok(MessageHit {
                    chat_id: parse_uuid(r.get::<_, String>(0)?),
                    message_id: parse_uuid(r.get::<_, String>(1)?),
                    role: r.get(2)?,
                    ts: r.get(3)?,
                    text: r.get(4)?,
                })
            })?
            .collect()
        })()
        .with_context(|| format!("full-text message query {fts_query:?}"))?;
        Ok(hits)
    }

    /// How many messages the query matches in total — the honest denominator of
    /// "showing N of M" when [`Self::search_messages`] hit its cap. Counts in
    /// the index alone (no join, no text read).
    pub fn count_matching_messages(&self, fts_query: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH ?1",
                params![fts_query],
                |r| r.get(0),
            )
            .with_context(|| format!("full-text message count {fts_query:?}"))?;
        Ok(n as usize)
    }

    /// The ids of one chat's matching messages — unlimited, because a single
    /// chat is bounded. Used by "open this chat at its first match" (`Enter` in
    /// the chat list's content mode): the caller picks the earliest by real
    /// chat order, which only it can know.
    pub fn matching_messages_in_chat(&self, fts_query: &str, chat_id: Uuid) -> Result<Vec<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.message_id
             FROM messages_fts f
             JOIN messages m ON m.id = f.rowid
             WHERE messages_fts MATCH ?1 AND m.chat_id = ?2",
        )?;
        let ids = (|| -> rusqlite::Result<Vec<Uuid>> {
            stmt.query_map(params![fts_query, chat_id.to_string()], |r| {
                Ok(parse_uuid(r.get::<_, String>(0)?))
            })?
            .collect()
        })()
        .with_context(|| format!("full-text query {fts_query:?} in one chat"))?;
        Ok(ids)
    }

    /// Number of indexed messages. Test-only for now — nothing in the app reads
    /// it, and gating it (rather than allowing dead code) keeps the module
    /// honest about what is actually wired. Lift the gate if a diagnostic ever
    /// wants it; the precedent is `SaveQueue::is_dirty`.
    #[cfg(test)]
    pub fn message_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

/// Brings the cache to [`CACHE_SCHEMA`]. Deliberately *not* the ADR 0006
/// machinery: there are no steps and no downgrade guard, because both answers
/// are the same — a version that is not ours means the caller should wipe the
/// file, which [`CacheDb::open`] does on any error from here.
fn migrate(conn: &Connection) -> Result<()> {
    let version = read_user_version(conn)?;
    if version != 0 && version != CACHE_SCHEMA {
        bail!("cache.db schema is {version}, this build indexes {CACHE_SCHEMA}");
    }
    // Idempotent, and also repairs a database whose creation was interrupted.
    baseline_ddl(conn)?;
    if version == 0 {
        set_user_version(conn, CACHE_SCHEMA)?;
    }
    Ok(())
}

/// The cache schema (research §7a). Idempotent — it runs on every open.
///
/// `messages_fts` is an **external-content** FTS5 table: it stores only the
/// index and reads column values back through `messages`, which is what lets a
/// re-index find one chat's rows by a real index instead of a full scan. Its two
/// triggers are the standard external-content pair; there is no `AFTER UPDATE`
/// trigger because [`CacheDb::index_chat`] never updates `text` — it deletes and
/// re-inserts.
fn baseline_ddl(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS indexed_chats (
             chat_id  TEXT PRIMARY KEY,
             mtime_ms INTEGER NOT NULL,
             size     INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS messages (
             id         INTEGER PRIMARY KEY,
             chat_id    TEXT NOT NULL,
             message_id TEXT NOT NULL,
             text_hash  INTEGER NOT NULL,
             role       TEXT NOT NULL,
             ts         TEXT NOT NULL,
             text       TEXT NOT NULL,
             UNIQUE(chat_id, message_id)
         );

         CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
             text,
             content='messages',
             content_rowid='id',
             tokenize='trigram'
         );

         CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
             INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
         END;

         CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
             INSERT INTO messages_fts(messages_fts, rowid, text)
             VALUES ('delete', old.id, old.text);
         END;",
    )?;
    Ok(())
}

fn read_user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as u32)
}

fn set_user_version(conn: &Connection, v: u32) -> Result<()> {
    // `PRAGMA user_version = N` doesn't accept a bound parameter — we format it
    // in (v: u32, injection is impossible).
    conn.execute_batch(&format!("PRAGMA user_version = {v};"))?;
    Ok(())
}

/// Deletes the database and its journal siblings. Best effort: a file that is
/// already gone (or cannot be removed) leaves [`CacheDb::open`] to report the
/// failure of the *recreate*, which is the error worth showing.
fn remove_db_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    // SQLite appends the suffix to the full file name (`cache.db-wal`), so this
    // is not `with_extension`.
    for suffix in ["-wal", "-shm"] {
        let mut name = OsString::from(path.as_os_str());
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}

/// FNV-1a, 64-bit — the message-level diff key (research §7a).
///
/// Hand-rolled rather than `DefaultHasher`, whose output is explicitly not
/// stable across Rust versions: a changed hash function would silently re-index
/// every message after a toolchain upgrade. Stored as `i64` because SQLite (and
/// `rusqlite`) has no unsigned integer; the wrap is bit-preserving, and the
/// value is only ever compared for equality.
fn text_hash(text: &str) -> i64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash as i64
}

fn parse_uuid(s: String) -> Uuid {
    Uuid::parse_str(&s).unwrap_or(Uuid::nil())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> CacheDb {
        CacheDb::open_in_memory().unwrap()
    }

    fn msg(text: &str) -> IndexedMessage {
        IndexedMessage {
            id: Uuid::new_v4(),
            role: "user".into(),
            ts: "2026-07-29T10:00:00Z".into(),
            text: text.into(),
        }
    }

    /// The rowids currently backing a chat's messages, keyed by message id. The
    /// rowid **is** the FTS docid, so preserving it is the property the
    /// message-level diff rests on.
    fn rowids(db: &CacheDb, chat: Uuid) -> HashMap<Uuid, i64> {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT message_id, id FROM messages WHERE chat_id = ?1")
            .unwrap();
        stmt.query_map(params![chat.to_string()], |r| {
            Ok((parse_uuid(r.get::<_, String>(0)?), r.get::<_, i64>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<HashMap<_, _>>>()
        .unwrap()
    }

    /// Rows in the FTS index with no message behind them — i.e. whether the
    /// delete trigger is doing its job. Returns the count and additionally runs
    /// SQLite's own check; both are pinned as *effective* by
    /// `orphan_check_catches_a_missing_delete_trigger`.
    ///
    /// Two corrections that a probe forced, because the obvious spellings of
    /// both halves are silently vacuous on an external-content table:
    ///
    /// - The join is against the `%_docsize` **shadow** table, not against
    ///   `messages_fts` itself. A plain scan of the FTS table reads its column
    ///   values back through the content table, so it yields exactly the rows of
    ///   `messages` and can never show an orphan — measured: with one stale
    ///   entry in the index, `SELECT count(*) FROM messages_fts` is 0 while
    ///   `MATCH` still returns the deleted row.
    /// - `integrity-check` is passed **1**. The bare
    ///   `VALUES('integrity-check')` — and the explicit `0` — only verify the
    ///   index's internal consistency; only the `1` form compares it against the
    ///   content table, which is the failure mode here.
    fn orphan_fts_rows(db: &CacheDb) -> i64 {
        let conn = db.conn.lock().unwrap();
        let orphans: i64 = conn
            .query_row(
                "SELECT count(*) FROM messages_fts_docsize d
                 LEFT JOIN messages m ON m.id = d.id
                 WHERE m.id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch(
            "INSERT INTO messages_fts(messages_fts, rank) VALUES('integrity-check', 1);",
        )
        .expect("the FTS index must match the content table");
        orphans
    }

    #[test]
    fn round_trip_index_and_search() {
        let db = cache();
        let chat = Uuid::new_v4();
        db.index_chat(chat, 42, 7, &[msg("hello world"), msg("second message")])
            .unwrap();

        assert_eq!(db.message_count().unwrap(), 2);
        assert_eq!(db.search_chats("\"hello\"").unwrap(), vec![chat]);
        assert!(db.search_chats("\"nothing here\"").unwrap().is_empty());
    }

    #[test]
    fn trigram_matches_cyrillic_infix_and_folds_case() {
        // The reason trigram was chosen (fork F1): today's chat filter is a
        // substring match, and FTS5 has no Russian stemmer. Both claims of
        // research §1.2 are pinned here.
        let db = cache();
        let chat = Uuid::new_v4();
        db.index_chat(chat, 1, 1, &[msg("это тестовое сообщение")])
            .unwrap();

        assert_eq!(
            db.search_chats("\"естов\"").unwrap(),
            vec![chat],
            "infix match, which a word-based tokenizer cannot do"
        );
        assert_eq!(
            db.search_chats("\"ЕСТОВ\"").unwrap(),
            vec![chat],
            "trigram folds case for Cyrillic on this SQLite build"
        );
    }

    #[test]
    fn unchanged_messages_keep_their_rowids_on_reindex() {
        // The property the whole design rests on: a chat is saved every ~800 ms
        // while a reply streams, so a save that only changes the last message
        // must rewrite exactly one row.
        let db = cache();
        let chat = Uuid::new_v4();
        let (a, b, c) = (msg("first"), msg("second"), msg("streaming original"));
        db.index_chat(chat, 1, 1, &[a.clone(), b.clone(), c.clone()])
            .unwrap();
        let before = rowids(&db, chat);

        let grown = IndexedMessage {
            text: "streaming replaced".into(),
            ..c.clone()
        };
        db.index_chat(chat, 2, 2, &[a.clone(), b.clone(), grown])
            .unwrap();
        let after = rowids(&db, chat);

        // The property: the two untouched messages keep their rowid — which is
        // the FTS docid, so their index entries were never rewritten.
        assert_eq!(after[&a.id], before[&a.id], "untouched message rewritten");
        assert_eq!(after[&b.id], before[&b.id], "untouched message rewritten");
        // The changed one *was* rewritten. Its rowid is deliberately not
        // asserted: it was the highest in the table, so SQLite hands the freed
        // value straight back — the observable effect is on the text, not the id.
        assert_eq!(db.message_count().unwrap(), 3);
        assert_eq!(db.search_chats("\"replaced\"").unwrap(), vec![chat]);
        assert!(
            db.search_chats("\"original\"").unwrap().is_empty(),
            "the superseded text is still searchable"
        );
        assert_eq!(orphan_fts_rows(&db), 0);
    }

    #[test]
    fn reindex_adds_and_removes_messages() {
        let db = cache();
        let chat = Uuid::new_v4();
        let (a, b) = (msg("alpha content"), msg("beta content"));
        db.index_chat(chat, 1, 1, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(db.search_chats("\"beta\"").unwrap(), vec![chat]);

        // Truncation (Ctrl+E / regenerate): the tail is gone from the index.
        db.index_chat(chat, 2, 2, std::slice::from_ref(&a)).unwrap();
        assert_eq!(db.message_count().unwrap(), 1);
        assert!(db.search_chats("\"beta\"").unwrap().is_empty());
        assert_eq!(orphan_fts_rows(&db), 0);

        // And an appended message becomes searchable.
        let c = msg("gamma content");
        db.index_chat(chat, 3, 3, &[a, c]).unwrap();
        assert_eq!(db.search_chats("\"gamma\"").unwrap(), vec![chat]);
        assert_eq!(db.message_count().unwrap(), 2);
        assert_eq!(orphan_fts_rows(&db), 0);
    }

    #[test]
    fn a_stale_writer_cannot_clobber_a_fresher_index() {
        // The lost update this guard exists for, in the order it actually
        // happens: the startup reconciliation reads a chat while it is still
        // empty, the app then saves and indexes the real conversation, and the
        // reconciliation only gets round to writing afterwards. Unguarded, its
        // older snapshot wins and the chat is missing from search until the
        // next launch.
        let db = cache();
        let chat = Uuid::new_v4();

        // What the reconciliation saw when it read: nothing indexed yet.
        let seen_by_reconcile = db.indexed_state().unwrap().get(&chat).copied();
        assert_eq!(seen_by_reconcile, None);

        // Meanwhile the live save indexes the real conversation.
        db.index_chat(chat, 200, 2000, &[msg("настоящая переписка")])
            .unwrap();

        // The reconciliation now tries to write its stale, empty snapshot.
        let wrote = db
            .index_chat_if_unchanged(chat, seen_by_reconcile, 100, 500, &[])
            .unwrap();
        assert!(!wrote, "the stale write must be refused");
        assert_eq!(db.message_count().unwrap(), 1, "the fresh index was wiped");
        assert_eq!(db.search_chats("\"настоящая\"").unwrap(), vec![chat]);
        assert_eq!(
            db.indexed_state().unwrap().get(&chat),
            Some(&(200, 2000)),
            "and the bookkeeping still describes the fresh write"
        );
    }

    #[test]
    fn a_guarded_write_goes_through_when_nothing_moved() {
        // The other half: the guard must not make the reconciliation a no-op.
        let db = cache();
        let chat = Uuid::new_v4();
        db.index_chat(chat, 1, 10, &[msg("старое содержимое")])
            .unwrap();

        let seen = db.indexed_state().unwrap().get(&chat).copied();
        let wrote = db
            .index_chat_if_unchanged(chat, seen, 2, 20, &[msg("новое содержимое")])
            .unwrap();
        assert!(wrote);
        assert_eq!(db.search_chats("\"новое\"").unwrap(), vec![chat]);
        assert!(db.search_chats("\"старое\"").unwrap().is_empty());
        assert_eq!(db.indexed_state().unwrap().get(&chat), Some(&(2, 20)));
    }

    #[test]
    fn forget_chat_drops_rows_and_bookkeeping_of_that_chat_only() {
        let db = cache();
        let (one, two) = (Uuid::new_v4(), Uuid::new_v4());
        db.index_chat(one, 1, 10, &[msg("shared word here")])
            .unwrap();
        db.index_chat(two, 2, 20, &[msg("shared word too")])
            .unwrap();

        db.forget_chat(one).unwrap();

        assert_eq!(db.search_chats("\"shared\"").unwrap(), vec![two]);
        assert_eq!(db.message_count().unwrap(), 1);
        let state = db.indexed_state().unwrap();
        assert!(!state.contains_key(&one), "bookkeeping left behind");
        assert_eq!(state.get(&two), Some(&(2, 20)));
        assert_eq!(orphan_fts_rows(&db), 0);
    }

    #[test]
    fn search_isolates_chats() {
        let db = cache();
        let (one, two) = (Uuid::new_v4(), Uuid::new_v4());
        db.index_chat(one, 1, 1, &[msg("apples and pears")])
            .unwrap();
        db.index_chat(two, 1, 1, &[msg("oranges and lemons")])
            .unwrap();

        assert_eq!(db.search_chats("\"apples\"").unwrap(), vec![one]);
        assert_eq!(db.search_chats("\"oranges\"").unwrap(), vec![two]);
        let both = db.search_chats("\"and\"").unwrap();
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn search_messages_returns_rows_per_message_and_isolates_chats() {
        // Stage 1 answers "which chats mention this?"; stage 2 answers "where
        // exactly" — so the same chat must yield one row per matching message,
        // carrying everything the screen shows.
        let db = cache();
        let (one, two) = (Uuid::new_v4(), Uuid::new_v4());
        let a = IndexedMessage {
            id: Uuid::new_v4(),
            role: "assistant".into(),
            ts: "2026-07-29T10:00:00+00:00".into(),
            text: "первое упоминание маркера".into(),
        };
        let b = msg("второе упоминание маркера");
        db.index_chat(one, 1, 1, &[a.clone(), b.clone(), msg("ничего")])
            .unwrap();
        db.index_chat(two, 1, 1, &[msg("маркера тут тоже")])
            .unwrap();

        let hits = db.search_messages("\"маркера\"", 100).unwrap();
        assert_eq!(hits.len(), 3);
        // Grouping is a single pass: all of a chat's hits are adjacent.
        let chats: Vec<Uuid> = hits.iter().map(|h| h.chat_id).collect();
        let mut deduped = chats.clone();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            2,
            "hits of one chat must be adjacent: {chats:?}"
        );

        let mine: Vec<&MessageHit> = hits.iter().filter(|h| h.chat_id == one).collect();
        assert_eq!(mine.len(), 2);
        let first = mine.iter().find(|h| h.message_id == a.id).unwrap();
        assert_eq!(first.role, "assistant");
        assert_eq!(first.ts, a.ts);
        assert_eq!(
            first.text, a.text,
            "the whole text — the snippet is built from it"
        );
        assert!(mine.iter().any(|h| h.message_id == b.id));

        // A query matching nothing yields nothing (not "everything").
        assert!(
            db.search_messages("\"отсутствует\"", 100)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn search_messages_respects_the_limit_and_the_count_stays_honest() {
        let db = cache();
        let chat = Uuid::new_v4();
        let msgs: Vec<IndexedMessage> = (0..10).map(|i| msg(&format!("совпадение {i}"))).collect();
        db.index_chat(chat, 1, 1, &msgs).unwrap();

        assert_eq!(db.search_messages("\"совпадение\"", 3).unwrap().len(), 3);
        assert_eq!(db.search_messages("\"совпадение\"", 100).unwrap().len(), 10);
        // The cap truncates the rows, never the count — that is what lets the
        // screen say "showing N of M" rather than quietly lying.
        assert_eq!(db.count_matching_messages("\"совпадение\"").unwrap(), 10);
        assert_eq!(db.count_matching_messages("\"нет\"").unwrap(), 0);
    }

    #[test]
    fn matching_messages_in_chat_is_scoped_to_that_chat() {
        let db = cache();
        let (one, two) = (Uuid::new_v4(), Uuid::new_v4());
        let (a, b) = (msg("общее слово раз"), msg("общее слово два"));
        db.index_chat(one, 1, 1, &[a.clone(), msg("прочее"), b.clone()])
            .unwrap();
        db.index_chat(two, 1, 1, &[msg("общее слово чужое")])
            .unwrap();

        let mut ids = db.matching_messages_in_chat("\"общее\"", one).unwrap();
        ids.sort();
        let mut want = vec![a.id, b.id];
        want.sort();
        assert_eq!(ids, want);
        assert_eq!(
            db.matching_messages_in_chat("\"общее\"", two)
                .unwrap()
                .len(),
            1
        );
        assert!(
            db.matching_messages_in_chat("\"прочее\"", two)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn malformed_message_queries_are_errors_not_panics() {
        // Same contract as `search_chats`: escaping is the caller's job, but a
        // slip must surface as an `Err` — these run from a background task.
        let db = cache();
        assert!(db.search_messages("\"unterminated", 10).is_err());
        assert!(db.count_matching_messages("\"unterminated").is_err());
        assert!(
            db.matching_messages_in_chat("\"unterminated", Uuid::new_v4())
                .is_err()
        );
    }

    #[test]
    fn indexed_state_round_trip() {
        let db = cache();
        assert!(db.indexed_state().unwrap().is_empty());

        let chat = Uuid::new_v4();
        db.index_chat(chat, 1_700_000_000_123, 4096, &[msg("x y z")])
            .unwrap();
        assert_eq!(
            db.indexed_state().unwrap().get(&chat),
            Some(&(1_700_000_000_123, 4096))
        );

        // Re-indexing replaces the recorded file state rather than duplicating it.
        db.index_chat(chat, 1_700_000_999_999, 8192, &[msg("x y z")])
            .unwrap();
        let state = db.indexed_state().unwrap();
        assert_eq!(state.len(), 1);
        assert_eq!(state.get(&chat), Some(&(1_700_000_999_999, 8192)));
    }

    #[test]
    fn a_malformed_query_is_an_error_not_a_panic() {
        let db = cache();
        // Unescaped input is the caller's bug (research §4), but it must surface
        // as an `Err` — this layer is called from a background task.
        assert!(db.search_chats("\"unterminated").is_err());
    }

    #[test]
    fn open_wipes_a_database_from_another_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let chat = Uuid::new_v4();
        {
            let db = CacheDb::open(&path).unwrap();
            db.index_chat(chat, 1, 1, &[msg("indexed under the old schema")])
                .unwrap();
            assert_eq!(db.message_count().unwrap(), 1);
        }
        {
            // Stamp a version this build does not index — as a future (or an
            // older) release would leave behind.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        }

        let db = CacheDb::open(&path).unwrap();
        assert_eq!(db.message_count().unwrap(), 0, "stale index kept");
        assert!(db.indexed_state().unwrap().is_empty());
        // And the fresh file is usable and stamped with our version.
        db.index_chat(chat, 2, 2, &[msg("indexed again")]).unwrap();
        assert_eq!(db.search_chats("\"again\"").unwrap(), vec![chat]);
        assert_eq!(
            read_user_version(&db.conn.lock().unwrap()).unwrap(),
            CACHE_SCHEMA
        );
    }

    #[test]
    fn open_wipes_a_file_that_is_not_a_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        std::fs::write(&path, b"this is not a database, it is a note to self").unwrap();

        let db = CacheDb::open(&path).unwrap();
        let chat = Uuid::new_v4();
        db.index_chat(chat, 1, 1, &[msg("recovered")]).unwrap();
        assert_eq!(db.search_chats("\"recovered\"").unwrap(), vec![chat]);
    }

    #[test]
    fn duplicate_message_ids_do_not_fail_the_whole_chat() {
        // Malformed input must not leave a chat unindexed — this is derived data.
        let db = cache();
        let chat = Uuid::new_v4();
        let one = msg("only once please");
        db.index_chat(chat, 1, 1, &[one.clone(), one.clone()])
            .unwrap();
        assert_eq!(db.message_count().unwrap(), 1);
        assert_eq!(db.search_chats("\"once please\"").unwrap(), vec![chat]);
    }

    #[test]
    fn text_hash_is_stable_and_distinguishes() {
        // Pinned values: FNV-1a is specified, so a rewrite that changed the
        // constants (or the byte order) would show up here rather than as a
        // silent full re-index after a toolchain upgrade.
        assert_eq!(text_hash(""), 0xcbf2_9ce4_8422_2325_u64 as i64);
        assert_eq!(text_hash("a"), 0xaf63_dc4c_8601_ec8c_u64 as i64);
        assert_ne!(text_hash("hello"), text_hash("hellp"));
    }

    #[test]
    fn orphan_check_catches_a_missing_delete_trigger() {
        // Mutation test for `orphan_fts_rows`: an orphan check that cannot fail
        // is worse than none, and on an external-content table the obvious
        // spellings genuinely cannot (see the helper's doc). This builds the
        // schema **without** the delete trigger and pins what each form sees.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE messages (
                 id INTEGER PRIMARY KEY, chat_id TEXT, message_id TEXT,
                 text_hash INTEGER, role TEXT, ts TEXT, text TEXT);
             CREATE VIRTUAL TABLE messages_fts USING fts5(
                 text, content='messages', content_rowid='id', tokenize='trigram');
             CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
                 INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
             END;
             -- deliberately no AFTER DELETE trigger
             INSERT INTO messages(chat_id, message_id, text_hash, role, ts, text)
             VALUES ('c', 'm', 0, 'user', 't', 'orphan me');
             DELETE FROM messages;",
        )
        .unwrap();

        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();

        // The bug this causes, stated in user terms: the message is gone, yet a
        // search still matches it — so its chat would still show up in results.
        assert_eq!(
            count("SELECT count(*) FROM messages_fts WHERE messages_fts MATCH '\"orphan\"'"),
            1
        );
        // What the helper checks does see it...
        assert_eq!(
            count(
                "SELECT count(*) FROM messages_fts_docsize d
                 LEFT JOIN messages m ON m.id = d.id WHERE m.id IS NULL"
            ),
            1
        );
        assert!(
            conn.execute_batch(
                "INSERT INTO messages_fts(messages_fts, rank) VALUES('integrity-check', 1);"
            )
            .is_err(),
            "integrity-check(1) must notice the index outliving its content"
        );
        // ...and the two spellings it deliberately avoids do not.
        assert_eq!(
            count(
                "SELECT count(*) FROM messages_fts f
                 LEFT JOIN messages m ON m.id = f.rowid WHERE m.id IS NULL"
            ),
            0,
            "a plain scan of an external-content FTS table reads through to the content table"
        );
        assert!(
            conn.execute_batch("INSERT INTO messages_fts(messages_fts) VALUES('integrity-check');")
                .is_ok(),
            "the bare integrity-check only verifies the index's internal consistency"
        );
    }
}
