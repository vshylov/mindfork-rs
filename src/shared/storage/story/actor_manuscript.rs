//! The actor's queries over the manuscript and the pauses (docs/fabula.md §4,
//! PR 6; docs/research/fabula-engine.md §6, §14 *Pauses*): chapter versions,
//! questions and their answers, and what a chapter's calls cost.
//!
//! A child of the actor's module, so it reaches the connections and the
//! registry of capabilities as the actor's own code does.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use super::{Actor, check, now};
use crate::entities::story::{
    ChapterId, ChapterVersion, Decision, Question, QuestionBody, QuestionId, Resolution, RunId,
    VersionOrigin,
};
use crate::shared::storage::story::{Spend, StoryDbError};

fn corrupt(what: String) -> StoryDbError {
    StoryDbError::Corrupt(what)
}

/// A stored timestamp, as the actor writes them (RFC 3339).
fn stamp(at: &str) -> Result<DateTime<Utc>, StoryDbError> {
    DateTime::parse_from_rfc3339(at)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|e| corrupt(format!("timestamp {at:?}: {e}")))
}

impl Actor {
    // ---------- the canon's approval (P1) ----------

    /// Whether every document of the canon as it stands is approved; false
    /// before an import.
    pub(in crate::shared::storage::story) fn canon_approved(
        &mut self,
    ) -> Result<bool, StoryDbError> {
        if self.canon()?.is_none() {
            return Ok(false);
        }
        let open: i64 = self.story.query_row(
            "SELECT COUNT(*) FROM canon_doc d
             WHERE d.body_json IS NOT NULL
               AND d.version = (SELECT MAX(version) FROM canon_doc m
                                WHERE m.kind = d.kind AND m.key = d.key)
               AND NOT EXISTS (SELECT 1 FROM canon_approval a WHERE a.canon_doc_id = d.id)",
            [],
            |r| r.get(0),
        )?;
        Ok(open == 0)
    }

    fn approve_canon(tx: &rusqlite::Transaction<'_>) -> Result<(), StoryDbError> {
        tx.execute(
            "INSERT INTO canon_approval (canon_doc_id, approved_at)
             SELECT d.id, ?1 FROM canon_doc d
             WHERE d.body_json IS NOT NULL
               AND d.version = (SELECT MAX(version) FROM canon_doc m
                                WHERE m.kind = d.kind AND m.key = d.key)
               AND NOT EXISTS (SELECT 1 FROM canon_approval a WHERE a.canon_doc_id = d.id)",
            [now()],
        )?;
        Ok(())
    }

    // ---------- chapter versions ----------

    pub(in crate::shared::storage::story) fn add_chapter(
        &mut self,
        token: u64,
        number: u32,
        origin: VersionOrigin,
        text: &str,
    ) -> Result<ChapterVersion, StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        let latest = self
            .chapter_versions(number)?
            .last()
            .map_or(0, |v| v.version);
        let in_order = match origin {
            VersionOrigin::Draft => latest == 0,
            VersionOrigin::Revision | VersionOrigin::Human => latest > 0,
        };
        if !in_order {
            return Err(StoryDbError::OutOfOrder(format!(
                "a {} of chapter {number} after {latest} versions",
                origin.as_str()
            )));
        }
        let version = latest + 1;
        let id = ChapterId::of(number, version);
        self.story.execute(
            "INSERT INTO chapter (id, number, version, origin, text_md) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.as_str(), number, version, origin.as_str(), text],
        )?;
        Ok(ChapterVersion {
            id,
            number,
            version,
            origin,
            text: text.to_owned(),
        })
    }

    pub(in crate::shared::storage::story) fn chapter_versions(
        &self,
        number: u32,
    ) -> Result<Vec<ChapterVersion>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT id, version, origin, text_md FROM chapter WHERE number = ?1 ORDER BY version",
        )?;
        let rows = stmt.query_map([number], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, version, origin, text) = row?;
            let origin = VersionOrigin::parse(&origin)
                .ok_or_else(|| corrupt(format!("chapter {id} has origin {origin:?}")))?;
            out.push(ChapterVersion {
                id: ChapterId::new(id),
                number,
                version,
                origin,
                text,
            });
        }
        Ok(out)
    }

    pub(in crate::shared::storage::story) fn accepted(
        &self,
        chapter: &ChapterId,
    ) -> Result<bool, StoryDbError> {
        Ok(self.story.query_row(
            "SELECT EXISTS (SELECT 1 FROM chapter_acceptance WHERE chapter_id = ?1)",
            [chapter.as_str()],
            |r| r.get(0),
        )?)
    }

    // ---------- the pauses ----------

    /// Asks a question; refused while another waits for its answer, since a
    /// run stops at its first pause (spec §14).
    pub(in crate::shared::storage::story) fn add_question(
        &mut self,
        token: u64,
        run: &RunId,
        step: &str,
        body: &QuestionBody,
    ) -> Result<Question, StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        self.require_run(run)?;
        if let Some((pending, _)) = self.questions()?.into_iter().find(|(_, a)| a.is_none()) {
            return Err(StoryDbError::QuestionPending(pending.id));
        }
        let count: i64 = self
            .story
            .query_row("SELECT COUNT(*) FROM question", [], |r| r.get(0))?;
        let id = QuestionId::nth(count as u64 + 1);
        self.story.execute(
            "INSERT INTO question (id, run_id, step, kind, body_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id.as_str(),
                run.as_str(),
                step,
                body.kind(),
                serde_json::to_string(body)?,
                now()
            ],
        )?;
        Ok(Question {
            id,
            run: run.clone(),
            step: step.to_owned(),
            body: body.clone(),
        })
    }

    /// Every question with its answer, in the order asked.
    pub(in crate::shared::storage::story) fn questions(
        &self,
    ) -> Result<Vec<(Question, Option<Resolution>)>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT q.id, q.run_id, q.step, q.body_json, a.decision, a.notes, a.evidence_json
             FROM question q LEFT JOIN answer a ON a.question_id = q.id ORDER BY q.rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                (
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ),
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let ((id, run, step, body), decision, notes, evidence) = row?;
            let resolution = match decision {
                None => None,
                Some(decision) => Some(Resolution {
                    decision: Decision::parse(&decision)
                        .ok_or_else(|| corrupt(format!("answer to {id} is {decision:?}")))?,
                    notes: notes.unwrap_or_default(),
                    evidence: serde_json::from_str(evidence.as_deref().unwrap_or("null"))?,
                }),
            };
            out.push((
                Question {
                    id: QuestionId::new(id),
                    run: RunId::new(run),
                    step,
                    body: serde_json::from_str(&body)?,
                },
                resolution,
            ));
        }
        Ok(out)
    }

    /// When the question `question` was asked; `None` for no such question.
    pub(in crate::shared::storage::story) fn asked_at(
        &self,
        question: &QuestionId,
    ) -> Result<Option<DateTime<Utc>>, StoryDbError> {
        let at: Option<String> = self
            .story
            .query_row(
                "SELECT created_at FROM question WHERE id = ?1",
                [question.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        at.map(|at| stamp(&at)).transpose()
    }

    /// When the canon last changed: its newest document's version, an import
    /// that removed one included; `None` before the first import.
    pub(in crate::shared::storage::story) fn canon_changed_at(
        &self,
    ) -> Result<Option<DateTime<Utc>>, StoryDbError> {
        let at: Option<String> =
            self.story
                .query_row("SELECT MAX(created_at) FROM canon_doc", [], |r| r.get(0))?;
        at.map(|at| stamp(&at)).transpose()
    }

    /// Records the human's answer to a question, with what it settles in the
    /// same transaction: an approved canon's approvals (P1), an accepted
    /// chapter's acceptance (P2).
    pub(in crate::shared::storage::story) fn resolve(
        &mut self,
        token: u64,
        question: &QuestionId,
        resolution: &Resolution,
    ) -> Result<(), StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        let (asked, answer) = self
            .questions()?
            .into_iter()
            .find(|(q, _)| &q.id == question)
            .ok_or_else(|| StoryDbError::Unknown(format!("question {question}")))?;
        if answer.is_some() {
            return Err(StoryDbError::Exists(format!("the answer to {question}")));
        }
        let tx = self.story.transaction()?;
        tx.execute(
            "INSERT INTO answer (question_id, decision, notes, evidence_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                question.as_str(),
                resolution.decision.as_str(),
                resolution.notes,
                serde_json::to_string(&resolution.evidence)?,
                now()
            ],
        )?;
        if resolution.decision == Decision::Approve {
            match asked.body {
                QuestionBody::Canon => Self::approve_canon(&tx)?,
                QuestionBody::Chapter { chapter, version } => {
                    tx.execute(
                        "INSERT INTO chapter_acceptance (chapter_id, accepted_at) VALUES (?1, ?2)",
                        params![ChapterId::of(chapter, version).as_str(), now()],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---------- the spend ----------

    /// What chapter `chapter` has cost, every call of it in every run
    /// (docs/fabula.md §6, T1).
    pub(in crate::shared::storage::story) fn chapter_spend(
        &self,
        chapter: u32,
    ) -> Result<Spend, StoryDbError> {
        self.spend(&format!("ch{chapter}/"))
    }

    /// What the calls at addresses under `prefix` (`ch1/`) cost, across every
    /// run of the story and every attempt (docs/fabula.md §6, T1). A run the
    /// call log holds and the story does not is left out: a story restored
    /// from a backup, which carries no log, is behind its log, and that run's
    /// calls built what the restore took back (docs/fabula.md §4, PR 7).
    pub(in crate::shared::storage::story) fn spend(
        &self,
        prefix: &str,
    ) -> Result<Spend, StoryDbError> {
        let mut runs = self.story.prepare_cached("SELECT id FROM run")?;
        let known: BTreeSet<String> = runs
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.calls.prepare_cached(
            "SELECT run_id, COUNT(*), COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0), SUM(cost_usd)
             FROM llm_call WHERE substr(address, 1, length(?1)) = ?1 GROUP BY run_id",
        )?;
        let rows = stmt.query_map([prefix], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Spend {
                    calls: r.get::<_, i64>(1)? as u64,
                    input_tokens: r.get::<_, i64>(2)? as u64,
                    output_tokens: r.get::<_, i64>(3)? as u64,
                    cost_usd: r.get(4)?,
                },
            ))
        })?;
        let mut total = Spend::default();
        for row in rows {
            let (run, spent) = row?;
            if known.contains(&run) {
                total.calls += spent.calls;
                total.input_tokens += spent.input_tokens;
                total.output_tokens += spent.output_tokens;
                total.cost_usd = match (total.cost_usd, spent.cost_usd) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
            }
        }
        Ok(total)
    }
}
