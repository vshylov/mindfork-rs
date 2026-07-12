//! Тесты оркестратора — живые #[ignore] e2e-смоуки (Gemma/bge-m3 через MINDFORK_*_URL). Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/history/refactoring-god-objects.md, этап 3.

use super::*;

/// Ярус 1 i18n (docs/i18n.md, go/no-go): профиль с языком служебного каркаса `En` —
/// авто-название англоязычной переписки английское, БЕЗ кириллицы. Свежий профиль
/// (bootstrap-профиль залочен: у него уже есть дефолтный чат), ставим ему En, заводим
/// чат, гоняем английский ход и авто-название. `#[ignore]`, вручную против живой модели.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn i18n_en_profile_title_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // Свежий профиль (у bootstrap-профиля есть дефолтный чат → его язык залочен).
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "English".into(),
            system_message: "You are a helpful assistant. Reply in English.".into(),
        })
        .unwrap();
    let pl = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(v) if v.len() >= 2),
    )
    .await
    .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v.last().unwrap().id,
        _ => unreachable!(),
    };
    // Язык каркаса En (профиль свежий, без данных → смена разрешена).
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                language: Some(crate::shared::i18n::Lang::En),
                ..Default::default()
            }),
        })
        .unwrap();
    // Новый чат под этим профилем (станет активным).
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let act = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match act {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };
    // Английский ход.
    let (reply, _) =
        run_turn_live(&cmd_tx, &mut evt_rx, "Tell me a fun fact about the Moon.").await;
    eprintln!("en reply: {:?}", reply.chars().take(80).collect::<String>());
    // Авто-название по переписке (дайджест/системное сообщение — на языке каркаса).
    cmd_tx.send(AppCommand::AutoRenameChat(chat_id)).unwrap();
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    let title = match renamed {
        AppEvent::ChatRenamed { title, .. } => title,
        _ => unreachable!(),
    };
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!("en auto-title: {title:?}");
    let has_cyr = title
        .chars()
        .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c));
    assert!(!title.trim().is_empty(), "пустой заголовок");
    assert!(
        !has_cyr,
        "заголовок англоязычной переписки содержит кириллицу: {title:?}"
    );
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

/// Живой смоук Яруса 2 i18n (docs/i18n.md, группа 2a): en-зеркало
/// [`self_model_gate_e2e_live`]. Профиль с языком каркаса `En` + все инструменты; ход
/// на английском записывает наблюдение, почти-дубль поднимает ворота `add_insight` —
/// **и текст ворот, и весь результат инструмента должны быть на английском** (ключевой
/// критерий Яруса 2: результаты инструментов локализованы, без кириллицы). Реальный
/// эмбеддер (`MINDFORK_EMBED_URL`) нужен, чтобы ворота сработали. Запуск:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test self_model_gate_en_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_gate_en_e2e_live() {
    use crate::features::tools::all_tool_ids;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // Свежий профиль (bootstrap-профиль залочен на Ru) → ставим язык каркаса En.
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "English".into(),
            system_message: "You are a helpful assistant. Reply in English.".into(),
        })
        .unwrap();
    let pl = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(v) if v.len() >= 2),
    )
    .await
    .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v.last().unwrap().id,
        _ => unreachable!(),
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                language: Some(crate::shared::i18n::Lang::En),
                enabled_tools: Some(all_tool_ids()),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Ход 1: записываем наблюдение (add_insight → self-заметка).
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Record an observation about me (call add_insight): I tend to ask for concise answers.",
    )
    .await;
    eprintln!("turn 1 tools: {tools1:?}");

    // Ход 2: почти-дубль — ворота add_insight показывают наблюдение #1 (по-английски).
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Record another very similar observation (call add_insight): the user prefers brief, \
         concise replies. If the tool shows a similar observation, decide yourself whether to \
         rewrite it (note_revise/note_supersede) or keep both.",
    )
    .await;
    eprintln!("turn 2 text={t2:?}\ncalls={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        tools1.iter().any(|t| t == "add_insight"),
        "turn 1: expected add_insight call"
    );
    // Результаты инструментов не содержат кириллицы (каркас переведён, Ярус 2).
    let has_cyr = |s: &str| {
        s.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
    };
    for (n, r) in &calls2 {
        assert!(!has_cyr(r), "tool {n} result has cyrillic: {r:?}");
    }
    // Ворота (реальный эмбеддер): текст — английский шаблон «Similar observations …».
    let gate_fired = calls2
        .iter()
        .any(|(n, r)| n == "add_insight" && r.contains("Similar observations"));
    let real_embedder = std::env::var("MINDFORK_EMBED_URL").is_ok();
    eprintln!("gate fired (English): {gate_fired}; real embedder: {real_embedder}");
    if real_embedder && calls2.iter().any(|(n, _)| n == "add_insight") {
        assert!(
            gate_fired,
            "en gate should have surfaced a similar observation in English: {calls2:?}"
        );
    }
}

/// End-to-end зонд **ворот размера summary** (этап 2, docs/summary-as-snapshot.md):
/// в БД сеется раздутое описание себя (сверх ориентира по умолчанию 1000 симв.);
/// модель видит подсказку сократить и в пассивной инъекции, и в `get_self_model`.
/// Просим прочитать модель себя и сократить описание, вынеся событийное в наблюдения.
/// Ассертим **механизм** (модель тронула модель себя: `update_self_model` и/или
/// `add_insight`); фактическое сокращение печатаем для go/no-go (поведение нестабильно).
/// `#[ignore]`, вручную:
/// `MINDFORK_ENGINE_URL=…/v1 cargo test summary_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn summary_gate_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Сеем раздутое описание себя (сверх ориентира по умолчанию 1000 симв.).
    let bloated = "Я ассистент, ценю честность и точность. ".repeat(40); // ~1600 симв.
    let bloated_len = bloated.chars().count();
    {
        let storage = Storage::open(Paths::with_root(&root)).unwrap();
        let mut m = crate::entities::self_model::SelfModel::new(pid);
        m.summary = bloated.clone();
        storage.db().self_model_upsert(&m).unwrap();
    }

    // Ход: просим прочитать модель себя и сократить описание.
    let (t, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Прочитай свою «модель себя» (вызови get_self_model). Если описание себя \
         разрослось — сократи его до сути через update_self_model.summary, а событийные \
         выводы вынеси в наблюдения (add_insight).",
    )
    .await;
    eprintln!("текст={t:?}\nвызовы={calls:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Ворота: результат get_self_model показал подсказку (детерминированно — summary
    // раздут сверх ориентира), если модель его вызвала.
    let gate_fired = calls
        .iter()
        .any(|(n, r)| n == "get_self_model" && r.contains("Описание себя разрослось"));
    if calls.iter().any(|(n, _)| n == "get_self_model") {
        assert!(
            gate_fired,
            "get_self_model при раздутом описании должен нести подсказку сократить: {calls:?}"
        );
    }
    // Итоговый размер описания в БД.
    let stored = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .self_model_get(pid)
        .unwrap();
    let final_len = stored
        .as_ref()
        .map(|m| m.summary.chars().count())
        .unwrap_or(0);
    let shrank = final_len < bloated_len;
    let wrote_insight = calls.iter().any(|(n, _)| n == "add_insight");
    eprintln!(
        "ворота показали подсказку: {gate_fired}; описание {bloated_len} → {final_len} \
         (сократилось: {shrank}); вынесено в наблюдения: {wrote_insight}"
    );
    // Модель должна была как-то тронуть модель себя (иначе гипотеза не проверяется).
    assert!(
        calls
            .iter()
            .any(|(n, _)| n == "update_self_model" || n == "add_insight"),
        "ожидали update_self_model/add_insight: {calls:?}"
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

/// End-to-end зонд **обзора self-консолидации** (Ярус 3, отложенный из Яруса 2 B):
/// модель записывает два похожих наблюдения, затем зовёт `reflect` — его результат
/// теперь несёт блок «Обзор наблюдений для консолидации» (похожие пары / contradicts /
/// без связей), под который модель сводит дубли (`note_merge`/`note_supersede`/
/// `note_revise`). Ассертим **механизм** (≥1 наблюдение-заметка; при вызове `reflect`
/// и ≥2 наблюдениях его результат содержит обзор — детерминированно, заголовок/счётчики
/// строятся без векторов, не зависят от порога эмбеддера); фактическое сведение дублей
/// печатаем для go/no-go (поведение нестабильно). `#[ignore]`, вручную:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test self_consolidation_overview_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_consolidation_overview_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Два похожих наблюдения (кандидаты в дубли) — пока НЕ объединяем.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение (add_insight): я ценю краткость в ответах. Пока не объединяй \
         ни с чем — просто запиши.",
    )
    .await;
    let (_t2, tools2) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши ещё одно наблюдение (add_insight): пользователь предпочитает лаконичные, \
         краткие ответы. Даже если инструмент покажет похожее — на этот раз оставь оба.",
    )
    .await;
    eprintln!("наблюдения: {tools1:?} + {tools2:?}");

    // Просим отрефлексировать и свести дубли — reflect несёт обзор self-консолидации.
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Вызови reflect и просмотри блок «Обзор наблюдений для консолидации». Если среди \
         наблюдений есть похожие дубли — сведи их (note_merge или note_supersede).",
    )
    .await;
    eprintln!("рефлексия: текст={t3:?}\nвызовы={calls3:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let self_notes = reopened
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
    // Механизм: наблюдения-заметки созданы.
    assert!(
        !self_notes.is_empty(),
        "ожидали self-заметки (@self) от add_insight"
    );

    // Обзор self-консолидации: если модель вызвала reflect и наблюдений ≥2, его
    // результат ОБЯЗАН нести блок «Обзор наблюдений» (детерминированно — заголовок и
    // счётчики строятся без векторов, порог эмбеддера влияет лишь на список похожих пар).
    // Если модель свела дубли к одному ещё в сессии 2, наблюдений < 2 и обзора нет — ок.
    if let Some((_, result)) = calls3.iter().find(|(n, _)| n == "reflect") {
        let has_overview = result.contains("Обзор наблюдений");
        eprintln!("reflect вернул обзор self-консолидации: {has_overview}");
        if self_notes.len() >= 2 {
            assert!(
                has_overview,
                "reflect при ≥2 наблюдениях должен нести обзор self-консолидации: {result}"
            );
        }
    }
    // Сведение дублей — go/no-go (нестабильно): печатаем.
    let consolidated = calls3
        .iter()
        .any(|(n, _)| n == "note_merge" || n == "note_supersede" || n == "note_revise");
    eprintln!("модель свела дубли (note_merge/supersede/revise): {consolidated}");
    // Модель должна была отрефлексировать и/или тронуть наблюдения.
    assert!(
        calls3.iter().any(|(n, _)| {
            n == "reflect" || n == "note_merge" || n == "note_supersede" || n == "note_revise"
        }),
        "сессия 3: ожидали reflect/note_merge/note_supersede/note_revise: {calls3:?}"
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

/// End-to-end зонд **кросс-органных связей** (Ярус 3, Путь 1): модель записывает факт
/// «о собеседнике» (`note_save`) и наблюдение «о себе» (`add_insight`), затем связывает
/// их (`note_link`) — ребро между органами памяти. Ассертим **механизм** (обе заметки
/// созданы; модель осмотрела оба органа и/или связала); появление **кросс-органного
/// ребра** (один конец `@self`, другой — пользовательская заметка) печатаем для
/// go/no-go. `#[ignore]`, вручную:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test cross_organ_link_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn cross_organ_link_e2e_live() {
    use crate::features::tools::notes::{SELF_NOTE_TAG, is_self_note};
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Факт «о собеседнике» → пользовательская заметка.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши заметку о собеседнике (note_save): пользователь ценит краткость в ответах.",
    )
    .await;
    // Наблюдение «о себе» → self-заметка.
    let (_t2, tools2) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение о себе (add_insight): я склонен давать многословные ответы.",
    )
    .await;
    eprintln!("сохранение: {tools1:?} + {tools2:?}");

    // Просим связать наблюдение «о себе» с фактом «о собеседнике» (кросс-органно).
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Посмотри свои наблюдения (get_self_model) и заметки о собеседнике (note_recall). \
         Если наблюдение о себе противоречит факту о собеседнике — свяжи их note_link \
         (relation=contradicts) по их id.",
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
    // Кросс-органное ребро: один конец @self, другой — пользовательская заметка.
    let cross = links
        .iter()
        .filter(|(f, t, _)| {
            let fs = reopened
                .db()
                .note_get(pid, *f)
                .ok()
                .flatten()
                .as_ref()
                .map(is_self_note);
            let ts = reopened
                .db()
                .note_get(pid, *t)
                .ok()
                .flatten()
                .as_ref()
                .map(is_self_note);
            matches!((fs, ts), (Some(a), Some(b)) if a != b)
        })
        .count();
    eprintln!(
        "self-заметок: {}; всего связей: {}; кросс-органных: {cross} — {links:?}",
        self_notes.len(),
        links.len()
    );

    // Механизм: обе заметки созданы (наблюдение @self + пользовательская).
    assert!(
        !self_notes.is_empty(),
        "ожидали self-заметку от add_insight"
    );
    let all = reopened.db().note_list(pid, None, &[], None).unwrap();
    assert!(
        all.iter().any(|n| !is_self_note(n)),
        "ожидали пользовательскую заметку от note_save"
    );
    let linked = calls3.iter().any(|(n, _)| n == "note_link");
    eprintln!("модель вызвала note_link: {linked}; кросс-органных рёбер: {cross}");
    // Модель должна была осмотреть органы и/или связать (иначе кросс-связь не проверена).
    assert!(
        calls3
            .iter()
            .any(|(n, _)| n == "get_self_model" || n == "note_recall" || n == "note_link"),
        "сессия 3: ожидали get_self_model/note_recall/note_link"
    );
}

/// End-to-end зонд **смешения выдачи** (Ярус 3, Путь 2): при включённом тумблере
/// `notes.recall_includes_self` наблюдения «о себе» (`@self`) входят в общий
/// `note_recall` с пометкой `[о себе]`. Ассертим **механизм** (тумблер применён; модель
/// вызвала `note_recall`); появление self-наблюдения с пометкой и **ответ модели**
/// (не «загрязняет» ли — go/no-go по безопасности смешения) печатаем. `#[ignore]`,
/// вручную: `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test recall_includes_self_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn recall_includes_self_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let _pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;
    // Включаем смешение выдачи (Путь 2). MockSupervisor игнорирует настройки серверов,
    // так что живой backend/embedder остаются; меняется лишь recall_includes_self.
    let config = AppConfig {
        notes: crate::shared::config::NotesSettings {
            recall_includes_self: true,
            ..Default::default()
        },
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(config)))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.notes.recall_includes_self),
    )
    .await
    .unwrap();

    // Факт «о собеседнике» + наблюдение «о себе».
    run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши заметку о собеседнике (note_save): пользователь любит краткость.",
    )
    .await;
    run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение о себе (add_insight): я склонен к многословию.",
    )
    .await;

    // Поиск: при включённом тумблере note_recall должен вернуть и заметку, и наблюдение
    // «о себе» (с пометкой [о себе]).
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Поищи в заметках (note_recall) всё про краткость и многословие — что там есть?",
    )
    .await;
    eprintln!("recall: текст={t3:?}\nвызовы={calls3:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Механизм: note_recall при включённом тумблере вернул self-наблюдение с пометкой.
    let self_marked = calls3
        .iter()
        .any(|(n, r)| n == "note_recall" && r.contains("[о себе]"));
    eprintln!("note_recall показал наблюдение «о себе» с пометкой: {self_marked}");
    assert!(
        calls3.iter().any(|(n, _)| n == "note_recall"),
        "ожидали вызов note_recall"
    );
}

/// End-to-end зонд **связи заметок с RAG-источниками** (Ярус 3, Путь 3): модель
/// добавляет документ в базу знаний, находит его (`rag_search`), записывает вывод
/// заметкой (`note_save`) и связывает её с источником (`note_cite_source`). Ассертим
/// **механизм** (источник в базе; модель искала/связывала); появление связи заметка↔
/// источник в БД печатаем для go/no-go. `#[ignore]`, вручную:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test note_cite_source_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn note_cite_source_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // База знаний: добавляем документ под источником «факты».
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Добавь в базу знаний (rag_add) текст «Столица Франции — Париж.» с источником «факты».",
    )
    .await;
    eprintln!("rag_add: {tools1:?}");

    // Модель ищет, записывает вывод и связывает его с источником.
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Найди в базе знаний (rag_search) про столицу Франции. Запиши краткий вывод \
         заметкой (note_save), затем свяжи эту заметку с источником через \
         note_cite_source (источник называется «факты»).",
    )
    .await;
    eprintln!("цитирование: текст={t2:?}\nвызовы={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    // Механизм: источник «факты» в базе знаний.
    assert!(
        reopened.db().rag_source_exists(pid, "факты").unwrap(),
        "ожидали источник «факты» в базе знаний"
    );
    // Связь заметка↔источник (обратный путь): заметки, ссылающиеся на «факты».
    let citing = reopened.db().notes_citing_source(pid, "факты").unwrap();
    let cited = calls2.iter().any(|(n, _)| n == "note_cite_source");
    eprintln!(
        "модель вызвала note_cite_source: {cited}; заметок со ссылкой на «факты»: {} — {:?}",
        citing.len(),
        citing.iter().map(|n| n.content.clone()).collect::<Vec<_>>()
    );
    // Модель должна была искать и/или связать (иначе путь не проверен).
    assert!(
        calls2
            .iter()
            .any(|(n, _)| n == "rag_search" || n == "note_cite_source"),
        "сессия 2: ожидали rag_search/note_cite_source"
    );
}
