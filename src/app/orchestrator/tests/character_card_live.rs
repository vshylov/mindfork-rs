//! A live `#[ignore]` smoke for character cards (spec §10,
//! docs/research/companion-platform.md C6): a card the person brings as a PNG
//! is read and mapped onto a profile the way `mindfork import` does it, a new
//! chat opens with its greeting, `/greeting` moves to the alternate, and a real
//! model, asked what it remembers, does not take the card's example exchanges
//! for a past it shares with the person — the reason they sit in a labelled
//! block of the system message rather than in the conversation (§3.7 there).
//!
//! `MINDFORK_ENGINE_URL=…/v1 cargo test character_card_live -- --ignored --nocapture --test-threads=1`

use super::*;

/// The probe's card (research §3.7): the example exchanges hold two things —
/// a cat called Pepper, a wedding in Porto — that never happened.
const CARD: &str = r#"{
  "spec": "chara_card_v2", "spec_version": "2.0",
  "data": {
    "name": "Mara",
    "description": "{{char}} keeps the Grey Rock lighthouse: a dry, patient woman in her fifties who talks little and notices everything.",
    "personality": "dry, patient, quietly warm",
    "scenario": "{{user}} often rows out to the lighthouse to talk.",
    "first_mes": "*{{char}} looks up from the logbook.* Kettle's on. Sit, {{user}}.",
    "alternate_greetings": ["*The lamp turns overhead.* You again. Good."],
    "mes_example": "<START>\n{{user}}: I finally adopted that grey cat. Her name is Pepper.\n{{char}}: *sets down the lamp oil.* Pepper. Good name for a cat that'll judge you.\n<START>\n{{user}}: My sister's wedding in Porto went badly. Everyone argued.\n{{char}}: Weddings are storms with better tablecloths. Did you at least dance?",
    "creator_notes": "", "system_prompt": "", "post_history_instructions": "",
    "tags": [], "creator": "", "character_version": "", "extensions": {},
    "character_book": { "extensions": {}, "entries": [
      { "keys": [], "content": "The lighthouse has 112 steps.", "enabled": true,
        "insertion_order": 0, "constant": true, "extensions": {} },
      { "keys": ["boat"], "content": "Alex's boat is called the Tern.", "enabled": true,
        "insertion_order": 1, "extensions": {} }
    ]}
  }
}"#;

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn a_card_becomes_a_companion_that_keeps_its_examples_apart_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
    let png = crate::features::character_card::png_with("chara", CARD);
    let crate::features::character_card::Sniffed::Card(card) =
        crate::features::character_card::sniff(&png, en).unwrap()
    else {
        panic!("the PNG was not read as a card");
    };
    let lang = crate::features::character_card::profile::detect_lang(&card);
    assert_eq!(lang, crate::shared::i18n::Lang::En);
    let import = crate::features::character_card::profile::to_profile(&card, lang);
    eprintln!(
        "{}\n",
        crate::features::character_card::profile::preview(&card, &import, en)
    );
    let mut profile = import.profile;
    profile.character_names.user = "Alex".into();

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    crate::shared::storage::JsonStore::new(Paths::with_root(&root))
        .upsert_profile(&profile)
        .unwrap();
    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    config.interface.language = crate::shared::i18n::Lang::En;
    let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend(Some(backend))),
        default_language: crate::shared::i18n::Lang::En,
        extra_tools: Vec::new(),
    }));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(profile.id),
        })
        .unwrap();
    let Some(AppEvent::ChatActivated { messages, .. }) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(
        messages[0].text,
        "*Mara looks up from the logbook.* Kettle's on. Sit, Alex."
    );
    cmd_tx
        .send(AppCommand::SwapGreeting { pick: None })
        .unwrap();
    let Some(AppEvent::ChatActivated { messages, .. }) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(
        messages[0].text,
        "*The lamp turns overhead.* You again. Good."
    );

    let (remembers, _) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Before we start: what do you remember about me from our earlier talks?",
    )
    .await;
    eprintln!("what it remembers:\n{remembers}\n");
    let (who, _) = run_turn_live(&cmd_tx, &mut evt_rx, "Remind me, who are you?").await;
    eprintln!("who it is:\n{who}\n");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    for reply in [&remembers, &who] {
        assert!(
            !reply.contains("{{"),
            "a macro reached the reply: {reply:?}"
        );
    }
    let lower = remembers.to_lowercase();
    for leaked in ["pepper", "porto", "wedding"] {
        assert!(
            !lower.contains(leaked),
            "an example exchange was taken for a memory ({leaked}): {remembers:?}"
        );
    }
    // In character: the keeper of a light, not an assistant. Gemma answered
    // "I keep the light. And I keep the kettle hot for when you arrive."
    let lower = who.to_lowercase();
    assert!(
        ["mara", "light", "keeper", "lamp"]
            .iter()
            .any(|w| lower.contains(w)),
        "out of character: {who:?}"
    );
    for stock in ["language model", "ai assistant", "an assistant"] {
        assert!(!lower.contains(stock), "out of character: {who:?}");
    }
}
