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
}
