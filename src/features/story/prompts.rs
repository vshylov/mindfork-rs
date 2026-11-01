//! The simulation's role prompts, in English by the owner's decision
//! (docs/research/fabula-engine.md §8, §15): a character's intent, reaction
//! and reflection, and the World Master's two rulings of a round.
//!
//! - **Stable content first.** The system part holds the role's rules, the
//!   world's rules, the map and the sheets, which stay fixed for the story; the
//!   user part holds the moment. Providers with automatic prefix caching reuse
//!   the first (spec §8, *Rules shared by all contracts*).
//! - **A character sees what it saw** (spec §4, rule 5): its own sheet, the
//!   others' public descriptions, the observable parts of the events it
//!   witnessed on its branch path with its own private parts, its reflections,
//!   and the secrets revealed to it. Never another's secret, never the Author's
//!   intent.
//! - **The World Master sees the world, not the minds.** It gets the rules, the
//!   map, the whole cast with their secrets (a reveal is its to rule on), the
//!   folded state, the scene's events with their witnesses, the setup without
//!   the intent, and each proposal's deed and target; never a character's
//!   private thought, which the engine copies into the event itself.

use super::port::Prompt;
use super::scene::{bullets, listed, name_of, place_of};
use crate::entities::story::{
    Action, BranchId, Canon, CharacterKey, Effect, Event, Holder, LocationKey, Proposal,
    SceneSetup, Severity, Visibility, WorldState,
};
use crate::shared::storage::story::Reflection;

/// What a character's prompt is built from: the world at the moment, and what
/// the character perceived and reflected on the branch path.
pub struct CharacterView<'a> {
    /// The canon.
    pub canon: &'a Canon,
    /// The world as it stands.
    pub world: &'a WorldState,
    /// Who.
    pub who: &'a CharacterKey,
    /// The round.
    pub round: u32,
    /// The events the character perceived, in narrative order.
    pub memory: &'a [Event],
    /// The character's reflections on the path, oldest first.
    pub reflections: &'a [Reflection],
    /// Whether this round they break their routine (spec §7, step 1).
    pub breaks_routine: bool,
}

/// The rules of playing a character, before the story's content.
const PLAYING: &str = "How to play:\n\
- Act only on what you know: your own sheet, what you have perceived, the secrets you have \
learned, and your reflections. Never act on what others think or hide unless you have \
perceived it.\n\
- Keep your secrets unless you would choose to give one away; a secret you say aloud is heard \
by everyone who perceives it.\n\
- One action at a time, in your own voice. A whisper reaches only the characters you name, \
who must be with you; a hidden action is perceived by no one else.\n\
- To wait is to let the moment pass.";

/// The stable part of a character's prompt: how to play, the world's rules,
/// the map, their own sheet, and the others as everyone knows them.
pub fn character_system(canon: &Canon, who: &CharacterKey) -> String {
    let name = name_of(canon, who);
    let places: Vec<String> = canon
        .locations
        .iter()
        .map(|l| format!("{} (`{}`): {}", l.name, l.key, l.description))
        .collect();
    let others: Vec<String> = canon
        .characters
        .iter()
        .filter(|c| &c.key != who)
        .map(|c| format!("{} (`{}`): {}", name_of(canon, &c.key), c.key, c.public))
        .collect();
    let mut system = format!(
        "You play {name} in a story. Each round you decide what {name} does next, in \
         character; the world decides what comes of it.\n\n{PLAYING}\n\n\
         The world's rules:\n{}\n\nThe places:\n{}\n\n",
        bullets(canon.world.rules.iter().map(String::as_str)),
        bullets(places.iter().map(String::as_str)),
    );
    if let Some(me) = canon.character(who) {
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
    system
}

/// The items `holder` has, in the canon's order.
fn items_of<'a>(canon: &'a Canon, world: &WorldState, holder: &Holder) -> Vec<&'a str> {
    canon
        .starting_items()
        .filter(|(item, _)| world.holder(item) == Some(holder))
        .map(|(item, _)| item.as_str())
        .collect()
}

/// The moment as a character perceives it: where they are and with whom,
/// what they carry and what lies there, what they have learned and reflected,
/// and what they perceived.
fn moment(view: &CharacterView) -> String {
    let (canon, world, who) = (view.canon, view.world, view.who);
    let here = world.position(who);
    let around = here.map(|at| world.present_at(at)).unwrap_or_default();
    let with: Vec<&str> = canon
        .characters
        .iter()
        .filter(|c| &c.key != who && around.contains(&c.key))
        .map(|c| name_of(canon, &c.key))
        .collect();
    let carried = items_of(canon, world, &Holder::Character(who.clone()));
    let lying = here
        .map(|at| items_of(canon, world, &Holder::Location(at.clone())))
        .unwrap_or_default();
    let mut text = format!(
        "Round {}. You are at {}, with {}. You carry: {}. Lying here: {}.",
        view.round,
        here.map_or("an unknown place", |at| place_of(canon, at)),
        listed(&with, "no one else"),
        listed(&carried, "nothing"),
        listed(&lying, "nothing"),
    );
    let injuries = world.injuries(who);
    if !injuries.is_empty() {
        let hurt: Vec<&str> = injuries.iter().map(|s| severity(*s)).collect();
        text.push_str(&format!(" You are hurt: {}.", hurt.join(", ")));
    }
    let learned: Vec<String> = canon
        .characters
        .iter()
        .filter(|c| &c.key != who)
        .flat_map(|c| c.secrets.iter().map(move |s| (c, s)))
        .filter(|(_, s)| world.knows(who, &s.key))
        .map(|(c, s)| format!("{}'s secret: {}", name_of(canon, &c.key), s.text))
        .collect();
    if !learned.is_empty() {
        text.push_str(&format!(
            "\n\nSecrets of others you have learned:\n{}",
            bullets(learned.iter().map(String::as_str))
        ));
    }
    if !view.reflections.is_empty() {
        let lines: Vec<String> = view
            .reflections
            .iter()
            .map(|r| {
                if r.goal_changes.is_empty() {
                    r.body.clone()
                } else {
                    format!("{} (Your goals now: {})", r.body, r.goal_changes.join("; "))
                }
            })
            .collect();
        text.push_str(&format!(
            "\n\nYour reflections so far, oldest first:\n{}",
            bullets(lines.iter().map(String::as_str))
        ));
    }
    text.push_str(&format!(
        "\n\nWhat you have perceived, oldest first:\n{}",
        perceived(view)
    ));
    text
}

/// The character's memory: each perceived event's observable part, marked
/// when it reached them alone, with their own thought under their own deeds.
fn perceived(view: &CharacterView) -> String {
    let mut lines = Vec::new();
    for event in view.memory {
        let mark = match &event.body.visibility {
            Visibility::Whisper { .. } if event.body.actor.as_ref() != Some(view.who) => {
                "(Whispered to you) "
            }
            Visibility::Whisper { .. } => "(You whispered) ",
            Visibility::Hidden => "(Unseen by others) ",
            Visibility::Public => "",
        };
        lines.push(format!("- {mark}{}", event.body.observable));
        if event.body.actor.as_ref() == Some(view.who)
            && let Some(thought) = &event.body.private
        {
            lines.push(format!("  (You thought: {thought})"));
        }
    }
    if lines.is_empty() {
        "- nothing yet".to_owned()
    } else {
        lines.join("\n")
    }
}

/// The line that tells a character to break their routine this round.
fn routine(view: &CharacterView) -> String {
    if view.breaks_routine {
        format!(
            "\n\nThis round {} breaks their routine: they do something they would not usually \
             do, though it is still true to who they are.",
            name_of(view.canon, view.who)
        )
    } else {
        String::new()
    }
}

/// A character's intent for a round (spec §7, step 1).
pub fn character_propose(view: &CharacterView) -> Prompt {
    let name = name_of(view.canon, view.who);
    Prompt {
        system: character_system(view.canon, view.who),
        user: format!(
            "{}{}\n\nWhat does {name} do now? Write the inner thought first, then the action.",
            moment(view),
            routine(view),
        ),
    }
}

/// A character's reaction to the round's initiative, which they witnessed
/// (spec §7, step 3).
pub fn character_react(view: &CharacterView, initiative: &Event) -> Prompt {
    let name = name_of(view.canon, view.who);
    Prompt {
        system: character_system(view.canon, view.who),
        user: format!(
            "{}\n\nJust now: {}\n\nDoes {name} respond to it? Write the inner thought first, \
             then the action; wait if {name} lets it pass.",
            moment(view),
            initiative.body.observable,
        ),
    }
}

/// A character's reflection on the scene so far (spec §7, step 4).
pub fn character_reflect(view: &CharacterView) -> Prompt {
    let name = name_of(view.canon, view.who);
    Prompt {
        system: character_system(view.canon, view.who),
        user: format!(
            "{}\n\nThe scene pauses. In {name}'s inner voice, reflect on what has happened so \
             far: what {name} now believes, suspects or intends. Then list any goals {name} now \
             holds that differ from the goals on their sheet; leave the list empty if none \
             changed.",
            moment(view),
        ),
    }
}

// ---------------------------------------------------------------------------
// The World Master.

/// The rules of ruling, before the story's content.
const RULING: &str = "How to rule:\n\
- You never decide what a character wants or does; they propose, and you decide what happens.\n\
- An action is feasible when the world's rules, the places and routes, and what the actor \
holds and knows allow it. It is refused when it breaks a rule, needs an item the actor does \
not hold, goes where no direct route leads, or acts on a fact the actor has no way of knowing: \
judge that by the secrets they know and the events they perceived.\n\
- Write what is perceived of an action as an onlooker would perceive it: the words spoken, \
word for word, and the visible deed. Never a character's thoughts, motives or secrets, unless \
the action itself gives one away. A whisper is perceived only by those it is whispered to.\n\
- Give an action the effects it makes, and only those. When a secret is given away, add a \
reveal for every character who perceives it.\n\
- Refer to characters, places, items and secrets by their keys in rulings and effects.";

/// The stable part of the World Master's prompt: how to rule, the world's
/// rules, the map with its routes, the items, and the whole cast with their
/// secrets.
pub fn master_system(canon: &Canon) -> String {
    let places: Vec<String> = canon
        .locations
        .iter()
        .map(|l| {
            let routes: Vec<String> = l
                .routes
                .iter()
                .map(|r| format!("`{}` ({} tick{})", r.to, r.ticks, plural(r.ticks)))
                .collect();
            format!(
                "{} (`{}`): {} Routes to: {}.",
                l.name,
                l.key,
                l.description,
                listed(
                    &routes.iter().map(String::as_str).collect::<Vec<_>>(),
                    "none"
                ),
            )
        })
        .collect();
    let cast: Vec<String> = canon
        .characters
        .iter()
        .map(|c| {
            let secrets: Vec<String> = c
                .secrets
                .iter()
                .map(|s| format!("`{}`: {}", s.key, s.text))
                .collect();
            format!(
                "{} (`{}`): {} Goals: {}. Secrets: {}.",
                name_of(canon, &c.key),
                c.key,
                c.public,
                listed(
                    &c.goals.iter().map(String::as_str).collect::<Vec<_>>(),
                    "none"
                ),
                listed(
                    &secrets.iter().map(String::as_str).collect::<Vec<_>>(),
                    "none"
                ),
            )
        })
        .collect();
    let items: Vec<&str> = canon.starting_items().map(|(i, _)| i.as_str()).collect();
    format!(
        "You are the World Master of a story: the characters propose what they do, and you rule \
         on what happens.\n\n{RULING}\n\n\
         The world's rules:\n{}\n\nThe places and their one-way routes:\n{}\n\n\
         The items: {}.\n\nThe cast, with what only you and each of them know:\n{}",
        bullets(canon.world.rules.iter().map(String::as_str)),
        bullets(places.iter().map(String::as_str)),
        listed(&items, "none"),
        bullets(cast.iter().map(String::as_str)),
    )
}

fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn severity(s: Severity) -> &'static str {
    match s {
        Severity::Minor => "minor",
        Severity::Serious => "serious",
        Severity::Fatal => "fatal",
    }
}

/// What the World Master is shown of the moment: the scene, the world as it
/// stands, and the scene's events so far.
pub struct MasterView<'a> {
    /// The canon.
    pub canon: &'a Canon,
    /// The scene's circumstances, without the Author's intent.
    pub setup: &'a SceneSetup,
    /// The world as it stands.
    pub world: &'a WorldState,
    /// The branch.
    pub branch: &'a BranchId,
    /// The branch path's events, in narrative order.
    pub path: &'a [Event],
    /// The round.
    pub round: u32,
}

/// One proposal as the World Master is shown it: the deed, never the thought.
pub struct Shown<'a> {
    /// Who proposes.
    pub who: &'a CharacterKey,
    /// What.
    pub proposal: &'a Proposal,
    /// The events of the path they perceived.
    pub perceived: Vec<String>,
}

/// An event's label in the World Master's prompt: its ordinal on this branch,
/// `e3`, or its whole id when an earlier branch of the path committed it.
pub fn label(event: &Event, branch: &BranchId) -> String {
    match event.id.as_str().strip_prefix(&format!("{branch}/")) {
        Some(own) if &event.branch == branch => own.to_owned(),
        _ => event.id.as_str().to_owned(),
    }
}

fn effect_text(canon: &Canon, effect: &Effect) -> String {
    let n = |who: &CharacterKey| name_of(canon, who).to_owned();
    match effect {
        Effect::Move { character, to } | Effect::Place { character, to } => {
            format!("{} to {}", n(character), place_of(canon, to))
        }
        Effect::Give { from, to, item } => format!("{} gives {item} to {}", n(from), n(to)),
        Effect::Take { by, item, from } => match from {
            Some(owner) => format!("{} takes {item} from {}", n(by), n(owner)),
            None => format!("{} takes {item}", n(by)),
        },
        Effect::Drop { by, item } => format!("{} drops {item}", n(by)),
        Effect::Reveal { secret, to } => format!("{} learns `{secret}`", n(to)),
        Effect::Injure {
            character,
            severity: s,
        } => format!("{} is hurt ({})", n(character), severity(*s)),
    }
}

fn visibility_text(canon: &Canon, visibility: &Visibility) -> String {
    match visibility {
        Visibility::Public => "openly".to_owned(),
        Visibility::Whisper { to } => {
            let names: Vec<&str> = to.iter().map(|w| name_of(canon, w)).collect();
            format!("whispered to {}", listed(&names, "no one"))
        }
        Visibility::Hidden => "unseen by anyone else".to_owned(),
    }
}

/// The world, the scene and its events, as the World Master's two rulings
/// share them.
fn master_moment(view: &MasterView) -> String {
    let (canon, world) = (view.canon, view.world);
    let state: Vec<String> = canon
        .characters
        .iter()
        .map(|c| {
            let at = world
                .position(&c.key)
                .map_or("nowhere", |at| place_of(canon, at));
            let holds = items_of(canon, world, &Holder::Character(c.key.clone()));
            let knows: Vec<&str> = canon
                .characters
                .iter()
                .flat_map(|o| &o.secrets)
                .filter(|s| world.knows(&c.key, &s.key))
                .map(|s| s.key.as_str())
                .collect();
            let mut line = format!(
                "`{}` is at {at}; holds {}; knows {}",
                c.key,
                listed(&holds, "nothing"),
                listed(&knows, "no secret"),
            );
            if !world.is_alive(&c.key) {
                line.push_str("; dead");
            } else if !world.injuries(&c.key).is_empty() {
                let hurt: Vec<&str> = world
                    .injuries(&c.key)
                    .iter()
                    .map(|s| severity(*s))
                    .collect();
                line.push_str(&format!("; hurt: {}", hurt.join(", ")));
            }
            line
        })
        .collect();
    let lying: Vec<String> = canon
        .locations
        .iter()
        .filter_map(|l| {
            let items = items_of(canon, world, &Holder::Location(l.key.clone()));
            (!items.is_empty()).then(|| format!("at `{}`: {}", l.key, items.join(", ")))
        })
        .collect();
    let events: Vec<String> = view
        .path
        .iter()
        .map(|e| {
            let by = e
                .body
                .actor
                .as_ref()
                .map_or(String::new(), |a| format!(" by `{a}`"));
            let effects: Vec<String> = e
                .body
                .effects
                .iter()
                .map(|f| effect_text(canon, f))
                .collect();
            let witnesses: Vec<&str> = e.witnesses.iter().map(CharacterKey::as_str).collect();
            let mut line = format!(
                "{} · round {} · {}{by}, {}: {}",
                label(e, view.branch),
                e.body.round,
                e.body.kind.as_str(),
                visibility_text(canon, &e.body.visibility),
                e.body.observable,
            );
            if !effects.is_empty() {
                line.push_str(&format!(" [{}]", effects.join("; ")));
            }
            line.push_str(&format!(" Perceived by: {}.", listed(&witnesses, "no one")));
            line
        })
        .collect();
    let present: Vec<&str> = view
        .setup
        .present
        .iter()
        .map(|w| name_of(canon, w))
        .collect();
    format!(
        "The scene: {} ({}), with {}.\nWhat had just happened: {}\nThe pressure: {}\n\n\
         The world now, in round {}:\n{}\n{}\n\nWhat has happened, oldest first:\n{}",
        place_of(canon, &view.setup.location),
        view.setup.location,
        listed(&present, "no one"),
        view.setup.just_happened,
        view.setup.pressure,
        view.round,
        bullets(state.iter().map(String::as_str)),
        if lying.is_empty() {
            "- Nothing lies anywhere.".to_owned()
        } else {
            bullets(lying.iter().map(String::as_str))
        },
        if events.is_empty() {
            "- nothing yet".to_owned()
        } else {
            bullets(events.iter().map(String::as_str))
        },
    )
}

/// One proposal's deed as the World Master reads it.
fn deed(canon: &Canon, shown: &Shown) -> String {
    let p = shown.proposal;
    let what = match &p.action {
        Action::Speak { line } => format!("says \"{line}\""),
        Action::Move { to } => format!("goes to {} (`{to}`)", place_of(canon, to)),
        Action::Act { verb, object, line } => {
            let mut text = verb.clone();
            if let Some(object) = object {
                text.push(' ');
                text.push_str(object);
            }
            if let Some(line) = line {
                text.push_str(&format!(", saying \"{line}\""));
            }
            text
        }
        Action::Wait => "waits".to_owned(),
    };
    let targets: Vec<&str> = p.targets.iter().map(CharacterKey::as_str).collect();
    let aimed = if targets.is_empty() {
        String::new()
    } else {
        format!(", aimed at {}", targets.join(", "))
    };
    format!(
        "`{}` {what}{aimed}, {}. They perceived: {}.",
        shown.who,
        visibility_text(canon, &p.visibility),
        listed(
            &shown
                .perceived
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            "nothing yet"
        ),
    )
}

/// The World Master's ruling on a round's intents (spec §7, step 2).
pub fn master_initiative(view: &MasterView, intents: &[Shown]) -> Prompt {
    let listed_intents: Vec<String> = intents.iter().map(|s| deed(view.canon, s)).collect();
    let ask = if intents.is_empty() {
        "Every character waits this round. Return no rulings and no initiator; decide whether \
         the world does something, and whether the scene is over."
            .to_owned()
    } else {
        "Rule on every intent, pick the one feasible intent that moves the scene most as the \
         round's initiative, and write what is perceived of it and what it changes. Then decide \
         whether the world does something this round, and whether the scene is over."
            .to_owned()
    };
    Prompt {
        system: master_system(view.canon),
        user: format!(
            "{}\n\nThe intents this round:\n{}\n\n{ask}",
            master_moment(view),
            if listed_intents.is_empty() {
                "- none".to_owned()
            } else {
                bullets(listed_intents.iter().map(String::as_str))
            },
        ),
    }
}

/// The World Master's ruling on the reactions to the round's initiative
/// (spec §7, step 3).
pub fn master_reactions(view: &MasterView, initiative: &Event, reactions: &[Shown]) -> Prompt {
    let listed_reactions: Vec<String> = reactions.iter().map(|s| deed(view.canon, s)).collect();
    Prompt {
        system: master_system(view.canon),
        user: format!(
            "{}\n\nThe initiative this round was {}. The reactions to it:\n{}\n\n\
             Rule on every reaction, in the order they happen: commit those the world allows, \
             refuse those it does not, and write what is perceived of each committed one and \
             what it changes.",
            master_moment(view),
            label(initiative, view.branch),
            bullets(listed_reactions.iter().map(String::as_str)),
        ),
    }
}

/// Where a character is, for the engine's choice of who is in the scene.
pub fn present_here(canon: &Canon, world: &WorldState, here: &LocationKey) -> Vec<CharacterKey> {
    canon
        .characters
        .iter()
        .map(|c| c.key.clone())
        .filter(|who| world.position(who) == Some(here) && world.is_alive(who))
        .collect()
}

#[cfg(test)]
#[path = "prompts_tests.rs"]
mod tests;
