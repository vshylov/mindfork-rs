//! The simulation (docs/research/fabula-engine.md §7, step 3 and *One
//! simulation round*; docs/fabula.md §4, PR 4): a scene simulated as K
//! branches from the same world snapshot, each played in rounds where the
//! characters propose and the World Master rules.
//!
//! One round:
//!
//! 1. **Intent.** Every character present proposes from the same round-start
//!    view; with probability `abnormal_p` one is told to break their routine.
//! 2. **Initiative.** The World Master rules on the intents and picks one; the
//!    engine checks the event it writes against the folded world and commits
//!    it, the other feasible intents lapse, and any world event follows.
//! 3. **Reaction.** The initiative's witnesses who are still here may respond;
//!    the World Master rules on them in the order they happen, and each one is
//!    checked against the world the one before it left.
//! 4. **Reflection.** Every `reflect_every` rounds, each character present
//!    writes a private reflection and its goal changes.
//!
//! A scene ends at `max_rounds`, after two rounds running in which no
//! character's deed was committed, when no one is left at its place, or when
//! the World Master judges its pressure resolved.
//!
//! **The World Master proposes, the engine commits.** Every event it writes is
//! held to the world's rules ([`WorldState::admit`]) and to what the witnesses
//! know ([`leaks::unknown_secrets`]) before it is committed; a refusal there is
//! the World Master's failure, recorded with the proposal and counted per scene
//! (Gate G1), never repaired. Where its ruling does not answer the proposals it
//! was given — a character it was not asked about, an initiator whose intent
//! was not feasible — the engine records a slip and goes on without it.
//!
//! **Branches run one after another** (docs/fabula.md §4, under `sessions =
//! 1`); a branch whose call failed is reported failed and does not stop its
//! siblings. A cancelled run stops at once.

use std::collections::{BTreeMap, BTreeSet};

use futures_util::future::try_join_all;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::contracts::{
    InitiativeRuling, IntentVerdict, ReactionVerdict, ReactionsRuling, ReflectionReply,
};
use super::leaks;
use super::port::{CallAddress, LlmPort, PortError, Prompt, Role, decide};
use super::prompts::{
    self, CharacterView, MasterView, Shown, character_propose, character_react, character_reflect,
    master_initiative, master_reactions,
};
use super::scene::{NOT_TAKEN, SceneError, WAITS, opening_events, scripted_event, seed_of};
use super::schema::{CanonKeys, Schema, StorySchema};
use crate::entities::chat_file::sha256_hex;
use crate::entities::story::{
    Action, BranchId, Canon, Cause, CharacterKey, Effect, Event, EventKind, LocationKey, NewEvent,
    Proposal, ProposalVerdict, RunId, SceneId, SceneSetup, Visibility, WorldState,
};
use crate::shared::storage::story::{
    AuthorWriter, BranchWriter, Reflection, ReflectionWriter, StoredProposal, StoryDb,
};

/// Why a lapsed proposal lapsed: the World Master's ruling left it out.
pub const NOT_RULED: &str = "the World Master did not rule on it";

/// How a scene is simulated; not the Author's to choose (spec §6, `RunConfig`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimConfig {
    /// K, the branches per scene.
    pub branches: u32,
    /// The most rounds a branch runs.
    pub max_rounds: u32,
    /// The chance a character breaks their routine in a round (StoryBox's
    /// abnormal-behaviour factor).
    pub abnormal_p: f64,
    /// Every how many rounds the characters reflect; 0 for never.
    pub reflect_every: u32,
}

impl Default for SimConfig {
    /// The spec's values (§13, *Configuration sketch*).
    fn default() -> Self {
        Self {
            branches: 3,
            max_rounds: 8,
            abnormal_p: 0.3,
            reflect_every: 3,
        }
    }
}

/// A scene the Author planned: where it sits, its circumstances, and the
/// Author's private intent, which reaches neither the characters nor the
/// World Master.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenePlan {
    /// The chapter, from 1.
    pub chapter: u32,
    /// The scene within it, from 1.
    pub scene: u32,
    /// The rerun, from 0.
    pub rerun: u32,
    /// The given circumstances.
    pub setup: SceneSetup,
    /// The Author's intent.
    pub intent: String,
}

/// Why a branch stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It ran its rounds.
    Rounds,
    /// Two rounds running committed no character's deed.
    Stalemate,
    /// The World Master judged the pressure resolved.
    Resolved,
    /// No one was left at the scene's place.
    Empty,
}

impl Ending {
    /// The name the trace records.
    pub fn as_str(self) -> &'static str {
        match self {
            Ending::Rounds => "rounds",
            Ending::Stalemate => "stalemate",
            Ending::Resolved => "resolved",
            Ending::Empty => "empty",
        }
    }
}

/// What a branch's rounds came to, for the scene's report (Gate G1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    /// Intents proposed, waits included.
    pub intents: u32,
    /// Reactions proposed, waits included.
    pub reactions: u32,
    /// Proposals that were waits.
    pub waits: u32,
    /// Characters' deeds committed.
    pub committed: u32,
    /// World events committed.
    pub world_events: u32,
    /// Proposals the World Master refused.
    pub refused_by_ruling: u32,
    /// Events the World Master wrote that the engine refused, with the reason.
    pub refused_by_engine: Vec<String>,
    /// Where the World Master's ruling did not answer what it was asked.
    pub slips: Vec<String>,
    /// Reflections written.
    pub reflections: u32,
}

/// A branch played to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRun {
    /// The branch.
    pub branch: BranchId,
    /// The rounds played.
    pub rounds: u32,
    /// Why it stopped.
    pub ending: Ending,
    /// The events it committed, in order.
    pub events: Vec<Event>,
    /// The proposals with their rulings, in the order recorded.
    pub proposals: Vec<StoredProposal>,
    /// Each character's reflections, in the canon's order.
    pub reflections: Vec<(CharacterKey, Reflection)>,
    /// What the rounds came to.
    pub tally: Tally,
}

/// A branch, played or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchResult {
    /// It played to its end.
    Played(BranchRun),
    /// A call gave no usable answer; the branch stops where it was.
    Failed {
        /// The branch.
        branch: BranchId,
        /// What failed.
        error: String,
    },
}

impl BranchResult {
    /// The branch.
    pub fn branch(&self) -> &BranchId {
        match self {
            BranchResult::Played(run) => &run.branch,
            BranchResult::Failed { branch, .. } => branch,
        }
    }
}

/// A scene's K branches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneRun {
    /// The scene.
    pub scene: SceneId,
    /// Its branches, in order.
    pub branches: Vec<BranchResult>,
}

impl SceneRun {
    /// The scene's trace: each branch's events with their witnesses, its
    /// proposals with their rulings, its reflections and its tally. What a
    /// replay is compared by, and what the evaluation exports (spec §13).
    pub fn to_json(&self) -> Value {
        json!({
            "scene": self.scene.as_str(),
            "branches": self.branches.iter().map(branch_json).collect::<Vec<_>>(),
        })
    }
}

fn branch_json(result: &BranchResult) -> Value {
    let run = match result {
        BranchResult::Played(run) => run,
        BranchResult::Failed { branch, error } => {
            return json!({"branch": branch.as_str(), "failed": error});
        }
    };
    let t = &run.tally;
    json!({
        "branch": run.branch.as_str(),
        "rounds": run.rounds,
        "ending": run.ending.as_str(),
        "events": run.events.iter().map(|e| json!({
            "seq": e.seq,
            "id": e.id.as_str(),
            "event": e.body,
            "witnesses": e.witnesses,
        })).collect::<Vec<_>>(),
        "proposals": run.proposals.iter().map(|p| json!({
            "id": p.id.as_str(),
            "round": p.round,
            "character": p.character.as_str(),
            "proposal": p.proposal,
            "verdict": p.verdict,
            "reason": p.reason,
        })).collect::<Vec<_>>(),
        "reflections": run.reflections.iter().map(|(who, r)| json!({
            "character": who.as_str(),
            "after_seq": r.after_seq,
            "reflection": r.body,
            "goal_changes": r.goal_changes,
        })).collect::<Vec<_>>(),
        "tally": {
            "intents": t.intents,
            "reactions": t.reactions,
            "waits": t.waits,
            "committed": t.committed,
            "world_events": t.world_events,
            "refused_by_ruling": t.refused_by_ruling,
            "refused_by_engine": t.refused_by_engine,
            "slips": t.slips,
            "reflections": t.reflections,
        },
    })
}

/// The schemas of a story's replies, derived once: they change only with the
/// canon (spec §8, *Schemas are stable per story*).
struct Schemas {
    proposal: Schema,
    reflection: Schema,
    initiative: Schema,
    reactions: Schema,
}

impl Schemas {
    fn of(canon: &Canon) -> Self {
        let keys = CanonKeys::of(canon);
        Self {
            proposal: Proposal::schema(&keys),
            reflection: ReflectionReply::schema(&keys),
            initiative: InitiativeRuling::schema(&keys),
            reactions: ReactionsRuling::schema(&keys),
        }
    }
}

/// Simulates `plan` as `config.branches` branches forked one after another
/// from the main line's tip, every call through `port` under `run`.
pub async fn simulate_scene(
    db: &StoryDb,
    author: &mut AuthorWriter,
    port: &dyn LlmPort,
    run: &RunId,
    plan: &ScenePlan,
    config: &SimConfig,
) -> Result<SceneRun, SceneError> {
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
            (plan.chapter, plan.scene, plan.rerun),
            &plan.setup,
            &plan.intent,
        )
        .await?;
    let schemas = Schemas::of(&canon);
    let stage = Stage {
        db,
        port,
        run,
        canon: &canon,
        setup: &plan.setup,
        config,
        schemas: &schemas,
    };
    let mut branches = Vec::with_capacity(config.branches as usize);
    for k in 0..config.branches {
        let branch = BranchId::of_scene(&scene, k);
        let result = match stage.play_branch(&scene, &tip, k).await {
            Ok(played) => BranchResult::Played(played),
            Err(SceneError::Port(e))
                if !matches!(e, PortError::Cancelled { .. } | PortError::Store(_)) =>
            {
                tracing::warn!(%branch, error = %e, "a story branch failed");
                BranchResult::Failed {
                    branch,
                    error: e.to_string(),
                }
            }
            Err(e) => return Err(e),
        };
        branches.push(result);
    }
    Ok(SceneRun { scene, branches })
}

/// What stays fixed for a scene's branches.
struct Stage<'a> {
    db: &'a StoryDb,
    port: &'a dyn LlmPort,
    run: &'a RunId,
    canon: &'a Canon,
    setup: &'a SceneSetup,
    config: &'a SimConfig,
    schemas: &'a Schemas,
}

/// One branch while it plays.
struct Playing<'a> {
    stage: &'a Stage<'a>,
    branch: BranchId,
    seed: u64,
    writer: BranchWriter,
    reflectors: BTreeMap<CharacterKey, ReflectionWriter>,
    opening_tick: u32,
    tally: Tally,
}

/// What a round did.
struct Played {
    /// Characters' deeds committed.
    deeds: u32,
    /// The World Master judged the pressure resolved.
    over: bool,
}

impl Stage<'_> {
    async fn play_branch(
        &self,
        scene: &SceneId,
        tip: &BranchId,
        k: u32,
    ) -> Result<BranchRun, SceneError> {
        let branch = BranchId::of_scene(scene, k);
        let seed = seed_of(&branch);
        let mut writer = self.db.fork(scene, tip, k, seed).await?;
        let start = self.db.world(&branch).await?;
        let opening_tick = start.tick();
        for event in opening_events(&start, self.setup, opening_tick) {
            writer.commit(event).await?;
        }
        let mut playing = Playing {
            stage: self,
            branch: branch.clone(),
            seed,
            writer,
            reflectors: BTreeMap::new(),
            opening_tick,
            tally: Tally::default(),
        };
        let (mut rounds, mut ending, mut quiet) = (0, Ending::Rounds, 0);
        for round in 1..=self.config.max_rounds {
            let Some(played) = playing.round(round).await? else {
                ending = Ending::Empty;
                break;
            };
            rounds = round;
            quiet = if played.deeds == 0 { quiet + 1 } else { 0 };
            if played.over {
                ending = Ending::Resolved;
                break;
            }
            if quiet == 2 {
                ending = Ending::Stalemate;
                break;
            }
        }
        let tally = std::mem::take(&mut playing.tally);
        drop(playing);
        let mut events = self.db.path(&branch).await?;
        events.retain(|e| e.branch == branch);
        let proposals = self.db.proposals(&branch).await?;
        let mut reflections = Vec::new();
        for character in &self.canon.characters {
            for r in self.db.reflections(&branch, &character.key).await? {
                reflections.push((character.key.clone(), r));
            }
        }
        Ok(BranchRun {
            branch,
            rounds,
            ending,
            events,
            proposals,
            reflections,
            tally,
        })
    }
}

/// Whether `who` breaks their routine in `round`: a draw from the branch's
/// seed, so a replayed branch draws the same (spec §9).
pub fn breaks_routine(seed: u64, round: u32, who: &CharacterKey, p: f64) -> bool {
    let hex = sha256_hex(format!("{seed}/{round}/{who}").as_bytes());
    let draw = u64::from_str_radix(&hex[..16], 16).expect("a SHA-256 is hex");
    (draw as f64 / u64::MAX as f64) < p
}

/// The event a proposal makes as the World Master wrote it: the deed's kind,
/// the proposal's visibility and thought, the ruling's observable and effects.
/// A move the ruling forgot to move is given its move; an empty observable is
/// the deed as M0's rule would have told it.
fn deed_event(
    canon: &Canon,
    here: &LocationKey,
    who: &CharacterKey,
    proposal: &Proposal,
    written: (&str, &[Effect]),
    (round, tick, cause): (u32, u32, Cause),
) -> Option<NewEvent> {
    let mut event = scripted_event(canon, here, who, proposal, round, tick)?;
    let (observable, effects) = written;
    if !observable.trim().is_empty() {
        event.observable = observable.trim().to_owned();
    }
    let mut effects = effects.to_vec();
    if let Action::Move { to } = &proposal.action
        && !effects
            .iter()
            .any(|e| matches!(e, Effect::Move { character, .. } if character == who))
    {
        effects.insert(
            0,
            Effect::Move {
                character: who.clone(),
                to: to.clone(),
            },
        );
    }
    event.effects = effects;
    event.cause = cause;
    Some(event)
}

/// Holds an event to the world's rules and to what its witnesses know: on
/// success, the world it leaves.
fn check(canon: &Canon, world: &WorldState, event: &NewEvent) -> Result<WorldState, String> {
    let admitted = world
        .admit(canon, event)
        .map_err(|refusal| format!("the world refuses it: {refusal}"))?;
    for witness in &admitted.witnesses {
        if let Some(secret) =
            leaks::unknown_secrets(canon, &admitted.state, witness, &event.observable).first()
        {
            return Err(format!(
                "what is perceived gives `{secret}` away to {witness}, who does not know it; \
                 a reveal effect must say who learns it"
            ));
        }
    }
    if let (Some(actor), Some(private)) = (&event.actor, &event.private)
        && let Some(secret) = leaks::unknown_secrets(canon, world, actor, private).first()
    {
        return Err(format!(
            "{actor} thinks of `{secret}`, which they do not know"
        ));
    }
    Ok(admitted.state)
}

/// A round's intents as the World Master's ruling leaves them, before the
/// initiative's event is checked: a verdict for each character present, and
/// the initiator among them when the ruling named one whose intent it found
/// feasible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntentsRead {
    /// One per character present, in their order.
    pub verdicts: Vec<(ProposalVerdict, Option<String>)>,
    /// The initiator's place among the present.
    pub initiator: Option<usize>,
    /// Intents the World Master refused.
    pub refused: u32,
    /// Where the ruling did not answer what it was asked.
    pub slips: Vec<String>,
}

/// Reads the World Master's ruling on the intents of `present`.
pub(crate) fn read_intents(
    present: &[CharacterKey],
    intents: &[Proposal],
    ruling: &InitiativeRuling,
) -> IntentsRead {
    let acting = |i: &usize| intents[*i].action != Action::Wait;
    let mut read = IntentsRead {
        verdicts: (0..present.len())
            .map(|i| {
                let reason = if acting(&i) { NOT_RULED } else { WAITS };
                (ProposalVerdict::Lapsed, Some(reason.to_owned()))
            })
            .collect(),
        initiator: None,
        refused: 0,
        slips: Vec::new(),
    };
    let mut ruled = BTreeSet::new();
    for r in &ruling.rulings {
        let Some(i) = (0..present.len()).find(|i| present[*i] == r.character && acting(i)) else {
            read.slips
                .push(format!("ruled on `{}`, who proposed no deed", r.character));
            continue;
        };
        if !ruled.insert(i) {
            read.slips.push(format!("ruled twice on `{}`", r.character));
            continue;
        }
        read.verdicts[i] = match r.verdict {
            IntentVerdict::Feasible => (ProposalVerdict::Lapsed, Some(NOT_TAKEN.to_owned())),
            IntentVerdict::Refused => {
                read.refused += 1;
                (ProposalVerdict::Refused, Some(r.cause.trim().to_owned()))
            }
        };
    }
    for i in (0..present.len()).filter(|i| acting(i) && !ruled.contains(i)) {
        read.slips.push(format!("did not rule on `{}`", present[i]));
    }
    let feasible = |i: usize| read.verdicts[i].1.as_deref() == Some(NOT_TAKEN);
    match &ruling.initiator {
        None if (0..present.len()).any(feasible) => read
            .slips
            .push("named no initiator though an intent was feasible".to_owned()),
        None => {}
        Some(who) => match (0..present.len()).find(|&i| &present[i] == who && feasible(i)) {
            Some(i) => read.initiator = Some(i),
            None => read.slips.push(format!(
                "named `{who}` as the initiator, whose intent was not feasible"
            )),
        },
    }
    read
}

/// One reaction as the World Master's ruling orders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step<'r> {
    /// Refused, for this cause.
    Refuse {
        /// The reactor's place.
        i: usize,
        /// Why.
        cause: &'r str,
    },
    /// To be committed as written.
    Commit {
        /// The reactor's place.
        i: usize,
        /// What is perceived.
        observable: &'r str,
        /// What it changes.
        effects: &'r [Effect],
    },
}

/// A round's reactions as the World Master's ruling leaves them: the ones it
/// ruled on, in its order, and the ones that lapse with their reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReactionsRead<'r> {
    /// In the order they happen.
    pub steps: Vec<Step<'r>>,
    /// Waits, and reactions the ruling left out.
    pub lapsed: Vec<(usize, &'static str)>,
    /// Where the ruling did not answer what it was asked.
    pub slips: Vec<String>,
}

/// Reads the World Master's ruling on the reactions of `reactors`.
pub(crate) fn read_reactions<'r>(
    reactors: &[CharacterKey],
    reactions: &[Proposal],
    ruling: &'r ReactionsRuling,
) -> ReactionsRead<'r> {
    let acting = |i: &usize| reactions[*i].action != Action::Wait;
    let mut read = ReactionsRead {
        steps: Vec::new(),
        lapsed: Vec::new(),
        slips: Vec::new(),
    };
    let mut ruled = BTreeSet::new();
    for r in &ruling.rulings {
        let who = &r.character;
        let Some(i) = (0..reactors.len()).find(|i| &reactors[*i] == who && acting(i)) else {
            read.slips
                .push(format!("ruled on `{who}`, who proposed no reaction"));
            continue;
        };
        if !ruled.insert(i) {
            read.slips
                .push(format!("ruled twice on `{who}`'s reaction"));
            continue;
        }
        read.steps.push(match r.verdict {
            ReactionVerdict::Refuse => Step::Refuse {
                i,
                cause: r.cause.trim(),
            },
            ReactionVerdict::Commit => Step::Commit {
                i,
                observable: &r.observable,
                effects: &r.effects,
            },
        });
    }
    for i in (0..reactors.len()).filter(|i| !ruled.contains(i)) {
        if acting(&i) {
            read.slips
                .push(format!("did not rule on `{}`'s reaction", reactors[i]));
            read.lapsed.push((i, NOT_RULED));
        } else {
            read.lapsed.push((i, WAITS));
        }
    }
    read
}

impl Playing<'_> {
    /// One round; none when no one is left at the scene's place.
    async fn round(&mut self, round: u32) -> Result<Option<Played>, SceneError> {
        let stage = self.stage;
        let world = stage.db.world(&self.branch).await?;
        let present = prompts::present_here(stage.canon, &world, &stage.setup.location);
        if present.is_empty() {
            return Ok(None);
        }
        let intents = self.ask_characters(&world, &present, round, None).await?;
        self.tally.intents += count(intents.len());
        let (initiative, over) = self.initiative(&world, round, &present, &intents).await?;
        let mut deeds = u32::from(initiative.is_some());
        if let Some(initiative) = initiative {
            deeds += self.reactions(round, &initiative).await?;
        }
        if stage.config.reflect_every > 0 && round.is_multiple_of(stage.config.reflect_every) {
            self.reflect(round).await?;
        }
        Ok(Some(Played { deeds, over }))
    }

    /// Asks each of `who` for a proposal, all at once from the same view: an
    /// intent, or with `initiative` a reaction to it.
    async fn ask_characters(
        &self,
        world: &WorldState,
        who: &[CharacterKey],
        round: u32,
        initiative: Option<&Event>,
    ) -> Result<Vec<Proposal>, SceneError> {
        let stage = self.stage;
        let mut prompts = Vec::with_capacity(who.len());
        for character in who {
            let prompt = self
                .view_prompt(world, character, round, |view| match initiative {
                    Some(event) => character_react(view, event),
                    None => character_propose(view),
                })
                .await?;
            prompts.push(prompt);
        }
        let act = if initiative.is_some() {
            "react"
        } else {
            "propose"
        };
        let asks = who.iter().zip(&prompts).map(|(character, prompt)| {
            let call = CallAddress::character(stage.run, &self.branch, round, character, act);
            async move {
                decide::<Proposal>(
                    stage.port,
                    &call,
                    Role::Character,
                    &stage.schemas.proposal,
                    prompt,
                )
                .await
            }
        });
        Ok(try_join_all(asks).await?)
    }

    /// The prompt `build` makes from what `who` perceived and reflected.
    async fn view_prompt(
        &self,
        world: &WorldState,
        who: &CharacterKey,
        round: u32,
        build: impl FnOnce(&CharacterView) -> Prompt,
    ) -> Result<Prompt, SceneError> {
        let stage = self.stage;
        let memory = stage.db.witnessed(&self.branch, who).await?;
        let reflections = stage.db.reflections(&self.branch, who).await?;
        let view = CharacterView {
            canon: stage.canon,
            world,
            who,
            round,
            memory: &memory,
            reflections: &reflections,
            breaks_routine: breaks_routine(self.seed, round, who, stage.config.abnormal_p),
        };
        Ok(build(&view))
    }

    /// What the World Master is shown of the deeds among `proposals`: each
    /// one's proposer, deed and the labels of the events they perceived.
    async fn show<'p>(
        &self,
        who: &'p [CharacterKey],
        proposals: &'p [Proposal],
    ) -> Result<Vec<Shown<'p>>, SceneError> {
        let mut shown = Vec::new();
        for (who, proposal) in who.iter().zip(proposals) {
            if proposal.action == Action::Wait {
                continue;
            }
            let perceived = self
                .stage
                .db
                .witnessed(&self.branch, who)
                .await?
                .iter()
                .map(|e| prompts::label(e, &self.branch))
                .collect();
            shown.push(Shown {
                who,
                proposal,
                perceived,
            });
        }
        Ok(shown)
    }

    /// A World Master's ruling on the prompt `build` makes from the moment.
    async fn ask_master<T: DeserializeOwned>(
        &self,
        world: &WorldState,
        round: u32,
        act: &str,
        schema: &Schema,
        build: impl FnOnce(&MasterView) -> Prompt,
    ) -> Result<T, SceneError> {
        let stage = self.stage;
        let path = stage.db.path(&self.branch).await?;
        let view = MasterView {
            canon: stage.canon,
            setup: stage.setup,
            world,
            branch: &self.branch,
            path: &path,
            round,
        };
        let call = CallAddress::world_master(stage.run, &self.branch, round, act);
        Ok(decide(stage.port, &call, Role::WorldMaster, schema, &build(&view)).await?)
    }

    /// Step 2 of the round: the World Master's ruling on the intents, every
    /// intent recorded with its verdict, the initiative and any world event
    /// committed. The initiative as committed, and whether the scene is over.
    async fn initiative(
        &mut self,
        world: &WorldState,
        round: u32,
        present: &[CharacterKey],
        intents: &[Proposal],
    ) -> Result<(Option<Event>, bool), SceneError> {
        let stage = self.stage;
        let shown = self.show(present, intents).await?;
        let ruling: InitiativeRuling = self
            .ask_master(
                world,
                round,
                "initiative",
                &stage.schemas.initiative,
                |view| master_initiative(view, &shown),
            )
            .await?;
        let read = read_intents(present, intents, &ruling);
        self.tally.refused_by_ruling += read.refused;
        for slip in read.slips {
            self.slip(round, slip);
        }
        let mut verdicts = read.verdicts;
        let tick = self.opening_tick + round;
        let mut after = world.clone();
        let mut event = None;
        if let Some(i) = read.initiator {
            let deed = deed_event(
                stage.canon,
                &stage.setup.location,
                &present[i],
                &intents[i],
                (&ruling.observable, &ruling.effects),
                (round, tick, Cause::Initiative),
            )
            .expect("a deed is not a wait");
            match check(stage.canon, world, &deed) {
                Ok(state) => {
                    after = state;
                    verdicts[i] = (ProposalVerdict::Committed, None);
                    event = Some(deed);
                }
                Err(reason) => {
                    self.refused_by_engine(round, &present[i], &reason);
                    verdicts[i] = (ProposalVerdict::Refused, Some(reason));
                }
            }
        }
        let mut committed_id = None;
        for (i, who) in present.iter().enumerate() {
            let (verdict, reason) = verdicts[i].clone();
            if intents[i].action == Action::Wait {
                self.tally.waits += 1;
            }
            let id = self
                .writer
                .record_proposal(round, who, &intents[i], verdict, reason)
                .await?;
            if verdict == ProposalVerdict::Committed {
                committed_id = Some(id);
            }
        }
        let mut committed = None;
        if let Some(mut event) = event {
            event.proposal = committed_id;
            committed = Some(self.commit(event).await?);
            self.tally.committed += 1;
        }
        if let Some(text) = ruling.world_event.as_deref().map(str::trim)
            && !text.is_empty()
        {
            self.world_event(&after, round, tick, text).await?;
        }
        Ok((committed, ruling.scene_over))
    }

    /// Commits a world event the engine admits.
    async fn world_event(
        &mut self,
        world: &WorldState,
        round: u32,
        tick: u32,
        text: &str,
    ) -> Result<(), SceneError> {
        let event = NewEvent {
            round,
            tick,
            location: self.stage.setup.location.clone(),
            kind: EventKind::WorldEvent,
            actor: None,
            visibility: Visibility::Public,
            observable: text.to_owned(),
            private: None,
            effects: Vec::new(),
            proposal: None,
            cause: Cause::World,
        };
        match check(self.stage.canon, world, &event) {
            Ok(_) => {
                self.commit(event).await?;
                self.tally.world_events += 1;
            }
            Err(reason) => self
                .tally
                .refused_by_engine
                .push(format!("round {round}, the world event: {reason}")),
        }
        Ok(())
    }

    /// Commits an event and returns it as stored.
    async fn commit(&mut self, event: NewEvent) -> Result<Event, SceneError> {
        let done = self.writer.commit(event.clone()).await?;
        Ok(Event {
            seq: done.seq,
            id: done.id,
            branch: self.branch.clone(),
            body: event,
            witnesses: done.witnesses,
        })
    }

    fn slip(&mut self, round: u32, what: String) {
        self.tally.slips.push(format!("round {round}: {what}"));
    }

    fn refused_by_engine(&mut self, round: u32, who: &CharacterKey, reason: &str) {
        self.tally
            .refused_by_engine
            .push(format!("round {round}, `{who}`: {reason}"));
    }

    /// Step 3 of the round: the initiative's witnesses who are still here
    /// react, and the World Master rules on the reactions in their order, each
    /// held to the world the one before it left. The deeds committed.
    async fn reactions(&mut self, round: u32, initiative: &Event) -> Result<u32, SceneError> {
        let stage = self.stage;
        let world = stage.db.world(&self.branch).await?;
        let reactors: Vec<CharacterKey> =
            prompts::present_here(stage.canon, &world, &stage.setup.location)
                .into_iter()
                .filter(|c| {
                    initiative.witnesses.contains(c) && Some(c) != initiative.body.actor.as_ref()
                })
                .collect();
        if reactors.is_empty() {
            return Ok(0);
        }
        let reactions = self
            .ask_characters(&world, &reactors, round, Some(initiative))
            .await?;
        self.tally.reactions += count(reactions.len());
        let shown = self.show(&reactors, &reactions).await?;
        let ruling = if shown.is_empty() {
            ReactionsRuling {
                rulings: Vec::new(),
            }
        } else {
            self.ask_master(
                &world,
                round,
                "reactions",
                &stage.schemas.reactions,
                |view| master_reactions(view, initiative, &shown),
            )
            .await?
        };
        let read = read_reactions(&reactors, &reactions, &ruling);
        for slip in read.slips {
            self.slip(round, slip);
        }
        let tick = self.opening_tick + round;
        let mut world = world;
        let mut deeds = 0;
        for step in &read.steps {
            let (i, verdict, reason, event) = match *step {
                Step::Refuse { i, cause } => {
                    self.tally.refused_by_ruling += 1;
                    (i, ProposalVerdict::Refused, Some(cause.to_owned()), None)
                }
                Step::Commit {
                    i,
                    observable,
                    effects,
                } => {
                    let cause = Cause::Reaction {
                        to: initiative.id.clone(),
                    };
                    let deed = deed_event(
                        stage.canon,
                        &stage.setup.location,
                        &reactors[i],
                        &reactions[i],
                        (observable, effects),
                        (round, tick, cause),
                    )
                    .expect("a deed is not a wait");
                    match check(stage.canon, &world, &deed) {
                        Ok(state) => {
                            world = state;
                            (i, ProposalVerdict::Committed, None, Some(deed))
                        }
                        Err(reason) => {
                            self.refused_by_engine(round, &reactors[i], &reason);
                            (i, ProposalVerdict::Refused, Some(reason), None)
                        }
                    }
                }
            };
            let id = self
                .writer
                .record_proposal(round, &reactors[i], &reactions[i], verdict, reason)
                .await?;
            if let Some(mut event) = event {
                event.proposal = Some(id);
                self.commit(event).await?;
                self.tally.committed += 1;
                deeds += 1;
            }
        }
        for &(i, reason) in &read.lapsed {
            if reason == WAITS {
                self.tally.waits += 1;
            }
            self.writer
                .record_proposal(
                    round,
                    &reactors[i],
                    &reactions[i],
                    ProposalVerdict::Lapsed,
                    Some(reason.to_owned()),
                )
                .await?;
        }
        Ok(deeds)
    }

    /// Step 4 of the round: each character present reflects, privately.
    async fn reflect(&mut self, round: u32) -> Result<(), SceneError> {
        let stage = self.stage;
        let world = stage.db.world(&self.branch).await?;
        let present = prompts::present_here(stage.canon, &world, &stage.setup.location);
        let mut prompts = Vec::with_capacity(present.len());
        for who in &present {
            prompts.push(
                self.view_prompt(&world, who, round, character_reflect)
                    .await?,
            );
        }
        let asks = present.iter().zip(&prompts).map(|(who, prompt)| {
            let call = CallAddress::character(stage.run, &self.branch, round, who, "reflect");
            async move {
                decide::<ReflectionReply>(
                    stage.port,
                    &call,
                    Role::Character,
                    &stage.schemas.reflection,
                    prompt,
                )
                .await
            }
        });
        let replies = try_join_all(asks).await?;
        for (who, reply) in present.iter().zip(replies) {
            if !self.reflectors.contains_key(who) {
                let writer = stage.db.reflection_writer(&self.branch, who).await?;
                self.reflectors.insert(who.clone(), writer);
            }
            let goals: Vec<String> = reply
                .goal_changes
                .iter()
                .map(|g| g.trim().to_owned())
                .filter(|g| !g.is_empty())
                .collect();
            let writer = self.reflectors.get_mut(who).expect("inserted above");
            writer.write(reply.reflection.trim(), &goals).await?;
            self.tally.reflections += 1;
        }
        Ok(())
    }
}

/// A count of calls, which fits a `u32` in any scene.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[path = "simulation_tests.rs"]
pub(crate) mod tests;
