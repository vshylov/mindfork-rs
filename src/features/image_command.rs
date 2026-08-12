//! Parses the image slash-commands in the input box (`/image attach <path>`,
//! `/image remove <name|#N>`, `/image list`). Pure, testable logic, mirroring
//! [`super::file_command`]: the chat screen calls it on send, a recognized command turns
//! into an intent, and an unrecognized string goes out as an ordinary message. Error
//! messages are in the interface language (axis B) — the caller passes its `Locale`.
//!
//! The surface deliberately matches `/file` — same verbs, same `#N` addressing, same
//! tri-state parse contract — but the **semantics differ**, and the messages say so: an
//! image is staged for the *next message* rather than pinned to the chat, so `remove`
//! only reaches something not yet sent (fork F1,
//! docs/research/multimodal-images.md).
//!
//! [`ImageProgress`] also lives here, in the `features` layer, so both `app` (emits
//! events) and `screens` (renders notes) can use it without breaking FSD's dependency
//! direction — the arrangement [`FileProgress`](super::file_command::FileProgress) uses.
//!
//! See spec §9.10.

use crate::entities::message_image::ImageInfo;
use crate::shared::i18n::Locale;

/// A recognized `/image` command.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageCommand {
    /// Stage an image for the next message.
    Attach { path: String },
    /// Unstage one by display name, path, or `#N` position.
    Remove { target: String },
    /// Show what is staged for the next message.
    List,
}

/// Outcome of an `/image` command, for the feed note / status chip.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageProgress {
    /// An image was staged: its card + how many are now staged.
    Attached { info: ImageInfo, staged: usize },
    /// An image was unstaged.
    Removed { name: String },
    /// The staged list (`/image list`); empty — nothing staged.
    Listed { items: Vec<ImageInfo> },
    /// The image was staged, but the engine could not say whether it takes images.
    /// Not an error — attaching optimistically is the deliberate choice for an engine
    /// that has no `/props` (fork F4); the note tells the user where they will find out.
    VisionUnknown,
    /// The command failed (no such file, undecodable content, cap reached, an engine
    /// that reports no vision support…).
    Failed(String),
}

/// Tries to parse an input string as an `/image` command.
///
/// - `None` — not an `/image` command: send it as a regular message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — an `/image` command with a syntax error (localized).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<ImageCommand, String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/image") {
        return None;
    }
    let Some(sub) = tokens.next() else {
        return Some(Err(loc.tf(
            "ui.image.err.missing_subcommand",
            &[("usage", loc.t("ui.image.usage"))],
        )));
    };
    let rest: Vec<&str> = tokens.collect();

    if sub.eq_ignore_ascii_case("attach") {
        match argument(&rest) {
            Some(path) => Some(Ok(ImageCommand::Attach { path })),
            None => Some(Err(loc.tf(
                "ui.image.err.missing_path",
                &[("usage", loc.t("ui.image.usage"))],
            ))),
        }
    } else if sub.eq_ignore_ascii_case("remove") {
        // `remove`, never `delete` — same wording rule as `/file` and `/rag`: nothing is
        // taken off disk, and a user who reads it otherwise loses a file.
        match argument(&rest) {
            Some(target) => Some(Ok(ImageCommand::Remove { target })),
            None => Some(Err(loc.tf(
                "ui.image.err.missing_target",
                &[("usage", loc.t("ui.image.usage"))],
            ))),
        }
    } else if sub.eq_ignore_ascii_case("list") {
        Some(Ok(ImageCommand::List))
    } else {
        Some(Err(loc.tf(
            "ui.image.err.unknown_subcommand",
            &[("sub", sub), ("usage", loc.t("ui.image.usage"))],
        )))
    }
}

/// Joins the tokens after the subcommand into one argument: a path or name may contain
/// spaces, and surrounding quotes are stripped. `None` — nothing left.
fn argument(tokens: &[&str]) -> Option<String> {
    let joined = tokens.join(" ");
    let arg = joined.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!arg.is_empty()).then(|| arg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::En)
    }

    #[test]
    fn non_image_input_is_none() {
        assert!(parse("hello", en()).is_none());
        assert!(parse("", en()).is_none());
        // A neighbouring command must not be swallowed.
        assert!(parse("/file attach a.txt", en()).is_none());
        // Nor a longer word starting with the same prefix.
        assert!(parse("/images list", en()).is_none());
    }

    #[test]
    fn parses_attach_with_path() {
        assert_eq!(
            parse("/image attach D:\\pics\\a.png", en()),
            Some(Ok(ImageCommand::Attach {
                path: "D:\\pics\\a.png".into()
            }))
        );
    }

    #[test]
    fn path_with_spaces_and_quotes() {
        assert_eq!(
            parse("/image attach \"C:\\my pics\\a b.png\"", en()),
            Some(Ok(ImageCommand::Attach {
                path: "C:\\my pics\\a b.png".into()
            }))
        );
        // Unquoted, spaces and all — the tokens are rejoined.
        assert_eq!(
            parse("/image attach C:\\my pics\\a b.png", en()),
            Some(Ok(ImageCommand::Attach {
                path: "C:\\my pics\\a b.png".into()
            }))
        );
    }

    #[test]
    fn command_is_case_insensitive() {
        assert_eq!(parse("/IMAGE LIST", en()), Some(Ok(ImageCommand::List)));
        assert_eq!(
            parse("/Image Remove #1", en()),
            Some(Ok(ImageCommand::Remove {
                target: "#1".into()
            }))
        );
    }

    #[test]
    fn delete_is_not_a_command() {
        // Only `remove`: `/image delete` must not silently do something.
        let Some(Err(msg)) = parse("/image delete a.png", en()) else {
            panic!("expected a syntax error");
        };
        assert!(
            msg.contains("delete"),
            "the message should name what was typed: {msg}"
        );
    }

    #[test]
    fn errors_on_missing_parts() {
        assert!(matches!(parse("/image", en()), Some(Err(_))));
        assert!(matches!(parse("/image attach", en()), Some(Err(_))));
        assert!(matches!(parse("/image remove", en()), Some(Err(_))));
        assert!(matches!(parse("/image attach \"\"", en()), Some(Err(_))));
    }

    /// Per-locale coverage (i18n gate discipline, docs/history/i18n-ui.md §3.5): every
    /// error path renders under EVERY built-in language with no unsubstituted `{…}` and,
    /// for `en`, with no Cyrillic leaking through.
    #[test]
    fn errors_are_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            for input in ["/image", "/image attach", "/image remove", "/image purge x"] {
                let Some(Err(msg)) = parse(input, loc) else {
                    panic!("expected a syntax error for {input:?} in {lang:?}");
                };
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                if lang == crate::shared::i18n::Lang::En {
                    assert!(
                        !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en message: {msg}"
                    );
                }
            }
        }
    }
}
