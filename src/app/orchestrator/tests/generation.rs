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
            prefill: None,
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
        engine_error_key(overflow, true, false, false),
        "ui.err.context_overflow"
    );
    assert_eq!(
        engine_error_key(overflow, false, false, false),
        "ui.err.context_overflow_off"
    );
    // ...including when text was already on screen — pointing at `/compact` beats
    // "press Ctrl+R", which would just overflow again — and regardless of
    // whether `/continue` could pick the fragment up: continuing would just
    // overflow again too, so `/compact` stays the named way out.
    assert_eq!(
        engine_error_key(overflow, true, true, true),
        "ui.err.context_overflow"
    );
    // Anything else: text already on screen means a fragment, so say so and name
    // the way to a whole reply — the resuming one when the mode can resume
    // (fork F9), the regenerating one when it cannot; nothing on screen is a
    // plain failure, whatever the mode.
    assert_eq!(
        engine_error_key(plain, true, true, true),
        "ui.err.generation_interrupted_continuable"
    );
    assert_eq!(
        engine_error_key(plain, true, true, false),
        "ui.err.generation_interrupted"
    );
    assert_eq!(
        engine_error_key(plain, true, false, true),
        "ui.err.generation_failed"
    );
    assert_eq!(
        engine_error_key(plain, true, false, false),
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

/// The code workspace is exempt from `max_tool_rounds` (spec §9.12), and the
/// exemption has to be **both** halves of a promise: a turn survives far past a
/// budget that would have cut it, and it still ends.
///
/// The engine here asks for `code_list` forever, with a budget of one round and
/// a project ceiling of three — without the exemption the turn would stop at the
/// first round, and without the ceiling it would never stop at all. The ceiling
/// is read from `workspace.max_rounds` rather than from a constant, which is
/// what the count below pins: three is a number only the setting can produce.
/// The wait is bounded, so a regression fails instead of hanging
/// (docs/lessons.md §2).
#[tokio::test]
async fn workspace_rounds_do_not_spend_the_budget_but_still_end() {
    use crate::shared::api::contract::ToolCallDelta;
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::ToolCall(ToolCallDelta {
            thought_signature: None,
            index: 0,
            id: Some("c1".into()),
            name: Some(crate::features::tools::code::CODE_LIST_ID.into()),
            arguments: "{}".into(),
        }),
        ChatChunk::Finished(FinishReason::ToolCalls),
    ])) as Arc<dyn EngineBackend>;

    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.rs"), "fn main() {}\n").unwrap();

    let config = AppConfig {
        // One round: enough that an unexempt tool would be cut off immediately.
        max_tool_rounds: 1,
        workspace: crate::shared::config::WorkspaceSettings {
            // Well below the default of 500, so the turn ends quickly *and* the
            // count proves the setting is what bounds it.
            max_rounds: 3,
            ..Default::default()
        },
        ..Default::default()
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    let profile = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .and_then(|e| match e {
            AppEvent::ProfileList(ps) => ps.first().map(|p| p.id),
            _ => None,
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(vec![crate::features::tools::code::CODE_LIST_ID.into()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::ProjectAttach {
            path: project.path().to_string_lossy().into_owned(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProjectProgress(_)))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage("посмотри проект".into()))
        .unwrap();
    let mut calls = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let finished = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Some(ev) = evt_rx.recv().await {
            match ev {
                AppEvent::ToolCall { .. } => calls += 1,
                AppEvent::Error(msg) => errors.push(msg),
                AppEvent::Finished { .. } => return true,
                _ => {}
            }
        }
        false
    })
    .await
    .expect("the turn must end rather than loop forever");
    assert!(finished, "the event stream closed before the turn finished");
    assert!(
        calls > 1,
        "a budget of one round must not stop an exempt tool: {calls} calls"
    );
    assert_eq!(
        calls, 3,
        "the ceiling must come from workspace.max_rounds: {calls} calls"
    );
    // And the note that ends such a turn must name the limit that fired. It
    // quoted `max_tool_rounds` for both until stage 3 — sending the user to
    // change a setting that was not the problem (docs/lessons.md §4).
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let expected = loc.tf("loop.workspace_round_limit_reached", &[("max_rounds", "3")]);
    assert!(
        errors.contains(&expected),
        "the project limit must name itself: {errors:?}"
    );
    drop(cmd_tx);
    handle.await.unwrap();
}

// ---------- /continue (spec §6.4, docs/research/continue-generation.md) ----------

/// The echo filter in isolation: an exact echo yields only the continuation, a
/// non-echoing stream loses nothing (including one that first coincides with
/// part of the seed), and a multi-byte character split across deltas keeps its
/// boundaries. Mutating the mismatch flush to drop the withheld bytes fails
/// the third case; mutating full-match detection fails the first.
#[test]
fn the_echo_filter_strips_an_echo_and_passes_real_content() {
    use super::super::generation::EchoFilter;

    let mut f = EchoFilter::new("Per the atlas".into());
    assert_eq!(f.push("Per the "), "");
    assert_eq!(f.push("atlas"), "");
    assert_eq!(f.push(" Paris."), " Paris.");

    let mut f = EchoFilter::new("Per the atlas".into());
    assert_eq!(f.push(" Paris."), " Paris.", "no echo: everything flows");

    let mut f = EchoFilter::new("Per the atlas".into());
    assert_eq!(f.push("Per"), "", "still ambiguous: withheld");
    assert_eq!(
        f.push(" it goes"),
        "Per it goes",
        "divergence flushes the withheld prefix"
    );

    let mut f = EchoFilter::new("По".into());
    assert_eq!(f.push("П"), "");
    assert_eq!(
        f.push("о!"),
        "!",
        "the split multi-byte echo is consumed whole"
    );
}

/// The in-place fold (fork F8) and the model-name rule (fork F7): the text
/// joins at the model's own seam, the id and timestamp stay the seed's, the
/// end state becomes the round's — and the model name stays the seed's unless
/// the continuation outgrew what it continued.
#[test]
fn merge_continuation_appends_in_place_and_keeps_the_model_name() {
    use super::super::generation::merge_continuation;
    use crate::entities::message::{Message, MessageFinish, MessageMetadata};

    let meta = |model: &str, finish: MessageFinish| MessageMetadata {
        sampling: Default::default(),
        mode: Default::default(),
        model: Some(model.into()),
        finish: Some(finish),
    };
    let mut seed = Message::assistant("Начало было длинным и обстоятельным");
    seed.metadata = Some(meta("gemma-4-31b", MessageFinish::Cancelled));
    let (id, stamp) = (seed.id, seed.timestamp);

    let mut round = Message::assistant(", конец.");
    round.metadata = Some(meta("qwen-3.6", MessageFinish::Stop));
    merge_continuation(&mut seed, round);

    assert_eq!(seed.text, "Начало было длинным и обстоятельным, конец.");
    assert_eq!(seed.id, id, "the same reply, finished later");
    assert_eq!(seed.timestamp, stamp);
    let md = seed.metadata.as_ref().unwrap();
    assert_eq!(
        md.finish,
        Some(MessageFinish::Stop),
        "the end state is the round's"
    );
    assert_eq!(
        md.model.as_deref(),
        Some("gemma-4-31b"),
        "a short continuation keeps the header on the model that wrote the bulk"
    );

    // The continuation outgrew the partial: the header follows the larger share.
    let mut seed = Message::assistant("Нач");
    seed.metadata = Some(meta("gemma-4-31b", MessageFinish::Error));
    let mut round = Message::assistant("ало, середина и весь длинный конец ответа.");
    round.metadata = Some(meta("qwen-3.6", MessageFinish::Stop));
    merge_continuation(&mut seed, round);
    assert_eq!(
        seed.metadata.unwrap().model.as_deref(),
        Some("qwen-3.6"),
        "an outgrown partial takes the continuing model's name"
    );
}

/// What [`interrupted_turn`] hands back: the harness, plus the first turn's
/// `Finished` event (some tests assert on what it announced).
type InterruptedOrch = (
    tempfile::TempDir,
    tokio::sync::mpsc::UnboundedSender<AppCommand>,
    tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
    AppEvent,
);

/// The shared road into a `/continue` scenario: one scripted turn that leaves
/// its interruption on disk, waited past the list re-emit — the proof the tail
/// has been applied before a test reads it (the result lands on its own
/// channel). Each test supplies only its script and its distinctive middle:
/// when the third test starts the same way as the first two, that opening is
/// a fixture (docs/lessons.md §2).
async fn interrupted_turn(script: Vec<Vec<ChatChunk>>) -> InterruptedOrch {
    let backend = Arc::new(MockBackend::sequence(script)) as Arc<dyn EngineBackend>;
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), no_auto_cfg());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("вопрос".into()))
        .unwrap();
    let finished = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();
    (dir, cmd_tx, evt_rx, handle, finished)
}

/// The chat as the closed app left it on disk.
fn reload_chat(root: &std::path::Path) -> crate::entities::chat::Chat {
    Storage::open(Paths::with_root(root))
        .unwrap()
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

/// The whole route: a cancelled reply, then `/continue` — the turn announces
/// itself as a continuation (the feed re-opens the bubble on that flag), and
/// on disk there is still **one** assistant message, joined at the model's own
/// seam, with the end state re-recorded from `Cancelled` to `Stop`.
#[tokio::test]
async fn continue_appends_into_the_same_message() {
    use crate::entities::message::MessageFinish;

    let (dir, cmd_tx, mut evt_rx, handle, _) = interrupted_turn(vec![
        vec![
            ChatChunk::Text("Нача".into()),
            ChatChunk::Finished(FinishReason::Cancelled),
        ],
        vec![
            ChatChunk::Text("ло готово.".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])
    .await;

    cmd_tx.send(AppCommand::ContinueLast).unwrap();
    let started = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::GenerationStarted { .. })
    })
    .await
    .unwrap();
    assert!(
        matches!(
            started,
            AppEvent::GenerationStarted {
                continuation: true,
                ..
            }
        ),
        "the feed must be told to re-open the bubble instead of pushing one"
    );
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = reload_chat(dir.path());
    assert_eq!(chat.messages.len(), 2, "{:?}", chat.messages);
    assert_eq!(
        chat.messages[1].text, "Начало готово.",
        "joined at the seam, with no separator"
    );
    assert_eq!(
        chat.messages[1].metadata.as_ref().and_then(|m| m.finish),
        Some(MessageFinish::Stop),
        "the end state is the continuation's"
    );
}

/// llama.cpp returns prefill + continuation (research §7.1): the echoed prefix
/// must reach neither the screen nor the stored message. The second turn's
/// script echoes the partial verbatim; the feed sees only the new text, and
/// the disk holds it exactly once.
#[tokio::test]
async fn continue_strips_the_echoed_prefill() {
    let (dir, cmd_tx, mut evt_rx, handle, _) = interrupted_turn(vec![
        vec![
            ChatChunk::Text("Нача".into()),
            ChatChunk::Finished(FinishReason::Cancelled),
        ],
        vec![
            ChatChunk::Text("Нача".into()),
            ChatChunk::Text("ло готово.".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])
    .await;

    cmd_tx.send(AppCommand::ContinueLast).unwrap();
    let mut chunks: Vec<String> = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Chunk { text, .. } => chunks.push(text),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(
        chunks,
        vec!["ло готово.".to_string()],
        "the echoed prefix must not re-stream into the feed"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(
        reload_chat(dir.path()).messages[1].text,
        "Начало готово.",
        "the echo must not double into the stored text"
    );
}

/// A length-cut reply records `Length` as its end state, and the turn's
/// `Finished` carries `continuable` so the feed's truncation note can name
/// `/continue` (fork F9). The mock supervisor runs as managed — the mode that
/// supports continuation.
#[tokio::test]
async fn a_length_cut_reply_is_recorded_and_announced_as_continuable() {
    use crate::entities::message::MessageFinish;

    let (dir, cmd_tx, _evt_rx, handle, finished) = interrupted_turn(vec![vec![
        ChatChunk::Text("Полов".into()),
        ChatChunk::Finished(FinishReason::Length),
    ]])
    .await;
    assert!(
        matches!(
            finished,
            AppEvent::Finished {
                reason: FinishReason::Length,
                continuable: true,
                ..
            }
        ),
        "a managed-mode length cut must announce the resume route: {finished:?}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(
        reload_chat(dir.path()).messages[1]
            .metadata
            .as_ref()
            .and_then(|m| m.finish),
        Some(MessageFinish::Length)
    );
}

/// A turn interrupted between tool rounds leaves a tool-result tail;
/// `/continue` then resumes the agentic loop as an ordinary round — a **new**
/// assistant message, no prefill, no bubble re-opening.
#[tokio::test]
async fn continue_resumes_the_loop_on_a_tool_result_tail() {
    use crate::shared::api::contract::ToolCallDelta;

    let (dir, cmd_tx, mut evt_rx, handle, _) = interrupted_turn(vec![
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("note_save".into()),
                arguments: "{\"content\":\"факт\"}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        // The round after the tool: nothing arrives — the turn ends as
        // cancelled, leaving the tool message as the chat's tail.
        vec![ChatChunk::Finished(FinishReason::Cancelled)],
        // The resumed round.
        vec![
            ChatChunk::Text("Записал.".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])
    .await;

    cmd_tx.send(AppCommand::ContinueLast).unwrap();
    let started = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::GenerationStarted { .. })
    })
    .await
    .unwrap();
    assert!(
        matches!(
            started,
            AppEvent::GenerationStarted {
                continuation: false,
                ..
            }
        ),
        "a loop resume opens a bubble of its own: {started:?}"
    );
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = reload_chat(dir.path());
    let last = chat.messages.last().unwrap();
    assert_eq!(last.role, MessageRole::Assistant);
    assert_eq!(last.text, "Записал.", "{:?}", chat.messages);
}

/// Every refusal answers with the route that works (docs/lessons.md §4), and
/// each is chosen by the tail's actual state. Driven on a bare orchestrator:
/// the refusals need no turn, only the answer.
#[tokio::test]
async fn continue_refusals_answer_with_the_route_that_works() {
    use crate::entities::chat::Chat;
    use crate::entities::message::{Message, MessageFinish, MessageMetadata};
    use crate::entities::profile::Profile;
    use crate::shared::config::ServerMode;

    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![])) as Arc<dyn EngineBackend>);
    orch.engines.server_status = ServerStatus::Ready;
    let mut chat = Chat::from_profile(&Profile::new("P", "sys"), "Чат");
    let chat_id = chat.id;
    chat.push_message(Message::user("вопрос"));
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    let mut expect_note = |orch: &mut super::super::Orchestrator, key: &str| {
        orch.handle_continue();
        let mut note = None;
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Error(text) = ev {
                note = Some(text);
            }
        }
        assert_eq!(note.as_deref(), Some(loc.t(key)), "expected {key}");
    };

    // A user tail: nothing to continue.
    expect_note(&mut orch, "ui.cmd.continue_nothing");

    // An assistant tail that finished on its own.
    let mut done = Message::assistant("Готовый ответ.");
    done.metadata = Some(MessageMetadata {
        sampling: Default::default(),
        mode: Default::default(),
        model: None,
        finish: Some(MessageFinish::Stop),
    });
    orch.chat_mut(chat_id).unwrap().push_message(done);
    expect_note(&mut orch, "ui.cmd.continue_complete");

    // Cut inside the reasoning, before any visible text (fork F4).
    {
        let chat = orch.chat_mut(chat_id).unwrap();
        let last = chat.messages.last_mut().unwrap();
        last.text = String::new();
        last.thoughts = Some("обрывок мысли".into());
    }
    expect_note(&mut orch, "ui.cmd.continue_thoughts");

    // A cloud mode: the capability gate answers before anything else.
    {
        let chat = orch.chat_mut(chat_id).unwrap();
        let last = chat.messages.last_mut().unwrap();
        last.text = "Оборванный отв".into();
        last.thoughts = None;
        if let Some(md) = last.metadata.as_mut() {
            md.finish = Some(MessageFinish::Cancelled);
        }
    }
    orch.config.engine.mode = ServerMode::OpenAi;
    expect_note(&mut orch, "ui.cmd.continue_unsupported");

    // Anthropic is per-model (stage 2): a 4.6+ id refuses — the wider matrix
    // is pinned in `shared::config`'s own capability test.
    orch.config.engine.mode = ServerMode::Claude;
    orch.config.engine.claude.model_name = Some("claude-opus-4-8".into());
    expect_note(&mut orch, "ui.cmd.continue_unsupported");

    // Back on a supporting mode with a continuable tail — and a message stored
    // before the bookkeeping existed (`finish: None`) is continuable too
    // (fork F1): the turn actually starts.
    orch.config.engine.mode = ServerMode::Managed;
    if let Some(md) = orch
        .chat_mut(chat_id)
        .unwrap()
        .messages
        .last_mut()
        .unwrap()
        .metadata
        .as_mut()
    {
        md.finish = None;
    }
    orch.handle_continue();
    let mut started = None;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::GenerationStarted { continuation, .. } = ev {
            started = Some(continuation);
        }
    }
    assert_eq!(
        started,
        Some(true),
        "a legacy partial without the end-state field must still continue"
    );
}

// ---------- the slow-prefill note (docs/research/slow-prefill-detection.md) ----------

mod slow_prefill {
    use super::*;
    use crate::shared::api::contract::{Prefill, TokenUsage};
    use crate::shared::config::ServerMode;
    use crate::shared::server::ServerStatus;

    /// The CPU build at the default batch: ~90 tok/s over a real prompt.
    const SLOW: Prefill = Prefill {
        tokens: 1800,
        ms: 20_000,
    };

    fn notices(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let AppEvent::Notice(text) = e {
                out.push(text);
            }
        }
        out
    }

    /// A managed server at the default batch: one note per server session,
    /// naming the field, the hold and the batch; the next turn says nothing;
    /// a server that reaches `Ready` again — a relaunch — may be told once
    /// more (§3.3).
    #[test]
    fn a_managed_server_is_told_once_per_session() {
        let (_d, mut orch, mut rx) = bare_orch_rx();
        orch.config.engine.mode = ServerMode::Managed;
        orch.config.engine.managed.gpu_layers = 99;
        orch.config.engine.managed.batch_size = None;

        orch.note_slow_prefill(Some(SLOW));
        let notes = notices(&mut rx);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("Batch (-b)") && notes[0].contains("23") && notes[0].contains("2048"),
            "{}",
            notes[0]
        );

        orch.note_slow_prefill(Some(SLOW));
        assert!(notices(&mut rx).is_empty(), "once per session");

        orch.engines.set_chat_status(ServerStatus::Ready);
        orch.note_slow_prefill(Some(SLOW));
        assert_eq!(notices(&mut rx).len(), 1, "a new session, told again");
    }

    /// Nothing to say: the batch already at the knee (typed, or the CPU auto),
    /// a fast prefill, a cloud, no sample.
    #[test]
    fn nothing_is_said_where_the_rule_does_not_hold() {
        let (_d, mut orch, mut rx) = bare_orch_rx();
        orch.config.engine.mode = ServerMode::Managed;
        orch.config.engine.managed.gpu_layers = 99;
        orch.config.engine.managed.batch_size = Some(256);
        orch.note_slow_prefill(Some(SLOW));
        assert!(notices(&mut rx).is_empty(), "typed 256: the change is made");

        orch.config.engine.managed.batch_size = None;
        orch.config.engine.managed.gpu_layers = 0;
        orch.note_slow_prefill(Some(SLOW));
        assert!(notices(&mut rx).is_empty(), "the CPU auto: 256 already");

        orch.config.engine.managed.gpu_layers = 99;
        orch.note_slow_prefill(Some(Prefill {
            tokens: 1800,
            ms: 900,
        }));
        assert!(notices(&mut rx).is_empty(), "a GPU's second");

        orch.config.engine.mode = ServerMode::OpenAi;
        orch.note_slow_prefill(Some(SLOW));
        assert!(notices(&mut rx).is_empty(), "a cloud has no batch");

        orch.config.engine.mode = ServerMode::Managed;
        orch.note_slow_prefill(None);
        assert!(notices(&mut rx).is_empty(), "no sample");
    }

    /// An external server: the launch line's wording, at the default batch the
    /// app assumes (fork F3).
    #[test]
    fn an_external_server_is_told_the_launch_line() {
        let (_d, mut orch, mut rx) = bare_orch_rx();
        orch.config.engine.mode = ServerMode::External;
        orch.note_slow_prefill(Some(SLOW));
        let notes = notices(&mut rx);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("-b 256 -ub 256") && notes[0].contains("2048"),
            "{}",
            notes[0]
        );
    }

    /// A tool's own request is a stream of the turn (spec §9.3.1): the
    /// timing it reports on its outcome folds into the turn's largest sample
    /// and reaches the note like a round's — the page summary's path without
    /// the page (docs/research/page-summary-usage.md §3.2). The engine's own
    /// rounds carry a warm sample under the floor; the tool's is the cold one.
    #[tokio::test]
    async fn a_tools_own_request_is_the_turns_sample_too() {
        use crate::app::orchestrator::tests::subagent::{Script, ScriptRecorder};
        let warm = ChatChunk::Usage(TokenUsage {
            prompt_tokens: 40,
            completion_tokens: 1,
            reasoning_tokens: 0,
            prefill: Some(Prefill { tokens: 40, ms: 20 }),
        });
        let call = Script {
            chunks: vec![
                ChatChunk::ToolCall(crate::shared::api::contract::ToolCallDelta {
                    thought_signature: None,
                    index: 0,
                    id: Some("s1".into()),
                    name: Some("sampled".into()),
                    arguments: "{}".into(),
                }),
                warm.clone(),
                ChatChunk::Finished(FinishReason::ToolCalls),
            ],
            hang: false,
        };
        let reply = Script {
            chunks: vec![
                ChatChunk::Text("done".into()),
                warm,
                ChatChunk::Finished(FinishReason::Stop),
            ],
            hang: false,
        };
        let backend = ScriptRecorder::new(vec![call, reply]);
        let mut config = no_auto_cfg();
        config.engine.mode = ServerMode::External;
        let tool = Arc::new(crate::app::orchestrator::tests::SampledTool {
            id: "sampled",
            sample: Some(SLOW),
        });
        let (_d, cmd_tx, mut evt_rx, handle) =
            spawn_orch_tools(Some(backend as Arc<dyn EngineBackend>), config, vec![tool]);
        // The startup emits the profile list and the active chat in whichever
        // order; both are needed before the tool can be enabled and the
        // message sent.
        let (mut pid, mut chat) = (None, None);
        while pid.is_none() || chat.is_none() {
            match tokio::time::timeout(std::time::Duration::from_secs(5), evt_rx.recv())
                .await
                .expect("startup events")
            {
                Some(AppEvent::ProfileList(v)) if !v.is_empty() => pid = Some(v[0].id),
                Some(AppEvent::ChatActivated { id, .. }) => chat = Some(id),
                Some(_) => {}
                None => panic!("the orchestrator went away during startup"),
            }
        }
        cmd_tx
            .send(AppCommand::UpdateProfile {
                id: pid.unwrap(),
                edit: Box::new(ProfileEdit {
                    enabled_tools: Some(vec!["sampled".into()]),
                    ..Default::default()
                }),
            })
            .unwrap();
        cmd_tx.send(AppCommand::SendMessage("go".into())).unwrap();
        let note = wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::Notice(t) if t.contains("-b 256 -ub 256")),
        )
        .await;
        assert!(note.is_some(), "the tool's cold sample reached the note");
        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
    }

    /// The whole path: the engine's `timings` on the stream's usage chunk
    /// travel through the round, the turn and the landing to the note
    /// (§3.1) — and the second turn on the same server says nothing.
    #[tokio::test]
    async fn a_turn_carries_the_engines_figure_to_the_note() {
        let backend = Arc::new(MockBackend::scripted(vec![
            ChatChunk::Text("hi".into()),
            ChatChunk::Usage(TokenUsage {
                prompt_tokens: 1850,
                completion_tokens: 1,
                reasoning_tokens: 0,
                prefill: Some(SLOW),
            }),
            ChatChunk::Finished(FinishReason::Stop),
        ])) as Arc<dyn EngineBackend>;
        let mut config = AppConfig::default();
        config.engine.mode = ServerMode::External;
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();

        cmd_tx.send(AppCommand::SendMessage("one".into())).unwrap();
        let note = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Notice(_)))
            .await
            .unwrap();
        let AppEvent::Notice(text) = note else {
            unreachable!()
        };
        assert!(text.contains("-b 256 -ub 256"), "{text}");

        cmd_tx.send(AppCommand::SendMessage("two".into())).unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        // The landing's note, had there been one, follows `Finished` at once;
        // a quit flushes the channel in order.
        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
        let mut second = Vec::new();
        while let Ok(e) = evt_rx.try_recv() {
            if let AppEvent::Notice(t) = e {
                second.push(t);
            }
        }
        assert!(second.is_empty(), "the same server, told once: {second:?}");
    }
}

/// The estimate counts the tool schemas as the wire sends them
/// (docs/research/roll-usage-calibration.md §3.1): one part at the text's
/// bytes-per-token, plus the per-message overhead once for the block; a
/// request without tools estimates as before.
#[test]
fn the_estimate_counts_the_tool_schemas() {
    use crate::shared::api::contract::ToolSchema;
    let bare = crate::shared::api::ChatRequest {
        continue_final: false,
        system: Some("sys".into()),
        messages: vec![crate::shared::api::ApiMessage::user("hi")],
        sampling: Default::default(),
        tools: Vec::new(),
    };
    let schema = ToolSchema {
        name: "note_save".into(),
        description: "Saves a note.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } }
        }),
    };
    let with = crate::shared::api::ChatRequest {
        tools: vec![schema],
        ..bare.clone()
    };
    let json = crate::shared::api::openai::tools_json(&with.tools);
    assert!(
        json.starts_with(r#"[{"type":"function","function":{"name":"note_save""#),
        "the wire's own wrapper: {json}"
    );
    let bare_estimate = super::super::generation::estimate_prompt_tokens(&bare);
    let with_estimate = super::super::generation::estimate_prompt_tokens(&with);
    assert_eq!(
        with_estimate - bare_estimate,
        (json.len() as u64).div_ceil(4) + 4,
        "the block's bytes over four, plus one message overhead"
    );
}
