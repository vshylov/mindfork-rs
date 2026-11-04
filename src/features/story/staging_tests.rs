//! The staging pass on its own: its schema, its prompt and its notes
//! (docs/fabula.md §4, PR 12).

use serde_json::json;

use super::{StagingFinding, StagingKind, StagingReply, staging_notes, staging_prompt};
use crate::entities::story::{NoteKind, NoteSeverity, fixture_canon};
use crate::features::story::schema::tests::holds;
use crate::features::story::schema::{CanonKeys, StorySchema};

#[test]
fn the_schema_keeps_the_dialect_names_four_kinds_and_decides_last() {
    let keys = CanonKeys::of(&fixture_canon());
    let schema = StagingReply::schema(&keys);
    assert!(holds::<StagingReply>(&schema) >= 1);
    let text = schema.to_json();
    assert!(text.contains(r#""enum":["staging","referent","announced","thanks"]"#));
    let at = |name: &str| text.find(&format!("\"{name}\"")).unwrap();
    assert!(at("anchor") < at("note") && at("note") < at("kind") && at("kind") < at("severity"));
    for banned in ["reason", "thinking", "thought", "rationale"] {
        assert!(!text.contains(&format!("\"{banned}")), "{banned}");
    }
}

#[test]
fn each_kind_is_stored_as_a_note_kind_of_its_own() {
    let stored: Vec<NoteKind> = [
        StagingKind::Staging,
        StagingKind::Referent,
        StagingKind::Announced,
        StagingKind::Thanks,
    ]
    .map(StagingKind::note_kind)
    .to_vec();
    assert_eq!(
        stored,
        [
            NoteKind::Staging,
            NoteKind::Referent,
            NoteKind::Announced,
            NoteKind::Thanks
        ]
    );
    let names: Vec<&str> = stored.iter().map(|k| k.as_str()).collect();
    assert_eq!(names, ["staging", "referent", "announced", "thanks"]);
    assert!(
        stored
            .iter()
            .all(|k| NoteKind::parse(k.as_str()) == Some(*k))
    );
    assert!(
        stored.iter().all(|k| k.is_triaged()),
        "the Author decides them"
    );
}

#[test]
fn the_prompt_holds_the_text_and_nothing_of_the_canon() {
    let text = "## Dawn\n\n— Сын Марьи, что с ним на материк уехала.";
    let first = staging_prompt(&[], text);
    assert!(
        first
            .user
            .starts_with("This is the story's first chapter.\n\nThe new chapter:\n\n## Dawn")
    );
    let summaries = vec![(
        1,
        "Timur comes to the island; Vera keeps the light.".to_owned(),
    )];
    let prompt = staging_prompt(&summaries, text);
    assert!(prompt.user.starts_with(
        "The story so far, a summary of each published chapter:\n- Chapter 1: Timur comes to \
         the island; Vera keeps the light.\n\nThe new chapter:\n\n## Dawn"
    ));
    assert!(prompt.user.contains("Сын Марьи"));
    assert!(
        prompt
            .system
            .contains("the summaries name is known to the reader")
    );
    for kind in ["staging:", "referent:", "announced:", "thanks:"] {
        assert!(prompt.system.contains(kind), "{kind}");
    }
    assert!(prompt.system.contains("who left with whom"));
    assert!(prompt.system.contains("the moment calls for is no fault"));
    let canon = fixture_canon();
    let secret = &canon.characters[0].secrets[0].text;
    for part in [&first.system, &first.user, &prompt.system, &prompt.user] {
        assert!(!part.contains(secret.as_str()) && !part.contains(&canon.arc.premise));
        assert!(!part.contains("Author of a story"));
    }
}

#[test]
fn a_reply_becomes_notes_with_blanks_dropped() {
    let reply: StagingReply = serde_json::from_value(json!({"findings": [
        {
            "anchor": " Сын Марьи, что с ним ",
            "note": " Whose son? The text has Timur leave with his mother. ",
            "kind": "referent",
            "severity": "high",
        },
        {"anchor": "Спасибо", "note": "   ", "kind": "thanks", "severity": "low"},
        {"anchor": "  ", "note": "Opens on thanks again.", "kind": "thanks", "severity": "low"},
    ]}))
    .unwrap();
    let notes = staging_notes(reply);
    assert_eq!(notes.len(), 2, "the blank note is dropped");
    assert_eq!(notes[0].kind, NoteKind::Referent);
    assert_eq!(notes[0].severity, NoteSeverity::High);
    assert_eq!(notes[0].anchor.as_deref(), Some("Сын Марьи, что с ним"));
    assert_eq!(
        notes[0].body,
        "Whose son? The text has Timur leave with his mother."
    );
    assert!(notes[0].evidence.is_empty());
    assert_eq!(notes[1].kind, NoteKind::Thanks);
    assert_eq!(notes[1].anchor, None, "an empty anchor is none");
    let none = staging_notes(StagingReply {
        findings: vec![StagingFinding {
            anchor: "x".into(),
            note: String::new(),
            kind: StagingKind::Staging,
            severity: NoteSeverity::Low,
        }],
    });
    assert!(none.is_empty());
}
