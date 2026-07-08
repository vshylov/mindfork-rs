//! Тесты оркестратора — список чатов, черновики, bootstrap, восстановление активного. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/history/refactoring-god-objects.md, этап 3.

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

    // None → первый профиль, без приветствия.
    let chat = orch.new_chat_value(None);
    assert_eq!(chat.profile_id, id1);
    assert!(chat.messages.is_empty());
}

#[test]
fn copy_chat_emits_clipboard_text_or_error_when_empty() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");

    // Чат с перепиской → событие CopyToClipboard с текстом ролей.
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
        other => panic!("ожидался CopyToClipboard, получено {other:?}"),
    }

    // Пустой чат (нет сообщений) → ошибка списка, не текст.
    let empty = Chat::from_profile(&profile, "Пустой");
    let empty_id = empty.id;
    orch.chats.push(empty);
    orch.handle_copy_chat(empty_id);
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::ChatListError(_)));
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
        "правка черновика не поднимает чат в списке"
    );
    assert!(orch.saves.is_dirty(id), "чат помечен для сохранения");

    // Повторная установка того же текста — без повторной пометки (no-op).
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

    // Черновик сохраняется в файле чата.
    cmd_tx
        .send(AppCommand::SetDraft("недописанное".into()))
        .unwrap();
    // Отправка очищает черновик.
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
    assert_eq!(chat.draft, "", "после отправки черновик очищен");
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

#[tokio::test]
async fn bootstrap_emits_chat_list_and_active_chat() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);

    let list = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();
    if let AppEvent::ChatList(chats) = list {
        assert_eq!(chats.len(), 1, "должен создаться один чат по умолчанию");
    }
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    assert!(matches!(active, AppEvent::ChatActivated { .. }));

    drop(cmd_tx);
    handle.await.unwrap();
}

/// Последний открытый чат запоминается в настройках и восстанавливается при
/// следующем запуске — даже если другой чат изменён позже (обычный fallback выбрал
/// бы самый недавний).
#[tokio::test]
async fn remembers_and_restores_last_opened_chat() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Запуск 1: bootstrap создаёт дефолтный чат, добавляем второй (он становится
    // активным и изменённым позже), затем переключаемся обратно на первый.
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

    // Настройки на диске помнят первый чат.
    let persisted = Storage::open(Paths::with_root(&root)).unwrap();
    let config = persisted.json().load_config().unwrap();
    assert_eq!(config.last_active_chat, Some(first_id));

    // Запуск 2 на тех же данных (конфиг загружается с диска, как в main.rs):
    // восстанавливается именно первый чат.
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
    // После удаления единственного чата создаётся новый и активируется.
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
