//! Fabula's domain: a story's canon, the events its World Master commits, and
//! the world state folded from them (docs/research/fabula-engine.md §4–§6,
//! docs/fabula.md §3).
//!
//! Pure data and pure functions. The story database
//! (`shared::storage::story`) stores these types, and it runs the fold
//! ([`WorldState::admit`]) inside its actor, so an event is checked against
//! the state of its branch and appended by the same writer, with nothing in
//! between. The fold and the witnesses live here rather than in
//! `features::story` for that reason: the layer gate lets `shared` name
//! `entities`, never `features`.
//!
//! Keys name what the canon fixes (characters, locations, items, secrets) and
//! are compared exactly here; a model's reply is matched to them without
//! regard to case before it reaches this module (spec §8). Ids name what grows
//! during a run (scenes, branches, events, proposals, runs) and are derived
//! from their position, never drawn at random, so a replayed run produces the
//! same ids and the same prompts (spec §9).

mod canon;
mod event;
mod world;

// The domain's whole surface; the engine that names the rest arrives from
// docs/fabula.md §4 PR 2 on.
#[allow(unused_imports)]
pub use canon::{
    Canon, CanonField, CanonProblem, ChapterArc, Character, CharacterStart, KeyKind, Location,
    Route, Secret, StoryArc, Style, World,
};
#[allow(unused_imports)]
pub use event::{
    Action, Cause, Effect, Event, EventKind, NewEvent, Proposal, ProposalVerdict, SceneSetup,
    Severity, Visibility,
};
#[allow(unused_imports)]
pub use world::{Admitted, FoldError, Holder, Refusal, WorldState};

use serde::{Deserialize, Serialize};

/// A string newtype: a key the canon fixes, or an id a run derives.
macro_rules! text_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps a string as it is: a key from the canon, or an id read back
            /// from the story database.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// The string itself.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    };
}

text_newtype!(
    /// A cast member's key in the canon (`"mara"`).
    CharacterKey
);
text_newtype!(
    /// A location's key in the canon (`"harbour"`).
    LocationKey
);
text_newtype!(
    /// An item's key (`"ledger"`): an item starts in exactly one place, held
    /// by a character or lying at a location.
    ItemKey
);
text_newtype!(
    /// A secret's key; a secret belongs to the character whose sheet lists it.
    SecretKey
);
text_newtype!(
    /// A scene's id: its chapter, its number in the chapter and its rerun,
    /// `ch1/sc2/r0`. A rerun is a new scene (spec §6).
    SceneId
);
text_newtype!(
    /// A branch's id: `genesis` for the root seeded from the canon, or its
    /// scene and its index among the scene's K branches, `ch1/sc2/r0/k1`.
    BranchId
);
text_newtype!(
    /// An event's id: its branch and its ordinal among the branch's own
    /// events, `ch1/sc2/r0/k1/e3`.
    EventId
);
text_newtype!(
    /// A proposal's id: its branch and its ordinal among the branch's
    /// proposals, `ch1/sc2/r0/k1/p4`.
    ProposalId
);
text_newtype!(
    /// A run's id, `run1`, `run2`: one per background run of the engine.
    RunId
);

impl SceneId {
    /// The scene numbered `scene` in chapter `chapter`, at rerun `rerun`.
    pub fn of(chapter: u32, scene: u32, rerun: u32) -> Self {
        Self(format!("ch{chapter}/sc{scene}/r{rerun}"))
    }
}

impl BranchId {
    /// The root of every story's tree, seeded from the canon (spec §6).
    pub fn genesis() -> Self {
        Self("genesis".to_owned())
    }

    /// Branch `k` of a scene's K branches.
    pub fn of_scene(scene: &SceneId, k: u32) -> Self {
        Self(format!("{scene}/k{k}"))
    }
}

impl EventId {
    /// The `n`-th event (from 1) of `branch`'s own events.
    pub fn nth(branch: &BranchId, n: u64) -> Self {
        Self(format!("{branch}/e{n}"))
    }
}

impl ProposalId {
    /// The `n`-th proposal (from 1) recorded on `branch`.
    pub fn nth(branch: &BranchId, n: u64) -> Self {
        Self(format!("{branch}/p{n}"))
    }
}

impl RunId {
    /// The `n`-th run (from 1) of a story.
    pub fn nth(n: u64) -> Self {
        Self(format!("run{n}"))
    }
}

/// The canon fixture the story tests share: three characters, five locations,
/// a planted secret token on every sheet (spec §11, §13).
#[cfg(test)]
pub(crate) fn fixture_canon() -> Canon {
    Canon::from_json(include_str!("fixtures/harbour.json")).expect("the fixture parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_derived_from_their_position() {
        let scene = SceneId::of(1, 2, 0);
        assert_eq!(scene.as_str(), "ch1/sc2/r0");
        let branch = BranchId::of_scene(&scene, 1);
        assert_eq!(branch.as_str(), "ch1/sc2/r0/k1");
        assert_eq!(EventId::nth(&branch, 3).as_str(), "ch1/sc2/r0/k1/e3");
        assert_eq!(ProposalId::nth(&branch, 4).as_str(), "ch1/sc2/r0/k1/p4");
        assert_eq!(EventId::nth(&BranchId::genesis(), 1).as_str(), "genesis/e1");
        assert_eq!(RunId::nth(2).to_string(), "run2");
    }

    #[test]
    fn a_key_serializes_as_its_bare_string() {
        let key = CharacterKey::from("mara");
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"mara\"");
        let back: CharacterKey = serde_json::from_str("\"mara\"").unwrap();
        assert_eq!(back, key);
        assert_eq!(CharacterKey::new(String::from("mara")), key);
    }
}
