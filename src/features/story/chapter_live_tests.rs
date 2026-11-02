//! PR 6's proof as a harness (docs/fabula.md §4): one chapter of the fixture
//! story end to end through the application's own clients — the canon
//! approved, the outline, the scenes planned, simulated and picked, the draft,
//! the reviews, the triage and the revisions, the summary, the checkpoint —
//! and the same chapter killed at random moments, its database closed and
//! opened again, and resumed each time until it reaches the checkpoint.
//!
//! What it reports, beside the markdown: every role's calls, first attempts
//! that met their schema and tokens; the picks and reruns; the notes and the
//! decisions; the leaks the audit finds in the characters' calls; the tokens
//! a chapter costs, for the budget's default (PR 8).
//!
//! Roles' thinking: the characters muted, the World Master at a low effort
//! (as Gate G1), the Author at a medium one, the reviewers at a low one.
//!
//! `#[ignore]`, skipped without a route — the variables are G1's
//! (`super::simulation_live_tests`), `MINDFORK_FABULA_MECHANISM=fallback`
//! included — and:
//!
//! - `MINDFORK_FABULA_CH_K` (3), `_ROUNDS` (6), `_SCENES` (3): the
//!   chapter's shape;
//! - `MINDFORK_FABULA_CH_OUT`: where the story, its call log, the markdown
//!   and the report go (`target/fabula/chapter-<model>` by default);
//! - `MINDFORK_FABULA_CH_KILLS` (4) and `_KILL_SECS` (`20-120`): how many
//!   times the second harness kills its run, and when.
//!
//! `cargo test chapter_live -- --ignored --nocapture --test-threads=1`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::chapter::{ChapterConfig, Engine, Outcome, advance, resolve};
use super::engine::{EngineAdapter, SharedLog};
use super::export;
use super::leaks::{AuditedCall, audit};
use super::port::Role;
use super::simulation::SimConfig;
use super::simulation_live_tests::{character_sampling, fallback_asked, master_sampling};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::{
    BranchId, Decision, QuestionBody, Resolution, VersionOrigin, fixture_canon,
};
use crate::features::story::schema_live_tests::{Cloud, engine_route};
use crate::shared::api::contract::EngineBackend;
use crate::shared::storage::story::{AuthorWriter, StepStatus, StoryDb};

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// The route the variables name: G1's.
pub(super) fn route() -> Option<(Arc<dyn EngineBackend>, String)> {
    Cloud::ALL
        .into_iter()
        .find_map(Cloud::backend)
        .or_else(engine_route)
}

/// The Author's sampling: thinking on at a medium effort, room for a chapter.
fn author_sampling() -> SamplingConfig {
    SamplingConfig {
        temperature: Some(0.8),
        thinking: Some(true),
        reasoning_effort: Some(ReasoningEffort::Medium),
        max_tokens: Some(32_000),
        ..SamplingConfig::default()
    }
}

/// The reviewers': thinking on at a low effort.
fn reviewer_sampling() -> SamplingConfig {
    SamplingConfig {
        temperature: Some(0.4),
        thinking: Some(true),
        reasoning_effort: Some(ReasoningEffort::Low),
        max_tokens: Some(16_384),
        ..SamplingConfig::default()
    }
}

fn config() -> ChapterConfig {
    ChapterConfig {
        sim: SimConfig {
            branches: env_or("MINDFORK_FABULA_CH_K", 3),
            max_rounds: env_or("MINDFORK_FABULA_CH_ROUNDS", 6),
            ..SimConfig::default()
        },
        max_scenes: env_or("MINDFORK_FABULA_CH_SCENES", 3),
        ..ChapterConfig::default()
    }
}

fn out_dir(model: &str, arm: &str) -> PathBuf {
    let safe: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    std::env::var("MINDFORK_FABULA_CH_OUT")
        .map_or_else(
            |_| PathBuf::from("target/fabula").join(format!("chapter-{safe}")),
            PathBuf::from,
        )
        .join(arm)
}

/// An open story in `dir`, its Author's capability and its run log.
struct Opened {
    db: StoryDb,
    author: AuthorWriter,
    log: SharedLog,
}

async fn open_in(dir: &Path) -> Opened {
    let db = StoryDb::open(&dir.join("story.db"), &dir.join("story.calls.db"))
        .await
        .unwrap();
    let author = db.author_writer().await.unwrap();
    let log = Arc::new(tokio::sync::Mutex::new(db.run_log_writer().await.unwrap()));
    Opened { db, author, log }
}

/// A fresh story in `dir` with the fixture canon.
async fn fresh_in(dir: &Path) -> Opened {
    if tokio::fs::try_exists(dir).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
    tokio::fs::create_dir_all(dir).await.unwrap();
    let mut opened = open_in(dir).await;
    opened.author.import_canon(&fixture_canon()).await.unwrap();
    opened
}

impl Opened {
    fn port(&self, backend: Arc<dyn EngineBackend>, model: &str) -> EngineAdapter {
        let port = EngineAdapter::sharing(
            backend,
            self.log.clone(),
            model,
            character_sampling(),
            CancellationToken::new(),
        )
        .with_role_sampling(Role::WorldMaster, master_sampling())
        .with_role_sampling(Role::Author, author_sampling())
        .with_role_sampling(Role::Reader, reviewer_sampling())
        .with_role_sampling(Role::Continuity, reviewer_sampling());
        if fallback_asked() {
            port.fallback_only()
        } else {
            port
        }
    }

    async fn advance(&mut self, port: &EngineAdapter, config: &ChapterConfig) -> Outcome {
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

    /// Answers the pending question as a person reading it would, by its kind:
    /// the canon approved, a failed scene tried again; a chapter is where the
    /// harness stops.
    async fn answer(&mut self) -> bool {
        let Some(question) = self.db.pending_question().await.unwrap() else {
            return false;
        };
        let (decision, notes) = match question.body {
            QuestionBody::Canon => (Decision::Approve, ""),
            QuestionBody::Scene { .. } => (Decision::Answer, "Try the scene again."),
            QuestionBody::Chapter { .. } => return false,
        };
        let resolution = Resolution {
            decision,
            notes: notes.to_owned(),
            evidence: json!({"harness": true}),
        };
        resolve(&mut self.author, &question.id, &resolution)
            .await
            .unwrap();
        eprintln!(
            "  answered {} ({:?}): {}",
            question.id,
            question.body,
            decision.as_str()
        );
        true
    }

    async fn close(self) {
        let Opened { db, author, log } = self;
        drop(author);
        drop(log);
        db.close().await.unwrap();
    }
}

/// Runs the story until it reaches chapter 1's checkpoint, answering the
/// pauses before it; the outcomes in order.
async fn to_checkpoint(
    opened: &mut Opened,
    port: &EngineAdapter,
    config: &ChapterConfig,
) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    loop {
        let outcome = opened.advance(port, config).await;
        eprintln!("  outcome: {}", short(&outcome));
        let done = match &outcome {
            Outcome::Paused(q) => {
                !matches!(q.body, QuestionBody::Canon | QuestionBody::Scene { .. })
            }
            Outcome::Finished | Outcome::Stopped { .. } => true,
        };
        outcomes.push(outcome);
        if done || !opened.answer().await {
            return outcomes;
        }
    }
}

fn short(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Paused(q) => format!("paused at {} ({:?})", q.step, q.body),
        Outcome::Finished => "finished".to_owned(),
        Outcome::Stopped { step, why } => format!("stopped at {step}: {why:?}"),
    }
}

/// What the story's stores and call log hold once the chapter is at its
/// checkpoint: the report, written beside the markdown.
async fn report(db: &StoryDb, model: &str, out: &Path, seconds: u64, extra: Value) -> Value {
    let canon = fixture_canon();
    let config = config();
    let mut calls = Vec::new();
    let mut run = 1;
    loop {
        let id = crate::entities::story::RunId::nth(run);
        let these = db.calls(&id).await.unwrap();
        if these.is_empty() && db.steps(&id).await.unwrap().is_empty() {
            break;
        }
        calls.extend(these);
        run += 1;
    }
    let mut by_role: BTreeMap<String, Value> = BTreeMap::new();
    for call in &calls {
        let entry = by_role.entry(call.call.role.clone()).or_insert_with(|| {
            json!({"attempts": 0, "first": 0, "ok_first": 0, "in": 0, "out": 0, "reasoning": 0, "mechanisms": {}})
        });
        let response = call.call.response.clone().unwrap_or(Value::Null);
        entry["attempts"] = json!(entry["attempts"].as_u64().unwrap() + 1);
        entry["in"] =
            json!(entry["in"].as_u64().unwrap() + call.call.usage.input_tokens.unwrap_or(0));
        entry["out"] =
            json!(entry["out"].as_u64().unwrap() + call.call.usage.output_tokens.unwrap_or(0));
        entry["reasoning"] = json!(
            entry["reasoning"].as_u64().unwrap()
                + response["reasoning_tokens"].as_u64().unwrap_or(0)
        );
        let mechanism = call.call.mechanism.as_str();
        let seen = entry["mechanisms"][mechanism].as_u64().unwrap_or(0);
        entry["mechanisms"][mechanism] = json!(seen + 1);
        if call.call.attempt == 1 {
            entry["first"] = json!(entry["first"].as_u64().unwrap() + 1);
            if call.call.status.as_str() == "ok" {
                entry["ok_first"] = json!(entry["ok_first"].as_u64().unwrap() + 1);
            }
        }
    }
    // The leak audit over every branch of the chapter.
    let mut paths = BTreeMap::new();
    let scenes = db.scenes_of(1).await.unwrap();
    for scene in &scenes {
        for k in 0..config.sim.branches {
            let branch = BranchId::of_scene(&scene.id, k);
            if let Ok(path) = db.path(&branch).await {
                paths.insert(branch, path);
            }
        }
    }
    let audited: Vec<AuditedCall> = calls
        .iter()
        .map(|c| AuditedCall {
            address: &c.call.address,
            role: &c.call.role,
            request: &c.call.request,
            response: c.call.response.as_ref(),
        })
        .collect();
    let leaks = audit(&canon, &paths, &audited);
    let mut picks = Vec::new();
    for scene in &scenes {
        let pick = db.pick_of(&scene.id).await.unwrap();
        picks.push(json!({
            "scene": scene.id.as_str(),
            "location": scene.setup.location.as_str(),
            "present": scene.setup.present,
            "intent": scene.intent,
            "pick": pick.as_ref().map(|p| p.branch.as_ref().map(|b| b.as_str().to_owned())),
            "note": pick.map(|p| p.rationale),
        }));
    }
    let versions = db.chapter_versions(1).await.unwrap();
    let mut chapters = Vec::new();
    for v in &versions {
        let notes = db.notes(&v.id).await.unwrap();
        let decisions = db.decisions(&v.id).await.unwrap();
        chapters.push(json!({
            "version": v.version,
            "origin": v.origin.as_str(),
            "words": v.text.split_whitespace().count(),
            "notes": notes.iter().map(|n| json!({
                "id": n.id.as_str(),
                "kind": n.note.kind.as_str(),
                "severity": n.note.severity.as_str(),
                "anchor": n.note.anchor,
                "body": n.note.body,
                "evidence": n.note.evidence,
                "decision": decisions.iter().find(|d| d.note == n.id).map(|d| (d.decision.as_str(), d.reason.clone())),
            })).collect::<Vec<_>>(),
            "summary": db.summary(&v.id).await.unwrap(),
        }));
    }
    let assumptions = db.assumptions("ch1/").await.unwrap();
    let spend = db.spend("ch1/").await.unwrap();
    let steps = db.all_steps().await.unwrap();
    let failed: Vec<&String> = steps
        .iter()
        .filter(|(_, s)| **s == StepStatus::Failed)
        .map(|(a, _)| a)
        .collect();
    let report = json!({
        "model": model,
        "mechanism": if fallback_asked() { "fallback" } else { "as the backend allows" },
        "config": {
            "branches": config.sim.branches, "max_rounds": config.sim.max_rounds,
            "max_scenes": config.max_scenes,
        },
        "seconds": seconds,
        "calls": calls.len(),
        "by_role": by_role,
        "spend": {"tokens": spend.tokens(), "in": spend.input_tokens, "out": spend.output_tokens, "usd": spend.cost_usd},
        "leaks": leaks.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "scenes": picks,
        "versions": chapters,
        "assumptions": assumptions.iter().map(|a| json!({"step": a.step, "doubt": a.assumption.doubt, "choice": a.assumption.choice})).collect::<Vec<_>>(),
        "failed_steps": failed,
        "extra": extra,
    });
    let text = export::markdown(db).await.unwrap();
    tokio::fs::write(out.join("chapter.md"), &text)
        .await
        .unwrap();
    tokio::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .await
    .unwrap();
    eprintln!(
        "\n{model}: {} calls, {} s; spend {} tokens ({} in, {} out){}; {} leaks",
        calls.len(),
        seconds,
        spend.tokens(),
        spend.input_tokens,
        spend.output_tokens,
        spend
            .cost_usd
            .map_or(String::new(), |c| format!(", ${c:.4}")),
        leaks.len(),
    );
    for (role, e) in &by_role {
        eprintln!("  {role}: {e}");
    }
    for p in &report["scenes"].as_array().unwrap().clone() {
        eprintln!("  {}: {} → {}", p["scene"], p["location"], p["pick"]);
    }
    for c in &report["versions"].as_array().unwrap().clone() {
        eprintln!(
            "  v{} {} — {} words, {} notes",
            c["version"],
            c["origin"],
            c["words"],
            c["notes"].as_array().unwrap().len()
        );
    }
    eprintln!("  markdown and report in {}", out.display());
    report
}

#[tokio::test]
#[ignore = "needs a live model: a cloud key + MINDFORK_FABULA_{CLAUDE,OPENAI,GEMINI}_MODEL, or MINDFORK_ENGINE_URL (+ _KEY, _MODEL, _DIALECT)"]
async fn chapter_live_end_to_end() {
    let Some((backend, model)) = route() else {
        eprintln!("skip: no live route; see the module's head");
        return;
    };
    let out = out_dir(&model, "whole");
    let started = Instant::now();
    let mut opened = fresh_in(&out).await;
    let port = opened.port(backend, &model);
    let config = config();
    eprintln!(
        "{model}: one chapter, K = {}, up to {} rounds and {} scenes; story in {}",
        config.sim.branches,
        config.sim.max_rounds,
        config.max_scenes,
        out.display()
    );
    let outcomes = to_checkpoint(&mut opened, &port, &config).await;
    let report = report(
        &opened.db,
        &model,
        &out,
        started.elapsed().as_secs(),
        json!({"outcomes": outcomes.iter().map(short).collect::<Vec<_>>()}),
    )
    .await;
    let last = outcomes.last().unwrap();
    assert!(
        matches!(last, Outcome::Paused(q) if matches!(q.body, QuestionBody::Chapter { chapter: 1, .. })),
        "the chapter reached its checkpoint: {}",
        short(last)
    );
    assert!(report["leaks"].as_array().unwrap().is_empty(), "no leak");
    let versions = opened.db.chapter_versions(1).await.unwrap();
    assert_eq!(versions[0].origin, VersionOrigin::Draft);
    assert!(
        !export::markdown(&opened.db)
            .await
            .unwrap()
            .trim()
            .is_empty()
    );
}

/// A draw in `lo..=hi` from the clock: the crate has no random numbers, and
/// a kill's moment needs no more.
fn draw(lo: u64, hi: u64) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos() as u64;
    lo + nanos % (hi - lo + 1)
}

#[tokio::test]
#[ignore = "needs a live model: a cloud key + MINDFORK_FABULA_{CLAUDE,OPENAI,GEMINI}_MODEL, or MINDFORK_ENGINE_URL (+ _KEY, _MODEL, _DIALECT)"]
async fn chapter_live_killed_and_resumed() {
    let Some((backend, model)) = route() else {
        eprintln!("skip: no live route; see the module's head");
        return;
    };
    let kills: usize = env_or("MINDFORK_FABULA_CH_KILLS", 4);
    let (lo, hi) = std::env::var("MINDFORK_FABULA_CH_KILL_SECS")
        .ok()
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        })
        .unwrap_or((20, 120));
    let out = out_dir(&model, "killed");
    let started = Instant::now();
    fresh_in(&out).await.close().await;
    let config = config();
    let mut log = Vec::new();
    let mut killed = 0;
    let outcomes = loop {
        let mut opened = open_in(&out).await;
        let port = opened.port(backend.clone(), &model);
        if killed < kills {
            let after = Duration::from_secs(draw(lo, hi));
            let steps_before = opened.db.all_steps().await.unwrap().len();
            let run = tokio::time::timeout(after, to_checkpoint(&mut opened, &port, &config)).await;
            match run {
                Ok(outcomes) => {
                    drop(port);
                    opened.close().await;
                    break outcomes;
                }
                Err(_) => {
                    killed += 1;
                    let steps = opened.db.all_steps().await.unwrap();
                    let last = steps.keys().last().cloned().unwrap_or_default();
                    eprintln!(
                        "kill {killed} after {} s: {} steps recorded ({} new), the latest by name {last}",
                        after.as_secs(),
                        steps.len(),
                        steps.len() - steps_before,
                    );
                    log.push(json!({"after_secs": after.as_secs(), "steps": steps.len(), "new_steps": steps.len() - steps_before}));
                    drop(port);
                    opened.close().await;
                }
            }
        } else {
            let outcomes = to_checkpoint(&mut opened, &port, &config).await;
            drop(port);
            opened.close().await;
            break outcomes;
        }
    };
    let opened = open_in(&out).await;
    let report = report(
        &opened.db,
        &model,
        &out,
        started.elapsed().as_secs(),
        json!({"kills": log, "outcomes": outcomes.iter().map(short).collect::<Vec<_>>()}),
    )
    .await;
    eprintln!("killed {killed} times");
    let last = outcomes.last().unwrap();
    assert!(
        matches!(last, Outcome::Paused(q) if matches!(q.body, QuestionBody::Chapter { chapter: 1, .. })),
        "the chapter reached its checkpoint after {killed} kills: {}",
        short(last)
    );
    assert!(report["leaks"].as_array().unwrap().is_empty(), "no leak");
    let questions = opened.db.questions().await.unwrap();
    let checkpoints = questions
        .iter()
        .filter(|(q, _)| matches!(q.body, QuestionBody::Chapter { .. }))
        .count();
    assert_eq!(checkpoints, 1, "one checkpoint, asked once");
}
