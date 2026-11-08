//! A name for a pasted image (spec §9.10, docs/research/pasted-image-names.md): the
//! request that asks the chat's model what a picture shows, the reduction of its answer
//! to a file-name slug, the `clipboard/<slug>.png` shape, the name made unique in its
//! chat, and the board a turn waits on while a name is on its way.
//!
//! Pure but for the board's waiting: no I/O and no localization. The request is in
//! English whatever the profile — the owner's decision F3, measured clean on every model
//! asked (research §3.2) — so its words are constants here rather than bundle keys.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uuid::Uuid;

use crate::entities::message_image::MessageImage;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::{ApiImage, ApiMessage, ChatRequest};

pub use crate::entities::message_image::PASTED_PREFIX;

/// The name a pasted image has when nothing better is known — the name it had before
/// names existed, and the one a failed or late name leaves (F9).
pub const FALLBACK_NAME: &str = "clipboard.png";

/// The extension every pasted image carries: the paste always encodes png, so the name
/// says the format rather than the model's opinion of it.
const PASTED_EXTENSION: &str = ".png";

/// The most words a slug keeps — enough for `sunset-lake-snowy-mountains`.
pub const MAX_WORDS: usize = 5;

/// The longest a slug may be, in characters.
pub const MAX_CHARS: usize = 48;

/// How long a turn waits for the names of the images it carries (F4). Measured, a name
/// takes ≤ 1.6 s locally and ≤ 3.4 s on the cheap clouds; past this the image goes
/// under the name it has and the late one is dropped.
pub const SEND_WAIT: Duration = Duration::from_secs(10);

/// How long the naming request itself may take, start to end. Longer than [`SEND_WAIT`]:
/// an image nobody has sent yet can still take its name late.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The reply token ceiling. A name is ten tokens, but a GGUF that thinks in spite of
/// the muted sampling needs the room the automatic title gives it.
const MAX_TOKENS: usize = 2048;

/// The system message — the one measured in research §3.2, 240 of 240 well formed.
pub const SYSTEM: &str = "You name image files. Look at the image and reply with a short file \
name for it: two to four lowercase English words that say what the image shows, joined by \
hyphens. No file extension, no quotes, nothing else. Example: summer-mountain-lake";

/// What the user part of the request says beside the image.
pub const ASK: &str = "Name this image.";

/// The request that asks for `image`'s name: the system message, the image as the one
/// content part, and the automatic title's muted sampling — thoughts off by every switch
/// a provider reads (spec §8.1), a low temperature, no tools.
pub fn request(image: &MessageImage) -> ChatRequest {
    ChatRequest {
        continue_final: false,
        output_schema: None,
        system: Some(SYSTEM.to_string()),
        messages: vec![ApiMessage::user(ASK).with_images(vec![ApiImage::new(
            image.mime.clone(),
            &image.data,
            None,
        )])],
        sampling: SamplingConfig {
            max_tokens: Some(MAX_TOKENS),
            temperature: Some(0.3),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..Default::default()
        },
        tools: Vec::new(),
    }
}

/// Which letters a slug keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Letters {
    /// `a–z` and `0–9` only — a name the **model** gave (F3). Whatever text the image
    /// carried, nothing but these survives into a label, a chip or `/w/in`.
    Ascii,
    /// Any letter or digit — a name the **person** typed (`/image paste <name>`, F7),
    /// in their own language.
    Any,
}

/// Reduces `raw` to a file-name slug: the first line that says something, lower case,
/// words of letters and digits joined by single hyphens, at most [`MAX_WORDS`] words and
/// [`MAX_CHARS`] characters. A trailing image extension is dropped — the name's own is
/// added after. `None` when nothing is left.
pub fn slug(raw: &str, letters: Letters) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    // Quotes and backticks around the answer would hide its extension.
    let line = strip_image_extension(line.trim_matches(|c: char| !c.is_alphanumeric()));
    let keep = |c: char| match letters {
        Letters::Ascii => c.is_ascii_alphanumeric(),
        Letters::Any => c.is_alphanumeric(),
    };
    let mut out = String::new();
    for word in line
        .split(|c: char| !keep(c))
        .filter(|w| !w.is_empty())
        .take(MAX_WORDS)
    {
        let word = word.to_lowercase();
        let sep = usize::from(!out.is_empty());
        if out.chars().count() + sep + word.chars().count() > MAX_CHARS {
            if out.is_empty() {
                // One word longer than the whole budget: its start is still a name.
                out = word.chars().take(MAX_CHARS).collect();
            }
            break;
        }
        if sep == 1 {
            out.push('-');
        }
        out.push_str(&word);
    }
    (!out.is_empty()).then_some(out)
}

/// `name.png` → `name`, for the formats an image decodes from; any other dot stays.
fn strip_image_extension(line: &str) -> &str {
    match line.rsplit_once('.') {
        Some((stem, ext)) if crate::features::image_prepare::is_image_extension(ext) => stem,
        _ => line,
    }
}

/// The model's answer, or — when it never stopped thinking and wrote nothing — the last
/// line its thoughts arrived at, the automatic title's salvage.
pub fn answer(text: &str, thoughts: &str) -> String {
    if !text.trim().is_empty() {
        return text.to_string();
    }
    thoughts
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// A pasted image's name for `slug`: `clipboard/<slug>.png` (F5).
pub fn pasted_name(slug: &str) -> String {
    format!("{PASTED_PREFIX}{slug}{PASTED_EXTENSION}")
}

/// `name` itself when nothing in the chat is called that, else the first of `-2`, `-3`, …
/// put before the extension that is free (F6) — `clipboard-2.png`,
/// `clipboard/sunset-lake-2.png`. Unique across the whole chat, so a pasted image is
/// reachable by name in `/file list` and in `python_exec`'s `files`, where a name two
/// items share resolves to nothing.
pub fn unique_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > name.rfind('/').map_or(0, |s| s + 1) => name.split_at(i),
        _ => (name, ""),
    };
    // Bounded by how many items one chat can hold; the guard only keeps a pathological
    // `taken` from spinning forever.
    (2..10_000)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_string())
}

/// Where a pasted image's name stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NameState {
    /// The naming request is out.
    Pending,
    /// A name landed in time, and is the image's.
    Named(String),
    /// No name will be used: the request failed, or a turn stopped waiting for it.
    Settled,
}

/// The one place that decides whether a pasted image takes the name its request
/// brings — shared by the orchestrator, which lands names, and a turn parked until the
/// names of its images are in (F4). A name lands only on an image still `Pending`: one
/// a turn has given up on stays as it was sent, since renaming a label the model has
/// read would rewrite history under the prompt cache.
#[derive(Debug, Default)]
pub struct NameBoard {
    states: Mutex<HashMap<Uuid, NameState>>,
}

impl NameBoard {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn with<R>(&self, f: impl FnOnce(&mut HashMap<Uuid, NameState>) -> R) -> R {
        // A poisoned lock means a panic while holding a plain map: the map is still
        // whole, and a name is not worth propagating a panic for.
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut states)
    }

    /// A naming request for `id` is out.
    pub fn expect(&self, id: Uuid) {
        self.with(|s| s.insert(id, NameState::Pending));
    }

    /// Lands `name` on `id`: `true` when the image was still waiting for it — the caller
    /// then renames the image — `false` when a turn already gave up on it.
    pub fn land(&self, id: Uuid, name: &str) -> bool {
        self.with(|s| match s.get_mut(&id) {
            Some(state @ NameState::Pending) => {
                *state = NameState::Named(name.to_string());
                true
            }
            _ => false,
        })
    }

    /// No name is coming for `id` — the request failed or the image has gone.
    pub fn settle(&self, id: Uuid) {
        self.with(|s| {
            if let Some(state @ NameState::Pending) = s.get_mut(&id) {
                *state = NameState::Settled;
            }
        });
    }

    /// The images among `ids` whose name is still on its way.
    pub fn pending(&self, ids: &[Uuid]) -> Vec<Uuid> {
        self.with(|s| {
            ids.iter()
                .copied()
                .filter(|id| s.get(id) == Some(&NameState::Pending))
                .collect()
        })
    }

    /// Stops waiting for every name among `ids` still on its way: from now on a name that
    /// arrives for one of them is dropped.
    pub fn give_up(&self, ids: &[Uuid]) {
        for id in ids {
            self.settle(*id);
        }
    }

    /// Forgets `id` once its request has answered — the entry has done its job.
    pub fn forget(&self, id: Uuid) {
        self.with(|s| s.remove(&id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_answer_becomes_a_slug() {
        let s = |raw: &str| slug(raw, Letters::Ascii);
        // What the probe's models actually answered, and what they might.
        assert_eq!(
            s("sunset-mountain-lake").as_deref(),
            Some("sunset-mountain-lake")
        );
        assert_eq!(s("Sunset Over Lake!").as_deref(), Some("sunset-over-lake"));
        assert_eq!(
            s("  \n`blue_abstract_waves.png`\n").as_deref(),
            Some("blue-abstract-waves")
        );
        assert_eq!(
            s("\"cuda-out-of-memory-error\"").as_deref(),
            Some("cuda-out-of-memory-error")
        );
        assert_eq!(
            s("weekend shopping list\nThis image shows…").as_deref(),
            Some("weekend-shopping-list")
        );
        // Five words at most.
        assert_eq!(
            s("one two three four five six seven").as_deref(),
            Some("one-two-three-four-five")
        );
        // An extension is the name's, not the model's; any other dot is a separator.
        assert_eq!(s("chart.JPEG").as_deref(), Some("chart"));
        assert_eq!(
            s("v1.2 release notes").as_deref(),
            Some("v1-2-release-notes")
        );
    }

    /// The slug is the only part of an answer that survives, so nothing an image's text
    /// talks the model into becomes a path, a quote or a second line.
    #[test]
    fn a_hostile_or_empty_answer_leaves_nothing_but_a_slug() {
        let s = |raw: &str| slug(raw, Letters::Ascii);
        assert_eq!(s("../../etc/passwd").as_deref(), Some("etc-passwd"));
        assert_eq!(
            s("C:\\Windows\\system32").as_deref(),
            Some("c-windows-system32")
        );
        assert_eq!(s("a\" ; rm -rf / \"").as_deref(), Some("a-rm-rf"));
        assert_eq!(s(""), None);
        assert_eq!(s("   \n  "), None);
        assert_eq!(s("!!! ??? ..."), None);
        // A model's Russian answer keeps no letters under F3: the name falls back.
        assert_eq!(s("\u{0437}\u{0430}\u{043a}\u{0430}\u{0442}"), None);
        // A run-on answer is cut to the budget at a word boundary…
        let long = "word ".repeat(40);
        let cut = s(&long).unwrap();
        assert!(cut.chars().count() <= MAX_CHARS);
        // …and one enormous word keeps its start.
        let huge = "x".repeat(400);
        assert_eq!(s(&huge).unwrap().chars().count(), MAX_CHARS);
        for raw in ["a b c d e f g", "\u{1F600} sun", "tab\tseparated\twords"] {
            let out = s(raw).unwrap();
            assert!(
                out.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{raw:?} → {out:?}"
            );
            assert!(!out.starts_with('-') && !out.ends_with('-') && !out.contains("--"));
        }
    }

    /// A name the person typed keeps their own letters (F7).
    #[test]
    fn a_typed_name_keeps_any_letters() {
        let typed = "\u{0417}\u{0430}\u{043a}\u{0430}\u{0442} \u{043d}\u{0430}\u{0434} \u{043e}\u{0437}\u{0435}\u{0440}\u{043e}\u{043c}";
        let out = slug(typed, Letters::Any).unwrap();
        assert_eq!(out.split('-').count(), 3);
        assert!(out.chars().all(|c| c == '-' || !c.is_uppercase()));
        assert_eq!(
            slug("My Chart.png", Letters::Any).as_deref(),
            Some("my-chart")
        );
        assert_eq!(slug("???", Letters::Any), None);
    }

    #[test]
    fn the_answer_falls_back_to_the_thoughts_last_line() {
        assert_eq!(answer("sunset-lake", "ignored"), "sunset-lake");
        assert_eq!(
            answer("  ", "hmm\nthe image shows a lake\nlake-at-dawn\n"),
            "lake-at-dawn"
        );
        assert_eq!(answer("", ""), "");
    }

    #[test]
    fn a_pasted_name_is_the_slug_under_the_clipboard_prefix() {
        assert_eq!(pasted_name("sunset-lake"), "clipboard/sunset-lake.png");
    }

    /// The first free number goes before the extension, the prefix's slash is not one,
    /// and a name with no extension still numbers.
    #[test]
    fn a_taken_name_gets_the_first_free_number() {
        let taken = [
            "clipboard.png",
            "clipboard-2.png",
            "clipboard/sunset-lake.png",
        ];
        let is_taken = |n: &str| taken.contains(&n);
        assert_eq!(unique_name("clipboard.png", is_taken), "clipboard-3.png");
        assert_eq!(
            unique_name("clipboard/sunset-lake.png", is_taken),
            "clipboard/sunset-lake-2.png"
        );
        assert_eq!(
            unique_name("clipboard/new.png", is_taken),
            "clipboard/new.png"
        );
        assert_eq!(unique_name("notes", |n| n == "notes"), "notes-2");
        assert_eq!(unique_name("a.b/name", |n| n == "a.b/name"), "a.b/name-2");
    }

    #[test]
    fn the_request_is_the_measured_one_with_thoughts_muted() {
        let image = MessageImage::new(
            "clipboard.png",
            "clipboard:x",
            "image/png",
            4,
            4,
            "AAAA".into(),
        );
        let req = request(&image);
        assert_eq!(req.system.as_deref(), Some(SYSTEM));
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].content, ASK);
        assert_eq!(req.messages[0].images.len(), 1);
        assert!(
            req.messages[0].images[0].label.is_none(),
            "the image needs no label"
        );
        assert!(req.tools.is_empty());
        let s = &req.sampling;
        assert_eq!(s.thinking, Some(false));
        assert_eq!(s.reasoning_effort, Some(ReasoningEffort::None));
        assert_eq!(s.reasoning_budget, Some(0));
    }

    /// A name lands only on an image still waiting for one; a turn that gave up keeps
    /// the name it sent, and a failed request settles the wait without a name.
    #[test]
    fn the_board_lands_a_name_only_while_it_is_awaited() {
        let board = NameBoard::new();
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for id in [a, b, c] {
            board.expect(id);
        }
        assert_eq!(board.pending(&[a, b, c]), vec![a, b, c]);

        assert!(board.land(a, "clipboard/lake.png"));
        assert!(!board.land(a, "clipboard/again.png"), "one name per image");
        board.give_up(&[b]);
        assert!(
            !board.land(b, "clipboard/late.png"),
            "a late name is dropped"
        );
        board.settle(c);
        assert!(board.pending(&[a, b, c]).is_empty());

        board.forget(a);
        assert!(
            !board.land(a, "clipboard/x.png"),
            "a forgotten image takes nothing"
        );
        assert!(
            board.pending(&[Uuid::new_v4()]).is_empty(),
            "an unknown image is not awaited"
        );
    }
}
