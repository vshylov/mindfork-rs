//! Demo fixture — the showcase content behind the generated screenshots and
//! the interactive `mindfork demo` mode (docs/history/demo-screenshots.md).
//!
//! The content is honest fabrication: a plausible conversation that walks the
//! renderer through its range — a GFM table, a highlighted code block, a
//! Mermaid flowchart, delimiter-scoped LaTeX, an expanded "thoughts" block and
//! a tool call with its result. Ids and timestamps are fixed so a captured
//! frame is byte-stable across runs (the drift gate depends on it) and so
//! [`provision`] is idempotent.

use chrono::{DateTime, TimeZone, Utc};
use uuid::Uuid;

#[cfg(test)]
use crate::entities::chat::ChatSummary;
use crate::entities::chat::{Chat, FeedView};
use crate::entities::message::{Message, MessageRole, ToolCallRecord};
use crate::entities::profile::Profile;
use crate::entities::self_model::{Goal, GoalStatus, NarrativeSegment, SelfModel, UserModel};
use crate::features::tools::default_tool_ids;
use crate::shared::api::contract::{ChatChunk, FinishReason, TokenUsage};
#[cfg(test)]
use crate::shared::config::Theme;
use crate::shared::config::{AppConfig, ServerMode};
use crate::shared::i18n::Lang;
use crate::shared::storage::Storage;

/// Title of the showcase chat (shown in the feed header).
pub const CHAT_TITLE: &str = "Gemma 4 on a 12 GB GPU";

/// The unsent draft in the input box: the hero must show a *live* seat — an
/// empty prompt reads as a screensaver. The question also sets up the next
/// beat of the demo world: the settings capture shows exactly this draft
/// model configured, and the chat list holds the speculative-decoding chat.
pub const INPUT_DRAFT: &str = "And speculative decoding — is a 1B draft worth it on this card?";

/// Fixed chat id — captures must not depend on a fresh `Uuid::new_v4()`.
pub fn chat_id() -> Uuid {
    Uuid::from_u128(0x6d69_6e64_666f_726b_5f64_656d_6f5f_3031)
}

/// The feed view for captures: thoughts and tool-call details expanded —
/// collapsed pills demonstrate nothing.
pub fn feed_view() -> FeedView {
    FeedView {
        thoughts: true,
        tools: true,
    }
}

/// Fixed timestamp base; messages step forward a minute at a time.
fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 1, 10, minute, 0).unwrap()
}

/// Fixed calendar date for the list/self-model fixtures.
fn date(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

fn message(role: MessageRole, minute: u32, text: &str) -> Message {
    let mut m = Message::new(role, text);
    // Fixed id derived from the slot, fixed time: determinism over realism.
    m.id = Uuid::from_u128(0x6d66_5f64_656d_6f00 + minute as u128);
    m.timestamp = at(minute);
    m
}

/// The showcase conversation. Composed so that the *tail* — what a capture at
/// the stage-1 frame size actually shows — contains the table's neighbourhood,
/// the flowchart, the LaTeX line, the expanded thoughts and the tool card,
/// while the opening exchange scrolls off the top (a conversation visibly has
/// history). `app/demo_shots.rs` asserts the load-bearing pieces are on
/// screen, so edits here cannot silently push them out of frame.
pub fn showcase_messages() -> Vec<Message> {
    let m1 = message(
        MessageRole::User,
        0,
        "Which Gemma 4 12B quant fits a 12 GB card with room for a 16k context?",
    );

    let m2 = message(
        MessageRole::Assistant,
        1,
        "Three candidates, weights only — the KV cache comes on top:\n\n\
         | Quant | Weights | Quality cost |\n\
         |---|---|---|\n\
         | Q4_K_M | ~7.3 GB | small but measurable |\n\
         | Q5_K_M | ~8.4 GB | negligible |\n\
         | Q6_K | ~9.7 GB | noise level |\n\n\
         At 16k context the KV cache adds roughly 1.5 GB, so **Q5_K_M** is the \
         sweet spot — quality headroom *and* a safety margin:\n\n\
         ```bash\n\
         llama-server -m gemma-4-12B-it-Q5_K_M.gguf -ngl 99 -c 16384 --jinja\n\
         ```",
    );

    let m3 = message(
        MessageRole::User,
        2,
        "Give me the bottom line — a small table plus a decision diagram, and \
         save a note about my setup.",
    );

    // The decision tree renders with three perfectly straight legs only for
    // these exact label widths (mermaid-text centers nodes by content) —
    // rewording a label can bend the middle edge into a `┌─┘` jog. Eyeball
    // the regenerated dump after any edit here.
    let mut m4 = message(
        MessageRole::Assistant,
        3,
        "The short version:\n\n\
         | Quant | 16k context? | Verdict |\n\
         |---|---|---|\n\
         | Q5_K_M | fits, ~1.5 GB spare | **sweet spot** |\n\
         | Q6_K | tight | quality first |\n\n\
         ```mermaid\n\
         flowchart TD\n\
             A{How much context?} -->|8k| B[Q6_K — quality first]\n\
             A -->|16k| C[Q5_K_M — sweet spot]\n\
             A -->|32k| D[Q4_K_M — KV headroom]\n\
         ```\n\n\
         Note saved. The cache rule to remember: $M_{kv} \\propto L$ — double \
         the window, double the cache.",
    );
    m4.thoughts = Some(
        "The note should record the 12 GB budget and the Q5_K_M choice, so \
         future sizing questions start from the hardware."
            .into(),
    );
    m4.tool_calls = vec![ToolCallRecord {
        id: "call_demo_1".into(),
        name: "note_save".into(),
        arguments: serde_json::json!({
            "title": "Hardware budget",
            "text": "12 GB GPU; Gemma 4 12B at Q5_K_M, 16k context."
        }),
        result: Some("Note saved: \"Hardware budget\".".into()),
        thought_signature: None,
    }];

    vec![m1, m2, m3, m4]
}

/// Fixed profile id for the demo companion.
pub fn profile_id() -> Uuid {
    Uuid::from_u128(0x6d66_5f64_656d_6f5f_7072_6f66_0000_0001)
}

/// The demo companion: a named profile with the base tools enabled (so the
/// settings screen's Tools section shows a real toggle set), a fixed id (the
/// seeded chats and self-model reference it) and a greeting that says plainly
/// what the demo is — a new chat's first message must close the door on "is
/// this a real model?".
pub fn profile() -> Profile {
    let mut p = Profile::new("Gaia", "You are Gaia — a thoughtful local companion.");
    p.id = profile_id();
    p.enabled_tools = default_tool_ids();
    p.greeting = Some(
        "Hi! You're in the **mindfork demo** — I'm a scripted engine, not a \
         model. Everything else is the real app: browse the chats (`Esc`), \
         peek at my self-model (`F3`), fold my thoughts (`Ctrl+T`), open \
         settings (`Ctrl+P`). Connect a real engine there — local llama.cpp \
         or a cloud key — and this seat gets a mind."
            .into(),
    );
    p
}

/// The app config the settings captures depict: a healthy managed llama.cpp
/// setup consistent with the showcase conversation (the same model and
/// context the hero chat recommends). Capture-only — the interactive demo
/// boots [`demo_config`] instead.
#[cfg(test)]
pub fn app_config(theme: Theme) -> AppConfig {
    let mut c = AppConfig::default();
    c.interface.theme = theme;
    c.interface.language = Lang::En;
    c.engine.managed.binary = Some("C:\\llama.cpp\\llama-server.exe".into());
    c.engine.managed.model_path = Some("C:\\models\\gemma-4-12B-it-Q5_K_M.gguf".into());
    c.engine.managed.gpu_layers = 99;
    c.engine.managed.context_size = 16384;
    // Speculative decoding with a 1B draft: the demo user acted on the
    // "Speculative decoding: draft models" chat, and the hero's input draft
    // asks about exactly this. Also what fills the Model/server capture —
    // draft-* types reveal four more parameter rows (settings::helpers).
    c.engine.managed.spec_type = crate::shared::config::SpecType::DraftSimple;
    c.engine.managed.draft_model = Some("C:\\models\\gemma-4-1B-it-Q8_0.gguf".into());
    c.engine.managed.draft_gpu_layers = Some(99);
    c.engine.managed.draft_n_max = Some(16);
    c
}

/// One filler-chat row. Columns: title, the message count the *list capture*
/// shows (the interactive demo's filler chats hold a two-message excerpt, so
/// their real count is 2), created (m,d,h,min), modified (m,d,h,min).
type Row = (
    &'static str,
    usize,
    (u32, u32, u32, u32),
    (u32, u32, u32, u32),
);

/// The filler chats behind both the list capture and the interactive demo's
/// chat list — enough of them to fill the uniform gallery frame (PANEL_H in
/// `app/demo_shots.rs`) to the last row. Sorted by `modified` descending, the
/// list screen's default order. One row per line — a repeated constructor
/// block per chat trips the duplication detector (structurally identical
/// multi-line blocks are a sliding self-duplicate), and rustfmt would reflow
/// the rows right back into that shape — hence the skip: this is a table,
/// and the row-per-line layout is the point.
#[rustfmt::skip]
const ROWS: [Row; 21] = [
    ("Why is prompt eval slow on long chats?",  14, (7, 31, 17,  5), (7, 31, 18, 22)),
    ("Grammar-constrained JSON output",         13, (7, 30, 20, 11), (7, 31,  9, 14)),
    ("Sampler settings for livelier replies",   18, (7, 29,  9, 12), (7, 30, 21, 40)),
    ("Speculative decoding: draft models",      12, (7, 29,  8,  0), (7, 29, 19,  5)),
    ("Embeddings: why bge-m3 for the notes",    10, (7, 28, 12, 40), (7, 28, 14,  3)),
    ("Refactoring a god object in Rust",        41, (7, 26, 14, 30), (7, 27, 17, 52)),
    ("A tokio deadlock, found and fixed",       33, (7, 26,  9, 12), (7, 26, 21, 35)),
    ("What does the DRY penalty actually do?",   9, (7, 25, 11,  3), (7, 25, 12, 44)),
    ("MCP servers worth installing",            16, (7, 24, 10,  2), (7, 24, 11, 47)),
    ("Terminal fonts and box-drawing glyphs",    8, (7, 23, 19,  1), (7, 23, 19, 58)),
    ("Trip notes: the Dolomites in October",    26, (7, 20, 18, 15), (7, 22, 20, 31)),
    ("Shrink the system prompt, keep the voice", 11, (7, 21, 14, 30), (7, 21, 15, 26)),
    ("RAG chunk size for code search",          19, (7, 20, 21, 15), (7, 20, 22, 44)),
    ("Reading list: attention papers",          15, (7, 17,  7, 45), (7, 19, 23, 10)),
    ("Backup dry run before the update",         7, (7, 18, 16, 20), (7, 18, 16, 58)),
    ("Regex or a real parser?",                 23, (7, 17,  8, 50), (7, 17, 12, 19)),
    ("Mermaid diagrams in the terminal",        11, (7, 15, 13,  0), (7, 16, 10, 27)),
    ("Wool or synthetic for autumn hikes?",     12, (7, 15, 18, 44), (7, 15, 20,  5)),
    ("Naming things: a short rant",             27, (7, 14, 13, 20), (7, 14, 16, 31)),
    ("Reading GGUF metadata in Python",          9, (7, 13, 10,  5), (7, 13, 11, 58)),
    ("First week with mindfork: impressions",    5, (7, 12, 20, 30), (7, 12, 21, 12)),
];

/// A two-message excerpt per filler chat, aligned with [`ROWS`] by index —
/// the interactive demo's chats open to a real (if brief) exchange, so the
/// list, search and export all have something true to show.
#[rustfmt::skip]
const ROW_BODIES: [(&str, &str); 21] = [
    ("Generation is fine, but the pause before the first token keeps growing. What gives?",
     "That pause is prompt eval: after an edit or a compaction the whole history is re-scored before the first new token. Keep FlashAttention on and watch `prompt eval time` in the server log — past ~10k tokens the wait is mostly that."),
    ("Can I force valid JSON out of the model for tool results?",
     "Yes — llama.cpp compiles a `json_schema` from the request into a GBNF grammar, and the sampler simply cannot emit an invalid token. Strictness is free; the model only loses the option to ramble."),
    ("What's a good starting point to make replies less flat?",
     "Raise the temperature a touch and add `min_p 0.05` — then one knob at a time. Dynamic temperature (`dynatemp_range`) is the fun one: it adapts to per-token entropy."),
    ("Do draft models actually help on a single GPU?",
     "Yes, when the draft is much smaller than the target: `--spec-type draft` with a ~1B draft for a 12B model often lands 1.5-2x. The `ngram` variants need no second model — try those first."),
    ("Why bge-m3 for the knowledge base and not something newer?",
     "Multilingual, an 8k input window, and one model covers notes and chat attachments alike. Dense retrieval does the heavy lifting; the full-text index catches the exact-match tail."),
    ("My `AppState` struct has 40 fields. Where do I start?",
     "Group the fields by who mutates them together — each cluster is a struct candidate. Then move methods to the cluster that owns their data; the borrow checker will referee."),
    ("Two tasks each hold a lock and await the other. How do I even see this?",
     "`tokio-console` shows both tasks parked on `Mutex::lock`. The cure is ordering: acquire the locks in one global order — or merge them; a message channel often deletes the second lock outright."),
    ("What does the DRY penalty actually do?",
     "It penalizes verbatim repetition of recent sequences, scaled by match length — it kills loops without flattening style the way a high `repeat_penalty` does."),
    ("Which MCP servers are actually useful day to day?",
     "Filesystem and fetch cover most of it; add a search server if you live outside the browser. Anything more exotic — write your own: the protocol fits in fifty lines."),
    ("Why do table borders fall apart in my terminal?",
     "Your font lacks the box-drawing block, so the terminal substitutes a wider glyph and the grid drifts. JetBrains Mono or Cascadia fixes it — or flip on compat mode and the frames go ASCII."),
    ("Three days around Cortina in October — doable?",
     "Doable, but pack for two seasons: the rifugi start closing mid-October. The Tre Cime loop, Cinque Torri and Lago di Sorapis cover the greatest hits."),
    ("My system prompt is 900 tokens. What survives a trim?",
     "Keep the identity, the tools and the two rules you actually enforce; cut the examples first — a model infers style from the voice, not from a list of adjectives. 300 tokens reads the same."),
    ("What chunk size works for retrieving code?",
     "Whole functions beat fixed windows: split on definitions and keep the signature with the body. For prose, 400-600 tokens with a little overlap is the boring answer that works."),
    ("Which attention papers should I read after the 2017 one?",
     "Sparse and linear attention surveys, FlashAttention for the systems side, then RoPE and its long-context descendants — that's the spine of the modern stack."),
    ("Anything to check before I update?",
     "Run `mindfork backup -p <password>`, then restore it into a scratch folder. A backup you never restored is a hope, not a backup."),
    ("At what point does a regex stop being enough?",
     "The moment you reach for a lookbehind to balance brackets. Nesting means a grammar — and a forty-line recursive descent is usually clearer than the clever pattern it replaces."),
    ("Can you draw a flowchart right in the chat?",
     "Yes — fence a ```mermaid block: flowcharts and sequence diagrams render as text graphics, and anything else falls back to the source."),
    ("Base layer for wet cold — wool or synthetic?",
     "Merino for the long smell-proof days, synthetic for the wet ones: it dries in an hour where wool sulks. Drizzle above zero — synthetic; crisp and dry — merino."),
    ("Why is `Manager` always a design smell?",
     "Because it names the absence of a decision — everything manages something. Name the responsibility (`Scheduler`, `Registry`, `Janitor`) and half the design questions answer themselves."),
    ("How do I read a GGUF header without loading the model?",
     "The header is plain: magic, version, then key-value pairs — the `gguf` package on PyPI reads it in three lines. Quant type, context length and the tokenizer are all right there."),
    ("So, a week in — what stuck?",
     "The notes: answers that start from what we already established feel different. And `Ctrl+T` — reading the thinking taught me how to ask better questions."),
];

/// The chat list: the showcase chat on top (active), then a spread of
/// plausible topics with fixed dates and counts — exactly enough rows to
/// fill the uniform gallery frame (`app/demo_shots.rs::PANEL_H`) with no
/// scrolling and no trailing void. Capture-only (the interactive demo's
/// list comes from the real seeded chats).
#[cfg(test)]
pub fn chat_summaries() -> Vec<ChatSummary> {
    let mut chats = vec![ChatSummary {
        id: chat_id(),
        title: CHAT_TITLE.into(),
        created_at: date(8, 1, 10, 0),
        modified_at: date(8, 1, 10, 3),
        message_count: 4,
    }];
    chats.extend(ROWS.iter().enumerate().map(|(i, (title, count, c, m))| {
        ChatSummary {
            // Slots continue the stage-2 numbering (1-based after the hero).
            id: Uuid::from_u128(0x6d66_5f64_656d_6f5f_6c69_7374_0000_0000 + i as u128 + 1),
            title: (*title).into(),
            created_at: date(c.0, c.1, c.2, c.3),
            modified_at: date(m.0, m.1, m.2, m.3),
            message_count: *count,
        }
    }));
    chats
}

/// The self-model the `F3` capture shows: a companion that has already lived
/// a little — a voice, three goals in two states, a user model, and a few
/// narrative observations consistent with the showcase chat.
pub fn self_model() -> SelfModel {
    let goal = |slot: u128,
                text: &str,
                status: GoalStatus,
                created: DateTime<Utc>,
                closed: Option<DateTime<Utc>>| Goal {
        id: Uuid::from_u128(0x6d66_5f64_656d_6f5f_676f_616c_0000_0000 + slot),
        description: text.into(),
        status,
        created_at: created,
        closed_at: closed,
    };
    let segment = |slot: u128, text: &str, created: DateTime<Utc>| NarrativeSegment {
        id: Uuid::from_u128(0x6d66_5f64_656d_6f5f_6e61_7272_0000_0000 + slot),
        text: text.into(),
        created_at: created,
    };
    SelfModel {
        profile_id: profile_id(),
        version: 7,
        summary: "I run locally and help with practical engineering — model \
                  sizing, Rust, the occasional trip plan. I prefer measured \
                  numbers to adjectives, and I write things down: advice \
                  should start from remembered facts, not fresh guesses. The \
                  current thread is fitting Gemma 4 onto a 12 GB card without \
                  giving up context; the next experiment on the list is a 1B \
                  draft for speculative decoding."
            .into(),
        goals: vec![
            goal(
                1,
                "Answer sizing questions from saved notes alone — no re-asking about hardware.",
                GoalStatus::Active,
                date(7, 22, 12, 0),
                None,
            ),
            goal(
                2,
                "Keep recommendations reproducible: name the exact quant, context and flags.",
                GoalStatus::Active,
                date(7, 25, 9, 30),
                None,
            ),
            goal(
                4,
                "Measure the 1B draft's speedup and save the number, not the impression.",
                GoalStatus::Active,
                date(7, 31, 18, 40),
                None,
            ),
            goal(
                3,
                "Index the llama.cpp server docs into the knowledge base.",
                GoalStatus::Completed,
                date(7, 16, 15, 0),
                Some(date(7, 19, 11, 20)),
            ),
            goal(
                5,
                "Turn the October trip research into a packing-checklist note.",
                GoalStatus::Completed,
                date(7, 18, 9, 15),
                Some(date(7, 21, 17, 30)),
            ),
        ],
        user_model: UserModel {
            perceived_traits: vec![
                "methodical".into(),
                "impatient with vague answers".into(),
                "allergic to hand-waving".into(),
            ],
            current_interests: vec![
                "local model tuning".into(),
                "terminal tooling".into(),
                "speculative decoding".into(),
            ],
            relationship_dynamic: "Collaborative and direct; jokes land better after the numbers."
                .into(),
        },
        // Chronological vec order — the screen shows the narrative newest-
        // -first by *insertion*, the way a real profile accretes segments.
        // Times stay mid-day UTC: the screen renders dates in local time, and
        // a near-midnight timestamp would show a different calendar day on
        // CI (UTC) than on the machine that regenerated the dumps.
        narrative: vec![
            segment(
                4,
                "The packing checklist came out of three scattered chats — consolidation is where notes earn their keep.",
                date(7, 21, 17, 35),
            ),
            segment(
                1,
                "Tables beat prose here: comparisons get read, paragraphs get skimmed.",
                date(7, 24, 19, 40),
            ),
            segment(
                5,
                "Trip planning keeps landing between engineering questions — the terminal is a place to live, not a topic.",
                date(7, 26, 19, 2),
            ),
            segment(
                2,
                "The 12 GB VRAM budget keeps coming up — saved it as a note so I stop re-asking.",
                date(7, 28, 10, 15),
            ),
            segment(
                6,
                "Answers that cite the note they came from get fewer follow-ups — receipts beat repetition.",
                date(7, 30, 15, 55),
            ),
            segment(
                3,
                "Showing the exact command I ran turns out to be the fastest way to build trust.",
                date(8, 1, 10, 3),
            ),
        ],
        updated_at: date(8, 1, 10, 3),
    }
}

/// The config the interactive demo boots with: English UI on the Auto theme
/// (a real terminal supplies the colors), the showcase chat active, and the
/// engine honestly labeled — `external` mode with the model name
/// `demo (mock engine)`, which the feed header shows as the model caption.
/// The supervisor is mocked, so no connection is ever attempted.
pub fn demo_config() -> AppConfig {
    let mut c = AppConfig::default();
    c.interface.language = Lang::En;
    c.engine.mode = ServerMode::External;
    c.engine.external.model_name = Some("demo (mock engine)".into());
    c.last_active_chat = Some(chat_id());
    c
}

/// The showcase conversation as a real, saveable chat — the one the demo
/// opens on. Fixed id/timestamps; thoughts and tool details pre-expanded,
/// like the hero screenshot.
pub fn showcase_chat() -> Chat {
    let mut chat = Chat::from_profile(&profile(), CHAT_TITLE);
    chat.id = chat_id();
    chat.created_at = at(0);
    chat.modified_at = at(3);
    chat.messages = showcase_messages();
    chat.feed_view = feed_view();
    // The same unsent question the hero screenshot shows in the input box —
    // the interactive demo opens mid-thought, ready to send.
    chat.draft = INPUT_DRAFT.into();
    chat
}

/// The filler chats as real chats: [`ROWS`] titles and dates with the
/// [`ROW_BODIES`] two-message excerpts.
pub fn filler_chats() -> Vec<Chat> {
    ROWS.iter()
        .zip(ROW_BODIES.iter())
        .enumerate()
        .map(|(i, ((title, _, c, m), (user, assistant)))| {
            let created = date(c.0, c.1, c.2, c.3);
            let modified = date(m.0, m.1, m.2, m.3);
            let mut chat = Chat::from_profile(&profile(), *title);
            chat.id = Uuid::from_u128(0x6d66_5f64_656d_6f5f_6c69_7374_0000_0000 + i as u128 + 1);
            chat.created_at = created;
            chat.modified_at = modified;
            let mut u = Message::user(*user);
            u.id = Uuid::from_u128(0x6d66_5f64_656d_6f5f_6d73_6700 + (i as u128) * 2);
            u.timestamp = created;
            let mut a = Message::assistant(*assistant);
            a.id = Uuid::from_u128(0x6d66_5f64_656d_6f5f_6d73_6700 + (i as u128) * 2 + 1);
            a.timestamp = modified;
            chat.messages = vec![u, a];
            chat
        })
        .collect()
}

/// Splits reply text into few-word chunks so the demo's streaming is visibly
/// a stream, the way a real server delivers tokens.
fn stream_text(text: &str, out: &mut Vec<ChatChunk>) {
    let words: Vec<&str> = text.split(' ').collect();
    for group in words.chunks(3) {
        let mut piece = group.join(" ");
        piece.push(' ');
        out.push(ChatChunk::Text(piece));
    }
    if let Some(ChatChunk::Text(last)) = out.last_mut() {
        while last.ends_with(' ') {
            last.pop();
        }
    }
}

/// One canned reply: optional thoughts, streamed text, plausible usage, stop.
fn reply(thoughts: Option<&str>, text: &str, prompt_tokens: u32) -> Vec<ChatChunk> {
    let mut chunks = Vec::new();
    let mut reasoning_tokens = 0;
    if let Some(t) = thoughts {
        chunks.push(ChatChunk::Thoughts(t.into()));
        reasoning_tokens = (t.len() / 4) as u32;
    }
    stream_text(text, &mut chunks);
    chunks.push(ChatChunk::Usage(TokenUsage {
        prompt_tokens,
        completion_tokens: (text.len() / 4) as u32 + reasoning_tokens,
        reasoning_tokens,
    }));
    chunks.push(ChatChunk::Finished(FinishReason::Stop));
    chunks
}

/// The demo engine's replies, cycled in order by `MockBackend::cycling`.
/// Self-contained on purpose: background calls (impersonation, a regenerate)
/// may consume a script out of turn, so no reply depends on which question
/// preceded it — each is honest about being scripted and shows something
/// real about the app.
pub fn demo_replies() -> Vec<Vec<ChatChunk>> {
    vec![
        reply(
            Some("Best to be upfront about what I am before pretending to be clever."),
            "Fair warning: **I'm the demo engine** — a scripted stand-in cycling \
             through a fixed set of replies, no model behind me. Everything around \
             me is real, though: this streaming, the collapsible thoughts above \
             (`Ctrl+T`), the notes, the search, the themes. Press `Ctrl+P` and \
             point mindfork at a real engine — then this seat gets a mind.",
            412,
        ),
        reply(
            Some("A quick rendering tour says more than a paragraph of claims."),
            "A few things the feed renders natively:\n\n\
             | Piece | In the terminal |\n\
             |---|---|\n\
             | GFM tables | this one |\n\
             | Code | `cargo run` with highlighting |\n\
             | Mermaid | flowcharts as text graphics |\n\
             | LaTeX | $E = mc^2$, no rasterization |\n\n\
             Ask about diagrams in the *Mermaid* chat in the list (`Esc`) to see \
             a flowchart drawn live.",
            498,
        ),
        reply(
            None,
            "Connecting a real engine takes a minute: `Ctrl+P` → Model/server. \
             Local — point the `external` mode at any OpenAI-compatible server \
             (llama.cpp, vLLM, LM Studio, Ollama), or let the `managed` mode \
             launch `llama-server` for you. Cloud — pick OpenAI, Gemini, Claude \
             or Grok and paste an API key; it is stored encrypted, bound to this \
             machine.",
            531,
        ),
        reply(
            Some(
                "The self-model is the differentiating feature — worth pointing at \
                 the living example seeded in this very demo.",
            ),
            "What makes mindfork more than a chat window is memory with a spine: \
             I keep notes, and I maintain a **self-model** — who I am, goals, a \
             model of you, observations over time. Press `F3` to read the one \
             seeded in this demo. With a real model in this seat, it grows on its \
             own as we talk.",
            577,
        ),
    ]
}

/// Provisions a demo data root: config, the companion profile, the showcase
/// chat, the filler chats and the self-model. Idempotent — every id is fixed,
/// so re-running overwrites the same records. Isolation is the caller's job
/// (`main` passes a throwaway temp root).
pub fn provision(storage: &Storage) -> anyhow::Result<()> {
    storage.json().save_config(&demo_config())?;
    storage.json().upsert_profile(&profile())?;
    storage.json().save_chat(&showcase_chat())?;
    for chat in filler_chats() {
        storage.json().save_chat(&chat)?;
    }
    storage.db().self_model_upsert(&self_model())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture is frozen: same ids, same timestamps, same text on every
    /// call — the property every capture and the drift gate stand on.
    #[test]
    fn fixture_is_deterministic() {
        let a = showcase_messages();
        let b = showcase_messages();
        assert_eq!(a, b);
        assert_eq!(a[0].timestamp, at(0));
        assert_ne!(a[0].id, a[1].id, "slot-derived ids must not collide");
    }

    /// The showcase must keep exercising the renderer's range — losing a
    /// feature from the fixture silently demotes the screenshots.
    #[test]
    fn fixture_covers_the_showcase_features() {
        let all = showcase_messages();
        let text: String = all.iter().map(|m| m.text.as_str()).collect();
        for needle in ["| Quant |", "```bash", "```mermaid", "$M_{kv}"] {
            assert!(text.contains(needle), "fixture lost {needle:?}");
        }
        let last = all.last().unwrap();
        assert!(
            last.thoughts.is_some(),
            "the thoughts block is part of the showcase"
        );
        assert_eq!(
            last.tool_calls.len(),
            1,
            "the tool card is part of the showcase"
        );
    }

    /// Provisioning a fresh root seeds everything the demo needs, and doing
    /// it twice changes nothing — every id is fixed.
    #[test]
    fn provision_seeds_a_complete_demo_root() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(crate::shared::paths::Paths::with_root(dir.path())).unwrap();
        provision(&storage).unwrap();
        provision(&storage).unwrap();

        let config = storage.json().load_config().unwrap();
        assert_eq!(config.last_active_chat, Some(chat_id()));
        assert_eq!(
            config.engine.external.model_name.as_deref(),
            Some("demo (mock engine)"),
            "the feed-header caption must say what the engine is"
        );

        let profiles = storage.json().load_profiles().unwrap();
        assert_eq!(profiles.len(), 1, "idempotent: one profile after two runs");
        assert_eq!(profiles[0].id, profile_id());
        let greeting = profiles[0].greeting.as_deref().unwrap_or_default();
        assert!(
            greeting.contains("demo"),
            "a new chat must say what this is"
        );

        let files = storage.json().chat_files().unwrap();
        assert_eq!(files.len(), 22, "the showcase chat + 21 fillers");
        let showcase = storage.json().load_chat(chat_id()).unwrap().unwrap();
        assert_eq!(showcase.messages.len(), 4);
        assert!(
            showcase.feed_view.thoughts,
            "the hero look: thoughts expanded"
        );
        assert_eq!(showcase.profile_id, profile_id());

        let model = storage.db().self_model_get(profile_id()).unwrap();
        assert!(model.is_some(), "F3 must have something to show");
    }

    /// Every canned reply is a complete turn — text, usage, a terminal Stop —
    /// and the first one names what it is: a demo must close the door on
    /// "is this a real model?".
    #[test]
    fn demo_replies_are_complete_selfcontained_turns() {
        let replies = demo_replies();
        assert!(replies.len() >= 3, "cycling needs variety");
        for (i, r) in replies.iter().enumerate() {
            assert!(
                matches!(r.last(), Some(ChatChunk::Finished(FinishReason::Stop))),
                "reply {i} must end with Stop"
            );
            assert!(
                r.iter().any(|c| matches!(c, ChatChunk::Text(_))),
                "reply {i} has text"
            );
            assert!(
                r.iter().any(|c| matches!(c, ChatChunk::Usage(_))),
                "reply {i} reports usage (the token counter is part of the showcase)"
            );
        }
        let first: String = replies[0]
            .iter()
            .filter_map(|c| match c {
                ChatChunk::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            first.contains("demo engine"),
            "the first reply says what it is"
        );
    }
}
