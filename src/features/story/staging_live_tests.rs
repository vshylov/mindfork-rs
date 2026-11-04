//! PR 12's known-answer probe on Gate G2's own story (docs/fabula.md §4,
//! PR 12; §6, T18): what the staging pass finds in the chapters G2's
//! readers read, where they said what is wrong.
//!
//! - **The referent**: chapter 2 has the shopkeeper say the father was "son
//!   of Marya, who left with him for the mainland", where the son says his
//!   mother took *him*; no reviewer named it. The pass is to.
//! - **The lines**: chapter 2's replies open on thanks again and again, and
//!   the inspector states his decision aloud at the grave. The pass is to
//!   mark a reply that opens on thanks and a line that announces.
//!
//! Chapter 1 is read too, for what the pass says of a chapter the readers
//! found sound: its findings are printed and counted, not scored. Chapter 2
//! is read after chapter 1's summary as G2 stored it, as a run gives it.
//!
//! Each probe runs `MINDFORK_FABULA_PROBE_N` times (3), the role sampled as
//! the application samples it ([`super::run::role_sampling`]). `#[ignore]`;
//! the route is G1's, and `MINDFORK_FABULA_G2_STORY` names G2's story
//! database, which is copied and never written.
//!
//! `cargo test staging_live -- --ignored --nocapture --test-threads=1`

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::engine::EngineAdapter;
use super::keeper::accepted_version;
use super::port::{CallAddress, Role, decide};
use super::run::role_sampling;
use super::schema::{CanonKeys, StorySchema};
use super::staging::{StagingKind, StagingReply, staging_prompt};
use crate::features::story::chapter_live_tests::route;
use crate::shared::storage::story::StoryDb;

/// Whether `text`, lowercased, holds one of `any`.
fn names(text: &str, any: &[&str]) -> bool {
    let text = text.to_lowercase();
    any.iter().any(|w| text.contains(w))
}

/// What a reading of chapter 2 found of the three known faults: Marya's
/// line as a referent or a kin, a reply opening on thanks, a line that
/// announces.
fn scored(reply: &StagingReply) -> [bool; 3] {
    let marya = reply.findings.iter().any(|f| {
        matches!(f.kind, StagingKind::Referent | StagingKind::Staging)
            && names(
                &format!("{} {}", f.anchor, f.note),
                &["марь", "marya", "maria"],
            )
    });
    let thanks = reply
        .findings
        .iter()
        .any(|f| f.kind == StagingKind::Thanks && names(&f.anchor, &["спасиб", "благодар"]));
    let announced = reply
        .findings
        .iter()
        .any(|f| f.kind == StagingKind::Announced);
    [marya, thanks, announced]
}

#[tokio::test]
#[ignore = "needs a live model and MINDFORK_FABULA_G2_STORY: Gate G2's story database"]
async fn staging_live_known_answers_on_g2() {
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
    assert!(
        ch2.text.contains("Сын Марьи"),
        "G2's chapter 2 has the line"
    );
    let before = [(1, db.summary(&ch1.id).await.unwrap().expect("G2's summary"))];
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({"probe": "staging"})).await.unwrap();
    let sampling = role_sampling()
        .into_iter()
        .find(|(r, _)| *r == Role::Continuity)
        .map(|(_, s)| s)
        .expect("a sampled role");
    let port = EngineAdapter::sharing(
        backend,
        Arc::new(tokio::sync::Mutex::new(log)),
        &model,
        sampling,
        CancellationToken::new(),
    );
    let schema = StagingReply::schema(&keys);
    let mut hits = [0usize; 3];
    let mut runs = 0usize;
    let mut found = [0usize; 2];
    for i in 0..n {
        for (chapter, summaries, text) in [(1u32, &[][..], &ch1.text), (2, &before[..], &ch2.text)]
        {
            let reply: Result<StagingReply, _> = decide(
                &port,
                &CallAddress {
                    run: run.clone(),
                    address: format!("probe/{i}/ch{chapter}/staging"),
                },
                Role::Continuity,
                &schema,
                &staging_prompt(summaries, text),
            )
            .await;
            let reply = match reply {
                Ok(reply) => reply,
                Err(e) => {
                    eprintln!("#{i} ch{chapter} staging failed: {e}");
                    continue;
                }
            };
            for f in &reply.findings {
                eprintln!(
                    "#{i} ch{chapter} {:?}, {:?}: «{}» {}",
                    f.kind, f.severity, f.anchor, f.note
                );
            }
            found[chapter as usize - 1] += reply.findings.len();
            if chapter == 2 {
                runs += 1;
                for (hit, got) in hits.iter_mut().zip(scored(&reply)) {
                    *hit += usize::from(got);
                }
            }
        }
    }
    let spent = db.spend("probe/").await.unwrap();
    eprintln!("\n{model}: PR 12's known answers on G2, {n} run(s)");
    eprintln!("| Probe | Hit | Runs |");
    eprintln!("| --- | --- | --- |");
    for (name, hit) in [
        "ch2: Marya's line (referent)",
        "ch2: a reply that opens on thanks",
        "ch2: a line that announces",
    ]
    .iter()
    .zip(hits)
    {
        eprintln!("| {name} | {hit} | {runs} |");
    }
    eprintln!(
        "findings in all: ch1 {}, ch2 {}; {} calls, {} tokens{}",
        found[0],
        found[1],
        spent.calls,
        spent.tokens(),
        spent
            .cost_usd
            .map_or(String::new(), |c| format!(", ${c:.4}"))
    );
}

#[test]
fn the_probe_reads_russian_by_its_stems() {
    let reply: StagingReply = serde_json::from_value(json!({"findings": [
        {"anchor": "Сын Марьи, что с ним", "note": "Чей сын?", "kind": "referent", "severity": "high"},
        {"anchor": "— Спасибо, что привели.", "note": "Opens on thanks.", "kind": "thanks", "severity": "low"},
        {"anchor": "закрытия я не подпишу", "note": "States it.", "kind": "announced", "severity": "low"},
    ]}))
    .unwrap();
    assert_eq!(scored(&reply), [true, true, true]);
    let none: StagingReply = serde_json::from_value(json!({"findings": [
        {"anchor": "Спасибо", "note": "Марья?", "kind": "announced", "severity": "low"},
    ]}))
    .unwrap();
    assert_eq!(scored(&none), [false, false, true], "kinds are held apart");
}
