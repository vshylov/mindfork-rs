//! Storage (SQLite) — the follow-ups the companion planned (spec §5.2, §9.3.6;
//! docs/research/companion-platform.md C4 (a)). Part of the [`super`] module.
//! `followup_plan` inserts, the orchestrator reads the pending ones to know when
//! the next falls due and marks each delivered, and the person's cancel, the
//! model's and a conversation's deletion close them.

use super::*;
use crate::entities::followup::{Followup, FollowupState};

const COLUMNS: &str = "id, profile_id, chat_id, about, due_at, created_at, state, closed_at";

impl Db {
    pub fn followup_insert(&self, f: &Followup) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO followups(id, profile_id, chat_id, about, due_at, created_at, state, closed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                f.id.to_string(),
                f.profile_id.to_string(),
                f.chat_id.to_string(),
                f.about,
                f.due_at.to_rfc3339(),
                f.created_at.to_rfc3339(),
                f.state.key(),
                f.closed_at.map(|t| t.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    /// One follow-up by id, whatever its state.
    #[cfg(test)]
    pub fn followup_get(&self, id: Uuid) -> Result<Option<Followup>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM followups WHERE id = ?1"),
                params![id.to_string()],
                row_to_followup,
            )
            .optional()?)
    }

    /// Every pending follow-up of every profile, soonest first — what the
    /// orchestrator schedules from and the tasks screen lists.
    pub fn followups_pending(&self) -> Result<Vec<Followup>> {
        let mut out = self.followups_where("state = 'pending'", params![])?;
        out.sort_by_key(|f| f.due_at);
        Ok(out)
    }

    /// The pending follow-ups of one conversation, soonest first.
    pub fn followups_pending_in_chat(&self, chat_id: Uuid) -> Result<Vec<Followup>> {
        let mut out = self.followups_where(
            "state = 'pending' AND chat_id = ?1",
            params![chat_id.to_string()],
        )?;
        out.sort_by_key(|f| f.due_at);
        Ok(out)
    }

    /// How many follow-ups of the profile are pending.
    pub fn followups_pending_count(&self, profile_id: Uuid) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM followups WHERE state = 'pending' AND profile_id = ?1",
            params![profile_id.to_string()],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// How many of the profile's follow-ups were delivered in `[from, to)` —
    /// the day's count the cap is checked against.
    pub fn followups_delivered_between(
        &self,
        profile_id: Uuid,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<usize> {
        let delivered = self.followups_where(
            "state = 'delivered' AND profile_id = ?1",
            params![profile_id.to_string()],
        )?;
        Ok(delivered
            .iter()
            .filter(|f| f.closed_at.is_some_and(|t| t >= from && t < to))
            .count())
    }

    /// Closes a pending follow-up as delivered (or cancelled) at `at`. Answers
    /// whether it was pending — a follow-up closed meanwhile is left as it is.
    pub fn followup_close(
        &self,
        id: Uuid,
        state: FollowupState,
        at: DateTime<Utc>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE followups SET state = ?2, closed_at = ?3
             WHERE id = ?1 AND state = 'pending'",
            params![id.to_string(), state.key(), at.to_rfc3339()],
        )?;
        Ok(n > 0)
    }

    /// Cancels every pending follow-up of a conversation planned at or after
    /// `since` (every one, with `None`) — the conversation was deleted, or the
    /// exchange that planned them was taken back. Answers how many.
    pub fn followups_cancel_in_chat(
        &self,
        chat_id: Uuid,
        since: Option<DateTime<Utc>>,
    ) -> Result<usize> {
        let pending = self.followups_pending_in_chat(chat_id)?;
        let now = Utc::now();
        let mut n = 0;
        for f in pending
            .iter()
            .filter(|f| since.is_none_or(|t| f.created_at >= t))
        {
            if self.followup_close(f.id, FollowupState::Cancelled, now)? {
                n += 1;
            }
        }
        Ok(n)
    }

    fn followups_where(&self, clause: &str, args: impl rusqlite::Params) -> Result<Vec<Followup>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM followups WHERE {clause}"))?;
        let rows = stmt.query_map(args, row_to_followup)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn row_to_followup(r: &rusqlite::Row) -> rusqlite::Result<Followup> {
    Ok(Followup {
        id: parse_uuid(r.get::<_, String>(0)?),
        profile_id: parse_uuid(r.get::<_, String>(1)?),
        chat_id: parse_uuid(r.get::<_, String>(2)?),
        about: r.get(3)?,
        due_at: parse_dt(r.get::<_, String>(4)?),
        created_at: parse_dt(r.get::<_, String>(5)?),
        state: FollowupState::parse(&r.get::<_, String>(6)?),
        closed_at: r.get::<_, Option<String>>(7)?.map(parse_dt),
    })
}

#[cfg(test)]
mod tests {
    use super::super::Db;
    use crate::entities::followup::{Followup, FollowupState};
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn a_follow_up_is_stored_listed_soonest_first_and_closed_once() {
        let (_d, db) = db();
        let (profile, chat) = (Uuid::new_v4(), Uuid::new_v4());
        let later = Followup::new(profile, chat, "later", Utc::now() + Duration::hours(5));
        let sooner = Followup::new(profile, chat, "sooner", Utc::now() + Duration::hours(1));
        db.followup_insert(&later).unwrap();
        db.followup_insert(&sooner).unwrap();
        let ids: Vec<Uuid> = db
            .followups_pending()
            .unwrap()
            .iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(ids, [sooner.id, later.id]);
        assert_eq!(db.followups_pending_count(profile).unwrap(), 2);

        let at = Utc::now();
        assert!(
            db.followup_close(sooner.id, FollowupState::Delivered, at)
                .unwrap()
        );
        // Closed once: a second close (a cancel racing a delivery) changes nothing.
        assert!(
            !db.followup_close(sooner.id, FollowupState::Cancelled, at)
                .unwrap()
        );
        let got = db.followup_get(sooner.id).unwrap().unwrap();
        assert_eq!(got.state, FollowupState::Delivered);
        assert_eq!(got.about, "sooner");
        assert_eq!(
            db.followups_delivered_between(
                profile,
                at - Duration::minutes(1),
                at + Duration::minutes(1)
            )
            .unwrap(),
            1
        );
        assert_eq!(db.followups_pending_in_chat(chat).unwrap().len(), 1);
    }

    #[test]
    fn a_take_back_cancels_only_what_was_planned_after_it() {
        let (_d, db) = db();
        let (profile, chat) = (Uuid::new_v4(), Uuid::new_v4());
        let mut old = Followup::new(profile, chat, "old", Utc::now() + Duration::hours(2));
        old.created_at = Utc::now() - Duration::hours(3);
        let new = Followup::new(profile, chat, "new", Utc::now() + Duration::hours(2));
        let other = Followup::new(
            profile,
            Uuid::new_v4(),
            "other",
            Utc::now() + Duration::hours(2),
        );
        for f in [&old, &new, &other] {
            db.followup_insert(f).unwrap();
        }
        let cut = Utc::now() - Duration::hours(1);
        assert_eq!(db.followups_cancel_in_chat(chat, Some(cut)).unwrap(), 1);
        let left: Vec<String> = db
            .followups_pending()
            .unwrap()
            .into_iter()
            .map(|f| f.about)
            .collect();
        assert_eq!(left, ["old", "other"]);
        // A deleted conversation takes all of its own with it, and nobody else's.
        assert_eq!(db.followups_cancel_in_chat(chat, None).unwrap(), 1);
        assert_eq!(db.followups_pending().unwrap().len(), 1);
    }
}
