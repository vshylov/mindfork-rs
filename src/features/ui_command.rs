//! The typed routes to actions that otherwise need a key chord — one registry
//! for all of them. Pure, testable parsing; the chat screen turns a parsed
//! command into the very intent its key produces (`screens::chat::commands`).
//!
//! **Why this exists.** A host that embeds the terminal claims chords before
//! crossterm ever sees them: VS Code's integrated terminal binds `Ctrl+P`,
//! `Ctrl+E`, `Ctrl+F`, `F1`, `F3`, `F5` and both quit keys by default, while a
//! browser tab (JupyterLab) reserves `Ctrl+N`/`Ctrl+T`/`Ctrl+W` — the last of
//! which closes the tab the session runs in. Typed text is the one input no
//! host can take away, which is the argument that already put `/image paste`
//! next to `Ctrl+V` (spec §9.10) and `/exit` next to `Ctrl+Q`/`F10`
//! (spec §11.7). See [docs/research/command-only-control.md](../../docs/research/command-only-control.md).
//!
//! **Why a registry and not nineteen parser modules.** Every command here is a
//! sibling of the last: an exact word, optionally one free-text argument. Copied
//! parsers would be the sliding self-duplication the duplication gate keeps
//! catching (docs/lessons.md §2 — "budget for the seam at design time"), and the
//! seam is this table. Commands with real syntax to explain — subcommands,
//! flags, `#N` targets — still earn modules of their own (`file_command`,
//! `image_command`, `rag_command`, `tts_command`); none of the commands here has
//! any.
//!
//! Command words are **not** localized (they are protocol, like CLI flags —
//! docs/roadmap.md); the error text is, in the interface language (axis B).

use crate::shared::i18n::Locale;

/// What a command does. The screen matches on this exhaustively, so a new row
/// in [`COMMANDS`] cannot ship without someone deciding what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiCommand {
    /// The settings screen (`Ctrl+P`).
    Settings,
    /// The self-model screen (`F3`); `clear` wipes the model (`Ctrl+K` twice
    /// inside that screen), behind the same confirmation.
    SelfModel,
    /// The chat list (`Esc`).
    Chats,
    /// The help/"About" dialog (`F1`/`?`).
    Help,
    /// A new chat; the argument picks the profile by name (`Ctrl+N`).
    NewChat,
    /// Rename the open chat; bare, it offers the current title for editing (`F2`).
    Rename,
    /// Clone the open chat (`Ctrl+D` in the chat list).
    Clone,
    /// Copy the whole conversation to the clipboard (`F5`).
    Copy,
    /// Regenerate the last reply (`Ctrl+R`).
    Regen,
    /// Delete the last exchange (`Ctrl+E`).
    Takeback,
    /// Write a message as the user; the argument seeds it (`Ctrl+U`).
    Impersonate,
    /// Cancel the running generation (`Esc` while generating).
    Stop,
    /// In-feed search; the argument is the query (`Ctrl+F`).
    Find,
    /// The message-level search screen across chats (`Ctrl+G` in the chat list).
    Search,
    /// The `chat://` reference picker (`Ctrl+L`).
    Links,
    /// Fold/unfold "thoughts" in the feed (`Ctrl+T`).
    Thoughts,
    /// Fold/unfold tool calls in the feed (`Ctrl+O`).
    ToolCalls,
    /// Toggle mouse capture (`Ctrl+W`).
    Mouse,
    /// The emoji picker (`Ctrl+B`).
    Emoji,
}

/// How many arguments a command takes. The whole remainder of the line is one
/// argument — a title, a query or a seed is free text with spaces in it, and
/// none of these commands has a subcommand to disambiguate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// Bare word. A trailing argument is reported rather than ignored.
    None,
    /// The argument may be omitted (bare has its own meaning).
    Optional,
    /// The argument is the point of the command; bare is reported.
    Required,
    /// The argument is optional and drawn from a **closed set** of words rather
    /// than being free text (`/self clear`). Anything else is reported here, in
    /// the parser, rather than by whoever runs the command — a command with two
    /// places to explain itself grows two wordings. A command whose subcommands
    /// take arguments of their own belongs in a module instead (`/profile`).
    Subcommand(&'static [&'static str]),
}

/// One row of the registry.
pub struct Spec {
    /// The accepted spellings, in the order the help label shows them.
    pub aliases: &'static [&'static str],
    pub command: UiCommand,
    pub arity: Arity,
    /// The help tab's label: a literal for a bare command, a `ui.help.k.*`
    /// bundle key when the label names an argument (`/find [text]` has to be
    /// translated). It doubles as the usage line in the "needs an argument"
    /// error, which is why the two can never disagree.
    pub label: &'static str,
    /// The help tab's description (a `ui.*` bundle key).
    pub description: &'static str,
}

/// A recognized command with its argument (empty when there is none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub command: UiCommand,
    pub argument: String,
    /// The spelling that matched, canonicalized to the registry's own casing.
    /// Notes quote **this** rather than the command's first alias: answering a
    /// typed `/retry` with `/regen` would read as a different command having
    /// been recognized (the `/exit` lesson, journal ui-input.md).
    pub alias: &'static str,
}

/// One row of [`COMMANDS`], as a function so the table reads as a table: five
/// columns per line rather than a five-line struct literal each. Ninety-five
/// lines of identically shaped rows is the sliding self-duplication the gate
/// measures as density (docs/lessons.md §2), and a table is easier to read
/// besides.
const fn row(
    aliases: &'static [&'static str],
    command: UiCommand,
    arity: Arity,
    label: &'static str,
    description: &'static str,
) -> Spec {
    Spec {
        aliases,
        command,
        arity,
        label,
        description,
    }
}

/// Every typed route, in the order the help tab lists them: the screens, the
/// conversation, finding things, the feed. The order is also the display order
/// of the "Commands" tab's last four groups (`COMMAND_GROUP_OPENERS` in
/// `screens::chat::popups` names the row that opens each).
///
/// The table is hand-aligned: `rustfmt` would explode each row into a five-line
/// call, and 19 identically shaped blocks is the duplication density the gate
/// measures (docs/lessons.md §2). A table reads as a table.
#[rustfmt::skip]
pub const COMMANDS: &[Spec] = &[
    // The screens. `/settings` and `/self` are unreachable in VS Code without
    // this route; `/chats` and `/help` have safe keys (`Esc`, `?`) and are here
    // for symmetry — and because `Esc` means three different things.
    row(&["/settings"], UiCommand::Settings, Arity::None, "/settings", "ui.help.cmd_settings"),
    row(&["/self"], UiCommand::SelfModel, Arity::Subcommand(&["clear"]), "ui.help.k.self", "ui.help.cmd_self"),
    row(&["/chats"], UiCommand::Chats, Arity::None, "/chats", "ui.help.cmd_chats"),
    row(&["/help"], UiCommand::Help, Arity::None, "/help", "ui.help.cmd_help"),
    // The conversation.
    row(&["/new"], UiCommand::NewChat, Arity::Optional, "ui.help.k.new", "ui.help.cmd_new"),
    row(&["/rename"], UiCommand::Rename, Arity::Optional, "ui.help.k.rename", "ui.help.cmd_rename"),
    row(&["/clone"], UiCommand::Clone, Arity::None, "/clone", "ui.help.cmd_clone"),
    row(&["/copy"], UiCommand::Copy, Arity::None, "/copy", "ui.help.cmd_copy"),
    // Two spellings, on the `/exit`·`/quit` rule: both words are pre-trained
    // elsewhere, and someone reaching for one does not want to learn the other.
    row(&["/regen", "/retry"], UiCommand::Regen, Arity::None, "/regen · /retry", "ui.help.cmd_regen"),
    row(&["/takeback"], UiCommand::Takeback, Arity::None, "/takeback", "ui.help.cmd_takeback"),
    row(&["/impersonate"], UiCommand::Impersonate, Arity::Optional, "ui.help.k.impersonate", "ui.help.cmd_impersonate"),
    row(&["/stop"], UiCommand::Stop, Arity::None, "/stop", "ui.help.cmd_stop"),
    // Finding things.
    row(&["/find"], UiCommand::Find, Arity::Optional, "ui.help.k.find", "ui.help.cmd_find"),
    row(&["/search"], UiCommand::Search, Arity::Required, "ui.help.k.search", "ui.help.cmd_search"),
    row(&["/links"], UiCommand::Links, Arity::None, "/links", "ui.help.cmd_links"),
    // The feed.
    row(&["/thoughts"], UiCommand::Thoughts, Arity::None, "/thoughts", "ui.help.cmd_thoughts"),
    row(&["/toolcalls"], UiCommand::ToolCalls, Arity::None, "/toolcalls", "ui.help.cmd_toolcalls"),
    row(&["/mouse"], UiCommand::Mouse, Arity::None, "/mouse", "ui.help.cmd_mouse"),
    row(&["/emoji"], UiCommand::Emoji, Arity::None, "/emoji", "ui.help.cmd_emoji"),
];

/// Tries to parse an input string as one of [`COMMANDS`].
///
/// - `None` — not one of them: the text goes out as an ordinary message.
/// - `Some(Ok(parsed))` — a recognized command (`argument` is empty when the
///   command takes none, or when an optional one was omitted).
/// - `Some(Err(msg))` — the right word with the wrong arguments, and `msg` is a
///   localized note. Reported rather than sent, exactly as `/compact` and
///   `/exit` do: silence on a mistyped command reads as the app refusing to act
///   (docs/lessons.md §4).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<Parsed, String>> {
    let trimmed = input.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next()?;
    let (spec, alias) = COMMANDS.iter().find_map(|s| {
        s.aliases
            .iter()
            .find(|a| head.eq_ignore_ascii_case(a))
            .map(|a| (s, *a))
    })?;
    let argument = parts.next().unwrap_or_default().trim();
    Some(match spec.arity {
        Arity::None if !argument.is_empty() => {
            Err(loc.tf("ui.cmd.bad_arg", &[("cmd", alias), ("arg", argument)]))
        }
        Arity::Required if argument.is_empty() => {
            Err(loc.tf("ui.cmd.needs_arg", &[("usage", loc.t(spec.label))]))
        }
        // A closed set: the word is normalized to the registry's own spelling,
        // so the runner matches on a known string rather than on user casing.
        Arity::Subcommand(words) if !argument.is_empty() => {
            match words.iter().find(|w| argument.eq_ignore_ascii_case(w)) {
                Some(word) => Ok(Parsed {
                    command: spec.command,
                    argument: (*word).to_string(),
                    alias,
                }),
                None => Err(loc.tf(
                    "ui.cmd.bad_subcommand",
                    &[
                        ("cmd", alias),
                        ("arg", argument),
                        ("usage", loc.t(spec.label)),
                    ],
                )),
            }
        }
        _ => Ok(Parsed {
            command: spec.command,
            argument: argument.to_string(),
            alias,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    /// Every spelling of every command, with the padding and letter case a real
    /// input box produces. Driven off [`COMMANDS`] so a new row cannot be added
    /// without being covered here.
    #[test]
    fn every_alias_parses_in_any_case_and_padding() {
        for spec in COMMANDS {
            // A required argument is supplied so this test is about the word,
            // not about the arity (which has its own tests below).
            let arg = if spec.arity == Arity::Required {
                " something"
            } else {
                ""
            };
            for alias in spec.aliases {
                for text in [
                    format!("{alias}{arg}"),
                    format!("  {alias}{arg}  "),
                    format!("\t{alias}{arg}\n"),
                    format!("{}{arg}", alias.to_uppercase()),
                ] {
                    let parsed = parse(&text, ru())
                        .unwrap_or_else(|| panic!("{text:?} was not recognized"))
                        .unwrap_or_else(|e| panic!("{text:?} did not parse: {e}"));
                    assert_eq!(parsed.command, spec.command, "input {text:?}");
                    // The spelling travels with the command: a note about
                    // `/retry` must not answer with `/regen`.
                    assert_eq!(parsed.alias, *alias, "input {text:?}");
                }
            }
        }
    }

    /// The registry's own hygiene: no word is claimed twice (the first match
    /// would silently win), every alias is a `/word`, and every label shows
    /// every spelling the parser accepts — the label is the only place a user
    /// learns that `/retry` exists.
    #[test]
    fn the_registry_is_unambiguous_and_self_describing() {
        let mut seen = Vec::new();
        for spec in COMMANDS {
            for alias in spec.aliases {
                assert!(
                    alias.starts_with('/') && !alias[1..].contains(char::is_whitespace),
                    "{alias:?} is not a bare /word"
                );
                assert!(!seen.contains(alias), "{alias:?} is claimed twice");
                seen.push(alias);
            }
            // A label that carries an argument is a bundle key (it needs
            // translating); a bare one is the literal shown as is.
            let label = ru().t(spec.label);
            for alias in spec.aliases {
                assert!(
                    label.contains(alias),
                    "the label {label:?} does not show {alias:?}"
                );
            }
            assert_eq!(
                spec.arity == Arity::None,
                !label.contains('<') && !label.contains('['),
                "the label {label:?} disagrees with the arity of {:?}",
                spec.command
            );
        }
    }

    #[test]
    fn a_bare_word_takes_no_arguments() {
        for spec in COMMANDS.iter().filter(|s| s.arity == Arity::None) {
            let text = format!("{} nonsense", spec.aliases[0]);
            let Some(Err(msg)) = parse(&text, ru()) else {
                panic!("{text:?} should have been reported");
            };
            assert!(msg.contains("nonsense"), "the argument is named: {msg}");
            assert!(
                msg.contains(spec.aliases[0]),
                "the typed spelling is named: {msg}"
            );
        }
    }

    /// The other spelling must not leak into the error — it would read as a
    /// different command having been recognized (`/exit`'s lesson, pinned here
    /// for the one command that has two spellings).
    #[test]
    fn the_error_quotes_the_typed_spelling_only() {
        let Some(Err(msg)) = parse("/retry now", ru()) else {
            panic!("expected a report");
        };
        assert!(msg.contains("/retry"), "{msg}");
        assert!(
            !msg.contains("/regen"),
            "the untyped spelling leaked: {msg}"
        );
    }

    #[test]
    fn a_required_argument_is_asked_for_and_an_optional_one_is_not() {
        for spec in COMMANDS {
            let parsed = parse(spec.aliases[0], ru()).expect("recognized");
            match spec.arity {
                Arity::Required => {
                    let Err(msg) = parsed else {
                        panic!("{:?} accepted a missing argument", spec.command);
                    };
                    // The usage line is the help label, so the note teaches the
                    // exact syntax the help tab shows (fork F4).
                    assert!(
                        msg.contains(ru().t(spec.label)),
                        "the usage line is missing: {msg}"
                    );
                }
                _ => {
                    let Ok(p) = parsed else {
                        panic!("{:?} rejected a bare word", spec.command);
                    };
                    assert!(p.argument.is_empty());
                }
            }
        }
    }

    /// A closed-set argument: the word is accepted in any case and handed back
    /// in the registry's spelling, anything else is reported by the parser (with
    /// the usage line), and omitting it keeps the bare meaning.
    #[test]
    fn a_subcommand_word_is_normalized_and_anything_else_reported() {
        assert_eq!(
            parse("/self CLEAR", ru())
                .expect("recognized")
                .expect("parsed"),
            Parsed {
                command: UiCommand::SelfModel,
                argument: "clear".into(),
                alias: "/self",
            }
        );
        assert!(
            parse("/self", ru())
                .expect("recognized")
                .is_ok_and(|p| p.argument.is_empty()),
            "bare /self keeps its own meaning"
        );
        let Some(Err(msg)) = parse("/self wipe", ru()) else {
            panic!("expected a report");
        };
        assert!(msg.contains("wipe"), "the word is quoted back: {msg}");
        assert!(
            msg.contains(ru().t("ui.help.k.self")),
            "the usage line is missing: {msg}"
        );
    }

    /// The argument is the rest of the line, spaces and all — a chat title and a
    /// search query are ordinary prose, and splitting them into tokens would
    /// quietly rename a chat to its first word.
    #[test]
    fn the_argument_is_the_whole_remainder() {
        for text in [
            "/rename  a long title, with punctuation  ",
            "/search  a long title, with punctuation",
            "/impersonate  a long title, with punctuation",
        ] {
            let parsed = parse(text, ru()).expect("recognized").expect("parsed");
            assert_eq!(parsed.argument, "a long title, with punctuation");
        }
    }

    #[test]
    fn other_input_is_none() {
        for text in [
            // The commands with parsers of their own keep their meaning.
            "/rag list",
            "/file list",
            "/image list",
            "/tts stop",
            "/compact",
            "/reindex",
            "/exit",
            // A longer word that merely starts with a command name is not it.
            "/settings2",
            "/self-model",
            "/newest",
            "/findings",
            // Prose, including the words themselves — ordinary things to say to
            // a model.
            "help",
            "how do I stop a runaway loop?",
            "",
            "   ",
        ] {
            assert_eq!(parse(text, ru()), None, "input {text:?}");
        }
    }

    /// Per-locale gate (docs/history/i18n-ui.md §3.5): both error paths render
    /// under every bundled language with no unsubstituted placeholder, and the
    /// `en` text is free of Cyrillic leaking from the `ru` default. Each message
    /// must also name a route that works (docs/lessons.md §4).
    #[test]
    fn errors_are_localized_for_all_langs() {
        for &lang in Lang::ALL {
            let loc = locale(lang);
            for (input, must_name) in [("/stop now", "/stop"), ("/search", "/search")] {
                let Some(Err(msg)) = parse(input, loc) else {
                    panic!("{input:?} should have been reported in {lang:?}");
                };
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                assert!(
                    msg.contains(must_name),
                    "the message must name {must_name} in {lang:?}: {msg}"
                );
                if lang == Lang::En {
                    assert!(
                        !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en message: {msg}"
                    );
                }
            }
        }
    }
}
