//! Orchestrator tests — the chat list, drafts, bootstrap, restoring the active chat. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

#[test]
fn new_chat_value_uses_chosen_profile_with_greeting() {
    let (_d, mut orch) = bare_orch();
    let p1 = Profile::new("A", "sys A");
    let mut p2 = Profile::new("B", "sys B");
    p2.greeting = Some("Здравствуйте!".into());
    let (id1, id2) = (p1.id, p2.id);
    orch.profiles.push(p1);
    orch.profiles.push(p2);

    let chat = orch.new_chat_value(Some(id2));
    assert_eq!(chat.profile_id, id2);
    assert_eq!(chat.system_message, "sys B");
    assert_eq!(chat.messages.len(), 1);
    assert_eq!(chat.messages[0].role, MessageRole::Assistant);
    assert_eq!(chat.messages[0].text, "Здравствуйте!");

    // None → the first profile, no greeting.
    let chat = orch.new_chat_value(None);
    assert_eq!(chat.profile_id, id1);
    assert!(chat.messages.is_empty());
}

#[test]
fn copy_chat_emits_clipboard_text_or_error_when_empty() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");

    // A chat with a conversation → a CopyToClipboard event with role-labeled text.
    let mut chat = Chat::from_profile(&profile, "Чат");
    let id = chat.id;
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуйте"));
    orch.chats.push(chat);
    orch.handle_copy_chat(id);
    match rx.try_recv().unwrap() {
        AppEvent::CopyToClipboard(text) => {
            assert!(text.contains("Пользователь:\nпривет"));
            assert!(text.contains("Ассистент:\nздравствуйте"));
        }
        other => panic!("expected CopyToClipboard, got {other:?}"),
    }

    // An empty chat (no messages) → a list error, not text.
    let empty = Chat::from_profile(&profile, "Пустой");
    let empty_id = empty.id;
    orch.chats.push(empty);
    orch.handle_copy_chat(empty_id);
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::ChatListError(_)));
}

/// `F5` labels the roles with the chat profile's custom names (spec §5.1) — resolved
/// from the profile, so a rename applies to old chats too.
#[test]
fn copy_chat_labels_roles_with_profile_names() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let mut profile = Profile::new("P", "sys");
    profile.character_names.user = "Гайя".into();
    profile.character_names.assistant = "Анна".into();
    let mut chat = Chat::from_profile(&profile, "Чат");
    let id = chat.id;
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуйте"));
    orch.profiles.push(profile);
    orch.chats.push(chat);

    orch.handle_copy_chat(id);
    match rx.try_recv().unwrap() {
        AppEvent::CopyToClipboard(text) => {
            assert!(text.contains("Гайя:\nпривет"), "{text}");
            assert!(text.contains("Анна:\nздравствуйте"), "{text}");
        }
        other => panic!("expected CopyToClipboard, got {other:?}"),
    }
}

#[test]
fn set_draft_persists_to_active_chat_without_bumping_modified() {
    let (_d, mut orch, _rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "Чат");
    let id = chat.id;
    let modified = chat.modified_at;
    orch.chats.push(chat);
    orch.active_id = Some(id);

    orch.handle_set_draft("недописанный текст".into());
    let c = orch.chats.iter().find(|c| c.id == id).unwrap();
    assert_eq!(c.draft, "недописанный текст");
    assert_eq!(
        c.modified_at, modified,
        "editing a draft doesn't bump the chat up the list"
    );
    assert!(orch.saves.is_dirty(id), "the chat is flagged for saving");

    // Setting the same text again — no re-flagging (a no-op).
    orch.saves.take();
    orch.handle_set_draft("недописанный текст".into());
    assert!(!orch.saves.is_dirty(id));
}

#[tokio::test]
async fn draft_persists_and_clears_on_send() {
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("ответ".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();

    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    // The draft is saved into the chat file.
    cmd_tx
        .send(AppCommand::SetDraft("недописанное".into()))
        .unwrap();
    // Sending clears the draft.
    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.draft, "", "the draft is cleared after sending");
}

#[tokio::test]
async fn draft_survives_reopen_when_not_sent() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();

    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    cmd_tx
        .send(AppCommand::SetDraft("черновик на потом".into()))
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.draft, "черновик на потом");
}

/// `OpenChatAt` goes through the ordinary activation path and carries the
/// message to put the feed on; every other activation carries `None`.
/// See docs/history/chat-search-stage2.md §3.
#[tokio::test]
async fn open_chat_at_activates_the_chat_carrying_the_focus() {
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("ответ".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));

    let activated = |e: &AppEvent| matches!(e, AppEvent::ChatActivated { .. });
    let a = wait_for(&mut evt_rx, activated).await.unwrap();
    let first_id = match a {
        AppEvent::ChatActivated { id, focus, .. } => {
            assert_eq!(focus, None, "bootstrap activation carries no focus");
            id
        }
        _ => unreachable!(),
    };

    // Give the chat something to jump to.
    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    // A second chat, so the jump has to activate a chat rather than only move
    // the feed of the open one.
    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    let a = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id != first_id),
    )
    .await
    .unwrap();
    let second_id = match a {
        AppEvent::ChatActivated { id, focus, .. } => {
            assert_eq!(focus, None, "creating a chat carries no focus");
            id
        }
        _ => unreachable!(),
    };

    // A plain switch — still no focus; and it hands us a message id to aim at.
    cmd_tx.send(AppCommand::SwitchChat(first_id)).unwrap();
    let a = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == first_id),
    )
    .await
    .unwrap();
    let msg_id = match a {
        AppEvent::ChatActivated {
            messages, focus, ..
        } => {
            assert_eq!(focus, None, "a plain switch carries no focus");
            messages.first().expect("the chat has messages").id
        }
        _ => unreachable!(),
    };

    // The cross-chat jump: from another chat, straight onto the message.
    cmd_tx.send(AppCommand::SwitchChat(second_id)).unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == second_id),
    )
    .await
    .unwrap();
    // The query rides along with the message: the feed highlights it inside the
    // focused bubble (fork S3(b)).
    let jumped = |e: AppEvent, what: &str| {
        match e {
            AppEvent::ChatActivated { id, focus, .. } => {
                assert_eq!(id, first_id, "{what}");
                let focus = focus.expect(what);
                assert_eq!(focus.message, msg_id, "{what}");
                assert_eq!(focus.query, "привет", "the query must ride along: {what}");
            }
            _ => unreachable!(),
        };
    };
    cmd_tx
        .send(AppCommand::OpenChatAt {
            chat: first_id,
            message: msg_id,
            query: "привет".into(),
        })
        .unwrap();
    let a = wait_for(&mut evt_rx, activated).await.unwrap();
    jumped(
        a,
        "a jump must activate the chat and carry the focused message",
    );

    // And onto the already-open chat: a plain switch would be a no-op, a jump
    // still has to move the feed — so it re-emits.
    cmd_tx
        .send(AppCommand::OpenChatAt {
            chat: first_id,
            message: msg_id,
            query: "привет".into(),
        })
        .unwrap();
    let a = wait_for(&mut evt_rx, activated).await.unwrap();
    jumped(a, "a jump within the open chat must re-emit with the focus");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn bootstrap_emits_chat_list_and_active_chat() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);

    let list = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();
    if let AppEvent::ChatList(chats) = list {
        assert_eq!(chats.len(), 1, "one default chat should be created");
    }
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    assert!(matches!(active, AppEvent::ChatActivated { .. }));

    drop(cmd_tx);
    handle.await.unwrap();
}

/// The last-opened chat is remembered in settings and restored on the
/// next launch — even if another chat was modified later (a plain fallback would
/// pick the most recent one).
#[tokio::test]
async fn remembers_and_restores_last_opened_chat() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Run 1: bootstrap creates the default chat, we add a second one (it becomes
    // active and modified later), then switch back to the first.
    let first_id;
    {
        let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, mut evt_rx) = unbounded_channel();
        let deps = OrchestratorDeps {
            cmd_rx,
            evt_tx,
            storage,
            config: AppConfig::default(),
            supervisor: Arc::new(MockSupervisor::with_backend(None)),
            default_language: crate::shared::i18n::Lang::default(),
        };
        let handle = tokio::spawn(run(deps));

        let a = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        first_id = match a {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        cmd_tx
            .send(AppCommand::NewChat { profile_id: None })
            .unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id != first_id),
        )
        .await
        .unwrap();

        cmd_tx.send(AppCommand::SwitchChat(first_id)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == first_id),
        )
        .await
        .unwrap();

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
    }

    // The settings on disk remember the first chat.
    let persisted = Storage::open(Paths::with_root(&root)).unwrap();
    let config = persisted.json().load_config().unwrap();
    assert_eq!(config.last_active_chat, Some(first_id));

    // Run 2 on the same data (the config is loaded from disk, as in main.rs):
    // exactly the first chat is restored.
    {
        let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, mut evt_rx) = unbounded_channel();
        let deps = OrchestratorDeps {
            cmd_rx,
            evt_tx,
            storage,
            config,
            supervisor: Arc::new(MockSupervisor::with_backend(None)),
            default_language: crate::shared::i18n::Lang::default(),
        };
        let handle = tokio::spawn(run(deps));

        let a = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        match a {
            AppEvent::ChatActivated { id, .. } => assert_eq!(id, first_id),
            _ => unreachable!(),
        }

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn new_chat_adds_to_list_and_activates() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 2),
    )
    .await
    .unwrap();
    assert!(matches!(list, AppEvent::ChatList(c) if c.len() == 2));

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn rename_updates_chat_list() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    cmd_tx
        .send(AppCommand::RenameChat {
            id,
            title: "Переименован".into(),
        })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.iter().any(|s| s.title == "Переименован")),
    )
    .await
    .unwrap();
    assert!(matches!(list, AppEvent::ChatList(_)));

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn delete_active_chat_creates_replacement() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    cmd_tx.send(AppCommand::DeleteChat(id)).unwrap();
    // After deleting the only chat, a new one is created and activated.
    let active2 = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id: nid, .. } if *nid != id),
    )
    .await
    .unwrap();
    assert!(matches!(active2, AppEvent::ChatActivated { .. }));

    drop(cmd_tx);
    handle.await.unwrap();
}
