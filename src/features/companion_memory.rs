//! What a companion knows before the conversation in front of it began, put
//! into the system prompt (spec §6.6, docs/research/companion-platform.md C2):
//! the profile's **conversations before this one** (C2 (b)) and the **notes**
//! nearest this one (C2 (a)). Each is a profile switch of its own —
//! `Profile.companion.earlier_chats`, `Profile.companion.notes_in_prompt` — off
//! by default: an assistant profile is sent exactly what it was before.
//!
//! Both blocks are decided by one moment, the **memory cut** ([`memory_cut`]):
//! when the part of the conversation the request carries began. What was said
//! and written before it is what the companion "already knew"; whatever this
//! conversation produces after it is in the conversation itself. So a block is
//! the same text from turn to turn and the prompt's prefix holds (spec §6.6) —
//! only its day-granular age labels move, as the self-model's do — and it moves
//! at a compaction roll, which re-prefills the prompt anyway.
//!
//! The notes are picked asynchronously (embedding the opening) in
//! `features::tools::notes::prompt_notes`; this module renders them.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entities::chat::Chat;
use crate::entities::message::MessageRole;
use crate::entities::self_model::{age_label, fit_observations};
use crate::shared::i18n::Locale;

/// How many earlier conversations the block lists (the owner's F4 (a): three).
pub const EARLIER_CHATS_SHOWN: usize = 3;

/// How many notes the notes block carries at most.
pub const NOTES_SHOWN: usize = 4;

/// The notes block's character cap, its header and trailer included. The picks
/// share what is left by water-filling, as the self-model's observations do:
/// a short note keeps its length, the longer ones are cut at a word boundary.
pub const NOTES_PROMPT_CAP: usize = 1600;

/// The moment the conversation in front of the model began: the first user
/// message with text among those the request carries — after the folded range
/// when a summary is in force, so the cut moves forward at each compaction roll
/// with the query the notes are picked by (`observation_query`). A conversation
/// with no such message yet (a greeting alone) is cut at its newest message, or
/// at its creation.
pub fn memory_cut(chat: &Chat, compaction_enabled: bool) -> DateTime<Utc> {
    let upto = chat
        .compaction_view(compaction_enabled)
        .map_or(0, |(_, upto)| upto);
    let visible = &chat.messages[upto.min(chat.messages.len())..];
    visible
        .iter()
        .find(|m| m.role == MessageRole::User && !m.text.trim().is_empty())
        .or(visible.last())
        .map_or(chat.created_at, |m| m.timestamp)
}

/// One of the profile's conversations before this one, as the block lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct EarlierChat {
    pub id: Uuid,
    pub title: String,
    /// Its last message before the cut — what the age label measures, and the
    /// order. A message written after the cut does not move it, so the block
    /// holds while the person goes back to an older conversation.
    pub last_at: DateTime<Utc>,
}

/// The profile's conversations before `current`, newest first, at most
/// [`EARLIER_CHATS_SHOWN`]: chats of the same profile, not hidden, with a user
/// message before the cut — a chat holding nothing but its greeting was never
/// a conversation. Sub-agent transcripts are not chats of the list and are not
/// counted. Pure, over the orchestrator's in-memory chats.
pub fn earlier_chats(chats: &[Chat], current: &Chat, cut: DateTime<Utc>) -> Vec<EarlierChat> {
    let mut found: Vec<EarlierChat> = chats
        .iter()
        .filter(|c| c.profile_id == current.profile_id && c.id != current.id && !c.is_hidden)
        .filter_map(|c| {
            let before = c.messages.iter().filter(|m| m.timestamp < cut);
            let spoke = before
                .clone()
                .any(|m| m.role == MessageRole::User && !m.text.trim().is_empty());
            let last_at = before.map(|m| m.timestamp).max()?;
            spoke.then(|| EarlierChat {
                id: c.id,
                title: one_line(&c.title),
                last_at,
            })
        })
        .collect();
    found.sort_by(|a, b| b.last_at.cmp(&a.last_at).then(a.id.cmp(&b.id)));
    found.truncate(EARLIER_CHATS_SHOWN);
    found
}

/// A title as one line: whitespace runs collapsed, so a pasted title cannot
/// break the list.
fn one_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The "conversations before this one" block, or `None` when there are none.
/// With `addresses` (the turn offers `chat_read`) each line carries the
/// conversation's `chat://` address and the block says how to read one; without,
/// it names no tool the turn does not have (docs/lessons.md §4).
pub fn render_earlier(
    list: &[EarlierChat],
    now: DateTime<Utc>,
    addresses: bool,
    loc: &Locale,
) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let mut out = loc.t("prompt.memory.earlier.header").to_string();
    for c in list {
        let age = age_label(c.last_at, now, loc);
        let title = if c.title.is_empty() {
            loc.t("defaults.chat_title").to_string()
        } else {
            c.title.clone()
        };
        out.push('\n');
        out.push_str(&if addresses {
            loc.tf(
                "prompt.memory.earlier.item_addr",
                &[
                    ("age", &age),
                    ("title", &title),
                    ("addr", &crate::features::chat_links::uri(c.id)),
                ],
            )
        } else {
            loc.tf(
                "prompt.memory.earlier.item",
                &[("age", &age), ("title", &title)],
            )
        });
    }
    if addresses {
        out.push('\n');
        out.push_str(loc.t("prompt.memory.earlier.read"));
    }
    Some(out)
}

/// The notes block: `picks` are (when the note was last written, its text),
/// nearest the opening first, at most [`NOTES_SHOWN`] of them, sharing what
/// [`NOTES_PROMPT_CAP`] leaves after the header and the trailer. With `recall`
/// (the turn offers `note_recall`) the block says where the rest is. `None` when
/// there are no picks, or not even the first one's beginning would fit.
///
/// There is no "latest notes" fallback for an opening near none of them: in the
/// probe it carried the person's freshest news into answers about anything —
/// TCP, the planets — on all five models, where a block picked by relevance
/// alone left those answers as they were (docs/research/companion-platform.md
/// §3.4).
pub fn render_notes(
    picks: &[(DateTime<Utc>, &str)],
    recall: bool,
    now: DateTime<Utc>,
    loc: &Locale,
) -> Option<String> {
    if picks.is_empty() {
        return None;
    }
    let header = loc.t("prompt.memory.notes.header");
    let trailer = recall.then(|| loc.t("prompt.memory.notes.more"));
    let frame = header.chars().count() + 1 + trailer.map_or(0, |t| t.chars().count());
    let lines: Vec<(String, &str)> = picks
        .iter()
        .take(NOTES_SHOWN)
        .map(|(at, text)| (age_label(*at, now, loc), text.trim()))
        .collect();
    let body = fit_observations(&lines, NOTES_PROMPT_CAP.saturating_sub(frame), loc)?;
    let mut out = format!("{header}\n{body}");
    if let Some(t) = trailer {
        out.push_str(t);
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::Message;
    use crate::entities::profile::Profile;
    use chrono::Duration;

    fn en() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::En)
    }

    fn at(base: DateTime<Utc>, hours_ago: i64) -> DateTime<Utc> {
        base - Duration::hours(hours_ago)
    }

    /// A chat of `profile` whose messages were written `hours_ago` before `base`.
    fn chat(
        profile: &Profile,
        title: &str,
        msgs: &[(MessageRole, i64)],
        base: DateTime<Utc>,
    ) -> Chat {
        let mut c = Chat::from_profile(profile, title);
        for (role, h) in msgs {
            let mut m = Message::new(*role, format!("{role:?} at -{h}h"));
            m.timestamp = at(base, *h);
            c.messages.push(m);
        }
        c
    }

    #[test]
    fn the_cut_is_the_first_user_message_the_request_carries() {
        let base = Utc::now();
        let p = Profile::new("P", "s");
        // A greeting written at creation, then the person's opening hours later.
        let c = chat(
            &p,
            "t",
            &[
                (MessageRole::Assistant, 30),
                (MessageRole::User, 2),
                (MessageRole::Assistant, 1),
            ],
            base,
        );
        assert_eq!(memory_cut(&c, true), at(base, 2));
        // A greeting alone: its own time.
        let g = chat(&p, "t", &[(MessageRole::Assistant, 5)], base);
        assert_eq!(memory_cut(&g, true), at(base, 5));
        // Nothing at all: the chat's creation.
        let e = Chat::from_profile(&p, "t");
        assert_eq!(memory_cut(&e, true), e.created_at);
    }

    #[test]
    fn earlier_chats_are_the_profiles_newest_before_the_cut() {
        let base = Utc::now();
        let p = Profile::new("P", "s");
        let other = Profile::new("Q", "s");
        let current = chat(&p, "now", &[(MessageRole::User, 0)], base);
        let cut = memory_cut(&current, true);
        let a = chat(
            &p,
            "a",
            &[(MessageRole::User, 50), (MessageRole::Assistant, 49)],
            base,
        );
        let b = chat(&p, "b", &[(MessageRole::User, 30)], base);
        let c = chat(&p, "c", &[(MessageRole::User, 70)], base);
        let d = chat(&p, "d", &[(MessageRole::User, 90)], base);
        let foreign = chat(&other, "foreign", &[(MessageRole::User, 1)], base);
        let mut hidden = chat(&p, "hidden", &[(MessageRole::User, 1)], base);
        hidden.is_hidden = true;
        let greeting_only = chat(&p, "greeting", &[(MessageRole::Assistant, 2)], base);
        let all = vec![
            current.clone(),
            a.clone(),
            b.clone(),
            c.clone(),
            d,
            foreign,
            hidden,
            greeting_only,
        ];
        let got = earlier_chats(&all, &current, cut);
        let titles: Vec<&str> = got.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["b", "a", "c"]);
        // The age is the last message before the cut.
        assert_eq!(got[1].last_at, at(base, 49));
    }

    #[test]
    fn a_message_after_the_cut_moves_nothing() {
        // The person went back to an older chat after this one began: the block
        // (and so the prompt's prefix) is what it was.
        let base = Utc::now();
        let p = Profile::new("P", "s");
        let current = chat(
            &p,
            "now",
            &[(MessageRole::User, 10), (MessageRole::User, 0)],
            base,
        );
        let cut = memory_cut(&current, true);
        let older = chat(
            &p,
            "older",
            &[(MessageRole::User, 40), (MessageRole::User, 1)],
            base,
        );
        let newer = chat(&p, "newer", &[(MessageRole::User, 20)], base);
        let got = earlier_chats(&[current.clone(), older, newer], &current, cut);
        let order: Vec<(&str, DateTime<Utc>)> =
            got.iter().map(|e| (e.title.as_str(), e.last_at)).collect();
        assert_eq!(order, [("newer", at(base, 20)), ("older", at(base, 40))]);
    }

    #[test]
    fn the_earlier_block_names_addresses_only_with_chat_read() {
        let base = Utc::now();
        let id = Uuid::parse_str("3f2a9c1e-0000-4000-8000-000000000001").unwrap();
        let list = [EarlierChat {
            id,
            title: "Taper plan".into(),
            last_at: base - Duration::days(2),
        }];
        let with = render_earlier(&list, base, true, en()).unwrap();
        assert_eq!(
            with,
            "[Conversations before this one]\n- (2d) Taper plan — chat://3f2a9c1e\nRead one with chat_read."
        );
        let without = render_earlier(&list, base, false, en()).unwrap();
        assert_eq!(
            without,
            "[Conversations before this one]\n- (2d) Taper plan"
        );
        assert!(!without.contains("chat_read") && !without.contains("chat://"));
        assert_eq!(render_earlier(&[], base, true, en()), None);
    }

    #[test]
    fn a_title_is_one_line_and_an_empty_one_is_named() {
        let base = Utc::now();
        let list = [EarlierChat {
            id: Uuid::nil(),
            title: one_line("  two\n lines  "),
            last_at: base,
        }];
        assert!(
            render_earlier(&list, base, false, en())
                .unwrap()
                .ends_with("- (today) two lines")
        );
        let list = [EarlierChat {
            id: Uuid::nil(),
            title: String::new(),
            last_at: base,
        }];
        let block = render_earlier(&list, base, false, en()).unwrap();
        assert!(
            block.ends_with(&format!("- (today) {}", en().t("defaults.chat_title"))),
            "{block}"
        );
    }

    #[test]
    fn the_notes_block_says_where_the_rest_is_only_with_note_recall() {
        let base = Utc::now();
        let picks = [
            (
                base - Duration::days(5),
                "Interview at Lumen Pay on Monday.",
            ),
            (base - Duration::days(40), "Allergic to peanuts."),
        ];
        let block = render_notes(&picks, true, base, en()).unwrap();
        assert_eq!(
            block,
            "[Your notes nearest this conversation]\n- (5d) Interview at Lumen Pay on Monday.\n- (1mo) Allergic to peanuts.\nMore notes are found with note_recall."
        );
        let without = render_notes(&picks, false, base, en()).unwrap();
        assert!(!without.contains("note_recall"), "{without}");
        assert_eq!(render_notes(&[], true, base, en()), None);
    }

    #[test]
    fn long_notes_share_the_cap() {
        let base = Utc::now();
        let long = "word ".repeat(400);
        let picks: Vec<(DateTime<Utc>, &str)> = (0..6).map(|_| (base, long.as_str())).collect();
        let block = render_notes(&picks, true, base, en()).unwrap();
        assert!(
            block.chars().count() <= NOTES_PROMPT_CAP,
            "{}",
            block.chars().count()
        );
        // Never more than NOTES_SHOWN picks, each cut at a word with an ellipsis.
        assert_eq!(block.matches("\n- (today) ").count(), NOTES_SHOWN);
        assert!(block.contains("…"));
    }

    #[test]
    fn every_locale_has_the_blocks() {
        let base = Utc::now();
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let list = [EarlierChat {
                id: Uuid::nil(),
                title: "t".into(),
                last_at: base,
            }];
            let e = render_earlier(&list, base, true, loc).unwrap();
            let n = render_notes(&[(base, "x")], true, base, loc).unwrap();
            // An unresolved key would come back as itself.
            for s in [e, n] {
                assert!(!s.contains('{') && !s.contains("prompt."), "{lang:?}: {s}");
            }
        }
    }
}
