//! The replies of the simulation's roles beyond a character's proposal
//! (docs/research/fabula-engine.md §7, *One simulation round*; §8, *Agent
//! contracts*): a character's reflection, and the World Master's two rulings
//! of a round — on the intents, where it picks the initiative, and on the
//! reactions to it.
//!
//! - **The deciding fields come last.** Each ruling states its in-world cause
//!   before its verdict, and the World Master's rulings come before the event
//!   it writes from them (spec §8, *Property order is generation order*).
//! - **Story content, not reasoning.** No field asks for the model's own
//!   reasoning, which Claude 5.x refuses as `reasoning_extraction`: a cause is
//!   the narrator's, a reflection the character's inner voice (spec §8).
//! - **The World Master writes what is perceived, not what is thought.** It
//!   is never shown a character's private thought: the event's private part is
//!   the proposal's own, copied by the engine, so a thought cannot slip into
//!   what the witnesses perceive through the World Master's words.
//! - **A shape is not a sense.** The engine checks every ruling against the
//!   proposals it answers and every event against the folded world before it
//!   is committed ([`super::simulation`]).

use serde::{Deserialize, Serialize};

use super::schema::{CanonKeys, Property, Schema, StorySchema};
use crate::entities::story::{CharacterKey, Effect};

/// A character's private reflection (spec §7, step 4): what they make of the
/// scene so far, and the goals they now hold that differ from their sheet's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionReply {
    /// The reflection, in the character's inner voice.
    pub reflection: String,
    /// Goals that changed; empty when none did.
    pub goal_changes: Vec<String>,
}

impl StorySchema for ReflectionReply {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Object(vec![
            Property::described(
                "reflection",
                "What the character privately makes of the scene so far, in their own inner \
                 voice: what they now believe, suspect or intend. Story content that no other \
                 character perceives.",
                String::schema(keys),
            ),
            Property::described(
                "goal_changes",
                "Goals the character now holds that differ from the goals on their sheet, one \
                 sentence each; empty when none changed.",
                Vec::<String>::schema(keys),
            ),
        ])
    }
}

/// The World Master's verdict on an intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentVerdict {
    /// The world allows it.
    Feasible,
    /// The world does not allow it.
    Refused,
}

/// The World Master's ruling on one intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentRuling {
    /// Whose intent.
    pub character: CharacterKey,
    /// Why it can or cannot happen now, as the story's narrator would say.
    pub cause: String,
    /// The verdict.
    pub verdict: IntentVerdict,
}

/// The World Master's ruling on a round's intents (spec §7, steps 1–2): a
/// verdict on each, the one that opens the round, what is perceived of it and
/// what it changes; a world event; and whether the scene's pressure is spent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitiativeRuling {
    /// One ruling per intent, waits aside.
    pub rulings: Vec<IntentRuling>,
    /// The feasible intent that opens the round; none when none can.
    pub initiator: Option<CharacterKey>,
    /// What anyone who perceives the initiative perceives of it.
    pub observable: String,
    /// What the initiative changes.
    pub effects: Vec<Effect>,
    /// Something the world does this round of its own accord.
    pub world_event: Option<String>,
    /// Whether this round is the scene's last: its pressure is resolved, or
    /// nothing more can happen here.
    pub scene_over: bool,
}

/// What the World Master is told an observable is. Shared by both rulings.
const OBSERVABLE: &str = "What anyone who perceives the action perceives of it: the words \
    spoken, word for word, and the visible deed, in plain third person. Never a character's \
    thoughts, motives or secrets, unless the action itself gives one away.";

/// What the World Master is told the effects are.
const EFFECTS: &str = "The typed changes the action makes to the world. A move along a route \
    needs a move effect; an item changing hands needs give or take; a secret given away needs \
    a reveal for every character who perceives it. Speech alone changes nothing unless it \
    reveals a secret.";

impl StorySchema for InitiativeRuling {
    fn schema(keys: &CanonKeys) -> Schema {
        let ruling = Schema::Object(vec![
            Property::described(
                "character",
                "Whose intent this rules on.",
                CharacterKey::schema(keys),
            ),
            Property::described(
                "cause",
                "In the story's world, why this action can or cannot happen now, in one \
                 sentence, as a narrator would state it.",
                String::schema(keys),
            ),
            Property::described(
                "verdict",
                "feasible when the world allows the action; refused when it breaks a rule, \
                 needs what the actor lacks, or acts on a fact the actor cannot know.",
                Schema::Enum(vec!["feasible".to_owned(), "refused".to_owned()]),
            ),
        ]);
        Schema::Object(vec![
            Property::described(
                "rulings",
                "One ruling for each intent listed, in the order listed.",
                Schema::Array(Box::new(ruling)),
            ),
            Property::described(
                "initiator",
                "The character whose feasible intent opens the round, or null when none is \
                 feasible.",
                Option::<CharacterKey>::schema(keys),
            ),
            Property::described(
                "observable",
                &format!("{OBSERVABLE} Empty when the initiator is null."),
                String::schema(keys),
            ),
            Property::described(
                "effects",
                &format!("{EFFECTS} Empty when the initiator is null."),
                Vec::<Effect>::schema(keys),
            ),
            Property::described(
                "world_event",
                "Something the world does this round of its own accord, perceived by everyone \
                 here: weather, a bell, a ship, a sound. Use one when the scene stalls or its \
                 pressure should rise; otherwise null.",
                Option::<String>::schema(keys),
            ),
            Property::described(
                "scene_over",
                "true when this round is the scene's last: its pressure is resolved, or \
                 nothing more can happen here.",
                bool::schema(keys),
            ),
        ])
    }
}

/// The World Master's verdict on a reaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionVerdict {
    /// It happens.
    Commit,
    /// It cannot happen.
    Refuse,
}

/// The World Master's ruling on one reaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReactionRuling {
    /// Whose reaction.
    pub character: CharacterKey,
    /// Why it can or cannot happen, as the story's narrator would say.
    pub cause: String,
    /// The verdict.
    pub verdict: ReactionVerdict,
    /// What is perceived of it; empty when refused.
    pub observable: String,
    /// What it changes; empty when refused.
    pub effects: Vec<Effect>,
}

/// The World Master's ruling on a round's reactions (spec §7, step 3), in the
/// order they happen: each committed one is checked against the world the one
/// before it left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReactionsRuling {
    /// One ruling per reaction, in the order they happen.
    pub rulings: Vec<ReactionRuling>,
}

impl StorySchema for ReactionsRuling {
    fn schema(keys: &CanonKeys) -> Schema {
        let ruling = Schema::Object(vec![
            Property::described(
                "character",
                "Whose reaction this rules on.",
                CharacterKey::schema(keys),
            ),
            Property::described(
                "cause",
                "In the story's world, why this reaction can or cannot happen now, in one \
                 sentence, as a narrator would state it.",
                String::schema(keys),
            ),
            Property::described(
                "verdict",
                "commit when the world allows the reaction; refuse when it does not.",
                Schema::Enum(vec!["commit".to_owned(), "refuse".to_owned()]),
            ),
            Property::described(
                "observable",
                &format!("{OBSERVABLE} Empty when refused."),
                String::schema(keys),
            ),
            Property::described(
                "effects",
                &format!("{EFFECTS} Empty when refused."),
                Vec::<Effect>::schema(keys),
            ),
        ]);
        Schema::Object(vec![Property::described(
            "rulings",
            "One ruling for each reaction listed, in the order the reactions happen.",
            Schema::Array(Box::new(ruling)),
        )])
    }
}

#[cfg(test)]
#[path = "contracts_tests.rs"]
mod tests;
