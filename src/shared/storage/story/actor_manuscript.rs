//! The actor's queries over the manuscript and the pauses (docs/fabula.md §4,
//! PR 6; docs/research/fabula-engine.md §6, §14 *Pauses*): outlines, chapter
//! versions and their summaries, review notes and the decisions on them,
//! questions, their answers and the assumptions; the steps of every run, and
//! what a chapter's calls cost.
//!
//! A child of the actor's module, so it reaches the connections and the
//! registry of capabilities as the actor's own code does.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};

use super::{Actor, check, now};
use crate::entities::story::{
    Assumption, BranchId, ChapterId, ChapterOutline, ChapterVersion, Decided, Decision, EventId,
    NewNote, NoteDecision, NoteId, NoteKind, NoteSeverity, Question, QuestionBody, QuestionId,
    Resolution, ReviewNote, Reviewer, RunId, SceneId, SceneSetup, StoredAssumption, VersionOrigin,
};
use crate::shared::storage::story::{Spend, StepStatus, StoredPick, StoredScene, StoryDbError};

fn corrupt(what: String) -> StoryDbError {
    StoryDbError::Corrupt(what)
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

    // ---------- outlines ----------

    pub(in crate::shared::storage::story) fn add_outline(
        &mut self,
        token: u64,
        chapter: u32,
        outline: &ChapterOutline,
    ) -> Result<(), StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        if self.outline(chapter)?.is_some() {
            return Err(StoryDbError::Exists(format!(
                "the outline of chapter {chapter}"
            )));
        }
        self.story.execute(
            "INSERT INTO outline (chapter_no, body_json) VALUES (?1, ?2)",
            params![chapter, serde_json::to_string(outline)?],
        )?;
        Ok(())
    }

    pub(in crate::shared::storage::story) fn outline(
        &self,
        chapter: u32,
    ) -> Result<Option<ChapterOutline>, StoryDbError> {
        let body: Option<String> = self
            .story
            .query_row(
                "SELECT body_json FROM outline WHERE chapter_no = ?1",
                [chapter],
                |r| r.get(0),
            )
            .optional()?;
        Ok(body.map(|b| serde_json::from_str(&b)).transpose()?)
    }

    // ---------- scenes and picks, read ----------

    pub(in crate::shared::storage::story) fn scenes_of(
        &self,
        chapter: u32,
    ) -> Result<Vec<StoredScene>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT id, scene_no, rerun, setup_json, author_intent FROM scene
             WHERE chapter_no = ?1 ORDER BY scene_no, rerun",
        )?;
        let rows = stmt.query_map([chapter], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, u32>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, scene, rerun, setup, intent) = row?;
            out.push(StoredScene {
                id: SceneId::new(id),
                chapter,
                scene,
                rerun,
                setup: serde_json::from_str::<SceneSetup>(&setup)?,
                intent,
            });
        }
        Ok(out)
    }

    pub(in crate::shared::storage::story) fn pick_of(
        &self,
        scene: &SceneId,
    ) -> Result<Option<StoredPick>, StoryDbError> {
        Ok(self
            .story
            .query_row(
                "SELECT branch_id, rationale FROM pick WHERE scene_id = ?1",
                [scene.as_str()],
                |r| {
                    Ok(StoredPick {
                        branch: r.get::<_, Option<String>>(0)?.map(BranchId::new),
                        rationale: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    // ---------- chapter versions and summaries ----------

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

    fn require_chapter(&self, chapter: &ChapterId) -> Result<(), StoryDbError> {
        let exists: bool = self.story.query_row(
            "SELECT EXISTS (SELECT 1 FROM chapter WHERE id = ?1)",
            [chapter.as_str()],
            |r| r.get(0),
        )?;
        if exists {
            Ok(())
        } else {
            Err(StoryDbError::Unknown(format!("chapter version {chapter}")))
        }
    }

    pub(in crate::shared::storage::story) fn add_summary(
        &mut self,
        token: u64,
        chapter: &ChapterId,
        summary: &str,
    ) -> Result<(), StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        self.require_chapter(chapter)?;
        if self.summary(chapter)?.is_some() {
            return Err(StoryDbError::Exists(format!("the summary of {chapter}")));
        }
        self.story.execute(
            "INSERT INTO chapter_summary (chapter_id, summary) VALUES (?1, ?2)",
            params![chapter.as_str(), summary],
        )?;
        Ok(())
    }

    pub(in crate::shared::storage::story) fn summary(
        &self,
        chapter: &ChapterId,
    ) -> Result<Option<String>, StoryDbError> {
        Ok(self
            .story
            .query_row(
                "SELECT summary FROM chapter_summary WHERE chapter_id = ?1",
                [chapter.as_str()],
                |r| r.get(0),
            )
            .optional()?)
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

    // ---------- reviews ----------

    pub(in crate::shared::storage::story) fn issue_review(
        &mut self,
        chapter: &ChapterId,
        reviewer: Reviewer,
    ) -> Result<u64, StoryDbError> {
        self.require_chapter(chapter)?;
        let key = (chapter.clone(), reviewer);
        if self.writers.reviews.contains_key(&key) {
            return Err(StoryDbError::WriterHeld(format!(
                "the {} writer of {chapter}",
                reviewer.as_str()
            )));
        }
        let token = self.writers.fresh();
        self.writers.reviews.insert(key, token);
        Ok(token)
    }

    pub(in crate::shared::storage::story) fn release_review(
        &mut self,
        chapter: &ChapterId,
        reviewer: Reviewer,
        token: u64,
    ) {
        let key = (chapter.clone(), reviewer);
        if self.writers.reviews.get(&key) == Some(&token) {
            self.writers.reviews.remove(&key);
        }
    }

    /// Writes one review's notes in one transaction, so a review is either
    /// stored whole or not at all, and refuses a second review of the same
    /// version by the same reviewer.
    pub(in crate::shared::storage::story) fn write_notes(
        &mut self,
        chapter: &ChapterId,
        reviewer: Reviewer,
        token: u64,
        notes: &[NewNote],
    ) -> Result<Vec<NoteId>, StoryDbError> {
        check(
            self.writers.reviews.get(&(chapter.clone(), reviewer)),
            token,
        )?;
        let written: bool = self.story.query_row(
            "SELECT EXISTS (SELECT 1 FROM review_note WHERE chapter_id = ?1 AND reviewer = ?2)",
            params![chapter.as_str(), reviewer.as_str()],
            |r| r.get(0),
        )?;
        if written {
            return Err(StoryDbError::Exists(format!(
                "the {} review of {chapter}",
                reviewer.as_str()
            )));
        }
        let tx = self.story.transaction()?;
        let mut ids = Vec::with_capacity(notes.len());
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO review_note
                     (id, chapter_id, reviewer, anchor, kind, severity, body, evidence_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (i, note) in notes.iter().enumerate() {
                let id = NoteId::nth(chapter, reviewer, i as u64 + 1);
                insert.execute(params![
                    id.as_str(),
                    chapter.as_str(),
                    reviewer.as_str(),
                    note.anchor,
                    note.kind.as_str(),
                    note.severity.as_str(),
                    note.body,
                    serde_json::to_string(&note.evidence)?,
                ])?;
                ids.push(id);
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    pub(in crate::shared::storage::story) fn notes(
        &self,
        chapter: &ChapterId,
    ) -> Result<Vec<ReviewNote>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT id, reviewer, anchor, kind, severity, body, evidence_json FROM review_note
             WHERE chapter_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([chapter.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, reviewer, anchor, kind, severity, body, evidence) = row?;
            let bad = |what: &str, value: &str| corrupt(format!("note {id} has {what} {value:?}"));
            out.push(ReviewNote {
                chapter: chapter.clone(),
                reviewer: Reviewer::parse(&reviewer).ok_or_else(|| bad("reviewer", &reviewer))?,
                note: NewNote {
                    kind: NoteKind::parse(&kind).ok_or_else(|| bad("kind", &kind))?,
                    severity: NoteSeverity::parse(&severity)
                        .ok_or_else(|| bad("severity", &severity))?,
                    anchor,
                    body,
                    evidence: serde_json::from_str::<Vec<EventId>>(&evidence)?,
                },
                id: NoteId::new(id),
            });
        }
        Ok(out)
    }

    /// Records the Author's decisions on a version's notes in one
    /// transaction: each note must be the version's, and decided once.
    pub(in crate::shared::storage::story) fn record_decisions(
        &mut self,
        token: u64,
        chapter: &ChapterId,
        decisions: &[Decided],
    ) -> Result<(), StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        let notes = self.notes(chapter)?;
        let decided = self.decisions(chapter)?;
        for d in decisions {
            if !notes.iter().any(|n| n.id == d.note) {
                return Err(StoryDbError::Unknown(format!(
                    "note {} on {chapter}",
                    d.note
                )));
            }
            if decided.iter().any(|e| e.note == d.note)
                || decisions.iter().filter(|e| e.note == d.note).count() > 1
            {
                return Err(StoryDbError::Exists(format!("the decision on {}", d.note)));
            }
        }
        let tx = self.story.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO note_decision (note_id, decision, reason) VALUES (?1, ?2, ?3)",
            )?;
            for d in decisions {
                insert.execute(params![d.note.as_str(), d.decision.as_str(), d.reason])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(in crate::shared::storage::story) fn decisions(
        &self,
        chapter: &ChapterId,
    ) -> Result<Vec<Decided>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT d.note_id, d.decision, d.reason FROM note_decision d
             JOIN review_note n ON n.id = d.note_id
             WHERE n.chapter_id = ?1 ORDER BY n.rowid",
        )?;
        let rows = stmt.query_map([chapter.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (note, decision, reason) = row?;
            let decision = NoteDecision::parse(&decision)
                .ok_or_else(|| corrupt(format!("decision on {note} is {decision:?}")))?;
            out.push(Decided {
                note: NoteId::new(note),
                decision,
                reason,
            });
        }
        Ok(out)
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
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, run, step, body, decision, notes, evidence) = row?;
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
                QuestionBody::Scene { .. } => {}
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(in crate::shared::storage::story) fn add_assumption(
        &mut self,
        token: u64,
        run: &RunId,
        step: &str,
        role: &str,
        assumption: &Assumption,
    ) -> Result<String, StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        self.require_run(run)?;
        let count: i64 = self
            .story
            .query_row("SELECT COUNT(*) FROM assumption", [], |r| r.get(0))?;
        let id = format!("a{}", count + 1);
        self.story.execute(
            "INSERT INTO assumption (id, run_id, step, role, doubt, choice)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                run.as_str(),
                step,
                role,
                assumption.doubt,
                assumption.choice
            ],
        )?;
        Ok(id)
    }

    /// The assumptions made at steps under `prefix` (`ch1/`), oldest first.
    pub(in crate::shared::storage::story) fn assumptions(
        &self,
        prefix: &str,
    ) -> Result<Vec<StoredAssumption>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT id, run_id, step, role, doubt, choice FROM assumption
             WHERE substr(step, 1, length(?1)) = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([prefix], |r| {
            Ok(StoredAssumption {
                id: r.get(0)?,
                run: RunId::new(r.get::<_, String>(1)?),
                step: r.get(2)?,
                role: r.get(3)?,
                assumption: Assumption {
                    doubt: r.get(4)?,
                    choice: r.get(5)?,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- every run's steps, and the spend ----------

    /// How each step address ended across all of the story's runs: done
    /// where any run finished it, failed where runs only failed it (spec §9:
    /// a resume is a new run that starts at the first missing step).
    pub(in crate::shared::storage::story) fn all_steps(
        &self,
    ) -> Result<BTreeMap<String, StepStatus>, StoryDbError> {
        let mut stmt = self
            .story
            .prepare_cached("SELECT address, status FROM step ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (address, status) = row?;
            let status = StepStatus::parse(&status)
                .ok_or_else(|| corrupt(format!("step {address} has status {status:?}")))?;
            let entry = out.entry(address).or_insert(status);
            if status == StepStatus::Done {
                *entry = StepStatus::Done;
            }
        }
        Ok(out)
    }

    /// What the calls at addresses under `prefix` (`ch1/`) cost, across every
    /// run and attempt (docs/fabula.md §6, T1).
    pub(in crate::shared::storage::story) fn spend(
        &self,
        prefix: &str,
    ) -> Result<Spend, StoryDbError> {
        Ok(self.calls.query_row(
            "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    SUM(cost_usd)
             FROM llm_call WHERE substr(address, 1, length(?1)) = ?1",
            [prefix],
            |r| {
                Ok(Spend {
                    calls: r.get::<_, i64>(0)? as u64,
                    input_tokens: r.get::<_, i64>(1)? as u64,
                    output_tokens: r.get::<_, i64>(2)? as u64,
                    cost_usd: r.get(3)?,
                })
            },
        )?)
    }
}
