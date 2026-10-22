//! A character card mapped onto a new profile (docs/research/companion-platform.md
//! C6, spec §10): the card's persona fields assembled into one system message
//! in SillyTavern's order, its greetings, its name. The macros stay macros —
//! substituted when a request is built and a chat created
//! ([`crate::features::persona_macros`]) — so renaming the character or the
//! person later still applies.

use crate::entities::profile::{CharacterNames, CompanionFeatures, Profile};
use crate::features::tools::default_tool_ids;
use crate::shared::i18n::{Lang, Locale, locale};

use super::Card;

/// What the import did with the parts of a card that have no exact home in a
/// profile — printed before the profile is saved, so nothing is lost silently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Lorebook entries that join the system message (always on).
    pub lore_kept: usize,
    /// Lorebook entries triggered by keywords — not imported (F7 (a)).
    pub lore_keyed: usize,
    /// Example exchanges placed in the system message's example block.
    pub examples: usize,
    /// The card's `post_history_instructions`, placed at the system message's
    /// end rather than after the conversation (no insert there, F11 (a)).
    pub post_history_folded: bool,
    /// The card brought its own system prompt.
    pub own_system_prompt: bool,
    /// The card's notes for the person, shown and not kept.
    pub creator_notes: String,
}

/// A profile made from a card, and what the import did to get there.
#[derive(Debug, Clone)]
pub struct CardImport {
    pub profile: Profile,
    pub report: Report,
}

/// Maps `card` onto a new profile whose scaffold language is `lang`.
pub fn to_profile(card: &Card, lang: Lang) -> CardImport {
    let loc = locale(lang);
    let (system, report) = system_message(card, loc);
    let mut profile = Profile::new(card.name.trim(), system);
    profile.language = lang;
    profile.character_names = CharacterNames {
        assistant: card.char_name().to_string(),
        ..CharacterNames::default()
    };
    profile.greeting = Some(card.first_mes.trim().to_string()).filter(|g| !g.is_empty());
    profile.alternate_greetings = card
        .alternate_greetings
        .iter()
        .map(|g| g.trim().to_string())
        .collect();
    // The standard tool set, as the neutral import gives (spec §9.4); and every
    // companion feature off, as for any new profile — a card is a persona, and
    // the person decides what it may do (spec §10).
    profile.enabled_tools = default_tool_ids();
    profile.known_tools = default_tool_ids();
    profile.companion = CompanionFeatures::default();
    CardImport { profile, report }
}

/// What `mindfork import` shows before a card's profile is saved, in the
/// interface language `ui`: where the card came from, what the profile will be,
/// what had no exact home, the creator's notes, and the whole system message —
/// a card is untrusted text that becomes the persona, so the person reads it
/// before it is theirs (C6's risks).
pub fn preview(card: &Card, import: &CardImport, ui: &Locale) -> String {
    let p = &import.profile;
    let r = &import.report;
    let mut lines: Vec<String> = Vec::new();
    let source = match card.container {
        super::Container::Json => ui.t("cli.card.from_json").to_string(),
        super::Container::Png(chunk) => ui.tf("cli.card.from_png", &[("chunk", chunk)]),
        super::Container::Charx => ui.t("cli.card.from_charx").to_string(),
    };
    lines.push(ui.tf(
        "cli.card.head",
        &[
            ("name", &p.name),
            ("spec", card.spec.label()),
            ("source", &source),
        ],
    ));
    lines.push(ui.tf(
        "cli.card.profile",
        &[
            ("name", &p.name),
            ("char", card.char_name()),
            ("lang", p.language.label()),
        ],
    ));
    let greetings = usize::from(p.greeting.is_some()) + p.alternate_greetings.len();
    if greetings > 1 {
        lines.push(ui.tf("cli.card.greetings", &[("n", &greetings.to_string())]));
    } else if greetings == 0 {
        lines.push(ui.t("cli.card.no_greeting").to_string());
    }
    if r.lore_kept + r.lore_keyed > 0 {
        lines.push(ui.tf(
            "cli.card.lore",
            &[
                ("kept", &r.lore_kept.to_string()),
                ("keyed", &r.lore_keyed.to_string()),
            ],
        ));
    }
    if r.examples > 0 {
        lines.push(ui.tf("cli.card.examples", &[("n", &r.examples.to_string())]));
    }
    if r.own_system_prompt {
        lines.push(ui.t("cli.card.own_prompt").to_string());
    }
    if r.post_history_folded {
        lines.push(ui.t("cli.card.post_history").to_string());
    }
    let mentions_user = std::iter::once(&p.default_system_message)
        .chain(&p.greeting)
        .chain(&p.alternate_greetings)
        .any(|t| t.to_ascii_lowercase().contains("{{user}}"));
    if mentions_user {
        let user = locale(p.language).t("prompt.card.user_default");
        lines.push(ui.tf(
            "cli.card.user_name",
            &[("macro", "{{user}}"), ("user", user)],
        ));
    }
    if !r.creator_notes.is_empty() {
        lines.push(String::new());
        lines.push(ui.t("cli.card.creator_notes").to_string());
        lines.push(r.creator_notes.clone());
    }
    lines.push(String::new());
    lines.push(ui.t("cli.card.system_message").to_string());
    lines.push(RULE.to_string());
    lines.push(p.default_system_message.clone());
    lines.push(RULE.to_string());
    lines.join("\n")
}

/// The line the preview draws above and below the system message.
const RULE: &str = "────────────────────────────────────────";

/// The scaffold language a card reads as: Russian when its persona text has more
/// Cyrillic letters than Latin ones, else English.
pub fn detect_lang(card: &Card) -> Lang {
    let (mut cyr, mut lat) = (0usize, 0usize);
    for c in [
        &card.description,
        &card.personality,
        &card.scenario,
        &card.first_mes,
    ]
    .iter()
    .flat_map(|s| s.chars())
    {
        if ('\u{0400}'..='\u{04FF}').contains(&c) {
            cyr += 1;
        } else if c.is_ascii_alphabetic() {
            lat += 1;
        }
    }
    if cyr > lat { Lang::Ru } else { Lang::En }
}

/// The card's persona as one system message, and the report of how it was made.
/// Order: the main prompt (the card's own, `{{original}}` standing for ours, or
/// ours), the description, the personality, the scenario, the always-on lore,
/// the example block, and the post-history instructions last.
fn system_message(card: &Card, loc: &Locale) -> (String, Report) {
    let mut report = Report {
        own_system_prompt: !card.system_prompt.trim().is_empty(),
        creator_notes: card.creator_notes.trim().to_string(),
        ..Report::default()
    };
    let macros = [("char", "{{char}}"), ("user", "{{user}}")];
    let main = loc.tf("prompt.card.main", &macros);
    let mut parts: Vec<String> = Vec::new();
    parts.push(if report.own_system_prompt {
        replace_original(card.system_prompt.trim(), &main)
    } else {
        main
    });
    if !card.description.trim().is_empty() {
        parts.push(card.description.trim().to_string());
    }
    if !card.personality.trim().is_empty() {
        parts.push(loc.tf(
            "prompt.card.personality",
            &[("char", "{{char}}"), ("text", card.personality.trim())],
        ));
    }
    if !card.scenario.trim().is_empty() {
        parts.push(loc.tf("prompt.card.scenario", &[("text", card.scenario.trim())]));
    }
    let lore: Vec<&str> = card
        .book
        .iter()
        .filter(|e| e.always)
        .map(|e| e.content.as_str())
        .collect();
    report.lore_kept = lore.len();
    report.lore_keyed = card.book.len() - lore.len();
    if !lore.is_empty() {
        parts.push(format!(
            "{}\n\n{}",
            loc.t("prompt.card.lore"),
            lore.join("\n\n")
        ));
    }
    let examples = examples(&card.mes_example);
    report.examples = examples.len();
    if !examples.is_empty() {
        // The other side of an example is a nameless visitor, not the person:
        // with `{{user}}` there, what the examples say became the person's
        // past on some models — gpt-oss "remembered" the example's cat in 5
        // runs of 5, and in none once the line was the visitor's (§3.7).
        let visitor = loc.t("prompt.card.example_user");
        let blocks: Vec<String> = examples
            .iter()
            .map(|e| {
                let e = crate::features::persona_macros::replace_user(e, visitor);
                format!("<example>\n{e}\n</example>")
            })
            .collect();
        parts.push(format!(
            "{}\n\n{}",
            loc.tf("prompt.card.examples", &macros),
            blocks.join("\n")
        ));
    }
    if !card.post_history_instructions.trim().is_empty() {
        report.post_history_folded = true;
        parts.push(format!(
            "{}\n\n{}",
            loc.t("prompt.card.post_history"),
            replace_original(card.post_history_instructions.trim(), "")
        ));
    }
    (parts.join("\n\n"), report)
}

/// `{{original}}` (any case) replaced with `with`, the text the card's field
/// would have replaced.
fn replace_original(text: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let tag = "{{original}}";
        if tail
            .get(..tag.len())
            .is_some_and(|t| t.eq_ignore_ascii_case(tag))
        {
            out.push_str(with);
            rest = &tail[tag.len()..];
        } else {
            out.push_str("{{");
            rest = &tail[2..];
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// The card's example exchanges, one per `<START>`-separated block.
fn examples(mes_example: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in mes_example.lines() {
        if line.trim().eq_ignore_ascii_case("<start>") {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::character_card::{BookEntry, Container, Spec};

    fn card() -> Card {
        Card {
            spec: Spec::V2,
            container: Container::Json,
            name: "Mara".into(),
            nickname: String::new(),
            description: "{{char}} keeps the lighthouse on Grey Rock.".into(),
            personality: "dry, patient".into(),
            scenario: "{{user}} rows out in a storm.".into(),
            first_mes: "*{{char}} opens the door.* You're soaked.".into(),
            mes_example: "<START>\n{{user}}: Hi\n{{char}}: Hm.\n<START>\n{{user}}: Tea?\n{{char}}: Kettle's on.".into(),
            system_prompt: String::new(),
            post_history_instructions: String::new(),
            alternate_greetings: vec!["Back again?".into()],
            creator_notes: "Works best with a slow pace.".into(),
            book: vec![
                BookEntry {
                    content: "Grey Rock has no trees.".into(),
                    always: true,
                    order: 0,
                },
                BookEntry {
                    content: "The mainland town is Halden.".into(),
                    always: false,
                    order: 1,
                },
            ],
        }
    }

    #[test]
    fn assembles_the_system_message_in_order_with_the_macros_kept() {
        let out = to_profile(&card(), Lang::En);
        let s = &out.profile.default_system_message;
        let at = |needle: &str| s.find(needle).unwrap_or_else(|| panic!("{needle} in {s}"));
        assert!(at("You are {{char}}") < at("{{char}} keeps the lighthouse"));
        assert!(at("lighthouse") < at("dry, patient"));
        assert!(at("dry, patient") < at("rows out"));
        assert!(at("rows out") < at("no trees"));
        assert!(at("no trees") < at("<example>"));
        assert!(!s.contains("Halden"), "a keyed entry is left out");
        assert_eq!(s.matches("<example>").count(), 2);
        // The other side of an example is a visitor, never the person.
        assert!(s.contains("<example>\nVisitor: Tea?\n{{char}}: Kettle's on.\n</example>"));
        assert!(
            s.contains("{{user}} has not said"),
            "the header still names the person"
        );
        assert_eq!(
            out.report,
            Report {
                lore_kept: 1,
                lore_keyed: 1,
                examples: 2,
                post_history_folded: false,
                own_system_prompt: false,
                creator_notes: "Works best with a slow pace.".into(),
            }
        );
        assert!(
            !s.contains("slow pace"),
            "creator notes never reach the model"
        );
    }

    #[test]
    fn maps_names_greetings_and_defaults() {
        let mut c = card();
        c.nickname = "Mar".into();
        let p = to_profile(&c, Lang::En).profile;
        assert_eq!(p.name, "Mara");
        assert_eq!(p.character_names.assistant, "Mar");
        assert_eq!(p.character_names.user, "");
        assert_eq!(
            p.greeting.as_deref(),
            Some("*{{char}} opens the door.* You're soaked.")
        );
        assert_eq!(p.alternate_greetings, vec!["Back again?".to_string()]);
        assert_eq!(p.language, Lang::En);
        assert!(p.companion.is_default());
        assert_eq!(p.enabled_tools, default_tool_ids());
    }

    #[test]
    fn the_cards_own_prompt_replaces_ours_with_original_standing_for_it() {
        let mut c = card();
        c.system_prompt = "{{original}} Never break character.".into();
        c.post_history_instructions = "Keep replies short. {{original}}".into();
        let out = to_profile(&c, Lang::En);
        let s = &out.profile.default_system_message;
        assert!(s.starts_with("You are {{char}}"), "{s}");
        assert!(s.contains("Never break character."));
        assert!(s.trim_end().ends_with("Keep replies short."), "{s}");
        assert!(!s.to_ascii_lowercase().contains("{{original}}"));
        assert!(out.report.own_system_prompt && out.report.post_history_folded);

        c.system_prompt = "Only this.".into();
        let s = to_profile(&c, Lang::En).profile.default_system_message;
        assert!(s.starts_with("Only this."), "{s}");
        assert!(!s.contains("You are {{char}}"));
    }

    #[test]
    fn russian_labels_for_a_russian_card() {
        let mut c = card();
        c.description = "{{char}} держит маяк на Серой скале.".into();
        c.personality = "сухая, терпеливая".into();
        c.scenario = "{{user}} гребёт сквозь шторм.".into();
        c.first_mes = "Промок насквозь.".into();
        assert_eq!(detect_lang(&c), Lang::Ru);
        assert_eq!(detect_lang(&card()), Lang::En);
        let s = to_profile(&c, Lang::Ru).profile.default_system_message;
        assert!(!s.contains("You are"), "{s}");
    }

    #[test]
    fn examples_split_on_start_in_any_case_and_drop_empty_blocks() {
        assert_eq!(
            examples("<START>\n\na\n<start>\n<START>\nb\nc"),
            vec!["a".to_string(), "b\nc".to_string()]
        );
        assert!(examples("  ").is_empty());
        // Text before the first separator is an example of its own.
        assert_eq!(
            examples("x\n<START>\ny"),
            vec!["x".to_string(), "y".to_string()]
        );
    }
}
