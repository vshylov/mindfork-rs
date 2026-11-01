//! The chapter's loop on a scripted port: the pauses, a chapter from its
//! outline to its checkpoint, the Author's reruns and the last attempt's
//! pick, a scene that fails on every attempt, the budget, the human's
//! revision, and a run killed at every one of its calls that resumes to the
//! same checkpoint.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::{
    Budget, ChapterConfig, Engine, Outcome, Stop, advance, decisions_of, held_to_the_world,
    markers_in, reader_notes, resolve,
};
use crate::entities::story::{
    BranchId, Decision, EventId, NewNote, NoteDecision, NoteKind, NoteSeverity, QuestionBody,
    Resolution, ReviewNote, Reviewer, RunId, VersionOrigin, WorldState,
};
use crate::entities::story::{ChapterId, CharacterKey, SceneId, fixture_canon};
use crate::features::story::author::{ReaderReply, SceneDraft, TriageReply};
use crate::features::story::engine::SharedLog;
use crate::features::story::port::{CallAddress, LlmPort, PortError, Prompt, Role};
use crate::features::story::schema::Schema;
use crate::features::story::schema::describe;
use crate::features::story::simulation::SimConfig;
use crate::features::story::simulation::tests::{feasible, initiative, says, waits};
use crate::shared::storage::story::{AuthorWriter, StepStatus, StoredScene, StoryDb};
use crate::shared::storage::story::{CallStatus, CallUsage, Mechanism, NewCall};

/// What the script answers.
enum Reply {
    Json(Value),
    Text(String),
}

type Answer = dyn Fn(&str, &Prompt) -> Result<Reply, PortError> + Send + Sync;

/// A port that answers each call from a script on its address, holds every
/// JSON answer to its schema, keeps the calls, and with a fuse hangs at its
/// n-th call, as a process killed mid-call would.
struct Script {
    answer: Arc<Answer>,
    seen: Mutex<Vec<(String, Prompt)>>,
    schemas: Mutex<Vec<(String, String)>>,
    count: AtomicUsize,
    fuse: Option<(Fuse, Arc<Notify>)>,
}

/// Where a fused port hangs: at its n-th call, or at its first call under an
/// address.
enum Fuse {
    Nth(usize),
    Under(String),
}

impl Script {
    fn new(answer: Arc<Answer>) -> Self {
        Self {
            answer,
            seen: Mutex::new(Vec::new()),
            schemas: Mutex::new(Vec::new()),
            count: AtomicUsize::new(0),
            fuse: None,
        }
    }

    fn fused(answer: Arc<Answer>, at: usize, blown: Arc<Notify>) -> Self {
        Self {
            fuse: Some((Fuse::Nth(at), blown)),
            ..Self::new(answer)
        }
    }

    fn fused_under(answer: Arc<Answer>, prefix: &str, blown: Arc<Notify>) -> Self {
        Self {
            fuse: Some((Fuse::Under(prefix.to_owned()), blown)),
            ..Self::new(answer)
        }
    }

    fn addresses(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// The schema the call at `address` was sent, as printed.
    fn schema_at(&self, address: &str) -> String {
        self.schemas
            .lock()
            .unwrap()
            .iter()
            .find(|(a, _)| a == address)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| panic!("no JSON call at {address}"))
    }

    fn prompt_at(&self, address: &str) -> Prompt {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|(a, _)| a == address)
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| panic!("no call at {address}"))
    }

    async fn reply(&self, call: &CallAddress, prompt: &Prompt) -> Result<Reply, PortError> {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        let hit = match &self.fuse {
            Some((Fuse::Nth(at), _)) => n == *at,
            Some((Fuse::Under(prefix), _)) => call.address.starts_with(prefix.as_str()),
            None => false,
        };
        if let Some((_, blown)) = &self.fuse
            && hit
        {
            blown.notify_one();
            std::future::pending::<()>().await;
        }
        self.seen
            .lock()
            .unwrap()
            .push((call.address.clone(), prompt.clone()));
        (self.answer)(&call.address, prompt)
    }
}

#[async_trait::async_trait]
impl LlmPort for Script {
    async fn json(
        &self,
        call: &CallAddress,
        _: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Result<Value, PortError> {
        self.schemas
            .lock()
            .unwrap()
            .push((call.address.clone(), schema.to_json()));
        match self.reply(call, prompt).await? {
            Reply::Json(value) => schema.validate(&value).map_err(|v| PortError::Invalid {
                address: call.address.clone(),
                violations: describe(&v),
            }),
            Reply::Text(_) => panic!("{} wants JSON", call.address),
        }
    }

    async fn text(
        &self,
        call: &CallAddress,
        _: Role,
        prompt: &Prompt,
    ) -> Result<String, PortError> {
        match self.reply(call, prompt).await? {
            Reply::Text(text) => Ok(text),
            Reply::Json(_) => panic!("{} wants text", call.address),
        }
    }
}

/// The last segment of an address, and the one before it.
fn tail(address: &str) -> (&str, &str) {
    let mut parts = address.rsplit('/');
    let last = parts.next().unwrap();
    (parts.next().unwrap_or_default(), last)
}

/// The scene number an address names, `ch1/sc2/...` → 2.
fn scene_of(address: &str) -> u32 {
    address
        .split('/')
        .find_map(|p| p.strip_prefix("sc"))
        .and_then(|n| n.parse().ok())
        .unwrap_or_default()
}

/// The labels a prompt lists at the start of its lines after `before`:
/// `Version k0:`, `- n1 ·`.
fn labels_in(prompt: &Prompt, before: &str, prefix: char) -> Vec<String> {
    prompt
        .user
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start_matches("- ").strip_prefix(before)?;
            let label: String = rest
                .chars()
                .take_while(|c| *c == prefix || c.is_ascii_digit())
                .collect();
            (label.len() > 1 && label.starts_with(prefix)).then_some(label)
        })
        .collect()
}

fn scene_plan(m: u32) -> Value {
    json!({
        "next": {
            "type": "scene",
            "intent": format!("Scene {m} tightens the screw."),
            "location": "harbour",
            "present": ["mara", "silas", "mara"],
            "just_happened": "The ferry was cancelled.",
            "pressure": "The gate closes at sunset.",
        },
        "assumptions": [],
    })
}

fn pick_first(prompt: &Prompt) -> Value {
    let shown = labels_in(prompt, "Version ", 'k');
    json!({
        "readings": shown.iter().map(|k| json!({
            "branch": k, "note": "It holds.", "tension": 3, "surprise": 3, "truth": 4,
            "consequences": 3,
        })).collect::<Vec<_>>(),
        "human_note": "The first version keeps the pressure on.",
        "choice": {"type": "branch", "branch": shown[0]},
        "assumptions": [],
    })
}

/// The story every test starts from: Mara speaks, everyone else waits, the
/// World Master lets her, two scenes then the end; the reviewers raise one
/// high note on the draft, which the Author accepts, and nothing after.
fn story(address: &str, prompt: &Prompt) -> Result<Reply, PortError> {
    let (who, act) = tail(address);
    let value = match act {
        "propose" if who == "mara" => says("mara", "We wait out the storm."),
        "propose" | "react" => waits(),
        "initiative" => initiative(
            vec![feasible("mara")],
            Some("mara"),
            "Mara says: \"We wait out the storm.\"",
            json!([]),
        ),
        "outline" => json!({
            "goals": ["The secret surfaces."],
            "assumptions": [{"doubt": "Is Ines new in town?", "choice": "Yes, a month."}],
        }),
        "plan" | "again" | "replan" if scene_of(address) <= 2 => scene_plan(scene_of(address)),
        "plan" => json!({"next": {"type": "end"}, "assumptions": []}),
        "pick" => pick_first(prompt),
        "write" => return Ok(Reply::Text("## The storm\n\nMara waits.".into())),
        "revise" => {
            return Ok(Reply::Text(format!(
                "## The storm\n\nRevised at {address}."
            )));
        }
        "summary" => return Ok(Reply::Text("Mara keeps the harbour shut.".into())),
        "reader" => json!({
            "confusions": [{"anchor": "Mara waits", "note": "Who is Mara?", "severity": "high"}],
            "slow_passages": [],
            "predictions": ["Silas runs for the boats."],
            "best_moment": "The quay in the rain.",
        }),
        "continuity" => json!({"findings": [{
            "anchor": "Mara waits",
            "evidence": ["ch1/sc1/r0/k0/e3", "`ch1/sc2/r0/k0/e3`", "ch1/sc1/r0/k1/e3"],
            "note": "She spoke; she did not only wait.",
            "kind": "contradiction",
            "severity": "low",
        }]}),
        "triage" => {
            let accept_first = address.contains("/v1/");
            let decisions: Vec<Value> = labels_in(prompt, "", 'n')
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    let decision = if accept_first && i == 0 {
                        "accepted"
                    } else {
                        "rejected"
                    };
                    json!({"note": label, "critique": "Fair enough.", "decision": decision})
                })
                .collect();
            json!({"decisions": decisions})
        }
        other => panic!("no script for {other} at {address}"),
    };
    Ok(Reply::Json(value))
}

/// The same with one round a branch.
fn short() -> ChapterConfig {
    let mut config = config();
    config.sim.max_rounds = 1;
    config
}

fn config() -> ChapterConfig {
    ChapterConfig {
        sim: SimConfig {
            branches: 2,
            max_rounds: 2,
            abnormal_p: 0.0,
            reflect_every: 0,
        },
        human_language: "Russian".to_owned(),
        ..ChapterConfig::default()
    }
}

/// An open story with the harbour canon, the Author's capability and the run
/// log, shared.
struct Open {
    _dir: tempfile::TempDir,
    db: StoryDb,
    author: AuthorWriter,
    log: SharedLog,
}

async fn open() -> Open {
    let dir = tempfile::tempdir().unwrap();
    let db = StoryDb::open(
        &dir.path().join("story.db"),
        &dir.path().join("story.calls.db"),
    )
    .await
    .unwrap();
    let mut author = db.author_writer().await.unwrap();
    author.import_canon(&fixture_canon()).await.unwrap();
    let log = Arc::new(tokio::sync::Mutex::new(db.run_log_writer().await.unwrap()));
    Open {
        _dir: dir,
        db,
        author,
        log,
    }
}

impl Open {
    async fn advance(&mut self, port: &dyn LlmPort, config: &ChapterConfig) -> Outcome {
        advance(Engine {
            db: &self.db,
            author: &mut self.author,
            log: &self.log,
            port,
            config,
        })
        .await
        .unwrap()
    }

    async fn answer(&mut self, decision: Decision, notes: &str) {
        let question = self
            .db
            .pending_question()
            .await
            .unwrap()
            .expect("a question waits");
        resolve(
            &mut self.author,
            &question.id,
            &Resolution {
                decision,
                notes: notes.into(),
                evidence: json!(null),
            },
        )
        .await
        .unwrap();
    }

    /// Opened, with the canon approved.
    async fn approved() -> Self {
        let mut open = Self::open_only().await;
        let port = Script::new(Arc::new(story));
        assert_eq!(
            open.advance(&port, &config()).await,
            Outcome::Paused(open.db.pending_question().await.unwrap().unwrap())
        );
        open.answer(Decision::Approve, "").await;
        open
    }

    async fn open_only() -> Self {
        open().await
    }
}

fn paused_at(outcome: &Outcome) -> &QuestionBody {
    match outcome {
        Outcome::Paused(q) => &q.body,
        other => panic!("not paused: {other:?}"),
    }
}

#[tokio::test]
async fn a_story_asks_for_its_canon_first_and_a_pending_question_starts_no_run() {
    let mut open = open().await;
    let port = Script::new(Arc::new(story));
    let first = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&first), &QuestionBody::Canon);
    assert!(port.addresses().is_empty(), "nothing is asked of a model");
    let again = open.advance(&port, &config()).await;
    assert_eq!(again, first, "the same question, still waiting");
    // One run asked the question; the second returned before starting one.
    let mut log = open.log.lock().await;
    assert!(log.start_run(&json!({})).await.unwrap() == RunId::nth(2));
}

#[tokio::test]
async fn a_chapter_runs_from_its_outline_to_its_checkpoint() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(
        paused_at(&outcome),
        &QuestionBody::Chapter {
            chapter: 1,
            version: 2
        }
    );
    let db = &open.db;
    assert_eq!(
        db.outline(1).await.unwrap().unwrap().goals,
        ["The secret surfaces."]
    );
    // Two scenes, each with one pick, then the Author ended the chapter.
    let scenes = db.scenes_of(1).await.unwrap();
    assert_eq!(
        scenes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["ch1/sc1/r0", "ch1/sc2/r0"]
    );
    assert_eq!(
        scenes[0].setup.present,
        [CharacterKey::from("mara"), CharacterKey::from("silas")],
        "a character named twice is present once"
    );
    for scene in &scenes {
        let pick = db.pick_of(&scene.id).await.unwrap().unwrap();
        assert_eq!(pick.branch.unwrap().as_str(), format!("{}/k0", scene.id));
        assert_eq!(pick.rationale, "The first version keeps the pressure on.");
    }
    // The second scene forked from the first one's pick.
    let second = db
        .branch(&BranchId::new("ch1/sc2/r0/k1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.parent.unwrap().as_str(), "ch1/sc1/r0/k0");
    // The draft, its accepted high note, the revision, and its rejections.
    let versions = db.chapter_versions(1).await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.origin).collect::<Vec<_>>(),
        [VersionOrigin::Draft, VersionOrigin::Revision]
    );
    let v1 = &versions[0];
    let notes = db.notes(&v1.id).await.unwrap();
    assert_eq!(
        notes.iter().map(|n| n.note.kind).collect::<Vec<_>>(),
        [
            NoteKind::Confusion,
            NoteKind::Prediction,
            NoteKind::BestMoment,
            NoteKind::Contradiction
        ]
    );
    assert_eq!(
        notes[3].note.evidence,
        [
            EventId::new("ch1/sc1/r0/k0/e3"),
            EventId::new("ch1/sc2/r0/k0/e3")
        ],
        "a quoted id is read, and one of a branch the chapter did not keep is dropped"
    );
    let decisions = db.decisions(&v1.id).await.unwrap();
    assert_eq!(
        decisions.iter().map(|d| d.decision).collect::<Vec<_>>(),
        [NoteDecision::Accepted, NoteDecision::Rejected],
        "the Reader's observations are not triaged"
    );
    let v2 = &versions[1];
    assert_eq!(v2.text, "## The storm\n\nRevised at ch1/v2/revise.");
    assert!(
        db.decisions(&v2.id)
            .await
            .unwrap()
            .iter()
            .all(|d| d.decision == NoteDecision::Rejected)
    );
    assert_eq!(
        db.summary(&v2.id).await.unwrap().as_deref(),
        Some("Mara keeps the harbour shut.")
    );
    assert_eq!(db.summary(&v1.id).await.unwrap(), None);
    let assumptions = db.assumptions("ch1/").await.unwrap();
    assert_eq!(assumptions.len(), 1);
    assert_eq!(assumptions[0].step, "ch1/outline");
    // The calls, in their order.
    let calls = port.addresses();
    let authored: Vec<&str> = calls
        .iter()
        .map(String::as_str)
        .filter(|a| !a.contains("/round"))
        .collect();
    assert_eq!(
        authored,
        [
            "ch1/outline",
            "ch1/sc1/plan",
            "ch1/sc1/r0/pick",
            "ch1/sc2/plan",
            "ch1/sc2/r0/pick",
            "ch1/sc3/plan",
            "ch1/v1/write",
            "ch1/v1/reader",
            "ch1/v1/continuity",
            "ch1/v1/triage",
            "ch1/v2/revise",
            "ch1/v2/reader",
            "ch1/v2/continuity",
            "ch1/v2/triage",
            "ch1/v2/summary",
        ]
    );
    // What each step was shown.
    let revise = port.prompt_at("ch1/v2/revise");
    assert!(revise.user.contains("Who is Mara?"), "the accepted note");
    assert!(!revise.user.contains("She spoke"), "not the rejected one");
    assert!(revise.user.contains("Mara waits."), "the text revised");
    let summary = port.prompt_at("ch1/v2/summary");
    assert!(summary.user.contains("in Russian"));
    assert!(summary.user.contains("Revised at ch1/v2/revise."));
    let reader = port.prompt_at("ch1/v1/reader");
    assert!(!reader.system.contains("bribe") && !reader.user.contains("bribe"));
    assert!(reader.user.contains("first chapter"));
    let plan = port.prompt_at("ch1/sc2/plan");
    assert!(
        plan.user.contains("We wait out the storm."),
        "what scene 1 kept"
    );
    assert!(plan.user.contains("Or end the chapter"));
    assert!(
        !port
            .prompt_at("ch1/sc1/plan")
            .user
            .contains("end the chapter")
    );
}

#[tokio::test]
async fn an_accepted_chapter_finishes_a_one_chapter_arc_and_exports_it() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    open.answer(Decision::Approve, "More rain next time.").await;
    let calls = port.addresses().len();
    assert_eq!(open.advance(&port, &config()).await, Outcome::Finished);
    assert_eq!(port.addresses().len(), calls, "nothing more is asked");
    assert!(open.db.accepted(&ChapterId::of(1, 2)).await.unwrap());
    let text = super::super::export::markdown(&open.db).await.unwrap();
    assert_eq!(text, "## The storm\n\nRevised at ch1/v2/revise.\n");
}

#[tokio::test]
async fn the_next_chapter_reads_the_last_ones_summary_text_and_notes() {
    let mut open = open().await;
    let mut canon = fixture_canon();
    canon.arc.chapters.push(crate::entities::story::ChapterArc {
        goal: "Ines decides whom to trust.".into(),
    });
    open.author.import_canon(&canon).await.unwrap();
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    open.answer(Decision::Approve, "").await;
    open.advance(&port, &config()).await;
    open.answer(Decision::Approve, "More rain next time.").await;
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(
        paused_at(&outcome),
        &QuestionBody::Chapter {
            chapter: 2,
            version: 2
        }
    );
    let outline = port.prompt_at("ch2/outline");
    assert!(
        outline
            .user
            .contains("Chapter 2 of 2. Its goal in the arc: Ines decides whom to trust.")
    );
    assert!(
        outline
            .user
            .contains("Chapter 1: Mara keeps the harbour shut."),
        "the summary"
    );
    assert!(
        outline.user.contains("Revised at ch1/v2/revise."),
        "the chapter as accepted"
    );
    assert!(
        outline.user.contains("More rain next time."),
        "the human's notes"
    );
    let reader = port.prompt_at("ch2/v1/reader");
    assert!(
        reader
            .user
            .contains("Chapter 1:\n\n## The storm\n\nRevised at ch1/v2/revise.")
    );
    // Chapter 2's first scene forked from chapter 1's last pick.
    let first = open
        .db
        .branch(&BranchId::new("ch2/sc1/r0/k0"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.parent.unwrap().as_str(), "ch1/sc2/r0/k0");
    open.answer(Decision::Approve, "").await;
    assert_eq!(open.advance(&port, &config()).await, Outcome::Finished);
    let text = super::super::export::markdown(&open.db).await.unwrap();
    assert_eq!(text.matches("## The storm").count(), 2);
}

#[tokio::test]
async fn a_change_at_the_checkpoint_revises_with_the_humans_notes() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    open.answer(Decision::Change, "Make it shorter, and let it rain.")
        .await;
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(
        paused_at(&outcome),
        &QuestionBody::Chapter {
            chapter: 1,
            version: 3
        }
    );
    let versions = open.db.chapter_versions(1).await.unwrap();
    assert_eq!(versions[2].origin, VersionOrigin::Human);
    let prompt = port.prompt_at("ch1/v3/revise");
    assert!(prompt.user.contains("Make it shorter, and let it rain."));
    assert!(prompt.user.contains("as the story's human author asked"));
    assert!(
        open.db.notes(&versions[2].id).await.unwrap().is_empty(),
        "not reviewed again"
    );
    assert!(open.db.summary(&versions[2].id).await.unwrap().is_some());
}

#[tokio::test]
async fn the_authors_reruns_play_the_scene_again_and_the_last_attempt_may_only_pick() {
    let mut open = Open::approved().await;
    let rerun = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        if address.starts_with("ch1/sc1/") && address.ends_with("/pick") {
            assert!(prompt.user.contains("rerun") || prompt.user.contains("No reruns"));
            if !prompt.user.contains("No reruns are left") {
                return Ok(Reply::Json(json!({
                    "readings": [],
                    "human_note": "Neither version bites; move them to the warehouse.",
                    "choice": {
                        "type": "rerun",
                        "intent": "Closer quarters.",
                        "location": "warehouse",
                        "present": ["mara", "silas"],
                        "just_happened": "Silas asked to see his crates.",
                        "pressure": "Customs arrive at dawn.",
                    },
                    "assumptions": [{"doubt": "Is the warehouse locked?", "choice": "Yes."}],
                })));
            }
        }
        story(address, prompt)
    };
    let port = Script::new(Arc::new(rerun));
    open.advance(&port, &config()).await;
    let scenes = open.db.scenes_of(1).await.unwrap();
    let first: Vec<&StoredScene> = scenes.iter().filter(|s| s.scene == 1).collect();
    assert_eq!(first.len(), 3, "the first attempt and two reruns");
    assert_eq!(first[1].setup.location.as_str(), "warehouse");
    assert_eq!(first[1].intent, "Closer quarters.");
    let picks: Vec<Option<String>> = {
        let mut out = Vec::new();
        for s in &first {
            out.push(
                open.db
                    .pick_of(&s.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .branch
                    .map(|b| b.to_string()),
            );
        }
        out
    };
    assert_eq!(picks, [None, None, Some("ch1/sc1/r2/k0".to_owned())]);
    let last = port.prompt_at("ch1/sc1/r2/pick");
    assert!(last.user.contains("No reruns are left"));
    assert!(
        !port
            .schema_at("ch1/sc1/r2/pick")
            .contains(r#""enum":["rerun"]"#),
        "the last attempt may only pick"
    );
    assert!(
        port.schema_at("ch1/sc1/r1/pick")
            .contains(r#""enum":["rerun"]"#)
    );
    let left = port.prompt_at("ch1/sc1/r1/pick");
    assert!(left.user.contains("1 rerun left"));
    // Every rerun forked from the same tip: genesis.
    for k in ["ch1/sc1/r0/k0", "ch1/sc1/r2/k1"] {
        let info = open.db.branch(&BranchId::new(k)).await.unwrap().unwrap();
        assert_eq!(info.parent, Some(BranchId::genesis()));
    }
    assert_eq!(open.db.assumptions("ch1/sc1/").await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_scene_that_fails_on_every_attempt_asks_the_human_and_their_answer_replans_it() {
    let mut open = Open::approved().await;
    let failing = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        let doomed = ["ch1/sc1/r0/", "ch1/sc1/r1/", "ch1/sc1/r2/"];
        if doomed.iter().any(|d| address.starts_with(d)) && address.ends_with("/propose") {
            return Err(PortError::Cut {
                address: address.to_owned(),
            });
        }
        story(address, prompt)
    };
    let port = Script::new(Arc::new(failing));
    let outcome = open.advance(&port, &config()).await;
    let QuestionBody::Scene {
        chapter,
        scene,
        rerun,
        errors,
    } = paused_at(&outcome)
    else {
        panic!("not P3: {outcome:?}")
    };
    assert_eq!((*chapter, *scene, *rerun), (1, 1, 2));
    assert_eq!(errors.len(), 2);
    assert!(errors[0].starts_with("ch1/sc1/r2/k0: "), "{errors:?}");
    let attempts = open.db.scenes_of(1).await.unwrap();
    assert_eq!(attempts.len(), 3);
    assert!(
        attempts.windows(2).all(|w| w[0].setup == w[1].setup),
        "a failed attempt is played again as it was"
    );
    assert!(!port.addresses().iter().any(|a| a.ends_with("/pick")));

    open.answer(Decision::Answer, "Put Ines in the scene.")
        .await;
    let outcome = open.advance(&port, &config()).await;
    assert!(matches!(paused_at(&outcome), QuestionBody::Chapter { .. }));
    let replan = port.prompt_at("ch1/sc1/r2/replan");
    assert!(replan.user.contains("Put Ines in the scene."));
    let kept = open
        .db
        .pick_of(&SceneId::of(1, 1, 3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.branch.unwrap().as_str(), "ch1/sc1/r3/k0");
    let superseded = open
        .db
        .pick_of(&SceneId::of(1, 1, 2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(superseded.branch, None);
}

#[tokio::test]
async fn the_budget_stops_the_run_before_its_next_call_and_a_larger_one_resumes() {
    let mut open = Open::approved().await;
    {
        let mut log = open.log.lock().await;
        let run = log.start_run(&json!({})).await.unwrap();
        log.record_call(NewCall {
            run,
            address: "ch1/earlier".to_owned(),
            attempt: 1,
            role: "author".to_owned(),
            model: "m".to_owned(),
            params: json!({}),
            mechanism: Mechanism::Text,
            request: json!({}),
            prompt_sha256: "h".to_owned(),
            response: None,
            status: CallStatus::Ok,
            usage: CallUsage {
                input_tokens: Some(900),
                output_tokens: Some(100),
                cached_tokens: None,
                cost_usd: Some(0.02),
            },
            latency_ms: None,
        })
        .await
        .unwrap();
    }
    let port = Script::new(Arc::new(story));
    let tight = ChapterConfig {
        budget: Budget {
            tokens: Some(1000),
            usd: None,
        },
        ..config()
    };
    let outcome = open.advance(&port, &tight).await;
    assert_eq!(
        outcome,
        Outcome::Stopped {
            step: "ch1/outline".to_owned(),
            why: Stop::Budget("1000 tokens of 1000".to_owned()),
        }
    );
    assert!(port.addresses().is_empty());
    assert_eq!(
        open.db.all_steps().await.unwrap().get("ch1/outline"),
        Some(&StepStatus::Failed)
    );
    let dollars = ChapterConfig {
        budget: Budget {
            tokens: None,
            usd: Some(0.01),
        },
        ..config()
    };
    let outcome = open.advance(&port, &dollars).await;
    assert!(
        matches!(&outcome, Outcome::Stopped { why: Stop::Budget(spent), .. } if spent == "$0.0200 of $0.01"),
        "{outcome:?}"
    );
    let roomy = ChapterConfig {
        budget: Budget {
            tokens: Some(1_000_000),
            usd: Some(5.0),
        },
        ..config()
    };
    let outcome = open.advance(&port, &roomy).await;
    assert!(matches!(paused_at(&outcome), QuestionBody::Chapter { .. }));
}

/// A plan of scene 1 with no one present; `again` decides whether the
/// second ask is refused too.
fn empty_plan(again: bool) -> Arc<Answer> {
    Arc::new(
        move |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
            let empty = address == "ch1/sc1/plan" || (again && address == "ch1/sc1/plan/again");
            if empty {
                let mut plan = scene_plan(1);
                plan["next"]["present"] = json!([]);
                return Ok(Reply::Json(plan));
            }
            story(address, prompt)
        },
    )
}

#[tokio::test]
async fn a_plan_the_world_refuses_is_asked_for_once_more_with_the_reason() {
    let mut open = Open::approved().await;
    let port = Script::new(empty_plan(false));
    let outcome = open.advance(&port, &config()).await;
    assert!(matches!(paused_at(&outcome), QuestionBody::Chapter { .. }));
    let again = port.prompt_at("ch1/sc1/plan/again");
    assert!(
        again
            .user
            .contains("The world refuses that plan: no one alive is present.")
    );
    let first = open.db.scenes_of(1).await.unwrap();
    assert_eq!(first[0].setup.present.len(), 2, "the second plan is stored");

    let mut twice = Open::approved().await;
    let port = Script::new(empty_plan(true));
    let outcome = twice.advance(&port, &config()).await;
    assert!(
        matches!(&outcome, Outcome::Stopped { step, why: Stop::Failed(why) }
            if step == "ch1/sc1/plan" && why.contains("refused twice")),
        "{outcome:?}"
    );
    assert!(twice.db.scenes_of(1).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_branch_a_cancellation_stopped_is_replaced_on_the_resume() {
    let mut open = Open::approved().await;
    let cancelled = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        if address.starts_with("ch1/sc1/r0/k0/round1/") {
            return Err(PortError::Cancelled {
                address: address.to_owned(),
            });
        }
        story(address, prompt)
    };
    let outcome = open
        .advance(&Script::new(Arc::new(cancelled)), &short())
        .await;
    assert_eq!(
        outcome,
        Outcome::Stopped {
            step: "ch1/sc1/r0".to_owned(),
            why: Stop::Cancelled,
        }
    );
    let steps = open.db.all_steps().await.unwrap();
    assert_eq!(steps.get("ch1/sc1/r0/k0"), Some(&StepStatus::Failed));
    assert_eq!(
        steps.get("ch1/sc1/r0/k0/interrupted"),
        Some(&StepStatus::Done)
    );
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &short()).await;
    assert!(matches!(paused_at(&outcome), QuestionBody::Chapter { .. }));
    let pick = open
        .db
        .pick_of(&SceneId::of(1, 1, 0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pick.branch.unwrap().as_str(), "ch1/sc1/r0/k1");
    let shown = port.prompt_at("ch1/sc1/r0/pick");
    assert!(
        shown.user.contains("Version k2:"),
        "k0's place is taken by k2"
    );
}

#[tokio::test]
async fn a_failed_author_call_stops_the_run_and_the_next_run_asks_it_again() {
    let mut open = Open::approved().await;
    let refusing = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        if address == "ch1/v1/write" {
            return Err(PortError::Refused {
                address: address.to_owned(),
            });
        }
        story(address, prompt)
    };
    let port = Script::new(Arc::new(refusing));
    let outcome = open.advance(&port, &config()).await;
    assert!(
        matches!(&outcome, Outcome::Stopped { step, why: Stop::Failed(_) } if step == "ch1/v1/write"),
        "{outcome:?}"
    );
    let calls = port.addresses().len();
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert!(matches!(paused_at(&outcome), QuestionBody::Chapter { .. }));
    let again = port.addresses();
    assert_eq!(
        again[0], "ch1/v1/write",
        "the run starts at the step that failed"
    );
    assert_eq!(
        again.len(),
        9,
        "the write, the reviews and the rest: {again:?}"
    );
    assert!(calls > again.len());
    let cancelled = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        if address.ends_with("/summary") {
            return Err(PortError::Cancelled {
                address: address.to_owned(),
            });
        }
        story(address, prompt)
    };
    let mut fresh = Open::approved().await;
    let outcome = fresh
        .advance(&Script::new(Arc::new(cancelled)), &config())
        .await;
    assert_eq!(
        outcome,
        Outcome::Stopped {
            step: "ch1/v2/summary".to_owned(),
            why: Stop::Cancelled,
        }
    );
}

/// A run killed at its n-th call, for every n, resumes as a new run to the
/// same checkpoint (docs/fabula.md §4, PR 6's proof, offline). A round a
/// branch keeps the cuts few; they run sixteen at a time, each a story of its
/// own whose writes wait on the disk.
#[tokio::test]
async fn a_run_killed_at_any_call_resumes_to_the_same_checkpoint() {
    let answer: Arc<Answer> = Arc::new(story);
    let (total, branch) = {
        let mut open = Open::approved().await;
        let port = Script::new(answer.clone());
        open.advance(&port, &short()).await;
        let calls = port.addresses();
        let branch = calls
            .iter()
            .filter(|a| a.starts_with("ch1/sc1/r0/k0/"))
            .count();
        (calls.len(), branch)
    };
    assert!(total > 20, "{total} calls");
    futures_util::stream::iter(1..=total)
        .map(|cut| killed_and_resumed(answer.clone(), cut, total, branch))
        .buffer_unordered(16)
        .collect::<Vec<()>>()
        .await;
}

async fn killed_and_resumed(answer: Arc<Answer>, cut: usize, total: usize, branch: usize) {
    let mut open = Open::approved().await;
    let blown = Arc::new(Notify::new());
    let fused = Script::fused(answer.clone(), cut, blown.clone());
    let cfg = short();
    tokio::select! {
        outcome = open.advance(&fused, &cfg) => panic!("cut {cut} ran through: {outcome:?}"),
        () = blown.notified() => {}
    }
    let port = Script::new(answer);
    let outcome = open.advance(&port, &cfg).await;
    assert_eq!(
        paused_at(&outcome),
        &QuestionBody::Chapter {
            chapter: 1,
            version: 2
        },
        "cut at call {cut}"
    );
    let db = &open.db;
    let questions = db.questions().await.unwrap();
    assert_eq!(questions.len(), 2, "cut {cut}: P1, then P2 once");
    let steps = db.all_steps().await.unwrap();
    for scene in db.scenes_of(1).await.unwrap() {
        assert!(scene.rerun == 0, "cut {cut}: no rerun was needed");
        let kept = db
            .pick_of(&scene.id)
            .await
            .unwrap()
            .unwrap()
            .branch
            .unwrap();
        assert_eq!(
            steps.get(kept.as_str()),
            Some(&StepStatus::Done),
            "cut {cut}: the chapter keeps only a branch played to its end"
        );
    }
    let versions = db.chapter_versions(1).await.unwrap();
    assert_eq!(versions.len(), 2, "cut {cut}");
    for v in &versions {
        let notes = db.notes(&v.id).await.unwrap();
        assert_eq!(notes.len(), 4, "cut {cut}: {} reviewed once", v.id);
    }
    assert!(db.summary(&versions[1].id).await.unwrap().is_some());
    let failed: Vec<&String> = steps
        .iter()
        .filter(|(_, s)| **s == StepStatus::Failed)
        .map(|(a, _)| a)
        .collect();
    assert!(failed.len() <= 1, "cut {cut}: {failed:?}");
    assert!(
        failed.iter().all(|a| a.contains("/r0/k")),
        "cut {cut}: only a branch in flight is lost: {failed:?}"
    );
    // A resume pays for the call in flight, its partner when the two reviews
    // ran side by side, and a lost branch played again under the next index;
    // nothing else a killed run finished.
    let resumed = port.addresses().len();
    assert!(
        resumed <= total - cut + 2 + branch,
        "cut {cut}: {resumed} of {total}"
    );
}

#[tokio::test]
async fn branches_lost_to_kills_are_replaced_and_cost_the_scene_no_attempt() {
    let answer: Arc<Answer> = Arc::new(story);
    let mut open = Open::approved().await;
    let cfg = short();
    for k in 0..3 {
        let blown = Arc::new(Notify::new());
        let prefix = format!("ch1/sc1/r0/k{k}/");
        let fused = Script::fused_under(answer.clone(), &prefix, blown.clone());
        tokio::select! {
            outcome = open.advance(&fused, &cfg) => panic!("k{k} ran through: {outcome:?}"),
            () = blown.notified() => {}
        }
    }
    let port = Script::new(answer);
    let outcome = open.advance(&port, &cfg).await;
    assert!(matches!(paused_at(&outcome), QuestionBody::Chapter { .. }));
    let db = &open.db;
    let first: Vec<StoredScene> = db
        .scenes_of(1)
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.scene == 1)
        .collect();
    assert_eq!(first.len(), 1, "three kills, and no rerun");
    let pick = db.pick_of(&first[0].id).await.unwrap().unwrap();
    assert_eq!(pick.branch.unwrap().as_str(), "ch1/sc1/r0/k3");
    let steps = db.all_steps().await.unwrap();
    for k in 0..3 {
        let branch = format!("ch1/sc1/r0/k{k}");
        assert_eq!(steps.get(&branch), Some(&StepStatus::Failed), "{branch}");
        assert_eq!(
            steps.get(&format!("{branch}/interrupted")),
            Some(&StepStatus::Done),
            "{branch} is marked lost, not failed"
        );
    }
    for k in 3..5 {
        assert_eq!(
            steps.get(&format!("ch1/sc1/r0/k{k}")),
            Some(&StepStatus::Done)
        );
    }
    let pick = port.prompt_at("ch1/sc1/r0/pick");
    assert!(pick.user.contains("Version k3:") && pick.user.contains("Version k4:"));
    assert!(
        !pick.user.contains("Version k0:"),
        "a lost branch is not shown"
    );
}

#[test]
fn the_engines_marker_check_writes_a_high_note() {
    let canon = fixture_canon();
    let notes = markers_in(
        &canon,
        "She took it. [secret-mara-7q] And Silas's [SECRET-SILAS-3K].",
    );
    assert_eq!(notes.len(), 2);
    assert!(
        notes
            .iter()
            .all(|n| n.kind == NoteKind::Token && n.severity == NoteSeverity::High)
    );
    assert_eq!(notes[0].anchor.as_deref(), Some("SECRET-MARA-7Q"));
    assert!(markers_in(&canon, "She took a bribe.").is_empty());
}

#[test]
fn a_triage_left_short_defers_and_a_second_word_is_ignored() {
    let note = |id: &str| ReviewNote {
        id: crate::entities::story::NoteId::new(id),
        chapter: ChapterId::of(1, 1),
        reviewer: Reviewer::Reader,
        note: NewNote {
            kind: NoteKind::Confusion,
            severity: NoteSeverity::High,
            anchor: None,
            body: "b".into(),
            evidence: Vec::new(),
        },
    };
    let (a, b) = (note("ch1/v1/reader/n1"), note("ch1/v1/reader/n2"));
    let shown = vec![("n1".to_owned(), &a), ("n2".to_owned(), &b)];
    let reply: TriageReply = serde_json::from_value(json!({"decisions": [
        {"note": "n1", "critique": "Yes.", "decision": "accepted"},
        {"note": "n1", "critique": "No.", "decision": "rejected"},
    ]}))
    .unwrap();
    let decided = decisions_of(&shown, &reply);
    assert_eq!(decided[0].decision, NoteDecision::Accepted);
    assert_eq!(decided[0].reason, "Yes.");
    assert_eq!(decided[1].decision, NoteDecision::Deferred);
    assert_eq!(decided[1].note, b.id);
}

#[test]
fn a_planned_scene_is_held_to_the_living() {
    let canon = fixture_canon();
    let world = WorldState::initial(&canon);
    let draft = |present: &[&str]| SceneDraft {
        intent: " i ".into(),
        location: "inn".into(),
        present: present.iter().map(|&c| c.into()).collect(),
        just_happened: "j".into(),
        pressure: "p".into(),
    };
    let held = held_to_the_world(&world, &draft(&["ines", "mara", "ines"])).unwrap();
    assert_eq!(
        held.present,
        [CharacterKey::from("ines"), CharacterKey::from("mara")]
    );
    assert_eq!(held.intent, "i");
    assert!(held_to_the_world(&world, &draft(&[])).is_err());
}

#[test]
fn the_readers_observations_ride_beside_its_notes() {
    let reply: ReaderReply = serde_json::from_value(json!({
        "confusions": [{"anchor": " ", "note": "Lost.", "severity": "high"}],
        "slow_passages": [{"anchor": "the stairs", "note": "Long.", "severity": "low"}],
        "predictions": ["", "Ines finds the ledger."],
        "best_moment": " ",
    }))
    .unwrap();
    let notes = reader_notes(reply);
    assert_eq!(
        notes.iter().map(|n| n.kind).collect::<Vec<_>>(),
        [
            NoteKind::Confusion,
            NoteKind::SlowPassage,
            NoteKind::Prediction
        ]
    );
    assert_eq!(notes[0].anchor, None, "an empty anchor is none");
}
