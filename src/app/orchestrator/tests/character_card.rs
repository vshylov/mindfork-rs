//! Orchestrator tests — a character card's profile at work (spec §10,
//! docs/research/companion-platform.md C6): the macros in its persona and its
//! greetings, and `/greeting` moving between them. Part of the [`super`] module
//! (fixtures in mod.rs).

use super::*;

fn card_profile() -> Profile {
    let mut p = Profile::new(
        "Mara of Grey Rock",
        "You are {{char}}; {{user}} has rowed out to the lighthouse.{{// a note for the person}} Weather: {{random:fog,rain}}.",
    );
    p.language = crate::shared::i18n::Lang::En;
    p.character_names.assistant = "Mara".into();
    p.greeting = Some("*{{char}} opens the door.* You're soaked, {{user}}.".into());
    p.alternate_greetings = vec!["Back again?".into(), "Roll: {{roll:1}}".into()];
    p
}

/// A new chat keeps its persona's macros — they are substituted when a request
/// is built; the greeting is placed with everything substituted, as a written
/// message.
#[test]
fn a_new_chat_draws_the_random_macros_and_names_the_greeting() {
    let (_d, mut orch) = bare_orch();
    let p = card_profile();
    let id = p.id;
    orch.profiles.push(p);
    let chat = orch.new_chat_value(Some(id));
    let s = &chat.system_message;
    assert_eq!(s, &card_profile().default_system_message);
    assert_eq!(chat.messages.len(), 1);
    assert_eq!(
        chat.messages[0].text,
        "*Mara opens the door.* You're soaked, User."
    );
}

/// What reaches the model: the persona with the names substituted, the comment
/// gone and the random macro drawn — the names the profile has *now*, so a
/// rename applies to a chat made before it, and the same draw on every request,
/// so the prompt's prefix holds.
#[tokio::test]
async fn the_request_carries_the_profiles_names_now() {
    let dir = tempfile::tempdir().unwrap();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(dir.path()));
    let mut profile = card_profile();
    profile.character_names.user = "Alex".into();
    json.upsert_profile(&profile).unwrap();
    let chat = Chat::from_profile(&profile, "storm");
    let chat_id = chat.id;
    json.save_chat(&chat).unwrap();

    let backend = CapturingBackend::new();
    let (cmd_tx, mut evt_rx, _h) = spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
    let send = |text: &str| AppCommand::SendMessage(text.into());
    cmd_tx.send(send("hello")).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. })).await;
    let system = backend.last_request().system.unwrap();
    assert!(
        system.starts_with("You are Mara; Alex has rowed out to the lighthouse. Weather: "),
        "{system}"
    );
    assert!(!system.contains("{{"), "{system}");
    let weather = system.rsplit("Weather: ").next().unwrap().to_string();
    assert!(weather == "fog." || weather == "rain.", "{system}");

    // Renamed: the same chat's next request says the new names.
    let edit = crate::features::profiles::ProfileEdit {
        character_names: Some(crate::entities::profile::CharacterNames {
            user: "Sam".into(),
            assistant: "Marta".into(),
            system: String::new(),
        }),
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile.id,
            edit: Box::new(edit),
        })
        .unwrap();
    cmd_tx.send(send("again")).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. })).await;
    let system = backend.last_request().system.unwrap();
    assert!(
        system.starts_with("You are Marta; Sam has rowed out"),
        "{system}"
    );
    assert!(system.ends_with(&weather), "the same draw: {system}");
}

fn notice(rx: &mut UnboundedReceiver<AppEvent>) -> String {
    let mut last = None;
    while let Ok(e) = rx.try_recv() {
        if let AppEvent::Notice(n) = e {
            last = Some(n);
        }
    }
    last.expect("a notice")
}

/// `/greeting` walks the profile's greetings in order and wraps; `/greeting N`
/// picks one; a number past the end, a chat already begun and a profile with
/// one greeting each say why nothing changed.
#[test]
fn greeting_moves_between_the_profiles_greetings() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    let p = card_profile();
    let pid = p.id;
    orch.profiles.push(p);
    let chat = orch.new_chat_value(Some(pid));
    let id = chat.id;
    orch.chats.push(chat);
    orch.active_id = Some(id);
    let shown = |orch: &Orchestrator| {
        let chat = orch.chats.iter().find(|c| c.id == id).unwrap();
        assert_eq!(chat.messages.len(), 1);
        chat.messages[0].text.clone()
    };

    orch.handle_swap_greeting(None);
    assert_eq!(shown(&orch), "Back again?");
    assert_eq!(notice(&mut rx), "Greeting 2 of 3.");
    orch.handle_swap_greeting(None);
    assert_eq!(shown(&orch), "Roll: 1");
    orch.handle_swap_greeting(None);
    assert_eq!(shown(&orch), "*Mara opens the door.* You're soaked, User.");
    orch.handle_swap_greeting(Some(3));
    assert_eq!(shown(&orch), "Roll: 1");

    orch.handle_swap_greeting(Some(4));
    assert_eq!(
        notice(&mut rx),
        "There is no greeting 4: this profile has 3."
    );
    assert_eq!(shown(&orch), "Roll: 1");

    orch.chats[0].push_message(Message::user("hi"));
    orch.handle_swap_greeting(None);
    assert!(notice(&mut rx).contains("has begun"));

    // One greeting: nothing to move to.
    let mut single = card_profile();
    single.alternate_greetings.clear();
    let sid = single.id;
    orch.profiles.push(single);
    let chat = orch.new_chat_value(Some(sid));
    orch.active_id = Some(chat.id);
    orch.chats.push(chat);
    orch.handle_swap_greeting(None);
    assert!(notice(&mut rx).contains("only one greeting"));
}

/// After a restart nothing remembers which greeting `/greeting` placed: the
/// one the chat shows is recognised by its text, and the next follows it.
#[test]
fn greeting_recognises_the_one_shown_by_its_text() {
    let (_d, mut orch) = bare_orch();
    let p = card_profile();
    let pid = p.id;
    orch.profiles.push(p);
    let mut chat = orch.new_chat_value(Some(pid));
    chat.messages[0].text = "Back again?".into();
    let id = chat.id;
    orch.chats.push(chat);
    orch.active_id = Some(id);
    orch.handle_swap_greeting(None);
    assert_eq!(orch.chats[0].messages[0].text, "Roll: 1");
}

/// What the background tasks read as the persona (a reflection, the notes'
/// consolidation, the self-model's "sleep") is what a turn sends: names
/// substituted, the chat's own draw — never a raw `{{char}}`.
#[test]
fn the_background_tasks_read_the_persona_substituted() {
    let mut profile = card_profile();
    profile.character_names.user = "Alex".into();
    let chat = Chat::from_profile(&profile, "storm");
    let text = crate::app::orchestrator::persona_text(&profile, &chat);
    assert!(
        text.starts_with("You are Mara; Alex has rowed out to the lighthouse. Weather: "),
        "{text}"
    );
    assert!(!text.contains("{{"), "{text}");
    assert_eq!(
        text,
        crate::app::orchestrator::persona_text(&profile, &chat)
    );
}
