//! Pure logic for conversation-history compression (a rolling summary): the
//! digest handed to the summarizer, the cut point, and the two prompt builders.
//! No I/O and no engine — the background task in `app/orchestrator` supplies
//! those. See docs/research/history-compression.md §5.4 and §6.3.
//!
//! The digest deliberately differs from [`super::rename_chat::build_conversation_digest`]
//! (the auto-title one, which this mirrors in style): it **carries tool
//! activity**. Tool results are the invisible bulk of a long conversation —
//! §1.3 of the research doc — so a digest that drops them would compress the
//! cheap half and leave the expensive half uncompressed.

use std::collections::HashMap;
use std::ops::Range;

use uuid::Uuid;

use crate::entities::attachment::paginate;
use crate::entities::message::{Message, MessageRole, ToolCallRecord};
use crate::shared::i18n::Locale;
use crate::shared::tokens::estimate_prompt;

/// How much of a tool result the digest keeps, in **characters** (not bytes —
/// Cyrillic). Enough to carry an identifier or a verdict; the rest is what the
/// summary is supposed to be dropping anyway.
pub const TOOL_RESULT_CLIP: usize = 200;

/// One rendered message of a range, with the id it came from.
struct Rendered {
    /// Blocks in conversation order: the id of the message and its text.
    blocks: Vec<(Uuid, String)>,
    /// `Tool`-role message id → index of the block that carries its result.
    ///
    /// A tool result is rendered *inside* the assistant block whose call
    /// produced it, so the `Tool` message has no block of its own — but it does
    /// have its own row in the full-text index, and it is the bulk a search is
    /// most likely to hit (§1.3). Without this a hit on a tool result would map
    /// to no page at all.
    aliases: HashMap<Uuid, usize>,
}

/// Renders `messages` one block per user/assistant message: a role line (in the
/// scaffold language `loc`, axis A) plus one compact line per tool call the
/// message made.
///
/// `clip` is the tool-result budget in characters, `None` for no clipping. That
/// parameter is the whole reason there is one renderer rather than two: the
/// digest handed to the summarizer clips a result to [`TOOL_RESULT_CLIP`], while
/// the transcript the read-back tools page through must not clip at all — paging
/// back to a `fetch_url` result only to receive the same 200 characters the
/// summary already carried would defeat the point. Sharing the renderer is what
/// makes "what was summarized is what can be re-read" true by construction.
///
/// Rules:
/// - `Tool`-role messages get no block of their own: they are already
///   represented by the call line of the message that produced them, and
///   emitting both would double-count the bulk this exists to compress.
/// - "Thoughts" (CoT) are excluded — the model's scratch space, not conversation.
/// - A blank message is skipped **unless** it carries tool calls (a round that
///   only called tools still happened, and its results matter).
fn render(messages: &[Message], loc: &Locale, clip: Option<usize>) -> Rendered {
    // Fallback source for a tool result the record itself doesn't carry (see
    // `tool_result_of`). Built once; empty for a conversation without tools.
    let by_call_id: HashMap<&str, &str> = messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .filter_map(|m| Some((m.tool_call_id.as_deref()?, m.text.as_str())))
        .collect();

    let mut blocks: Vec<(Uuid, String)> = Vec::new();
    let mut aliases: HashMap<Uuid, usize> = HashMap::new();
    for m in messages {
        // A tool result belongs to the block above it; System never occurs in
        // `chat.messages` at all (research §5.5).
        if m.role == MessageRole::Tool {
            if let Some(i) = blocks.len().checked_sub(1) {
                aliases.insert(m.id, i);
            }
            continue;
        }
        if !matches!(m.role, MessageRole::User | MessageRole::Assistant) {
            continue;
        }
        let text = m.text.trim();
        if text.is_empty() && m.tool_calls.is_empty() {
            continue;
        }
        let who = match m.role {
            MessageRole::User => loc.t("digest.role.user"),
            _ => loc.t("digest.role.assistant"),
        };
        // A tool-only round has no text: keep the role line bare rather than
        // trailing a space into the prompt — the call lines hang off it.
        let mut lines = vec![if text.is_empty() {
            format!("{who}:")
        } else {
            format!("{who}: {text}")
        }];
        for call in &m.tool_calls {
            let result = tool_result_of(call, &by_call_id, clip);
            lines.push(loc.tf(
                "compaction.digest.tool",
                // The result goes last, as in `features/tools/fetch.rs`: it is
                // the largest, most attacker-shaped value. `tf` is single-pass,
                // so a `{placeholder}` inside it is not re-expanded either way.
                &[("name", &call.name), ("result", &result)],
            ));
        }
        blocks.push((m.id, lines.join("\n")));
    }
    Rendered { blocks, aliases }
}

/// Builds the summarizer's input digest for `messages` — [`render`] with the
/// tool-result clip, blocks joined one per line.
///
/// The digest is **not** truncated as a whole: the caller sizes the window it
/// passes in (`plan_cut` + the roll loop), so truncating here would silently
/// drop content the caller believes it handed over.
///
/// Returns `None` when nothing substantive was produced.
pub fn build_compaction_digest(messages: &[Message], loc: &Locale) -> Option<String> {
    let rendered = render(messages, loc, Some(TOOL_RESULT_CLIP));
    if rendered.blocks.is_empty() {
        return None;
    }
    Some(
        rendered
            .blocks
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The compacted-away part of a conversation, rendered once per turn so the
/// read-back tools can page through it and locate a match in it (spec §6.7,
/// stage 3 / fork F9b).
///
/// Pages are token-budgeted slices of one rendered transcript, cut on line
/// boundaries by [`paginate`] — the same helper `attachment_read` pages a file
/// with, and for the same reason: the model can walk `1..M` and *know* it has
/// read everything, which is the guarantee retrieval cannot give. Sharing it
/// also means the two readers cut text the same way.
pub struct HistoryView {
    /// The rendered transcript of the compacted-away range.
    text: String,
    /// Byte range of each block within [`Self::text`], in conversation order.
    blocks: Vec<(Uuid, Range<usize>)>,
    /// `Tool`-role message id → index into [`Self::blocks`] (see [`Rendered`]).
    aliases: HashMap<Uuid, usize>,
}

/// Separator between blocks of the readable transcript. Deliberately wider than
/// the digest's single newline: a multi-line user message would otherwise run
/// into the next speaker's line, and a blank line is also where [`paginate`]
/// prefers to cut.
const BLOCK_SEPARATOR: &str = "\n\n";

impl HistoryView {
    /// Renders `messages` (the compacted-away prefix) for reading back.
    /// `None` when there is nothing substantive in it.
    pub fn render(messages: &[Message], loc: &Locale) -> Option<Self> {
        let rendered = render(messages, loc, None);
        if rendered.blocks.is_empty() {
            return None;
        }
        let mut text = String::new();
        let mut blocks = Vec::with_capacity(rendered.blocks.len());
        for (id, block) in rendered.blocks {
            if !text.is_empty() {
                text.push_str(BLOCK_SEPARATOR);
            }
            let start = text.len();
            text.push_str(&block);
            blocks.push((id, start..text.len()));
        }
        Some(Self {
            text,
            blocks,
            aliases: rendered.aliases,
        })
    }

    /// How many pages the transcript takes at this page size.
    pub fn page_count(&self, page_tokens: usize) -> usize {
        paginate(&self.text, page_tokens).len()
    }

    /// Page `n` (1-based), or `None` when out of range.
    pub fn page(&self, page_tokens: usize, n: usize) -> Option<&str> {
        let pages = paginate(&self.text, page_tokens);
        n.checked_sub(1).and_then(|i| pages.get(i)).copied()
    }

    /// Which page (1-based) a message landed on, and its rendered block —
    /// the pair a search hit needs so the model can widen it with a page read.
    ///
    /// A `Tool`-role message resolves to the block of the call that produced it.
    /// `None` when the id is not in this range at all.
    pub fn locate(&self, page_tokens: usize, id: Uuid) -> Option<(usize, &str)> {
        let i = self
            .blocks
            .iter()
            .position(|(bid, _)| *bid == id)
            .or_else(|| self.aliases.get(&id).copied())?;
        let range = self.blocks[i].1.clone();
        // Pages tile the text in order, so the page holding an offset is the
        // first whose cumulative end passes it.
        let mut end = 0;
        for (p, page) in paginate(&self.text, page_tokens).iter().enumerate() {
            end += page.len();
            if range.start < end {
                return Some((p + 1, &self.text[range]));
            }
        }
        None
    }

    /// Conversation order of a message within this range — what search hits are
    /// sorted by, since the index returns them in an arbitrary order.
    pub fn order_of(&self, id: Uuid) -> Option<usize> {
        self.blocks
            .iter()
            .position(|(bid, _)| *bid == id)
            .or_else(|| self.aliases.get(&id).copied())
    }
}

/// One tool call's result text, clipped to `clip` characters when asked.
///
/// The live agentic loop always stores the result on the record itself
/// (`orchestrator/generation.rs::execute_call` sets `result: Some(…)` even for a
/// gated or refused call), so that is the normal source. The field is still an
/// `Option` — an imported or hand-edited chat can lack it — so we fall back to
/// the `Tool`-role message carrying the same `tool_call_id`.
fn tool_result_of(
    call: &ToolCallRecord,
    by_call_id: &HashMap<&str, &str>,
    clip: Option<usize>,
) -> String {
    let raw = call
        .result
        .as_deref()
        .or_else(|| by_call_id.get(call.id.as_str()).copied())
        .unwrap_or("")
        .trim();
    match clip {
        Some(budget) => clip_chars(raw, budget),
        None => raw.to_string(),
    }
}

/// Limits a string to `budget` **characters** (Unicode scalars, not bytes),
/// appending `…` when something was dropped.
fn clip_chars(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let head: String = text.chars().take(budget).collect();
    format!("{head}…")
}

/// Chooses where to cut the history: everything before the returned index is
/// what a compaction folds into the summary, and `messages[i..]` stays verbatim.
///
/// Picks the **largest** index whose tail still estimates to at least
/// `tail_tokens`, then snaps **back** to the nearest [`MessageRole::User`]
/// message at or before it.
///
/// The snap-back is load-bearing, not cosmetic: it guarantees a request never
/// splits an assistant turn from its tool results. Cutting inside such a group
/// would break Anthropic's strict user/assistant alternation (a `tool_result`
/// block with no preceding `tool_use`) and Gemini 3's thought-signature replay
/// (a `functionResponse` whose `functionCall` is gone → `400`). A user message
/// is the only index at which the history is always self-contained.
///
/// Returns `None` when there is nothing worth compacting: the whole
/// conversation estimates below `tail_tokens`, or the only user boundary at or
/// before the cut is index 0 (cutting there would fold nothing).
pub fn plan_cut(messages: &[Message], tail_tokens: usize) -> Option<usize> {
    // Walk back from the end accumulating the tail estimate. It grows
    // monotonically as the index falls, so the first index that reaches the
    // budget is the largest one that satisfies it. Sizes come from
    // `estimate_prompt` one message at a time, so the per-message chat-template
    // overhead is counted exactly as the status-bar estimate counts it.
    let mut acc: u64 = 0;
    let mut cut: Option<usize> = None;
    for (i, m) in messages.iter().enumerate().rev() {
        acc += estimate_prompt(None, [m.text.as_str()]);
        if acc >= tail_tokens as u64 {
            cut = Some(i);
            break;
        }
    }
    let cut = cut?;
    let boundary = messages[..=cut]
        .iter()
        .rposition(|m| m.role == MessageRole::User)?;
    (boundary > 0).then_some(boundary)
}

/// Markers of "the prompt did not fit the context window" in a provider's error
/// text, lowercased.
///
/// The client deliberately does not swallow an error body — it puts the first 500
/// characters into the error text — so this reads what the server actually said.
/// Best-effort by construction: a marker that stops matching costs the *hint*,
/// never correctness, and the generic message still carries the raw body.
const OVERFLOW_MARKERS: &[&str] = &[
    // llama.cpp: `{"error":{"type":"exceed_context_size_error",…,"n_ctx":M}}`
    // (measured, §9a M3 — it arrives as HTTP 400 before the SSE stream starts).
    "exceed_context_size_error",
    // OpenAI, both the modern code and the older prose.
    "context_length_exceeded",
    "maximum context length",
    // Anthropic: `prompt is too long: N tokens > M maximum`.
    "prompt is too long",
    // Gemini: "The input token count (N) exceeds the maximum number of tokens…".
    "exceeds the maximum number of tokens",
];

/// Did this generation fail because the conversation no longer fits the model's
/// context window?
///
/// A `true` only changes which message the user is shown — one naming the way out
/// instead of a generic failure wrapped around raw JSON. Spec §6.7.
pub fn is_context_overflow(err: &str) -> bool {
    let err = err.to_lowercase();
    OVERFLOW_MARKERS.iter().any(|m| err.contains(m))
}

/// The summarizer's system message (scaffold language, axis A). `words` is the
/// stated length limit: the probe (research §9a) measured that a bare
/// `max_tokens` cap truncates mid-sentence instead of making the model
/// prioritize, so the limit has to be **in the prompt**.
pub fn summary_system_message(loc: &Locale, words: usize) -> String {
    loc.tf("prompt.compact.system", &[("words", &words.to_string())])
}

/// The summarizer's user message when a summary already exists: roll `previous`
/// forward over `digest` (the part it does not yet cover) into a single
/// replacement summary. The template instructs the model to drop what later
/// parts superseded — without that the summary grows with every roll, which
/// defeats the point (research §6.3).
///
/// A first compaction has no `previous`; the caller sends the digest itself as
/// the user message rather than rolling an empty summary forward.
pub fn roll_user_message(previous: &str, digest: &str, loc: &Locale, words: usize) -> String {
    loc.tf(
        "prompt.compact.roll",
        // `digest` last, as in `features/tools/fetch.rs` — the largest and least
        // trusted value. (`tf` substitutes in a single pass, so ordering cannot
        // change the result; the convention is kept for consistency.)
        &[
            ("summary", previous),
            ("words", &words.to_string()),
            ("digest", digest),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    /// The reference locale (ru) — assertions on Russian roles pin the ru bundle.
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    /// An assistant message with one tool call whose result lives on the record
    /// (the live path — `execute_call` always populates it).
    fn assistant_with_call(text: &str, id: &str, name: &str, result: Option<&str>) -> Message {
        let mut m = Message::assistant(text);
        m.tool_calls = vec![ToolCallRecord {
            id: id.into(),
            name: name.into(),
            arguments: serde_json::json!({}),
            result: result.map(str::to_string),
            thought_signature: None,
            images: 0,
            subagent: None,
        }];
        m
    }

    fn tool_msg(id: &str, name: &str, text: &str) -> Message {
        let mut m = Message::new(MessageRole::Tool, text);
        m.tool_call_id = Some(id.into());
        m.tool_name = Some(name.into());
        m
    }

    #[test]
    fn digest_carries_tool_activity() {
        let msgs = vec![
            Message::user("найди файл"),
            assistant_with_call("сейчас", "c1", "fs_list", Some("main.rs")),
            tool_msg("c1", "fs_list", "main.rs"),
            Message::assistant("нашёл"),
        ];
        let d = build_compaction_digest(&msgs, ru()).unwrap();
        assert!(d.contains("Пользователь: найди файл"), "{d}");
        assert!(d.contains("Ассистент: сейчас"), "{d}");
        assert!(d.contains("fs_list"), "the tool name must be carried: {d}");
        assert!(
            d.contains("main.rs"),
            "the tool result must be carried: {d}"
        );
    }

    /// The `Tool`-role message is already represented by the call line, so it
    /// must not produce a line of its own — that would double-count exactly the
    /// bulk this digest exists to compress (research §1.3).
    #[test]
    fn tool_messages_are_not_double_counted() {
        let msgs = vec![
            Message::user("q"),
            assistant_with_call("a", "c1", "calc", Some("42")),
            tool_msg("c1", "calc", "42"),
        ];
        let d = build_compaction_digest(&msgs, ru()).unwrap();
        assert_eq!(d.matches("42").count(), 1, "the result appears once: {d}");
        assert_eq!(d.lines().count(), 3, "user + assistant + one call: {d}");
    }

    /// A round that only called tools still happened: it is kept even though its
    /// text is blank.
    #[test]
    fn blank_assistant_with_tool_calls_is_kept() {
        let msgs = vec![
            Message::user("посчитай"),
            assistant_with_call("   ", "c1", "calculate", Some("7")),
        ];
        let d = build_compaction_digest(&msgs, ru()).unwrap();
        assert!(d.contains("calculate"), "{d}");
        // The blank text still gets its role line — the call lines hang off it.
        assert!(d.contains("Ассистент:"), "{d}");
    }

    /// A blank message with nothing attached is dropped, as in the title digest.
    #[test]
    fn blank_message_without_calls_is_skipped() {
        let msgs = vec![Message::user("вопрос"), Message::assistant("   ")];
        let d = build_compaction_digest(&msgs, ru()).unwrap();
        assert_eq!(d, "Пользователь: вопрос");
    }

    #[test]
    fn long_tool_result_is_clipped_at_a_character_boundary() {
        // Cyrillic: 2 bytes per character — clipping by bytes would split one.
        let long = "я".repeat(TOOL_RESULT_CLIP + 50);
        let msgs = vec![
            Message::user("q"),
            assistant_with_call("a", "c1", "note_recall", Some(&long)),
        ];
        let d = build_compaction_digest(&msgs, ru()).unwrap();
        let kept = d.matches('я').count();
        assert_eq!(
            kept, TOOL_RESULT_CLIP,
            "clipped by chars, not bytes: {kept}"
        );
        assert!(d.contains('…'), "the clip marker: {d}");
    }

    /// The record's `result` is the live path's source; the `Tool` message is the
    /// fallback for a record that lacks one (an import or a hand-edited chat).
    #[test]
    fn tool_result_falls_back_to_the_tool_message() {
        let msgs = vec![
            Message::user("q"),
            assistant_with_call("a", "c1", "fs_read", None),
            tool_msg("c1", "fs_read", "содержимое файла"),
        ];
        let d = build_compaction_digest(&msgs, ru()).unwrap();
        assert!(d.contains("содержимое файла"), "{d}");
    }

    /// Neither source available → the call is still reported by name (that it
    /// ran is itself information), just with an empty result.
    #[test]
    fn tool_call_without_any_result_is_still_named() {
        let msgs = vec![
            Message::user("q"),
            assistant_with_call("a", "c9", "web_search", None),
        ];
        let d = build_compaction_digest(&msgs, ru()).unwrap();
        assert!(d.contains("web_search"), "{d}");
    }

    #[test]
    fn digest_is_none_without_meaningful_messages() {
        assert!(build_compaction_digest(&[], ru()).is_none());
        let msgs = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("   "),
            tool_msg("c1", "calc", "42"),
        ];
        assert!(build_compaction_digest(&msgs, ru()).is_none());
    }

    /// Per-language (§3.5): roles and the tool line come from the active
    /// language's bundle, with no unsubstituted placeholder left.
    #[test]
    fn digest_localized_for_all_langs() {
        let msgs = vec![
            Message::user("hi"),
            assistant_with_call("yo", "c1", "calc", Some("42")),
        ];
        for &lang in Lang::ALL {
            let l = locale(lang);
            let d = build_compaction_digest(&msgs, l).unwrap();
            assert!(d.contains(l.t("digest.role.user")), "{lang:?}: {d}");
            assert!(d.contains(l.t("digest.role.assistant")), "{lang:?}: {d}");
            assert!(d.contains("calc") && d.contains("42"), "{lang:?}: {d}");
            assert!(
                !d.contains('{') && !d.contains('}'),
                "unsubstituted placeholder in {lang:?}: {d}"
            );
        }
    }

    // ---- plan_cut ---------------------------------------------------------

    /// A conversation with two exchanges, the second one an assistant+tool group:
    /// indices 0=user 1=assistant 2=tool 3=assistant | 4=user 5=assistant
    /// 6=tool 7=assistant.
    fn two_exchanges() -> Vec<Message> {
        vec![
            Message::user("первый вопрос"),
            assistant_with_call("думаю", "c1", "calc", Some("1")),
            tool_msg("c1", "calc", "1"),
            Message::assistant("первый ответ"),
            Message::user("второй вопрос"),
            assistant_with_call("считаю", "c2", "calc", Some("2")),
            tool_msg("c2", "calc", "2"),
            Message::assistant("второй ответ"),
        ]
    }

    #[test]
    fn cut_is_none_when_the_conversation_is_short() {
        let msgs = two_exchanges();
        // A tail budget larger than the whole conversation: no index qualifies.
        assert_eq!(plan_cut(&msgs, 100_000), None);
        assert_eq!(plan_cut(&[], 10), None);
    }

    /// The one that matters: the raw cut lands inside the second exchange, and
    /// the result snaps back **past** the whole assistant+tool group to its user
    /// message — never between an assistant turn and its tool results.
    #[test]
    fn cut_snaps_back_past_an_assistant_tool_group() {
        let msgs = two_exchanges();
        // Size the budget so the raw cut lands at the trailing tool/assistant
        // pair (indices 6..7), i.e. inside the second exchange.
        let raw_tail = estimate_prompt(None, msgs[6..].iter().map(|m| m.text.as_str()));
        let cut = plan_cut(&msgs, raw_tail as usize).unwrap();
        assert_eq!(cut, 4, "must snap back to the user message of the exchange");
        assert_eq!(msgs[cut].role, MessageRole::User);
    }

    #[test]
    fn cut_always_lands_on_a_user_message_and_never_on_zero() {
        let msgs = two_exchanges();
        // Sweep every budget that yields a cut at all: the answer is always a
        // user index above 0, whatever the raw index happened to be.
        let total = estimate_prompt(None, msgs.iter().map(|m| m.text.as_str())) as usize;
        let mut seen_some = false;
        for budget in 1..=total {
            if let Some(i) = plan_cut(&msgs, budget) {
                assert_eq!(msgs[i].role, MessageRole::User, "budget {budget} → {i}");
                assert!(i > 0, "budget {budget} → {i}");
                seen_some = true;
            }
        }
        assert!(seen_some, "some budget must produce a cut");
    }

    /// A single exchange has no user boundary above 0, so there is nothing to
    /// fold — even though a raw cut index exists.
    #[test]
    fn cut_is_none_without_a_user_boundary_above_zero() {
        let msgs = vec![
            Message::user("вопрос"),
            Message::assistant("ответ"),
            Message::assistant("ещё ответ"),
        ];
        for budget in 1..=40 {
            assert_eq!(plan_cut(&msgs, budget), None, "budget {budget}");
        }
    }

    /// Asking for a bigger verbatim tail can only move the cut earlier (fold
    /// more), never later — the monotonicity the accumulate-from-the-end scan
    /// relies on to stop at the first qualifying index.
    #[test]
    fn a_bigger_tail_budget_never_moves_the_cut_later() {
        let msgs = two_exchanges();
        let mut prev: Option<usize> = None;
        for budget in 1..=60 {
            if let Some(i) = plan_cut(&msgs, budget) {
                if let Some(p) = prev {
                    assert!(i <= p, "budget {budget}: cut moved later {p} → {i}");
                }
                prev = Some(i);
            }
        }
    }

    // ---- prompt builders --------------------------------------------------

    /// Per-language: both builders substitute every placeholder in every
    /// built-in bundle (a renamed key would otherwise ship a literal `{words}`
    /// into the model's prompt).
    #[test]
    fn prompts_localized_for_all_langs() {
        for &lang in Lang::ALL {
            let l = locale(lang);
            let sys = summary_system_message(l, 250);
            assert!(sys.contains("250"), "{lang:?}: {sys}");
            assert!(
                !sys.contains('{') && !sys.contains('}'),
                "unsubstituted placeholder in {lang:?}: {sys}"
            );

            let roll = roll_user_message("ПРЕДЫДУЩЕЕ", "ДАЙДЖЕСТ", l, 250);
            assert!(roll.contains("ПРЕДЫДУЩЕЕ"), "{lang:?}: {roll}");
            assert!(roll.contains("ДАЙДЖЕСТ"), "{lang:?}: {roll}");
            assert!(roll.contains("250"), "{lang:?}: {roll}");
            assert!(
                !roll.contains('{') && !roll.contains('}'),
                "unsubstituted placeholder in {lang:?}: {roll}"
            );
        }
    }

    /// `tf` is single-pass, so a `{placeholder}` inside the digest is carried
    /// verbatim instead of being expanded from the argument list.
    #[test]
    fn a_placeholder_inside_the_digest_is_not_re_expanded() {
        let roll = roll_user_message("", "the user wrote {words} literally", ru(), 250);
        assert!(roll.contains("{words} literally"), "{roll}");
    }

    // ---- HistoryView (the read-back view, stage 3) ------------------------

    /// A small page size so the fixtures span several pages: 10 estimated tokens
    /// ≈ 40 bytes ≈ 20 Cyrillic characters.
    const PAGE: usize = 10;

    /// The guarantee the whole page shape exists for: walking `1..M` returns the
    /// transcript in full, with nothing dropped between pages.
    #[test]
    fn walking_every_page_reassembles_the_transcript() {
        let msgs = two_exchanges();
        let view = HistoryView::render(&msgs, ru()).unwrap();
        let total = view.page_count(PAGE);
        assert!(total > 1, "the fixture must span several pages");
        let mut joined = String::new();
        for p in 1..=total {
            joined.push_str(view.page(PAGE, p).unwrap());
        }
        assert_eq!(joined, view.text);
        assert!(view.page(PAGE, 0).is_none(), "pages are 1-based");
        assert!(view.page(PAGE, total + 1).is_none());
    }

    /// A hit has to come back with the page it sits on — that is what makes
    /// search and page reading compose (S16).
    #[test]
    fn locate_maps_a_message_to_its_page_and_block() {
        let msgs = two_exchanges();
        let view = HistoryView::render(&msgs, ru()).unwrap();
        for m in &msgs {
            if m.role == MessageRole::Tool {
                continue;
            }
            let (page, block) = view.locate(PAGE, m.id).unwrap();
            assert!(
                (1..=view.page_count(PAGE)).contains(&page),
                "page {page} out of range for {:?}",
                m.text
            );
            assert!(block.contains(m.text.trim()), "{block}");
        }
        assert_eq!(view.locate(PAGE, Uuid::new_v4()), None);
    }

    /// The one that matters for search: a tool **result** is rendered inside the
    /// assistant block, but the full-text index stores it under the `Tool`
    /// message's own id — so without the alias a hit on the bulk this feature is
    /// most about would map to no page at all.
    #[test]
    fn a_tool_message_resolves_to_the_block_that_carries_its_result() {
        let msgs = two_exchanges();
        let tool_msg = msgs.iter().find(|m| m.role == MessageRole::Tool).unwrap();
        let assistant = &msgs[1];
        let view = HistoryView::render(&msgs, ru()).unwrap();

        let (page, block) = view.locate(PAGE, tool_msg.id).unwrap();
        let (a_page, a_block) = view.locate(PAGE, assistant.id).unwrap();
        assert_eq!((page, block), (a_page, a_block));
        assert_eq!(view.order_of(tool_msg.id), view.order_of(assistant.id));
    }

    /// One renderer, two clips (see [`render`]): the summary got a clipped tool
    /// result, the reader must get the whole thing — otherwise paging back would
    /// return exactly what the summary already carried.
    #[test]
    fn the_reader_sees_a_tool_result_the_digest_had_to_clip() {
        let long = "я".repeat(TOOL_RESULT_CLIP + 50);
        let msgs = vec![
            Message::user("вопрос"),
            assistant_with_call("ответ", "c1", "fetch_url", Some(&long)),
        ];
        let digest = build_compaction_digest(&msgs, ru()).unwrap();
        let view = HistoryView::render(&msgs, ru()).unwrap();
        assert_eq!(digest.matches('я').count(), TOOL_RESULT_CLIP);
        assert!(digest.contains('…'));
        assert_eq!(
            view.text.matches('я').count(),
            TOOL_RESULT_CLIP + 50,
            "the reader must not clip"
        );
    }

    /// Conversation order, whatever order the index hands hits back in.
    #[test]
    fn order_of_follows_the_conversation() {
        let msgs = two_exchanges();
        let view = HistoryView::render(&msgs, ru()).unwrap();
        let first = view.order_of(msgs[0].id).unwrap();
        let later = view.order_of(msgs[4].id).unwrap();
        assert!(first < later, "{first} !< {later}");
    }

    #[test]
    fn view_is_none_without_meaningful_messages() {
        assert!(HistoryView::render(&[], ru()).is_none());
        let msgs = vec![Message::user("   "), tool_msg("c1", "calc", "42")];
        assert!(HistoryView::render(&msgs, ru()).is_none());
    }

    /// Blocks are separated, so a multi-line message cannot run into the next
    /// speaker's line — and the blank line is also where `paginate` prefers to cut.
    #[test]
    fn blocks_are_separated_in_the_readable_transcript() {
        let msgs = vec![
            Message::user("первая строка\nвторая строка"),
            Message::assistant("ответ"),
        ];
        let view = HistoryView::render(&msgs, ru()).unwrap();
        assert!(
            view.text.contains("вторая строка\n\nАссистент: ответ"),
            "{}",
            view.text
        );
    }

    // ---- the overflow detector ------------------------------------------

    /// One real body per provider, in the shape the client hands over: the
    /// status line it prepends, then the server's own words.
    #[test]
    fn every_provider_overflow_is_recognized() {
        let bodies = [
            // llama.cpp — measured (§9a M3), the primary audience's failure.
            "engine returned status 400 Bad Request: {\"error\":{\"code\":400,\
             \"type\":\"exceed_context_size_error\",\"n_prompt_tokens\":32706,\"n_ctx\":16384}}",
            "engine returned status 400: {\"error\":{\"code\":\"context_length_exceeded\"}}",
            "This model's maximum context length is 128000 tokens",
            "invalid_request_error: prompt is too long: 210000 tokens > 200000 maximum",
            "INVALID_ARGUMENT: The input token count (1200000) exceeds the maximum \
             number of tokens allowed (1048576)",
        ];
        for body in bodies {
            assert!(is_context_overflow(body), "not recognized: {body}");
            // Providers are inconsistent about case; matching must not depend on it.
            assert!(is_context_overflow(&body.to_uppercase()), "case: {body}");
        }
    }

    /// The detector only changes *which* advice is shown, so a false positive
    /// would send someone chasing a compaction that cannot help. An ordinary
    /// failure — including one that mentions tokens or a context — stays generic.
    #[test]
    fn ordinary_failures_are_not_mistaken_for_an_overflow() {
        for body in [
            "connection refused (os error 10061)",
            "engine returned status 503: server is still loading the model",
            "engine returned status 401: invalid api key",
            "engine returned status 400: unknown field `top_k`",
            // Mentions both words, means neither.
            "the tool returned 4096 tokens of context",
        ] {
            assert!(!is_context_overflow(body), "false positive: {body}");
        }
    }
}
