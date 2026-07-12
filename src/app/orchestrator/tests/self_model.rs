//! Тесты оркестратора — модель себя: инъекция, F3-правки, сигналы. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/history/refactoring-god-objects.md, этап 3.

use super::*;

#[test]
fn inject_self_model_respects_flag_and_emptiness() {
    use super::super::generation::inject_self_model;
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
        inject_self_model(
            Some("S".into()),
            Some(&m),
            false,
            true,
            &pp,
            now,
            &[],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        ),
        Some("S".into())
    );
    // Включено, протокол выкл, непустая модель → блок дописывается, протокола нет.
    let out = inject_self_model(
        Some("S".into()),
        Some(&m),
        true,
        false,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(out.starts_with("S\n\n"));
    assert!(out.contains("ценю ясность"));
    assert!(!out.contains("угодливости"));
    // Включено, протокол выкл, модели нет, наблюдений нет → без изменений.
    assert_eq!(
        inject_self_model(
            Some("S".into()),
            None,
            true,
            false,
            &pp,
            now,
            &[],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        ),
        Some("S".into())
    );
    // Модели нет, но есть наблюдения (self-заметки) → инъекция всё равно происходит.
    let obs = [seg("заметил склонность к краткости")];
    let only_obs = inject_self_model(
        None,
        None,
        true,
        false,
        &pp,
        now,
        &obs,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(only_obs.contains("Недавние наблюдения:"));
    assert!(only_obs.contains("склонность к краткости"));
    // Пустая модель + пустые наблюдения, system=None → нечего подмешивать → None.
    let empty = SelfModel::new(Uuid::new_v4());
    assert_eq!(
        inject_self_model(
            None,
            Some(&empty),
            true,
            false,
            &pp,
            now,
            &[],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        ),
        None
    );
    // Пустой system + непустая модель (протокол выкл) → блок становится системой.
    let only = inject_self_model(
        None,
        Some(&m),
        true,
        false,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(only.contains("О себе: ценю ясность"));

    // Протокол вкл + пустая модель → протокол всё равно подмешивается (bootstrap).
    let boot = inject_self_model(
        Some("S".into()),
        Some(&empty),
        true,
        true,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(boot.starts_with("S\n\n"));
    assert!(boot.contains("угодливости"));
    // Протокол вкл + непустая модель → и рендер, и протокол.
    let both = inject_self_model(
        None,
        Some(&m),
        true,
        true,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(both.contains("ценю ясность"));
    assert!(both.contains("угодливости"));
}

#[test]
fn inject_self_model_appends_summary_hint_when_bloated() {
    // Этап 2: при включённом протоколе и раздутом описании (сверх ориентира) в
    // инъекцию дописывается data-aware подсказка сократить; при выключенном протоколе
    // подсказки нет даже при раздутом описании.
    use super::super::generation::inject_self_model;
    use crate::entities::self_model::{SelfModel, SelfModelParams};
    use crate::shared::config::SelfModelSettings;

    // Ориентир 5 санитизируется до пола 200 — описание берём длиннее 200 символов.
    let params = SelfModelParams::from_settings(&SelfModelSettings {
        summary_target_chars: 5,
        ..SelfModelSettings::default()
    });
    let mut m = SelfModel::new(Uuid::new_v4());
    m.summary = "я".repeat(250);
    let now = chrono::Utc::now();

    // Протокол вкл → подсказка присутствует.
    let with = inject_self_model(
        None,
        Some(&m),
        true,
        true,
        &params,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(with.contains("Описание себя разрослось"));
    // Протокол выкл → протокола и подсказки нет (только рендер модели).
    let without = inject_self_model(
        None,
        Some(&m),
        true,
        false,
        &params,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(!without.contains("Описание себя разрослось"));
}

#[test]
fn blend_self_notes_prioritizes_relevant_and_guarantees_freshest() {
    use super::super::generation::blend_self_notes;
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
    use super::super::generation::injection_recent;
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
        thought_signature: None,
        id: "c1".into(),
        name: "update_self_model".into(),
        arguments: serde_json::json!({}),
        result: Some("ok".into()),
    }];
    orch.handle_done(super::super::generation::GenResult {
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
