//! Orchestrator tests — mapping domain messages to the engine format. Part of the
//! [`super`] module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::app::orchestrator::request::inject_attachments;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::shared::config::{AttachmentSettings, CompactionSettings};

fn ru() -> &'static crate::shared::i18n::Locale {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
}

/// "Nothing is indexed" — the default state for the pure injection tests (the
/// semantic index is built in the background and is checked in its own tests).
const NO_INDEX: &[uuid::Uuid] = &[];

fn att(name: &str, text: &str, mode: AttachMode) -> Attachment {
    let bytes = text.len();
    Attachment::new(name, format!("/tmp/{name}"), text.to_string(), bytes, mode)
}

#[test]
fn build_request_puts_system_aside_and_maps_roles() {
    let mut p = Profile::new("X", "Ты — X.");
    p.greeting = Some("Здравствуйте!".into());
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::assistant("Здравствуйте!"));
    chat.push_message(Message::user("привет"));

    let req = build_request(
        &chat,
        SamplingConfig::default(),
        vec![],
        &PromptContext {
            attachments: &AttachmentSettings::default(),
            compaction: &CompactionSettings::default(),
            indexed: NO_INDEX,
            history_tools: true,
            loc: ru(),
        },
    );
    assert_eq!(req.system.as_deref(), Some("Ты — X."));
    assert_eq!(req.messages.len(), 2);
}

#[test]
fn build_request_appends_attached_files_to_system() {
    let p = Profile::new("X", "Ты — X.");
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::user("что в файле?"));
    chat.attachments
        .push(att("notes.md", "секретное число 4242", AttachMode::Inline));

    let req = build_request(
        &chat,
        SamplingConfig::default(),
        vec![],
        &PromptContext {
            attachments: &AttachmentSettings::default(),
            compaction: &CompactionSettings::default(),
            indexed: NO_INDEX,
            history_tools: true,
            loc: ru(),
        },
    );
    let system = req.system.expect("system with the attachment block");
    // The chat's own system message stays first, the block is appended.
    assert!(system.starts_with("Ты — X."), "{system}");
    assert!(system.contains("notes.md"), "{system}");
    assert!(system.contains("секретное число 4242"), "{system}");
    // Messages are untouched: the file doesn't pollute the conversation.
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].content, "что в файле?");
}

#[test]
fn inject_attachments_is_a_noop_without_attachments() {
    let system = Some("Ты — X.".to_string());
    assert_eq!(
        inject_attachments(
            system.clone(),
            &[],
            &AttachmentSettings::default(),
            NO_INDEX,
            ru()
        ),
        system
    );
    assert_eq!(
        inject_attachments(None, &[], &AttachmentSettings::default(), NO_INDEX, ru()),
        None
    );
}

#[test]
fn inline_carries_full_text_by_reference_only_an_excerpt() {
    // 20 estimated tokens ≈ 80 bytes ≈ 40 Cyrillic characters — enough for the
    // opening phrase, nowhere near the tail.
    let cfg = AttachmentSettings {
        excerpt_tokens: 20,
        ..Default::default()
    };
    let tail = "ХВОСТ-МАРКЕР";
    let long = format!("начало документа, довольно длинное вступление… {tail}");
    let items = vec![
        att("small.txt", "короткий текст", AttachMode::Inline),
        att("big.txt", &long, AttachMode::ByReference),
    ];
    let out = inject_attachments(None, &items, &cfg, NO_INDEX, ru()).expect("a block");

    // Inline — in full.
    assert!(out.contains("короткий текст"), "{out}");
    // By reference — the head is shown, the tail is not.
    assert!(out.contains("начало документа"), "{out}");
    assert!(
        !out.contains(tail),
        "the by-reference tail must stay out of the prompt: {out}"
    );
    // Both are announced by name, and the header marks the content as data.
    assert!(
        out.contains("small.txt") && out.contains("big.txt"),
        "{out}"
    );
    assert!(out.contains("ДАННЫЕ"), "{out}");
}

/// Regression for a defect seen on a live run: the by-reference entry showed an
/// excerpt but never said **how to read the rest**, so the model improvised with
/// the wrong tools (`fs_read` into the sandbox, then `web_search`) and ended up
/// asking the user for the impossible. The entry must name `attachment_read`,
/// state the page range, and say the file is unreachable by other means.
#[test]
fn by_reference_entry_tells_the_model_how_to_read_the_rest() {
    let cfg = AttachmentSettings {
        excerpt_tokens: 5,
        page_tokens: 10,
        ..Default::default()
    };
    let long = "слово ".repeat(200);
    let items = vec![att("big.txt", &long, AttachMode::ByReference)];
    let out = inject_attachments(None, &items, &cfg, NO_INDEX, ru()).unwrap();

    assert!(
        out.contains("attachment_read"),
        "the model must be told which tool reads the rest: {out}"
    );
    let pages = items[0].page_count(cfg.page_tokens);
    assert!(pages > 1, "the fixture must span several pages");
    assert!(
        out.contains(&pages.to_string()),
        "the page range must be stated ({pages} pages): {out}"
    );

    // An inline file needs no such pointer — it is already there in full.
    let inline = vec![att("small.txt", "коротко", AttachMode::Inline)];
    let out = inject_attachments(None, &inline, &cfg, NO_INDEX, ru()).unwrap();
    assert!(!out.contains("attachment_read"), "{out}");
}

/// Search is only offered for a file that actually has an index — promising it
/// over an unindexed file (no embedder configured) would send the model down a
/// dead end. Page reading is offered either way: it is the guaranteed path.
#[test]
fn search_is_offered_only_for_an_indexed_file() {
    let cfg = AttachmentSettings {
        excerpt_tokens: 5,
        page_tokens: 10,
        ..Default::default()
    };
    let items = vec![att(
        "big.txt",
        &"слово ".repeat(200),
        AttachMode::ByReference,
    )];

    let without = inject_attachments(None, &items, &cfg, NO_INDEX, ru()).unwrap();
    assert!(
        !without.contains("attachment_search"),
        "an unindexed file must not advertise search: {without}"
    );
    assert!(without.contains("attachment_read"), "{without}");

    let with = inject_attachments(None, &items, &cfg, &[items[0].id], ru()).unwrap();
    assert!(with.contains("attachment_search"), "{with}");
    assert!(
        with.contains("attachment_read"),
        "the guaranteed path stays advertised: {with}"
    );
}

#[test]
fn fence_widens_so_a_file_cannot_close_its_own_section() {
    // A file quoting the default fence must not be able to end its section and
    // have the rest read as instructions.
    let hostile = "текст >>> и ещё >>> внутри";
    let items = vec![att("evil.md", hostile, AttachMode::Inline)];
    let out =
        inject_attachments(None, &items, &AttachmentSettings::default(), NO_INDEX, ru()).unwrap();
    assert!(
        out.contains(hostile),
        "the content is still delivered: {out}"
    );
    // The fence around it is wider than any run of '>' in the content.
    assert!(
        out.contains(">>>>") && out.contains("<<<<"),
        "the fence must widen past the content's own run: {out}"
    );
}

#[test]
fn block_stands_alone_when_the_chat_has_no_system_message() {
    let items = vec![att("a.txt", "содержимое", AttachMode::Inline)];
    let out =
        inject_attachments(None, &items, &AttachmentSettings::default(), NO_INDEX, ru()).unwrap();
    assert!(out.starts_with('['), "the block leads: {out}");
    assert!(out.contains("содержимое"));
}

#[test]
fn block_is_localized_for_all_langs() {
    let items = vec![att("a.txt", "payload", AttachMode::ByReference)];
    for &lang in crate::shared::i18n::Lang::ALL {
        let loc = crate::shared::i18n::locale(lang);
        let out = inject_attachments(None, &items, &AttachmentSettings::default(), NO_INDEX, loc)
            .unwrap();
        assert!(!out.contains('{') && !out.contains('}'), "{lang:?}: {out}");
        if lang == crate::shared::i18n::Lang::En {
            assert!(
                !out.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                "Cyrillic leaked into the en block: {out}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// History compression (spec §6.7): the request carries a summary plus the
// verbatim tail, while `chat.messages` stays whole.
// ---------------------------------------------------------------------------

/// A chat of `n` user/assistant pairs, compacted at message index `upto`.
fn compacted_chat(n: usize, upto: usize, summary: &str) -> Chat {
    let p = Profile::new("X", "Ты — X.");
    let mut chat = Chat::from_profile(&p, "c");
    for i in 0..n {
        chat.push_message(Message::user(format!("вопрос {i}")));
        chat.push_message(Message::assistant(format!("ответ {i}")));
    }
    chat.compaction = Some(crate::entities::chat::Compaction {
        summary: summary.into(),
        upto,
        boundary_id: chat.messages[upto].id,
        compacted_at: chrono::Utc::now(),
        rolls: 1,
    });
    chat
}

#[test]
fn compaction_replaces_the_prefix_with_a_summary_block() {
    let chat = compacted_chat(5, 6, "Ранее: обсудили хранилище, выбрали SQLite.");
    let req = build_request(
        &chat,
        SamplingConfig::default(),
        vec![],
        &PromptContext {
            attachments: &AttachmentSettings::default(),
            compaction: &CompactionSettings::default(),
            indexed: NO_INDEX,
            history_tools: true,
            loc: ru(),
        },
    );
    // The persona stays first, the block is appended after it — ordered by
    // volatility so the most stable content keeps its prefix (spec §6.6).
    let system = req.system.expect("system with the summary block");
    assert!(system.starts_with("Ты — X."), "{system}");
    assert!(system.contains("выбрали SQLite"), "{system}");
    // Only the verbatim tail is sent, and the whole history is still on the chat.
    assert_eq!(req.messages.len(), 4);
    assert_eq!(chat.messages.len(), 10);
}

/// The block must name the read-back tools **only when this turn offers them**.
/// They normally travel together (sub-decision S12 gates both on the same
/// `compaction_view`), but a profile can switch the two tools off — and a block
/// that names a tool the model does not have is the dead end the sentence exists
/// to prevent, the fourth instance of that defect class in this codebase.
#[test]
fn the_block_names_the_read_back_tools_only_when_they_are_offered() {
    let chat = compacted_chat(5, 6, "Ранее: выбрали SQLite.");
    let block = |history_tools| {
        build_request(
            &chat,
            SamplingConfig::default(),
            vec![],
            &PromptContext {
                attachments: &AttachmentSettings::default(),
                compaction: &CompactionSettings::default(),
                indexed: NO_INDEX,
                history_tools,
                loc: ru(),
            },
        )
        .system
        .expect("system with the summary block")
    };

    let with = block(true);
    assert!(
        with.contains("history_search") && with.contains("history_read"),
        "{with}"
    );

    let without = block(false);
    assert!(
        !without.contains("history_search") && !without.contains("history_read"),
        "a tool the model does not have must not be named: {without}"
    );
    // Both wordings still carry the summary and say the block is a record.
    for system in [&with, &without] {
        assert!(system.contains("выбрали SQLite"), "{system}");
        assert!(system.contains("ДАННЫЕ"), "{system}");
    }
}

#[test]
fn the_master_switch_off_makes_compression_inert() {
    // Fork F10: off means the request is byte-for-byte what it was before the
    // feature existed — no block, no truncation — and the stored summary is
    // merely dormant, never discarded.
    let chat = compacted_chat(5, 6, "Ранее: выбрали SQLite.");
    let off = CompactionSettings {
        enabled: false,
        ..Default::default()
    };
    let req = build_request(
        &chat,
        SamplingConfig::default(),
        vec![],
        &PromptContext {
            attachments: &AttachmentSettings::default(),
            compaction: &off,
            indexed: NO_INDEX,
            history_tools: true,
            loc: ru(),
        },
    );
    assert_eq!(req.system.as_deref(), Some("Ты — X."));
    assert_eq!(req.messages.len(), 10);
    assert!(
        chat.compaction.is_some(),
        "the summary must survive the flip"
    );
}

#[test]
fn a_vanished_boundary_falls_back_to_the_whole_history() {
    // The boundary is re-found by id. If the message is gone the summary can no
    // longer be placed, so the full history is sent rather than a summary
    // silently covering the wrong span.
    let mut chat = compacted_chat(5, 6, "Ранее: выбрали SQLite.");
    chat.messages.remove(6);
    let req = build_request(
        &chat,
        SamplingConfig::default(),
        vec![],
        &PromptContext {
            attachments: &AttachmentSettings::default(),
            compaction: &CompactionSettings::default(),
            indexed: NO_INDEX,
            history_tools: true,
            loc: ru(),
        },
    );
    assert_eq!(req.system.as_deref(), Some("Ты — X."));
    assert_eq!(req.messages.len(), 9);
}
