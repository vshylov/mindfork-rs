//! The writer's prompts' shared parts (docs/research/fabula-engine.md §15;
//! docs/history/fabula.md §6, T21): the canon as the writer reads it and the rules of
//! the prose, in English by the owner's decision. The writer sees the whole
//! canon, the arc and every secret included, but never a planted token: a
//! secret's text is shown without its `[SECRET-…]` marker, so the prose
//! cannot copy one. The frozen premises carry them.

use crate::entities::story::{Canon, CharacterKey};

/// The language the prose is written in.
pub fn story_language(canon: &Canon) -> &str {
    match canon.style.language.trim() {
        "" => "English",
        language => language,
    }
}

/// The canon as the Author reads it: the world's rules, the places and their
/// routes, the items, the whole cast with their goals and secrets, the style,
/// the premise and the arc. M3's B0 is given it too (docs/history/fabula.md §6, T21).
pub fn canon_brief(canon: &Canon) -> String {
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
        "The world's rules:\n{}\n\nThe places and their one-way routes:\n{}\n\n\
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

/// A secret is the scene's to spend, not the narrator's (T19's pilot): told
/// from its keeper, the prose gave away the theft before the confession.
const SECRETS: &str = "A secret enters the prose when a scene spends it, not before: a scene \
told from its keeper shows their fear and what they do to hide it, never the secret itself, \
until it comes out in a deed or a line.";

/// A talk is shown, not reported (T19's pilot): rounds of questions came out
/// as "he asked whether…, she answered that…".
const TALK: &str = "Never retell a talk in indirect speech (he asked whether…, she answered \
that…): keep the one or two lines that turn the scene, in direct speech, with what the speakers \
do, and leave the rest unsaid.";

/// How the prose is to be written: language, point of view, tense, length,
/// and the rules of the prose itself (secrets, talk), every writer's, as T21
/// has it; the writer has no record, so it invents what happens.
pub fn writing_rules(canon: &Canon, words: (u32, u32)) -> String {
    let telling = "Invent what happens, true to the canon and the cast, and never let a \
                   character know what they have not perceived in your story.";
    format!(
        "Write in {}, in {}, in the {} tense, between {} and {} words. {telling} \
         {SECRETS} {TALK} Keep each character's voice. In the dialogue, the characters act on a \
         plan or a boundary rather than announce it, open no reply on thanks, apology or \
         agreement the moment does not call for, and show a want in a deed rather than in a line \
         that names it. The canon's \
         keys are not the story's words: never write a key in backticks. Begin with the \
         chapter's title as a level-two markdown heading (`## `). Reply with the chapter's text \
         and nothing else.",
        story_language(canon),
        or_unset(&canon.style.pov),
        or_unset(&canon.style.tense),
        words.0,
        words.1,
    )
}

/// A revision's names (docs/history/fabula.md §4, *The last version checked*, F4):
/// each writing call puts the canon's names into the story's language anew,
/// and Haiku's last revision on `villain-4` gave Pavel a second patronymic.
pub const KEEP_NAMES: &str = "Keep every name, patronymic and form of address as the chapter \
    has them, unless a note asks to change one.";

/// A text's words as the length rule counts them (F2): a run between spaces
/// with a letter or a digit in it, so a dialogue's dash and a heading's `##`
/// are not words.
pub fn words_of(text: &str) -> u32 {
    let n = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A character's name in the story's text; the key when the sheet gives none.
fn name_of<'a>(canon: &'a Canon, who: &'a CharacterKey) -> &'a str {
    canon
        .character(who)
        .map(|c| c.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(who.as_str())
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

fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The prefix of a planted token.
const TOKEN_PREFIX: &str = "SECRET-";

/// `text` without its planted tokens: each bracketed `[SECRET-…]` run is
/// cut, with the space before it. What the Author and the reviewers are shown
/// of a secret, so the prose never learns the evaluation's markers.
fn without_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&format!("[{TOKEN_PREFIX}")) {
        let Some(len) = rest[at..].find(']') else {
            break;
        };
        out.push_str(rest[..at].trim_end_matches(' '));
        rest = &rest[at + len + 1..];
    }
    out.push_str(rest);
    out
}
