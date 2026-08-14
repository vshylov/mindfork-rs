//! The `chat://` address scheme — one format for one conversation, shared by
//! the tools that mint addresses and the feed that draws them (spec §9.11,
//! §11.3; docs/research/chat-uri-links.md).
//!
//! An address is `chat://` plus a prefix of the conversation's uuid in its
//! `simple()` (dash-free) form. `chat_search` prints it, `chat_read` accepts it
//! back, the model cites it to the user, and the feed turns it into something
//! the user can open — so the string has exactly one producer
//! ([`uri`]/[`short_id`]) and one reader ([`hex_needle`]).
//!
//! Two rules the callers depend on:
//!
//! - **A reference is only a reference if it resolves.** [`find_refs`] returns
//!   nothing for an id no known conversation carries, so the feed never styles
//!   a door that opens onto nothing (docs/lessons.md §4).
//! - **The prefix floor is half a short id.** Below four characters a
//!   hex-looking word ("cafe", "beef") would routinely shadow a title in
//!   `chat_read`'s resolution ladder.

use std::ops::Range;

use uuid::Uuid;

/// The scheme every chat address is written in. Matched case-insensitively on
/// the way in, always produced lowercase.
pub const SCHEME: &str = "chat://";

/// How many characters of a chat id serve as its address — unambiguous at any
/// realistic chat count, and short enough to read.
pub const SHORT_ID_CHARS: usize = 8;

/// Shortest id prefix a reference may carry (half a short id — see the module
/// docs).
pub const MIN_ID_CHARS: usize = SHORT_ID_CHARS / 2;

/// Longest one: a uuid's `simple()` form is 32 hex characters, so anything
/// longer is not an id at all.
pub const MAX_ID_CHARS: usize = 32;

/// The address of a conversation, without the scheme.
pub fn short_id(id: Uuid) -> String {
    let hex = id.simple().to_string();
    hex[..SHORT_ID_CHARS.min(hex.len())].to_string()
}

/// The full address of a conversation — what the tools print and the model
/// cites.
pub fn uri(id: Uuid) -> String {
    format!("{SCHEME}{}", short_id(id))
}

/// Reads a reference as an id prefix: strips a leading [`SCHEME`] and any
/// dashes, lower-cases, and accepts only hex of a plausible length. `None`
/// means "this is not an id" — the caller is free to read it as a title
/// instead, which is what `chat_read` does.
pub fn hex_needle(reference: &str) -> Option<String> {
    let trimmed = reference.trim();
    // Compared as bytes: the scheme is ASCII, and `reference` may not have a
    // character boundary at byte 7 (a Cyrillic title reaching `chat_read`).
    let body = match trimmed.as_bytes() {
        b if b.len() >= SCHEME.len()
            && b[..SCHEME.len()].eq_ignore_ascii_case(SCHEME.as_bytes()) =>
        {
            &trimmed[SCHEME.len()..]
        }
        _ => trimmed,
    };
    let hex: String = body
        .chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (hex.len() >= MIN_ID_CHARS
        && hex.len() <= MAX_ID_CHARS
        && hex.chars().all(|c| c.is_ascii_hexdigit()))
    .then_some(hex)
}

/// The one conversation whose id starts with `hex`, if exactly one does.
/// Several candidates are as good as none here: an ambiguous address cannot be
/// followed, and the tools have their own ladder for saying so.
pub fn resolve_prefix(known: &[Uuid], hex: &str) -> Option<Uuid> {
    let mut hits = known
        .iter()
        .filter(|id| id.simple().to_string().starts_with(hex));
    let first = hits.next()?;
    hits.next().is_none().then_some(*first)
}

/// One resolved reference inside a string: where it sits, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLink {
    /// Byte range of the whole address (scheme included) within the text.
    pub range: Range<usize>,
    pub chat: Uuid,
}

/// Finds every **resolvable** `chat://` address in `text`, left to right and
/// non-overlapping. Unresolvable ones are skipped entirely rather than
/// reported: a caller that styled them would be promising a jump it cannot
/// make.
///
/// The scan runs over rendered text (spec §11.3), so it must tolerate what
/// surrounds an address in prose — a closing paren from the markdown link form
/// `[Title](chat://id)`, a full stop, a comma. Anything that is not a hex digit
/// or a dash ends the address, and trailing dashes are dropped.
pub fn find_refs(text: &str, known: &[Uuid]) -> Vec<ChatLink> {
    // The overwhelmingly common case is a line with no address at all.
    if known.is_empty() || !text.contains("://") {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + SCHEME.len() <= bytes.len() {
        // Byte comparison throughout: the scheme is ASCII, and `at` walks over
        // a string that may hold multi-byte characters anywhere.
        if !bytes[at..at + SCHEME.len()].eq_ignore_ascii_case(SCHEME.as_bytes()) {
            at += 1;
            continue;
        }
        let body = at + SCHEME.len();
        let mut end = body;
        while end < bytes.len() && (bytes[end].is_ascii_hexdigit() || bytes[end] == b'-') {
            end += 1;
        }
        // A trailing dash belongs to the prose, not to the address.
        while end > body && bytes[end - 1] == b'-' {
            end -= 1;
        }
        match hex_needle(&text[body..end]).and_then(|hex| resolve_prefix(known, &hex)) {
            Some(chat) => {
                out.push(ChatLink {
                    range: at..end,
                    chat,
                });
                at = end;
            }
            None => at = body,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids whose `simple()` forms differ from the first character, so a
    /// four-character prefix already resolves.
    fn ids() -> Vec<Uuid> {
        vec![
            Uuid::parse_str("1a2b3c4d-0000-4000-8000-000000000001").unwrap(),
            Uuid::parse_str("9f8e7d6c-0000-4000-8000-000000000002").unwrap(),
        ]
    }

    #[test]
    fn uri_is_scheme_plus_short_id() {
        let id = ids()[0];
        assert_eq!(uri(id), "chat://1a2b3c4d");
        assert_eq!(short_id(id).len(), SHORT_ID_CHARS);
    }

    #[test]
    fn hex_needle_accepts_both_forms_and_case() {
        assert_eq!(hex_needle("chat://1A2B3C4D").as_deref(), Some("1a2b3c4d"));
        assert_eq!(hex_needle("CHAT://1a2b3c4d").as_deref(), Some("1a2b3c4d"));
        assert_eq!(hex_needle("1a2b3c4d").as_deref(), Some("1a2b3c4d"));
        assert_eq!(
            hex_needle("1a2b3c4d-0000-4000-8000-000000000001").as_deref(),
            Some("1a2b3c4d000040008000000000000001")
        );
    }

    /// The floor that keeps a hex-looking word from shadowing a title, and the
    /// ceiling past which a string is not an id.
    #[test]
    fn hex_needle_rejects_non_ids() {
        assert!(hex_needle("abc").is_none());
        assert!(hex_needle("a project").is_none());
        assert!(hex_needle("").is_none());
        assert!(hex_needle(&"a".repeat(MAX_ID_CHARS + 1)).is_none());
        // "cafe" is hex and long enough — deliberately accepted, see the module
        // docs; the ladder above it falls through to titles anyway.
        assert!(hex_needle("cafe").is_some());
    }

    #[test]
    fn resolve_prefix_needs_exactly_one() {
        let ids = ids();
        assert_eq!(resolve_prefix(&ids, "1a2b"), Some(ids[0]));
        assert_eq!(resolve_prefix(&ids, "dead"), None);
        // A prefix every id shares is not an address.
        let same = vec![
            Uuid::parse_str("aaaaaaaa-0000-4000-8000-000000000001").unwrap(),
            Uuid::parse_str("aaaaaaaa-0000-4000-8000-000000000002").unwrap(),
        ];
        assert_eq!(resolve_prefix(&same, "aaaa"), None);
    }

    #[test]
    fn finds_a_bare_reference() {
        let ids = ids();
        let text = format!("see {} for the numbers", uri(ids[0]));
        let found = find_refs(&text, &ids);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].chat, ids[0]);
        assert_eq!(&text[found[0].range.clone()], "chat://1a2b3c4d");
    }

    /// What the markdown link form leaves on screen: `Title (chat://id)`.
    #[test]
    fn closing_paren_and_punctuation_are_not_part_of_the_address() {
        let ids = ids();
        for text in [
            "Budget (chat://1a2b3c4d)",
            "Budget chat://1a2b3c4d.",
            "Budget chat://1a2b3c4d, and more",
            "Budget chat://1a2b3c4d-",
        ] {
            let found = find_refs(text, &ids);
            assert_eq!(found.len(), 1, "{text}");
            assert_eq!(&text[found[0].range.clone()], "chat://1a2b3c4d", "{text}");
        }
    }

    #[test]
    fn finds_several_and_keeps_them_ordered() {
        let ids = ids();
        let text = format!("{} then {}", uri(ids[0]), uri(ids[1]));
        let found = find_refs(&text, &ids);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].chat, ids[0]);
        assert_eq!(found[1].chat, ids[1]);
        assert!(found[0].range.end <= found[1].range.start);
    }

    /// The rule the feed relies on: an address that goes nowhere is not an
    /// address (docs/lessons.md §4).
    #[test]
    fn unresolvable_references_are_not_returned() {
        let ids = ids();
        assert!(find_refs("chat://deadbeef", &ids).is_empty());
        assert!(find_refs("chat://abc", &ids).is_empty());
        assert!(find_refs("chat://", &ids).is_empty());
        assert!(find_refs("no address here", &ids).is_empty());
        assert!(find_refs(&uri(ids[0]), &[]).is_empty());
    }

    /// A full uuid pasted instead of the short form still resolves.
    #[test]
    fn accepts_a_full_uuid_in_the_address() {
        let ids = ids();
        let text = "chat://1a2b3c4d-0000-4000-8000-000000000001!";
        let found = find_refs(text, &ids);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].chat, ids[0]);
        assert!(!text[found[0].range.clone()].ends_with('!'));
    }

    /// Multi-byte text around an address must not shift its range or panic on
    /// a character boundary.
    #[test]
    fn survives_non_ascii_neighbours() {
        let ids = ids();
        let text = format!("см. беседу {} — там числа", uri(ids[0]));
        let found = find_refs(&text, &ids);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].range.clone()], "chat://1a2b3c4d");
    }

    #[test]
    fn another_scheme_is_left_alone() {
        let ids = ids();
        assert!(find_refs("https://example.com/1a2b3c4d", &ids).is_empty());
    }
}
