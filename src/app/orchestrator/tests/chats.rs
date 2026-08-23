//! Orchestrator tests — the chat list, drafts, bootstrap, restoring the active chat. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::features::export_command::ExportFormat;

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

#[test]
fn set_feed_view_persists_to_active_chat_without_bumping_modified() {
    // The same rules as the draft above: the collapse state is stored on the
    // chat, flagged for saving, and does NOT bump it up the list — folding a
    // block away isn't a change to the conversation (spec §11.3).
    let (_d, mut orch, _rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "Чат");
    let id = chat.id;
    let modified = chat.modified_at;
    orch.chats.push(chat);
    orch.active_id = Some(id);

    let view = FeedView {
        thoughts: true,
        tools: true,
    };
    orch.handle_set_feed_view(view);
    let c = orch.chats.iter().find(|c| c.id == id).unwrap();
    assert_eq!(c.feed_view, view);
    assert_eq!(
        c.modified_at, modified,
        "collapsing a block doesn't bump the chat up the list"
    );
    assert!(orch.saves.is_dirty(id), "the chat is flagged for saving");

    // The same state again — a no-op, no re-flagging (and so no needless write).
    orch.saves.take();
    orch.handle_set_feed_view(view);
    assert!(!orch.saves.is_dirty(id));
}

#[tokio::test]
async fn feed_view_is_per_chat_and_survives_a_reopen() {
    // The point of storing it on the chat: expanding in one chat leaves another
    // alone, and the choice is still there after a restart.
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();

    let activated = |e: &AppEvent| matches!(e, AppEvent::ChatActivated { .. });
    let first = match wait_for(&mut evt_rx, activated).await.unwrap() {
        AppEvent::ChatActivated {
            id, feed_view: v, ..
        } => {
            assert_eq!(v, FeedView::default(), "a fresh chat opens collapsed");
            id
        }
        _ => unreachable!(),
    };

    let expanded = FeedView {
        thoughts: true,
        tools: true,
    };
    cmd_tx.send(AppCommand::SetFeedView(expanded)).unwrap();

    // A second chat is unaffected — the state is per chat, not global.
    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    let second = match wait_for(&mut evt_rx, activated).await.unwrap() {
        AppEvent::ChatActivated {
            id, feed_view: v, ..
        } => {
            assert_eq!(v, FeedView::default(), "the new chat is its own");
            id
        }
        _ => unreachable!(),
    };
    assert_ne!(first, second);

    // Switching back hands the stored state to the UI.
    cmd_tx.send(AppCommand::SwitchChat(first)).unwrap();
    match wait_for(&mut evt_rx, activated).await.unwrap() {
        AppEvent::ChatActivated { feed_view: v, .. } => assert_eq!(v, expanded),
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(first).unwrap().unwrap();
    assert_eq!(chat.feed_view, expanded, "the choice survives a restart");
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

/// Chat files carried to another machine without `data.db`: the launch says so
/// once, in the feed. Without the note the situation is invisible — the chats
/// are on screen and the assistant remembers nothing about them, with no
/// explanation anywhere (docs/lessons.md §4).
///
/// Two things are asserted, and the second is the fragile one: the note must
/// arrive **after** `ChatActivated`, because activation rebuilds the feed from
/// the chat's messages and would wipe a note pushed before it.
#[tokio::test]
async fn a_missing_database_is_reported_once_in_the_feed_after_the_feed_is_built() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // A data root as a copy without `data.db` leaves it: a profile and a chat,
    // no database.
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(&root));
    let profile = Profile::new("A", "sys");
    json.upsert_profile(&profile).unwrap();
    json.save_chat(&Chat::from_profile(&profile, "перенесённый чат"))
        .unwrap();
    assert!(!Paths::with_root(&root).data_db().exists());

    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, None, AppConfig::default());

    // Everything up to `Settings`, which the loop emits right after the note.
    let mut seen: Vec<AppEvent> = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        let done = matches!(ev, AppEvent::Settings { .. });
        seen.push(ev);
        if done {
            break;
        }
    }

    let note_at = seen
        .iter()
        .position(|e| matches!(e, AppEvent::Notice(_)))
        .expect("the launch must say the database was missing");
    let activated_at = seen
        .iter()
        .position(|e| matches!(e, AppEvent::ChatActivated { .. }))
        .expect("the chat is activated at bootstrap");
    assert!(
        activated_at < note_at,
        "a note before the feed rebuild would be wiped by it: {seen:?}"
    );
    match &seen[note_at] {
        AppEvent::Notice(text) => assert_eq!(
            text,
            crate::shared::i18n::locale(crate::shared::i18n::Lang::default())
                .t("ui.startup.db_missing")
        ),
        _ => unreachable!(),
    }
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(e, AppEvent::Notice(_)))
            .count(),
        1,
        "said once, not per chat"
    );

    drop(cmd_tx);
    handle.await.unwrap();
}

/// The other half, and the one that keeps the test above from passing for the
/// wrong reason: an ordinary restart on a root that has its database says
/// nothing. Two phases on one root — the first launch creates `data.db`, the
/// second must find it and stay quiet.
#[tokio::test]
async fn an_ordinary_restart_says_nothing_about_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    {
        let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, None, AppConfig::default());
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
            .await
            .unwrap();
        drop(cmd_tx);
        handle.await.unwrap();
    }
    assert!(Paths::with_root(&root).data_db().exists());

    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, None, AppConfig::default());
    let mut seen: Vec<AppEvent> = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        let done = matches!(ev, AppEvent::Settings { .. });
        seen.push(ev);
        if done {
            break;
        }
    }
    assert!(
        !seen.iter().any(|e| matches!(e, AppEvent::Notice(_))),
        "a database that is there is not news: {seen:?}"
    );

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

// ---------- /export: writing a conversation to a file ----------

/// Seeds a bare orchestrator with one profile and a two-message chat, the way
/// the copy tests above do — synchronous, no event loop and no backend, because
/// exporting neither starts a turn nor needs one.
fn orch_with_conversation() -> (
    tempfile::TempDir,
    Orchestrator,
    UnboundedReceiver<AppEvent>,
    Uuid,
) {
    let (dir, mut orch, rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "Про космос");
    let id = chat.id;
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуйте"));
    orch.profiles.push(profile);
    orch.chats.push(chat);
    (dir, orch, rx, id)
}

/// The happy path and the rule that guards it (fork F5): the file appears with
/// the conversation in it and the note names its path — then a second export to
/// the same name is refused, and the first file is left untouched.
#[test]
fn export_writes_the_file_and_refuses_to_overwrite_it() {
    let (dir, mut orch, mut rx, id) = orch_with_conversation();
    let target = dir.path().join("вывод.md");

    orch.handle_export_chat(
        id,
        ExportFormat::Markdown,
        Some(&target.display().to_string()),
    );
    match rx.try_recv().unwrap() {
        AppEvent::Notice(text) => assert!(text.contains("вывод.md"), "the note names it: {text}"),
        other => panic!("expected a Notice, got {other:?}"),
    }
    let written = std::fs::read_to_string(&target).expect("the file exists");
    assert!(
        written.contains("привет"),
        "the conversation is in it: {written}"
    );
    assert!(written.contains("здравствуйте"), "{written}");

    // Second time: refused, and what is on disk is still the first export.
    orch.handle_export_chat(
        id,
        ExportFormat::Markdown,
        Some(&target.display().to_string()),
    );
    match rx.try_recv().unwrap() {
        AppEvent::Error(text) => assert!(text.contains("вывод.md"), "the refusal names it: {text}"),
        other => panic!("expected an Error, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        written,
        "the existing export must survive untouched"
    );
}

/// The Markdown file is byte-for-byte what `F5` puts on the clipboard — the
/// claim fork F6 rests on, and the reason there is one formatter rather than two.
#[test]
fn the_markdown_file_is_exactly_what_the_clipboard_gets() {
    let (dir, mut orch, mut rx, id) = orch_with_conversation();
    orch.handle_copy_chat(id);
    let copied = match rx.try_recv().unwrap() {
        AppEvent::CopyToClipboard(text) => text,
        other => panic!("expected CopyToClipboard, got {other:?}"),
    };

    let target = dir.path().join("out.md");
    orch.handle_export_chat(
        id,
        ExportFormat::Markdown,
        Some(&target.display().to_string()),
    );
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::Notice(_)));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), copied);
}

/// A JSON export is a document our **own importer** accepts, coming back as the
/// same chat — and its note carries the warning about tool calls, said on every
/// such export rather than left in the documentation to be discovered later.
#[test]
fn a_json_export_imports_back_and_says_what_it_drops() {
    let (dir, mut orch, mut rx, id) = orch_with_conversation();
    let target = dir.path().join("chat.json");

    orch.handle_export_chat(id, ExportFormat::Json, Some(&target.display().to_string()));
    match rx.try_recv().unwrap() {
        AppEvent::Notice(text) => {
            assert!(text.contains("chat.json"), "the note names it: {text}");
            assert!(
                text.contains("md"),
                "the note must name the format that keeps tool calls: {text}"
            );
        }
        other => panic!("expected a Notice, got {other:?}"),
    }

    let json = std::fs::read_to_string(&target).unwrap();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let result = crate::features::import::parse_import(&json, loc)
        .expect("the export is a valid import document");
    assert_eq!(result.chats.len(), 1);
    assert_eq!(result.chats[0].id, id, "it comes back as the same chat");
    assert_eq!(result.chats[0].title, "Про космос");
}

/// An empty conversation is refused rather than written: a file holding a title
/// and nothing else is not what anyone meant by "export".
#[test]
fn exporting_an_empty_chat_is_refused() {
    let (dir, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let empty = Chat::from_profile(&profile, "Пустой");
    let id = empty.id;
    orch.profiles.push(profile);
    orch.chats.push(empty);

    let target = dir.path().join("empty.md");
    orch.handle_export_chat(
        id,
        ExportFormat::Markdown,
        Some(&target.display().to_string()),
    );
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::Error(_)));
    assert!(!target.exists(), "nothing may be written");
}

// The bare form (`path: None`) is deliberately **not** tested here: it resolves
// the generated name against the process's current directory, and a test that
// chdir'd would be changing global state shared with every other test running in
// parallel. What it does with the name — `export_filename`, and the slug inside
// it — is covered in `features::chat_export`; what is left is `PathBuf::from` of
// that name, which is the current directory by definition.
