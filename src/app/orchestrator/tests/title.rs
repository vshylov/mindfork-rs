//! Orchestrator tests — auto-titling a chat. Part of the [`super`] module
//! (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

#[tokio::test]
async fn auto_rename_sets_title_from_model() {
    // The first request (send) → a "reply"; the second (auto-title) → a title.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("ответ".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        vec![
            ChatChunk::Text("«Тема разговора»".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    // Need at least one reply, otherwise there's nothing to title.
    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::AutoRenameChat(chat_id)).unwrap();
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    match renamed {
        AppEvent::ChatRenamed { id, title } => {
            assert_eq!(id, chat_id);
            assert_eq!(title, "Тема разговора", "the model's quotes are stripped");
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[test]
fn salvage_prefers_text_else_last_thought_line() {
    // There's a primary reply — take it.
    assert_eq!(
        salvage_title_source("Заголовок".into(), "мысли".into()),
        "Заголовок"
    );
    // The reply is empty — salvage the last substantive line of the reasoning.
    assert_eq!(
        salvage_title_source("  ".into(), "рассуждаю\nитог: Планы\n\n".into()),
        "итог: Планы"
    );
    // Entirely empty — an empty string (clean_generated_title returns None → an error).
    assert_eq!(salvage_title_source(String::new(), String::new()), "");
}

#[test]
fn auto_rename_without_messages_emits_error() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "Новый чат"); // no messages
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);

    orch.handle_auto_rename(chat_id);
    // An empty chat → an error into the chat-list area, the background task doesn't start.
    let ev = rx.try_recv().unwrap();
    assert!(matches!(ev, AppEvent::ChatListError(_)));
}

#[test]
fn auto_rename_when_server_not_ready_errors_into_chat_list() {
    // The server is still connecting: the readiness error must go into the chat-list
    // overlay (`ChatListError`), not the chat feed (`Error`) — otherwise the
    // full-screen list overlay would hide it.
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.engines.server_status = ServerStatus::Connecting;
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "Новый чат");
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуйте"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);

    orch.handle_auto_rename(chat_id);
    let ev = rx.try_recv().unwrap();
    assert!(
        matches!(ev, AppEvent::ChatListError(_)),
        "a not-ready error during auto-titling must go into the chat list, got: {ev:?}"
    );
}
