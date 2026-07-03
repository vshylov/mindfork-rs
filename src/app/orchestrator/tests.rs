//! Тесты оркестратора (без UI и реальной модели): автомат генерации, гонки,
//! agentic-loop, приоритеты семплинга, операции списка/профилей/RAG.

use super::engines::EngineManager;
use super::impersonation::{build_impersonation_request, swap_role_message};
use super::request::build_request;
use super::save_queue::SaveQueue;
use super::title::salvage_title_source;
use super::*;

use crate::app::events::RagProgress;
use crate::entities::message::{Message, MessageRole};
use crate::features::profiles::ProfileEdit;
use crate::shared::api::{ChatChunk, Embedder, EngineBackend};

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
    // Менеджер серверов: сразу «готов» с тестовым эмбеддером (как было у голого
    // оркестратора). chat-движок тесты при необходимости проставляют сами.
    let mut engines = EngineManager::new(
        Arc::new(MockSupervisor::with_backend(None)),
        status_tx,
        imp_status_tx,
    );
    engines.server_status = ServerStatus::Ready;
    engines.embedder = test_embedder();
    let orch = Orchestrator {
        evt_tx,
        engines,
        imp_cancel: None,
        imp_gen: None,
        imp_done_tx,
        storage,
        config,
        registry,
        title_tx,
        profiles: Vec::new(),
        chats: Vec::new(),
        active_id: None,
        gen_state: GenState::Idle,
        done_tx,
        rag_cancel: None,
        reflect_cancel: None,
        reflect_done_tx: unbounded_channel().0,
        reflect_failures: 0,
        consolidate_cancel: None,
        consolidate_counts: std::collections::HashMap::new(),
        consolidate_done_tx: unbounded_channel().0,
        consolidate_failures: 0,
        saves: SaveQueue::default(),
    };
    (dir, orch, evt_rx)
}

#[test]
fn inject_self_model_respects_flag_and_emptiness() {
    use super::generation::inject_self_model;
    use crate::entities::self_model::{NarrativeSegment, SelfModel, SelfModelParams};

    let pp = SelfModelParams::default();
    let mut m = SelfModel::new(Uuid::new_v4());
    m.summary = "ценю ясность".into();
    let now = chrono::Utc::now();
    let seg = |t: &str| NarrativeSegment {
        id: Uuid::new_v4(),
        text: t.into(),
        created_at: now,
    };

    // Выключено → система не меняется (протокол тоже не подмешивается).
    assert_eq!(
        inject_self_model(Some("S".into()), Some(&m), false, true, &pp, now, &[]),
        Some("S".into())
    );
    // Включено, протокол выкл, непустая модель → блок дописывается, протокола нет.
    let out = inject_self_model(Some("S".into()), Some(&m), true, false, &pp, now, &[]).unwrap();
    assert!(out.starts_with("S\n\n"));
    assert!(out.contains("ценю ясность"));
    assert!(!out.contains("угодливости"));
    // Включено, протокол выкл, модели нет, наблюдений нет → без изменений.
    assert_eq!(
        inject_self_model(Some("S".into()), None, true, false, &pp, now, &[]),
        Some("S".into())
    );
    // Модели нет, но есть наблюдения (self-заметки) → инъекция всё равно происходит.
    let obs = [seg("заметил склонность к краткости")];
    let only_obs = inject_self_model(None, None, true, false, &pp, now, &obs).unwrap();
    assert!(only_obs.contains("Недавние наблюдения:"));
    assert!(only_obs.contains("склонность к краткости"));
    // Пустая модель + пустые наблюдения, system=None → нечего подмешивать → None.
    let empty = SelfModel::new(Uuid::new_v4());
    assert_eq!(
        inject_self_model(None, Some(&empty), true, false, &pp, now, &[]),
        None
    );
    // Пустой system + непустая модель (протокол выкл) → блок становится системой.
    let only = inject_self_model(None, Some(&m), true, false, &pp, now, &[]).unwrap();
    assert!(only.contains("О себе: ценю ясность"));

    // Протокол вкл + пустая модель → протокол всё равно подмешивается (bootstrap).
    let boot =
        inject_self_model(Some("S".into()), Some(&empty), true, true, &pp, now, &[]).unwrap();
    assert!(boot.starts_with("S\n\n"));
    assert!(boot.contains("угодливости"));
    // Протокол вкл + непустая модель → и рендер, и протокол.
    let both = inject_self_model(None, Some(&m), true, true, &pp, now, &[]).unwrap();
    assert!(both.contains("ценю ясность"));
    assert!(both.contains("угодливости"));
}

#[test]
fn blend_self_notes_prioritizes_relevant_and_guarantees_freshest() {
    use super::generation::blend_self_notes;
    use crate::entities::note::Note;
    let p = Uuid::new_v4();
    let mk = |c: &str| Note::new(p, c, vec![]);
    let (r1, r2) = (mk("релевантное 1"), mk("релевантное 2"));
    let (f0, f1) = (mk("самое свежее"), mk("свежее 1"));
    let relevant = vec![r1.clone(), r2.clone()];
    let fresh = vec![f0.clone(), f1.clone()];

    // n=3: 2 релевантных + гарантированное самое свежее (f1 не влезает).
    let out = blend_self_notes(relevant.clone(), &fresh, 3);
    let ids: Vec<_> = out.iter().map(|n| n.id).collect();
    assert_eq!(out.len(), 3);
    assert!(ids.contains(&r1.id) && ids.contains(&r2.id));
    assert!(
        ids.contains(&f0.id),
        "самое свежее наблюдение гарантированно включено"
    );
    assert!(!ids.contains(&f1.id));

    // n=2 при 2 релевантных: последнюю релевантную теснит самое свежее.
    let out = blend_self_notes(relevant, &fresh, 2);
    let ids: Vec<_> = out.iter().map(|n| n.id).collect();
    assert_eq!(out.len(), 2);
    assert!(ids.contains(&r1.id));
    assert!(ids.contains(&f0.id));
    assert!(!ids.contains(&r2.id));

    // Дедуп: если самое свежее уже среди релевантных — не дублируется.
    let out = blend_self_notes(vec![f0.clone(), r1.clone()], &fresh, 3);
    assert_eq!(out.iter().filter(|n| n.id == f0.id).count(), 1);
}

#[tokio::test]
async fn injection_recent_surfaces_relevant_over_fresh() {
    // Ярус 2: инъекция по релевантности поднимает СТАРОЕ, но релевантное запросу
    // наблюдение — то, что чистая свежесть потеряла бы.
    use super::generation::injection_recent;
    use crate::entities::note::Note;
    use crate::entities::self_model::SelfModelParams;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    use chrono::{Duration, Utc};

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    let embedder = MockEmbedder::new(16);
    let profile = Uuid::new_v4();

    // X — старое (10 дней назад), тема «xxxx». Затем 4 свежих Y (тема «yyyy»),
    // вытесняющих X из свежести (narrative_in_prompt=3).
    let now = Utc::now();
    let mut seeds: Vec<(String, chrono::DateTime<Utc>)> =
        vec![("xxxx старое наблюдение".into(), now - Duration::days(10))];
    for i in 0..4 {
        seeds.push((format!("yyyy свежее {i}"), now));
    }
    for (content, at) in &seeds {
        let note = Note {
            id: Uuid::new_v4(),
            profile_id: profile,
            content: content.clone(),
            tags: vec![SELF_NOTE_TAG.to_string()],
            created_at: *at,
            updated_at: *at,
        };
        storage.db().note_insert(&note).unwrap();
        let emb = embedder
            .embed(vec![content.clone()])
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        storage
            .db()
            .note_vector_upsert(note.id, profile, &emb)
            .unwrap();
    }
    let params = SelfModelParams::default();

    // Запрос про «xxxx» → старое релевантное наблюдение поднято (хоть не свежайшее).
    let recent = injection_recent(&storage, &embedder, profile, true, "xxxx", &params).await;
    assert!(
        recent.iter().any(|s| s.text.contains("xxxx старое")),
        "релевантное старое наблюдение должно быть поднято: {recent:?}"
    );
    // Запрос про «yyyy» → нерелевантное старое X не поднимается.
    let recent = injection_recent(&storage, &embedder, profile, true, "yyyy", &params).await;
    assert!(!recent.iter().any(|s| s.text.contains("xxxx")));
    // Инъекция выключена → пусто.
    assert!(
        injection_recent(&storage, &embedder, profile, false, "xxxx", &params)
            .await
            .is_empty()
    );
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
async fn emits_token_counter_during_generation() {
    use crate::shared::api::contract::TokenUsage;
    // Две текстовые дельты (live-счёт = 2), затем точный usage от сервера (= 5).
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("При".into()),
        ChatChunk::Text("вет".into()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 12,
            completion_tokens: 5,
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
    use crate::shared::api::contract::ToolCallDelta;
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
    use crate::shared::api::contract::ToolCallDelta;
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
    use crate::shared::api::contract::ToolCallDelta;
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

/// Включает в профиле весь каталог инструментов (в т.ч. управляющие followup/
/// rewrite) — для тестов управляющих инструментов. Возвращает id профиля.
async fn enable_all_tools(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
) -> Uuid {
    use crate::features::tools::all_tool_ids;
    let pl = wait_for(evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v[0].id,
        _ => unreachable!(),
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(all_tool_ids()),
                ..Default::default()
            }),
        })
        .unwrap();
    pid
}

#[tokio::test]
async fn followup_tool_makes_two_assistant_messages() {
    use crate::shared::api::contract::ToolCallDelta;
    // Раунд 1: текст + вызов send_followup_message → раунд 2: второе сообщение.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("Первое сообщение.".into()),
            ChatChunk::ToolCall(ToolCallDelta {
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

/// Реальный chat-движок из `MINDFORK_ENGINE_URL` (для end-to-end смоуков на живой
/// модели). `None` — переменная не задана (тест пропускается).
fn live_backend() -> Option<Arc<dyn EngineBackend>> {
    let url = std::env::var("MINDFORK_ENGINE_URL").ok()?;
    Some(Arc::new(crate::shared::api::OpenAiClient::new(url)) as Arc<dyn EngineBackend>)
}

/// Реальный эмбеддер из `MINDFORK_EMBED_URL` (для живых смоуков — bge-m3 и т.п.);
/// `None`, если не задан → живой смоук берёт тестовый `MockEmbedder`.
fn live_embedder() -> Option<Arc<dyn Embedder>> {
    let url = std::env::var("MINDFORK_EMBED_URL").ok()?;
    Some(Arc::new(crate::shared::api::OpenAiClient::new(url)) as Arc<dyn Embedder>)
}

/// Кортеж поднятого оркестратора (как у [`spawn_orch`]): каталог данных, канал
/// команд, приёмник событий, handle петли.
type OrchHandle = (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
);

/// Поднимает оркестратор для живого смоука: chat из `MINDFORK_ENGINE_URL`, эмбеддер
/// из `MINDFORK_EMBED_URL` (реальный сервер; иначе детерминированный `MockEmbedder`).
/// `None`, если `MINDFORK_ENGINE_URL` не задан (смоук пропускается).
fn spawn_orch_live() -> Option<OrchHandle> {
    let backend = live_backend()?;
    let embedder = live_embedder();
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config: AppConfig::default(),
        supervisor: Arc::new(MockSupervisor::with_backend_and_embedder(
            Some(backend),
            embedder,
        )),
    };
    let handle = tokio::spawn(run(deps));
    Some((dir, cmd_tx, evt_rx, handle))
}

/// Дренирует события до `Finished` (или закрытия), помечая, встретилось ли
/// `pred`-событие по пути. Для end-to-end смоуков управляющих инструментов.
async fn drain_until_finished<F: Fn(&AppEvent) -> bool>(
    rx: &mut UnboundedReceiver<AppEvent>,
    pred: F,
) -> bool {
    let mut saw = false;
    while let Some(ev) = rx.recv().await {
        if pred(&ev) {
            saw = true;
        }
        if matches!(ev, AppEvent::Finished { .. }) {
            break;
        }
    }
    saw
}

/// End-to-end на живой модели: с включённым `send_followup_message` ассистент
/// пишет **второе сообщение** отдельным пузырём. Проверяем и сигнал UI
/// (`AssistantContinue`), и итоговую структуру чата (`Message.new_bubble`).
/// Модель нестабильна — тест `#[ignore]`, гоняется вручную против Gemma/Qwen.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn followup_tool_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage(
            "Ответь короткой первой репликой-приветствием, затем ОБЯЗАТЕЛЬНО вызови \
             инструмент send_followup_message и напиши вторую реплику с интересным \
             фактом о космосе."
                .into(),
        ))
        .unwrap();

    let saw_continue = drain_until_finished(&mut evt_rx, |e| {
        matches!(e, AppEvent::AssistantContinue { .. })
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let new_bubbles = chat.messages.iter().filter(|m| m.new_bubble).count();
    eprintln!(
        "followup e2e: saw_continue={saw_continue}, new_bubble={new_bubbles}, \
         сообщений={}",
        chat.messages.len()
    );
    for (i, m) in chat.messages.iter().enumerate() {
        eprintln!(
            "  [{i}] {:?} new_bubble={} tools={:?} text={:?}",
            m.role,
            m.new_bubble,
            m.tool_calls.iter().map(|t| &t.name).collect::<Vec<_>>(),
            m.text.chars().take(60).collect::<String>()
        );
    }
    assert!(
        saw_continue && new_bubbles >= 1,
        "ожидали второе сообщение отдельным пузырём (followup)"
    );
}

/// End-to-end на живой модели: с включённым `rewrite_current_message` ассистент
/// отбрасывает начатый ответ и пишет заново; отброшенное уходит в `Chat.deleted`.
/// Проверяем сигнал UI (`AssistantRewrite`) и непустой архив удалённого.
/// Модель нестабильна — тест `#[ignore]`, гоняется вручную.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn rewrite_tool_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage(
            "Продемонстрируй инструмент rewrite_current_message строго по шагам, НИ ОДИН \
             не пропуская. Шаг 1: напиши ровно «2+2=5». Шаг 2 (ОБЯЗАТЕЛЬНЫЙ): сразу \
             вызови инструмент rewrite_current_message — без него задание не выполнено. \
             Шаг 3: после вызова напиши правильный ответ «2+2=4». Самое важное — \
             обязательно вызвать rewrite_current_message между шагами 1 и 3."
                .into(),
        ))
        .unwrap();

    let saw_rewrite = drain_until_finished(&mut evt_rx, |e| {
        matches!(e, AppEvent::AssistantRewrite { .. })
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    eprintln!(
        "rewrite e2e: saw_rewrite={saw_rewrite}, deleted={}, сообщений={}",
        chat.deleted.len(),
        chat.messages.len()
    );
    for (i, m) in chat.messages.iter().enumerate() {
        eprintln!(
            "  msg[{i}] {:?} text={:?}",
            m.role,
            m.text.chars().take(60).collect::<String>()
        );
    }
    assert!(
        saw_rewrite && !chat.deleted.is_empty(),
        "ожидали отброшенный (переписанный) ответ в Chat.deleted"
    );
}

/// Прогоняет один ход: шлёт сообщение, дренирует события до `Finished`, собирая
/// текст ответа и имена вызванных инструментов. Для live-смоуков.
#[cfg(test)]
async fn run_turn_live(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> (String, Vec<String>) {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    let mut tools = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => out.push_str(text),
            AppEvent::ToolCall { name, .. } => tools.push(name.clone()),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    (out, tools)
}

/// Как [`run_turn_live`], но собирает пары (имя инструмента, результат) — чтобы
/// проверить текст результата (напр. срабатывание ворот `add_insight`).
async fn run_turn_capture(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> (String, Vec<(String, String)>) {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    let mut calls = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => out.push_str(text),
            AppEvent::ToolCall { name, result, .. } => calls.push((name.clone(), result.clone())),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    (out, calls)
}

/// End-to-end зонд SelfModel на живой модели (две сессии, один профиль):
/// 1) сессия 1 — сообщаем факты о себе и просим зафиксировать в «модели себя»
///    (ожидаем вызовы `update_self_model`/`update_user_model`, запись в БД);
/// 2) сессия 2 (новый чат тем же профилем) — спрашиваем «что ты обо мне помнишь»;
///    «модель себя» подмешана в системный промпт → ожидаем припоминание.
/// Поведение модели нестабильно — тест `#[ignore]`, гоняется вручную; ассертим
/// **механизм** (БД заполнена), а текст припоминания печатаем для оценки.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // --- Сессия 1: сообщаем факты и просим зафиксировать модель себя. ---
    let (s1_text, s1_tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Меня зовут Владимир, я пишу на Rust и не люблю многословие. \
         Запомни это: вызови update_user_model (черты, интересы) и update_self_model \
         (краткое описание себя и цель — помогать мне кратко и по делу).",
    )
    .await;
    eprintln!("сессия 1: инструменты={s1_tools:?}\nтекст={s1_text:?}\n");

    // --- Сессия 2: новый чат тем же профилем, проверяем припоминание. ---
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let (s2_text, s2_tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Что ты обо мне помнишь и какие у тебя цели в общении со мной?",
    )
    .await;
    eprintln!("сессия 2: инструменты={s2_tools:?}\nтекст={s2_text:?}\n");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Механизм: после сессии 1 модель себя профиля непуста и сохранена на диск.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let model = reopened.db().self_model_get(pid).unwrap();
    eprintln!("self_model в БД: {model:#?}");
    let model = model.expect("ожидали сохранённую модель себя после сессии 1");
    assert!(
        !model.is_empty(),
        "ожидали непустую модель себя (модель должна была вызвать update_*)"
    );
    // Хотя бы один из мутаторов реально вызван.
    assert!(
        s1_tools
            .iter()
            .any(|t| t == "update_self_model" || t == "update_user_model"),
        "ожидали вызов update_self_model/update_user_model в сессии 1"
    );
}

/// End-to-end зонд наблюдений: просим модель зафиксировать наблюдение через
/// `add_insight` — ожидаем self-заметку (@self) в БД (нарратив переехал в заметки,
/// Ярус 1 «нарратив как заметки»). `#[ignore]`, вручную.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_insight_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    let (text, tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Я заметил, что иногда прошу кратко, а иногда — подробно. \
         Зафиксируй это наблюдение в своих наблюдениях: вызови инструмент add_insight \
         с коротким описанием этого противоречия.",
    )
    .await;
    eprintln!("insight: инструменты={tools:?}\nтекст={text:?}\n");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Наблюдение — self-заметка (@self), а не запись в блобе модели.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let self_notes = reopened
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    eprintln!("self-заметки (наблюдения) в БД: {self_notes:#?}");
    assert!(
        !self_notes.is_empty(),
        "ожидали хотя бы одну self-заметку (@self) — наблюдение от add_insight"
    );
    assert!(
        tools.iter().any(|t| t == "add_insight"),
        "ожидали вызов add_insight"
    );
}

/// End-to-end авто-рефлексии (Tier 3) на живой модели: `auto_reflect_every=1` →
/// после первого же ответа ассистента в фоне запускается рефлексия, которая сама
/// обновляет «модель себя». Рефлексия молчалива (нет UI-события) — ждём появления
/// данных в БД опросом. `#[ignore]`, вручную.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn auto_reflect_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let mut config = AppConfig::default();
    config.self_model.auto_reflect_every = 1; // рефлексия после каждого ответа
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Обычная отправка: сообщаем факты, ассистент отвечает (а затем фоновая
    // рефлексия должна сама зафиксировать «модель себя»).
    let (_t, _tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Привет! Меня зовут Владимир, пишу на Rust и ценю краткость. Просто ответь \
         коротким приветствием.",
    )
    .await;

    // Ждём, пока фоновая рефлексия что-то запишет (опрос БД до ~60с): блоб модели
    // (summary/цели/собеседник) ИЛИ наблюдение self-заметкой (@self, Ярус 1).
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let mut model = None;
    let mut self_notes = Vec::new();
    for _ in 0..120 {
        let db = Storage::open(Paths::with_root(&root)).unwrap();
        let m = db.db().self_model_get(pid).unwrap();
        self_notes = db
            .db()
            .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap();
        let blob_nonempty = m.as_ref().map(|m| !m.is_empty()).unwrap_or(false);
        if blob_nonempty || !self_notes.is_empty() {
            model = m;
            break;
        }
        drop(db);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!("auto-reflect: self_model={model:#?}\nself-заметки={self_notes:#?}");
    let blob_nonempty = model.as_ref().map(|m| !m.is_empty()).unwrap_or(false);
    assert!(
        blob_nonempty || !self_notes.is_empty(),
        "ожидали, что фоновая авто-рефлексия заполнит модель себя (блоб или наблюдение-заметку)"
    );
}

/// End-to-end зонд **ворот** (ядро гипотезы Яруса 1 «нарратив как заметки»): модель
/// записывает наблюдение (`add_insight` → self-заметка @self), затем почти-дубль —
/// ворота `add_insight` показывают похожее существующее наблюдение с подсказкой
/// переписать его через `note_revise`/`note_supersede` вместо копии. Ассертим
/// **механизм** (self-заметки создаются; ворота срабатывают детерминированно —
/// эмбеддер в тестах `MockEmbedder`, наблюдение #1 уже есть); **решение** модели
/// интегрировать печатаем для go/no-go (поведение нестабильно). Запуск (нужен
/// живой сервер + возможно эмбеддер):
/// `MINDFORK_ENGINE_URL=…/v1 cargo test self_model_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_gate_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    // Реальный chat + эмбеддер (MINDFORK_ENGINE_URL / MINDFORK_EMBED_URL) — ворота
    // работают на настоящих эмбеддингах (bge-m3 и т.п.), а не на MockEmbedder.
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Сессия 1: записываем наблюдение → self-заметка #1.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши в свои наблюдения (вызови add_insight): я склонен просить краткие ответы.",
    )
    .await;
    eprintln!("сессия 1: инструменты={tools1:?}");

    // Сессия 2: почти-дубль — ворота add_insight должны показать наблюдение #1.
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Запиши ещё одно, очень похожее наблюдение (вызови add_insight): пользователь \
         предпочитает лаконичные, краткие ответы. Если инструмент покажет похожее \
         наблюдение — реши сам, переписать ли его (note_revise/note_supersede) или \
         оставить оба.",
    )
    .await;
    eprintln!("сессия 2: текст={t2:?}\nвызовы={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Механизм: add_insight в сессии 1 создал self-заметку (@self).
    assert!(
        tools1.iter().any(|t| t == "add_insight"),
        "сессия 1: ожидали вызов add_insight"
    );
    let self_notes = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    eprintln!(
        "self-заметок в БД: {} — {:#?}",
        self_notes.len(),
        self_notes
            .iter()
            .map(|n| n.content.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !self_notes.is_empty(),
        "ожидали self-заметки (@self) от add_insight"
    );

    // Ворота: результат add_insight в сессии 2 показал похожее наблюдение?
    let gate_fired = calls2
        .iter()
        .any(|(n, r)| n == "add_insight" && r.contains("Похожие наблюдения"));
    // С тестовым MockEmbedder (MINDFORK_EMBED_URL не задан) ворота детерминированны:
    // если модель вызвала add_insight, они ОБЯЗАНЫ сработать (наблюдение #1 уже есть).
    // С реальным эмбеддером срабатывание зависит от его настройки (напр. llama-server
    // нужен `--embeddings`), а при недоступности ворота мягко деградируют в пусто —
    // поэтому там это лишь диагностика, не жёсткая проверка.
    let real_embedder = std::env::var("MINDFORK_EMBED_URL").is_ok();
    if !real_embedder && calls2.iter().any(|(n, _)| n == "add_insight") {
        assert!(
            gate_fired,
            "ворота add_insight должны были показать похожее наблюдение (MockEmbedder, ядро гипотезы): {calls2:?}"
        );
    }
    // Интеграция почти-дубля: перепись/замещение/слияние наблюдений.
    let integrated = calls2
        .iter()
        .any(|(n, _)| n == "note_revise" || n == "note_supersede" || n == "note_merge");
    eprintln!(
        "ворота показали похожее: {gate_fired} (реальный эмбеддер: {real_embedder}); \
         модель интегрировала (note_revise/supersede/merge): {integrated}"
    );
    // Модель должна была как-то тронуть наблюдения (иначе гипотеза не проверяется).
    assert!(
        calls2.iter().any(|(n, _)| {
            n == "add_insight" || n == "note_revise" || n == "note_supersede" || n == "note_merge"
        }),
        "сессия 2: ожидали add_insight/note_revise/note_supersede/note_merge"
    );
}

/// End-to-end зонд **графа над наблюдениями** (Ярус 2, шаг B): модель записывает два
/// соотносящихся наблюдения, затем связывает их (`note_link`). Ассертим механизм
/// (наблюдения-заметки создаются; модель осмотрела модель себя / связала); появление
/// связи в графе печатаем для go/no-go (поведение нестабильно). Инъекция по
/// релевантности проверена детерминированно (`injection_recent_surfaces_relevant_over_fresh`)
/// + ручной мульти-сессионный прогон пользователя. `#[ignore]`, вручную:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test self_model_graph_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_graph_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Два соотносящихся (противоречащих) наблюдения.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение (add_insight): я ценю краткость в ответах.",
    )
    .await;
    let (_t2, tools2) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши ещё одно наблюдение (add_insight): но иногда я даю слишком многословные ответы.",
    )
    .await;
    eprintln!("наблюдения: {tools1:?} + {tools2:?}");

    // Просим осмотреть модель себя и связать противоречащие наблюдения.
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Посмотри свои наблюдения (get_self_model). Если два из них противоречат друг \
         другу — свяжи их инструментом note_link (relation=contradicts) по полному id.",
    )
    .await;
    eprintln!("связывание: текст={t3:?}\nвызовы={calls3:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let self_notes = reopened
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    let links = reopened.db().note_links_all(pid).unwrap();
    eprintln!(
        "self-заметок: {}; связей в графе наблюдений: {} — {links:?}",
        self_notes.len(),
        links.len()
    );

    // Механизм: наблюдения-заметки созданы.
    assert!(self_notes.len() >= 2, "ожидали ≥2 наблюдения-заметки");
    let linked = calls3.iter().any(|(n, _)| n == "note_link");
    eprintln!(
        "модель вызвала note_link: {linked}; связей появилось: {}",
        links.len()
    );
    // Модель должна была осмотреть себя и/или связать (иначе граф не проверен).
    assert!(
        calls3
            .iter()
            .any(|(n, _)| n == "get_self_model" || n == "note_link"),
        "сессия 3: ожидали get_self_model/note_link"
    );
}

/// End-to-end зонд **ворот родственных черт** `user_model` (Ярус 2, шаг C): модель
/// добавляет черту собеседника (`update_user_model` с `add_traits`), затем близкую —
/// ворота `add_traits` показывают родственную черту и просят решить (дубль/противоречие).
/// Ассертим **механизм** (черта записана в `perceived_traits`; во второй сессии снова
/// вызван `update_user_model`); срабатывание ворот и решение модели печатаем для
/// go/no-go. **Порог ворот 0.72** (откалиброван на bge-m3, в отличие от беспороговых
/// ворот `add_insight`), поэтому на реальном эмбеддере срабатывание зависит от близости
/// сгенерированных моделью формулировок — здесь это диагностика, не жёсткая проверка.
/// `#[ignore]`, вручную:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test trait_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn trait_gate_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Сессия 1: добавляем черту собеседника → user_model.perceived_traits.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Обнови модель собеседника (вызови update_user_model): добавь черту (add_traits) \
         — «ценит краткость в ответах».",
    )
    .await;
    eprintln!("сессия 1: инструменты={tools1:?}");

    // Сессия 2: очень похожая черта — ворота add_traits должны предупредить о дубле.
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Обнови модель собеседника ещё раз (update_user_model): добавь очень похожую \
         черту (add_traits) — «любит лаконичность». Если инструмент предупредит о \
         почти-дубле — реши сам, объединить ли их через remove_traits.",
    )
    .await;
    eprintln!("сессия 2: текст={t2:?}\nвызовы={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Механизм: update_user_model в сессии 1 записал черту.
    assert!(
        tools1.iter().any(|t| t == "update_user_model"),
        "сессия 1: ожидали вызов update_user_model"
    );
    let stored = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .self_model_get(pid)
        .unwrap();
    let traits = stored
        .as_ref()
        .map(|m| m.user_model.perceived_traits.clone())
        .unwrap_or_default();
    eprintln!("черты собеседника в БД: {traits:?}");
    assert!(
        !traits.is_empty(),
        "ожидали ≥1 черту в user_model от update_user_model"
    );

    // Ворота: результат update_user_model в сессии 2 показал родственную черту?
    // (`remove_traits` — аргумент update_user_model, не отдельное имя инструмента,
    // поэтому интеграцию читаем по итоговому состоянию БД, а не по имени вызова.)
    let gate_fired = calls2
        .iter()
        .any(|(n, r)| n == "update_user_model" && r.contains("Родственные черты"));
    // Интеграция почти-дубля: модель свела перефразы к одной черте (не оставила обе).
    let integrated = traits.len() <= 1;
    eprintln!(
        "ворота предупредили о похожей черте: {gate_fired}; \
         модель свела к одной черте (не копит перефразы): {integrated}"
    );

    // Модель должна была снова тронуть модель собеседника (иначе ворота не проверены).
    assert!(
        calls2.iter().any(|(n, _)| n == "update_user_model"),
        "сессия 2: ожидали update_user_model"
    );
}

#[tokio::test]
async fn update_self_model_persists_and_reemits() {
    use crate::entities::self_model::SelfModelEdit;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Правка из UI-редактора (без модели — оркестратор создаёт её на месте).
    cmd_tx
        .send(AppCommand::UpdateSelfModel(SelfModelEdit::SetSummary(
            "ценю ясность".into(),
        )))
        .unwrap();

    // Переэмит снимка отражает правку.
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::SelfModelView(_)))
        .await
        .unwrap();
    match ev {
        AppEvent::SelfModelView(m) => {
            assert_eq!(m.expect("ожидали модель").summary, "ценю ясность");
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Персистентность: запись видна после перезапуска.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let pid = reopened.json().load_profiles().unwrap()[0].id;
    let stored = reopened.db().self_model_get(pid).unwrap().unwrap();
    assert_eq!(stored.summary, "ценю ясность");
}

/// Готовит голый оркестратор с профилем + активным чатом (для F3-правок).
fn orch_with_active_profile() -> (tempfile::TempDir, Orchestrator, Uuid) {
    let (dir, mut orch) = bare_orch();
    let profile = Profile::new("P", "sys");
    let pid = profile.id;
    let chat = Chat::from_profile(&profile, "t");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, pid)
}

#[test]
fn f3_delete_insight_removes_self_note() {
    use crate::entities::self_model::SelfModelEdit;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let (_d, orch, pid) = orch_with_active_profile();
    // Наблюдение — self-заметка (@self).
    let note = crate::entities::note::Note::new(pid, "наблюдение", vec![SELF_NOTE_TAG.to_string()]);
    let nid = note.id;
    orch.storage.db().note_insert(&note).unwrap();

    // F3 «удалить наблюдение» → удаление self-заметки (не правка блоба).
    orch.handle_update_self_model(SelfModelEdit::DeleteInsight(nid));
    assert!(
        orch.storage
            .db()
            .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn f3_clear_removes_self_notes_and_blob() {
    use crate::entities::self_model::SelfModelEdit;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let (_d, orch, pid) = orch_with_active_profile();
    // Наблюдение-заметка + непустой блоб модели.
    orch.storage
        .db()
        .note_insert(&crate::entities::note::Note::new(
            pid,
            "наблюдение",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    orch.storage
        .db()
        .self_model_update(pid, |m| {
            m.summary = "о себе".into();
            true
        })
        .unwrap();

    orch.handle_update_self_model(SelfModelEdit::Clear);
    // Self-заметки снесены, блоб очищен.
    assert!(
        orch.storage
            .db()
            .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap()
            .is_empty()
    );
    let stored = orch.storage.db().self_model_get(pid).unwrap();
    assert!(stored.map(|m| m.summary.is_empty()).unwrap_or(true));
}

/// Готовит оркестратор с чатом (user+assistant) и профилем, включившим модель себя;
/// `auto_reflect_every=1`. Возвращает `(dir, orch, chat_id)`.
fn orch_ready_for_reflection() -> (tempfile::TempDir, Orchestrator, Uuid) {
    use crate::features::tools::self_model::GET_SELF_MODEL_ID;
    let (dir, mut orch) = bare_orch();
    orch.config.self_model.auto_reflect_every = 1;
    let mut profile = Profile::new("P", "sys");
    profile.enabled_tools = vec![GET_SELF_MODEL_ID.into()];
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуй"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, chat_id)
}

#[tokio::test]
async fn auto_reflect_advances_watermark_on_spawn() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    // Готовый движок — рефлексия реально спавнится (пустой скрипт → задача завершится).
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>);

    orch.maybe_auto_reflect(chat_id);

    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    // Ватермарк сдвинут на всю длину истории (окно охвачено), рефлексия запущена.
    assert_eq!(chat.reflected_upto, Some(2));
    assert!(chat.reflected_at.is_some());
    assert!(orch.reflect_cancel.is_some());
}

#[tokio::test]
async fn auto_reflect_keeps_watermark_when_server_not_ready() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    // Движок не задан → backend_if_ready вернёт Err → пропуск БЕЗ сдвига ватермарка.
    orch.maybe_auto_reflect(chat_id);

    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    assert_eq!(chat.reflected_upto, None); // цикл не потерян — повторим позже
    assert!(orch.reflect_cancel.is_none());
}

#[tokio::test]
async fn reflect_failures_alert_once_then_reset() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let saw_error = |rx: &mut UnboundedReceiver<AppEvent>| {
        let mut seen = false;
        while let Ok(e) = rx.try_recv() {
            if matches!(e, AppEvent::Error(_)) {
                seen = true;
            }
        }
        seen
    };
    // Две неудачи подряд — в UI ещё тихо (наблюдаемость без спама).
    orch.handle_reflect_done(Err("boom".into()));
    orch.handle_reflect_done(Err("boom".into()));
    assert!(!saw_error(&mut rx));
    // Третья подряд — одна ошибка.
    orch.handle_reflect_done(Err("boom".into()));
    assert!(saw_error(&mut rx));
    assert_eq!(orch.reflect_failures, 3);
    // Успех сбрасывает серию и шлёт SelfModelChanged.
    orch.handle_reflect_done(Ok(()));
    assert_eq!(orch.reflect_failures, 0);
    let mut changed = false;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, AppEvent::SelfModelChanged) {
            changed = true;
        }
    }
    assert!(changed);
}

#[tokio::test]
async fn handle_done_signals_self_model_changed_on_self_model_tool_call() {
    use crate::entities::message::ToolCallRecord;
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    let gen_id = Uuid::new_v4();
    orch.gen_state
        .begin(gen_id, tokio_util::sync::CancellationToken::new());

    // Ответ ассистента с вызовом self-model-инструмента → SelfModelChanged.
    let mut msg = Message::assistant("готово");
    msg.tool_calls = vec![ToolCallRecord {
        id: "c1".into(),
        name: "update_self_model".into(),
        arguments: serde_json::json!({}),
        result: Some("ok".into()),
    }];
    orch.handle_done(super::generation::GenResult {
        id: gen_id,
        chat_id,
        messages: vec![msg],
        effects: vec![],
        deleted: vec![],
    });
    let mut changed = false;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, AppEvent::SelfModelChanged) {
            changed = true;
        }
    }
    assert!(changed);
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
            managed: crate::shared::config::ManagedSettings {
                model_path: Some("other.gguf".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(config)))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.engine.managed.model_path.as_deref() == Some("other.gguf")),
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

#[tokio::test]
async fn rag_list_reports_sources() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    let docs = root.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("a.txt"), "кошки любят рыбу").unwrap();
    std::fs::write(docs.join("b.md"), "собаки любят кости").unwrap();
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

    cmd_tx.send(AppCommand::RagList).unwrap();
    let listed = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::RagProgress(RagProgress::Listed { .. }))
    })
    .await
    .unwrap();
    match listed {
        AppEvent::RagProgress(RagProgress::Listed { sources }) => {
            assert_eq!(sources.len(), 2, "два источника в базе");
            assert!(sources.iter().all(|s| s.chunks == 1));
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn rag_rebuild_reindexes_from_stored_content_without_file() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    let file = root.join("doc.txt");
    std::fs::write(&file, "кошки любят рыбу").unwrap();
    cmd_tx
        .send(AppCommand::RagAdd {
            path: file.display().to_string(),
            recursive: false,
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::RagProgress(RagProgress::Finished { .. }))
    })
    .await
    .unwrap();

    // Удаляем файл с диска — реиндексация должна опереться на сохранённый исходник.
    std::fs::remove_file(&file).unwrap();

    cmd_tx.send(AppCommand::RagRebuild).unwrap();
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
            assert_eq!(files, 1, "один источник реиндексирован");
            assert_eq!(chunks, 1);
            assert_eq!(errors, 0, "исходник взят из БД, а не с диска");
            assert!(!cancelled);
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(reopened.db().rag_count(chat.profile_id).unwrap(), 1);
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
