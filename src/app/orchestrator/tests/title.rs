//! Orchestrator tests — auto-titling a chat. Part of the [`super`] module
//! (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

/// One scripted engine reply: stream `text`, then finish. Every e2e test here
/// scripts a few of these, and the four-line `vec![Text, Finished]` blocks
/// were sliding duplicates of each other and of the impersonation suite's —
/// hoisting the plumbing keeps each script to one line (docs/lessons.md §2).
fn script(text: &str) -> Vec<ChatChunk> {
    vec![
        ChatChunk::Text(text.into()),
        ChatChunk::Finished(FinishReason::Stop),
    ]
}

/// Spawns the orchestrator over a `MockBackend::sequence` of `scripts` and
/// waits out the initial activation — the shared opening of every e2e test in
/// this file: a fixture, not a test (docs/lessons.md §2). `sequence` repeats
/// its **last** entry once the rest are consumed, so a one-entry list serves
/// every request the same script.
async fn orch_with_scripts(
    scripts: Vec<Vec<ChatChunk>>,
    cfg: AppConfig,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
    Uuid,
) {
    let backend = Arc::new(MockBackend::sequence(scripts)) as Arc<dyn EngineBackend>;
    let (d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), cfg);
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };
    (d, cmd_tx, evt_rx, handle, chat_id)
}

/// The next `ChatRenamed` event's payload.
async fn wait_renamed(evt_rx: &mut UnboundedReceiver<AppEvent>) -> (Uuid, String) {
    match wait_for(evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap()
    {
        AppEvent::ChatRenamed { id, title } => (id, title),
        _ => unreachable!(),
    }
}

/// A bare orchestrator holding one chat with `messages` — the shared opening
/// of the non-async tests here, under the same fixture rule.
fn bare_with_chat(
    messages: Vec<Message>,
) -> (
    tempfile::TempDir,
    Orchestrator,
    UnboundedReceiver<AppEvent>,
    Uuid,
) {
    let (d, mut orch, rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "Новый чат");
    for m in messages {
        chat.push_message(m);
    }
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    (d, orch, rx, chat_id)
}

/// A [`TitleResult`] as the background task would deliver it.
fn title_result(chat_id: Uuid, text: Result<&str, &str>, origin: TitleOrigin) -> TitleResult {
    TitleResult {
        chat_id,
        text: text.map(str::to_string).map_err(str::to_string),
        origin,
    }
}

#[tokio::test]
async fn auto_rename_sets_title_from_model() {
    // The first script answers the send; the second — the requested title.
    let (_d, cmd_tx, mut evt_rx, handle, chat_id) = orch_with_scripts(
        vec![script("ответ"), script("«Тема разговора»")],
        no_auto_cfg(),
    )
    .await;

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
    let (id, title) = wait_renamed(&mut evt_rx).await;
    assert_eq!(id, chat_id);
    assert_eq!(title, "Тема разговора", "the model's quotes are stripped");

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
    let (_d, mut orch, mut rx, chat_id) = bare_with_chat(vec![]);

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
    // The second script serves the automatic title task — and every later
    // request too, being the sequence's last entry.
    let (_d, cmd_tx, mut evt_rx, handle, chat_id) = orch_with_scripts(
        vec![script("ответ"), script("«Планы на дачу»")],
        AppConfig::default(),
    )
    .await;

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    // No AutoRenameChat was sent — the rename arrives on its own.
    let (id, title) = wait_renamed(&mut evt_rx).await;
    assert_eq!(id, chat_id);
    assert_eq!(title, "Планы на дачу");

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
    // One script served to every request: both the reply and the racing title
    // task read the same text, so the assertion does not depend on which of
    // the two concurrent requests lands first.
    let mut cfg = AppConfig::default();
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::AfterUserMessage;
    let (_d, cmd_tx, mut evt_rx, handle, chat_id) =
        orch_with_scripts(vec![script("Дачный сезон")], cfg).await;

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    let (id, title) = wait_renamed(&mut evt_rx).await;
    assert_eq!(id, chat_id);
    assert_eq!(title, "Дачный сезон");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// Regenerating the first reply re-fires the trigger (design D2): the
/// truncation removed the conversation's only reply, so the next one is again
/// the first — and the title follows what the exchange actually became.
#[tokio::test]
async fn regenerating_the_first_reply_retitles() {
    let (_d, cmd_tx, mut evt_rx, handle, _chat) = orch_with_scripts(
        vec![
            script("ответ №1"),
            script("«Первое имя»"),
            script("ответ №2"),
            script("«Второе имя»"),
        ],
        AppConfig::default(),
    )
    .await;

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    // Wait the first title out before regenerating, so the two title tasks
    // cannot race each other for scripts.
    let (_, first) = wait_renamed(&mut evt_rx).await;
    assert_eq!(first, "Первое имя");

    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    let (_, second) = wait_renamed(&mut evt_rx).await;
    assert_eq!(second, "Второе имя");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// A manual rename wins over the automatic path at both ends (spec §11.2, D1):
/// the flag it sets blocks a later trigger, and an automatic result arriving
/// *after* the rename is dropped — while a requested one still applies, which
/// is the positive control for both absences.
#[test]
fn manual_rename_outranks_the_automatic_title() {
    let (_d, mut orch, mut rx, chat_id) = bare_with_chat(vec![
        Message::user("привет"),
        Message::assistant("здравствуйте"),
    ]);

    orch.handle_rename(chat_id, "Моё имя".into());
    assert!(
        orch.chats[0].renamed_manually,
        "a manual rename must set the flag"
    );
    while rx.try_recv().is_ok() {} // drop the rename's own events

    // An automatic result that lost the race to the rename: dropped silently.
    orch.handle_title_result(title_result(
        chat_id,
        Ok("«Модельное имя»"),
        TitleOrigin::Auto,
    ));
    assert_eq!(orch.chats[0].title, "Моё имя");
    assert!(rx.try_recv().is_err(), "an automatic result must be silent");

    // The requested path (the chat-list action) still applies: the user asked
    // for this title moments ago, so last write wins.
    orch.handle_title_result(title_result(
        chat_id,
        Ok("«Модельное имя»"),
        TitleOrigin::Requested,
    ));
    assert_eq!(orch.chats[0].title, "Модельное имя");
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::ChatList(_) | AppEvent::ChatRenamed { .. }
    ));
}

/// The same rule for a sub-agent transcript (spec §11.2, D1): an automatic
/// result arriving after the transcript was renamed by hand is dropped —
/// silently, so the list is not told of a title that never landed — while a
/// requested one still applies, the positive control.
#[test]
fn manual_rename_outranks_the_automatic_title_on_a_transcript() {
    let run = crate::entities::subagent::SubagentRun::fixture("Критик", &["x", "y"]);
    let run_id = run.id;
    let mut carrier = Message::assistant("делегировал");
    carrier.tool_calls = vec![run.on_record()];
    let (_d, mut orch, mut rx, chat_id) = bare_with_chat(vec![Message::user("привет"), carrier]);

    orch.handle_rename(run_id, "Моё имя".into());
    assert!(orch.chats[0].child(run_id).unwrap().renamed_manually);
    while rx.try_recv().is_ok() {} // drop the rename's own events

    orch.handle_title_result(title_result(
        run_id,
        Ok("«Модельное имя»"),
        TitleOrigin::Auto,
    ));
    assert_eq!(orch.chats[0].child(run_id).unwrap().title, "Моё имя");
    assert!(rx.try_recv().is_err(), "an automatic result must be silent");

    orch.handle_title_result(title_result(
        run_id,
        Ok("«Модельное имя»"),
        TitleOrigin::Requested,
    ));
    assert_eq!(orch.chats[0].child(run_id).unwrap().title, "Модельное имя");
    assert!(matches!(
        rx.try_recv().unwrap(),
        AppEvent::ChatList(_) | AppEvent::ChatRenamed { .. }
    ));
    // The parent keeps its own title throughout.
    assert_eq!(orch.chats[0].title, "Новый чат");
    assert_eq!(orch.chats[0].id, chat_id);
}

/// Failures are reported where their origin belongs (design D4): an automatic
/// run logs and stays out of the UI, a requested one lands in the chat-list
/// overlay — the loud arm proving the quiet arm's silence is deliberate.
#[test]
fn automatic_title_failures_are_quiet_requested_ones_are_loud() {
    let (_d, mut orch, mut rx, chat_id) = bare_with_chat(vec![]); // empty: no digest

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
    orch.handle_title_result(title_result(
        chat_id,
        Err("engine exploded"),
        TitleOrigin::Auto,
    ));
    assert!(
        rx.try_recv().is_err(),
        "an automatic failure must be silent"
    );

    // The requested path reports both the same conditions.
    orch.handle_auto_rename(chat_id);
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::ChatListError(_)));
    orch.handle_title_result(title_result(
        chat_id,
        Err("engine exploded"),
        TitleOrigin::Requested,
    ));
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::ChatListError(_)));
}

#[test]
fn auto_rename_when_server_not_ready_errors_into_chat_list() {
    // The server is still connecting: the readiness error must go into the chat-list
    // overlay (`ChatListError`), not the chat feed (`Error`) — otherwise the
    // full-screen list overlay would hide it.
    let (_d, mut orch, mut rx, chat_id) = bare_with_chat(vec![
        Message::user("привет"),
        Message::assistant("здравствуйте"),
    ]);
    orch.engines.server_status = ServerStatus::Connecting;

    orch.handle_auto_rename(chat_id);
    let ev = rx.try_recv().unwrap();
    assert!(
        matches!(ev, AppEvent::ChatListError(_)),
        "a not-ready error during auto-titling must go into the chat list, got: {ev:?}"
    );
}
