//! Orchestrator tests — mapping domain messages to the engine format. Part of the
//! [`super`] module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::app::orchestrator::request::inject_attachments;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::shared::config::AttachmentSettings;

fn ru() -> &'static crate::shared::i18n::Locale {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
}

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
        &AttachmentSettings::default(),
        ru(),
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
        &AttachmentSettings::default(),
        ru(),
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
        inject_attachments(system.clone(), &[], &AttachmentSettings::default(), ru()),
        system
    );
    assert_eq!(
        inject_attachments(None, &[], &AttachmentSettings::default(), ru()),
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
    let out = inject_attachments(None, &items, &cfg, ru()).expect("a block");

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

#[test]
fn fence_widens_so_a_file_cannot_close_its_own_section() {
    // A file quoting the default fence must not be able to end its section and
    // have the rest read as instructions.
    let hostile = "текст >>> и ещё >>> внутри";
    let items = vec![att("evil.md", hostile, AttachMode::Inline)];
    let out = inject_attachments(None, &items, &AttachmentSettings::default(), ru()).unwrap();
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
    let out = inject_attachments(None, &items, &AttachmentSettings::default(), ru()).unwrap();
    assert!(out.starts_with('['), "the block leads: {out}");
    assert!(out.contains("содержимое"));
}

#[test]
fn block_is_localized_for_all_langs() {
    let items = vec![att("a.txt", "payload", AttachMode::ByReference)];
    for &lang in crate::shared::i18n::Lang::ALL {
        let loc = crate::shared::i18n::locale(lang);
        let out = inject_attachments(None, &items, &AttachmentSettings::default(), loc).unwrap();
        assert!(!out.contains('{') && !out.contains('}'), "{lang:?}: {out}");
        if lang == crate::shared::i18n::Lang::En {
            assert!(
                !out.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                "Cyrillic leaked into the en block: {out}"
            );
        }
    }
}
