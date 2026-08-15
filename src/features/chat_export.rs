//! Exports a chat conversation: as plain text for the clipboard (`F5`/`/copy`)
//! and for a file (`/export`), and as a `mindfork-import` v1 document
//! (`/export json`). Pure logic, testable without the UI — the orchestrator
//! calls it and owns the disk I/O. See spec §11.2,
//! [docs/history/chat-export-file.md](../../docs/history/chat-export-file.md).

use chrono::{DateTime, Utc};

use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::{CharacterNames, Profile};
use crate::shared::config::CopySettings;
use crate::shared::i18n::Locale;

/// Longest slug taken from a chat title for a generated filename. Well under
/// every filesystem's limit once the date prefix and extension are added, and
/// long enough to tell two conversations apart.
const SLUG_MAX: usize = 60;

/// Formats a whole chat conversation into readable text for the clipboard: labels
/// roles (`User`/`Assistant`), preserves multiline text, skips
/// system/tool messages. A non-empty `title` becomes the header.
///
/// `names` — the profile's custom role names (spec §5.1): a set name replaces the
/// localized label (`Gaia:` instead of `User:`); empty fields keep the default.
///
/// By default (`CopySettings` with all flags `false`) only the message text is copied.
/// Optionally (`opts`) the assistant block also gets: "thoughts" (CoT, before the text),
/// tool-call parameters (name + arguments), and their results — taken from
/// `Message.tool_calls` (separate tool messages are still skipped, to avoid
/// duplication). Returns `None` if there are no substantive blocks (nothing to copy).
pub fn format_conversation(
    title: &str,
    messages: &[Message],
    opts: &CopySettings,
    names: &CharacterNames,
    loc: &'static Locale,
) -> Option<String> {
    let mut blocks: Vec<String> = Vec::new();

    let title = title.trim();
    if !title.is_empty() {
        blocks.push(title.to_string());
    }

    for m in messages {
        match m.role {
            MessageRole::User => {
                let text = m.text.trim();
                if !text.is_empty() {
                    blocks.push(format!(
                        "{}\n{text}",
                        role_label(names.user_name(), "ui.export.user", loc)
                    ));
                }
            }
            MessageRole::Assistant => {
                if let Some(block) = format_assistant(m, opts, names, loc) {
                    blocks.push(block);
                }
            }
            // System and tool messages don't go into the conversation
            // (tool results are taken from the assistant's `tool_calls`).
            MessageRole::System | MessageRole::Tool => continue,
        }
    }

    // Only the title (or nothing at all) — there's no substantive conversation.
    let has_messages = blocks.len() > usize::from(!title.is_empty());
    if !has_messages {
        return None;
    }
    Some(blocks.join("\n\n"))
}

/// A role's label for the export: the custom name (with the same trailing colon the
/// localized label carries) or the localized default under `key`.
fn role_label(custom: Option<&str>, key: &str, loc: &'static Locale) -> String {
    match custom {
        Some(name) => format!("{name}:"),
        None => loc.t(key).to_string(),
    }
}

/// Builds the block for a single assistant message: optional "thoughts", text, and
/// optional tool blocks. `None` if the block is empty after filtering.
fn format_assistant(
    m: &Message,
    opts: &CopySettings,
    names: &CharacterNames,
    loc: &'static Locale,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if opts.copy_thoughts
        && let Some(thoughts) = &m.thoughts
    {
        let thoughts = thoughts.trim();
        if !thoughts.is_empty() {
            parts.push(format!("{}\n{thoughts}", loc.t("ui.export.thoughts")));
        }
    }

    let text = m.text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }

    if opts.copy_tool_calls || opts.copy_tool_results {
        for tc in &m.tool_calls {
            let mut lines = vec![loc.tf("ui.export.tool", &[("name", &tc.name)])];
            if opts.copy_tool_calls {
                let args = serde_json::to_string(&tc.arguments).unwrap_or_default();
                lines.push(loc.tf("ui.export.args", &[("args", &args)]));
            }
            if opts.copy_tool_results {
                let result = tc.result.as_deref().unwrap_or("").trim();
                lines.push(loc.tf("ui.export.result", &[("result", result)]));
            }
            parts.push(lines.join("\n"));
        }
    }

    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "{}\n{}",
        role_label(names.assistant_name(), "ui.export.assistant", loc),
        parts.join("\n\n")
    ))
}

/// A filename for an export with no path given: `<date>-<slug>.<ext>`, e.g.
/// `2026-08-15-about-space.md`.
///
/// The date comes first so a directory of exports sorts chronologically, and
/// the slug is a *hint*, not the identity — a title of punctuation or of a
/// script with no ASCII form yields `chat`, which is still a usable name.
pub fn export_filename(title: &str, ext: &str, now: DateTime<Utc>) -> String {
    let slug = slugify(title);
    format!("{}-{slug}.{ext}", now.format("%Y-%m-%d"))
}

/// Turns a title into a filename-safe slug: lowercase, non-alphanumerics
/// collapsed to single dashes, trimmed to [`SLUG_MAX`] **characters** (not
/// bytes — a Cyrillic title must not be cut mid-character).
///
/// Non-ASCII letters are **kept**: every filesystem the app supports stores
/// UTF-8 names, and transliterating a Russian title into Latin would produce
/// something its owner cannot search for.
fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut len = 0usize; // in characters, tracked rather than recounted
    let mut pending_dash = false;
    for ch in title.trim().chars() {
        // Anything that is not a letter or a digit becomes a word break — which
        // is also how path separators and the characters Windows forbids are
        // kept out, without a list of them to maintain.
        if !ch.is_alphanumeric() {
            pending_dash = true;
            continue;
        }
        // Lowercasing can yield more than one character, so the budget is
        // checked **before** writing anything: a cut that overshoots by a
        // character is how the first version of this ran past the limit.
        let lower: String = ch.to_lowercase().collect();
        let dash = usize::from(pending_dash && !out.is_empty());
        let cost = dash + lower.chars().count();
        if len + cost > SLUG_MAX {
            break;
        }
        if dash == 1 {
            out.push('-');
        }
        pending_dash = false;
        out.push_str(&lower);
        len += cost;
    }
    if out.is_empty() {
        "chat".to_string()
    } else {
        out
    }
}

/// Builds the `mindfork-import` v1 document for one chat (docs/import-format.md):
/// the chat plus the profile it belongs to, since the format requires a chat's
/// `profile_key` to name a profile in the same file.
///
/// Both entities carry an **explicit `id`**, so importing the file back lands on
/// the same chat and profile rather than creating copies — export and import are
/// a round trip, which is the whole reason for choosing this format over a
/// private one (fork F2).
///
/// **Tool calls are not carried**: the format's messages are
/// `{role, text, thoughts?, timestamp?}` and there is nowhere to put them. The
/// caller says so rather than letting the omission be discovered later.
pub fn to_import_json(chat: &Chat, profile: &Profile) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = chat
        .messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .map(|m| {
            let mut o = serde_json::json!({
                "role": match m.role {
                    MessageRole::User => "user",
                    _ => "assistant",
                },
                "text": m.text,
                "timestamp": m.timestamp.to_rfc3339(),
            });
            if let Some(thoughts) = m.thoughts.as_ref().filter(|t| !t.trim().is_empty()) {
                o["thoughts"] = serde_json::Value::String(thoughts.clone());
            }
            o
        })
        .collect();
    serde_json::json!({
        "format": "mindfork-import",
        "version": 1,
        "profiles": [{
            "key": profile.id.to_string(),
            "id": profile.id.to_string(),
            "name": profile.name,
            "language": profile.language.code(),
            "system_message": profile.default_system_message,
        }],
        "chats": [{
            "key": chat.id.to_string(),
            "id": chat.id.to_string(),
            "profile_key": profile.id.to_string(),
            "title": chat.title,
            "created_at": chat.created_at.to_rfc3339(),
            "modified_at": chat.modified_at.to_rfc3339(),
            "system_message": chat.system_message,
            "character_names": {
                "user": chat.character_names.user,
                "assistant": chat.character_names.assistant,
                "system": chat.character_names.system,
            },
            "messages": messages,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::ToolCallRecord;

    // ---------- export to a file (docs/history/chat-export-file.md) ----------

    /// A generated name: the date first (so a directory of exports sorts
    /// chronologically), then a slug of the title.
    #[test]
    fn a_generated_filename_leads_with_the_date() {
        let when = chrono::DateTime::parse_from_rfc3339("2026-08-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            export_filename("About space", "md", when),
            "2026-08-15-about-space.md"
        );
        assert_eq!(
            export_filename("About space", "json", when),
            "2026-08-15-about-space.json"
        );
    }

    /// The slug is filename-safe and keeps non-Latin letters: every filesystem
    /// here stores UTF-8, and transliterating a title would produce something
    /// its owner cannot search for. A title with nothing usable still yields a
    /// name.
    #[test]
    fn the_slug_is_safe_and_keeps_its_letters() {
        for (title, expected) in [
            ("About space", "about-space"),
            ("  Trimmed  ", "trimmed"),
            ("Punctuation! and, symbols?", "punctuation-and-symbols"),
            ("slashes/and\\colons:", "slashes-and-colons"),
            ("многоточие… и буквы", "многоточие-и-буквы"),
            ("CamelCase", "camelcase"),
            ("", "chat"),
            ("!!!", "chat"),
            ("...", "chat"),
        ] {
            assert_eq!(slugify(title), expected, "title {title:?}");
        }
        // No path separator can survive, whatever the title held — a slug is a
        // file name, not a path.
        for title in ["a/b", "a\\b", "../escape", "C:\\Windows"] {
            let slug = slugify(title);
            assert!(
                !slug.contains('/') && !slug.contains('\\') && !slug.contains(':'),
                "{title:?} produced {slug:?}"
            );
        }
    }

    /// A very long title is cut to a sane length, and cut on a **character**
    /// boundary — a Cyrillic title cut mid-character would not be a valid name.
    #[test]
    fn a_long_title_is_trimmed_by_characters() {
        let slug = slugify(&"я".repeat(500));
        assert!(
            slug.chars().count() <= SLUG_MAX,
            "{} chars",
            slug.chars().count()
        );
        assert!(!slug.is_empty());
        let latin = slugify(&"word ".repeat(200));
        assert!(latin.chars().count() <= SLUG_MAX);
    }

    /// The claim fork F2 rests on: a JSON export is a `mindfork-import` v1
    /// document that the app's **own importer** accepts, and it comes back as
    /// the same chat (the explicit `id`), with the same title and the same
    /// user/assistant text.
    #[test]
    fn a_json_export_imports_back_as_the_same_chat() {
        let profile = Profile::new("Гайя", "system");
        let mut chat = Chat::from_profile(&profile, "Про космос");
        chat.system_message = "будь краток".into();
        chat.messages = vec![
            Message::new(MessageRole::System, "не сюда"),
            Message::user("привет"),
            {
                let mut m = Message::assistant("здравствуйте");
                m.thoughts = Some("подумал".into());
                m
            },
        ];
        let doc = to_import_json(&chat, &profile);
        let json = serde_json::to_string_pretty(&doc).unwrap();

        let result = crate::features::import::parse_import(&json, ru())
            .expect("our own importer accepts what we emit");
        assert_eq!(result.profiles.len(), 1);
        assert_eq!(result.chats.len(), 1);
        let back = &result.chats[0];
        assert_eq!(back.id, chat.id, "a re-import lands on the same chat");
        assert_eq!(result.profiles[0].id, profile.id, "…and the same profile");
        assert_eq!(back.title, "Про космос");
        assert_eq!(back.system_message, "будь краток");
        // The system message does not become a history entry (the format says
        // so), so two messages come back, in order, with the CoT preserved.
        let texts: Vec<&str> = back.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, vec!["привет", "здравствуйте"]);
        assert_eq!(back.messages[1].thoughts.as_deref(), Some("подумал"));
    }

    /// The documented loss, pinned so nobody "fixes" the format silently: the
    /// import document has nowhere to put tool calls, which is why every JSON
    /// export says so in its note.
    #[test]
    fn a_json_export_drops_tool_calls() {
        let profile = Profile::new("P", "sys");
        let mut chat = Chat::from_profile(&profile, "T");
        chat.messages = vec![assistant_with_tool()];
        let json = serde_json::to_string(&to_import_json(&chat, &profile)).unwrap();
        assert!(json.contains("ответ"), "the text survives: {json}");
        assert!(
            !json.contains("note_save"),
            "the format carries no tool calls: {json}"
        );
    }

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Text-only copy (the default behavior).
    fn plain() -> CopySettings {
        CopySettings::default()
    }

    /// No custom role names — the localized labels are used.
    fn no_names() -> CharacterNames {
        CharacterNames::default()
    }

    /// An assistant message with "thoughts" and one tool call (name/arguments/
    /// result) — for checking the optional blocks.
    fn assistant_with_tool() -> Message {
        let mut m = Message::assistant("ответ");
        m.thoughts = Some("я думаю".into());
        m.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            images: 0,
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({"text": "заметка"}),
            result: Some("сохранено".into()),
        }];
        m
    }

    #[test]
    fn formats_roles_with_title_and_skips_system_and_tools() {
        let msgs = vec![
            Message::new(MessageRole::System, "ты ассистент"),
            Message::user("как дела?"),
            Message::assistant("хорошо"),
            Message::new(MessageRole::Tool, "tool output"),
        ];
        let out = format_conversation("Мой чат", &msgs, &plain(), &no_names(), ru()).unwrap();
        assert_eq!(
            out,
            "Мой чат\n\nПользователь:\nкак дела?\n\nАссистент:\nхорошо"
        );
    }

    #[test]
    fn empty_title_is_omitted() {
        let msgs = vec![Message::user("привет")];
        let out = format_conversation("   ", &msgs, &plain(), &no_names(), ru()).unwrap();
        assert_eq!(out, "Пользователь:\nпривет");
    }

    #[test]
    fn multiline_text_is_preserved() {
        let msgs = vec![Message::assistant("строка 1\nстрока 2")];
        let out = format_conversation("", &msgs, &plain(), &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\nстрока 1\nстрока 2");
    }

    #[test]
    fn none_when_no_meaningful_messages() {
        // Only system/tool/empty ones — nothing to copy, even with a title.
        let msgs = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("   "),
            Message::new(MessageRole::Tool, "out"),
        ];
        assert!(format_conversation("Заголовок", &msgs, &plain(), &no_names(), ru()).is_none());
        assert!(format_conversation("", &[], &plain(), &no_names(), ru()).is_none());
    }

    #[test]
    fn plain_omits_thoughts_and_tools() {
        // By default "thoughts" and tool blocks aren't copied — only the text.
        let out =
            format_conversation("", &[assistant_with_tool()], &plain(), &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\nответ");
    }

    #[test]
    fn thoughts_included_before_text_when_enabled() {
        let opts = CopySettings {
            copy_thoughts: true,
            ..Default::default()
        };
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\n[Мысли]\nя думаю\n\nответ");
    }

    #[test]
    fn tool_calls_include_name_and_arguments() {
        let opts = CopySettings {
            copy_tool_calls: true,
            ..Default::default()
        };
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
        assert_eq!(
            out,
            "Ассистент:\nответ\n\n[Инструмент: note_save]\nАргументы: {\"text\":\"заметка\"}"
        );
    }

    #[test]
    fn tool_results_include_name_and_result() {
        let opts = CopySettings {
            copy_tool_results: true,
            ..Default::default()
        };
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
        assert_eq!(
            out,
            "Ассистент:\nответ\n\n[Инструмент: note_save]\nРезультат: сохранено"
        );
    }

    #[test]
    fn calls_and_results_combine_under_one_header() {
        let opts = CopySettings {
            copy_tool_calls: true,
            copy_tool_results: true,
            ..Default::default()
        };
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
        assert_eq!(
            out,
            "Ассистент:\nответ\n\n[Инструмент: note_save]\n\
             Аргументы: {\"text\":\"заметка\"}\nРезультат: сохранено"
        );
    }

    #[test]
    fn assistant_with_only_tool_calls_is_included_when_enabled() {
        // Empty text, but there's a tool call and the option is enabled → the block isn't dropped.
        let mut m = Message::assistant("");
        m.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            images: 0,
            id: "c1".into(),
            name: "calculate".into(),
            arguments: serde_json::json!({"expr": "2+2"}),
            result: Some("4".into()),
        }];
        let opts = CopySettings {
            copy_tool_results: true,
            ..Default::default()
        };
        let out = format_conversation("", &[m], &opts, &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\n[Инструмент: calculate]\nРезультат: 4");
        // But with the options off, such a message is skipped entirely.
        let mut m2 = Message::assistant("");
        m2.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            images: 0,
            id: "c1".into(),
            name: "calculate".into(),
            arguments: serde_json::json!({}),
            result: Some("4".into()),
        }];
        assert!(format_conversation("", &[m2], &plain(), &no_names(), ru()).is_none());
    }

    /// Custom role names from the profile replace the localized labels; an empty
    /// field keeps its default (a name may be set for one side only).
    #[test]
    fn custom_names_replace_role_labels() {
        let msgs = vec![Message::user("привет"), Message::assistant("здравствуй")];
        let names = CharacterNames {
            user: "Гайя".into(),
            assistant: "Анна".into(),
            system: String::new(),
        };
        let out = format_conversation("", &msgs, &plain(), &names, ru()).unwrap();
        assert_eq!(out, "Гайя:\nпривет\n\nАнна:\nздравствуй");

        let only_assistant = CharacterNames {
            assistant: "Анна".into(),
            ..Default::default()
        };
        let out = format_conversation("", &msgs, &plain(), &only_assistant, ru()).unwrap();
        assert_eq!(out, "Пользователь:\nпривет\n\nАнна:\nздравствуй");
    }

    /// A whitespace-only name counts as "not set" (the field isn't a way to blank
    /// out the label).
    #[test]
    fn blank_custom_name_falls_back_to_default() {
        let names = CharacterNames {
            user: "   ".into(),
            ..Default::default()
        };
        let out =
            format_conversation("", &[Message::user("привет")], &plain(), &names, ru()).unwrap();
        assert_eq!(out, "Пользователь:\nпривет");
    }
}
