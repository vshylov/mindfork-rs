//! The database actor: the thread that owns a story's connections, the
//! registry of the capabilities it issued, and every query
//! (docs/research/fabula-engine.md §6, §9).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, mpsc};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::schema::{self, CALLS_FILE, STORY_FILE};
use super::{
    BranchInfo, CallStatus, CallUsage, CommittedEvent, Mechanism, NewCall, Reflection, StepStatus,
    StoredCall, StoredProposal, StoryDbError,
};
use crate::entities::story::{
    BranchId, Canon, Cause, CharacterKey, Event, EventId, EventKind, NewEvent, Proposal,
    ProposalId, ProposalVerdict, Refusal, RunId, SceneId, SceneSetup, WorldState,
};

/// A job for the actor's thread.
pub(super) type Job = Box<dyn FnOnce(&mut Actor) + Send>;

/// The namespace of call-log ids: a call's id is the UUIDv5 of its run,
/// address and attempt in it (spec §6), so a replay finds it by name.
const CALL_NAMESPACE: Uuid = Uuid::from_u128(0x6d69_6e64_666f_726b_6661_6275_6c61_0001);

/// A guard against a branch chain that never reaches its root. Branches are
/// append-only and a parent exists before its child, so a cycle cannot be
/// stored; the guard turns a hand-edited file into an error, not a hang.
const MAX_DEPTH: usize = 100_000;

/// A proposal and its ruling, as the World Master records them.
pub(super) struct NewProposal {
    pub round: u32,
    pub character: CharacterKey,
    pub proposal: Proposal,
    pub verdict: ProposalVerdict,
    pub reason: Option<String>,
}

/// Who holds which capability. A token names one issue of a capability; a
/// write carries it and must match, and a release frees only its own issue.
#[derive(Default)]
struct Writers {
    next: u64,
    author: Option<u64>,
    run_log: Option<u64>,
    branches: HashMap<BranchId, u64>,
    reflections: HashMap<(BranchId, CharacterKey), u64>,
}

impl Writers {
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

fn check(held: Option<&u64>, token: u64) -> Result<(), StoryDbError> {
    match held {
        Some(&t) if t == token => Ok(()),
        _ => Err(StoryDbError::WriterRevoked),
    }
}

/// A canon document as stored: the canon split by kind and key.
struct Doc {
    kind: &'static str,
    key: String,
    position: i64,
    body: String,
}

/// The latest stored version of one document.
struct Latest {
    version: i64,
    position: i64,
    body: Option<String>,
}

fn docs_of(canon: &Canon) -> Result<Vec<Doc>, serde_json::Error> {
    let single = |kind: &'static str, body: String| Doc {
        kind,
        key: kind.to_owned(),
        position: 0,
        body,
    };
    let mut docs = vec![
        single("world", serde_json::to_string(&canon.world)?),
        single("style", serde_json::to_string(&canon.style)?),
        single("arc", serde_json::to_string(&canon.arc)?),
    ];
    for (i, location) in canon.locations.iter().enumerate() {
        docs.push(Doc {
            kind: "location",
            key: location.key.to_string(),
            position: i as i64,
            body: serde_json::to_string(location)?,
        });
    }
    for (i, character) in canon.characters.iter().enumerate() {
        docs.push(Doc {
            kind: "character",
            key: character.key.to_string(),
            position: i as i64,
            body: serde_json::to_string(character)?,
        });
    }
    Ok(docs)
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

/// An event row as selected, before its JSON columns are parsed.
struct EventRow {
    seq: i64,
    id: String,
    round: u32,
    tick: u32,
    location: String,
    kind: String,
    actor: Option<String>,
    visibility: String,
    observable: String,
    private: Option<String>,
    effects: String,
    proposal: Option<String>,
    cause: String,
}

const EVENT_COLUMNS: &str = "seq, id, round, tick, location_key, kind, actor_key, \
     visibility_json, observable, private, effects_json, proposal_id, cause_json";

impl EventRow {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            seq: r.get(0)?,
            id: r.get(1)?,
            round: r.get(2)?,
            tick: r.get(3)?,
            location: r.get(4)?,
            kind: r.get(5)?,
            actor: r.get(6)?,
            visibility: r.get(7)?,
            observable: r.get(8)?,
            private: r.get(9)?,
            effects: r.get(10)?,
            proposal: r.get(11)?,
            cause: r.get(12)?,
        })
    }

    fn into_event(self, branch: &BranchId) -> Result<Event, StoryDbError> {
        let kind = EventKind::parse(&self.kind).ok_or_else(|| {
            StoryDbError::Corrupt(format!("event {} has kind {:?}", self.id, self.kind))
        })?;
        Ok(Event {
            seq: self.seq as u64,
            id: EventId::new(self.id),
            branch: branch.clone(),
            body: NewEvent {
                round: self.round,
                tick: self.tick,
                location: self.location.as_str().into(),
                kind,
                actor: self.actor.map(CharacterKey::new),
                visibility: serde_json::from_str(&self.visibility)?,
                observable: self.observable,
                private: self.private,
                effects: serde_json::from_str(&self.effects)?,
                proposal: self.proposal.map(ProposalId::new),
                cause: serde_json::from_str(&self.cause)?,
            },
            witnesses: BTreeSet::new(),
        })
    }
}

/// The actor's state: the connections, the issued capabilities, the
/// assembled canon.
pub(super) struct Actor {
    story: Connection,
    calls: Connection,
    writers: Writers,
    canon: Option<Arc<Canon>>,
    closing: Option<oneshot::Sender<()>>,
}

impl Actor {
    pub(super) fn open(story: &Path, calls: &Path) -> Result<Self, StoryDbError> {
        Ok(Self {
            story: schema::open_file(story, &STORY_FILE)?,
            calls: schema::open_file(calls, &CALLS_FILE)?,
            writers: Writers::default(),
            canon: None,
            closing: None,
        })
    }

    /// Runs jobs until every sender is gone or a close arrives; the close's
    /// acknowledgement, sent by the caller once the claim on the file is
    /// given back. The connections close when `self` drops, on return.
    pub(super) fn run(mut self, rx: mpsc::Receiver<Job>) -> Option<oneshot::Sender<()>> {
        while let Ok(job) = rx.recv() {
            job(&mut self);
            if self.closing.is_some() {
                break;
            }
        }
        self.closing.take()
    }

    pub(super) fn close_with(&mut self, ack: oneshot::Sender<()>) {
        self.closing = Some(ack);
    }

    // ---------- capabilities ----------

    pub(super) fn issue_author(&mut self) -> Result<u64, StoryDbError> {
        if self.writers.author.is_some() {
            return Err(StoryDbError::WriterHeld("the Author's writer".into()));
        }
        let token = self.writers.fresh();
        self.writers.author = Some(token);
        Ok(token)
    }

    pub(super) fn release_author(&mut self, token: u64) {
        if self.writers.author == Some(token) {
            self.writers.author = None;
        }
    }

    pub(super) fn issue_run_log(&mut self) -> Result<u64, StoryDbError> {
        if self.writers.run_log.is_some() {
            return Err(StoryDbError::WriterHeld("the run log's writer".into()));
        }
        let token = self.writers.fresh();
        self.writers.run_log = Some(token);
        Ok(token)
    }

    pub(super) fn release_run_log(&mut self, token: u64) {
        if self.writers.run_log == Some(token) {
            self.writers.run_log = None;
        }
    }

    pub(super) fn issue_branch(&mut self, branch: &BranchId) -> Result<u64, StoryDbError> {
        if self.branch_info(branch)?.is_none() {
            return Err(StoryDbError::UnknownBranch(branch.clone()));
        }
        if self.writers.branches.contains_key(branch) {
            return Err(StoryDbError::WriterHeld(format!(
                "the writer of branch {branch}"
            )));
        }
        let token = self.writers.fresh();
        self.writers.branches.insert(branch.clone(), token);
        Ok(token)
    }

    pub(super) fn release_branch(&mut self, branch: &BranchId, token: u64) {
        if self.writers.branches.get(branch) == Some(&token) {
            self.writers.branches.remove(branch);
        }
    }

    pub(super) fn issue_reflection(
        &mut self,
        branch: &BranchId,
        character: &CharacterKey,
    ) -> Result<u64, StoryDbError> {
        if self.branch_info(branch)?.is_none() {
            return Err(StoryDbError::UnknownBranch(branch.clone()));
        }
        self.require_character(character)?;
        let key = (branch.clone(), character.clone());
        if self.writers.reflections.contains_key(&key) {
            return Err(StoryDbError::WriterHeld(format!(
                "the writer of {character}'s reflections on branch {branch}"
            )));
        }
        let token = self.writers.fresh();
        self.writers.reflections.insert(key, token);
        Ok(token)
    }

    pub(super) fn release_reflection(
        &mut self,
        branch: &BranchId,
        character: &CharacterKey,
        token: u64,
    ) {
        let key = (branch.clone(), character.clone());
        if self.writers.reflections.get(&key) == Some(&token) {
            self.writers.reflections.remove(&key);
        }
    }

    // ---------- the canon ----------

    /// The canon assembled from the latest version of every document, cached
    /// until the next import.
    pub(super) fn canon(&mut self) -> Result<Option<Arc<Canon>>, StoryDbError> {
        if self.canon.is_none() {
            self.canon = self.assemble_canon()?.map(Arc::new);
        }
        Ok(self.canon.clone())
    }

    fn require_canon(&mut self) -> Result<Arc<Canon>, StoryDbError> {
        self.canon()?.ok_or(StoryDbError::NoCanon)
    }

    fn require_character(&mut self, character: &CharacterKey) -> Result<(), StoryDbError> {
        if self.require_canon()?.character(character).is_none() {
            return Err(Refusal::UnknownCharacter(character.clone()).into());
        }
        Ok(())
    }

    fn latest_docs(&self) -> Result<BTreeMap<(String, String), Latest>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT kind, key, version, position, body_json FROM canon_doc d
             WHERE version = (SELECT MAX(version) FROM canon_doc m
                              WHERE m.kind = d.kind AND m.key = d.key)",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                Latest {
                    version: r.get(2)?,
                    position: r.get(3)?,
                    body: r.get(4)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn assemble_canon(&self) -> Result<Option<Canon>, StoryDbError> {
        let docs = self.latest_docs()?;
        let body = |kind: &str| {
            docs.get(&(kind.to_owned(), kind.to_owned()))
                .and_then(|d| d.body.as_deref())
        };
        let (Some(world), Some(arc)) = (body("world"), body("arc")) else {
            return Ok(None);
        };
        let style = match body("style") {
            Some(style) => serde_json::from_str(style)?,
            None => Default::default(),
        };
        let mut locations = Vec::new();
        let mut characters = Vec::new();
        for ((kind, _), doc) in &docs {
            let Some(body) = doc.body.as_deref() else {
                continue;
            };
            match kind.as_str() {
                "location" => locations.push((doc.position, serde_json::from_str(body)?)),
                "character" => characters.push((doc.position, serde_json::from_str(body)?)),
                _ => {}
            }
        }
        locations.sort_by_key(|(position, _)| *position);
        characters.sort_by_key(|(position, _)| *position);
        Ok(Some(Canon {
            world: serde_json::from_str(world)?,
            locations: locations.into_iter().map(|(_, l)| l).collect(),
            characters: characters.into_iter().map(|(_, c)| c).collect(),
            style,
            arc: serde_json::from_str(arc)?,
        }))
    }

    pub(super) fn import_canon(
        &mut self,
        token: u64,
        canon: &Canon,
    ) -> Result<usize, StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        let problems = canon.check();
        if !problems.is_empty() {
            return Err(StoryDbError::Canon(problems));
        }
        let has_events: bool =
            self.story
                .query_row("SELECT EXISTS (SELECT 1 FROM event)", [], |r| r.get(0))?;
        if has_events {
            return Err(StoryDbError::CanonFrozen);
        }
        let docs = docs_of(canon)?;
        let latest = self.latest_docs()?;
        let created_at = now();
        let tx = self.story.transaction()?;
        let mut written = 0;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO canon_doc (id, kind, key, version, position, body_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut put =
                |kind: &str, key: &str, version: i64, position: i64, body: Option<&str>| {
                    insert
                        .execute(params![
                            format!("{kind}/{key}/v{version}"),
                            kind,
                            key,
                            version,
                            position,
                            body,
                            created_at,
                        ])
                        .map(|_| ())
                };
            for doc in &docs {
                let previous = latest.get(&(doc.kind.to_owned(), doc.key.clone()));
                let unchanged = previous.is_some_and(|p| {
                    p.body.as_deref() == Some(doc.body.as_str()) && p.position == doc.position
                });
                if unchanged {
                    continue;
                }
                let version = previous.map_or(1, |p| p.version + 1);
                put(doc.kind, &doc.key, version, doc.position, Some(&doc.body))?;
                written += 1;
            }
            for ((kind, key), previous) in &latest {
                let gone = !docs.iter().any(|d| d.kind == kind && &d.key == key);
                if gone && previous.body.is_some() {
                    put(kind, key, previous.version + 1, previous.position, None)?;
                    written += 1;
                }
            }
        }
        tx.commit()?;
        self.canon = None;
        Ok(written)
    }

    // ---------- scenes and picks ----------

    fn scene_exists(&self, scene: &SceneId) -> Result<bool, StoryDbError> {
        Ok(self.story.query_row(
            "SELECT EXISTS (SELECT 1 FROM scene WHERE id = ?1)",
            [scene.as_str()],
            |r| r.get(0),
        )?)
    }

    pub(super) fn add_scene(
        &mut self,
        token: u64,
        (chapter, scene, rerun): (u32, u32, u32),
        setup: &SceneSetup,
        intent: &str,
    ) -> Result<SceneId, StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        let id = SceneId::of(chapter, scene, rerun);
        let canon = self.require_canon()?;
        if canon.location(&setup.location).is_none() {
            return Err(Refusal::UnknownLocation(setup.location.clone()).into());
        }
        if let Some(stranger) = setup.present.iter().find(|c| canon.character(c).is_none()) {
            return Err(Refusal::UnknownCharacter(stranger.clone()).into());
        }
        if self.scene_exists(&id)? {
            return Err(StoryDbError::SceneExists(id));
        }
        self.story.execute(
            "INSERT INTO scene (id, chapter_no, scene_no, rerun, setup_json, author_intent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id.as_str(),
                chapter,
                scene,
                rerun,
                serde_json::to_string(setup)?,
                intent
            ],
        )?;
        Ok(id)
    }

    pub(super) fn record_pick(
        &mut self,
        token: u64,
        scene: &SceneId,
        branch: Option<&BranchId>,
        rationale: &str,
    ) -> Result<(), StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        if !self.scene_exists(scene)? {
            return Err(StoryDbError::UnknownScene(scene.clone()));
        }
        let picked: bool = self.story.query_row(
            "SELECT EXISTS (SELECT 1 FROM pick WHERE scene_id = ?1)",
            [scene.as_str()],
            |r| r.get(0),
        )?;
        if picked {
            return Err(StoryDbError::PickExists(scene.clone()));
        }
        if let Some(branch) = branch {
            let info = self
                .branch_info(branch)?
                .ok_or_else(|| StoryDbError::UnknownBranch(branch.clone()))?;
            if info.scene.as_ref() != Some(scene) {
                return Err(StoryDbError::PickElsewhere {
                    scene: scene.clone(),
                    branch: branch.clone(),
                });
            }
        }
        self.story.execute(
            "INSERT INTO pick (scene_id, branch_id, rationale) VALUES (?1, ?2, ?3)",
            params![scene.as_str(), branch.map(BranchId::as_str), rationale],
        )?;
        Ok(())
    }

    pub(super) fn main_line_tip(&mut self) -> Result<Option<BranchId>, StoryDbError> {
        let picked: Option<String> = self
            .story
            .query_row(
                "SELECT p.branch_id FROM pick p JOIN scene s ON s.id = p.scene_id
                 WHERE p.branch_id IS NOT NULL
                 ORDER BY s.chapter_no DESC, s.scene_no DESC, s.rerun DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(branch) = picked {
            return Ok(Some(BranchId::new(branch)));
        }
        let genesis = BranchId::genesis();
        Ok(self.branch_info(&genesis)?.map(|_| genesis))
    }

    // ---------- branches and the path ----------

    pub(super) fn branch_info(&self, id: &BranchId) -> Result<Option<BranchInfo>, StoryDbError> {
        Ok(self
            .story
            .query_row(
                "SELECT parent_id, fork_seq, scene_id, k, seed FROM branch WHERE id = ?1",
                [id.as_str()],
                |r| {
                    Ok(BranchInfo {
                        id: id.clone(),
                        parent: r.get::<_, Option<String>>(0)?.map(BranchId::new),
                        fork_seq: r.get::<_, Option<i64>>(1)?.map(|s| s as u64),
                        scene: r.get::<_, Option<String>>(2)?.map(SceneId::new),
                        k: r.get(3)?,
                        seed: r.get::<_, i64>(4)? as u64,
                    })
                },
            )
            .optional()?)
    }

    fn insert_branch(&self, info: &BranchInfo) -> Result<(), StoryDbError> {
        self.story.execute(
            "INSERT INTO branch (id, parent_id, fork_seq, scene_id, k, seed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                info.id.as_str(),
                info.parent.as_ref().map(BranchId::as_str),
                info.fork_seq.map(|s| s as i64),
                info.scene.as_ref().map(SceneId::as_str),
                info.k,
                // A seed is a bit pattern; SQLite's integer is signed.
                info.seed as i64,
            ],
        )?;
        Ok(())
    }

    pub(super) fn create_genesis(&mut self) -> Result<u64, StoryDbError> {
        self.require_canon()?;
        let id = BranchId::genesis();
        if self.branch_info(&id)?.is_some() {
            return Err(StoryDbError::BranchExists(id));
        }
        self.insert_branch(&BranchInfo {
            id: id.clone(),
            parent: None,
            fork_seq: None,
            scene: None,
            k: 0,
            seed: 0,
        })?;
        self.issue_branch(&id)
    }

    pub(super) fn create_fork(
        &mut self,
        id: &BranchId,
        scene: &SceneId,
        parent: &BranchId,
        k: u32,
        seed: u64,
    ) -> Result<u64, StoryDbError> {
        if !self.scene_exists(scene)? {
            return Err(StoryDbError::UnknownScene(scene.clone()));
        }
        if self.branch_info(parent)?.is_none() {
            return Err(StoryDbError::UnknownBranch(parent.clone()));
        }
        if self.branch_info(id)?.is_some() {
            return Err(StoryDbError::BranchExists(id.clone()));
        }
        let fork_seq = self.path_tip(parent)?;
        self.insert_branch(&BranchInfo {
            id: id.clone(),
            parent: Some(parent.clone()),
            fork_seq: Some(fork_seq),
            scene: Some(scene.clone()),
            k,
            seed,
        })?;
        self.issue_branch(id)
    }

    /// `branch`'s ancestry, the branch itself first: each with the last
    /// `seq` of its own events the path takes (spec §6, *A branch is a path*).
    fn ancestry(&self, branch: &BranchId) -> Result<Vec<(BranchId, i64)>, StoryDbError> {
        let mut out = Vec::new();
        let mut current = branch.clone();
        let mut limit = i64::MAX;
        loop {
            let Some(info) = self.branch_info(&current)? else {
                return Err(if out.is_empty() {
                    StoryDbError::UnknownBranch(current)
                } else {
                    StoryDbError::Corrupt(format!("the parent branch {current} is missing"))
                });
            };
            out.push((current, limit));
            let (Some(parent), Some(fork_seq)) = (info.parent, info.fork_seq) else {
                break;
            };
            if out.len() > MAX_DEPTH {
                return Err(StoryDbError::Corrupt(format!(
                    "branch {branch} has no root within {MAX_DEPTH} ancestors"
                )));
            }
            limit = limit.min(fork_seq as i64);
            current = parent;
        }
        Ok(out)
    }

    /// The last `seq` on `branch`'s path; 0 when the path is empty.
    fn path_tip(&self, branch: &BranchId) -> Result<u64, StoryDbError> {
        let mut tip = 0;
        for (segment, limit) in self.ancestry(branch)? {
            let max: Option<i64> = self.story.query_row(
                "SELECT MAX(seq) FROM event WHERE branch_id = ?1 AND seq <= ?2",
                params![segment.as_str(), limit],
                |r| r.get(0),
            )?;
            tip = tip.max(max.unwrap_or(0) as u64);
        }
        Ok(tip)
    }

    /// Every event on `branch`'s path with its witnesses, in `seq` order.
    pub(super) fn path_events(&self, branch: &BranchId) -> Result<Vec<Event>, StoryDbError> {
        let mut events = Vec::new();
        let mut witnesses: HashMap<String, BTreeSet<CharacterKey>> = HashMap::new();
        for (segment, limit) in self.ancestry(branch)? {
            let mut stmt = self.story.prepare_cached(&format!(
                "SELECT {EVENT_COLUMNS} FROM event
                 WHERE branch_id = ?1 AND seq <= ?2 ORDER BY seq"
            ))?;
            let rows = stmt.query_map(params![segment.as_str(), limit], EventRow::read)?;
            for row in rows {
                events.push(row?.into_event(&segment)?);
            }
            let mut stmt = self.story.prepare_cached(
                "SELECT w.event_id, w.character_key FROM event_witness w
                 JOIN event e ON e.id = w.event_id
                 WHERE e.branch_id = ?1 AND e.seq <= ?2",
            )?;
            let rows = stmt.query_map(params![segment.as_str(), limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (event, character) = row?;
                witnesses
                    .entry(event)
                    .or_default()
                    .insert(CharacterKey::new(character));
            }
        }
        events.sort_by_key(|e| e.seq);
        for event in &mut events {
            if let Some(seen) = witnesses.remove(event.id.as_str()) {
                event.witnesses = seen;
            }
        }
        Ok(events)
    }

    pub(super) fn world(&mut self, branch: &BranchId) -> Result<WorldState, StoryDbError> {
        let canon = self.require_canon()?;
        let path = self.path_events(branch)?;
        WorldState::fold(&canon, &path).map_err(|e| StoryDbError::Corrupt(e.to_string()))
    }

    // ---------- the event log ----------

    pub(super) fn record_proposal(
        &mut self,
        branch: &BranchId,
        token: u64,
        row: &NewProposal,
    ) -> Result<ProposalId, StoryDbError> {
        check(self.writers.branches.get(branch), token)?;
        self.require_character(&row.character)?;
        let count: i64 = self.story.query_row(
            "SELECT COUNT(*) FROM proposal WHERE branch_id = ?1",
            [branch.as_str()],
            |r| r.get(0),
        )?;
        let id = ProposalId::nth(branch, count as u64 + 1);
        self.story.execute(
            "INSERT INTO proposal (id, branch_id, round, character_key, body_json, verdict, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id.as_str(),
                branch.as_str(),
                row.round,
                row.character.as_str(),
                serde_json::to_string(&row.proposal)?,
                row.verdict.as_str(),
                row.reason,
            ],
        )?;
        Ok(id)
    }

    pub(super) fn proposals(&self, branch: &BranchId) -> Result<Vec<StoredProposal>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT id, round, character_key, body_json, verdict, reason FROM proposal
             WHERE branch_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([branch.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, round, character, body, verdict, reason) = row?;
            let verdict = ProposalVerdict::parse(&verdict).ok_or_else(|| {
                StoryDbError::Corrupt(format!("proposal {id} has verdict {verdict:?}"))
            })?;
            out.push(StoredProposal {
                id: ProposalId::new(id),
                round,
                character: CharacterKey::new(character),
                proposal: serde_json::from_str(&body)?,
                verdict,
                reason,
            });
        }
        Ok(out)
    }

    /// Checks `event` against the world at `branch`'s tip and appends it with
    /// its witnesses, in one transaction (spec §6).
    pub(super) fn commit(
        &mut self,
        branch: &BranchId,
        token: u64,
        event: &NewEvent,
    ) -> Result<CommittedEvent, StoryDbError> {
        check(self.writers.branches.get(branch), token)?;
        let canon = self.require_canon()?;
        let path = self.path_events(branch)?;
        let state =
            WorldState::fold(&canon, &path).map_err(|e| StoryDbError::Corrupt(e.to_string()))?;
        if let Cause::Reaction { to } = &event.cause
            && !path.iter().any(|e| &e.id == to)
        {
            return Err(Refusal::UnknownEvent(to.clone()).into());
        }
        if let Some(proposal) = &event.proposal {
            let owner: Option<String> = self
                .story
                .query_row(
                    "SELECT branch_id FROM proposal WHERE id = ?1",
                    [proposal.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            match owner {
                None => return Err(StoryDbError::UnknownProposal(proposal.clone())),
                Some(owner) if owner != branch.as_str() => {
                    return Err(StoryDbError::ProposalElsewhere {
                        proposal: proposal.clone(),
                        branch: BranchId::new(owner),
                    });
                }
                Some(_) => {}
            }
        }
        let admitted = state.admit(&canon, event)?;
        let own: i64 = self.story.query_row(
            "SELECT COUNT(*) FROM event WHERE branch_id = ?1",
            [branch.as_str()],
            |r| r.get(0),
        )?;
        let id = EventId::nth(branch, own as u64 + 1);
        let tx = self.story.transaction()?;
        tx.execute(
            "INSERT INTO event (id, branch_id, round, tick, location_key, kind, actor_key,
                 visibility_json, observable, private, effects_json, proposal_id, cause_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id.as_str(),
                branch.as_str(),
                event.round,
                event.tick,
                event.location.as_str(),
                event.kind.as_str(),
                event.actor.as_ref().map(CharacterKey::as_str),
                serde_json::to_string(&event.visibility)?,
                event.observable,
                event.private,
                serde_json::to_string(&event.effects)?,
                event.proposal.as_ref().map(ProposalId::as_str),
                serde_json::to_string(&event.cause)?,
            ],
        )?;
        let seq = tx.last_insert_rowid();
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO event_witness (event_id, character_key) VALUES (?1, ?2)",
            )?;
            for witness in &admitted.witnesses {
                insert.execute(params![id.as_str(), witness.as_str()])?;
            }
        }
        tx.commit()?;
        Ok(CommittedEvent {
            id,
            seq: seq as u64,
            witnesses: admitted.witnesses,
        })
    }

    // ---------- reflections ----------

    pub(super) fn write_reflection(
        &mut self,
        branch: &BranchId,
        character: &CharacterKey,
        token: u64,
        body: &str,
        goal_changes: &[String],
    ) -> Result<String, StoryDbError> {
        let key = (branch.clone(), character.clone());
        check(self.writers.reflections.get(&key), token)?;
        let after_seq = self.path_tip(branch)?;
        let count: i64 = self.story.query_row(
            "SELECT COUNT(*) FROM reflection WHERE branch_id = ?1 AND character_key = ?2",
            params![branch.as_str(), character.as_str()],
            |r| r.get(0),
        )?;
        let id = format!("{branch}/{character}/f{}", count + 1);
        self.story.execute(
            "INSERT INTO reflection (id, branch_id, character_key, after_seq, body, goal_changes_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                branch.as_str(),
                character.as_str(),
                after_seq as i64,
                body,
                serde_json::to_string(goal_changes)?,
            ],
        )?;
        Ok(id)
    }

    pub(super) fn reflections(
        &self,
        branch: &BranchId,
        character: &CharacterKey,
    ) -> Result<Vec<Reflection>, StoryDbError> {
        let mut out = Vec::new();
        // Root first, so the oldest reflections come first.
        for (segment, limit) in self.ancestry(branch)?.into_iter().rev() {
            let mut stmt = self.story.prepare_cached(
                "SELECT id, after_seq, body, goal_changes_json FROM reflection
                 WHERE branch_id = ?1 AND character_key = ?2 AND after_seq <= ?3
                 ORDER BY rowid",
            )?;
            let rows =
                stmt.query_map(params![segment.as_str(), character.as_str(), limit], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?;
            for row in rows {
                let (id, after_seq, body, goal_changes) = row?;
                out.push(Reflection {
                    id,
                    branch: segment.clone(),
                    after_seq: after_seq as u64,
                    body,
                    goal_changes: serde_json::from_str(&goal_changes)?,
                });
            }
        }
        Ok(out)
    }

    // ---------- the run log ----------

    fn require_run(&self, run: &RunId) -> Result<(), StoryDbError> {
        let exists: bool = self.story.query_row(
            "SELECT EXISTS (SELECT 1 FROM run WHERE id = ?1)",
            [run.as_str()],
            |r| r.get(0),
        )?;
        if exists {
            Ok(())
        } else {
            Err(StoryDbError::UnknownRun(run.clone()))
        }
    }

    pub(super) fn start_run(&mut self, token: u64, config: &Value) -> Result<RunId, StoryDbError> {
        check(self.writers.run_log.as_ref(), token)?;
        let count: i64 = self
            .story
            .query_row("SELECT COUNT(*) FROM run", [], |r| r.get(0))?;
        let id = RunId::nth(count as u64 + 1);
        self.story.execute(
            "INSERT INTO run (id, config_json, started_at) VALUES (?1, ?2, ?3)",
            params![id.as_str(), serde_json::to_string(config)?, now()],
        )?;
        Ok(id)
    }

    pub(super) fn finish_step(
        &mut self,
        token: u64,
        run: &RunId,
        address: &str,
        status: StepStatus,
    ) -> Result<(), StoryDbError> {
        check(self.writers.run_log.as_ref(), token)?;
        self.require_run(run)?;
        self.story.execute(
            "INSERT INTO step (run_id, address, status, finished_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (run_id, address)
             DO UPDATE SET status = excluded.status, finished_at = excluded.finished_at",
            params![run.as_str(), address, status.as_str(), now()],
        )?;
        Ok(())
    }

    pub(super) fn steps(&self, run: &RunId) -> Result<Vec<(String, StepStatus)>, StoryDbError> {
        let mut stmt = self
            .story
            .prepare_cached("SELECT address, status FROM step WHERE run_id = ?1 ORDER BY rowid")?;
        let rows = stmt.query_map([run.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (address, status) = row?;
            let status = StepStatus::parse(&status).ok_or_else(|| {
                StoryDbError::Corrupt(format!("step {address} has status {status:?}"))
            })?;
            out.push((address, status));
        }
        Ok(out)
    }

    pub(super) fn record_call(
        &mut self,
        token: u64,
        call: &NewCall,
    ) -> Result<String, StoryDbError> {
        check(self.writers.run_log.as_ref(), token)?;
        self.require_run(&call.run)?;
        let id = call_id(&call.run, &call.address, call.attempt);
        let exists: bool = self.calls.query_row(
            "SELECT EXISTS (SELECT 1 FROM llm_call WHERE id = ?1)",
            [id.as_str()],
            |r| r.get(0),
        )?;
        if exists {
            return Err(StoryDbError::CallExists {
                run: call.run.clone(),
                address: call.address.clone(),
                attempt: call.attempt,
            });
        }
        let response = call
            .response
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        self.calls.execute(
            "INSERT INTO llm_call (id, run_id, address, attempt, role, model, params_json,
                 mechanism, request_json, prompt_sha256, response_json, status, input_tokens,
                 output_tokens, cached_tokens, cost_usd, latency_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                id,
                call.run.as_str(),
                call.address,
                call.attempt,
                call.role,
                call.model,
                serde_json::to_string(&call.params)?,
                call.mechanism.as_str(),
                serde_json::to_string(&call.request)?,
                call.prompt_sha256,
                response,
                call.status.as_str(),
                call.usage.input_tokens.map(|n| n as i64),
                call.usage.output_tokens.map(|n| n as i64),
                call.usage.cached_tokens.map(|n| n as i64),
                call.usage.cost_usd,
                call.latency_ms.map(|n| n as i64),
                now(),
            ],
        )?;
        Ok(id)
    }

    pub(super) fn call(
        &self,
        run: &RunId,
        address: &str,
        attempt: u32,
    ) -> Result<Option<StoredCall>, StoryDbError> {
        let row = self
            .calls
            .query_row(
                "SELECT id, role, model, params_json, mechanism, request_json, prompt_sha256,
                        response_json, status, input_tokens, output_tokens, cached_tokens,
                        cost_usd, latency_ms, created_at
                 FROM llm_call WHERE run_id = ?1 AND address = ?2 AND attempt = ?3",
                params![run.as_str(), address, attempt],
                |r| {
                    Ok(CallRow {
                        id: r.get(0)?,
                        role: r.get(1)?,
                        model: r.get(2)?,
                        params: r.get(3)?,
                        mechanism: r.get(4)?,
                        request: r.get(5)?,
                        prompt_sha256: r.get(6)?,
                        response: r.get(7)?,
                        status: r.get(8)?,
                        input_tokens: r.get(9)?,
                        output_tokens: r.get(10)?,
                        cached_tokens: r.get(11)?,
                        cost_usd: r.get(12)?,
                        latency_ms: r.get(13)?,
                        created_at: r.get(14)?,
                    })
                },
            )
            .optional()?;
        row.map(|row| row.into_call(run, address, attempt))
            .transpose()
    }

    #[cfg(test)]
    pub(super) fn execute_raw(&self, sql: &str, calls: bool) -> Result<usize, StoryDbError> {
        let conn = if calls { &self.calls } else { &self.story };
        Ok(conn.execute(sql, [])?)
    }
}

/// A call's id: the UUIDv5 of its run, address and attempt.
fn call_id(run: &RunId, address: &str, attempt: u32) -> String {
    let name = format!("{run}\n{address}\n{attempt}");
    Uuid::new_v5(&CALL_NAMESPACE, name.as_bytes()).to_string()
}

/// A call-log row as selected.
struct CallRow {
    id: String,
    role: String,
    model: String,
    params: String,
    mechanism: String,
    request: String,
    prompt_sha256: String,
    response: Option<String>,
    status: String,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cached_tokens: Option<i64>,
    cost_usd: Option<f64>,
    latency_ms: Option<i64>,
    created_at: String,
}

impl CallRow {
    fn into_call(
        self,
        run: &RunId,
        address: &str,
        attempt: u32,
    ) -> Result<StoredCall, StoryDbError> {
        let corrupt = |what: &str, value: &str| {
            StoryDbError::Corrupt(format!("call {} has {what} {value:?}", self.id))
        };
        let mechanism = Mechanism::parse(&self.mechanism)
            .ok_or_else(|| corrupt("mechanism", &self.mechanism))?;
        let status =
            CallStatus::parse(&self.status).ok_or_else(|| corrupt("status", &self.status))?;
        let count = |n: Option<i64>| n.map(|n| n as u64);
        Ok(StoredCall {
            call: NewCall {
                run: run.clone(),
                address: address.to_owned(),
                attempt,
                role: self.role,
                model: self.model,
                params: serde_json::from_str(&self.params)?,
                mechanism,
                request: serde_json::from_str(&self.request)?,
                prompt_sha256: self.prompt_sha256,
                response: self
                    .response
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?,
                status,
                usage: CallUsage {
                    input_tokens: count(self.input_tokens),
                    output_tokens: count(self.output_tokens),
                    cached_tokens: count(self.cached_tokens),
                    cost_usd: self.cost_usd,
                },
                latency_ms: count(self.latency_ms),
            },
            id: self.id,
            created_at: self.created_at,
        })
    }
}
