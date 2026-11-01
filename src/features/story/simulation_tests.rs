//! The simulation on a scripted port: a round's four steps, what each role
//! is shown, the engine's checks on the World Master's events, the endings,
//! the K branches, and a scene recorded through the engine adapter that
//! replays identically from its call log.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::entities::sampling::SamplingConfig;
use crate::entities::story::{ProposalVerdict, fixture_canon};
use crate::features::story::contracts::{IntentRuling, ReactionRuling};
use crate::features::story::engine::EngineAdapter;
use crate::features::story::leaks::{AuditedCall, audit};
use crate::features::story::port::{LlmPort, PortError, Prompt, Role};
use crate::features::story::replay::ReplayAdapter;
use crate::features::story::scene::harbour_script;
use crate::features::story::scene::tests::{Story, fresh_story};
use crate::features::story::schema::{Schema, describe};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason,
};

/// A call the scripted port saw.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub address: String,
    pub role: Role,
    pub prompt: Prompt,
}

type Answer = Box<dyn Fn(&str, &Prompt) -> Result<Value, PortError> + Send + Sync>;

/// A port that answers each call from a script on its address, holds every
/// answer to its schema (a mistaken script fails loudly), and keeps the calls.
pub(crate) struct Scripted {
    answer: Answer,
    calls: Mutex<Vec<Seen>>,
}

impl Scripted {
    pub(crate) fn new(
        answer: impl Fn(&str, &Prompt) -> Result<Value, PortError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(answer),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn calls(&self) -> Vec<Seen> {
        self.calls.lock().unwrap().clone()
    }

    /// The calls of one act (`propose`, `initiative`, ...), in the order made.
    fn of(&self, act: &str) -> Vec<Seen> {
        self.calls()
            .into_iter()
            .filter(|c| c.address.ends_with(&format!("/{act}")))
            .collect()
    }
}

#[async_trait::async_trait]
impl LlmPort for Scripted {
    async fn json(
        &self,
        call: &CallAddress,
        role: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Result<Value, PortError> {
        self.calls.lock().unwrap().push(Seen {
            address: call.address.clone(),
            role,
            prompt: prompt.clone(),
        });
        let value = (self.answer)(&call.address, prompt)?;
        schema.validate(&value).map_err(|v| PortError::Invalid {
            address: call.address.clone(),
            violations: describe(&v),
        })
    }

    async fn text(&self, _: &CallAddress, _: Role, _: &Prompt) -> Result<String, PortError> {
        unreachable!("the simulation asks for no prose")
    }
}

/// The character and the act a call's address names.
pub(crate) fn who_and_act(address: &str) -> (&str, &str) {
    let mut parts = address.rsplitn(3, '/');
    let act = parts.next().unwrap();
    let who = parts.next().unwrap();
    (who, act)
}

/// The round a call's address names.
fn round_of(address: &str) -> u32 {
    address
        .split('/')
        .find_map(|p| p.strip_prefix("round"))
        .and_then(|n| n.parse().ok())
        .unwrap()
}

pub(crate) fn intent(thought: &str, action: Value, visibility: Value) -> Value {
    json!({"private_thought": thought, "action": action, "targets": [], "visibility": visibility})
}

pub(crate) fn says(who: &str, line: &str) -> Value {
    intent(
        &format!("{who} means it."),
        json!({"type": "speak", "line": line}),
        json!({"type": "public"}),
    )
}

pub(crate) fn waits() -> Value {
    intent("", json!({"type": "wait"}), json!({"type": "public"}))
}

pub(crate) fn feasible(who: &str) -> Value {
    json!({"character": who, "cause": "Nothing stops it.", "verdict": "feasible"})
}

pub(crate) fn initiative(
    rulings: Vec<Value>,
    initiator: Option<&str>,
    observable: &str,
    effects: Value,
) -> Value {
    json!({
        "rulings": rulings,
        "initiator": initiator,
        "observable": observable,
        "effects": effects,
        "world_event": null,
        "scene_over": false,
    })
}

pub(crate) fn reflection(text: &str) -> Value {
    json!({"reflection": text, "goal_changes": []})
}

fn plan() -> ScenePlan {
    let script = harbour_script();
    ScenePlan {
        chapter: 1,
        scene: 1,
        rerun: 0,
        setup: script.setup,
        intent: script.intent,
    }
}

fn config(branches: u32, max_rounds: u32, reflect_every: u32) -> SimConfig {
    SimConfig {
        branches,
        max_rounds,
        abnormal_p: 0.0,
        reflect_every,
    }
}

async fn simulate(port: &dyn LlmPort, config: &SimConfig) -> (Story, SceneRun) {
    let (mut story, log) = fresh_story().await;
    drop(log);
    let run = simulate_scene(
        &story.db,
        &mut story.author,
        port,
        &story.run,
        &plan(),
        config,
    )
    .await
    .unwrap();
    (story, run)
}

fn played(result: &BranchResult) -> &BranchRun {
    match result {
        BranchResult::Played(run) => run,
        BranchResult::Failed { error, .. } => panic!("the branch failed: {error}"),
    }
}

/// The round of the module's head on the harbour: Mara speaks and opens the
/// round, Silas's feasible intent lapses, Ines waits; Silas answers Mara,
/// Ines lets it pass; the world adds thunder; all three reflect.
fn one_round(address: &str, _: &Prompt) -> Result<Value, PortError> {
    let (who, act) = who_and_act(address);
    Ok(match (who, act) {
        ("mara", "propose") => says("mara", "Gate shuts at sunset. Go home."),
        ("silas", "propose") => intent(
            "My crates must not be opened.",
            json!({"type": "act", "verb": "asks after", "object": "his crates", "line": null}),
            json!({"type": "public"}),
        ),
        ("ines", "propose") | ("ines", "react") => waits(),
        ("world_master", "initiative") => {
            let mut ruling = initiative(
                vec![feasible("mara"), feasible("silas")],
                Some("mara"),
                "Mara says: \"Gate shuts at sunset. Go home.\"",
                json!([]),
            );
            ruling["world_event"] = json!("Thunder rolls in over the quay.");
            ruling
        }
        ("silas", "react") => says("silas", "A wise man waits out a storm."),
        ("world_master", "reactions") => json!({"rulings": [{
            "character": "silas", "cause": "He is standing right there.", "verdict": "commit",
            "observable": "Silas says: \"A wise man waits out a storm.\"", "effects": []
        }]}),
        (_, "reflect") => reflection(&format!("{who} does not trust the others.")),
        other => panic!("unscripted call {other:?}"),
    })
}

#[tokio::test]
async fn a_round_runs_its_four_steps_in_order() {
    let port = Scripted::new(one_round);
    let (story, scene) = simulate(&port, &config(1, 1, 1)).await;
    assert_eq!(scene.scene.as_str(), "ch1/sc1/r0");
    let run = played(&scene.branches[0]);
    assert_eq!(run.branch.as_str(), "ch1/sc1/r0/k0");
    assert_eq!((run.rounds, run.ending), (1, Ending::Rounds));

    let addresses: Vec<String> = port.calls().into_iter().map(|c| c.address).collect();
    let tail: Vec<&str> = addresses
        .iter()
        .map(|a| a.strip_prefix("ch1/sc1/r0/k0/round1/").unwrap())
        .collect();
    assert_eq!(
        tail,
        [
            "mara/propose",
            "silas/propose",
            "ines/propose",
            "world_master/initiative",
            "silas/react",
            "ines/react",
            "world_master/reactions",
            "mara/reflect",
            "silas/reflect",
            "ines/reflect",
        ]
    );
    let roles: Vec<Role> = port.calls().iter().map(|c| c.role).collect();
    assert_eq!(roles[3], Role::WorldMaster);
    assert_eq!(roles[0], Role::Character);

    let kinds: Vec<(EventKind, Option<&str>)> = run
        .events
        .iter()
        .map(|e| (e.body.kind, e.body.actor.as_ref().map(CharacterKey::as_str)))
        .collect();
    assert_eq!(
        kinds,
        [
            (EventKind::Opening, None),
            (EventKind::Opening, None),
            (EventKind::Speech, Some("mara")),
            (EventKind::WorldEvent, None),
            (EventKind::Speech, Some("silas")),
        ]
    );
    let opening = &run.events[2];
    assert_eq!(opening.body.cause, Cause::Initiative);
    assert_eq!(opening.body.private.as_deref(), Some("mara means it."));
    assert_eq!(opening.witnesses.len(), 3);
    assert_eq!(
        run.events[4].body.cause,
        Cause::Reaction {
            to: opening.id.clone()
        }
    );

    let verdicts: Vec<(&str, ProposalVerdict, Option<&str>)> = run
        .proposals
        .iter()
        .map(|p| (p.character.as_str(), p.verdict, p.reason.as_deref()))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("mara", ProposalVerdict::Committed, None),
            ("silas", ProposalVerdict::Lapsed, Some(NOT_TAKEN)),
            ("ines", ProposalVerdict::Lapsed, Some(WAITS)),
            ("silas", ProposalVerdict::Committed, None),
            ("ines", ProposalVerdict::Lapsed, Some(WAITS)),
        ]
    );
    assert_eq!(opening.body.proposal.as_ref(), Some(&run.proposals[0].id));
    assert_eq!(
        run.events[4].body.proposal.as_ref(),
        Some(&run.proposals[3].id)
    );
    assert_eq!(run.reflections.len(), 3);
    assert_eq!(run.reflections[0].1.body, "mara does not trust the others.");
    assert_eq!(
        run.tally,
        Tally {
            intents: 3,
            reactions: 2,
            waits: 2,
            committed: 2,
            world_events: 1,
            refused_by_ruling: 0,
            refused_by_engine: Vec::new(),
            slips: Vec::new(),
            reflections: 3,
        }
    );
    // The trace carries all of it.
    let trace = scene.to_json();
    assert_eq!(trace["branches"][0]["events"].as_array().unwrap().len(), 5);
    assert_eq!(trace["branches"][0]["tally"]["world_events"], 1);
    assert_eq!(trace["branches"][0]["ending"], "rounds");
    drop(story);
}

#[tokio::test]
async fn each_role_is_shown_only_what_it_may_know() {
    let port = Scripted::new(one_round);
    let (_story, _) = simulate(&port, &config(1, 1, 1)).await;
    let canon = fixture_canon();
    let intent = plan().intent;
    for call in port.calls() {
        let text = format!("{}\n{}", call.prompt.system, call.prompt.user);
        assert!(
            !text.contains(&intent),
            "{}: the Author's intent",
            call.address
        );
        let (who, _) = who_and_act(&call.address);
        for character in &canon.characters {
            for secret in &character.secrets {
                let token = leaks::token_of(&secret.text).unwrap();
                let shown = text.contains(token);
                match call.role {
                    // The World Master rules on reveals, so it knows them all.
                    Role::WorldMaster => assert!(shown, "{}: {token}", call.address),
                    _ if character.key.as_str() == who => {
                        assert!(shown, "{}: its own {token}", call.address)
                    }
                    _ => assert!(!shown, "{}: {token} leaked", call.address),
                }
            }
        }
        if call.role == Role::WorldMaster {
            // Never a character's thought: the engine copies it into the event.
            assert!(!text.contains("means it."), "{}", call.address);
            assert!(!text.contains("must not be opened"), "{}", call.address);
        }
    }
    // Silas reacts to what he perceived, Mara's line, and is told of it.
    let react = &port.of("react")[0];
    assert!(react.prompt.user.contains("Just now: Mara says"));
    // A reflection sees the round's reaction.
    let reflect = &port.of("reflect")[0];
    assert!(
        reflect
            .prompt
            .user
            .contains("A wise man waits out a storm.")
    );
    // Her own thought comes back to her, the others' never.
    assert!(
        reflect
            .prompt
            .user
            .contains("(You thought: mara means it.)")
    );
    let silas = port
        .of("reflect")
        .into_iter()
        .find(|c| c.address.contains("/silas/"))
        .unwrap();
    assert!(!silas.prompt.user.contains("mara means it."));
}

/// A whisper that gives Mara's secret away, its token and all: refused
/// while the ruling gives no reveal, committed when it reveals to Ines.
fn whisper_secret(reveal: bool) -> impl Fn(&str, &Prompt) -> Result<Value, PortError> {
    move |address, _| {
        let (who, act) = who_and_act(address);
        Ok(match (who, act) {
            ("mara", "propose") if round_of(address) == 1 => intent(
                "She has to know.",
                json!({"type": "speak", "line": "I took the money. [SECRET-MARA-7Q]"}),
                json!({"type": "whisper", "to": ["ines"]}),
            ),
            ("world_master", "initiative") => {
                let rulings = vec![feasible("mara")];
                let effects = if reveal {
                    json!([{"type": "reveal", "secret": "bribe", "to": "ines"}])
                } else {
                    json!([])
                };
                initiative(
                    rulings,
                    Some("mara"),
                    "Mara whispers to Ines: \"I took the money. [SECRET-MARA-7Q]\"",
                    effects,
                )
            }
            ("world_master", "reactions") => json!({"rulings": []}),
            (_, "reflect") => reflection("Hm."),
            _ => waits(),
        })
    }
}

#[tokio::test]
async fn a_secret_given_away_needs_a_reveal() {
    let port = Scripted::new(whisper_secret(false));
    let (_story, scene) = simulate(&port, &config(1, 1, 0)).await;
    let run = played(&scene.branches[0]);
    assert_eq!(run.tally.committed, 0);
    assert_eq!(run.tally.refused_by_engine.len(), 1);
    let reason = &run.tally.refused_by_engine[0];
    assert!(
        reason.contains("gives `bribe` away to ines, who does not know it"),
        "{reason}"
    );
    assert_eq!(run.proposals[0].verdict, ProposalVerdict::Refused);
    assert!(
        run.proposals[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("reveal")
    );

    let port = Scripted::new(whisper_secret(true));
    let (story, scene) = simulate(&port, &config(1, 2, 0)).await;
    let run = played(&scene.branches[0]);
    assert_eq!(run.tally.refused_by_engine, Vec::<String>::new());
    let whisper = &run.events[2];
    assert_eq!(
        whisper
            .witnesses
            .iter()
            .map(CharacterKey::as_str)
            .collect::<Vec<_>>(),
        ["ines", "mara"]
    );
    // Ines alone was told, and is told from then on; Silas never.
    let round2 = |who: &str| {
        port.of("propose")
            .into_iter()
            .find(|c| round_of(&c.address) == 2 && c.address.contains(&format!("/{who}/")))
            .unwrap()
    };
    assert!(
        round2("ines")
            .prompt
            .user
            .contains("Mara's secret: She took a bribe")
    );
    assert!(
        round2("ines")
            .prompt
            .user
            .contains("(Whispered to you) Mara whispers")
    );
    assert!(!round2("silas").prompt.user.contains("SECRET-MARA"));
    // The audit agrees: nothing reached anyone before they knew it.
    let calls = port.calls();
    let requests: Vec<Value> = calls
        .iter()
        .map(|c| json!({"system": c.prompt.system, "user": c.prompt.user}))
        .collect();
    let audited: Vec<AuditedCall> = calls
        .iter()
        .zip(&requests)
        .map(|(c, request)| AuditedCall {
            address: &c.address,
            role: if c.role == Role::Character {
                "character"
            } else {
                "world_master"
            },
            request,
            response: None,
        })
        .collect();
    let paths = BTreeMap::from([(
        run.branch.clone(),
        story.db.path(&run.branch).await.unwrap(),
    )]);
    assert_eq!(audit(&fixture_canon(), &paths, &audited), Vec::new());
}

#[tokio::test]
async fn an_effect_the_world_does_not_allow_is_refused_by_the_engine() {
    let port = Scripted::new(|address, _| {
        let (who, act) = who_and_act(address);
        Ok(match (who, act) {
            ("silas", "propose") => intent(
                "The ledger.",
                json!({"type": "act", "verb": "hands over", "object": "the ledger", "line": null}),
                json!({"type": "public"}),
            ),
            ("world_master", "initiative") => initiative(
                vec![feasible("silas")],
                Some("silas"),
                "Silas hands the ledger to Ines.",
                // Mara holds the ledger, not Silas.
                json!([{"type": "give", "from": "silas", "to": "ines", "item": "ledger"}]),
            ),
            _ => waits(),
        })
    });
    let (_story, scene) = simulate(&port, &config(1, 1, 0)).await;
    let run = played(&scene.branches[0]);
    assert_eq!(
        run.tally.refused_by_engine,
        ["round 1, `silas`: the world refuses it: silas does not hold ledger"]
    );
    let silas = run
        .proposals
        .iter()
        .find(|p| p.character.as_str() == "silas")
        .unwrap();
    assert_eq!(silas.verdict, ProposalVerdict::Refused);
    assert_eq!(run.events.len(), 2, "the openings alone");
}

#[tokio::test]
async fn a_scene_ends_in_a_stalemate_when_it_resolves_or_when_it_empties() {
    // Everyone waits: two quiet rounds end it.
    let port = Scripted::new(|address, _| {
        Ok(match who_and_act(address) {
            ("world_master", _) => initiative(vec![], None, "", json!([])),
            _ => waits(),
        })
    });
    let (_s, scene) = simulate(&port, &config(1, 8, 0)).await;
    let run = played(&scene.branches[0]);
    assert_eq!((run.rounds, run.ending), (2, Ending::Stalemate));
    assert_eq!(run.tally.waits, 6);
    assert_eq!(
        port.of("initiative").len(),
        2,
        "the World Master rules on an all-wait round"
    );

    // The World Master calls it over.
    let port = Scripted::new(|address, _| {
        Ok(match who_and_act(address) {
            ("world_master", "initiative") => {
                let mut r = initiative(vec![], None, "", json!([]));
                r["scene_over"] = json!(true);
                r
            }
            _ => waits(),
        })
    });
    let (_s, scene) = simulate(&port, &config(1, 8, 0)).await;
    let run = played(&scene.branches[0]);
    assert_eq!((run.rounds, run.ending), (1, Ending::Resolved));

    // Everyone walks off the quay: the next round finds no one.
    let port = Scripted::new(|address, prompt| {
        let (who, act) = who_and_act(address);
        Ok(match (who, act) {
            (_, "propose") if prompt.user.contains("Round 1.") => intent(
                "Enough.",
                json!({"type": "move", "to": "inn"}),
                json!({"type": "public"}),
            ),
            ("world_master", "initiative") => {
                let mover = if prompt.user.contains("`mara` goes") {
                    "mara"
                } else {
                    "silas"
                };
                // No move effect: the engine adds the deed's own.
                initiative(
                    vec![feasible("mara"), feasible("silas"), feasible("ines")],
                    Some(mover),
                    "",
                    json!([]),
                )
            }
            ("world_master", "reactions") => json!({"rulings": [
                {"character": "silas", "cause": "The door is open.", "verdict": "commit",
                 "observable": "Silas follows her out.", "effects": []},
                {"character": "ines", "cause": "The door is open.", "verdict": "commit",
                 "observable": "Ines goes too.", "effects": []}
            ]}),
            (_, "react") => intent(
                "Not alone here.",
                json!({"type": "move", "to": "inn"}),
                json!({"type": "public"}),
            ),
            _ => waits(),
        })
    });
    let (_s, scene) = simulate(&port, &config(1, 8, 0)).await;
    let run = played(&scene.branches[0]);
    assert_eq!((run.rounds, run.ending), (1, Ending::Empty));
    assert_eq!(run.tally.committed, 3);
    let moves: Vec<&str> = run
        .events
        .iter()
        .filter(|e| e.body.kind == EventKind::Movement)
        .map(|e| e.body.observable.as_str())
        .collect();
    // The empty observable falls back to the deed as told by M0's rule.
    assert_eq!(
        moves,
        [
            "Mara leaves for The Gull Inn.",
            "Silas follows her out.",
            "Ines goes too."
        ]
    );
}

#[tokio::test]
async fn branches_run_one_after_another_and_a_failed_one_spares_its_siblings() {
    let port = Scripted::new(|address, _| {
        if address.starts_with("ch1/sc1/r0/k1/") && address.ends_with("/initiative") {
            return Err(PortError::Failed {
                address: address.to_owned(),
                message: "the provider is down".to_owned(),
            });
        }
        let (who, act) = who_and_act(address);
        Ok(match (who, act) {
            ("mara", "propose") => says("mara", "Storm's early."),
            ("world_master", "initiative") => initiative(
                vec![feasible("mara")],
                Some("mara"),
                "Mara says: \"Storm's early.\"",
                json!([]),
            ),
            ("world_master", "reactions") => json!({"rulings": []}),
            _ => waits(),
        })
    });
    let (story, scene) = simulate(&port, &config(3, 1, 0)).await;
    let ids: Vec<&str> = scene.branches.iter().map(|b| b.branch().as_str()).collect();
    assert_eq!(ids, ["ch1/sc1/r0/k0", "ch1/sc1/r0/k1", "ch1/sc1/r0/k2"]);
    assert!(
        matches!(&scene.branches[1], BranchResult::Failed { error, .. } if error.contains("the provider is down"))
    );
    for k in [0, 2] {
        let run = played(&scene.branches[k]);
        assert_eq!(run.tally.committed, 1);
        // A branch never sees its sibling's events.
        let path = story.db.path(&run.branch).await.unwrap();
        assert!(
            path.iter().all(|e| e.branch == run.branch),
            "{:?}",
            run.branch
        );
    }
    // One after another: no call of k2 before the last of k0.
    let calls = port.calls();
    let last_k0 = calls
        .iter()
        .rposition(|c| c.address.contains("/k0/"))
        .unwrap();
    let first_k2 = calls
        .iter()
        .position(|c| c.address.contains("/k2/"))
        .unwrap();
    assert!(last_k0 < first_k2);
    let trace = scene.to_json();
    assert!(
        trace["branches"][1]["failed"]
            .as_str()
            .unwrap()
            .contains("down")
    );
}

#[tokio::test]
async fn a_cancelled_call_stops_the_scene() {
    let port = Scripted::new(|address, _| {
        Err(PortError::Cancelled {
            address: address.to_owned(),
        })
    });
    let (mut story, log) = fresh_story().await;
    drop(log);
    let result = simulate_scene(
        &story.db,
        &mut story.author,
        &port,
        &story.run,
        &plan(),
        &config(3, 1, 0),
    )
    .await;
    assert!(matches!(
        result,
        Err(SceneError::Port(PortError::Cancelled { .. }))
    ));
    let calls = port.calls();
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|c| c.address.starts_with("ch1/sc1/r0/k0/round1/")
                && c.address.ends_with("/propose")),
        "the first round's intents on the first branch, then nothing"
    );
}

fn present() -> Vec<CharacterKey> {
    ["mara", "silas", "ines"].map(CharacterKey::from).to_vec()
}

fn proposals(values: [Value; 3]) -> Vec<Proposal> {
    values
        .into_iter()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect()
}

fn ruling(rulings: Vec<IntentRuling>, initiator: Option<&str>) -> InitiativeRuling {
    InitiativeRuling {
        rulings,
        initiator: initiator.map(CharacterKey::from),
        observable: String::new(),
        effects: Vec::new(),
        world_event: None,
        scene_over: false,
    }
}

fn on(who: &str, verdict: IntentVerdict) -> IntentRuling {
    IntentRuling {
        character: who.into(),
        cause: format!(" {who}'s cause "),
        verdict,
    }
}

#[test]
fn a_ruling_that_does_not_answer_its_intents_is_read_with_its_slips() {
    let intents = proposals([says("mara", "a"), says("silas", "b"), waits()]);
    // A clean ruling: one refused, one picked.
    let read = read_intents(
        &present(),
        &intents,
        &ruling(
            vec![
                on("mara", IntentVerdict::Refused),
                on("silas", IntentVerdict::Feasible),
            ],
            Some("silas"),
        ),
    );
    assert_eq!(read.initiator, Some(1));
    assert_eq!(read.refused, 1);
    assert_eq!(read.slips, Vec::<String>::new());
    assert_eq!(
        read.verdicts,
        [
            (ProposalVerdict::Refused, Some("mara's cause".to_owned())),
            (ProposalVerdict::Lapsed, Some(NOT_TAKEN.to_owned())),
            (ProposalVerdict::Lapsed, Some(WAITS.to_owned())),
        ]
    );
    // Every way it can go wrong.
    let read = read_intents(
        &present(),
        &intents,
        &ruling(
            vec![
                on("ines", IntentVerdict::Feasible),
                on("mara", IntentVerdict::Refused),
                on("mara", IntentVerdict::Feasible),
            ],
            Some("mara"),
        ),
    );
    assert_eq!(read.initiator, None);
    assert_eq!(
        read.slips,
        [
            "ruled on `ines`, who proposed no deed",
            "ruled twice on `mara`",
            "did not rule on `silas`",
            "named `mara` as the initiator, whose intent was not feasible",
        ]
    );
    assert_eq!(
        read.verdicts[1],
        (ProposalVerdict::Lapsed, Some(NOT_RULED.to_owned()))
    );
    let read = read_intents(
        &present(),
        &intents,
        &ruling(vec![on("mara", IntentVerdict::Feasible)], None),
    );
    assert_eq!(
        read.slips,
        [
            "did not rule on `silas`",
            "named no initiator though an intent was feasible"
        ]
    );
    let all_wait = proposals([waits(), waits(), waits()]);
    let read = read_intents(&present(), &all_wait, &ruling(vec![], None));
    assert_eq!(read.slips, Vec::<String>::new());
    assert!(read.verdicts.iter().all(|v| v.1.as_deref() == Some(WAITS)));
}

#[test]
fn reactions_are_read_in_the_ruling_order() {
    let reactors = vec![CharacterKey::from("silas"), CharacterKey::from("ines")];
    let reactions: Vec<Proposal> = [says("silas", "a"), says("ines", "b")]
        .into_iter()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect();
    let rule = |who: &str, verdict| ReactionRuling {
        character: who.into(),
        cause: " too far ".to_owned(),
        verdict,
        observable: format!("{who} answers."),
        effects: Vec::new(),
    };
    let ruling = ReactionsRuling {
        rulings: vec![
            rule("ines", ReactionVerdict::Commit),
            rule("silas", ReactionVerdict::Refuse),
            rule("mara", ReactionVerdict::Commit),
            rule("ines", ReactionVerdict::Refuse),
        ],
    };
    let read = read_reactions(&reactors, &reactions, &ruling);
    assert_eq!(
        read.steps,
        [
            Step::Commit {
                i: 1,
                observable: "ines answers.",
                effects: &[]
            },
            Step::Refuse {
                i: 0,
                cause: "too far"
            },
        ]
    );
    assert_eq!(
        read.slips,
        [
            "ruled on `mara`, who proposed no reaction",
            "ruled twice on `ines`'s reaction"
        ]
    );
    assert!(read.lapsed.is_empty());
    let none = ReactionsRuling { rulings: vec![] };
    let read = read_reactions(
        &reactors,
        &[
            reactions[0].clone(),
            serde_json::from_value(waits()).unwrap(),
        ],
        &none,
    );
    assert_eq!(read.lapsed, [(0, NOT_RULED), (1, WAITS)]);
    assert_eq!(read.slips, ["did not rule on `silas`'s reaction"]);
}

#[test]
fn a_routine_breaks_by_the_seed_alone() {
    let mara = CharacterKey::from("mara");
    assert!(!breaks_routine(7, 1, &mara, 0.0));
    assert!(breaks_routine(7, 1, &mara, 1.0));
    assert_eq!(
        breaks_routine(7, 3, &mara, 0.3),
        breaks_routine(7, 3, &mara, 0.3)
    );
    let hits = (0..2000)
        .filter(|&round| breaks_routine(42, round, &mara, 0.3))
        .count();
    assert!((450..750).contains(&hits), "{hits} of 2000 at p = 0.3");
}

#[test]
fn a_move_is_given_its_effect_and_an_empty_observable_the_deed() {
    let canon = fixture_canon();
    let here = LocationKey::from("harbour");
    let mara = CharacterKey::from("mara");
    let go: Proposal = serde_json::from_value(intent(
        " Off. ",
        json!({"type": "move", "to": "inn"}),
        json!({"type": "public"}),
    ))
    .unwrap();
    let event = deed_event(
        &canon,
        &here,
        &mara,
        &go,
        ("  ", &[]),
        (2, 5, Cause::Initiative),
    )
    .unwrap();
    assert_eq!(event.observable, "Mara leaves for The Gull Inn.");
    assert_eq!(event.private.as_deref(), Some("Off."));
    assert_eq!(
        event.effects,
        [Effect::Move {
            character: mara.clone(),
            to: "inn".into()
        }]
    );
    assert_eq!(
        (event.round, event.tick, event.kind),
        (2, 5, EventKind::Movement)
    );
    // A move the World Master wrote stays as written.
    let written = [Effect::Move {
        character: mara.clone(),
        to: "warehouse".into(),
    }];
    let event = deed_event(
        &canon,
        &here,
        &mara,
        &go,
        ("She goes.", &written),
        (2, 5, Cause::World),
    )
    .unwrap();
    assert_eq!(event.effects, written);
    assert_eq!(event.observable, "She goes.");
    let wait: Proposal = serde_json::from_value(waits()).unwrap();
    assert_eq!(
        deed_event(&canon, &here, &mara, &wait, ("", &[]), (1, 1, Cause::World)),
        None
    );
}

#[test]
fn the_engine_check_guards_the_actors_thought_too() {
    let canon = fixture_canon();
    let world = WorldState::initial(&canon);
    let mut event = scripted_event(
        &canon,
        &"inn".into(),
        &"silas".into(),
        &serde_json::from_value(says("silas", "Wine, only wine.")).unwrap(),
        1,
        1,
    )
    .unwrap();
    assert!(check(&canon, &world, &event).is_ok());
    event.private = Some("Does she know of SECRET-INES-9D?".to_owned());
    assert_eq!(
        check(&canon, &world, &event).unwrap_err(),
        "silas thinks of `signal`, which they do not know"
    );
    event.private = Some("The rifles, SECRET-SILAS-3K, are safe.".to_owned());
    assert!(check(&canon, &world, &event).is_ok());
}

// ---------------------------------------------------------------------------
// Through the engine adapter, and back from its call log.

/// A model that plays every role from the prompt alone: each character
/// speaks, the World Master takes every deed listed, in order.
struct Ensemble;

fn listed_characters(user: &str) -> Vec<String> {
    user.lines()
        .filter_map(|l| l.strip_prefix("- `"))
        .filter_map(|l| l.split_once("` "))
        .filter(|(_, rest)| rest.starts_with("says"))
        .map(|(who, _)| who.to_owned())
        .collect()
}

fn ensemble_reply(req: &ChatRequest) -> String {
    let system = req.system.clone().unwrap_or_default();
    let user = req
        .messages
        .last()
        .map(|m| m.content.clone())
        .unwrap_or_default();
    if let Some(rest) = system.strip_prefix("You play ") {
        let name = rest.split(' ').next().unwrap_or_default();
        if user.contains("The scene pauses.") {
            return reflection(&format!("{name} is uneasy.")).to_string();
        }
        let round = user.split('.').next().unwrap_or_default();
        return says(name, &format!("{name}, {round}.")).to_string();
    }
    let who = listed_characters(&user);
    if user.contains("The reactions to it:") {
        let rulings: Vec<Value> = who
            .iter()
            .map(|w| {
                json!({"character": w, "cause": "They are here.", "verdict": "commit",
                             "observable": format!("{w} answers."), "effects": []})
            })
            .collect();
        return json!({"rulings": rulings}).to_string();
    }
    initiative(
        who.iter().map(|w| feasible(w)).collect(),
        who.first().map(String::as_str),
        &format!("{} speaks.", who.first().cloned().unwrap_or_default()),
        json!([]),
    )
    .to_string()
}

#[async_trait::async_trait]
impl EngineBackend for Ensemble {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        _: CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        let chunks = vec![
            ChatChunk::Text(ensemble_reply(&req)),
            ChatChunk::Finished(FinishReason::Stop),
        ];
        Ok(Box::pin(futures_util::stream::iter(chunks)))
    }
}

#[tokio::test]
async fn a_simulated_scene_replays_identically_from_its_call_log() {
    let config = SimConfig {
        branches: 2,
        max_rounds: 3,
        abnormal_p: 0.5,
        reflect_every: 2,
    };
    let (mut story, log) = fresh_story().await;
    let port = EngineAdapter::new(
        Arc::new(Ensemble),
        log,
        "ensemble",
        SamplingConfig::default(),
        CancellationToken::new(),
    );
    let live = simulate_scene(
        &story.db,
        &mut story.author,
        &port,
        &story.run,
        &plan(),
        &config,
    )
    .await
    .unwrap();
    let run = played(&live.branches[0]);
    assert_eq!(run.rounds, 3);
    assert!(run.tally.committed >= 3, "{:?}", run.tally);
    assert_eq!(run.tally.slips, Vec::<String>::new());
    assert_eq!(run.tally.reflections, 3);
    let calls = story.db.calls(&story.run).await.unwrap();
    assert!(calls.iter().all(|c| c.call.attempt == 1));

    // The same scene on a fresh story, served from those calls alone.
    let (mut again, mut log) = fresh_story().await;
    for call in calls {
        let mut call = call.call;
        call.run = again.run.clone();
        log.record_call(call).await.unwrap();
    }
    drop(log);
    let replay = ReplayAdapter::new(again.db.clone());
    let replayed = simulate_scene(
        &again.db,
        &mut again.author,
        &replay,
        &again.run,
        &plan(),
        &config,
    )
    .await
    .unwrap();
    assert_eq!(replay.drift(), Vec::new());
    assert_eq!(replayed.to_json(), live.to_json());
}

#[test]
fn the_ensemble_reads_the_deeds_listed() {
    let user = "x\n- `mara` says \"a\", openly. They perceived: e1.\n- `silas` goes to Inn.";
    assert_eq!(listed_characters(user), ["mara"]);
}
