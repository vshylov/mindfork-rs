//! Тесты оркестратора — авто-название чата. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/history/refactoring-god-objects.md, этап 3.

use super::*;

#[tokio::test]
async fn auto_rename_sets_title_from_model() {
    // Первый запрос (отправка) → «ответ»; второй (авто-название) → заголовок.
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

    // Нужна хотя бы одна реплика, иначе нечего озаглавливать.
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
            assert_eq!(title, "Тема разговора", "кавычки модели сняты");
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[test]
fn salvage_prefers_text_else_last_thought_line() {
    // Есть основной ответ — берём его.
    assert_eq!(
        salvage_title_source("Заголовок".into(), "мысли".into()),
        "Заголовок"
    );
    // Ответ пуст — спасаем последнюю содержательную строку рассуждений.
    assert_eq!(
        salvage_title_source("  ".into(), "рассуждаю\nитог: Планы\n\n".into()),
        "итог: Планы"
    );
    // Совсем пусто — пустая строка (clean_generated_title вернёт None → ошибка).
    assert_eq!(salvage_title_source(String::new(), String::new()), "");
}

#[test]
fn auto_rename_without_messages_emits_error() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "Новый чат"); // без сообщений
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);

    orch.handle_auto_rename(chat_id);
    // Пустой чат → ошибка в область списка чатов, фоновая задача не запускается.
    let ev = rx.try_recv().unwrap();
    assert!(matches!(ev, AppEvent::ChatListError(_)));
}

#[test]
fn auto_rename_when_server_not_ready_errors_into_chat_list() {
    // Сервер ещё подключается: ошибка готовности должна идти в оверлей списка
    // чатов (`ChatListError`), а не в ленту чата (`Error`) — иначе её скрыл бы
    // полноэкранный оверлей списка.
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
        "ошибка неготовности при авто-названии должна идти в список чатов, было: {ev:?}"
    );
}
