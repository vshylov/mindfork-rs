//! Storage (SQLite) — the self-model: get/upsert/atomic update. Part of the [`super`] module; split out
//! of the db.rs monolith (see docs/history/refactoring-god-objects.md, stage 5).

use super::*;

impl Db {
    // ---------- self-model (SelfModel) ----------

    /// The profile's self-model (`None` if it hasn't been created yet). Isolated by PK.
    pub fn self_model_get(&self, profile_id: Uuid) -> Result<Option<SelfModel>> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_get_conn(&conn, profile_id)
    }

    /// Saves the self-model (INSERT OR REPLACE), bumping `version`. The
    /// `version` field on the passed-in model is ignored — the store keeps
    /// the authoritative counter.
    ///
    /// For editing an existing model, [`Self::self_model_update`] is
    /// preferred: it does the read-edit-write **atomically** (under a single
    /// mutex acquisition), ruling out a "read → someone else wrote → wrote
    /// over it" race between background auto-reflection, a manual `F3` edit,
    /// and turn tools. The direct upsert is kept as a symmetric storage
    /// primitive (get/upsert/update) and is used by tests; application
    /// writers go through `self_model_update`.
    #[allow(dead_code)]
    pub fn self_model_upsert(&self, model: &SelfModel) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_upsert_conn(&conn, model)?;
        Ok(())
    }

    /// An atomic read-edit-write of the profile's model: SELECT + `mutate` +
    /// upsert under a **single** connection-mutex acquisition. `mutate`
    /// returns `true` if the model changed (otherwise no write happens and
    /// `version` doesn't grow). Returns the model after the edit (with the
    /// authoritative `version`/`updated_at` if it was written) and whether it
    /// was written. This rules out a load-modify-save race between
    /// concurrent writers (see the doc on [`Self::self_model_upsert`]).
    ///
    /// `mutate` is called while the DB mutex is held — inside it you
    /// **cannot** call other `Db` methods on the same connection (a
    /// reentrant `lock()` on a non-reentrant `Mutex` = a deadlock); it must
    /// be a pure edit of the `SelfModel` value.
    pub fn self_model_update(
        &self,
        profile_id: Uuid,
        mutate: impl FnOnce(&mut SelfModel) -> bool,
    ) -> Result<(SelfModel, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut model = Self::self_model_get_conn(&conn, profile_id)?
            .unwrap_or_else(|| SelfModel::new(profile_id));
        let changed = mutate(&mut model);
        if changed {
            model = Self::self_model_upsert_conn(&conn, &model)?;
        }
        Ok((model, changed))
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
    /// while it's held). Returns the model as actually saved (with the
    /// bumped `version` and a fresh `updated_at`), so the caller doesn't
    /// need to re-read the DB.
    fn self_model_upsert_conn(conn: &Connection, model: &SelfModel) -> Result<SelfModel> {
        // rusqlite doesn't support u64 in ToSql/FromSql — we store the version as i64.
        let prev: Option<i64> = conn
            .query_row(
                "SELECT version FROM self_models WHERE profile_id = ?1",
                params![model.profile_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let mut stored = model.clone();
        stored.version = prev.unwrap_or(0).max(0) as u64 + 1;
        stored.updated_at = Utc::now();
        conn.execute(
            "INSERT INTO self_models(profile_id, data, version, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(profile_id) DO UPDATE SET
                 data = excluded.data,
                 version = excluded.version,
                 updated_at = excluded.updated_at",
            params![
                stored.profile_id.to_string(),
                serde_json::to_string(&stored)?,
                stored.version as i64,
                stored.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
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
        db.self_model_upsert(&m).unwrap();

        let loaded = db.self_model_get(a).unwrap().unwrap();
        assert_eq!(loaded.summary, "о себе A");
        assert_eq!(loaded.goals.len(), 1);
        assert_eq!(loaded.version, 1); // the version is set by the store

        // Profile B doesn't see A's model.
        assert!(db.self_model_get(b).unwrap().is_none());

        // A repeat upsert bumps the version and replaces the data.
        let mut m2 = loaded.clone();
        m2.summary = "обновлено".into();
        db.self_model_upsert(&m2).unwrap();
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
                        db.self_model_update(pid, |m| {
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
    }

    #[test]
    fn self_model_update_skips_write_when_unchanged() {
        let db = db();
        let pid = Uuid::new_v4();
        // mutate returned false → no write and no version growth, no row appeared in the DB.
        let (model, changed) = db.self_model_update(pid, |_m| false).unwrap();
        assert!(!changed);
        assert_eq!(model.version, 0);
        assert!(db.self_model_get(pid).unwrap().is_none());
    }
}
