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
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), no_auto_cfg());
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

/// The automatic trigger end-to-end with the **default** config (spec §11.2):
/// the first reply titles the chat with no command from anyone, and the second
/// exchange does not re-title — the first fire is the positive control that
/// makes the absence assertion meaningful (docs/lessons.md §2).
#[tokio::test]
async fn first_reply_titles_the_chat_automatically() {
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("ответ".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        // Serves the automatic title task — and, being the last script, every
        // later request too (`sequence` repeats its final entry).
        vec![
            ChatChunk::Text("«Планы на дачу»".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    // No AutoRenameChat was sent — the rename arrives on its own.
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    match renamed {
        AppEvent::ChatRenamed { title, .. } => assert_eq!(title, "Планы на дачу"),
        _ => unreachable!(),
    }

    // The second exchange must not re-title: the conversation already has its
    // first reply. Drain to the channel's end (after Quit) so a late rename
    // cannot hide behind the assertion.
    cmd_tx
        .send(AppCommand::SendMessage("ещё вопрос".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let mut late_renames = 0;
    while let Ok(ev) = evt_rx.try_recv() {
        if matches!(ev, AppEvent::ChatRenamed { .. }) {
            late_renames += 1;
        }
    }
    assert_eq!(late_renames, 0, "the second exchange must not re-title");
}

/// The `AfterUserMessage` timing: the title task starts on send, without
/// waiting for the reply.
#[tokio::test]
async fn after_user_mode_titles_on_send() {
    // One script served to every request (`scripted` repeats): both the reply
    // and the racing title task read the same text, so the assertion does not
    // depend on which of the two concurrent requests lands first.
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("Дачный сезон".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let mut cfg = AppConfig::default();
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::AfterUserMessage;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), cfg);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    match renamed {
        AppEvent::ChatRenamed { title, .. } => assert_eq!(title, "Дачный сезон"),
        _ => unreachable!(),
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// Regenerating the first reply re-fires the trigger (design D2): the
/// truncation removed the conversation's only reply, so the next one is again
/// the first — and the title follows what the exchange actually became.
#[tokio::test]
async fn regenerating_the_first_reply_retitles() {
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("ответ №1".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        vec![
            ChatChunk::Text("«Первое имя»".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        vec![
            ChatChunk::Text("ответ №2".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        vec![
            ChatChunk::Text("«Второе имя»".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    // Wait the first title out before regenerating, so the two title tasks
    // cannot race each other for scripts.
    let first = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    match first {
        AppEvent::ChatRenamed { title, .. } => assert_eq!(title, "Первое имя"),
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    let second = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    match second {
        AppEvent::ChatRenamed { title, .. } => assert_eq!(title, "Второе имя"),
        _ => unreachable!(),
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// A manual rename wins over the automatic path at both ends (spec §11.2, D1):
/// the flag it sets blocks a later trigger, and an automatic result arriving
/// *after* the rename is dropped — while a requested one still applies, which
/// is the positive control for both absences.
#[test]
fn manual_rename_outranks_the_automatic_title() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "Новый чат");
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуйте"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);

    orch.handle_rename(chat_id, "Моё имя".into());
    assert!(
        orch.chats[0].renamed_manually,
        "a manual rename must set the flag"
    );
    while rx.try_recv().is_ok() {} // drop the rename's own events

    // An automatic result that lost the race to the rename: dropped silently.
    orch.handle_title_result(TitleResult {
        chat_id,
        text: Ok("«Модельное имя»".into()),
        origin: TitleOrigin::Auto,
    });
    assert_eq!(orch.chats[0].title, "Моё имя");
    assert!(rx.try_recv().is_err(), "an automatic result must be silent");

    // The requested path (the chat-list action) still applies: the user asked
    // for this title moments ago, so last write wins.
    orch.handle_title_result(TitleResult {
        chat_id,
        text: Ok("«Модельное имя»".into()),
        origin: TitleOrigin::Requested,
    });
    assert_eq!(orch.chats[0].title, "Модельное имя");
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::ChatList(_) | AppEvent::ChatRenamed { .. }
    ));
}

/// Failures are reported where their origin belongs (design D4): an automatic
/// run logs and stays out of the UI, a requested one lands in the chat-list
/// overlay — the loud arm proving the quiet arm's silence is deliberate.
#[test]
fn automatic_title_failures_are_quiet_requested_ones_are_loud() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "Новый чат"); // empty: no digest
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);

    // Trigger-side: an empty digest on the automatic path says nothing.
    orch.maybe_auto_title(
        chat_id,
        crate::shared::config::AutoTitleMode::AfterAssistantReply,
    );
    assert!(
        rx.try_recv().is_err(),
        "the automatic path must not emit UI events"
    );
    // Result-side: an error outcome on the automatic path says nothing either.
    orch.handle_title_result(TitleResult {
        chat_id,
        text: Err("engine exploded".into()),
        origin: TitleOrigin::Auto,
    });
    assert!(
        rx.try_recv().is_err(),
        "an automatic failure must be silent"
    );

    // The requested path reports both the same conditions.
    orch.handle_auto_rename(chat_id);
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::ChatListError(_)));
    orch.handle_title_result(TitleResult {
        chat_id,
        text: Err("engine exploded".into()),
        origin: TitleOrigin::Requested,
    });
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::ChatListError(_)));
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
