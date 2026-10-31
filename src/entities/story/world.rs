//! The world state folded over a branch path, the checks an event must pass
//! against it, and who perceives the event (docs/research/fabula-engine.md §6).
//!
//! The state starts from the canon (where everyone is, who holds what, who
//! knows which secret) and changes only through the typed effects of
//! committed events. [`WorldState::admit`] is the one gate: it checks an event
//! against the state it would apply to, applies it, and computes its witnesses
//! from presence and visibility. The story database calls it inside its actor
//! for every commit, and [`WorldState::fold`] replays a path through it, so
//! what was admitted once folds the same way again.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Canon, CharacterKey, Effect, Event, EventId, EventKind, ItemKey, LocationKey, NewEvent,
    SecretKey, Severity, Visibility,
};

/// Where an item is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    /// In a character's hands.
    Character(CharacterKey),
    /// Lying at a location.
    Location(LocationKey),
}

/// The world as it stands at one point of a branch path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorldState {
    positions: BTreeMap<CharacterKey, LocationKey>,
    items: BTreeMap<ItemKey, Holder>,
    knows: BTreeMap<CharacterKey, BTreeSet<SecretKey>>,
    injuries: BTreeMap<CharacterKey, Vec<Severity>>,
    tick: u32,
}

/// An event that passed [`WorldState::admit`]: the state it leaves, and who
/// perceived it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    /// The world after the event.
    pub state: WorldState,
    /// Who perceives its observable part.
    pub witnesses: BTreeSet<CharacterKey>,
}

/// Why an event cannot be committed. The World Master gets the text back as
/// the reason for a refusal; it is English, as the story's prompts are (spec
/// §15).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// A location the canon lacks.
    #[error("{0} is not a location in the canon")]
    UnknownLocation(LocationKey),
    /// A character the canon lacks.
    #[error("{0} is not a character in the canon")]
    UnknownCharacter(CharacterKey),
    /// An item the canon lacks.
    #[error("{0} is not an item in the canon")]
    UnknownItem(ItemKey),
    /// A secret the canon lacks.
    #[error("{0} is not a secret in the canon")]
    UnknownSecret(SecretKey),
    /// A reaction to an event its branch path does not have.
    #[error("event {0} is not on this branch")]
    UnknownEvent(EventId),
    /// A dead character cannot act, be acted on, or perceive.
    #[error("{0} is dead")]
    Dead(CharacterKey),
    /// A character named by the event is elsewhere.
    #[error("{character} is at {actually}, not at {location}")]
    Absent {
        /// Who.
        character: CharacterKey,
        /// The event's location.
        location: LocationKey,
        /// Where they are.
        actually: LocationKey,
    },
    /// A speech, action or movement with no one doing it.
    #[error("a {} event needs an actor", .0.as_str())]
    NoActor(EventKind),
    /// A world event credited to a character.
    #[error("a world event has no actor, but {0} was named")]
    ActorOnWorldEvent(CharacterKey),
    /// A whisper or a hidden deed with no one doing it.
    #[error("only a character can whisper or act unseen")]
    UnseenWithoutActor,
    /// A whisper addressed to no one.
    #[error("a whisper needs at least one addressee")]
    WhisperToNobody,
    /// A whisper addressed to its speaker.
    #[error("{0} cannot whisper to themselves")]
    WhisperToSelf(CharacterKey),
    /// Story time going backwards.
    #[error("tick {tick} is before the branch's last tick, {last}")]
    TimeRunsBackwards {
        /// The event's tick.
        tick: u32,
        /// The path's last tick.
        last: u32,
    },
    /// A move to where the character already is.
    #[error("{character} is already at {location}")]
    AlreadyThere {
        /// Who.
        character: CharacterKey,
        /// Where.
        location: LocationKey,
    },
    /// A move with no direct route.
    #[error("there is no direct route from {from} to {to}")]
    NoRoute {
        /// Where the character is.
        from: LocationKey,
        /// Where they were to go.
        to: LocationKey,
    },
    /// A placement nothing connects.
    #[error("{to} cannot be reached from {from}")]
    Unreachable {
        /// Where the character is.
        from: LocationKey,
        /// Where they were to be placed.
        to: LocationKey,
    },
    /// A placement inside a scene.
    #[error("a character can be placed only by an opening or settle event, not by a {}", .0.as_str())]
    PlaceOutsideOpening(EventKind),
    /// A gift to oneself.
    #[error("{0} cannot give to themselves")]
    GiveToSelf(CharacterKey),
    /// Taking from oneself.
    #[error("{0} cannot take from themselves")]
    TakeFromSelf(CharacterKey),
    /// An item the character does not hold.
    #[error("{character} does not hold {item}")]
    NotHolding {
        /// Who.
        character: CharacterKey,
        /// What.
        item: ItemKey,
    },
    /// An item not lying where the taker is.
    #[error("{item} is not lying at {location}")]
    NotHere {
        /// What.
        item: ItemKey,
        /// The event's location.
        location: LocationKey,
    },
}

/// A stored event that no longer folds: the rules changed under it, or the
/// file was edited by hand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("stored event {event} no longer folds: {refusal}")]
pub struct FoldError {
    /// The event.
    pub event: EventId,
    /// What its check says now.
    pub refusal: Refusal,
}

impl WorldState {
    /// The world when the story starts, as the canon sets it.
    pub fn initial(canon: &Canon) -> Self {
        let positions = canon
            .characters
            .iter()
            .map(|c| (c.key.clone(), c.start.location.clone()))
            .collect();
        let items = canon
            .starting_items()
            .map(|(item, holder)| (item.clone(), holder))
            .collect();
        let knows = canon
            .characters
            .iter()
            .map(|c| {
                (
                    c.key.clone(),
                    c.secrets.iter().map(|s| s.key.clone()).collect(),
                )
            })
            .collect();
        Self {
            positions,
            items,
            knows,
            injuries: BTreeMap::new(),
            tick: 0,
        }
    }

    /// The state at the end of `events`, a branch path in narrative order.
    pub fn fold<'a>(
        canon: &Canon,
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Result<Self, FoldError> {
        let mut state = Self::initial(canon);
        for event in events {
            state = state
                .admit(canon, &event.body)
                .map_err(|refusal| FoldError {
                    event: event.id.clone(),
                    refusal,
                })?
                .state;
        }
        Ok(state)
    }

    /// Checks `event` against this state; on success, the state it leaves
    /// and its witnesses.
    pub fn admit(&self, canon: &Canon, event: &NewEvent) -> Result<Admitted, Refusal> {
        if canon.location(&event.location).is_none() {
            return Err(Refusal::UnknownLocation(event.location.clone()));
        }
        if event.tick < self.tick {
            return Err(Refusal::TimeRunsBackwards {
                tick: event.tick,
                last: self.tick,
            });
        }
        match (&event.actor, event.kind) {
            (None, kind) if kind.needs_actor() => return Err(Refusal::NoActor(kind)),
            (Some(actor), EventKind::WorldEvent) => {
                return Err(Refusal::ActorOnWorldEvent(actor.clone()));
            }
            _ => {}
        }
        if let Some(actor) = &event.actor {
            self.require_at(canon, actor, &event.location)?;
        }
        match &event.visibility {
            Visibility::Public => {}
            Visibility::Hidden if event.actor.is_none() => return Err(Refusal::UnseenWithoutActor),
            Visibility::Hidden => {}
            Visibility::Whisper { to } => {
                let Some(actor) = &event.actor else {
                    return Err(Refusal::UnseenWithoutActor);
                };
                if to.is_empty() {
                    return Err(Refusal::WhisperToNobody);
                }
                for addressee in to {
                    if addressee == actor {
                        return Err(Refusal::WhisperToSelf(actor.clone()));
                    }
                    self.require_at(canon, addressee, &event.location)?;
                }
            }
        }
        let mut state = self.clone();
        state.tick = event.tick;
        for effect in &event.effects {
            state.apply(canon, event.kind, &event.location, effect)?;
        }
        Ok(Admitted {
            witnesses: self.witnesses(event),
            state,
        })
    }

    /// Who perceives `event`, judged on the state before it (spec §6): for a
    /// public event, everyone alive at its location and at the destination of
    /// a move; for a whisper, the addressees; for a hidden deed, no one else.
    /// The actor perceives their own event, and the characters an effect acts
    /// on perceive it unless it is hidden. The dead perceive nothing.
    fn witnesses(&self, event: &NewEvent) -> BTreeSet<CharacterKey> {
        let mut out = BTreeSet::new();
        match &event.visibility {
            Visibility::Public => {
                out.extend(self.present_at(&event.location));
                for effect in &event.effects {
                    if let Effect::Move { to, .. } | Effect::Place { to, .. } = effect {
                        out.extend(self.present_at(to));
                    }
                }
            }
            Visibility::Whisper { to } => out.extend(to.iter().cloned()),
            Visibility::Hidden => {}
        }
        if !matches!(event.visibility, Visibility::Hidden) {
            for effect in &event.effects {
                out.extend(effect.subjects().into_iter().cloned());
            }
        }
        out.extend(event.actor.iter().cloned());
        out.retain(|c| self.is_alive(c));
        out
    }

    /// Applies one effect, checking it against the state as the event's
    /// earlier effects left it.
    fn apply(
        &mut self,
        canon: &Canon,
        kind: EventKind,
        location: &LocationKey,
        effect: &Effect,
    ) -> Result<(), Refusal> {
        match effect {
            Effect::Move { character, to } => {
                self.require_at(canon, character, location)?;
                require_location(canon, to)?;
                if to == location {
                    return Err(Refusal::AlreadyThere {
                        character: character.clone(),
                        location: to.clone(),
                    });
                }
                if canon.route(location, to).is_none() {
                    return Err(Refusal::NoRoute {
                        from: location.clone(),
                        to: to.clone(),
                    });
                }
                self.positions.insert(character.clone(), to.clone());
            }
            Effect::Place { character, to } => {
                if !kind.may_place() {
                    return Err(Refusal::PlaceOutsideOpening(kind));
                }
                let from = self.require_alive(canon, character)?.clone();
                require_location(canon, to)?;
                if !canon.reachable(&from, to) {
                    return Err(Refusal::Unreachable {
                        from,
                        to: to.clone(),
                    });
                }
                self.positions.insert(character.clone(), to.clone());
            }
            Effect::Give { from, to, item } => {
                self.require_at(canon, from, location)?;
                self.require_at(canon, to, location)?;
                if from == to {
                    return Err(Refusal::GiveToSelf(from.clone()));
                }
                self.require_held(canon, item, from)?;
                self.items
                    .insert(item.clone(), Holder::Character(to.clone()));
            }
            Effect::Take { by, item, from } => {
                self.require_at(canon, by, location)?;
                require_item(canon, item)?;
                match from {
                    Some(owner) => {
                        self.require_at(canon, owner, location)?;
                        if owner == by {
                            return Err(Refusal::TakeFromSelf(by.clone()));
                        }
                        self.require_held(canon, item, owner)?;
                    }
                    None => {
                        if self.items.get(item) != Some(&Holder::Location(location.clone())) {
                            return Err(Refusal::NotHere {
                                item: item.clone(),
                                location: location.clone(),
                            });
                        }
                    }
                }
                self.items
                    .insert(item.clone(), Holder::Character(by.clone()));
            }
            Effect::Drop { by, item } => {
                self.require_at(canon, by, location)?;
                self.require_held(canon, item, by)?;
                self.items
                    .insert(item.clone(), Holder::Location(location.clone()));
            }
            Effect::Reveal { secret, to } => {
                if canon.secret_holder(secret).is_none() {
                    return Err(Refusal::UnknownSecret(secret.clone()));
                }
                self.require_at(canon, to, location)?;
                self.knows
                    .entry(to.clone())
                    .or_default()
                    .insert(secret.clone());
            }
            Effect::Injure {
                character,
                severity,
            } => {
                self.require_at(canon, character, location)?;
                self.injuries
                    .entry(character.clone())
                    .or_default()
                    .push(*severity);
            }
        }
        Ok(())
    }

    /// The character exists and is alive; their location.
    fn require_alive(&self, canon: &Canon, who: &CharacterKey) -> Result<&LocationKey, Refusal> {
        if canon.character(who).is_none() {
            return Err(Refusal::UnknownCharacter(who.clone()));
        }
        if !self.is_alive(who) {
            return Err(Refusal::Dead(who.clone()));
        }
        // Every character of the canon has a position from the initial state on.
        self.positions
            .get(who)
            .ok_or_else(|| Refusal::UnknownCharacter(who.clone()))
    }

    /// The character exists, is alive and is at `location`.
    fn require_at(
        &self,
        canon: &Canon,
        who: &CharacterKey,
        location: &LocationKey,
    ) -> Result<(), Refusal> {
        let at = self.require_alive(canon, who)?;
        if at != location {
            return Err(Refusal::Absent {
                character: who.clone(),
                location: location.clone(),
                actually: at.clone(),
            });
        }
        Ok(())
    }

    /// The item exists and `who` holds it.
    fn require_held(
        &self,
        canon: &Canon,
        item: &ItemKey,
        who: &CharacterKey,
    ) -> Result<(), Refusal> {
        require_item(canon, item)?;
        if self.items.get(item) != Some(&Holder::Character(who.clone())) {
            return Err(Refusal::NotHolding {
                character: who.clone(),
                item: item.clone(),
            });
        }
        Ok(())
    }

    /// Where the character is.
    pub fn position(&self, who: &CharacterKey) -> Option<&LocationKey> {
        self.positions.get(who)
    }

    /// Who is alive at `location`.
    pub fn present_at(&self, location: &LocationKey) -> BTreeSet<CharacterKey> {
        self.positions
            .iter()
            .filter(|(who, at)| *at == location && self.is_alive(who))
            .map(|(who, _)| who.clone())
            .collect()
    }

    /// Where the item is.
    pub fn holder(&self, item: &ItemKey) -> Option<&Holder> {
        self.items.get(item)
    }

    /// Whether the character knows the secret: it is on their sheet, or it
    /// was revealed to them.
    pub fn knows(&self, who: &CharacterKey, secret: &SecretKey) -> bool {
        self.knows.get(who).is_some_and(|s| s.contains(secret))
    }

    /// The character's injuries, oldest first.
    pub fn injuries(&self, who: &CharacterKey) -> &[Severity] {
        self.injuries.get(who).map_or(&[], Vec::as_slice)
    }

    /// Whether the character is alive: no fatal injury.
    pub fn is_alive(&self, who: &CharacterKey) -> bool {
        !self.injuries(who).contains(&Severity::Fatal)
    }

    /// Story time of the last event folded in.
    pub fn tick(&self) -> u32 {
        self.tick
    }
}

fn require_location(canon: &Canon, location: &LocationKey) -> Result<(), Refusal> {
    match canon.location(location) {
        Some(_) => Ok(()),
        None => Err(Refusal::UnknownLocation(location.clone())),
    }
}

fn require_item(canon: &Canon, item: &ItemKey) -> Result<(), Refusal> {
    if canon.has_item(item) {
        Ok(())
    } else {
        Err(Refusal::UnknownItem(item.clone()))
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;
