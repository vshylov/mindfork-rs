//! The character-card macros in a persona's text (spec §10,
//! docs/research/companion-platform.md C6): `{{char}}` and `{{user}}`, the
//! comments, `{{reverse:…}}`, and the random ones — `{{random:…}}`,
//! `{{pick:…}}`, `{{roll:N}}`. Case-insensitive, as the V3 spec advises; a
//! macro this module does not know is left as it was written.
//!
//! A persona's macros are substituted when a request is built ([`persona`]),
//! so both a rename and the prompt's prefix hold:
//!
//! - the names are the profile's as they are now, so renaming the character or
//!   the person applies to every chat at once, the way the feed's role names do
//!   (spec §10);
//! - the random macros are drawn from the chat's own id, not from the clock: a
//!   pick re-drawn on every request would change the system message turn to
//!   turn and break the cached prefix (spec §6.6), and one drawn per chat is
//!   what a card's author means by it.
//!
//! A greeting becomes a message of the conversation, so its macros are
//! substituted once, when it is placed — [`names`] and [`roll`] with
//! [`below`]'s real randomness.
//!
//! A text without `{{` comes back borrowed, so a persona that uses no macro —
//! every persona written before cards — is sent byte for byte as before.

use std::borrow::Cow;

/// What `{{char}}` and `{{user}}` stand for.
#[derive(Debug, Clone, Copy)]
pub struct Names<'a> {
    pub char: &'a str,
    pub user: &'a str,
}

/// Everything a persona's macros need when a request is built.
#[derive(Debug, Clone, Copy)]
pub struct Persona<'a> {
    pub names: Names<'a>,
    /// The chat's own draw for the random macros ([`seed_of`]).
    pub seed: u64,
}

/// A chat's seed for its persona's random macros: its id, folded.
pub fn seed_of(chat: uuid::Uuid) -> u64 {
    let (hi, lo) = chat.as_u64_pair();
    hi ^ lo
}

/// A persona as the model reads it: the names substituted, the comments
/// dropped, and the random macros drawn — the same draw on every request of
/// the chat whose seed this is.
pub fn persona<'t>(text: &'t str, p: Persona<'_>) -> Cow<'t, str> {
    let named = names(text, p.names);
    let mut state = p.seed;
    let rolled = roll(&named, &mut |n| {
        // SplitMix64: a few lines of well-mixed, reproducible numbers.
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % n.max(1)
    });
    match rolled {
        Cow::Borrowed(_) => named,
        Cow::Owned(s) => Cow::Owned(s),
    }
}

/// `{{char}}`, `{{user}}`, `{{reverse:…}}` substituted and the comments
/// (`{{// …}}`, `{{comment: …}}`, `{{hidden_key: …}}`) dropped.
pub fn names<'t>(text: &'t str, names: Names<'_>) -> Cow<'t, str> {
    expand(text, |inner| {
        let lower = inner.to_ascii_lowercase();
        if lower == "char" {
            return Some(names.char.to_string());
        }
        if lower == "user" {
            return Some(names.user.to_string());
        }
        if lower.starts_with("//")
            || lower.starts_with("comment:")
            || lower.starts_with("hidden_key:")
        {
            return Some(String::new());
        }
        if let Some(rest) = strip_prefix_ci(inner, "reverse:") {
            return Some(rest.chars().rev().collect());
        }
        None
    })
}

/// `{{user}}` alone replaced with `with`, every other macro left for later —
/// how a card's example exchanges are stored, their other side a stranger
/// rather than the person (docs/research/companion-platform.md §3.7).
pub fn replace_user<'t>(text: &'t str, with: &str) -> Cow<'t, str> {
    expand(text, |inner| {
        inner.eq_ignore_ascii_case("user").then(|| with.to_string())
    })
}

/// `{{random:…}}`, `{{pick:…}}` and `{{roll:N}}` drawn with `below`, which
/// returns a number in `0..n`.
pub fn roll<'t>(text: &'t str, below: &mut dyn FnMut(u64) -> u64) -> Cow<'t, str> {
    expand(text, |inner| {
        if let Some(rest) =
            strip_prefix_ci(inner, "random:").or_else(|| strip_prefix_ci(inner, "pick:"))
        {
            let items = choices(rest);
            if items.is_empty() {
                return Some(String::new());
            }
            return Some(items[below(items.len() as u64) as usize].clone());
        }
        if let Some(rest) = strip_prefix_ci(inner, "roll:") {
            let n = rest.trim();
            let n = n
                .strip_prefix('d')
                .or_else(|| n.strip_prefix('D'))
                .unwrap_or(n);
            let sides: u64 = n.parse().ok().filter(|&s| s > 0)?;
            return Some((below(sides) + 1).to_string());
        }
        None
    })
}

/// The process's randomness for [`roll`]: a fresh v4 UUID's bits — no dice
/// worth a dependency of their own.
pub fn below(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    (uuid::Uuid::new_v4().as_u128() % u128::from(n)) as u64
}

/// The items of a random macro: SillyTavern's `random::a::b` form when the
/// list opens with a colon, else comma-separated with `\,` for a literal comma.
fn choices(rest: &str) -> Vec<String> {
    if let Some(list) = rest.strip_prefix(':') {
        return list.split("::").map(str::to_string).collect();
    }
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&',') => {
                cur.push(',');
                chars.next();
            }
            ',' => items.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    items.push(cur);
    items
}

fn strip_prefix_ci<'s>(s: &'s str, prefix: &str) -> Option<&'s str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// Every `{{…}}` in `text` handed to `f` (its inside, trimmed); `Some` replaces
/// the macro, `None` keeps it as written. Macros do not nest: a macro ends at
/// the first `}}`.
fn expand<'t>(text: &'t str, mut f: impl FnMut(&str) -> Option<String>) -> Cow<'t, str> {
    if !text.contains("{{") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str(&rest[open..]);
            return Cow::Owned(out);
        };
        let inner = after[..close].trim();
        match f(inner) {
            Some(s) => out.push_str(&s),
            None => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: Names = Names {
        char: "Mara",
        user: "Alex",
    };

    #[test]
    fn names_are_substituted_in_any_case() {
        assert_eq!(
            names("{{char}} greets {{user}}. {{Char}}/{{USER}}/{{ char }}", N),
            "Mara greets Alex. Mara/Alex/Mara"
        );
    }

    /// A text without macros is the same allocation — the request a persona
    /// without them builds is the one it built before.
    #[test]
    fn a_text_without_macros_is_borrowed() {
        let s = "You are a helpful assistant. {not a macro}";
        assert!(matches!(names(s, N), Cow::Borrowed(_)));
        assert!(matches!(roll(s, &mut |_| 0), Cow::Borrowed(_)));
    }

    #[test]
    fn comments_go_and_reverse_reverses_and_the_unknown_stays() {
        assert_eq!(
            names(
                "a{{// note to self}}b{{comment: shown}}c{{hidden_key: k}}d {{reverse:abc}} {{time}} {{original}}",
                N
            ),
            "abcd cba {{time}} {{original}}"
        );
        // An unclosed macro is text.
        assert_eq!(names("{{char}} and {{user", N), "Mara and {{user");
    }

    #[test]
    fn random_and_pick_draw_one_item_and_roll_a_number() {
        let mut seq = [2u64, 0, 5].into_iter();
        let mut below = |n: u64| seq.next().unwrap() % n;
        assert_eq!(
            roll("{{random:a,b,c}} {{pick::x::y}} {{roll:d6}}", &mut below),
            "c x 6"
        );
        // An escaped comma is part of an item.
        assert_eq!(roll("{{random:one\\, two,three}}", &mut |_| 0), "one, two");
        // A roll that is not a number stays as written; names are not rolled.
        assert_eq!(
            roll("{{roll:many}} {{char}}", &mut |_| 0),
            "{{roll:many}} {{char}}"
        );
    }

    /// One chat draws the same on every request; chats differ between them.
    #[test]
    fn a_persona_draws_the_same_for_its_chat_every_time() {
        let text = "{{char}}: {{random:a,b,c,d,e,f,g,h}}{{roll:1000}}";
        let at = |seed| persona(text, Persona { names: N, seed }).into_owned();
        let first = at(7);
        assert!(first.starts_with("Mara: "), "{first}");
        assert_eq!(at(7), first);
        let others: std::collections::HashSet<String> = (0..20).map(at).collect();
        assert!(others.len() > 10, "{others:?}");
        let id = uuid::Uuid::new_v4();
        assert_eq!(seed_of(id), seed_of(id));
        assert!(matches!(
            persona("plain", Persona { names: N, seed: 1 }),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn below_stays_in_range() {
        for n in [1u64, 2, 7] {
            for _ in 0..50 {
                assert!(below(n) < n);
            }
        }
        assert_eq!(below(0), 0);
    }
}
