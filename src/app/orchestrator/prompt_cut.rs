//! A prompt the server cut in silence, told from the server's own `usage`
//! (docs/research/prompt-cut-detection.md, spec §6.7).
//!
//! Ollama answers a prompt over its window with a `200`: it drops the oldest
//! messages, and when the system message and the last one alone do not fit it
//! cuts the rendered prompt from the front to half the window — the
//! instructions first. Its `usage` then reports what it processed, so the
//! exact figure the compaction trigger reads is the cut one. A cut is told here
//! where a **lower bound** of what the request held exceeds that figure: the
//! previous request's exact size when this one extends it (§3.1), or the
//! request's own text counted so that it never over-counts (§3.2).

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::shared::api::{ApiMessage, ChatRequest};
use crate::shared::tokens::floor_text;

/// What a template may add or drop around a message: a processed figure this
/// close under the bound is not a cut (§3.3).
const TOLERANCE: u64 = 32;

/// An upper bound on a template's markup around one message or one tool call —
/// what a removed one took beyond its own bytes (§3.1).
const MARKUP: u64 = 16;

/// What one request held, kept so the next one can be bounded by it (§3.1):
/// the parts a server renders as given — the system text and the messages —
/// and, for the tool schemas it renders in its own format, only whether they
/// changed.
#[derive(Debug, Clone)]
pub(super) struct RequestShape {
    /// Another model is another tokenizer: no bound carries across.
    model: Option<String>,
    tools: u64,
    system: String,
    messages: Vec<MessageShape>,
}

#[derive(Debug, Clone, Copy)]
struct MessageShape {
    hash: u64,
    /// Everything the message carries as text — content, call names and
    /// arguments — plus the markup bound per part: an upper bound on its tokens,
    /// since a byte-level tokenizer spends at most one token a byte.
    most: u64,
    /// [`floor_text`] of its content: a lower bound on its tokens.
    least: u64,
    /// An image's tokens have no byte bound, so a removed message carrying one
    /// leaves the next request unbounded.
    images: bool,
}

/// A request's shape beside the size the server reported for it.
#[derive(Debug, Clone)]
pub(super) struct PromptAnchor {
    shape: RequestShape,
    processed: u32,
}

/// A request the server processed less of than it certainly held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PromptCut {
    /// What the server's `usage` said it processed.
    pub(super) processed: u32,
    /// The lower bound of what the request held.
    pub(super) held: u64,
}

impl PromptCut {
    /// The rule (§3.3): a cut when the processed figure is more than
    /// [`TOLERANCE`] under the bound.
    pub(super) fn judge(processed: u32, held: u64) -> Option<Self> {
        (processed as u64 + TOLERANCE < held).then_some(Self { processed, held })
    }
}

impl RequestShape {
    pub(super) fn of(request: &ChatRequest, model: Option<&str>) -> Self {
        let tools = if request.tools.is_empty() {
            0
        } else {
            hash_of(&crate::shared::api::openai::tools_json(&request.tools))
        };
        Self {
            model: model.map(str::to_string),
            tools,
            system: request.system.clone().unwrap_or_default(),
            messages: request.messages.iter().map(MessageShape::of).collect(),
        }
    }

    /// The lower bound of what this request holds: its own text's floor, or what
    /// the previous request's exact size says, whichever is higher.
    pub(super) fn held(&self, anchor: Option<&PromptAnchor>) -> u64 {
        let own = floor_text(&self.system) + self.messages.iter().map(|m| m.least).sum::<u64>();
        own.max(anchor.and_then(|a| self.extending(a)).unwrap_or(0))
    }

    /// This request's shape with the size the server reported for it — kept
    /// whether or not it was cut, since a cut figure is still under what the
    /// request held and so still bounds the next one.
    pub(super) fn served(self, processed: u32) -> PromptAnchor {
        PromptAnchor {
            shape: self,
            processed,
        }
    }

    /// The bound the previous request's exact size gives this one (§3.1): that
    /// size, less an upper bound of what this request no longer carries, plus a
    /// lower bound of what it carries instead. `None` when nothing bounds what
    /// was removed — another model, other tools, an image.
    fn extending(&self, anchor: &PromptAnchor) -> Option<u64> {
        let before = &anchor.shape;
        if self.model != before.model || self.tools != before.tools {
            return None;
        }
        let common = self
            .messages
            .iter()
            .zip(&before.messages)
            .take_while(|(now, then)| now.hash == then.hash)
            .count();
        let removed = &before.messages[common..];
        if removed.iter().any(|m| m.images) {
            return None;
        }
        let (system_removed, system_added) = system_change(&before.system, &self.system);
        let removed = removed.iter().map(|m| m.most).sum::<u64>() + system_removed;
        let added = self.messages[common..].iter().map(|m| m.least).sum::<u64>() + system_added;
        Some((anchor.processed as u64 + added).saturating_sub(removed))
    }
}

impl MessageShape {
    fn of(message: &ApiMessage) -> Self {
        let mut h = DefaultHasher::new();
        (message.role as u8).hash(&mut h);
        message.content.hash(&mut h);
        message.tool_call_id.hash(&mut h);
        let mut most = message.content.len() as u64 + MARKUP;
        for call in &message.tool_calls {
            call.id.hash(&mut h);
            call.name.hash(&mut h);
            call.arguments.hash(&mut h);
            most += (call.name.len() + call.arguments.len()) as u64 + MARKUP;
        }
        for image in &message.images {
            image.mime.hash(&mut h);
            image.data.hash(&mut h);
            image.label.hash(&mut h);
        }
        Self {
            hash: h.finish(),
            most,
            least: floor_text(&message.content),
            images: !message.images.is_empty(),
        }
    }
}

/// What changed between two system texts: the bytes of the old one between
/// their common head and tail (an upper bound on the tokens it lost), and the
/// floor of the new one's text in their place. The system text carries the
/// time and the observations chosen for the turn (`inject_self_model`), so it
/// changes from turn to turn in the middle and stays the same around it.
fn system_change(before: &str, now: &str) -> (u64, u64) {
    let (a, b) = (before.as_bytes(), now.as_bytes());
    let mut head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !now.is_char_boundary(head) {
        head -= 1;
    }
    let room = a.len().min(b.len()) - head;
    let mut tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(room)
        .take_while(|(x, y)| x == y)
        .count();
    while !now.is_char_boundary(b.len() - tail) {
        tail -= 1;
    }
    let removed = (a.len() - head - tail) as u64;
    let added = floor_text(&now[head..b.len() - tail]);
    (removed, added)
}

fn hash_of(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::{ApiImage, ApiToolCall, ToolSchema};

    fn request(system: &str, messages: Vec<ApiMessage>) -> ChatRequest {
        ChatRequest {
            system: Some(system.to_string()),
            messages,
            ..Default::default()
        }
    }

    fn prose(words: usize) -> String {
        (0..words)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn anchor(req: &ChatRequest, processed: u32) -> PromptAnchor {
        RequestShape::of(req, Some("m")).served(processed)
    }

    #[test]
    fn a_figure_within_the_tolerance_is_not_a_cut() {
        assert_eq!(PromptCut::judge(1000, 1032), None);
        assert_eq!(
            PromptCut::judge(1000, 1033),
            Some(PromptCut {
                processed: 1000,
                held: 1033
            })
        );
        assert_eq!(PromptCut::judge(4000, 100), None);
    }

    #[test]
    fn a_request_that_extends_the_last_is_bounded_by_its_exact_size() {
        let first = request("sys", vec![ApiMessage::user("hello")]);
        let mut next = first.clone();
        next.messages.push(ApiMessage::assistant(prose(200)));
        next.messages.push(ApiMessage::user(prose(100)));
        let shape = RequestShape::of(&next, Some("m"));
        let added = floor_text(&prose(200)) + floor_text(&prose(100));
        assert_eq!(
            shape.held(Some(&anchor(&first, 3500))),
            3500 + added,
            "the exact size plus the floor of what was appended"
        );
        // Ollama's prompt cut lands at half its window: told.
        assert!(PromptCut::judge(2051, shape.held(Some(&anchor(&first, 3500)))).is_some());
    }

    #[test]
    fn without_an_anchor_the_request_is_its_own_floor() {
        let req = request(&prose(50), vec![ApiMessage::user(prose(300))]);
        let shape = RequestShape::of(&req, Some("m"));
        assert_eq!(
            shape.held(None),
            floor_text(&prose(50)) + floor_text(&prose(300))
        );
    }

    #[test]
    fn the_tool_schemas_count_nothing_toward_the_floor() {
        let mut req = request("sys", vec![ApiMessage::user("hi")]);
        let bare = RequestShape::of(&req, Some("m")).held(None);
        req.tools = vec![ToolSchema {
            name: "t".into(),
            description: prose(500),
            parameters: serde_json::json!({"type": "object"}),
        }];
        assert_eq!(RequestShape::of(&req, Some("m")).held(None), bare);
    }

    #[test]
    fn what_a_regeneration_removed_is_taken_off_at_its_bytes() {
        // The previous turn's last request ended in a tool round; a regeneration
        // asks again from the user message, without it.
        let base = request("sys", vec![ApiMessage::user("question")]);
        let mut with_round = base.clone();
        let call = ApiToolCall {
            id: "c1".into(),
            name: "note_save".into(),
            arguments: r#"{"content":"x"}"#.into(),
            thought_signature: None,
        };
        with_round
            .messages
            .push(ApiMessage::assistant_tool_calls("", vec![call]));
        with_round.messages.push(ApiMessage::tool("c1", prose(100)));
        let removed =
            (MARKUP + "note_save".len() as u64 + 15 + MARKUP) + (prose(100).len() as u64 + MARKUP);
        let shape = RequestShape::of(&base, Some("m"));
        assert_eq!(shape.held(Some(&anchor(&with_round, 2000))), 2000 - removed);
    }

    #[test]
    fn an_edit_near_the_start_leaves_nothing_to_bound() {
        let before = request(
            "sys",
            vec![
                ApiMessage::user(prose(400)),
                ApiMessage::assistant(prose(400)),
                ApiMessage::user("next"),
            ],
        );
        let after = request("sys", vec![ApiMessage::user("edited")]);
        let shape = RequestShape::of(&after, Some("m"));
        // The previous size, less everything after the first message: below
        // the request's own floor, which is then the bound.
        assert_eq!(shape.held(Some(&anchor(&before, 1200))), shape.held(None));
    }

    #[test]
    fn the_system_text_changing_in_the_middle_costs_only_the_middle() {
        let head = prose(100);
        let tail = prose(80);
        let before = request(
            &format!("{head} at 10:00, notes A B {tail}"),
            vec![ApiMessage::user("q")],
        );
        let now_system = format!("{head} at 10:05, notes C {tail}");
        let after = request(&now_system, vec![ApiMessage::user("q")]);
        let (removed, added) = system_change(before.system.as_deref().unwrap(), &now_system);
        // The common head runs to "at 10:0", the common tail from " " on.
        assert_eq!(removed, "0, notes A B".len() as u64);
        assert_eq!(added, floor_text("5, notes C"));
        assert_eq!(
            RequestShape::of(&after, Some("m")).held(Some(&anchor(&before, 900))),
            900 - removed + added
        );
    }

    #[test]
    fn a_system_change_splits_on_a_character_boundary() {
        // Two Cyrillic letters that share their first UTF-8 byte: the head
        // must not end inside the character.
        let (removed, added) = system_change("а т б", "а ц б");
        assert_eq!(removed, "т".len() as u64);
        assert_eq!(added, floor_text("ц"));
        assert_eq!(system_change("same", "same"), (0, 0));
        assert_eq!(system_change("", "new text"), (0, floor_text("new text")));
        assert_eq!(system_change("old text", ""), ("old text".len() as u64, 0));
    }

    #[test]
    fn another_model_or_other_tools_give_no_bound() {
        let first = request("sys", vec![ApiMessage::user(prose(300))]);
        let mut next = first.clone();
        next.messages.push(ApiMessage::user("more"));
        let own = RequestShape::of(&next, Some("m")).held(None);
        let other_model = RequestShape::of(&first, Some("other")).served(5000);
        assert_eq!(
            RequestShape::of(&next, Some("m")).held(Some(&other_model)),
            own
        );
        // The final round of a turn drops the tools: no bound across it.
        let mut with_tools = first.clone();
        with_tools.tools = vec![ToolSchema {
            name: "t".into(),
            description: "d".into(),
            parameters: serde_json::json!({}),
        }];
        assert_eq!(
            RequestShape::of(&next, Some("m")).held(Some(&anchor(&with_tools, 5000))),
            own
        );
    }

    #[test]
    fn a_removed_image_leaves_the_request_unbounded() {
        let image = ApiImage::new("image/png", "AAAA", None);
        let before = request(
            "sys",
            vec![ApiMessage::user("look").with_images(vec![image])],
        );
        let after = request("sys", vec![ApiMessage::user("look")]);
        let shape = RequestShape::of(&after, Some("m"));
        assert_eq!(shape.held(Some(&anchor(&before, 3000))), shape.held(None));
        // Kept, the image counts nothing toward the bound and removes nothing.
        let mut kept = before.clone();
        kept.messages.push(ApiMessage::user("and this"));
        assert_eq!(
            RequestShape::of(&kept, Some("m")).held(Some(&anchor(&before, 3000))),
            3000 + floor_text("and this")
        );
    }
}
