use std::io::Write;

use base64::Engine;

use super::*;
use crate::shared::i18n::{Lang, locale};

fn en() -> &'static Locale {
    locale(Lang::En)
}

fn card_of(bytes: &[u8]) -> Card {
    match sniff(bytes, en()).unwrap() {
        Sniffed::Card(c) => *c,
        Sniffed::NotCard => panic!("not read as a card"),
    }
}

fn err_of(bytes: &[u8]) -> String {
    match sniff(bytes, en()) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("read without an error"),
    }
}

const V2: &str = r#"{
  "spec": "chara_card_v2", "spec_version": "2.0",
  "name": "top-level copy, ignored",
  "data": {
    "name": "Mara", "description": "Keeps a lighthouse.", "personality": "dry",
    "scenario": "A storm.", "first_mes": "You're soaked.",
    "mes_example": "<START>\n{{user}}: Hi\n{{char}}: Hm.",
    "creator_notes": "slow pace", "system_prompt": "", "post_history_instructions": null,
    "alternate_greetings": ["Back again?", "", 7],
    "tags": ["sea"], "creator": "me", "character_version": "1", "extensions": {},
    "character_book": { "extensions": {}, "entries": [
      { "keys": ["rock"], "content": "Grey Rock has no trees.", "enabled": true,
        "insertion_order": 2, "constant": true, "use_regex": true, "extensions": {} },
      { "keys": ["town"], "content": "The town is Halden.", "enabled": true,
        "insertion_order": 1, "extensions": {} },
      { "keys": ["x"], "content": "Disabled.", "enabled": false, "constant": true,
        "insertion_order": 0, "extensions": {} },
      { "keys": [], "content": "@@activate\n@@depth 4\nThe lamp is old.", "enabled": true,
        "insertion_order": 3, "extensions": {} },
      { "keys": [], "content": "@@dont_activate\nNever.", "enabled": true, "constant": true,
        "insertion_order": 4, "extensions": {} }
    ]}
  }
}"#;

#[test]
fn reads_a_v2_card_from_its_data_and_tolerates_nulls_and_stray_types() {
    let c = card_of(V2.as_bytes());
    assert_eq!(c.spec, Spec::V2);
    assert_eq!(c.container, Container::Json);
    assert_eq!(c.name, "Mara");
    assert_eq!(c.first_mes, "You're soaked.");
    assert_eq!(c.post_history_instructions, "");
    assert_eq!(c.alternate_greetings, vec!["Back again?".to_string()]);
    assert_eq!(c.creator_notes, "slow pace");
    // Disabled and `@@dont_activate` entries are gone; the rest in insertion
    // order; `constant` counts even beside `use_regex`, and `@@activate` too,
    // its decorator lines stripped.
    let book: Vec<(&str, bool)> = c
        .book
        .iter()
        .map(|e| (e.content.as_str(), e.always))
        .collect();
    assert_eq!(
        book,
        vec![
            ("The town is Halden.", false),
            ("Grey Rock has no trees.", true),
            ("The lamp is old.", true),
        ]
    );
}

#[test]
fn reads_v3_with_its_nickname_and_v1_with_its_old_spellings() {
    let v3 = r#"{"spec":"chara_card_v3","spec_version":"3.0","data":{"name":"Margaret",
        "nickname":"Mara","description":"d","first_mes":"hi","group_only_greetings":[]}}"#;
    let c = card_of(v3.as_bytes());
    assert_eq!((c.spec, c.char_name()), (Spec::V3, "Mara"));

    // A later edition is read as V3 rather than refused.
    let v4 = r#"{"spec":"chara_card_v4","data":{"name":"N","description":"d"}}"#;
    assert_eq!(card_of(v4.as_bytes()).spec, Spec::V3);

    let v1 = r#"{"char_name":"Old","char_persona":"<BOT> likes <user>.",
        "world_scenario":"s","char_greeting":"Hello, <USER>","example_dialogue":"<START>\n<user>: a"}"#;
    let c = card_of(v1.as_bytes());
    assert_eq!(c.spec, Spec::V1);
    assert_eq!(c.name, "Old");
    assert_eq!(c.description, "{{char}} likes {{user}}.");
    assert_eq!(c.first_mes, "Hello, {{user}}");
    assert_eq!(c.scenario, "s");
    assert_eq!(c.mes_example, "<START>\n{{user}}: a");
}

/// What is not a card is handed back for the neutral format to judge — a
/// mindfork-import file, a lorebook, any other JSON — and a card that says it
/// is one but holds no data or no name is an error of its own.
#[test]
fn tells_a_card_from_other_json() {
    for other in [
        r#"{"format":"mindfork-import","version":1,"profiles":[]}"#,
        r#"{"spec":"lorebook_v3","data":{"entries":[]}}"#,
        r#"{"name":"just a name"}"#,
        "[1,2]",
        "not json",
    ] {
        assert!(
            matches!(sniff(other.as_bytes(), en()).unwrap(), Sniffed::NotCard),
            "{other}"
        );
    }
    assert!(err_of(br#"{"spec":"chara_card_v2"}"#).contains("data"));
    assert!(err_of(br#"{"spec":"chara_card_v2","data":{"name":" "}}"#).contains("name"));
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .encode(s)
        .into_bytes()
}

#[test]
fn reads_a_png_preferring_ccv3_and_falling_back_to_chara() {
    let v2 = r#"{"spec":"chara_card_v2","data":{"name":"From chara","description":"d"}}"#;
    let v3 = r#"{"spec":"chara_card_v3","data":{"name":"From ccv3","description":"d"}}"#;
    let both = png::fixture(&[
        (b"tEXt", png::text("chara", &b64(v2))),
        (b"tEXt", png::text("ccv3", &b64(v3))),
    ]);
    let c = card_of(&both);
    assert_eq!(
        (c.name.as_str(), c.container),
        ("From ccv3", Container::Png("ccv3"))
    );

    // A broken `ccv3` does not fail a card whose V2 copy is whole.
    let broken = png::fixture(&[
        (b"tEXt", png::text("ccv3", b"!!!not base64 or json")),
        (b"tEXt", png::text("chara", &b64(v2))),
    ]);
    assert_eq!(card_of(&broken).container, Container::Png("chara"));

    // Raw JSON in the chunk, from writers that skip the base64.
    let raw = png::fixture(&[(b"tEXt", png::text("chara", v2.as_bytes()))]);
    assert_eq!(card_of(&raw).name, "From chara");
}

#[test]
fn a_png_without_a_card_says_it_is_a_picture() {
    let pic = png::fixture(&[(b"tEXt", png::text("Comment", b"made in paint"))]);
    assert!(
        err_of(&pic).contains("no character card"),
        "{}",
        err_of(&pic)
    );
    let junk = png::fixture(&[(b"tEXt", png::text("chara", b"@@@"))]);
    assert!(
        err_of(&junk).contains("cannot be read"),
        "{}",
        err_of(&junk)
    );
}

fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn reads_a_charx_and_points_other_zips_at_their_command() {
    let v3 = r#"{"spec":"chara_card_v3","data":{"name":"Zipped","description":"d"}}"#;
    let charx = zip_of(&[("card.json", v3), ("assets/icon/images/main.png", "png")]);
    let c = card_of(&charx);
    assert_eq!((c.name.as_str(), c.container), ("Zipped", Container::Charx));

    let companion = zip_of(&[("companion.json", "{}"), ("profile.json", "{}")]);
    assert!(err_of(&companion).contains("mindfork companion import"));
    let backup = zip_of(&[("manifest.json", "{}")]);
    assert!(err_of(&backup).contains("mindfork restore"));
    let other = zip_of(&[("readme-mf.txt", "x")]);
    assert!(err_of(&other).contains("card.json"));
    let not_card = zip_of(&[("card.json", r#"{"hello":1}"#)]);
    assert!(err_of(&not_card).contains("not a character card"));
}

#[test]
fn a_bom_and_crlf_are_tolerated() {
    let v2 = "\u{feff}{\"spec\":\"chara_card_v2\",\"data\":{\"name\":\"M\",\"description\":\"a\\r\\nb\"}}";
    assert_eq!(card_of(v2.as_bytes()).description, "a\nb");
}
