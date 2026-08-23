//! Renaming a chat: title normalization (pure logic) and building a
//! conversation digest for the model's auto-title. See spec §11.2.

use crate::entities::message::{Message, MessageRole};
use crate::shared::i18n::Locale;
use crate::shared::title::sanitize_title;

/// The conversation-character budget passed to the model for coming up with a title.
/// If the conversation is longer — take the start and the end (the topic is usually set at the start,
/// but may shift toward the end of the conversation). See spec §11.2.
pub const TITLE_CONTEXT_BUDGET: usize = 4000;

/// The model's system message for auto-titling a chat, in the agent-scaffold language
/// (`loc`, axis A). The title is still requested "in the conversation's language" — so the
/// scaffold's language doesn't force the title's language (docs/history/i18n.md, fork 6).
pub fn title_system_message(loc: &Locale) -> String {
    loc.t("prompt.title.system").to_string()
}

/// Builds a compact conversation digest for the auto-title request: tags
/// roles (in the scaffold's language `loc`), skips system/tool and empty
/// messages. If the total size exceeds [`TITLE_CONTEXT_BUDGET`], takes the start
/// and the end (the middle is dropped). Returns `None` if there are no substantive
/// messages.
pub fn build_conversation_digest(messages: &[Message], loc: &Locale) -> Option<String> {
    let lines: Vec<String> = messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .filter(|m| !m.text.trim().is_empty())
        .map(|m| {
            let who = match m.role {
                MessageRole::User => loc.t("digest.role.user"),
                _ => loc.t("digest.role.assistant"),
            };
            format!("{who}: {}", m.text.trim())
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(truncate_middle(&lines.join("\n"), TITLE_CONTEXT_BUDGET))
}

/// Limits a string to `budget` characters (by Unicode scalar, not bytes): if
/// longer — takes the start (≈⅔ of the budget) and the end, replacing the middle with a marker.
fn truncate_middle(text: &str, budget: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= budget {
        return text.to_string();
    }
    let head = budget * 2 / 3;
    let tail = budget - head;
    let head_str: String = chars[..head].iter().collect();
    let tail_str: String = chars[chars.len() - tail..].iter().collect();
    format!("{head_str}\n…\n{tail_str}")
}

/// Whether the conversation already holds a substantive assistant reply — an
/// assistant message with non-empty text **after** the first user message. A
/// profile greeting (an assistant message before any user one) does not count.
///
/// The automatic titling trigger (spec §11.2) reads this *before* a turn's
/// messages are applied: `false` there means whatever reply the turn delivered
/// is the chat's first, however many earlier turns were cancelled or failed
/// before producing text — and an existing pre-feature chat can never match.
pub fn has_assistant_reply(messages: &[Message]) -> bool {
    let Some(first_user) = messages.iter().position(|m| m.role == MessageRole::User) else {
        return false;
    };
    messages[first_user..]
        .iter()
        .any(|m| m.role == MessageRole::Assistant && !m.text.trim().is_empty())
}

/// Whether the last-appended user message is the conversation's **first** one —
/// the `AfterUserMessage` trigger point of automatic titling (spec §11.2).
pub fn is_first_user_message(messages: &[Message]) -> bool {
    messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .count()
        == 1
}

/// Cleans a model-generated title: takes the first non-empty line, strips
/// surrounding quotes/asterisks/backticks, and normalizes via
/// [`crate::shared::title::sanitize_title`].
/// Returns `None` if the string is empty after cleaning.
pub fn clean_generated_title(raw: &str) -> Option<String> {
    let first = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let trimmed = first.trim_matches(|c: char| {
        matches!(c, '"' | '\'' | '«' | '»' | '*' | '`') || c.is_whitespace()
    });
    sanitize_title(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference locale (ru) — assertions on Russian roles pin the ru bundle.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn digest_labels_roles_and_skips_system_and_tools() {
        let msgs = vec![
            Message::new(MessageRole::System, "ты ассистент"),
            Message::user("как дела?"),
            Message::assistant("хорошо"),
            Message::new(MessageRole::Tool, "tool output"),
        ];
        let digest = build_conversation_digest(&msgs, ru()).unwrap();
        assert_eq!(digest, "Пользователь: как дела?\nАссистент: хорошо");
    }

    /// Per-language (§3.5): digest roles come from the active language's bundle (an en profile
    /// gets "User:/Assistant:", not the Russian roles).
    #[test]
    fn digest_roles_localized_for_all_langs() {
        let msgs = vec![Message::user("hi"), Message::assistant("yo")];
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let d = build_conversation_digest(&msgs, l).unwrap();
            assert!(d.contains(l.t("digest.role.user")), "{lang:?}: {d}");
            assert!(d.contains(l.t("digest.role.assistant")), "{lang:?}: {d}");
        }
    }

    #[test]
    fn digest_is_none_without_meaningful_messages() {
        let msgs = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("   "),
        ];
        assert!(build_conversation_digest(&msgs, ru()).is_none());
    }

    #[test]
    fn digest_truncates_long_conversation_keeping_head_and_tail() {
        // A long user message — should be truncated with a middle marker.
        let long = "слово ".repeat(2000); // ~12000 characters
        let msgs = vec![Message::user(long)];
        let digest = build_conversation_digest(&msgs, ru()).unwrap();
        assert!(digest.chars().count() <= TITLE_CONTEXT_BUDGET + 16);
        assert!(digest.contains('…'), "the middle-truncation marker");
        assert!(digest.starts_with("Пользователь:"));
    }

    /// The trigger predicates (spec §11.2): a greeting is not a reply, an
    /// empty-text assistant turn is not a reply, tool/system messages are
    /// invisible to both.
    #[test]
    fn trigger_predicates_see_through_greetings_and_empty_turns() {
        // Greeting only: no user message yet — neither predicate holds.
        let greeting = vec![Message::assistant("привет!")];
        assert!(!has_assistant_reply(&greeting));
        assert!(!is_first_user_message(&greeting));

        // Greeting + the first user message: the reply is still owed.
        let mut msgs = greeting.clone();
        msgs.push(Message::user("вопрос"));
        assert!(!has_assistant_reply(&msgs));
        assert!(is_first_user_message(&msgs));

        // A cancelled/failed turn left an empty assistant message — still owed.
        msgs.push(Message::assistant("   "));
        assert!(!has_assistant_reply(&msgs));

        // System/tool noise doesn't count as a reply either.
        msgs.push(Message::new(MessageRole::System, "sys"));
        msgs.push(Message::new(MessageRole::Tool, "tool output"));
        assert!(!has_assistant_reply(&msgs));

        // A substantive reply lands: the chat is answered, and a second user
        // message means the first-message point is past.
        msgs.push(Message::assistant("ответ"));
        assert!(has_assistant_reply(&msgs));
        msgs.push(Message::user("ещё"));
        assert!(!is_first_user_message(&msgs));
    }

    #[test]
    fn clean_title_strips_quotes_and_takes_first_line() {
        assert_eq!(
            clean_generated_title("«Планы на выходные»").as_deref(),
            Some("Планы на выходные")
        );
        assert_eq!(
            clean_generated_title("\"Рецепт борща\"\nещё текст").as_deref(),
            Some("Рецепт борща")
        );
        assert_eq!(clean_generated_title("   \n  ").as_deref(), None);
    }
}
