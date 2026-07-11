//! Тесты оркестратора — генерация, agentic-loop, отмена, гейты готовности. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/history/refactoring-god-objects.md, этап 3.

use super::*;

#[test]
fn effective_sampling_resolves_three_tiers() {
    let (_d, mut orch) = bare_orch();
    let mut profile = Profile::new("P", "sys");
    profile.default_sampling = Some(SamplingConfig {
        temperature: Some(0.5),
        ..Default::default()
    });
    let chat = Chat::from_profile(&profile, "c");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);

    // override = None → берётся дефолт профиля.
    assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.5));

    // override = Some → берётся он (приоритет чата).
    orch.chats[0].sampling_override = Some(SamplingConfig {
        temperature: Some(0.9),
        ..Default::default()
    });
    assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.9));

    // нет ни override, ни дефолта профиля → глобальный.
    orch.chats[0].sampling_override = None;
    orch.profiles[0].default_sampling = None;
    assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.1));
}

#[tokio::test]
async fn send_streams_and_persists_assistant_message() {
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Thoughts("думаю".into()),
        ChatChunk::Text("Привет".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();

    // Ждём активации и узнаём id активного чата.
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    // Завершаем и проверяем, что чат сохранён с двумя сообщениями.
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.messages.len(), 2);
    assert_eq!(chat.messages[0].role, MessageRole::User);
    assert_eq!(chat.messages[1].role, MessageRole::Assistant);
    assert_eq!(chat.messages[1].text, "Привет");
    assert_eq!(chat.messages[1].thoughts.as_deref(), Some("думаю"));
}

#[tokio::test]
async fn assistant_metadata_records_mode_model_and_filtered_sampling() {
    use crate::entities::sampling::SamplingConfig;
    use crate::shared::config::{CloudSettings, EngineSettings, ServerMode};

    // Облачный режим (OpenAI) + имя модели; глобальный семплинг с top_k, который
    // строгий облачный диалект не принимает → в снимок метаданных он попасть не должен.
    let config = AppConfig {
        engine: EngineSettings {
            mode: ServerMode::OpenAi,
            openai: CloudSettings {
                model_name: Some("gpt-test".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        default_sampling: SamplingConfig {
            temperature: Some(0.7),
            top_k: Some(40),
            max_tokens: Some(128),
            ..Default::default()
        },
        ..Default::default()
    };

    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("Привет".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    let root = _d.path().to_path_buf();

    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };
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
    let meta = chat.messages[1]
        .metadata
        .as_ref()
        .expect("снимок метаданных");
    assert_eq!(meta.mode, ServerMode::OpenAi);
    assert_eq!(meta.model.as_deref(), Some("gpt-test"));
    // Доступные в режиме поля сохранены; недоступные top_k и temperature (её
    // отвергают GPT 5.5/5.6) — обнулены.
    assert_eq!(meta.sampling.max_tokens, Some(128));
    assert_eq!(meta.sampling.top_k, None);
    assert_eq!(meta.sampling.temperature, None);
}

#[tokio::test]
async fn emits_token_counter_during_generation() {
    use crate::shared::api::contract::TokenUsage;
    // Две текстовые дельты (live-счёт = 2), затем точный usage от сервера (= 5).
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("При".into()),
        ChatChunk::Text("вет".into()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 12,
            completion_tokens: 5,
            reasoning_tokens: 0,
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));

    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();

    // Сразу после старта — оценка переписки (промпта): context=Some, неточная,
    // ответа ещё нет (completion=0).
    let est = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::TokenUsage { .. }))
        .await
        .unwrap();
    assert!(
        matches!(
            est,
            AppEvent::TokenUsage {
                completion: 0,
                context: Some(c),
                context_exact: false,
                ..
            } if c > 0
        ),
        "оценка переписки: {est:?}"
    );

    // Дельты ответа → счётчик ответа растёт, оценку переписки не трогают (None).
    let first = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::TokenUsage { context: None, .. })
    })
    .await
    .unwrap();
    assert!(
        matches!(
            first,
            AppEvent::TokenUsage {
                completion: 1,
                context: None,
                ..
            }
        ),
        "счётчик ответа: {first:?}"
    );

    // Точный счётчик из usage сервера приходит до завершения: completion=5, context=12.
    let exact = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::TokenUsage {
                context_exact: true,
                ..
            }
        )
    })
    .await
    .unwrap();
    assert!(
        matches!(
            exact,
            AppEvent::TokenUsage {
                completion: 5,
                context: Some(12),
                context_exact: true,
                ..
            }
        ),
        "точный счётчик из usage: {exact:?}"
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn regenerate_replaces_last_assistant_message() {
    // Два разных ответа по очереди: исходный ход → «первый», перегенерация → «второй».
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("первый".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        vec![
            ChatChunk::Text("второй".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
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

    cmd_tx
        .send(AppCommand::SendMessage("вопрос".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    // ChatList после Finished — признак, что handle_done применил ответ (state Idle).
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Старый ответ заменён новым; сообщение пользователя не дублируется.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.messages.len(), 2, "{:?}", chat.messages);
    assert_eq!(chat.messages[0].role, MessageRole::User);
    assert_eq!(chat.messages[0].text, "вопрос");
    assert_eq!(chat.messages[1].role, MessageRole::Assistant);
    assert_eq!(chat.messages[1].text, "второй");
}

#[tokio::test]
async fn delete_last_exchange_restores_user_text() {
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

    cmd_tx
        .send(AppCommand::SendMessage("забудь это".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::DeleteLastExchange).unwrap();
    let restore = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::RestoreInput(_)))
        .await
        .unwrap();
    assert!(matches!(restore, AppEvent::RestoreInput(t) if t == "забудь это"));

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Обмен удалён полностью (дефолтный чат без приветствия → пусто).
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert!(chat.messages.is_empty(), "{:?}", chat.messages);
    // Удалённый обмен сохранён для ручного восстановления (сообщение пользователя +
    // ответ ассистента), черновик ввода был пуст (spec §11.7).
    assert_eq!(chat.deleted.len(), 1);
    let removed = &chat.deleted[0];
    assert_eq!(removed.messages.len(), 2);
    assert_eq!(removed.messages[0].role, MessageRole::User);
    assert_eq!(removed.messages[0].text, "забудь это");
    assert_eq!(removed.messages[1].role, MessageRole::Assistant);
    assert_eq!(removed.draft, "");
}

#[tokio::test]
async fn regenerate_without_user_message_is_noop() {
    // Чат с приветствием-ассистентом, но без сообщения пользователя — нечего
    // перегенерировать; команда не должна стартовать генерацию.
    let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>;
    let (_d, mut orch) = bare_orch();
    orch.engines.backend = Some(backend);
    let mut profile = Profile::new("P", "sys");
    profile.greeting = Some("Привет!".into());
    let mut chat = Chat::from_profile(&profile, "c");
    chat.push_message(Message::assistant("Привет!"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    orch.handle_regenerate();
    // Состояние осталось Idle (генерация не запущена), история не тронута.
    assert!(orch.gen_state.is_idle());
    assert_eq!(orch.chats[0].messages.len(), 1);
}

#[test]
fn regenerate_on_not_ready_server_keeps_reply() {
    // Сервер ещё подключается (managed грузит модель) — перегенерация не должна
    // ни сносить прежний ответ, ни уходить запросом на не-готовый сервер (иначе
    // 503 «engine returned an error status» и потеря ответа). Регресс-тест.
    let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>;
    let (_d, mut orch) = bare_orch();
    orch.engines.backend = Some(backend);
    orch.engines.server_status = ServerStatus::Connecting; // ещё не готов
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "c");
    chat.push_message(Message::user("вопрос"));
    chat.push_message(Message::assistant("старый ответ"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    orch.handle_regenerate();

    // Ответ сохранён, генерация не стартовала (история не усечена).
    assert!(orch.gen_state.is_idle());
    assert_eq!(
        orch.chats[0].messages.len(),
        2,
        "прежний ответ не должен быть снесён на не-готовом сервере"
    );
    assert_eq!(orch.chats[0].messages[1].text, "старый ответ");
}

#[test]
fn send_on_not_ready_server_restores_input() {
    // Сервер ещё подключается — отправка отклоняется, но текст возвращается в
    // поле ввода (RestoreInput), а в чат сообщение не добавляется.
    let backend = Arc::new(MockBackend::scripted(vec![])) as Arc<dyn EngineBackend>;
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.engines.backend = Some(backend);
    orch.engines.server_status = ServerStatus::Connecting;
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "c");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    orch.handle_send("привет".into());

    assert!(orch.gen_state.is_idle());
    assert!(orch.chats[0].messages.is_empty(), "сообщение не добавлено");
    // Среди эмитнутых событий — ошибка и возврат текста в поле ввода.
    let mut got_error = false;
    let mut restored = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AppEvent::Error(_) => got_error = true,
            AppEvent::RestoreInput(t) => restored = Some(t),
            _ => {}
        }
    }
    assert!(got_error, "должна быть эмитнута ошибка о неготовности");
    assert_eq!(restored.as_deref(), Some("привет"));
}

#[test]
fn ready_backend_gates_by_status() {
    let (_d, mut orch) = bare_orch();
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![])) as Arc<dyn EngineBackend>);

    orch.engines.server_status = ServerStatus::Ready;
    assert!(orch.ready_backend().is_some());

    for status in [
        ServerStatus::Connecting,
        ServerStatus::NotConfigured,
        ServerStatus::Disconnected("боль".into()),
    ] {
        orch.engines.server_status = status;
        assert!(orch.ready_backend().is_none());
    }
}

#[tokio::test]
async fn cancel_stops_generation_and_saves_partial() {
    let backend = Arc::new(MockBackend::cancellable(vec![ChatChunk::Text(
        "часть".into(),
    )])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();

    let mut sent_cancel = false;
    let mut cancelled = false;
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Chunk { .. } if !sent_cancel => {
                cmd_tx.send(AppCommand::Cancel).unwrap();
                sent_cancel = true;
            }
            AppEvent::Finished { reason, .. } => {
                assert_eq!(reason, FinishReason::Cancelled);
                cancelled = true;
                break;
            }
            _ => {}
        }
    }
    assert!(cancelled);

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn agentic_loop_executes_tool_then_finalizes() {
    use crate::shared::api::contract::ToolCallDelta;
    // Раунд 1: вызов note_save → раунд 2: финальный текст.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("note_save".into()),
                arguments: "{\"content\":\"любит чай\"}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        vec![
            ChatChunk::Text("Запомнил.".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
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

    cmd_tx
        .send(AppCommand::SendMessage("запомни про чай".into()))
        .unwrap();

    // Событие исполнения инструмента доходит до UI.
    let tool_ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ToolCall { .. }))
        .await
        .unwrap();
    assert!(matches!(tool_ev, AppEvent::ToolCall { name, .. } if name == "note_save"));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // История: user → assistant(tool_calls) → tool → assistant(финал).
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.messages.len(), 4, "{:?}", chat.messages);
    assert_eq!(chat.messages[0].role, MessageRole::User);
    assert_eq!(chat.messages[1].role, MessageRole::Assistant);
    assert_eq!(chat.messages[1].tool_calls.len(), 1);
    assert_eq!(chat.messages[1].tool_calls[0].name, "note_save");
    assert_eq!(chat.messages[2].role, MessageRole::Tool);
    assert_eq!(chat.messages[3].text, "Запомнил.");

    // Заметка действительно сохранена инструментом (изоляция по профилю).
    let notes = reopened
        .db()
        .note_list(chat.profile_id, None, &[], None)
        .unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].content.contains("любит чай"));
}

#[tokio::test]
async fn disabled_tool_is_refused_by_loop() {
    use crate::shared::api::contract::ToolCallDelta;
    // python_exec выключен глобально (spawn_orch: python_enabled = false) —
    // даже если модель его вызовет, loop откажет, не исполняя.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("python_exec".into()),
                arguments: "{\"code\":\"print(1)\"}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        vec![
            ChatChunk::Text("ок".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;

    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("посчитай".into()))
        .unwrap();

    let tool_ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ToolCall { .. }))
        .await
        .unwrap();
    match tool_ev {
        AppEvent::ToolCall { name, result, .. } => {
            assert_eq!(name, "python_exec");
            assert!(result.contains("недоступен"), "got: {result}");
        }
        _ => unreachable!(),
    }
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn tool_round_limit_is_respected() {
    use crate::shared::api::contract::ToolCallDelta;
    // Движок всегда просит инструмент — должен сработать лимит раундов.
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::ToolCall(ToolCallDelta {
            thought_signature: None,
            index: 0,
            id: Some("c1".into()),
            name: Some("get_sampling".into()),
            arguments: "{}".into(),
        }),
        ChatChunk::Finished(FinishReason::ToolCalls),
    ])) as Arc<dyn EngineBackend>;

    let config = AppConfig {
        max_tool_rounds: 2,
        ..Default::default()
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("зациклись".into()))
        .unwrap();

    // Дойдём до Finished; лимит породит ошибку-пометку, но генерация завершится.
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn round_limit_forces_final_synthesis_without_tools() {
    use crate::shared::api::contract::ToolCallDelta;
    // При лимите раундов модель не должна оставить пользователя без ответа: после
    // исчерпания раундов делается финальный раунд БЕЗ инструментов, где модель
    // сводит итог. Скрипты: раунд 1 и 2 — вызовы инструмента, 3-й (форс-синтез) — текст.
    let toolcall = || {
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("get_sampling".into()),
                arguments: "{}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ]
    };
    let backend = Arc::new(MockBackend::sequence(vec![
        toolcall(),
        toolcall(),
        vec![
            ChatChunk::Text("Итог по собранному материалу.".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;

    let config = AppConfig {
        max_tool_rounds: 1,
        ..Default::default()
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("собери и проанализируй".into()))
        .unwrap();

    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Последнее сообщение — финальный текст ассистента (свод), а не пустота.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let last = chat.messages.last().unwrap();
    assert_eq!(last.role, MessageRole::Assistant);
    assert_eq!(last.text, "Итог по собранному материалу.");
}

#[tokio::test]
async fn followup_tool_makes_two_assistant_messages() {
    use crate::shared::api::contract::ToolCallDelta;
    // Раунд 1: текст + вызов send_followup_message → раунд 2: второе сообщение.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("Первое сообщение.".into()),
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("send_followup_message".into()),
                arguments: "{}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        vec![
            ChatChunk::Text("Второе сообщение.".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;

    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage("давай".into()))
        .unwrap();

    // UI получает сигнал «начать новый пузырь».
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::AssistantContinue { .. })
    })
    .await
    .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // История: user → assistant(followup tool_call) → tool → assistant(2-е, new_bubble).
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(chat.messages.len(), 4, "{:?}", chat.messages);
    assert_eq!(chat.messages[1].role, MessageRole::Assistant);
    assert_eq!(chat.messages[1].text, "Первое сообщение.");
    assert_eq!(chat.messages[1].tool_calls[0].name, "send_followup_message");
    assert!(!chat.messages[1].new_bubble);
    assert_eq!(chat.messages[2].role, MessageRole::Tool);
    assert_eq!(chat.messages[3].text, "Второе сообщение.");
    assert!(
        chat.messages[3].new_bubble,
        "второе сообщение — отдельным пузырём"
    );
    // Управляющий инструмент ничего не отбрасывает.
    assert!(chat.deleted.is_empty());
}

#[tokio::test]
async fn rewrite_tool_discards_partial_and_saves_it() {
    use crate::shared::api::contract::ToolCallDelta;
    // Раунд 1: неверный текст + вызов rewrite_current_message → раунд 2: переписанный.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("Неправильный ответ".into()),
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("rewrite_current_message".into()),
                arguments: "{}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        vec![
            ChatChunk::Text("Правильный ответ.".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;

    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage("вопрос".into()))
        .unwrap();

    // UI получает сигнал «отбросить текущий пузырь».
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::AssistantRewrite { .. })
    })
    .await
    .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // История: user → assistant(переписанный). Неверная версия — в архиве удалённого.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(chat.messages.len(), 2, "{:?}", chat.messages);
    assert_eq!(chat.messages[0].role, MessageRole::User);
    assert_eq!(chat.messages[1].role, MessageRole::Assistant);
    assert_eq!(chat.messages[1].text, "Правильный ответ.");
    // Отброшенный (неверный) ответ + его tool-сообщение сохранены для восстановления.
    assert_eq!(chat.deleted.len(), 1);
    let discarded = &chat.deleted[0].messages;
    assert_eq!(discarded[0].role, MessageRole::Assistant);
    assert_eq!(discarded[0].text, "Неправильный ответ");
    assert_eq!(discarded[0].tool_calls[0].name, "rewrite_current_message");
}

#[tokio::test]
async fn send_without_backend_emits_error() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();

    let err = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .unwrap();
    assert!(matches!(err, AppEvent::Error(_)));

    drop(cmd_tx);
    handle.await.unwrap();
}
