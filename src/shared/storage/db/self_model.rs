//! Storage (SQLite) — the self-model: get/upsert/atomic update, and its history
//! (`self_model_versions`, spec §17.9). Part of the [`super`] module; split out
//! of the db.rs monolith (see docs/history/refactoring-god-objects.md, stage 5).

use super::*;
use crate::entities::self_model_history::{SelfModelVersion, VersionAuthor};

impl Db {
    // ---------- self-model (SelfModel) ----------

    /// The profile's self-model (`None` if it hasn't been created yet). Isolated by PK.
    pub fn self_model_get(&self, profile_id: Uuid) -> Result<Option<SelfModel>> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_get_conn(&conn, profile_id)
    }

    /// Saves the self-model (INSERT OR REPLACE), bumping `version`, and records
    /// the version in the history as `author`'s (spec §17.9). The `version`
    /// field on the passed-in model is ignored — the store keeps the
    /// authoritative counter.
    ///
    /// For editing an existing model, [`Self::self_model_update`] is
    /// preferred: it does the read-edit-write **atomically** (under a single
    /// mutex acquisition), ruling out a "read → someone else wrote → wrote
    /// over it" race between background auto-reflection, a manual `F3` edit,
    /// and turn tools. The direct upsert is kept as a symmetric storage
    /// primitive (get/upsert/update) and is used by tests and the demo;
    /// application writers go through `self_model_update`.
    pub fn self_model_upsert(&self, model: &SelfModel, author: &VersionAuthor) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_upsert_conn(&conn, model, author)?;
        Ok(())
    }

    /// An atomic read-edit-write of the profile's model: SELECT + `mutate` +
    /// upsert under a **single** connection-mutex acquisition. `mutate`
    /// returns `true` if the model changed (otherwise no write happens and
    /// `version` doesn't grow). Returns the model after the edit (with the
    /// authoritative `version`/`updated_at` if it was written) and whether it
    /// was written. This rules out a load-modify-save race between
    /// concurrent writers (see the doc on [`Self::self_model_upsert`]). A
    /// write is recorded in the history as `author`'s.
    ///
    /// `mutate` is called while the DB mutex is held — inside it you
    /// **cannot** call other `Db` methods on the same connection (a
    /// reentrant `lock()` on a non-reentrant `Mutex` = a deadlock); it must
    /// be a pure edit of the `SelfModel` value.
    pub fn self_model_update(
        &self,
        profile_id: Uuid,
        author: &VersionAuthor,
        mutate: impl FnOnce(&mut SelfModel) -> bool,
    ) -> Result<(SelfModel, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut model = Self::self_model_get_conn(&conn, profile_id)?
            .unwrap_or_else(|| SelfModel::new(profile_id));
        let changed = mutate(&mut model);
        if changed {
            model = Self::self_model_upsert_conn(&conn, &model, author)?;
        }
        Ok((model, changed))
    }

    /// Deletes the profile's self-model **and its history** — what clearing the
    /// model means (`Ctrl+K` twice on `F3`, `/self clear`; spec §17.9): a
    /// person who clears it wants it forgotten, versions included. The next
    /// write starts the model, and its count, from 1.
    pub fn self_model_delete(&self, profile_id: Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        in_savepoint(&conn, || {
            for table in ["self_models", "self_model_versions"] {
                conn.execute(
                    &format!("DELETE FROM {table} WHERE profile_id = ?1"),
                    params![profile_id.to_string()],
                )?;
            }
            Ok(())
        })
    }

    /// The profile's self-model history, oldest first (spec §17.9). A current
    /// model newer than the newest recorded version — one written before the
    /// history was kept, or by an older build, and not written since — is the
    /// last entry, its author [`VersionAuthor::Unknown`], so the history always
    /// ends at the model as it stands.
    pub fn self_model_history(&self, profile_id: Uuid) -> Result<Vec<SelfModelVersion>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT version, written_at, author, model, mode, chat_id, data
             FROM self_model_versions WHERE profile_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![profile_id.to_string()], |r| {
            Ok(StoredVersion {
                version: r.get(0)?,
                written_at: r.get(1)?,
                author: r.get(2)?,
                model: r.get(3)?,
                mode: r.get(4)?,
                chat_id: r.get(5)?,
                data: r.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.parse()?);
        }
        if let Some(current) = Self::self_model_get_conn(&conn, profile_id)?
            && out.last().is_none_or(|v| v.version < current.version)
        {
            out.push(SelfModelVersion {
                version: current.version,
                written_at: current.updated_at,
                author: VersionAuthor::Unknown,
                model: current,
            });
        }
        Ok(out)
    }

    /// Reads the model over a connection (no mutex acquisition — called while it's held).
    fn self_model_get_conn(conn: &Connection, profile_id: Uuid) -> Result<Option<SelfModel>> {
        let data: Option<String> = conn
            .query_row(
                "SELECT data FROM self_models WHERE profile_id = ?1",
                params![profile_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Writes the model over a connection (no mutex acquisition — called
    /// while it's held) and records the version in the history, both in one
    /// savepoint. Returns the model as actually saved (with the bumped
    /// `version` and a fresh `updated_at`), so the caller doesn't need to
    /// re-read the DB.
    ///
    /// **The version it replaces is caught up first**: when the stored model
    /// is newer than the newest recorded version — written before the history
    /// was kept, or by an older build — it is recorded as
    /// [`VersionAuthor::Unknown`] before it is overwritten, so a history
    /// begins with the model as it was found rather than losing it to the
    /// first write.
    fn self_model_upsert_conn(
        conn: &Connection,
        model: &SelfModel,
        author: &VersionAuthor,
    ) -> Result<SelfModel> {
        in_savepoint(conn, || {
            let pid = model.profile_id.to_string();
            // rusqlite doesn't support u64 in ToSql/FromSql — we store the version as i64.
            let prev: Option<(i64, String, String)> = conn
                .query_row(
                    "SELECT version, data, updated_at FROM self_models WHERE profile_id = ?1",
                    params![pid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((version, data, updated_at)) = &prev {
                let recorded: Option<i64> = conn.query_row(
                    "SELECT MAX(version) FROM self_model_versions WHERE profile_id = ?1",
                    params![pid],
                    |r| r.get(0),
                )?;
                if recorded.is_none_or(|v| v < *version) {
                    let found = Row {
                        version: *version,
                        data,
                        written_at: updated_at,
                    };
                    insert_version(conn, &pid, &found, &VersionAuthor::Unknown)?;
                }
            }
            let mut stored = model.clone();
            stored.version = prev.map_or(0, |(v, _, _)| v).max(0) as u64 + 1;
            stored.updated_at = Utc::now();
            let data = serde_json::to_string(&stored)?;
            let written_at = stored.updated_at.to_rfc3339();
            conn.execute(
                "INSERT INTO self_models(profile_id, data, version, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(profile_id) DO UPDATE SET
                     data = excluded.data,
                     version = excluded.version,
                     updated_at = excluded.updated_at",
                params![pid, data, stored.version as i64, written_at],
            )?;
            let row = Row {
                version: stored.version as i64,
                data: &data,
                written_at: &written_at,
            };
            insert_version(conn, &pid, &row, author)?;
            Ok(stored)
        })
    }
}

/// Runs `f` inside a savepoint — released when it succeeds, rolled back when
/// it fails. A savepoint rather than a transaction because it nests: a test
/// fixture may already hold one open around a seed ([`Db::batch`]).
fn in_savepoint<T>(conn: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    conn.execute_batch("SAVEPOINT self_model")?;
    match f() {
        Ok(value) => {
            conn.execute_batch("RELEASE self_model")?;
            Ok(value)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK TO self_model; RELEASE self_model");
            Err(err)
        }
    }
}

/// A version's stored content, as written into both tables.
struct Row<'a> {
    version: i64,
    data: &'a str,
    written_at: &'a str,
}

/// Appends one version to the history.
fn insert_version(
    conn: &Connection,
    profile_id: &str,
    row: &Row<'_>,
    author: &VersionAuthor,
) -> Result<()> {
    let (model, mode, chat_id) = author.columns();
    conn.execute(
        "INSERT INTO self_model_versions
             (profile_id, version, data, written_at, author, model, mode, chat_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            profile_id,
            row.version,
            row.data,
            row.written_at,
            author.kind_key(),
            model,
            mode,
            chat_id
        ],
    )?;
    Ok(())
}

/// A history row as stored.
struct StoredVersion {
    version: i64,
    written_at: String,
    author: String,
    model: Option<String>,
    mode: Option<String>,
    chat_id: Option<String>,
    data: String,
}

impl StoredVersion {
    /// The row back into a version. Only this module writes the rows, so an
    /// unreadable timestamp or model is a loud error rather than a silent skip
    /// (as in `llm_history`); the author is read leniently
    /// ([`VersionAuthor::from_columns`]).
    fn parse(self) -> Result<SelfModelVersion> {
        let written_at = DateTime::parse_from_rfc3339(&self.written_at)
            .with_context(|| format!("self_model_versions: bad written_at {:?}", self.written_at))?
            .with_timezone(&Utc);
        Ok(SelfModelVersion {
            version: self.version.max(0) as u64,
            written_at,
            author: VersionAuthor::from_columns(
                &self.author,
                self.model,
                self.mode.as_deref(),
                self.chat_id.as_deref(),
            ),
            model: serde_json::from_str(&self.data)
                .context("self_model_versions: unreadable model")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::ServerMode;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn turn(name: &str, chat: Uuid) -> VersionAuthor {
        VersionAuthor::Model {
            name: Some(name.into()),
            mode: ServerMode::External,
            chat_id: Some(chat),
            background: false,
        }
    }

    #[test]
    fn self_model_round_trip_and_isolation() {
        use crate::entities::self_model::SelfModel;
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        // Nothing initially.
        assert!(db.self_model_get(a).unwrap().is_none());

        let mut m = SelfModel::new(a);
        m.summary = "о себе A".into();
        m.add_goal("цель A");
        db.self_model_upsert(&m, &VersionAuthor::App).unwrap();

        let loaded = db.self_model_get(a).unwrap().unwrap();
        assert_eq!(loaded.summary, "о себе A");
        assert_eq!(loaded.goals.len(), 1);
        assert_eq!(loaded.version, 1); // the version is set by the store

        // Profile B doesn't see A's model.
        assert!(db.self_model_get(b).unwrap().is_none());
        assert!(db.self_model_history(b).unwrap().is_empty());

        // A repeat upsert bumps the version and replaces the data.
        let mut m2 = loaded.clone();
        m2.summary = "обновлено".into();
        db.self_model_upsert(&m2, &VersionAuthor::Person).unwrap();
        let reloaded = db.self_model_get(a).unwrap().unwrap();
        assert_eq!(reloaded.summary, "обновлено");
        assert_eq!(reloaded.version, 2);
    }

    #[test]
    fn self_model_update_is_atomic_under_concurrency() {
        use std::sync::Arc;
        // Two threads concurrently append insights to one profile's model.
        // With a non-atomic read-modify-write, some writes would be lost (a
        // "read → someone else wrote → wrote over it" race). `self_model_update`
        // holds SELECT+upsert under a single mutex acquisition — no write is lost.
        let db = Arc::new(db());
        let pid = Uuid::new_v4();
        let n: usize = 50;
        let handles: Vec<_> = ["A", "B"]
            .iter()
            .map(|prefix| {
                let db = db.clone();
                let prefix = prefix.to_string();
                std::thread::spawn(move || {
                    for i in 0..n {
                        db.self_model_update(pid, &VersionAuthor::App, |m| {
                            // Accumulate goals — an exact count (add_goal has no ceiling).
                            m.add_goal(format!("{prefix}{i}"));
                            true
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let m = db.self_model_get(pid).unwrap().unwrap();
        assert_eq!(m.goals.len(), 2 * n);
        assert_eq!(m.version, (2 * n) as u64); // each edit = one upsert
        // ...and each one is a version of its own, in order, none lost.
        let history = db.self_model_history(pid).unwrap();
        let versions: Vec<u64> = history.iter().map(|v| v.version).collect();
        assert_eq!(versions, (1..=(2 * n) as u64).collect::<Vec<_>>());
        assert!(history.iter().all(|v| v.model.version == v.version));
    }

    #[test]
    fn self_model_update_skips_write_when_unchanged() {
        let db = db();
        let pid = Uuid::new_v4();
        // mutate returned false → no write and no version growth, no row appeared in the DB.
        let (model, changed) = db
            .self_model_update(pid, &VersionAuthor::Person, |_m| false)
            .unwrap();
        assert!(!changed);
        assert_eq!(model.version, 0);
        assert!(db.self_model_get(pid).unwrap().is_none());
        assert!(db.self_model_history(pid).unwrap().is_empty());
    }

    #[test]
    fn every_write_is_a_version_with_its_author() {
        let db = db();
        let pid = Uuid::new_v4();
        let chat = Uuid::new_v4();
        db.self_model_update(pid, &turn("gemma-4-31b", chat), |m| {
            m.summary = "I am Sol.".into();
            true
        })
        .unwrap();
        let reflection = VersionAuthor::Model {
            name: Some("qwen-3.8".into()),
            mode: ServerMode::Managed,
            chat_id: Some(chat),
            background: true,
        };
        db.self_model_update(pid, &reflection, |m| {
            m.add_goal("learn chess");
            true
        })
        .unwrap();
        db.self_model_update(pid, &VersionAuthor::Person, |m| {
            m.summary = "I am Sol, Alex's friend.".into();
            true
        })
        .unwrap();

        let history = db.self_model_history(pid).unwrap();
        let shape: Vec<(u64, &VersionAuthor, &str)> = history
            .iter()
            .map(|v| (v.version, &v.author, v.model.summary.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (1, &turn("gemma-4-31b", chat), "I am Sol."),
                (2, &reflection, "I am Sol."),
                (3, &VersionAuthor::Person, "I am Sol, Alex's friend."),
            ]
        );
        // Each version holds the model as it stood then, not as it stands now.
        assert!(history[0].model.goals.is_empty());
        assert_eq!(history[1].model.goals.len(), 1);
        let current = db.self_model_get(pid).unwrap().unwrap();
        assert_eq!(history[2].written_at, current.updated_at);
    }

    /// A model the store already held when the history began — version 81 on
    /// the dev profile — is caught up as a version of unknown authorship by the
    /// first write, rather than lost to it; before that write the history
    /// still ends at it.
    #[test]
    fn a_model_written_before_the_history_is_caught_up() {
        let db = db();
        let pid = Uuid::new_v4();
        let mut old = SelfModel::new(pid);
        old.summary = "written long ago".into();
        old.version = 81;
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO self_models(profile_id, data, version, updated_at)
                 VALUES (?1, ?2, 81, ?3)",
                params![
                    pid.to_string(),
                    serde_json::to_string(&old).unwrap(),
                    old.updated_at.to_rfc3339()
                ],
            )
            .unwrap();
        }
        let before = db.self_model_history(pid).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(
            (before[0].version, &before[0].author),
            (81, &VersionAuthor::Unknown)
        );
        assert_eq!(before[0].model.summary, "written long ago");

        db.self_model_update(pid, &VersionAuthor::Person, |m| {
            m.summary = "rewritten".into();
            true
        })
        .unwrap();
        let after = db.self_model_history(pid).unwrap();
        let shape: Vec<(u64, &VersionAuthor, &str)> = after
            .iter()
            .map(|v| (v.version, &v.author, v.model.summary.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (81, &VersionAuthor::Unknown, "written long ago"),
                (82, &VersionAuthor::Person, "rewritten"),
            ]
        );
    }

    /// An older build writes the model without a version row; the next write
    /// by this one records what the older build left, so the history has a
    /// gap in its numbers but never a model it cannot show.
    #[test]
    fn versions_an_older_build_wrote_are_caught_up_too() {
        let db = db();
        let pid = Uuid::new_v4();
        db.self_model_update(pid, &VersionAuthor::Person, |m| {
            m.summary = "v1".into();
            true
        })
        .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            let mut m = Db::self_model_get_conn(&conn, pid).unwrap().unwrap();
            m.summary = "v5 by an older build".into();
            conn.execute(
                "UPDATE self_models SET data = ?2, version = 5 WHERE profile_id = ?1",
                params![pid.to_string(), serde_json::to_string(&m).unwrap()],
            )
            .unwrap();
        }
        db.self_model_update(pid, &VersionAuthor::Person, |m| {
            m.summary = "v6".into();
            true
        })
        .unwrap();
        let history = db.self_model_history(pid).unwrap();
        let shape: Vec<(u64, &VersionAuthor)> =
            history.iter().map(|v| (v.version, &v.author)).collect();
        assert_eq!(
            shape,
            vec![
                (1, &VersionAuthor::Person),
                (5, &VersionAuthor::Unknown),
                (6, &VersionAuthor::Person),
            ]
        );
        assert_eq!(history[1].model.summary, "v5 by an older build");
    }

    #[test]
    fn deleting_the_model_forgets_its_history_and_restarts_the_count() {
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        for pid in [a, b] {
            for text in ["one", "two"] {
                db.self_model_update(pid, &VersionAuthor::Person, |m| {
                    m.summary = text.into();
                    true
                })
                .unwrap();
            }
        }
        db.self_model_delete(a).unwrap();
        assert!(db.self_model_get(a).unwrap().is_none());
        assert!(db.self_model_history(a).unwrap().is_empty());
        // The other profile's model and history are untouched.
        assert_eq!(db.self_model_history(b).unwrap().len(), 2);

        let (fresh, _) = db
            .self_model_update(a, &VersionAuthor::Person, |m| {
                m.summary = "anew".into();
                true
            })
            .unwrap();
        assert_eq!(fresh.version, 1);
        let history = db.self_model_history(a).unwrap();
        assert_eq!(
            history.len(),
            1,
            "nothing from before the clear is caught up"
        );
    }

    /// The two writes of one version — the model and its history row — land
    /// together or not at all: a failure between them leaves neither.
    #[test]
    fn a_failed_history_write_leaves_the_model_as_it_was() {
        let db = db();
        let pid = Uuid::new_v4();
        db.self_model_update(pid, &VersionAuthor::Person, |m| {
            m.summary = "kept".into();
            true
        })
        .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch("DROP TABLE self_model_versions")
            .unwrap();
        let failed = db.self_model_update(pid, &VersionAuthor::Person, |m| {
            m.summary = "lost".into();
            true
        });
        assert!(failed.is_err());
        let model = db.self_model_get(pid).unwrap().unwrap();
        assert_eq!((model.summary.as_str(), model.version), ("kept", 1));
    }
}
