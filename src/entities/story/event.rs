//! What the simulation produces: the Author's circumstances, a character's
//! proposal, and the events the World Master commits from them
//! (docs/research/fabula-engine.md §4, §6, §7).
//!
//! Every tagged union is tagged by a `type` field, so each one maps onto an
//! `anyOf` of closed objects with a one-value `enum` tag — the schema dialect
//! of spec §8 — when the engine derives its schemas from these types.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{BranchId, CharacterKey, EventId, ItemKey, LocationKey, ProposalId, SecretKey};

/// The Author's given circumstances for a scene (spec §4, rule 2): who is
/// where, what has just happened, and what pressure applies — never what a
/// character will do. The World Master commits it as the branch's opening
/// events, so the characters perceive it rather than read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSetup {
    /// Where the scene happens.
    pub location: LocationKey,
    /// Who is there.
    pub present: Vec<CharacterKey>,
    /// What has just happened; committed as opening events.
    pub just_happened: String,
    /// What the world pushes.
    pub pressure: String,
}

/// A character's proposal for its next action (spec §8). The thought comes
/// first, because a model writes the fields in order and the action should
/// follow from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// The character's inner voice: it becomes the event's private part.
    pub private_thought: String,
    /// What they do.
    pub action: Action,
    /// Whom it is aimed at.
    pub targets: Vec<CharacterKey>,
    /// Who may perceive it.
    pub visibility: Visibility,
}

/// What a character proposes to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Says something.
    Speak {
        /// The line.
        line: String,
    },
    /// Goes somewhere.
    Move {
        /// The destination.
        to: LocationKey,
    },
    /// Does something, perhaps saying something while at it.
    Act {
        /// The deed, "slams".
        verb: String,
        /// What it is done to, "the ledger".
        object: Option<String>,
        /// What is said meanwhile.
        line: Option<String>,
    },
    /// Does nothing: commits no event, and counts toward a stalemate (spec §7).
    Wait,
}

/// Who may perceive an action. The witnesses are computed from it and from
/// who is present; no model chooses them (spec §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Visibility {
    /// Everyone present.
    Public,
    /// Only the named characters, who must be present.
    Whisper {
        /// The addressees.
        to: Vec<CharacterKey>,
    },
    /// The actor alone.
    Hidden,
}

/// What an event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The scene's circumstances, committed from the Author's setup.
    Opening,
    /// A line spoken.
    Speech,
    /// A deed.
    Action,
    /// A character going somewhere.
    Movement,
    /// Something the world does: weather, a bell, a ship.
    WorldEvent,
    /// A change of the world between chapters (spec §7, step 9).
    Settle,
}

impl EventKind {
    /// Every kind, in declaration order.
    pub const ALL: [EventKind; 6] = [
        EventKind::Opening,
        EventKind::Speech,
        EventKind::Action,
        EventKind::Movement,
        EventKind::WorldEvent,
        EventKind::Settle,
    ];

    /// The stored name, the same as the serialized one.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Opening => "opening",
            EventKind::Speech => "speech",
            EventKind::Action => "action",
            EventKind::Movement => "movement",
            EventKind::WorldEvent => "world_event",
            EventKind::Settle => "settle",
        }
    }

    /// The kind with this stored name.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Whether a character must perform it.
    pub fn needs_actor(self) -> bool {
        matches!(
            self,
            EventKind::Speech | EventKind::Action | EventKind::Movement
        )
    }

    /// Whether it may put a character anywhere reachable, as the time
    /// between scenes passes (`Effect::Place`).
    pub fn may_place(self) -> bool {
        matches!(self, EventKind::Opening | EventKind::Settle)
    }
}

/// How badly a character is hurt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A bruise, a cut.
    Minor,
    /// A wound that changes what they can do.
    Serious,
    /// Death: the character neither acts nor perceives afterwards.
    Fatal,
}

/// A typed change of the world state, checked by code against the state it
/// applies to (spec §6). World state is the fold of these over a branch path.
/// Every character an effect names must be at the event's location, except
/// the destination of a move and a placed character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    /// A character walks a direct route to another location.
    Move {
        /// Who goes.
        character: CharacterKey,
        /// Where.
        to: LocationKey,
    },
    /// A character is put at a location reachable from where they are, as
    /// the time between scenes passes. Only in opening and settle events.
    Place {
        /// Who.
        character: CharacterKey,
        /// Where.
        to: LocationKey,
    },
    /// An item passes from one character's hands to another's.
    Give {
        /// The giver, who holds the item.
        from: CharacterKey,
        /// The receiver.
        to: CharacterKey,
        /// The item.
        item: ItemKey,
    },
    /// A character takes an item: from another character, or lying where
    /// they are when `from` is null.
    Take {
        /// The taker.
        by: CharacterKey,
        /// The item.
        item: ItemKey,
        /// Whose hands it leaves; null when it lies at the location.
        from: Option<CharacterKey>,
    },
    /// A character puts an item down where they are.
    Drop {
        /// Who drops it.
        by: CharacterKey,
        /// The item.
        item: ItemKey,
    },
    /// A character learns a secret.
    Reveal {
        /// The secret.
        secret: SecretKey,
        /// Who learns it.
        to: CharacterKey,
    },
    /// A character is hurt.
    Injure {
        /// Who.
        character: CharacterKey,
        /// How badly.
        severity: Severity,
    },
}

impl Effect {
    /// The characters the effect acts on. They perceive the event unless it
    /// is hidden.
    pub fn subjects(&self) -> Vec<&CharacterKey> {
        match self {
            Effect::Move { character, .. }
            | Effect::Place { character, .. }
            | Effect::Injure { character, .. } => vec![character],
            Effect::Give { from, to, .. } => vec![from, to],
            Effect::Take { by, from, .. } => std::iter::once(by).chain(from).collect(),
            Effect::Drop { by, .. } => vec![by],
            Effect::Reveal { to, .. } => vec![to],
        }
    }
}

/// Why an event happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cause {
    /// The scene's setup (an opening event).
    Setup,
    /// The round's initiating proposal (spec §7, *One simulation round*).
    Initiative,
    /// A response to an earlier event on the same branch path.
    Reaction {
        /// The event responded to.
        to: EventId,
    },
    /// The World Master's own world event.
    World,
    /// A world-state delta between chapters.
    Settle,
}

/// An event as the World Master commits it, before it has an id or
/// witnesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewEvent {
    /// The simulation round, 0 for opening events.
    pub round: u32,
    /// Story time in ticks; never earlier than the branch path's last event.
    pub tick: u32,
    /// Where it happens.
    pub location: LocationKey,
    /// What it is.
    pub kind: EventKind,
    /// Who does it; none for a world event.
    pub actor: Option<CharacterKey>,
    /// Who may perceive it.
    pub visibility: Visibility,
    /// What its witnesses perceive: speech, visible action.
    pub observable: String,
    /// The actor's thought, emotion or motive; theirs alone.
    pub private: Option<String>,
    /// What it changes.
    pub effects: Vec<Effect>,
    /// The proposal it was committed from.
    pub proposal: Option<ProposalId>,
    /// Why it happened.
    pub cause: Cause,
}

/// A committed event as the story database returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Its place in the whole story's commit order; within a branch path,
    /// narrative order (spec §9).
    pub seq: u64,
    /// Its id.
    pub id: EventId,
    /// The branch that committed it.
    pub branch: BranchId,
    /// What was committed.
    pub body: NewEvent,
    /// Who perceived its observable part, computed at commit.
    pub witnesses: BTreeSet<CharacterKey>,
}

/// The World Master's ruling on a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalVerdict {
    /// Committed as an event.
    Committed,
    /// Refused, with a reason.
    Refused,
    /// Not picked as the round's initiative; the character may propose again
    /// (spec §7).
    Lapsed,
}

impl ProposalVerdict {
    /// Every verdict, in declaration order.
    pub const ALL: [ProposalVerdict; 3] = [
        ProposalVerdict::Committed,
        ProposalVerdict::Refused,
        ProposalVerdict::Lapsed,
    ];

    /// The stored name, the same as the serialized one.
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalVerdict::Committed => "committed",
            ProposalVerdict::Refused => "refused",
            ProposalVerdict::Lapsed => "lapsed",
        }
    }

    /// The verdict with this stored name.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_names_match_the_serialized_ones() {
        for kind in EventKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(EventKind::parse(kind.as_str()), Some(kind));
        }
        for verdict in ProposalVerdict::ALL {
            let json = serde_json::to_string(&verdict).unwrap();
            assert_eq!(json, format!("\"{}\"", verdict.as_str()));
            assert_eq!(ProposalVerdict::parse(verdict.as_str()), Some(verdict));
        }
        assert_eq!(EventKind::parse("dream"), None);
        assert_eq!(ProposalVerdict::parse("maybe"), None);
    }

    #[test]
    fn unions_are_tagged_by_type() {
        let proposal: Proposal = serde_json::from_value(serde_json::json!({
            "private_thought": "Not tonight.",
            "action": {"type": "act", "verb": "slams", "object": "the ledger", "line": null},
            "targets": ["ines"],
            "visibility": {"type": "whisper", "to": ["ines"]}
        }))
        .unwrap();
        assert_eq!(
            proposal.action,
            Action::Act {
                verb: "slams".into(),
                object: Some("the ledger".into()),
                line: None
            }
        );
        let effect: Effect = serde_json::from_value(serde_json::json!(
            {"type": "take", "by": "silas", "item": "ledger", "from": "mara"}
        ))
        .unwrap();
        assert_eq!(
            effect.subjects(),
            [&CharacterKey::from("silas"), &CharacterKey::from("mara")]
        );
        let wait: Action = serde_json::from_str(r#"{"type":"wait"}"#).unwrap();
        assert_eq!(wait, Action::Wait);
        // A struct variant refuses a field it lacks; a unit variant does not,
        // even under `deny_unknown_fields`: serde reads this whisper-shaped
        // reply as `Public`. The closed schema is what refuses it, so a
        // model's reply is validated against its schema before it is parsed
        // (spec §8; docs/fabula.md §4, PR 2).
        let stray = serde_json::from_str::<Visibility>(r#"{"type":"whisper","to":[],"loud":true}"#);
        assert!(stray.is_err(), "a field the variant lacks is refused");
        let lenient = serde_json::from_str::<Visibility>(r#"{"type":"public","to":["ines"]}"#);
        assert_eq!(lenient.unwrap(), Visibility::Public);
    }

    #[test]
    fn only_deeds_need_an_actor_and_only_openings_place() {
        let needs: Vec<_> = EventKind::ALL
            .into_iter()
            .filter(|k| k.needs_actor())
            .collect();
        assert_eq!(
            needs,
            [EventKind::Speech, EventKind::Action, EventKind::Movement]
        );
        let places: Vec<_> = EventKind::ALL
            .into_iter()
            .filter(|k| k.may_place())
            .collect();
        assert_eq!(places, [EventKind::Opening, EventKind::Settle]);
    }

    #[test]
    fn every_effect_names_its_subjects() {
        let c = CharacterKey::from;
        let cases = [
            (
                Effect::Move {
                    character: c("mara"),
                    to: "inn".into(),
                },
                vec!["mara"],
            ),
            (
                Effect::Give {
                    from: c("mara"),
                    to: c("ines"),
                    item: "ledger".into(),
                },
                vec!["mara", "ines"],
            ),
            (
                Effect::Take {
                    by: c("ines"),
                    item: "crate".into(),
                    from: None,
                },
                vec!["ines"],
            ),
            (
                Effect::Drop {
                    by: c("silas"),
                    item: "warehouse_key".into(),
                },
                vec!["silas"],
            ),
            (
                Effect::Reveal {
                    secret: "bribe".into(),
                    to: c("ines"),
                },
                vec!["ines"],
            ),
            (
                Effect::Injure {
                    character: c("silas"),
                    severity: Severity::Minor,
                },
                vec!["silas"],
            ),
        ];
        for (effect, subjects) in cases {
            let got: Vec<&str> = effect.subjects().into_iter().map(|k| k.as_str()).collect();
            assert_eq!(got, subjects, "{effect:?}");
        }
    }
}
