//! PR 11's known-answer probes on Gate G2's own story (docs/fabula.md §4,
//! PR 11; §6, T18): what the Canon keeper and the Continuity checker find in
//! the chapters G2's readers read, where the readers said what is wrong.
//!
//! - **The old summary checked**: chapter 1's summary, written by the Author
//!   with the canon before it, has the shopkeeper hide something in the
//!   chapel; the text shows her carrying something out. The check is to name
//!   that sentence.
//! - **A summary from the text alone**, made as the engine makes it now
//!   (written, checked, written again against the check), is to put nothing
//!   into the chapel.
//! - **Chapter 1's register** is to hold the logbook's line "in another hand"
//!   and the footsteps on the path among its threads, and the father lost "a
//!   few years ago" among its facts.
//! - **Chapter 2 checked against it**, as the arc's last chapter, is to find
//!   the line gone "empty", the twenty-eight years against "a few years ago",
//!   and the footsteps left open.
//!
//! Each probe runs `MINDFORK_FABULA_PROBE_N` times (3), every role sampled
//! as the application samples it ([`super::run::role_sampling`]).
//! `#[ignore]`; the route is G1's, and `MINDFORK_FABULA_G2_STORY` names G2's
//! story database, which is copied and never written.
//!
//! `cargo test register_live -- --ignored --nocapture --test-threads=1`

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::author::{ContinuityReply, FindingKind};
use super::author_prompts::{self, Kept, Trace};
use super::engine::EngineAdapter;
use super::keeper::{
    RegisterReply, SoFar, SummaryCheck, accepted_version, register_of, register_prompt,
    summary_check_prompt, summary_prompt,
};
use super::port::{CallAddress, LlmPort, Role, decide};
use super::run::role_sampling;
use super::schema::{CanonKeys, StorySchema};
use crate::entities::story::{Register, RunId};
use crate::features::story::chapter_live_tests::route;
use crate::shared::api::contract::EngineBackend;
use crate::shared::storage::story::{RunLogWriter, StoredScene, StoryDb};

/// Whether `text`, lowercased, holds every one of `all` and one of `any`.
fn holds(text: &str, all: &[&str], any: &[&str]) -> bool {
    let text = text.to_lowercase();
    all.iter().all(|w| text.contains(w)) && (any.is_empty() || any.iter().any(|w| text.contains(w)))
}

/// A sentence that puts something hidden into the chapel.
fn hides_in_the_chapel(summary: &str) -> bool {
    summary
        .split(['.', '!', '?', ';'])
        .any(|s| holds(s, &["часовн"], &["пряч", "спрят"]))
}

/// The ports a probe asks through: one per role, sampled as the app does.
struct Ports {
    backend: Arc<dyn EngineBackend>,
    model: String,
    log: Arc<tokio::sync::Mutex<RunLogWriter>>,
    run: RunId,
}

impl Ports {
    fn of(&self, role: Role) -> EngineAdapter {
        let sampling = role_sampling()
            .into_iter()
            .find(|(r, _)| *r == role)
            .map(|(_, s)| s)
            .expect("a sampled role");
        EngineAdapter::sharing(
            self.backend.clone(),
            self.log.clone(),
            &self.model,
            sampling,
            CancellationToken::new(),
        )
    }

    fn call(&self, address: String) -> CallAddress {
        CallAddress {
            run: self.run.clone(),
            address,
        }
    }
}

/// A chapter's kept scenes with their traces, as the checker reads them.
async fn kept_of(db: &StoryDb, chapter: u32) -> Vec<(StoredScene, Trace)> {
    let canon = db.canon().await.unwrap().unwrap();
    let mut kept = Vec::new();
    for line in db.kept_scenes(chapter).await.unwrap() {
        let branch = line.branch;
        let mut events = db.path(&branch).await.unwrap();
        events.retain(|e| e.branch == branch);
        let mut reflections = Vec::new();
        for c in &canon.characters {
            for r in db.reflections(&branch, &c.key).await.unwrap() {
                if r.branch == branch {
                    reflections.push((c.key.clone(), r));
                }
            }
        }
        kept.push((
            line.scene,
            Trace {
                branch,
                events,
                reflections,
            },
        ));
    }
    kept
}

#[tokio::test]
#[ignore = "needs a live model and MINDFORK_FABULA_G2_STORY: Gate G2's story database"]
async fn register_live_known_answers_on_g2() {
    let Some((backend, model)) = route() else {
        eprintln!("skip: no live route");
        return;
    };
    let Ok(from) = std::env::var("MINDFORK_FABULA_G2_STORY") else {
        eprintln!("skip: MINDFORK_FABULA_G2_STORY names no story");
        return;
    };
    let n: usize = std::env::var("MINDFORK_FABULA_PROBE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::copy(PathBuf::from(&from), dir.path().join("story.db"))
        .await
        .unwrap();
    let db = StoryDb::open(
        &dir.path().join("story.db"),
        &dir.path().join("story.calls.db"),
    )
    .await
    .unwrap();
    let canon = db.canon().await.unwrap().unwrap();
    let keys = CanonKeys::of(&canon);
    let ch1 = accepted_version(&db, 1).await.unwrap().expect("chapter 1");
    let ch2 = accepted_version(&db, 2).await.unwrap().expect("chapter 2");
    let old_summary = db.summary(&ch1.id).await.unwrap().expect("G2's summary");
    assert!(
        hides_in_the_chapel(&old_summary),
        "G2's own summary has the fault: {old_summary}"
    );
    let kept2 = kept_of(&db, 2).await;
    let parent = db
        .branch(&kept2[0].1.branch)
        .await
        .unwrap()
        .unwrap()
        .parent
        .unwrap();
    let opened2 = db.world(&parent).await.unwrap();
    let view2: Vec<Kept> = kept2
        .iter()
        .map(|(s, t)| Kept {
            scene: s.scene,
            setup: &s.setup,
            intent: &s.intent,
            trace: t,
        })
        .collect();
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({"probe": "register"})).await.unwrap();
    let ports = Ports {
        backend,
        model: model.clone(),
        log: Arc::new(tokio::sync::Mutex::new(log)),
        run,
    };
    let keeper = ports.of(Role::CanonKeeper);
    let checker = ports.of(Role::Continuity);
    // name → hits, runs
    let mut tally: Vec<(&str, usize, usize)> = [
        "old summary's chapel named",
        "new summary keeps out of the chapel",
        "ch1 thread: the other hand",
        "ch1 thread: the footsteps",
        "ch1 fact: a few years ago",
        "ch2 finding: the line gone empty",
        "ch2 finding: the years",
        "ch2 open thread: the footsteps",
        "ch2 pays off the footsteps",
    ]
    .into_iter()
    .map(|k| (k, 0, 0))
    .collect();
    let mut count = |name: &str, hit: bool| {
        let row = tally.iter_mut().find(|(k, _, _)| *k == name).unwrap();
        row.2 += 1;
        if hit {
            row.1 += 1;
        }
    };
    for i in 0..n {
        // The old summary, checked.
        let check: Result<SummaryCheck, _> = decide(
            &keeper,
            &ports.call(format!("probe/{i}/old-summary-check")),
            Role::CanonKeeper,
            &SummaryCheck::schema(&keys),
            &summary_check_prompt(&ch1.text, &old_summary),
        )
        .await;
        match check {
            Ok(check) => {
                for f in &check.faults {
                    eprintln!("#{i} old summary fault: {} — {}", f.sentence, f.text_says);
                }
                count(
                    "old summary's chapel named",
                    check
                        .faults
                        .iter()
                        .any(|f| hides_in_the_chapel(&f.sentence)),
                );
            }
            Err(e) => eprintln!("#{i} old summary check failed: {e}"),
        }
        // A summary from the text alone, as the engine makes it.
        let first = keeper
            .text(
                &ports.call(format!("probe/{i}/summary")),
                Role::CanonKeeper,
                &summary_prompt("Russian", &ch1.text, None),
            )
            .await;
        if let Ok(first) = first {
            let check: Result<SummaryCheck, _> = decide(
                &keeper,
                &ports.call(format!("probe/{i}/summary-check")),
                Role::CanonKeeper,
                &SummaryCheck::schema(&keys),
                &summary_check_prompt(&ch1.text, &first),
            )
            .await;
            let faults = check.map(|c| c.faults).unwrap_or_default();
            let summary = if faults.is_empty() {
                first.clone()
            } else {
                keeper
                    .text(
                        &ports.call(format!("probe/{i}/summary-again")),
                        Role::CanonKeeper,
                        &summary_prompt("Russian", &ch1.text, Some((&first, &faults))),
                    )
                    .await
                    .unwrap_or(first.clone())
            };
            eprintln!(
                "#{i} summary ({} fault(s) in the first):\n{summary}",
                faults.len()
            );
            count(
                "new summary keeps out of the chapel",
                !hides_in_the_chapel(&summary),
            );
        }
        // Chapter 1's register.
        let reply: Result<RegisterReply, _> = decide(
            &keeper,
            &ports.call(format!("probe/{i}/ch1-register")),
            Role::CanonKeeper,
            &RegisterReply::schema_for(&keys, &[]),
            &register_prompt(&SoFar::default(), 1, &ch1.text, "Russian"),
        )
        .await;
        let Ok(reply) = reply else {
            eprintln!("#{i} ch1 register failed");
            continue;
        };
        let first: Register = register_of(1, reply, &SoFar::default());
        for t in &first.opened {
            eprintln!("#{i} ch1 {}: {} («{}»)", t.id, t.question, t.quote);
        }
        for f in &first.facts {
            eprintln!(
                "#{i} ch1 {} ({}): {} («{}»)",
                f.id, f.stated_by, f.fact, f.quote
            );
        }
        let thread_on = |words: &[&str]| {
            first
                .opened
                .iter()
                .find(|t| holds(&format!("{} {}", t.question, t.quote), &[], words))
                .map(|t| t.id.clone())
        };
        let footsteps = thread_on(&["шаги"]);
        count(
            "ch1 thread: the other hand",
            thread_on(&["почерк"]).is_some(),
        );
        count("ch1 thread: the footsteps", footsteps.is_some());
        count(
            "ch1 fact: a few years ago",
            first
                .facts
                .iter()
                .any(|f| holds(&format!("{} {}", f.fact, f.quote), &["несколько лет"], &[])),
        );
        let mut so_far = SoFar::default();
        so_far.add(1, &first);
        // Chapter 2, the arc's last, checked against it.
        let prompt =
            author_prompts::continuity(&canon, &opened2, &view2, &ch2.text, (&so_far, true));
        let reply: Result<ContinuityReply, _> = decide(
            &checker,
            &ports.call(format!("probe/{i}/ch2-continuity")),
            Role::Continuity,
            &ContinuityReply::schema_for(&keys, !so_far.open.is_empty()),
            &prompt,
        )
        .await;
        match reply {
            Ok(reply) => {
                for f in &reply.findings {
                    eprintln!(
                        "#{i} ch2 {:?}, {:?}: «{}» {}",
                        f.kind, f.severity, f.anchor, f.note
                    );
                }
                let about = |words: &[&str]| {
                    reply
                        .findings
                        .iter()
                        .any(|f| holds(&format!("{} {}", f.anchor, f.note), &[], words))
                };
                count("ch2 finding: the line gone empty", about(&["пуст"]));
                // A finding that names the years and then clears them is none.
                count(
                    "ch2 finding: the years",
                    reply.findings.iter().any(|f| {
                        holds(&f.note, &["несколько лет"], &[])
                            && !holds(&f.note, &[], &["нарушения нет", "не противоречит"])
                    }),
                );
                count(
                    "ch2 open thread: the footsteps",
                    footsteps.as_ref().is_some_and(|id| {
                        reply.findings.iter().any(|f| {
                            f.kind == FindingKind::OpenThread
                                && f.anchor.trim().trim_matches('`') == id
                        })
                    }),
                );
            }
            Err(e) => eprintln!("#{i} ch2 continuity failed: {e}"),
        }
        // Chapter 2's register against chapter 1's.
        let reply: Result<RegisterReply, _> = decide(
            &keeper,
            &ports.call(format!("probe/{i}/ch2-register")),
            Role::CanonKeeper,
            &RegisterReply::schema_for(&keys, &so_far.open_ids()),
            &register_prompt(&so_far, 2, &ch2.text, "Russian"),
        )
        .await;
        if let Ok(reply) = reply {
            let second = register_of(2, reply, &so_far);
            for p in &second.paid {
                eprintln!("#{i} ch2 pays {}: {}", p.thread, p.how);
            }
            count(
                "ch2 pays off the footsteps",
                footsteps
                    .as_ref()
                    .is_some_and(|id| second.paid.iter().any(|p| &p.thread == id)),
            );
        }
    }
    let spent = db.spend("probe/").await.unwrap();
    eprintln!("\n{model}: PR 11's known answers on G2, {n} run(s) a probe");
    eprintln!("| Probe | Hit | Runs |");
    eprintln!("| --- | --- | --- |");
    for (name, hit, runs) in &tally {
        eprintln!("| {name} | {hit} | {runs} |");
    }
    eprintln!(
        "{} calls, {} tokens{}",
        spent.calls,
        spent.tokens(),
        spent
            .cost_usd
            .map_or(String::new(), |c| format!(", ${c:.4}"))
    );
}

#[test]
fn the_probes_read_russian_by_its_stems() {
    assert!(hides_in_the_chapel(
        "Она просит его, а сама прячет что-то из корзинки в часовне. Ночью он идёт."
    ));
    assert!(!hides_in_the_chapel(
        "Галина что-то уносит из часовни. Позже она признаётся, что спрятала тетрадь."
    ));
    assert!(holds("Почерк ЧУЖОЙ", &["почерк"], &[]));
    assert!(!holds("шаги", &["почерк"], &["шаг"]));
}
