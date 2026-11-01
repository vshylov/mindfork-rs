//! A story's canon and its deterministic checks (docs/research/fabula-engine.md
//! §5, §6, §13 *Canon fixture sketch*).
//!
//! The canon holds the world's rules, the map with its routes, the cast with
//! goals and secrets, the style and the arc. It arrives as JSON — from a file
//! in the evaluation, from `story_create`'s arguments in the chat (spec §14) —
//! and [`Canon::check`] refuses it with every problem named, so whoever
//! composed it can fix all of them at once. Every list is an array of closed
//! objects (routes too, rather than a map keyed by destination), because the
//! same shape has to fit the schema dialect of spec §8.

use std::collections::{BTreeSet, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::{CharacterKey, ItemKey, LocationKey, SecretKey};
use crate::entities::story::Holder;

/// A story's canon (spec §5, *Store ownership*): written only by the Author,
/// with the human's approval; read by role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canon {
    /// The rules the World Master enforces.
    pub world: World,
    /// The map: every place a scene can happen, with its routes.
    pub locations: Vec<Location>,
    /// The cast.
    pub characters: Vec<Character>,
    /// How the text is told.
    #[serde(default)]
    pub style: Style,
    /// The story's arc, for the Author alone.
    pub arc: StoryArc,
}

/// The world's rules, in plain sentences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct World {
    /// "No magic.", "The harbour gate closes at sunset."
    #[serde(default)]
    pub rules: Vec<String>,
}

/// A place on the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    /// The location's key.
    pub key: LocationKey,
    /// Its name in the story's text.
    pub name: String,
    /// What a character there perceives of it.
    #[serde(default)]
    pub description: String,
    /// The places one can walk to from here. A route is one way; the way back
    /// is a route of its own.
    #[serde(default)]
    pub routes: Vec<Route>,
    /// Items lying here when the story starts.
    #[serde(default)]
    pub items: Vec<ItemKey>,
}

/// A one-way route from the location that lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// Where it leads.
    pub to: LocationKey,
    /// How long it takes, in scene ticks (spec §13); at least 1.
    pub ticks: u32,
}

/// A cast member's sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Character {
    /// The character's key.
    pub key: CharacterKey,
    /// Their name in the story's text; the key when empty.
    #[serde(default)]
    pub name: String,
    /// What the others know of them: the description every other character
    /// receives (spec §4, rule 5).
    pub public: String,
    /// How they speak.
    #[serde(default)]
    pub voice: String,
    /// What they want; private to them.
    #[serde(default)]
    pub goals: Vec<String>,
    /// What they hide; private to them until revealed.
    #[serde(default)]
    pub secrets: Vec<Secret>,
    /// Where they are, and what they hold, when the story starts.
    pub start: CharacterStart,
}

/// One secret on a character's sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secret {
    /// The secret's key, named by a `reveal` effect.
    pub key: SecretKey,
    /// The secret itself. The evaluation plants a unique token in it, so a
    /// leak can be counted (spec §11).
    pub text: String,
}

/// A character's starting position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterStart {
    /// Where they are.
    pub location: LocationKey,
    /// What they carry.
    #[serde(default)]
    pub holds: Vec<ItemKey>,
}

/// How the story is told.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Style {
    /// Point of view, "close third, rotating".
    #[serde(default)]
    pub pov: String,
    /// Grammatical tense, "past".
    #[serde(default)]
    pub tense: String,
    /// The language the prose is written in, "English"; empty for English.
    /// The role prompts stay English whatever it is (spec §8, §15).
    #[serde(default)]
    pub language: String,
}

/// The story's arc (spec §7): drafted by the Author, approved by the human
/// before chapter 1, never sent to the World Master or the characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoryArc {
    /// What the story is about, in a sentence or two.
    pub premise: String,
    /// One goal per planned chapter.
    pub chapters: Vec<ChapterArc>,
}

/// A planned chapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChapterArc {
    /// What the chapter has to achieve.
    pub goal: String,
}

/// What a canon key names, for the problems that cite one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// A location.
    Location,
    /// A character.
    Character,
    /// An item.
    Item,
    /// A secret.
    Secret,
}

impl std::fmt::Display for KeyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            KeyKind::Location => "location",
            KeyKind::Character => "character",
            KeyKind::Item => "item",
            KeyKind::Secret => "secret",
        })
    }
}

/// A text the canon must not leave empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonField {
    /// A location's name.
    LocationName,
    /// A character's public description.
    CharacterPublic,
    /// A secret's text.
    SecretText,
    /// The arc's premise.
    Premise,
    /// A chapter's goal; the key is the chapter's number.
    ChapterGoal,
}

/// One reason the canon cannot be used. The texts are English: they go to
/// the model that composed the canon, and the story's prompts are English by
/// the owner's decision (spec §15). A caller that shows one to the person
/// renders the variant in the profile's language instead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonProblem {
    /// The map is empty.
    #[error("the canon has no locations")]
    NoLocations,
    /// The cast is empty.
    #[error("the canon has no characters")]
    NoCharacters,
    /// The arc plans no chapter.
    #[error("the arc has no chapters")]
    NoChapters,
    /// A key that cannot serve as one.
    #[error(
        "{kind} key {key:?} is not a valid key: use 1 to 64 characters, \
         with no spaces, slashes or control characters"
    )]
    BadKey {
        /// What the key names.
        kind: KeyKind,
        /// The key as written.
        key: String,
    },
    /// Two of one kind share a key, compared without regard to case. For an
    /// item it means the item starts in two places.
    #[error("{kind} key {key:?} is used more than once; keys are compared without regard to case")]
    DuplicateKey {
        /// What the key names.
        kind: KeyKind,
        /// The key where it repeats, as written there.
        key: String,
    },
    /// A text that must say something is empty.
    #[error("{}", empty_text(*field, key))]
    EmptyText {
        /// Which text.
        field: CanonField,
        /// Whose: the location, character or secret key, or the chapter's number.
        key: String,
    },
    /// A route leads to a place the map does not have.
    #[error("location \"{from}\" has a route to \"{to}\", which is not a location")]
    UnknownRoute {
        /// The location listing the route.
        from: LocationKey,
        /// Its destination.
        to: LocationKey,
    },
    /// A route leads back to where it starts.
    #[error("location \"{location}\" has a route to itself")]
    SelfRoute {
        /// The location.
        location: LocationKey,
    },
    /// A route that takes no time.
    #[error("the route from \"{from}\" to \"{to}\" takes 0 ticks; a route takes at least 1")]
    ZeroTicks {
        /// The location listing the route.
        from: LocationKey,
        /// Its destination.
        to: LocationKey,
    },
    /// The same destination listed twice by one location.
    #[error("location \"{from}\" lists its route to \"{to}\" more than once")]
    DuplicateRoute {
        /// The location listing the route.
        from: LocationKey,
        /// The destination listed twice.
        to: LocationKey,
    },
    /// A character starts somewhere the map does not have.
    #[error("character \"{character}\" starts at \"{location}\", which is not a location")]
    UnknownStart {
        /// The character.
        character: CharacterKey,
        /// Their start.
        location: LocationKey,
    },
}

fn empty_text(field: CanonField, key: &str) -> String {
    match field {
        CanonField::LocationName => format!("location {key:?} has an empty name"),
        CanonField::CharacterPublic => {
            format!("character {key:?} has an empty public description")
        }
        CanonField::SecretText => format!("secret {key:?} has an empty text"),
        CanonField::Premise => "the arc's premise is empty".to_owned(),
        CanonField::ChapterGoal => format!("chapter {key} of the arc has an empty goal"),
    }
}

/// The longest key the canon accepts, in characters.
const MAX_KEY_CHARS: usize = 64;

/// Whether `key` can serve as a key: it goes into schemas as an `enum` value
/// and into the World Master's prompts, so no whitespace, no `/` (the
/// separator of ids) and no control characters.
fn is_valid_key(key: &str) -> bool {
    let len = key.chars().count();
    (1..=MAX_KEY_CHARS).contains(&len)
        && !key
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/')
}

/// Checks one kind's keys: each must be valid, and none may repeat without
/// regard to case. A repeated key is reported once.
fn check_keys<'a>(
    kind: KeyKind,
    keys: impl IntoIterator<Item = &'a str>,
    out: &mut Vec<CanonProblem>,
) {
    let mut seen = HashSet::new();
    let mut reported = HashSet::new();
    for key in keys {
        if !is_valid_key(key) {
            out.push(CanonProblem::BadKey {
                kind,
                key: key.to_owned(),
            });
            continue;
        }
        let folded = key.to_lowercase();
        if !seen.insert(folded.clone()) && reported.insert(folded) {
            out.push(CanonProblem::DuplicateKey {
                kind,
                key: key.to_owned(),
            });
        }
    }
}

fn require_text(text: &str, field: CanonField, key: &str, out: &mut Vec<CanonProblem>) {
    if text.trim().is_empty() {
        out.push(CanonProblem::EmptyText {
            field,
            key: key.to_owned(),
        });
    }
}

impl Canon {
    /// Parses a canon. Unknown fields are refused, so a misspelt one is named
    /// instead of read as absent.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Every deterministic problem of the canon (spec §8, *Rules shared by all
    /// contracts*): keys, routes, starts, empty texts. An empty list means the
    /// canon can be stored.
    pub fn check(&self) -> Vec<CanonProblem> {
        let mut out = Vec::new();
        if self.locations.is_empty() {
            out.push(CanonProblem::NoLocations);
        }
        if self.characters.is_empty() {
            out.push(CanonProblem::NoCharacters);
        }
        check_keys(
            KeyKind::Location,
            self.locations.iter().map(|l| l.key.as_str()),
            &mut out,
        );
        check_keys(
            KeyKind::Character,
            self.characters.iter().map(|c| c.key.as_str()),
            &mut out,
        );
        check_keys(
            KeyKind::Item,
            self.starting_items().map(|(item, _)| item.as_str()),
            &mut out,
        );
        check_keys(
            KeyKind::Secret,
            self.characters
                .iter()
                .flat_map(|c| c.secrets.iter().map(|s| s.key.as_str())),
            &mut out,
        );
        for location in &self.locations {
            self.check_location(location, &mut out);
        }
        for character in &self.characters {
            require_text(
                &character.public,
                CanonField::CharacterPublic,
                character.key.as_str(),
                &mut out,
            );
            if self.location(&character.start.location).is_none() {
                out.push(CanonProblem::UnknownStart {
                    character: character.key.clone(),
                    location: character.start.location.clone(),
                });
            }
            for secret in &character.secrets {
                require_text(
                    &secret.text,
                    CanonField::SecretText,
                    secret.key.as_str(),
                    &mut out,
                );
            }
        }
        require_text(&self.arc.premise, CanonField::Premise, "", &mut out);
        if self.arc.chapters.is_empty() {
            out.push(CanonProblem::NoChapters);
        }
        for (i, chapter) in self.arc.chapters.iter().enumerate() {
            require_text(
                &chapter.goal,
                CanonField::ChapterGoal,
                &(i + 1).to_string(),
                &mut out,
            );
        }
        out
    }

    fn check_location(&self, location: &Location, out: &mut Vec<CanonProblem>) {
        require_text(
            &location.name,
            CanonField::LocationName,
            location.key.as_str(),
            out,
        );
        let mut seen = HashSet::new();
        for route in &location.routes {
            if route.to == location.key {
                out.push(CanonProblem::SelfRoute {
                    location: location.key.clone(),
                });
            } else if self.location(&route.to).is_none() {
                out.push(CanonProblem::UnknownRoute {
                    from: location.key.clone(),
                    to: route.to.clone(),
                });
            }
            if route.ticks == 0 {
                out.push(CanonProblem::ZeroTicks {
                    from: location.key.clone(),
                    to: route.to.clone(),
                });
            }
            if !seen.insert(&route.to) {
                out.push(CanonProblem::DuplicateRoute {
                    from: location.key.clone(),
                    to: route.to.clone(),
                });
            }
        }
    }

    /// The location with this key.
    pub fn location(&self, key: &LocationKey) -> Option<&Location> {
        self.locations.iter().find(|l| &l.key == key)
    }

    /// The character with this key.
    pub fn character(&self, key: &CharacterKey) -> Option<&Character> {
        self.characters.iter().find(|c| &c.key == key)
    }

    /// Whether the canon has this item, wherever it starts.
    pub fn has_item(&self, item: &ItemKey) -> bool {
        self.starting_items().any(|(i, _)| i == item)
    }

    /// Whose sheet lists this secret.
    pub fn secret_holder(&self, secret: &SecretKey) -> Option<&CharacterKey> {
        self.characters
            .iter()
            .find(|c| c.secrets.iter().any(|s| &s.key == secret))
            .map(|c| &c.key)
    }

    /// Every item and where it starts: in a character's hands, or lying at a
    /// location. In canon order, locations first.
    pub fn starting_items(&self) -> impl Iterator<Item = (&ItemKey, Holder)> {
        let lying = self.locations.iter().flat_map(|l| {
            l.items
                .iter()
                .map(move |item| (item, Holder::Location(l.key.clone())))
        });
        let held = self.characters.iter().flat_map(|c| {
            c.start
                .holds
                .iter()
                .map(move |item| (item, Holder::Character(c.key.clone())))
        });
        lying.chain(held)
    }

    /// The direct route from `from` to `to`, in ticks.
    pub fn route(&self, from: &LocationKey, to: &LocationKey) -> Option<u32> {
        self.location(from)?
            .routes
            .iter()
            .find(|r| &r.to == to)
            .map(|r| r.ticks)
    }

    /// Whether `to` can be reached from `from` along routes, however many.
    pub fn reachable(&self, from: &LocationKey, to: &LocationKey) -> bool {
        if from == to {
            return true;
        }
        let mut seen = BTreeSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(here) = queue.pop_front() {
            let Some(location) = self.location(here) else {
                continue;
            };
            for route in &location.routes {
                if &route.to == to {
                    return true;
                }
                if seen.insert(&route.to) {
                    queue.push_back(&route.to);
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::story::fixture_canon;

    #[test]
    fn the_fixture_has_no_problems() {
        let canon = fixture_canon();
        assert_eq!(canon.check(), Vec::new());
        assert_eq!(canon.locations.len(), 5);
        assert_eq!(canon.characters.len(), 3);
    }

    #[test]
    fn an_unknown_field_is_named_rather_than_read_as_absent() {
        let json = include_str!("fixtures/harbour.json").replacen("\"secrets\"", "\"secret\"", 1);
        let err = Canon::from_json(&json).unwrap_err().to_string();
        assert!(err.contains("unknown field `secret`"), "{err}");
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        let mut canon = fixture_canon();
        canon.locations[0].routes.push(Route {
            to: LocationKey::from("moon"),
            ticks: 0,
        });
        canon.locations[1].routes.push(Route {
            to: LocationKey::from("inn"),
            ticks: 1,
        });
        canon.locations[2].routes.push(Route {
            to: LocationKey::from("harbour"),
            ticks: 3,
        });
        canon.locations[3].name = "  ".into();
        canon.characters[1].key = CharacterKey::from("Mara");
        canon.characters[2].key = CharacterKey::from("old ines");
        canon.characters[2].start.location = LocationKey::from("nowhere");
        canon.characters[0].secrets[0].text.clear();
        canon.characters[0].start.holds.push(ItemKey::from("crate"));
        canon.arc.chapters[0].goal.clear();
        let problems = canon.check();
        let expected = vec![
            CanonProblem::DuplicateKey {
                kind: KeyKind::Character,
                key: "Mara".into(),
            },
            CanonProblem::BadKey {
                kind: KeyKind::Character,
                key: "old ines".into(),
            },
            CanonProblem::DuplicateKey {
                kind: KeyKind::Item,
                key: "crate".into(),
            },
            CanonProblem::UnknownRoute {
                from: LocationKey::from("harbour"),
                to: LocationKey::from("moon"),
            },
            CanonProblem::ZeroTicks {
                from: LocationKey::from("harbour"),
                to: LocationKey::from("moon"),
            },
            CanonProblem::SelfRoute {
                location: LocationKey::from("inn"),
            },
            CanonProblem::DuplicateRoute {
                from: LocationKey::from("lighthouse"),
                to: LocationKey::from("harbour"),
            },
            CanonProblem::EmptyText {
                field: CanonField::LocationName,
                key: "chapel".into(),
            },
            CanonProblem::EmptyText {
                field: CanonField::SecretText,
                key: "bribe".into(),
            },
            CanonProblem::UnknownStart {
                character: CharacterKey::from("old ines"),
                location: LocationKey::from("nowhere"),
            },
            CanonProblem::EmptyText {
                field: CanonField::ChapterGoal,
                key: "1".into(),
            },
        ];
        assert_eq!(problems, expected);
    }

    #[test]
    fn an_empty_canon_names_what_is_missing() {
        let mut canon = fixture_canon();
        canon.locations.clear();
        canon.characters.clear();
        canon.arc.chapters.clear();
        canon.arc.premise.clear();
        assert_eq!(
            canon.check(),
            vec![
                CanonProblem::NoLocations,
                CanonProblem::NoCharacters,
                CanonProblem::EmptyText {
                    field: CanonField::Premise,
                    key: String::new(),
                },
                CanonProblem::NoChapters,
            ]
        );
    }

    #[test]
    fn a_key_is_bounded_and_has_no_separators() {
        assert!(is_valid_key("mara"));
        assert!(is_valid_key("warehouse_key"));
        assert!(is_valid_key(&"x".repeat(MAX_KEY_CHARS)));
        assert!(!is_valid_key(""));
        assert!(!is_valid_key(&"x".repeat(MAX_KEY_CHARS + 1)));
        assert!(!is_valid_key("a/b"));
        assert!(!is_valid_key("a\tb"));
        assert!(!is_valid_key("a\u{7}b"));
    }

    #[test]
    fn problems_read_as_sentences() {
        let texts: Vec<String> = [
            CanonProblem::BadKey {
                kind: KeyKind::Secret,
                key: "a b".into(),
            },
            CanonProblem::DuplicateKey {
                kind: KeyKind::Location,
                key: "inn".into(),
            },
            CanonProblem::EmptyText {
                field: CanonField::CharacterPublic,
                key: "mara".into(),
            },
            CanonProblem::EmptyText {
                field: CanonField::Premise,
                key: String::new(),
            },
            CanonProblem::EmptyText {
                field: CanonField::ChapterGoal,
                key: "2".into(),
            },
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            texts,
            [
                "secret key \"a b\" is not a valid key: use 1 to 64 characters, \
                 with no spaces, slashes or control characters",
                "location key \"inn\" is used more than once; keys are compared without regard to case",
                "character \"mara\" has an empty public description",
                "the arc's premise is empty",
                "chapter 2 of the arc has an empty goal",
            ]
        );
        assert_eq!(KeyKind::Item.to_string(), "item");
        assert_eq!(KeyKind::Character.to_string(), "character");
    }

    #[test]
    fn routes_are_one_way_and_reachability_follows_them() {
        let canon = fixture_canon();
        let at = LocationKey::from;
        assert_eq!(canon.route(&at("harbour"), &at("lighthouse")), Some(2));
        assert_eq!(canon.route(&at("lighthouse"), &at("inn")), None);
        assert_eq!(canon.route(&at("moon"), &at("inn")), None);
        assert!(canon.reachable(&at("lighthouse"), &at("chapel")));
        assert!(canon.reachable(&at("inn"), &at("inn")));
        let mut island = canon.clone();
        island.locations[3].routes.clear();
        assert!(!island.reachable(&at("chapel"), &at("harbour")));
        assert!(island.reachable(&at("harbour"), &at("chapel")));
    }

    #[test]
    fn lookups_find_what_the_canon_holds() {
        let canon = fixture_canon();
        assert_eq!(
            canon.secret_holder(&SecretKey::from("rifles")),
            Some(&CharacterKey::from("silas"))
        );
        assert_eq!(canon.secret_holder(&SecretKey::from("nothing")), None);
        assert!(canon.has_item(&ItemKey::from("crate")));
        assert!(canon.has_item(&ItemKey::from("ledger")));
        assert!(!canon.has_item(&ItemKey::from("rifle")));
        let items: Vec<_> = canon
            .starting_items()
            .map(|(i, h)| (i.as_str(), h))
            .collect();
        assert_eq!(
            items,
            [
                ("lantern", Holder::Location("inn".into())),
                ("crate", Holder::Location("warehouse".into())),
                ("ledger", Holder::Character("mara".into())),
                ("warehouse_key", Holder::Character("silas".into())),
                ("logbook", Holder::Character("ines".into())),
            ]
        );
    }
}
