//! Storage (SQLite) — the profile's language-model history (spec §9.14):
//! dated records of which LLM answered the profile's exchanges. Part of the
//! [`super`] module. The explicit `rowid INTEGER PRIMARY KEY` is what keeps
//! insertion order stable across `VACUUM` (backup compaction preserves
//! explicit rowids — see [`super::vacuum_into`]).

use super::*;
use crate::entities::profile::LlmChange;
use crate::shared::config::ServerMode;

impl Db {
    /// The profile's language-model history, oldest first. Order is insertion
    /// order (`rowid`), not `changed_at` — two records can share a timestamp.
    pub fn llm_history(&self, profile_id: Uuid) -> Result<Vec<LlmChange>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT changed_at, model, mode FROM llm_history
             WHERE profile_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![profile_id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(parse_record(row?)?);
        }
        Ok(out)
    }

    /// Appends a record when the profile's newest record differs from it by
    /// model name **or** engine mode (spec §9.14) — a read-compare-append
    /// under a single mutex acquisition, like
    /// [`Self::self_model_update`]. Returns whether a record was written.
    pub fn llm_history_note(&self, profile_id: Uuid, rec: &LlmChange) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let last: Option<(String, String)> = conn
            .query_row(
                "SELECT model, mode FROM llm_history
                 WHERE profile_id = ?1 ORDER BY rowid DESC LIMIT 1",
                params![profile_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if last.is_some_and(|(model, mode)| model == rec.model && mode == rec.mode.key()) {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO llm_history(profile_id, changed_at, model, mode)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                profile_id.to_string(),
                rec.changed_at.to_rfc3339(),
                rec.model,
                rec.mode.key(),
            ],
        )?;
        Ok(true)
    }

    /// Fills an **empty** history with `records`, in the order given, and
    /// answers how many rows were written — the one-time backfill of a profile
    /// whose exchanges predate the recorder (spec §9.14). The records are
    /// derived from what the stored replies already attest to; deriving them is
    /// the caller's job (`Orchestrator::seed_llm_history`), because chats live
    /// in JSON, not here.
    ///
    /// "Only an empty history" is checked **inside the transaction** that
    /// writes, so the function is safe to call from anywhere: a history that
    /// gained its first record meanwhile is left alone, and a seed that fails
    /// halfway leaves nothing behind — a partially seeded history would look
    /// non-empty and never be completed.
    pub fn llm_history_seed(&self, profile_id: Uuid, records: &[LlmChange]) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let occupied = tx
            .query_row(
                "SELECT 1 FROM llm_history WHERE profile_id = ?1 LIMIT 1",
                params![profile_id.to_string()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if occupied {
            // Dropping the transaction rolls back a read-only body — nothing
            // to undo, and nothing written.
            return Ok(0);
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO llm_history(profile_id, changed_at, model, mode)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for rec in records {
                stmt.execute(params![
                    profile_id.to_string(),
                    rec.changed_at.to_rfc3339(),
                    rec.model,
                    rec.mode.key(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(records.len())
    }
}

/// A stored row back into the entity. Only this module ever writes the rows,
/// so an unreadable value is a loud error rather than a silent skip.
fn parse_record((changed_at, model, mode): (String, String, String)) -> Result<LlmChange> {
    Ok(LlmChange {
        changed_at: DateTime::parse_from_rfc3339(&changed_at)
            .with_context(|| format!("llm_history: bad changed_at {changed_at:?}"))?
            .with_timezone(&Utc),
        model,
        mode: ServerMode::from_key(&mode)
            .with_context(|| format!("llm_history: unknown mode {mode:?}"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(model: &str, mode: ServerMode) -> LlmChange {
        LlmChange {
            changed_at: Utc::now(),
            model: model.into(),
            mode,
        }
    }

    #[test]
    fn llm_history_round_trip_isolation_and_order() {
        let db = Db::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        assert!(db.llm_history(a).unwrap().is_empty());
        assert!(
            db.llm_history_note(a, &rec("gemma-4", ServerMode::Managed))
                .unwrap()
        );
        assert!(
            db.llm_history_note(a, &rec("qwen-3.6", ServerMode::External))
                .unwrap()
        );

        let list = db.llm_history(a).unwrap();
        assert_eq!(
            list.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(),
            ["gemma-4", "qwen-3.6"]
        );
        assert_eq!(list[0].mode, ServerMode::Managed);
        assert_eq!(list[1].mode, ServerMode::External);

        // Profile B does not see A's history.
        assert!(db.llm_history(b).unwrap().is_empty());
    }

    #[test]
    fn llm_history_note_dedups_against_the_newest_record_only() {
        let db = Db::open_in_memory().unwrap();
        let pid = Uuid::new_v4();

        assert!(
            db.llm_history_note(pid, &rec("m1", ServerMode::Managed))
                .unwrap()
        );
        // Same name and mode — no record.
        assert!(
            !db.llm_history_note(pid, &rec("m1", ServerMode::Managed))
                .unwrap()
        );
        // Same name, different mode — a record (the stored mode must not lie).
        assert!(
            db.llm_history_note(pid, &rec("m1", ServerMode::External))
                .unwrap()
        );
        // A→B→A: the comparison is against the newest record, not "ever seen".
        assert!(
            db.llm_history_note(pid, &rec("m2", ServerMode::External))
                .unwrap()
        );
        assert!(
            db.llm_history_note(pid, &rec("m1", ServerMode::External))
                .unwrap()
        );

        let list = db.llm_history(pid).unwrap();
        assert_eq!(
            list.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(),
            ["m1", "m1", "m2", "m1"]
        );
    }

    /// A dated record, so a seed's order can be told from its timestamps.
    fn dated(ts: &str, model: &str, mode: ServerMode) -> LlmChange {
        LlmChange {
            changed_at: ts.parse::<DateTime<Utc>>().unwrap(),
            model: model.into(),
            mode,
        }
    }

    #[test]
    fn llm_history_seed_fills_an_empty_history_in_the_order_given() {
        let db = Db::open_in_memory().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let records = [
            dated("2026-01-01T10:00:00Z", "gemma-4", ServerMode::Managed),
            dated("2026-02-01T10:00:00Z", "qwen-3.6", ServerMode::External),
            dated("2026-03-01T10:00:00Z", "gemma-4", ServerMode::Managed),
        ];

        assert_eq!(db.llm_history_seed(a, &records).unwrap(), 3);
        let list = db.llm_history(a).unwrap();
        // The stored dates are the derived ones, not the seeding moment: a
        // backfill whose records all carried "now" would say nothing.
        assert_eq!(list, records.to_vec());
        // …and it is a per-profile operation, like every other query here.
        assert!(db.llm_history(b).unwrap().is_empty());

        // The record the recorder appends next dedups against the seed's tail:
        // the seeded history is a real history, not a decoration.
        assert!(
            !db.llm_history_note(a, &rec("gemma-4", ServerMode::Managed))
                .unwrap()
        );
    }

    #[test]
    fn llm_history_seed_never_touches_a_history_that_has_records() {
        let db = Db::open_in_memory().unwrap();
        let pid = Uuid::new_v4();
        assert!(
            db.llm_history_note(pid, &rec("m1", ServerMode::Managed))
                .unwrap()
        );

        // One record is enough to make the profile "already recorded" — the
        // backfill is one-time by construction, not by a flag someone can
        // forget to set.
        assert_eq!(
            db.llm_history_seed(
                pid,
                &[dated("2020-01-01T00:00:00Z", "old", ServerMode::External)]
            )
            .unwrap(),
            0
        );
        let list = db.llm_history(pid).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].model, "m1");
    }

    #[test]
    fn llm_history_seed_of_nothing_writes_nothing() {
        // A profile whose chats attest to no model at all (every reply predates
        // `MessageMetadata.model`) stays empty — and stays *seedable*, so a
        // later launch that does find something still fills it.
        let db = Db::open_in_memory().unwrap();
        let pid = Uuid::new_v4();
        assert_eq!(db.llm_history_seed(pid, &[]).unwrap(), 0);
        assert!(db.llm_history(pid).unwrap().is_empty());
        assert_eq!(
            db.llm_history_seed(pid, &[dated("2026-01-01T00:00:00Z", "m", ServerMode::Grok)])
                .unwrap(),
            1
        );
    }
}
