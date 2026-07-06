//! Тесты оркестратора — имперсонация (построение запроса + поток). Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/refactoring-god-objects.md, этап 3.

use super::*;

#[test]
fn impersonation_request_swaps_roles_and_sets_system() {
    let profile = Profile::new("P", "sys ассистента");
    let mut chat = Chat::from_profile(&profile, "c");
    chat.push_message(Message::assistant("Привет! Чем помочь?"));
    chat.push_message(Message::user("Расскажи о Rust"));
    chat.push_message(Message::assistant("Rust — системный язык…"));

    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "",
        SamplingConfig::default(),
        None,
    );

    assert_eq!(req.system.as_deref(), Some("Ты — пользователь"));
    assert!(req.tools.is_empty());
    // Роли поменялись местами: assistant↔user.
    assert_eq!(req.messages.len(), 3);
    assert_eq!(
        req.messages[0].role,
        crate::shared::api::contract::ApiRole::User
    );
    assert_eq!(req.messages[0].content, "Привет! Чем помочь?");
    assert_eq!(
        req.messages[1].role,
        crate::shared::api::contract::ApiRole::Assistant
    );
    assert_eq!(req.messages[1].content, "Расскажи о Rust");
    assert_eq!(
        req.messages[2].role,
        crate::shared::api::contract::ApiRole::User
    );
}

#[test]
fn impersonation_request_disables_reasoning() {
    // Имперсонация отбрасывает «мысли», поэтому запрос обязан гасить reasoning —
    // иначе модели со «вшитым» thinking (Gemma/Qwen) тратят весь бюджет на
    // reasoning_content, а ответный текст приходит пустым (предпросмотр пуст).
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "c");
    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "",
        // Пользователь оставил «мысли» включёнными в семплинге имперсонации —
        // запрос всё равно должен их выключить.
        SamplingConfig {
            thinking: Some(true),
            ..Default::default()
        },
        None,
    );
    assert_eq!(req.sampling.thinking, Some(false));
    assert_eq!(req.sampling.reasoning_budget, Some(0));
    assert_eq!(
        req.sampling.reasoning_effort,
        Some(crate::entities::sampling::ReasoningEffort::None)
    );
}

#[test]
fn impersonation_request_with_seed_adds_continuation_hint() {
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "c");
    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "Мне нужно ",
        SamplingConfig::default(),
        None,
    );
    let system = req.system.unwrap();
    assert!(system.contains("Ты — пользователь"));
    assert!(system.contains("Мне нужно"), "затравка попала в инструкцию");
}

#[test]
fn impersonation_request_includes_user_hint() {
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "c");
    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "",
        SamplingConfig::default(),
        Some("Известное о человеке: черты — скептик"),
    );
    let system = req.system.unwrap();
    assert!(system.contains("Ты — пользователь"));
    // Модель собеседника подмешана в системный промпт имперсонации.
    assert!(system.contains("черты — скептик"));
}

#[test]
fn swap_role_skips_system_tool_and_empty() {
    assert!(swap_role_message(&Message::new(MessageRole::System, "x")).is_none());
    assert!(swap_role_message(&Message::new(MessageRole::Tool, "x")).is_none());
    assert!(swap_role_message(&Message::user("   ")).is_none());
}

#[tokio::test]
async fn impersonate_streams_into_preview_and_finishes() {
    // Первый запрос (отправка) → «ответ»; второй (имперсонация) → реплика.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("ответ".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        vec![
            ChatChunk::Text("моя реплика".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Нужна хотя бы одна реплика в истории.
    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::Impersonate {
            seed: String::new(),
        })
        .unwrap();
    // Старт имперсонации.
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImpersonationStarted { .. })
    })
    .await
    .unwrap();
    // Текст реплики приходит дельтами.
    let chunk = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImpersonationChunk { .. })
    })
    .await
    .unwrap();
    assert!(matches!(chunk, AppEvent::ImpersonationChunk { text, .. } if text == "моя реплика"));
    // Завершение со Stop.
    let fin = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImpersonationFinished { .. })
    })
    .await
    .unwrap();
    assert!(matches!(
        fin,
        AppEvent::ImpersonationFinished {
            reason: FinishReason::Stop,
            ..
        }
    ));

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}
