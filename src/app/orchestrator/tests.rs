//! Тесты оркестратора (без UI и реальной модели): автомат генерации, гонки,
//! agentic-loop, приоритеты семплинга, операции списка/профилей/RAG.

use super::impersonation::{build_impersonation_request, swap_role_message};
use super::request::build_request;
use super::title::salvage_title_source;
use super::*;

use crate::app::events::RagProgress;
use crate::entities::message::{Message, MessageRole};
use crate::features::profiles::ProfileEdit;
use crate::shared::api::ChatChunk;

use crate::app::supervisor::MockSupervisor;
use crate::shared::api::mock::{MockBackend, MockEmbedder};
use crate::shared::paths::Paths;

fn test_embedder() -> Arc<dyn Embedder> {
    Arc::new(MockEmbedder::new(16))
}

/// Поднимает оркестратор на временном хранилище. Возвращает каналы и handle.
fn spawn_orch(
    backend: Option<Arc<dyn EngineBackend>>,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    spawn_orch_cfg(backend, AppConfig::default())
}

/// Как [`spawn_orch`], но с заданной конфигурацией (max_tool_rounds и т.п.).
fn spawn_orch_cfg(
    backend: Option<Arc<dyn EngineBackend>>,
    config: AppConfig,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend(backend)),
    };
    let handle = tokio::spawn(run(deps));
    (dir, cmd_tx, evt_rx, handle)
}

/// Дренирует события до первого, удовлетворяющего предикату (или закрытия).
async fn wait_for<F: Fn(&AppEvent) -> bool>(
    rx: &mut UnboundedReceiver<AppEvent>,
    pred: F,
) -> Option<AppEvent> {
    while let Some(ev) = rx.recv().await {
        if pred(&ev) {
            return Some(ev);
        }
    }
    None
}

/// Собирает «голый» оркестратор для юнит-тестов чистых методов (без петли),
/// отбрасывая поток событий.
fn bare_orch() -> (tempfile::TempDir, Orchestrator) {
    let (dir, orch, _rx) = bare_orch_rx();
    (dir, orch)
}

/// Как [`bare_orch`], но возвращает и приёмник событий (для проверки эмиссии).
fn bare_orch_rx() -> (tempfile::TempDir, Orchestrator, UnboundedReceiver<AppEvent>) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (evt_tx, evt_rx) = unbounded_channel();
    let (done_tx, _done_rx) = unbounded_channel();
    let (status_tx, _status_rx) = unbounded_channel();
    let (title_tx, _title_rx) = unbounded_channel();
    let (imp_status_tx, _imp_status_rx) = unbounded_channel();
    let (imp_done_tx, _imp_done_rx) = unbounded_channel();
    let config = AppConfig {
        default_sampling: SamplingConfig {
            temperature: Some(0.1),
            ..Default::default()
        },
        ..Default::default()
    };
    let registry = Arc::new(build_registry(&config));
    let orch = Orchestrator {
        evt_tx,
        supervisor: Arc::new(MockSupervisor::with_backend(None)),
        backend: None,
        chat_handle: None,
        embed_handle: None,
        imp_backend: None,
        imp_handle: None,
        imp_status: ServerStatus::NotConfigured,
        imp_status_tx,
        imp_cancel: None,
        imp_gen: None,
        imp_done_tx,
        embedder: test_embedder(),
        storage,
        config,
        registry,
        status_tx,
        title_tx,
        server_status: ServerStatus::Ready,
        profiles: Vec::new(),
        chats: Vec::new(),
        active_id: None,
        gen_state: GenState::Idle,
        done_tx,
        rag_cancel: None,
        dirty: HashSet::new(),
        save_deadline: None,
    };
    (dir, orch, evt_rx)
}

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
    assert!(orch.dirty.contains(&id), "чат помечен для сохранения");

    // Повторная установка того же текста — без повторной пометки (no-op).
    orch.dirty.clear();
    orch.handle_set_draft("недописанный текст".into());
    assert!(!orch.dirty.contains(&id));
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
}

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
    );

    assert_eq!(req.system.as_deref(), Some("Ты — пользователь"));
    assert!(req.tools.is_empty());
    // Роли поменялись местами: assistant↔user.
    assert_eq!(req.messages.len(), 3);
    assert_eq!(
        req.messages[0].role,
        crate::shared::api::backend::ApiRole::User
    );
    assert_eq!(req.messages[0].content, "Привет! Чем помочь?");
    assert_eq!(
        req.messages[1].role,
        crate::shared::api::backend::ApiRole::Assistant
    );
    assert_eq!(req.messages[1].content, "Расскажи о Rust");
    assert_eq!(
        req.messages[2].role,
        crate::shared::api::backend::ApiRole::User
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
    );
    let system = req.system.unwrap();
    assert!(system.contains("Ты — пользователь"));
    assert!(system.contains("Мне нужно"), "затравка попала в инструкцию");
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
    orch.server_status = ServerStatus::Connecting;
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

#[tokio::test]
async fn regenerate_without_user_message_is_noop() {
    // Чат с приветствием-ассистентом, но без сообщения пользователя — нечего
    // перегенерировать; команда не должна стартовать генерацию.
    let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>;
    let (_d, mut orch) = bare_orch();
    orch.backend = Some(backend);
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
    orch.backend = Some(backend);
    orch.server_status = ServerStatus::Connecting; // ещё не готов
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
    orch.backend = Some(backend);
    orch.server_status = ServerStatus::Connecting;
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
    orch.backend = Some(Arc::new(MockBackend::scripted(vec![])) as Arc<dyn EngineBackend>);

    orch.server_status = ServerStatus::Ready;
    assert!(orch.ready_backend().is_some());

    for status in [
        ServerStatus::Connecting,
        ServerStatus::NotConfigured,
        ServerStatus::Disconnected("боль".into()),
    ] {
        orch.server_status = status;
        assert!(orch.ready_backend().is_none());
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
    use crate::shared::api::backend::ToolCallDelta;
    // Раунд 1: вызов note_save → раунд 2: финальный текст.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
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
    use crate::shared::api::backend::ToolCallDelta;
    // python_exec выключен глобально (spawn_orch: python_enabled = false) —
    // даже если модель его вызовет, loop откажет, не исполняя.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
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
    use crate::shared::api::backend::ToolCallDelta;
    // Движок всегда просит инструмент — должен сработать лимит раундов.
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::ToolCall(ToolCallDelta {
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

#[tokio::test]
async fn create_profile_appears_in_profile_list() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    // Бутстрап создаёт один профиль по умолчанию.
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap();

    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "  Второй  ".into(),
            system_message: "sys".into(),
        })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
    )
    .await
    .unwrap();
    if let AppEvent::ProfileList(profiles) = list {
        assert!(profiles.iter().any(|p| p.name == "Второй")); // имя нормализовано
    }

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn delete_profile_cascades_to_its_chats() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Создаём второй профиль и узнаём его id.
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Второй".into(),
            system_message: "sys".into(),
        })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
    )
    .await
    .unwrap();
    let second_id = match list {
        AppEvent::ProfileList(profiles) => profiles.iter().find(|p| p.name == "Второй").unwrap().id,
        _ => unreachable!(),
    };

    // Создаём чат из второго профиля → всего два чата.
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(second_id),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 2),
    )
    .await
    .unwrap();

    // Удаляем второй профиль: его чат каскадно скрывается → остаётся один.
    cmd_tx.send(AppCommand::DeleteProfile(second_id)).unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 1),
    )
    .await
    .unwrap();

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn cannot_delete_last_profile() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let list = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let only_id = match list {
        AppEvent::ProfileList(p) => p[0].id,
        _ => unreachable!(),
    };

    cmd_tx.send(AppCommand::DeleteProfile(only_id)).unwrap();
    let err = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .unwrap();
    assert!(matches!(err, AppEvent::Error(_)));

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn bootstrap_emits_settings_snapshot() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    if let AppEvent::Settings { config, profiles } = ev {
        assert_eq!(config.schema_version, AppConfig::default().schema_version);
        assert_eq!(profiles.len(), 1, "дефолтный профиль в снимке");
    }
    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn update_config_persists_and_reemits_settings() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();

    let config = AppConfig {
        max_tool_rounds: 3,
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(config)))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.max_tool_rounds == 3),
    )
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Конфиг сохранён на диск.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    assert_eq!(reopened.json().load_config().unwrap().max_tool_rounds, 3);
}

#[tokio::test]
async fn update_profile_persists_edit() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    let id = match ev {
        AppEvent::Settings { profiles, .. } => profiles[0].id,
        _ => unreachable!(),
    };

    cmd_tx
        .send(AppCommand::UpdateProfile {
            id,
            edit: Box::new(ProfileEdit {
                system_message: Some("новое sys".into()),
                ..Default::default()
            }),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { profiles, .. }
            if profiles.iter().any(|p| p.default_system_message == "новое sys"))
    })
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn model_change_restarts_chat_server() {
    let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>;
    let sup = Arc::new(MockSupervisor::with_backend(Some(backend)));
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config: AppConfig::default(),
        supervisor: sup.clone(),
    }));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    // Бутстрап поднял сервер один раз.
    assert_eq!(sup.chat_call_count(), 1);

    // Смена модели → перезапуск (повторный apply_chat, spec §11.6 DoD).
    let config = AppConfig {
        engine: crate::shared::config::EngineSettings {
            model_path: Some("other.gguf".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(config)))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.engine.model_path.as_deref() == Some("other.gguf")),
    )
    .await
    .unwrap();
    assert_eq!(
        sup.chat_call_count(),
        2,
        "смена модели должна перезапустить сервер"
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn rag_add_indexes_files_and_reports_progress() {
    // Без chat-движка (RAG не зависит от него); эмбеддер даёт MockSupervisor.
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    // Папка с двумя поддерживаемыми файлами и одним неподдерживаемым.
    let docs = root.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("a.txt"), "кошки любят рыбу").unwrap();
    std::fs::write(docs.join("b.md"), "собаки любят кости").unwrap();
    std::fs::write(docs.join("c.bin"), "пропустить").unwrap();

    cmd_tx
        .send(AppCommand::RagAdd {
            path: docs.display().to_string(),
            recursive: false,
        })
        .unwrap();

    let started = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::RagProgress(RagProgress::Started { .. }))
    })
    .await
    .unwrap();
    assert!(matches!(
        started,
        AppEvent::RagProgress(RagProgress::Started { total: 2 })
    ));

    let finished = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::RagProgress(RagProgress::Finished { .. }))
    })
    .await
    .unwrap();
    match finished {
        AppEvent::RagProgress(RagProgress::Finished {
            files,
            chunks,
            errors,
            cancelled,
        }) => {
            assert_eq!(files, 2);
            assert_eq!(chunks, 2, "по одному чанку на файл");
            assert_eq!(errors, 0);
            assert!(!cancelled);
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Документы записаны под профилем активного чата (изоляция).
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(reopened.db().rag_count(chat.profile_id).unwrap(), 2);
}

#[tokio::test]
async fn rag_add_is_idempotent_on_reindex() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    let docs = root.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("a.txt"), "кошки любят рыбу").unwrap();
    std::fs::write(docs.join("b.md"), "собаки любят кости").unwrap();

    // Дважды индексируем ту же папку.
    for _ in 0..2 {
        cmd_tx
            .send(AppCommand::RagAdd {
                path: docs.display().to_string(),
                recursive: false,
            })
            .unwrap();
        wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::RagProgress(RagProgress::Finished { .. }))
        })
        .await
        .unwrap();
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(
        reopened.db().rag_count(chat.profile_id).unwrap(),
        2,
        "повторное добавление заменяет, а не дублирует"
    );
}

#[tokio::test]
async fn rag_delete_removes_indexed_documents() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    let docs = root.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("a.txt"), "кошки").unwrap();
    std::fs::write(docs.join("b.md"), "собаки").unwrap();

    cmd_tx
        .send(AppCommand::RagAdd {
            path: docs.display().to_string(),
            recursive: false,
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::RagProgress(RagProgress::Finished { .. }))
    })
    .await
    .unwrap();

    // Удаляем всю папку — оба файла уходят из базы.
    cmd_tx
        .send(AppCommand::RagDelete {
            path: docs.display().to_string(),
        })
        .unwrap();
    let removed = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::RagProgress(RagProgress::Removed { .. }))
    })
    .await
    .unwrap();
    assert!(matches!(
        removed,
        AppEvent::RagProgress(RagProgress::Removed { chunks: 2 })
    ));

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(reopened.db().rag_count(chat.profile_id).unwrap(), 0);
}

#[test]
fn build_request_puts_system_aside_and_maps_roles() {
    let mut p = Profile::new("X", "Ты — X.");
    p.greeting = Some("Здравствуйте!".into());
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::assistant("Здравствуйте!"));
    chat.push_message(Message::user("привет"));

    let req = build_request(&chat, SamplingConfig::default(), vec![]);
    assert_eq!(req.system.as_deref(), Some("Ты — X."));
    assert_eq!(req.messages.len(), 2);
}
