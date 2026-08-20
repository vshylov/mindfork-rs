//! Orchestrator tests — mapping domain messages to the engine format. Part of the
//! [`super`] module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::app::orchestrator::request::inject_attachments;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::shared::api::ChatRequest;
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

/// A request built with default injection settings — the shape almost every test
/// here wants. The two knobs that tests actually vary get parameters; the rest
/// would only be noise repeated at each call site.
fn request_of(chat: &Chat, compaction: &CompactionSettings, history_tools: bool) -> ChatRequest {
    build_request(
        chat,
        SamplingConfig::default(),
        vec![],
        &PromptContext {
            attachments: &AttachmentSettings::default(),
            compaction,
            indexed: NO_INDEX,
            history_tools,
            // The workspace block has its own tests below; this shared helper
            // builds requests for chats with no project, where the flag is moot.
            workspace_tools: true,
            loc: ru(),
        },
    )
}

#[test]
fn build_request_puts_system_aside_and_maps_roles() {
    let mut p = Profile::new("X", "Ты — X.");
    p.greeting = Some("Здравствуйте!".into());
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::assistant("Здравствуйте!"));
    chat.push_message(Message::user("привет"));

    let req = request_of(&chat, &CompactionSettings::default(), true);
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

    let req = request_of(&chat, &CompactionSettings::default(), true);
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
    let req = request_of(&chat, &CompactionSettings::default(), true);
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
        request_of(&chat, &CompactionSettings::default(), history_tools)
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
    let req = request_of(&chat, &off, true);
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
    let req = request_of(&chat, &CompactionSettings::default(), true);
    assert_eq!(req.system.as_deref(), Some("Ты — X."));
    assert_eq!(req.messages.len(), 9);
}

/// The workspace block: present only with a project, naming the tools only when
/// the turn actually offers them, and absent entirely otherwise — the property
/// that keeps every existing conversation's request unchanged (spec §9.12).
#[test]
fn the_workspace_block_appears_only_with_a_project() {
    use crate::app::orchestrator::request::inject_workspace;
    use crate::entities::workspace::Workspace;

    let ws = Workspace::new("D:/Projects/app");
    let none = inject_workspace(Some("persona".into()), None, true, ru());
    assert_eq!(
        none,
        Some("persona".into()),
        "no project must leave the system prompt untouched"
    );

    let with = inject_workspace(Some("persona".into()), Some(&ws), true, ru()).unwrap();
    assert!(
        with.starts_with("persona"),
        "the persona stays first: {with}"
    );
    assert!(with.contains("D:/Projects/app"), "{with}");
    assert!(with.contains("app"), "the name is shown too: {with}");
    // It must name the tools that reach the project, or the model improvises
    // with the wrong ones (docs/lessons.md §4).
    assert!(with.contains("code_read"), "{with}");

    // Attached, but the profile has the tools off: the block must say the
    // project is out of reach rather than advertise an absent capability.
    let unreachable = inject_workspace(Some("persona".into()), Some(&ws), false, ru()).unwrap();
    assert!(unreachable.contains("D:/Projects/app"), "{unreachable}");
    assert!(
        !unreachable.contains("code_read"),
        "a tool this turn does not have must not be named: {unreachable}"
    );
}

/// An empty persona must not leave a stray blank line, and the block must still
/// be the whole system prompt.
#[test]
fn the_workspace_block_stands_alone_without_a_persona() {
    use crate::app::orchestrator::request::inject_workspace;
    use crate::entities::workspace::Workspace;

    let ws = Workspace::new("/home/u/app");
    let only = inject_workspace(None, Some(&ws), true, ru()).unwrap();
    assert!(!only.starts_with('\n'), "leading blank line: {only:?}");
    assert!(only.contains("/home/u/app"));
}

/// A request for a chat with no project is byte-identical to one built before
/// the feature — the safety property for every stored conversation.
#[test]
fn a_chat_without_a_project_builds_an_unchanged_request() {
    let p = Profile::new("X", "persona");
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::user("hi"));
    assert!(chat.workspace.is_none());
    let req = request_of(&chat, &CompactionSettings::default(), false);
    assert!(
        req.system.is_none() || !req.system.as_deref().unwrap().contains("code_read"),
        "no project must add nothing: {:?}",
        req.system
    );
}
