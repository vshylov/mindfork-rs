//! Secrets kept and secrets leaked (docs/research/fabula-engine.md §4, rule 5;
//! §11, *Secret-token leaks*; §13, Gate G1).
//!
//! The evaluation plants a unique token in every secret's text,
//! `[SECRET-MARA-7Q]`, so that a secret can be followed through the run by its
//! token. Two uses:
//!
//! - **The engine's own check** ([`unknown_secrets`]): an event whose
//!   observable carries a secret's token is refused unless every witness knows
//!   that secret once the event's effects apply — a secret given away is a
//!   `reveal`, never a silent copy (spec §8, *knowledge against witnesses*).
//! - **The audit** ([`audit`]), independent of that check: it reads every
//!   character call back from the call log and finds the tokens each one was
//!   shown or wrote that its character did not know at that moment of the
//!   branch. Gate G1 asks for none.
//!
//! A secret without a token cannot be followed this way; the tokens are the
//! evaluation's, and a story of a person's need not carry them.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::entities::story::{
    BranchId, Canon, Cause, CharacterKey, Effect, Event, EventKind, SecretKey, WorldState,
};

/// The prefix of a planted token.
const TOKEN_PREFIX: &str = "SECRET-";

/// The token planted in a secret's text: the inside of a bracketed run that
/// starts with `SECRET-`, as `SECRET-MARA-7Q`.
pub fn token_of(text: &str) -> Option<&str> {
    let start = text.find(&format!("[{TOKEN_PREFIX}"))? + 1;
    let end = start + text[start..].find(']')?;
    let token = &text[start..end];
    (token.len() > TOKEN_PREFIX.len()).then_some(token)
}

/// Every secret of the canon whose token `text` carries, compared without
/// regard to case, in the canon's order.
pub fn secrets_in<'a>(canon: &'a Canon, text: &str) -> Vec<&'a SecretKey> {
    let haystack = text.to_uppercase();
    canon
        .characters
        .iter()
        .flat_map(|c| &c.secrets)
        .filter(|s| token_of(&s.text).is_some_and(|t| haystack.contains(&t.to_uppercase())))
        .map(|s| &s.key)
        .collect()
}

/// The secrets whose tokens `text` carries that `who` does not know in
/// `world`.
pub fn unknown_secrets(
    canon: &Canon,
    world: &WorldState,
    who: &CharacterKey,
    text: &str,
) -> Vec<SecretKey> {
    secrets_in(canon, text)
        .into_iter()
        .filter(|s| !world.knows(who, s))
        .cloned()
        .collect()
}

/// When in a round a call or an event happens: a character's intent, the
/// initiative and any world event, the reactions to it, the reactions'
/// events, then the reflections (spec §7, *One simulation round*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// The opening events, before round 1.
    Opening,
    /// The characters' intents.
    Propose,
    /// The initiative's event and the world's.
    Initiative,
    /// The witnesses' reactions.
    React,
    /// The reactions' events.
    Reaction,
    /// The reflections.
    Reflect,
}

impl Phase {
    /// The phase of a character call's act, as its address names it.
    pub fn of_act(act: &str) -> Option<Self> {
        match act {
            "propose" => Some(Phase::Propose),
            "react" => Some(Phase::React),
            "reflect" => Some(Phase::Reflect),
            _ => None,
        }
    }

    /// The phase of a committed event.
    fn of_event(event: &Event) -> Self {
        match (&event.body.cause, event.body.kind) {
            (_, EventKind::Opening) | (Cause::Setup, _) => Phase::Opening,
            (Cause::Reaction { .. }, _) => Phase::Reaction,
            _ => Phase::Initiative,
        }
    }
}

/// A moment of a branch: its round and the phase within it.
type Moment = (u32, Phase);

/// A token a character was shown, or wrote, before they knew its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    /// The call's address.
    pub address: String,
    /// Whose call.
    pub character: CharacterKey,
    /// The secret whose token it carried.
    pub secret: SecretKey,
    /// Where the token was: `request` or `reply`.
    pub place: &'static str,
}

impl std::fmt::Display for Leak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} saw `{}` in its {} before knowing it",
            self.address, self.character, self.secret, self.place
        )
    }
}

/// From when each character knows each secret on one branch: their own from
/// the start, one revealed on the path before the branch forked from the
/// start too, and one revealed on the branch itself from the moment of its
/// event.
fn knowledge(
    canon: &Canon,
    branch: &BranchId,
    path: &[Event],
) -> BTreeMap<(CharacterKey, SecretKey), Moment> {
    let mut known = BTreeMap::new();
    let start = (0, Phase::Opening);
    for character in &canon.characters {
        for secret in &character.secrets {
            known.insert((character.key.clone(), secret.key.clone()), start);
        }
    }
    for event in path {
        let at = if &event.branch == branch {
            (event.body.round, Phase::of_event(event))
        } else {
            start
        };
        for effect in &event.body.effects {
            if let Effect::Reveal { secret, to } = effect {
                known.entry((to.clone(), secret.clone())).or_insert(at);
            }
        }
    }
    known
}

/// The parts of a character call's address: its branch, round, character and
/// act, from `{branch}/round{n}/{who}/{act}`.
fn character_call(address: &str) -> Option<(BranchId, u32, CharacterKey, Phase)> {
    let (branch, rest) = address.rsplit_once("/round")?;
    let mut parts = rest.splitn(3, '/');
    let round = parts.next()?.parse().ok()?;
    let who = parts.next()?;
    let phase = Phase::of_act(parts.next()?)?;
    Some((BranchId::new(branch), round, CharacterKey::new(who), phase))
}

/// One recorded call as the audit reads it.
#[derive(Debug, Clone, Copy)]
pub struct AuditedCall<'a> {
    /// Its address.
    pub address: &'a str,
    /// Its role, as the call log records it.
    pub role: &'a str,
    /// The request as sent.
    pub request: &'a Value,
    /// The reply, when there was one.
    pub response: Option<&'a Value>,
}

/// Every token a character call was shown or wrote before its character knew
/// the secret, given the calls and each branch's path. A call that names a
/// branch with no path given, or that is not a character's, is not audited.
pub fn audit(
    canon: &Canon,
    paths: &BTreeMap<BranchId, Vec<Event>>,
    calls: &[AuditedCall],
) -> Vec<Leak> {
    let known: BTreeMap<_, _> = paths
        .iter()
        .map(|(branch, path)| (branch, knowledge(canon, branch, path)))
        .collect();
    let mut leaks = Vec::new();
    for call in calls.iter().filter(|c| c.role == "character") {
        let Some((branch, round, who, phase)) = character_call(call.address) else {
            continue;
        };
        let Some(known) = known.get(&branch) else {
            continue;
        };
        let reply = call
            .response
            .and_then(|r| r.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        for (place, text) in [
            ("request", call.request.to_string()),
            ("reply", reply.to_owned()),
        ] {
            for secret in secrets_in(canon, &text) {
                let since = known.get(&(who.clone(), secret.clone()));
                if since.is_none_or(|&since| since >= (round, phase)) {
                    leaks.push(Leak {
                        address: call.address.to_owned(),
                        character: who.clone(),
                        secret: secret.clone(),
                        place,
                    });
                }
            }
        }
    }
    leaks
}

#[cfg(test)]
#[path = "leaks_tests.rs"]
mod tests;
