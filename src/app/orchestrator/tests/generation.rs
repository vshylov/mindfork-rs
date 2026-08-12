//! Orchestrator tests — generation, the agentic loop, cancellation, readiness gates. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

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

    // override = None → the profile's default is used.
    assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.5));

    // override = Some → it's used (the chat takes priority).
    orch.chats[0].sampling_override = Some(SamplingConfig {
        temperature: Some(0.9),
        ..Default::default()
    });
    assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.9));

    // Neither an override nor a profile default → the global one.
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

    // Wait for activation and learn the active chat's id.
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

    // Finish and check that the chat was saved with two messages.
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

    // Cloud mode (OpenAI) + a model name; global sampling with top_k, which
    // the strict cloud dialect doesn't accept → it must not land in the metadata snapshot.
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
        .expect("expected a metadata snapshot");
    assert_eq!(meta.mode, ServerMode::OpenAi);
    assert_eq!(meta.model.as_deref(), Some("gpt-test"));
    // Fields available in the mode are kept; unavailable top_k and temperature (rejected
    // by GPT 5.5/5.6) are cleared.
    assert_eq!(meta.sampling.max_tokens, Some(128));
    assert_eq!(meta.sampling.top_k, None);
    assert_eq!(meta.sampling.temperature, None);
}

#[tokio::test]
async fn emits_token_counter_during_generation() {
    use crate::shared::api::contract::TokenUsage;
    // Two text deltas (the live count = 2), then exact usage from the server (= 5).
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

    // Right after the start — an estimate of the conversation (prompt): context=Some, inexact,
    // no reply yet (completion=0).
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
        "conversation estimate: {est:?}"
    );

    // Reply deltas → the reply counter grows, the conversation estimate is untouched (None).
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
        "reply counter: {first:?}"
    );

    // The exact count from the server's usage arrives before completion: completion=5, context=12.
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
        "exact count from usage: {exact:?}"
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn regenerate_replaces_last_assistant_message() {
    // Two different replies in turn: the original turn gives the first, regeneration gives the second.
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
    // ChatList after Finished — a sign that handle_done applied the reply (state Idle).
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

    // The old reply is replaced by the new one; the user's message isn't duplicated.
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

    // The exchange is deleted entirely (a default chat with no greeting → empty).
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert!(chat.messages.is_empty(), "{:?}", chat.messages);
    // The deleted exchange is saved for manual recovery (the user's message +
    // the assistant's reply); the input draft was empty (spec §11.7).
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
    // A chat with an assistant greeting but no user message — there's nothing to
    // regenerate; the command must not start generation.
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
    // State stayed Idle (generation wasn't started), history untouched.
    assert!(orch.gen_state.is_idle());
    assert_eq!(orch.chats[0].messages.len(), 1);
}

#[test]
fn regenerate_on_not_ready_server_keeps_reply() {
    // The server is still connecting (managed is loading the model) — regeneration must
    // neither wipe the previous reply nor go out as a request to a not-ready server (otherwise
    // a 503 "engine returned an error status" and the reply is lost). Regression test.
    let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>;
    let (_d, mut orch) = bare_orch();
    orch.engines.backend = Some(backend);
    orch.engines.server_status = ServerStatus::Connecting; // not ready yet
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "c");
    chat.push_message(Message::user("вопрос"));
    chat.push_message(Message::assistant("старый ответ"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    orch.handle_regenerate();

    // The reply is preserved, generation didn't start (history wasn't truncated).
    assert!(orch.gen_state.is_idle());
    assert_eq!(
        orch.chats[0].messages.len(),
        2,
        "the previous reply must not be wiped on a not-ready server"
    );
    assert_eq!(orch.chats[0].messages[1].text, "старый ответ");
}

#[test]
fn send_on_not_ready_server_restores_input() {
    // The server is still connecting — the send is rejected, but the text is returned to the
    // input box (RestoreInput), and no message is added to the chat.
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
    assert!(
        orch.chats[0].messages.is_empty(),
        "no message should be added"
    );
    // Among the emitted events — an error and the text returning to the input box.
    let mut got_error = false;
    let mut restored = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AppEvent::Error(_) => got_error = true,
            AppEvent::RestoreInput(t) => restored = Some(t),
            _ => {}
        }
    }
    assert!(got_error, "an error about not-readiness should be emitted");
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
    // Round 1: a call to note_save → round 2: the final text.
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

    // The tool-execution event reaches the UI.
    let tool_ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ToolCall { .. }))
        .await
        .unwrap();
    assert!(matches!(tool_ev, AppEvent::ToolCall { name, .. } if name == "note_save"));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // History: user → assistant(tool_calls) → tool → assistant(final).
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.messages.len(), 4, "{:?}", chat.messages);
    assert_eq!(chat.messages[0].role, MessageRole::User);
    assert_eq!(chat.messages[1].role, MessageRole::Assistant);
    assert_eq!(chat.messages[1].tool_calls.len(), 1);
    assert_eq!(chat.messages[1].tool_calls[0].name, "note_save");
    assert_eq!(chat.messages[2].role, MessageRole::Tool);
    assert_eq!(chat.messages[3].text, "Запомнил.");

    // The note is actually saved by the tool (isolation by profile).
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
    // python_exec is disabled globally (spawn_orch: python_enabled = false) —
    // even if the model calls it, the loop will refuse without executing it.
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
    // The engine always requests a tool — the round limit should kick in.
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

    // Reach Finished; the limit produces an error marker, but generation completes.
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn round_limit_forces_final_synthesis_without_tools() {
    use crate::shared::api::contract::ToolCallDelta;
    // With the round limit, the model must not leave the user with no reply: after
    // exhausting the rounds a final round runs WITHOUT tools, where the model
    // sums up. Scripts: rounds 1 and 2 — tool calls, round 3 (forced synthesis) — text.
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

    // The last message — the assistant's final summary text, not emptiness.
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
    // Round 1: text + a call to send_followup_message → round 2: the second message.
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

    // The UI receives the signal to start a new bubble.
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

    // History: user → assistant(followup tool_call) → tool → assistant(2nd, new_bubble).
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
        "the second message should be a separate bubble"
    );
    // The control tool discards nothing.
    assert!(chat.deleted.is_empty());
}

#[tokio::test]
async fn rewrite_tool_discards_partial_and_saves_it() {
    use crate::shared::api::contract::ToolCallDelta;
    // Round 1: an incorrect text + a call to rewrite_current_message → round 2: the rewritten one.
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

    // The UI receives the signal to discard the current bubble.
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

    // History: user → assistant(rewritten). The incorrect version is in the deleted archive.
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
    // The discarded (incorrect) reply + its tool message are saved for recovery.
    assert_eq!(chat.deleted.len(), 1);
    let discarded = &chat.deleted[0].messages;
    assert_eq!(discarded[0].role, MessageRole::Assistant);
    assert_eq!(discarded[0].text, "Неправильный ответ");
    assert_eq!(discarded[0].tool_calls[0].name, "rewrite_current_message");
}

/// A bounded [`wait_for`].
///
/// The shared helper blocks until the event channel *closes*, so a regression that
/// simply stops emitting an event makes a test hang rather than fail — and in CI a
/// hang reads as broken infrastructure instead of a broken promise. Measured while
/// mutation-testing the note below: with its `send` removed the test ran past ten
/// minutes; bounded, it fails in five seconds.
async fn wait_for_bounded<F: Fn(&AppEvent) -> bool>(
    rx: &mut UnboundedReceiver<AppEvent>,
    what: &str,
    pred: F,
) -> AppEvent {
    tokio::time::timeout(std::time::Duration::from_secs(5), wait_for(rx, pred))
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|| panic!("the event stream closed before {what}"))
}

/// The regression this whole change exists for: a provider that dies **after** the
/// stream opened used to end the turn silently — `Finished(Error)` pushes no note
/// (only `Cancelled` does), so a reply cut off mid-sentence was indistinguishable
/// from a finished one, and the only trace was a line in the file log.
#[tokio::test]
async fn a_mid_stream_error_reaches_the_feed_and_keeps_the_partial_reply() {
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("Половина отв".into()),
        ChatChunk::Error {
            message: "overloaded_error: Overloaded".into(),
            transient: true,
        },
        ChatChunk::Finished(FinishReason::Error),
    ])) as Arc<dyn EngineBackend>;
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = dir.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("вопрос".into()))
        .unwrap();

    // The note comes before Finished, and `wait_for` drains — so it must be
    // pulled first or the wait below eats it.
    let AppEvent::Error(note) = wait_for_bounded(&mut evt_rx, "the failure note", |e| {
        matches!(e, AppEvent::Error(_))
    })
    .await
    else {
        unreachable!("filtered on Error")
    };
    assert!(
        note.contains("overloaded_error"),
        "the note must carry what the provider said, got {note:?}"
    );
    wait_for_bounded(&mut evt_rx, "Finished", |e| {
        matches!(e, AppEvent::Finished { .. })
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // What did arrive is still there — the note explains it, it does not replace it.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(chat.messages.len(), 2, "{:?}", chat.messages);
    assert_eq!(chat.messages[1].role, MessageRole::Assistant);
    assert_eq!(chat.messages[1].text, "Половина отв");
}

/// Which advice a failed turn gets. Asserted as the *decision* (the bundle key)
/// rather than the prose, so a translation edit cannot break it and a wrong branch
/// cannot pass.
#[test]
fn the_failure_note_is_chosen_by_condition() {
    use crate::app::orchestrator::generation::engine_error_key;

    let overflow =
        "engine returned status 400: {\"error\":{\"type\":\"exceed_context_size_error\"}}";
    let plain = "overloaded_error: Overloaded";

    // Overflow wins over everything: it is the one failure with a specific way
    // out, and which way out depends on whether compression is switched on.
    assert_eq!(
        engine_error_key(overflow, true, false),
        "ui.err.context_overflow"
    );
    assert_eq!(
        engine_error_key(overflow, false, false),
        "ui.err.context_overflow_off"
    );
    // ...including when text was already on screen — pointing at `/compact` beats
    // "press Ctrl+R", which would just overflow again.
    assert_eq!(
        engine_error_key(overflow, true, true),
        "ui.err.context_overflow"
    );
    // Anything else: text already on screen means a fragment, so say so and name
    // the way to a whole reply; nothing on screen is a plain failure.
    assert_eq!(
        engine_error_key(plain, true, true),
        "ui.err.generation_interrupted"
    );
    assert_eq!(
        engine_error_key(plain, true, false),
        "ui.err.generation_failed"
    );
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
