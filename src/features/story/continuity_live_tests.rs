//! How much the Continuity checker finds, by mechanism and thinking effort
//! (docs/fabula.md §4, PR 6: what PR 5's finding — Gemini 3.x thinks about
//! a tenth as much under a schema — costs a role that judges).
//!
//! A chapter a live run wrote is taken with its record, and three faults the
//! record contradicts are written into its text, each a sentence of its own:
//!
//! - **a knowledge leak** — at the chapter's start, a character knows a secret
//!   they had not learned when it opened;
//! - **a cast error** — in the first scene, a character who is there in the
//!   record is put somewhere else;
//! - **a contradiction** — a third of the way in, a character falls dead who
//!   acts in the record afterwards.
//!
//! The checker is asked `MINDFORK_FABULA_PROBE_N` times (5) in each of four
//! arms — the schema sent or only in the prompt, at a low or a high effort —
//! the arms taking turns. A fault counts as found when a finding's anchor
//! shares three words running with its sentence, or its note names the fault's
//! own word (the place, "dead", the secret's key).
//!
//! `#[ignore]`; the route is G1's, and `MINDFORK_FABULA_PROBE_FROM` names a
//! chapter harness's story directory (`.../whole`), copied before use.
//!
//! `cargo test continuity_live -- --ignored --nocapture --test-threads=1`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::author::ContinuityReply;
use super::author_prompts::{self, Kept, Trace};
use super::engine::EngineAdapter;
use super::leaks::without_tokens;
use super::port::{CallAddress, Role, decide};
use super::schema::{CanonKeys, StorySchema};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::{BranchId, Canon, CharacterKey, WorldState};
use crate::features::story::chapter_live_tests::route;
use crate::shared::storage::story::{StoredScene, StoryDb};

/// One call's result: which faults it found, how many other findings, and
/// the reasoning tokens it was billed.
type Probed = (Vec<bool>, usize, u64);

/// A fault written into the text: its sentence, and the words a finding may
/// name it by.
struct Fault {
    name: &'static str,
    sentence: String,
    words: Vec<String>,
}

fn normalized(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether `anchor` shares three words running with `sentence`.
fn overlaps(anchor: &str, sentence: &str) -> bool {
    let (a, s) = (normalized(anchor), normalized(sentence));
    if a.len() < 3 {
        return !a.is_empty() && s.windows(a.len()).any(|w| w == a.as_slice());
    }
    a.windows(3).any(|w| s.windows(3).any(|v| v == w))
}

fn name_of(canon: &Canon, who: &CharacterKey) -> String {
    canon
        .character(who)
        .map_or_else(|| who.to_string(), |c| c.name.clone())
}

/// The three faults, built from the record and placed into `text`.
fn plant(
    canon: &Canon,
    opened: &WorldState,
    first: &StoredScene,
    kept: &[(StoredScene, Trace)],
    text: &str,
) -> (String, Vec<Fault>) {
    // A secret someone had not learned when the chapter opened.
    let (knower, holder, secret) = canon
        .characters
        .iter()
        .flat_map(|c| {
            canon.characters.iter().flat_map(move |o| {
                o.secrets
                    .iter()
                    .filter(move |_| o.key != c.key)
                    .map(move |s| (c, o, s))
            })
        })
        .find(|(c, _, s)| !opened.knows(&c.key, &s.key))
        .expect("someone does not know someone's secret");
    let leak = Fault {
        name: "knowledge_leak",
        sentence: format!(
            "From the first moment {} already knew what {} hid: {}",
            knower.name,
            holder.name,
            without_tokens(&secret.text)
        ),
        words: vec![secret.key.to_string(), "already knew".to_owned()],
    };
    // Someone present in the first scene, put elsewhere.
    let present = &first.setup.present[0];
    let away = canon
        .locations
        .iter()
        .find(|l| l.key != first.setup.location)
        .expect("a second place");
    let cast = Fault {
        name: "cast_error",
        sentence: format!(
            "All this while {} was not there at all, but alone in {}, far from the others.",
            name_of(canon, present),
            away.name
        ),
        words: vec![away.name.to_lowercase(), away.key.to_string()],
    };
    // Someone who acts in the record after the first third of it.
    let events: Vec<_> = kept.iter().flat_map(|(_, t)| &t.events).collect();
    let late = events
        .iter()
        .skip(events.len() / 3)
        .find_map(|e| e.body.actor.clone())
        .unwrap_or_else(|| present.clone());
    let death = Fault {
        name: "contradiction",
        sentence: format!(
            "Then {} slipped on the wet stones, struck their head, and lay dead.",
            name_of(canon, &late)
        ),
        words: vec!["dead".to_owned(), "died".to_owned()],
    };
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut out: Vec<String> = paragraphs.iter().map(|p| (*p).to_owned()).collect();
    let at = |share: usize| (out.len() * share / 6).clamp(1, out.len().saturating_sub(1).max(1));
    let (a, b, c) = (at(1), at(2), at(3));
    out.insert(c, death.sentence.clone());
    out.insert(b, cast.sentence.clone());
    out.insert(a, leak.sentence.clone());
    (out.join("\n\n"), vec![leak, cast, death])
}

#[tokio::test]
#[ignore = "needs a live model and MINDFORK_FABULA_PROBE_FROM: a chapter harness's story directory"]
async fn continuity_live_recall_by_mechanism_and_effort() {
    let Some((backend, model)) = route() else {
        eprintln!("skip: no live route");
        return;
    };
    let Ok(from) = std::env::var("MINDFORK_FABULA_PROBE_FROM") else {
        eprintln!("skip: MINDFORK_FABULA_PROBE_FROM names no story");
        return;
    };
    let n: usize = std::env::var("MINDFORK_FABULA_PROBE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let dir = tempfile::tempdir().unwrap();
    for file in ["story.db", "story.calls.db"] {
        std::fs::copy(PathBuf::from(&from).join(file), dir.path().join(file)).unwrap();
    }
    let db = StoryDb::open(
        &dir.path().join("story.db"),
        &dir.path().join("story.calls.db"),
    )
    .await
    .unwrap();
    let canon = db.canon().await.unwrap().unwrap();
    let mut kept = Vec::new();
    for scene in db.scenes_of(1).await.unwrap() {
        let Some(branch) = db.pick_of(&scene.id).await.unwrap().and_then(|p| p.branch) else {
            continue;
        };
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
            scene,
            Trace {
                branch,
                events,
                reflections,
            },
        ));
    }
    let first_branch: &BranchId = &kept[0].1.branch;
    let parent = db
        .branch(first_branch)
        .await
        .unwrap()
        .unwrap()
        .parent
        .unwrap();
    let opened = db.world(&parent).await.unwrap();
    let text = db
        .chapter_versions(1)
        .await
        .unwrap()
        .last()
        .unwrap()
        .text
        .clone();
    let (planted, faults) = plant(&canon, &opened, &kept[0].0, &kept, &text);
    for f in &faults {
        eprintln!("planted {}: {}", f.name, f.sentence);
    }
    let view: Vec<Kept> = kept
        .iter()
        .map(|(s, t)| Kept {
            scene: s.scene,
            setup: &s.setup,
            intent: &s.intent,
            trace: t,
        })
        .collect();
    let prompt = author_prompts::continuity(&canon, &opened, &view, &planted);
    let schema = ContinuityReply::schema(&CanonKeys::of(&canon));
    let mut log = db.run_log_writer().await.unwrap();
    let run = log
        .start_run(&json!({"probe": "continuity"}))
        .await
        .unwrap();
    let log = Arc::new(tokio::sync::Mutex::new(log));
    let arms = [
        ("schema, low", false, ReasoningEffort::Low),
        ("fallback, low", true, ReasoningEffort::Low),
        ("schema, high", false, ReasoningEffort::High),
        ("fallback, high", true, ReasoningEffort::High),
    ];
    let mut tally: BTreeMap<&str, Vec<Probed>> = BTreeMap::new();
    for i in 0..n {
        for (name, fallback, effort) in arms {
            let sampling = SamplingConfig {
                temperature: Some(0.4),
                thinking: Some(true),
                reasoning_effort: Some(effort),
                max_tokens: Some(16_384),
                ..SamplingConfig::default()
            };
            let port = EngineAdapter::sharing(
                backend.clone(),
                log.clone(),
                &model,
                sampling,
                CancellationToken::new(),
            );
            let port = if fallback { port.fallback_only() } else { port };
            let address = format!("probe/{}/{i}", name.replace([',', ' '], ""));
            let call = CallAddress {
                run: run.clone(),
                address: address.clone(),
            };
            let reply: ContinuityReply =
                match decide(&port, &call, Role::Continuity, &schema, &prompt).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("{name} #{i}: failed: {e}");
                        continue;
                    }
                };
            let found: Vec<bool> = faults
                .iter()
                .map(|f| {
                    reply.findings.iter().any(|x| {
                        overlaps(&x.anchor, &f.sentence)
                            || f.words.iter().any(|w| x.note.to_lowercase().contains(w))
                    })
                })
                .collect();
            let other = reply
                .findings
                .iter()
                .filter(|x| {
                    !faults.iter().any(|f| {
                        overlaps(&x.anchor, &f.sentence)
                            || f.words.iter().any(|w| x.note.to_lowercase().contains(w))
                    })
                })
                .count();
            let stored = db.call(&run, &address, 1).await.unwrap().unwrap();
            let reasoning = stored.call.response.as_ref().unwrap()["reasoning_tokens"]
                .as_u64()
                .unwrap_or(0);
            eprintln!(
                "{name} #{i}: found {found:?}, {other} other findings, {reasoning} reasoning tokens"
            );
            tally
                .entry(name)
                .or_default()
                .push((found, other, reasoning));
        }
    }
    eprintln!(
        "\n{model}: the Continuity checker on a chapter with three planted faults, {n} calls an arm"
    );
    eprintln!(
        "| Arm | Calls | Leak found | Cast error found | Death found | Other findings | Reasoning tokens, median |"
    );
    eprintln!("| --- | --- | --- | --- | --- | --- | --- |");
    for (name, _, _) in arms {
        let rows = tally.get(name).cloned().unwrap_or_default();
        let hit = |k: usize| rows.iter().filter(|r| r.0[k]).count();
        let mut reasoning: Vec<u64> = rows.iter().map(|r| r.2).collect();
        reasoning.sort_unstable();
        let median = reasoning.get(reasoning.len() / 2).copied().unwrap_or(0);
        eprintln!(
            "| {name} | {} | {} | {} | {} | {} | {median} |",
            rows.len(),
            hit(0),
            hit(1),
            hit(2),
            rows.iter().map(|r| r.1).sum::<usize>(),
        );
    }
}

#[test]
fn an_anchor_overlaps_by_three_words_running() {
    let s = "Then Silas slipped on the wet stones, struck their head, and lay dead.";
    assert!(overlaps("slipped on the wet", s));
    assert!(overlaps("lay dead", s), "a short anchor whole");
    assert!(!overlaps("the wet sails", s));
    assert!(!overlaps("", s));
}
