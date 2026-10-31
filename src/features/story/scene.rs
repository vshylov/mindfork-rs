//! One scene played by script (docs/fabula.md §4, PR 2; docs/research/
//! fabula-engine.md §7): the setup's opening events, then rounds in which
//! every present character proposes through the model port, and a fixed rule
//! rules on the proposals where M1's World Master will. It is M0's whole
//! simulation: enough to run the port, the call log, the validators and the
//! store end to end, and to replay all of it.
//!
//! The rule: in round n the present characters are taken in the canon's
//! order, starting from the n-th, and the first proposal the world admits is
//! the round's initiative and is committed; one the world refuses is recorded
//! with the world's reason, and the rest lapse. A proposal to wait commits
//! nothing. The scene ends after its rounds, when no one is left at its
//! location, or after two rounds with no event (spec §7). M1 replaces the
//! rule with the World Master's contract, and the round with intent,
//! initiative, reaction and reflection (spec §7, *One simulation round*).
//!
//! A character's prompt holds its own sheet, the others' public descriptions
//! and what it perceived on its branch path, never another character's
//! secrets (spec §4, rule 5).

use futures_util::future::try_join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::port::{CallAddress, LlmPort, PortError, Prompt, Role, decide};
use super::schema::{CanonKeys, Schema, StorySchema};
use crate::entities::chat_file::sha256_hex;
use crate::entities::story::{
    Action, BranchId, Canon, Cause, CharacterKey, Effect, Event, EventKind, Holder, LocationKey,
    NewEvent, Proposal, ProposalVerdict, RunId, SceneId, SceneSetup, Visibility, WorldState,
};
use crate::shared::storage::story::{
    AuthorWriter, BranchWriter, StoredProposal, StoryDb, StoryDbError,
};

/// Why a lapsed proposal lapsed: it asked to wait.
pub const WAITS: &str = "waits";
/// Why a lapsed proposal lapsed: another one took the round first.
pub const NOT_TAKEN: &str = "another proposal took the round";

/// What the scene is: where it sits in the story, the Author's
/// circumstances and intent, and how many rounds it may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneScript {
    /// The chapter, from 1.
    pub chapter: u32,
    /// The scene within it, from 1.
    pub scene: u32,
    /// The rerun, from 0.
    pub rerun: u32,
    /// The branch, from 0.
    pub k: u32,
    /// The given circumstances.
    pub setup: SceneSetup,
    /// The Author's private intent; never sent to a character.
    pub intent: String,
    /// The most rounds the scene runs.
    pub rounds: u32,
}

/// What a played scene left in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneOutcome {
    /// The scene.
    pub scene: SceneId,
    /// Its branch.
    pub branch: BranchId,
    /// The events the branch committed, in order.
    pub events: Vec<Event>,
    /// The proposals with their rulings, in the order recorded.
    pub proposals: Vec<StoredProposal>,
}

impl SceneOutcome {
    /// The scene's trace: its events with their witnesses, and the proposals
    /// with their rulings. A replay is compared by it, and the evaluation
    /// exports it (spec §13).
    pub fn to_json(&self) -> Value {
        json!({
            "scene": self.scene.as_str(),
            "branch": self.branch.as_str(),
            "events": self.events.iter().map(|e| json!({
                "seq": e.seq,
                "id": e.id.as_str(),
                "event": e.body,
                "witnesses": e.witnesses,
            })).collect::<Vec<_>>(),
            "proposals": self.proposals.iter().map(|p| json!({
                "id": p.id.as_str(),
                "round": p.round,
                "character": p.character.as_str(),
                "proposal": p.proposal,
                "verdict": p.verdict,
                "reason": p.reason,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Why a scene could not be played.
#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    /// No canon was imported yet.
    #[error("the story has no canon yet")]
    NoCanon,
    /// The story database refused or failed.
    #[error(transparent)]
    Store(#[from] StoryDbError),
    /// A call gave no usable answer.
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Plays `script` on a new branch forked from the main line's tip, asking
/// `port` for every character's proposal under `run`.
pub async fn play_scripted_scene(
    db: &StoryDb,
    author: &mut AuthorWriter,
    port: &dyn LlmPort,
    run: &RunId,
    script: &SceneScript,
) -> Result<SceneOutcome, SceneError> {
    let canon = db.canon().await?.ok_or(SceneError::NoCanon)?;
    let tip = match db.main_line_tip().await? {
        Some(tip) => tip,
        None => {
            drop(db.genesis().await?);
            BranchId::genesis()
        }
    };
    let scene = author
        .add_scene(
            (script.chapter, script.scene, script.rerun),
            &script.setup,
            &script.intent,
        )
        .await?;
    let branch = BranchId::of_scene(&scene, script.k);
    let mut writer = db.fork(&scene, &tip, script.k, seed_of(&branch)).await?;
    let start = db.world(&branch).await?;
    let opening_tick = start.tick();
    for event in opening_events(&start, &script.setup, opening_tick) {
        writer.commit(event).await?;
    }

    let schema = Proposal::schema(&CanonKeys::of(&canon));
    let stage = Stage {
        db,
        port,
        run,
        canon: &canon,
        schema: &schema,
        here: &script.setup.location,
        branch: &branch,
        opening_tick,
    };
    let mut quiet = 0;
    for round in 1..=script.rounds {
        match play_round(&stage, &mut writer, round).await? {
            Played::Event => quiet = 0,
            Played::Quiet if quiet == 1 => break,
            Played::Quiet => quiet += 1,
            Played::Empty => break,
        }
    }
    drop(writer);

    let mut events = db.path(&branch).await?;
    events.retain(|e| e.branch == branch);
    let proposals = db.proposals(&branch).await?;
    Ok(SceneOutcome {
        scene,
        branch,
        events,
        proposals,
    })
}

/// What stays fixed for a scene's rounds.
struct Stage<'a> {
    db: &'a StoryDb,
    port: &'a dyn LlmPort,
    run: &'a RunId,
    canon: &'a Canon,
    schema: &'a Schema,
    here: &'a LocationKey,
    branch: &'a BranchId,
    opening_tick: u32,
}

/// What a round did.
enum Played {
    /// Its initiative was committed.
    Event,
    /// Nothing was committed.
    Quiet,
    /// No one was left at the scene's place to propose.
    Empty,
}

/// One round: every present character proposes from the same view of the
/// world, the rule rules, the proposals are recorded with their rulings,
/// and the initiative is committed.
async fn play_round(
    stage: &Stage<'_>,
    writer: &mut BranchWriter,
    round: u32,
) -> Result<Played, SceneError> {
    let (canon, branch) = (stage.canon, stage.branch);
    let world = stage.db.world(branch).await?;
    let present: Vec<CharacterKey> = canon
        .characters
        .iter()
        .map(|c| c.key.clone())
        .filter(|who| world.position(who) == Some(stage.here) && world.is_alive(who))
        .collect();
    if present.is_empty() {
        return Ok(Played::Empty);
    }
    let mut prompts = Vec::with_capacity(present.len());
    for who in &present {
        let memory = stage.db.witnessed(branch, who).await?;
        prompts.push(character_prompt(canon, &world, &memory, who, round));
    }
    let asks =
        present.iter().zip(&prompts).map(|(who, prompt)| {
            let call = CallAddress::character(stage.run, branch, round, who, "propose");
            async move {
                decide::<Proposal>(stage.port, &call, Role::Character, stage.schema, prompt).await
            }
        });
    let proposals = try_join_all(asks).await?;

    let tick = stage.opening_tick + round;
    let ruling = rule(canon, &world, stage.here, &present, &proposals, round, tick);
    let mut committed = None;
    for (i, who) in present.iter().enumerate() {
        let (verdict, reason) = ruling.verdicts[i].clone();
        let id = writer
            .record_proposal(round, who, &proposals[i], verdict, reason)
            .await?;
        if verdict == ProposalVerdict::Committed {
            committed = Some(id);
        }
    }
    let Some(mut event) = ruling.initiative else {
        return Ok(Played::Quiet);
    };
    event.proposal = committed;
    writer.commit(event).await?;
    Ok(Played::Event)
}

/// The seed a branch's calls derive from: the first eight bytes of its id's
/// hash, so a replayed branch has the seed it was recorded with.
fn seed_of(branch: &BranchId) -> u64 {
    let hex = sha256_hex(branch.as_str().as_bytes());
    u64::from_str_radix(&hex[..16], 16).expect("a SHA-256 is hex")
}

/// The setup as the branch's opening events: what has just happened, with
/// everyone present put at the scene's place, then the pressure (spec §6,
/// *Circumstances become events*).
fn opening_events(world: &WorldState, setup: &SceneSetup, tick: u32) -> Vec<NewEvent> {
    let opening = |observable: &str, effects: Vec<Effect>| NewEvent {
        round: 0,
        tick,
        location: setup.location.clone(),
        kind: EventKind::Opening,
        actor: None,
        visibility: Visibility::Public,
        observable: observable.to_owned(),
        private: None,
        effects,
        proposal: None,
        cause: Cause::Setup,
    };
    let placed = setup
        .present
        .iter()
        .filter(|who| world.position(who) != Some(&setup.location))
        .map(|who| Effect::Place {
            character: who.clone(),
            to: setup.location.clone(),
        })
        .collect();
    vec![
        opening(&setup.just_happened, placed),
        opening(&setup.pressure, Vec::new()),
    ]
}

/// A round's rulings, one per present character, and the event the
/// initiative commits.
struct Ruling {
    verdicts: Vec<(ProposalVerdict, Option<String>)>,
    initiative: Option<NewEvent>,
}

/// The fixed rule of the module's head.
fn rule(
    canon: &Canon,
    world: &WorldState,
    here: &LocationKey,
    present: &[CharacterKey],
    proposals: &[Proposal],
    round: u32,
    tick: u32,
) -> Ruling {
    let n = present.len();
    let lapsed = || (ProposalVerdict::Lapsed, Some(NOT_TAKEN.to_owned()));
    let mut ruling = Ruling {
        verdicts: vec![lapsed(); n],
        initiative: None,
    };
    let first = (round as usize).saturating_sub(1) % n;
    for i in (0..n).map(|j| (first + j) % n) {
        let Some(event) = scripted_event(canon, here, &present[i], &proposals[i], round, tick)
        else {
            ruling.verdicts[i] = (ProposalVerdict::Lapsed, Some(WAITS.to_owned()));
            continue;
        };
        if ruling.initiative.is_some() {
            continue;
        }
        match world.admit(canon, &event) {
            Ok(_) => {
                ruling.verdicts[i] = (ProposalVerdict::Committed, None);
                ruling.initiative = Some(event);
            }
            Err(refusal) => {
                ruling.verdicts[i] = (ProposalVerdict::Refused, Some(refusal.to_string()));
            }
        }
    }
    ruling
}

/// The event a proposal would commit; none for a wait.
fn scripted_event(
    canon: &Canon,
    here: &LocationKey,
    who: &CharacterKey,
    proposal: &Proposal,
    round: u32,
    tick: u32,
) -> Option<NewEvent> {
    let name = name_of(canon, who);
    let (kind, observable, effects) = match &proposal.action {
        Action::Speak { line } => (
            EventKind::Speech,
            format!("{name} says: \"{line}\""),
            vec![],
        ),
        Action::Move { to } => (
            EventKind::Movement,
            format!("{name} leaves for {}.", place_of(canon, to)),
            vec![Effect::Move {
                character: who.clone(),
                to: to.clone(),
            }],
        ),
        Action::Act { verb, object, line } => {
            let mut text = format!("{name} {verb}");
            if let Some(object) = object {
                text.push(' ');
                text.push_str(object);
            }
            text.push('.');
            if let Some(line) = line {
                text.push_str(&format!(" {name} says: \"{line}\""));
            }
            (EventKind::Action, text, vec![])
        }
        Action::Wait => return None,
    };
    let thought = proposal.private_thought.trim();
    Some(NewEvent {
        round,
        tick,
        location: here.clone(),
        kind,
        actor: Some(who.clone()),
        visibility: proposal.visibility.clone(),
        observable,
        private: (!thought.is_empty()).then(|| thought.to_owned()),
        effects,
        proposal: None,
        cause: Cause::Initiative,
    })
}

fn name_of<'a>(canon: &'a Canon, who: &'a CharacterKey) -> &'a str {
    canon
        .character(who)
        .map_or(who.as_str(), |c| c.name.as_str())
}

fn place_of<'a>(canon: &'a Canon, at: &'a LocationKey) -> &'a str {
    canon.location(at).map_or(at.as_str(), |l| l.name.as_str())
}

fn bullets<'a>(lines: impl IntoIterator<Item = &'a str>) -> String {
    lines
        .into_iter()
        .map(|l| format!("- {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn listed(names: &[&str], none: &str) -> String {
    if names.is_empty() {
        none.to_owned()
    } else {
        names.join(", ")
    }
}

/// A character's prompt for a round: the stable part (the world, the map,
/// its own sheet, the others as everyone knows them), then the moment (where
/// it is, who is with it, what it carries, what it has perceived).
pub fn character_prompt(
    canon: &Canon,
    world: &WorldState,
    memory: &[Event],
    who: &CharacterKey,
    round: u32,
) -> Prompt {
    let me = canon.character(who);
    let name = name_of(canon, who);
    let places = canon
        .locations
        .iter()
        .map(|l| format!("{} (`{}`): {}", l.name, l.key, l.description))
        .collect::<Vec<_>>();
    let others = canon
        .characters
        .iter()
        .filter(|c| &c.key != who)
        .map(|c| format!("{} (`{}`): {}", c.name, c.key, c.public))
        .collect::<Vec<_>>();
    let mut system = format!(
        "You play {name} in a story. Each round you decide what {name} does next, in \
         character; the world decides what comes of it.\n\n\
         The world's rules:\n{}\n\nThe places:\n{}\n\n",
        bullets(canon.world.rules.iter().map(String::as_str)),
        bullets(places.iter().map(String::as_str)),
    );
    if let Some(me) = me {
        system.push_str(&format!(
            "You are {name} (`{who}`). {}\nYour voice: {}\nYour goals:\n{}\n\
             Your secrets, which no one else knows unless you reveal them:\n{}\n\n",
            me.public,
            me.voice,
            bullets(me.goals.iter().map(String::as_str)),
            bullets(me.secrets.iter().map(|s| s.text.as_str())),
        ));
    }
    system.push_str(&format!(
        "The others, as everyone knows them:\n{}",
        bullets(others.iter().map(String::as_str))
    ));

    let here = world.position(who);
    let around = here.map(|at| world.present_at(at)).unwrap_or_default();
    let with: Vec<&str> = canon
        .characters
        .iter()
        .filter(|c| &c.key != who && around.contains(&c.key))
        .map(|c| c.name.as_str())
        .collect();
    let held = |holder: Holder| -> Vec<&str> {
        canon
            .starting_items()
            .filter(|(item, _)| world.holder(item) == Some(&holder))
            .map(|(item, _)| item.as_str())
            .collect()
    };
    let carried = held(Holder::Character(who.clone()));
    let lying = here
        .map(|at| held(Holder::Location(at.clone())))
        .unwrap_or_default();
    let mut perceived = Vec::new();
    for event in memory {
        perceived.push(format!("- {}", event.body.observable));
        if event.body.actor.as_ref() == Some(who)
            && let Some(thought) = &event.body.private
        {
            perceived.push(format!("  (You thought: {thought})"));
        }
    }
    let user = format!(
        "Round {round}. You are at {}, with {}. You carry: {}. Lying here: {}.\n\n\
         What you have perceived, oldest first:\n{}\n\n\
         What does {name} do now? Write the inner thought first, then the action.",
        here.map_or("an unknown place", |at| place_of(canon, at)),
        listed(&with, "no one else"),
        listed(&carried, "nothing"),
        listed(&lying, "nothing"),
        if perceived.is_empty() {
            "- nothing yet".to_owned()
        } else {
            perceived.join("\n")
        },
    );
    Prompt { system, user }
}

/// The scene of the harbour fixture, with all three characters on the quay.
#[cfg(test)]
pub(crate) fn harbour_script() -> SceneScript {
    SceneScript {
        chapter: 1,
        scene: 1,
        rerun: 0,
        k: 0,
        setup: SceneSetup {
            location: "harbour".into(),
            present: vec!["mara".into(), "silas".into(), "ines".into()],
            just_happened: "The last ferry has been cancelled and the storm has shut the \
                harbour for the night. Silas came down from the inn to ask after his crates; \
                Ines came down from the lighthouse to say the lamp's oil will not last till \
                dawn."
                .to_owned(),
            pressure: "The harbour gate closes at sunset, a quarter of an hour from now, and \
                the customs ledger lies open on the harbour master's table."
                .to_owned(),
        },
        intent: "Ines begins to suspect that the ledger is false.".to_owned(),
        rounds: 3,
    }
}

// Its story and fixture helpers serve the adapters' tests and the live smoke.
#[cfg(test)]
#[path = "scene_tests.rs"]
pub(crate) mod tests;
