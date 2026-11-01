//! The Author's and the reviewers' prompts, in English by the owner's decision
//! (docs/research/fabula-engine.md §7, §8, §15; docs/fabula.md §4, PR 6).
//!
//! - **The Author sees everything.** The whole canon, the arc included, every
//!   secret, and the full trace of every branch it reads: each event's
//!   observable part, its actor's private thought, its effects and its
//!   witnesses, and the characters' reflections (spec §4, rule 5). Never the
//!   planted tokens: a secret's text is shown without its marker
//!   ([`leaks::without_tokens`]), so the prose cannot copy one.
//! - **The Reader sees only the published text** and the new chapter, nothing
//!   of the plan or the record (spec §8).
//! - **The Continuity checker sees the record** of the kept scenes with every
//!   event's id, the canon, and what each character knew when the chapter
//!   opened, so a finding can cite its evidence.
//! - **The human's language** is the language of what the human reads: a
//!   pick's note, an assumption, a chapter's summary (spec §8, §15). The prose
//!   is in the story's own language, `Style::language`.

use super::leaks::without_tokens;
use super::port::Prompt;
use super::prompts::{effect_text, items_of, plural, severity, visibility_text};
use super::scene::{bullets, listed, name_of, place_of};
use crate::entities::story::{
    BranchId, Canon, CharacterKey, Decided, Event, EventKind, Holder, ReviewNote, SceneSetup,
    WorldState,
};
use crate::shared::storage::story::Reflection;

/// The language the prose is written in.
pub fn story_language(canon: &Canon) -> &str {
    match canon.style.language.trim() {
        "" => "English",
        language => language,
    }
}

/// A branch as the Author reads it: its own events, and the reflections
/// written on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    /// The branch.
    pub branch: BranchId,
    /// Its own events, in narrative order: the opening first.
    pub events: Vec<Event>,
    /// The reflections written on it, by character in the canon's order.
    pub reflections: Vec<(CharacterKey, Reflection)>,
}

/// A scene the chapter kept: its circumstances, the Author's intent, and the
/// branch the story keeps.
pub struct Kept<'a> {
    /// The scene's number in the chapter.
    pub scene: u32,
    /// Its circumstances.
    pub setup: &'a SceneSetup,
    /// The Author's intent for it.
    pub intent: &'a str,
    /// The branch picked.
    pub trace: &'a Trace,
}

/// What every prompt of a chapter shares.
pub struct ChapterContext<'a> {
    /// The canon.
    pub canon: &'a Canon,
    /// The chapter, from 1.
    pub number: u32,
    /// The language of what the human reads.
    pub human_language: &'a str,
    /// The earlier chapters' summaries, by number.
    pub summaries: &'a [(u32, String)],
    /// The previous chapter as it was accepted.
    pub previous: Option<&'a str>,
    /// What the human asked of this chapter at the last checkpoint.
    pub human_notes: Option<&'a str>,
    /// The chapter's goals; empty before its outline.
    pub goals: &'a [String],
}

impl ChapterContext<'_> {
    fn total(&self) -> usize {
        self.canon.arc.chapters.len()
    }

    fn arc_goal(&self) -> &str {
        self.canon
            .arc
            .chapters
            .get(self.number as usize - 1)
            .map_or("none set", |c| c.goal.as_str())
    }

    /// The story so far, the previous chapter, and the human's notes, as
    /// far as there are any.
    fn so_far(&self) -> String {
        let mut text = String::new();
        if !self.summaries.is_empty() {
            let lines: Vec<String> = self
                .summaries
                .iter()
                .map(|(n, s)| format!("Chapter {n}: {s}"))
                .collect();
            text.push_str(&format!(
                "\n\nThe story so far, a summary per chapter:\n{}",
                bullets(lines.iter().map(String::as_str))
            ));
        }
        if let Some(previous) = self.previous {
            text.push_str(&format!(
                "\n\nThe previous chapter as published:\n\n{previous}"
            ));
        }
        if let Some(notes) = self.human_notes.filter(|n| !n.trim().is_empty()) {
            text.push_str(&format!(
                "\n\nWhat the story's human author asked of this chapter:\n{notes}"
            ));
        }
        text
    }

    fn goals_text(&self) -> String {
        bullets(self.goals.iter().map(String::as_str))
    }
}

/// The rules of authoring, before the story's content.
const AUTHORING: &str = "How to author:\n\
- You control circumstances; the characters control their choices. A scene's setup says who \
is where, what has just happened and what pressure the world applies, never what a character \
will do or feel.\n\
- Every scene is played several times from the same moment, and the versions differ. You read \
them all and keep the one that serves the chapter best.\n\
- What happened is what the record says. In the prose you may reorder, compress, skip and \
change point of view, but never contradict the record, and a character knows only what they \
have perceived.\n\
- Refer to characters and places by their keys in setups.";

/// The stable part of every Author prompt: how to author, the world, the
/// whole cast with their secrets, the style and the arc.
pub fn author_system(canon: &Canon, human_language: &str) -> String {
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
                .map(|s| format!("`{}`: {}", s.key, without_tokens(&s.text)))
                .collect();
            format!(
                "{} (`{}`): {} Voice: {} Goals: {}. Secrets: {}.",
                name_of(canon, &c.key),
                c.key,
                c.public,
                c.voice,
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
    let arc: Vec<String> = canon
        .arc
        .chapters
        .iter()
        .enumerate()
        .map(|(i, c)| format!("Chapter {}: {}", i + 1, c.goal))
        .collect();
    format!(
        "You are the Author of a story told in chapters. Other players play its characters and \
         choose what they do; a World Master rules on what happens and writes it into the \
         story's record. You set the circumstances of each scene, keep one of the versions the \
         world played of it, and write the chapters.\n\n{AUTHORING}\n\
         - What the story's human author reads from you (a note on a pick, a doubt you met, a \
         chapter's summary) is written in {human_language}.\n\n\
         The world's rules:\n{}\n\nThe places and their one-way routes:\n{}\n\n\
         The items: {}.\n\nThe cast, with what only you and each of them know:\n{}\n\n\
         How the story is told: point of view {}; {} tense; written in {}.\n\n\
         The premise: {}\n\nThe arc, a goal per chapter:\n{}",
        bullets(canon.world.rules.iter().map(String::as_str)),
        bullets(places.iter().map(String::as_str)),
        listed(&items, "none"),
        bullets(cast.iter().map(String::as_str)),
        or_unset(&canon.style.pov),
        or_unset(&canon.style.tense),
        story_language(canon),
        canon.arc.premise,
        bullets(arc.iter().map(String::as_str)),
    )
}

fn or_unset(value: &str) -> &str {
    if value.trim().is_empty() {
        "of your choosing"
    } else {
        value
    }
}

/// The chapter's outline (spec §7, step 1).
pub fn outline(ctx: &ChapterContext) -> Prompt {
    Prompt {
        system: author_system(ctx.canon, ctx.human_language),
        user: format!(
            "Chapter {} of {}. Its goal in the arc: {}{}\n\n\
             Set this chapter's goals: what must have changed by its end, two to four goals. Do \
             not plan its scenes yet; they are planned one at a time, each after the one before \
             it has been played.",
            ctx.number,
            ctx.total(),
            ctx.arc_goal(),
            ctx.so_far(),
        ),
    }
}

/// Each character's place, belongings, knowledge and health, as the Author
/// and the Continuity checker read the world.
fn world_lines(canon: &Canon, world: &WorldState) -> String {
    let lines: Vec<String> = canon
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
                "{} (`{}`) is at {at}; holds {}; knows {}",
                name_of(canon, &c.key),
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
    bullets(lines.iter().map(String::as_str))
}

/// One event of a trace: what was perceived, by whom, with its effects and
/// the actor's private thought; with `id`, its id first.
fn event_line(canon: &Canon, event: &Event, id: bool) -> String {
    let who = event
        .body
        .actor
        .as_ref()
        .map_or("The world", |a| name_of(canon, a));
    let mut line = if event.body.kind == EventKind::Opening {
        format!("Opening: {}", without_tokens(&event.body.observable))
    } else {
        format!(
            "Round {} · {who}, {}: {}",
            event.body.round,
            visibility_text(canon, &event.body.visibility),
            without_tokens(&event.body.observable),
        )
    };
    if id {
        line = format!("`{}` {line}", event.id);
    }
    let effects: Vec<String> = event
        .body
        .effects
        .iter()
        .map(|f| effect_text(canon, f))
        .collect();
    if !effects.is_empty() {
        line.push_str(&format!(" [{}]", effects.join("; ")));
    }
    let witnesses: Vec<&str> = event.witnesses.iter().map(|w| name_of(canon, w)).collect();
    line.push_str(&format!(" Perceived by: {}.", listed(&witnesses, "no one")));
    if let Some(thought) = &event.body.private {
        line.push_str(&format!(" ({who} thought: {})", without_tokens(thought)));
    }
    line
}

/// A branch's trace: its events, then its reflections.
fn trace_text(canon: &Canon, trace: &Trace, ids: bool) -> String {
    let mut lines: Vec<String> = trace
        .events
        .iter()
        .map(|e| event_line(canon, e, ids))
        .collect();
    for (who, r) in &trace.reflections {
        let mut line = format!(
            "{} reflected, privately: {}",
            name_of(canon, who),
            without_tokens(&r.body)
        );
        if !r.goal_changes.is_empty() {
            line.push_str(&format!(" (Goals now: {})", r.goal_changes.join("; ")));
        }
        lines.push(line);
    }
    bullets(lines.iter().map(String::as_str))
}

fn setup_text(canon: &Canon, setup: &SceneSetup) -> String {
    let present: Vec<&str> = setup.present.iter().map(|w| name_of(canon, w)).collect();
    format!(
        "at {} (`{}`), with {}. What had just happened: {} The pressure: {}",
        place_of(canon, &setup.location),
        setup.location,
        listed(&present, "no one"),
        setup.just_happened,
        setup.pressure,
    )
}

/// How the kept scenes are shown: with the events' ids for the Continuity
/// checker, with the Author's intents for the Author alone.
#[derive(Clone, Copy)]
struct Showing {
    ids: bool,
    intents: bool,
}

const TO_AUTHOR: Showing = Showing {
    ids: false,
    intents: true,
};

const TO_CHECKER: Showing = Showing {
    ids: true,
    intents: false,
};

/// The kept scenes: each one's setup, the intent where the reader may see
/// it, and the trace.
fn kept_text(canon: &Canon, kept: &[Kept], showing: Showing) -> String {
    kept.iter()
        .map(|k| {
            let intent = if showing.intents {
                format!("\nYour intent for it: {}", k.intent)
            } else {
                String::new()
            };
            format!(
                "Scene {}, {}{intent}\nWhat happened:\n{}",
                k.scene,
                setup_text(canon, k.setup),
                trace_text(canon, k.trace, showing.ids),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The plan of the chapter's next scene, or its end (spec §7, step 2). With
/// `notes`, the human asked for this scene to be planned again.
pub fn plan(
    ctx: &ChapterContext,
    kept: &[Kept],
    world: &WorldState,
    (scene, most): (u32, u32),
    notes: Option<&str>,
) -> Prompt {
    let kept_so_far = if kept.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nThe scenes this chapter has kept so far:\n\n{}",
            kept_text(ctx.canon, kept, TO_AUTHOR)
        )
    };
    let again = notes
        .filter(|n| !n.trim().is_empty())
        .map_or(String::new(), |n| {
            format!(
                "\n\nThis scene failed on every attempt, and the story's human author asked for it \
             to be planned again: {n}"
            )
        });
    let ask = if scene == 1 {
        format!(
            "Plan scene 1 of at most {most}: first your private intent for it, then its \
             circumstances."
        )
    } else if scene == most {
        format!(
            "Plan scene {scene}, the chapter's last, or end the chapter here if its goals are \
             met: first your private intent, then the circumstances."
        )
    } else {
        format!(
            "Plan scene {scene} of at most {most}: first your private intent for it, then its \
             circumstances. Or end the chapter here if its goals are met."
        )
    };
    Prompt {
        system: author_system(ctx.canon, ctx.human_language),
        user: format!(
            "Chapter {}. Its goals:\n{}{}{kept_so_far}\n\nThe world now:\n{}{again}\n\n{ask}",
            ctx.number,
            ctx.goals_text(),
            ctx.so_far(),
            world_lines(ctx.canon, world),
        ),
    }
}

/// The pick among a scene's branches (spec §7, step 4), each shown under its
/// label (`k0`).
pub fn pick(
    ctx: &ChapterContext,
    (scene, setup, intent): (u32, &SceneSetup, &str),
    shown: &[(String, &Trace)],
    reruns_left: u32,
) -> Prompt {
    let versions: Vec<String> = shown
        .iter()
        .map(|(label, trace)| format!("Version {label}:\n{}", trace_text(ctx.canon, trace, false)))
        .collect();
    let choose = if reruns_left == 0 {
        "No reruns are left: keep the best of these.".to_owned()
    } else {
        format!(
            "Then keep one version for the story, or ask for the scene again under changed \
             circumstances ({reruns_left} rerun{} left).",
            plural(reruns_left)
        )
    };
    Prompt {
        system: author_system(ctx.canon, ctx.human_language),
        user: format!(
            "Chapter {}. Its goals:\n{}\n\nScene {scene}, {}\nYour intent for it: {intent}\n\n\
             The world played it {} time{} from the same moment; each version below is what \
             happened in it.\n\n{}\n\n\
             Read each version against your intent and four measures: tension, surprise, truth \
             to character and consequences, each scored from 1 to 5. {choose}",
            ctx.number,
            ctx.goals_text(),
            setup_text(ctx.canon, setup),
            shown.len(),
            plural(shown.len() as u32),
            versions.join("\n\n"),
        ),
    }
}

/// How the prose is to be written: language, point of view, tense, length,
/// and the record's terms kept out of it.
fn writing_rules(canon: &Canon, words: (u32, u32)) -> String {
    format!(
        "Write in {}, in {}, in the {} tense, between {} and {} words. Tell what the record \
         holds: you may reorder, compress, skip and change point of view, but never contradict \
         it, and never let a character know what they have not perceived. Keep each \
         character's voice. The record's keys are not the story's words: never write a key in \
         backticks. Begin with the chapter's title as a level-two markdown heading (`## `). \
         Reply with the chapter's text and nothing else.",
        story_language(canon),
        or_unset(&canon.style.pov),
        or_unset(&canon.style.tense),
        words.0,
        words.1,
    )
}

/// The chapter's first draft (spec §7, step 6).
pub fn write(ctx: &ChapterContext, kept: &[Kept], words: (u32, u32)) -> Prompt {
    Prompt {
        system: author_system(ctx.canon, ctx.human_language),
        user: format!(
            "Write chapter {} of {}. Its goal in the arc: {}\nIts goals:\n{}{}\n\n\
             The scenes the story keeps, in order, as the record holds them:\n\n{}\n\n{}",
            ctx.number,
            ctx.total(),
            ctx.arc_goal(),
            ctx.goals_text(),
            ctx.so_far(),
            kept_text(ctx.canon, kept, TO_AUTHOR),
            writing_rules(ctx.canon, words),
        ),
    }
}

/// A note as the Author and the reviser read it.
fn note_line(label: &str, note: &ReviewNote) -> String {
    let n = &note.note;
    let anchor = n
        .anchor
        .as_deref()
        .map_or(String::new(), |a| format!(" \"{a}\":"));
    let evidence = if n.evidence.is_empty() {
        String::new()
    } else {
        let ids: Vec<&str> = n.evidence.iter().map(|e| e.as_str()).collect();
        format!(" (events: {})", ids.join(", "))
    };
    format!(
        "{label} · {} · {}, {}:{anchor} {}{evidence}",
        note.reviewer.as_str(),
        n.kind.as_str(),
        n.severity.as_str(),
        n.body,
    )
}

/// A revision of the chapter (spec §7, step 8): the notes the Author
/// accepted, or the human's at the checkpoint.
pub fn revise(
    ctx: &ChapterContext,
    kept: &[Kept],
    text: &str,
    accepted: &[(&ReviewNote, &Decided)],
    words: (u32, u32),
) -> Prompt {
    let notes: Vec<String> = accepted
        .iter()
        .enumerate()
        .map(|(i, (note, decided))| {
            format!(
                "{} (your critique: {})",
                note_line(&format!("n{}", i + 1), note),
                decided.reason
            )
        })
        .collect();
    let asked = if notes.is_empty() {
        "Revise it as the story's human author asked above.".to_owned()
    } else {
        format!(
            "The notes on it you accepted:\n{}\n\nRevise it, acting on every one of them.",
            bullets(notes.iter().map(String::as_str))
        )
    };
    Prompt {
        system: author_system(ctx.canon, ctx.human_language),
        user: format!(
            "Chapter {} of {}. Its goal in the arc: {}\nIts goals:\n{}{}\n\n\
             The scenes the story keeps, as the record holds them:\n\n{}\n\n\
             The chapter as it stands:\n\n{text}\n\n{asked} Keep what works as it is, and \
             rewrite the whole chapter. {}",
            ctx.number,
            ctx.total(),
            ctx.arc_goal(),
            ctx.goals_text(),
            ctx.so_far(),
            kept_text(ctx.canon, kept, TO_AUTHOR),
            writing_rules(ctx.canon, words),
        ),
    }
}

/// The final version's summary, for the human and for the chapters after.
pub fn summary(ctx: &ChapterContext, text: &str) -> Prompt {
    Prompt {
        system: author_system(ctx.canon, ctx.human_language),
        user: format!(
            "Chapter {} as it stands:\n\n{text}\n\n\
             Summarize it in {} for the story's human author, in three to six sentences: what \
             happens, and who learns what. Reply with the summary and nothing else.",
            ctx.number, ctx.human_language,
        ),
    }
}

/// The Author's decisions on a version's notes, each shown under its label
/// (`n1`).
pub fn triage(ctx: &ChapterContext, text: &str, shown: &[(String, &ReviewNote)]) -> Prompt {
    let notes: Vec<String> = shown.iter().map(|(l, n)| note_line(l, n)).collect();
    Prompt {
        system: author_system(ctx.canon, ctx.human_language),
        user: format!(
            "Chapter {} as it stands:\n\n{text}\n\nThe reviewers' notes on it:\n{}\n\n\
             Decide each note: accepted when your revision will act on it, rejected when it is \
             wrong or not worth acting on, deferred when it belongs to a later chapter. A note \
             the record shows to be wrong is rejected.",
            ctx.number,
            bullets(notes.iter().map(String::as_str)),
        ),
    }
}

/// The Reader's prompt (spec §8): the published chapters and the new one,
/// nothing else.
pub fn reader(published: &[(u32, String)], text: &str) -> Prompt {
    let before = if published.is_empty() {
        "This is the story's first chapter.".to_owned()
    } else {
        let chapters: Vec<String> = published
            .iter()
            .map(|(n, t)| format!("Chapter {n}:\n\n{t}"))
            .collect();
        format!(
            "The chapters published so far:\n\n{}",
            chapters.join("\n\n")
        )
    };
    Prompt {
        system: "You are a reader of a story published a chapter at a time. You know only what \
                 is published: no plan, no notes, nothing of how it was made. Read as a reader \
                 reads, and say plainly where you were lost and where you were bored, what you \
                 expect to happen next, and what you liked best. Quote the passages you mean."
            .to_owned(),
        user: format!(
            "{before}\n\nThe new chapter:\n\n{text}\n\nWrite your notes on the new chapter."
        ),
    }
}

/// The Continuity checker's prompt (spec §8): the canon, what the cast knew
/// when the chapter opened, the kept scenes' record with every event's id,
/// and the chapter.
pub fn continuity(canon: &Canon, opened: &WorldState, kept: &[Kept], text: &str) -> Prompt {
    let cast: Vec<String> = canon
        .characters
        .iter()
        .map(|c| {
            let secrets: Vec<String> = c
                .secrets
                .iter()
                .map(|s| format!("`{}`: {}", s.key, without_tokens(&s.text)))
                .collect();
            format!(
                "{} (`{}`): {} Secrets: {}.",
                name_of(canon, &c.key),
                c.key,
                c.public,
                listed(
                    &secrets.iter().map(String::as_str).collect::<Vec<_>>(),
                    "none"
                ),
            )
        })
        .collect();
    let places: Vec<String> = canon
        .locations
        .iter()
        .map(|l| format!("{} (`{}`): {}", l.name, l.key, l.description))
        .collect();
    Prompt {
        system: format!(
            "You check a chapter of a story against the record of what happened in its world, \
             and against its canon. The record is the truth. The chapter may reorder, compress, \
             skip and change point of view, but it may not contradict the record, and a \
             character in it knows only what they have perceived: a secret only once it was \
             revealed to them. Report each place where the chapter departs from the record or \
             the canon, with the ids of the events that show it. Style and pace are not yours \
             to judge.\n\nThe world's rules:\n{}\n\nThe places:\n{}\n\nThe cast:\n{}",
            bullets(canon.world.rules.iter().map(String::as_str)),
            bullets(places.iter().map(String::as_str)),
            bullets(cast.iter().map(String::as_str)),
        ),
        user: format!(
            "When the chapter opened:\n{}\n\nThe record of its scenes, in order:\n\n{}\n\n\
             The chapter:\n\n{text}\n\nReport every departure.",
            world_lines(canon, opened),
            kept_text(canon, kept, TO_CHECKER),
        ),
    }
}

#[cfg(test)]
#[path = "author_prompts_tests.rs"]
mod tests;
