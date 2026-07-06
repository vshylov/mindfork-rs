//! Тесты заметок. См. mod.rs.

use super::super::testkit::ctx_with_storage;
use super::*;
use uuid::Uuid;

#[tokio::test]
async fn save_then_recall_isolated_by_profile() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);

    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "любит чай", "tags": ["pref"]}),
        )
        .await
        .unwrap();
    // Заметка действительно записана под этим профилем.
    assert_eq!(
        storage
            .db()
            .note_list(profile, None, &[], None)
            .unwrap()
            .len(),
        1
    );

    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "чай"}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));

    // Чужой профиль не видит заметку.
    let (_d2, _s2, other) = ctx_with_storage(Uuid::new_v4());
    // другой профиль — другое хранилище: проверяем изоляцию на уровне фильтра
    let empty = NoteRecall
        .invoke(&other, serde_json::json!({}))
        .await
        .unwrap();
    assert!(empty.result.contains("не найдены"));
}

#[tokio::test]
async fn recall_by_tag() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "a", "tags": ["x"]}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "b", "tags": ["y"]}))
        .await
        .unwrap();
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"tags": ["x"]}))
        .await
        .unwrap();
    assert!(out.result.contains("a"));
    assert!(!out.result.contains("- b"));
}

#[tokio::test]
async fn save_rejects_empty_content() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    assert!(
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "  "}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recall_excludes_self_notes() {
    // Ярус 1 «нарратив как заметки»: self-заметки (@self) не всплывают в
    // пользовательском note_recall — ни подстрочном, ни семантическом.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "любит чай"}))
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "сам люблю чай",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();

    // Подстрочный путь (без query).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));
    assert!(!out.result.contains("сам люблю чай"));

    // Семантический путь (есть query + эмбеддер).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "чай"}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));
    assert!(!out.result.contains("сам люблю чай"));
}

#[tokio::test]
async fn recall_includes_self_notes_when_enabled() {
    // Ярус 3, Путь 2: при recall_includes_self self-заметки (@self) ВХОДЯТ в общий
    // note_recall с пометкой [о себе] (обе ветки: подстрочная и семантическая);
    // служебный тег @self в выводе скрыт.
    let profile = Uuid::new_v4();
    let (_d, storage, mut ctx) = ctx_with_storage(profile);
    ctx.recall_includes_self = true;
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "любит чай"}))
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "сам люблю чай",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();

    // Подстрочный путь (без query).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));
    assert!(out.result.contains("[о себе] сам люблю чай"));
    // Служебный тег @self в показе тегов скрыт (его заменяет пометка).
    assert!(!out.result.contains("@self"));

    // Семантический путь (query + эмбеддер).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "чай"}))
        .await
        .unwrap();
    assert!(out.result.contains("[о себе] сам люблю чай"));
}

#[tokio::test]
async fn recall_shows_note_ids() {
    // Ярус 3: note_recall выводит id заметок — иначе модель не сможет ссылаться на
    // них в note_link/note_revise (в т.ч. кросс-органно).
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "любит чай"}))
        .await
        .unwrap();
    let id = storage.db().note_list(profile, None, &[], None).unwrap()[0].id;
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains(&format!("(id={id})")));
}

#[tokio::test]
async fn note_cite_source_links_and_recall_shows_it() {
    // Ярус 3, Путь 3: заметка ссылается на RAG-источник; note_recall показывает
    // блок «Ссылки на источники».
    use crate::entities::rag::RagDocument;
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .rag_insert(&RagDocument::new(
            profile,
            "spec.md",
            "текст про X",
            vec![1.0, 0.0],
        ))
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "вывод про X"}))
        .await
        .unwrap();
    let id = storage.db().note_list(profile, None, &[], None).unwrap()[0].id;

    // Неизвестный источник — понятный отказ, ничего не создано.
    let out = NoteCiteSource
        .invoke(
            &ctx,
            serde_json::json!({"note_id": id.to_string(), "source": "нет.md"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("не найден в базе знаний"));

    // Существующий источник — связь создана; повтор — уже существовала.
    let out = NoteCiteSource
        .invoke(
            &ctx,
            serde_json::json!({"note_id": id.to_string(), "source": "spec.md"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("Связь с источником создана"));
    let out = NoteCiteSource
        .invoke(
            &ctx,
            serde_json::json!({"note_id": id.to_string(), "source": "spec.md"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("уже существовала"));

    // note_recall показывает ссылку на источник.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("Ссылки на источники"));
    assert!(out.result.contains("spec.md"));
}

#[tokio::test]
async fn recall_surfaces_cross_organ_self_neighbor_marked() {
    // Ярус 3 (кросс-органные связи): пользовательская заметка, ЯВНО связанная с
    // наблюдением «о себе», показывает его в блоке «Связанные заметки» с пометкой
    // [о себе] — но обычный поиск self-заметки по-прежнему не тащит.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "пользователь любит краткость"}),
        )
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "я склонен к многословию",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    let user_id = id_by_content(&storage, profile, "пользователь любит краткость");
    let self_id = id_by_content(&storage, profile, "я склонен к многословию");
    storage
        .db()
        .note_link_insert(profile, user_id, self_id, "contradicts")
        .unwrap();

    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "краткость"}))
        .await
        .unwrap();
    // Первичная выдача — только пользовательская заметка (self скрыта из поиска).
    assert!(out.result.contains("пользователь любит краткость"));
    // Но связанное наблюдение всплывает в блоке связей с пометкой [о себе].
    assert!(out.result.contains("Связанные заметки"));
    assert!(out.result.contains("[о себе]"));
    assert!(out.result.contains("я склонен к многословию"));
}

#[tokio::test]
async fn self_related_block_surfaces_cross_organ_user_note_marked() {
    // Ярус 3: чтение «модели себя» показывает пользовательскую заметку-соседа
    // наблюдения с пометкой [заметка] (self↔user ребро).
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "я склонен к многословию",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "пользователь любит краткость"}),
        )
        .await
        .unwrap();
    let self_id = id_by_content(&storage, profile, "я склонен к многословию");
    let user_id = id_by_content(&storage, profile, "пользователь любит краткость");
    storage
        .db()
        .note_link_insert(profile, self_id, user_id, "contradicts")
        .unwrap();

    let block = self_related_block(&ctx, &[self_id]).expect("ожидали блок связей");
    assert!(block.contains("[заметка]"));
    assert!(block.contains("пользователь любит краткость"));
    assert!(block.contains("contradicts"));
}

#[tokio::test]
async fn save_gate_excludes_self_notes() {
    // Ворота note_save не показывают семантически близкие self-заметки — обычная
    // запись не должна натыкаться на наблюдения «модели себя».
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "aaaa bbbb",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    let out = NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaab"}))
        .await
        .unwrap();
    // Единственная близкая заметка — self → блок «Похожие заметки» не появляется.
    assert!(!out.result.contains("Похожие заметки"));
    assert!(!out.result.contains("aaaa bbbb"));
}

#[tokio::test]
async fn consolidation_overview_excludes_self_notes() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "обычная одна"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "обычная два"}))
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "наблюдение о себе",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();

    let overview = build_consolidation_overview(&storage, profile);
    // Self-заметка не в счёте активных и не в списках обзора.
    assert!(overview.contains("Активных заметок: 2"));
    assert!(!overview.contains("наблюдение о себе"));
}

#[tokio::test]
async fn self_consolidation_overview_covers_self_only() {
    // Ярус 3: обзор self-консолидации над наблюдениями (@self) — похожие пары,
    // contradicts, без связей; пользовательские заметки исключены; None при < 2.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    assert!(build_self_consolidation_overview(&storage, profile).is_none());
    create_note(&ctx, "aaaa bbbb".into(), vec![SELF_NOTE_TAG.to_string()])
        .await
        .unwrap();
    // 1 наблюдение → всё ещё None.
    assert!(build_self_consolidation_overview(&storage, profile).is_none());
    create_note(&ctx, "aaab".into(), vec![SELF_NOTE_TAG.to_string()])
        .await
        .unwrap();
    create_note(&ctx, "wwww".into(), vec![SELF_NOTE_TAG.to_string()])
        .await
        .unwrap();
    // Пользовательская заметка не должна попасть в обзор наблюдений.
    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "aaaa пользовательская"}),
        )
        .await
        .unwrap();

    let ov = build_self_consolidation_overview(&storage, profile).unwrap();
    assert!(ov.contains("Обзор наблюдений"));
    assert!(ov.contains("Наблюдений: 3")); // только @self
    assert!(!ov.contains("пользовательская"));
    // Похожая пара среди наблюдений (aaaa bbbb ↔ aaab, cosine ≈ 0.89 ≥ 0.85).
    assert!(ov.contains("aaaa bbbb"));
    assert!(ov.contains("aaab"));

    // Связь contradicts среди наблюдений — обзор её показывает.
    let selves = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    storage
        .db()
        .note_link_insert(profile, selves[0].id, selves[1].id, "contradicts")
        .unwrap();
    let ov = build_self_consolidation_overview(&storage, profile).unwrap();
    assert!(ov.contains("Связи contradicts среди наблюдений: 1"));
}

#[test]
fn migrate_self_narrative_moves_and_is_idempotent() {
    use crate::entities::self_model::{NarrativeSegment, SelfModel};
    use chrono::{Duration, Utc};
    let profile = Uuid::new_v4();
    let (_d, storage, _ctx) = ctx_with_storage(profile);
    // «Старая» модель с нарративом-блобом (как до Яруса 1).
    let mut m = SelfModel::new(profile);
    let old = Utc::now() - Duration::days(3);
    m.narrative = vec![
        NarrativeSegment {
            id: Uuid::new_v4(),
            text: "старое наблюдение".into(),
            created_at: old,
        },
        NarrativeSegment {
            id: Uuid::new_v4(),
            text: "ещё одно".into(),
            created_at: Utc::now(),
        },
    ];
    storage.db().self_model_upsert(&m).unwrap();

    migrate_self_narrative(&storage, profile);

    // Нарратив блоба очищен, наблюдения стали @self-заметками (created_at сохранён).
    assert!(
        storage
            .db()
            .self_model_get(profile)
            .unwrap()
            .unwrap()
            .narrative
            .is_empty()
    );
    let self_notes = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    assert_eq!(self_notes.len(), 2);
    assert!(
        self_notes
            .iter()
            .any(|n| n.content == "старое наблюдение" && n.created_at == old)
    );

    // Повторный проход — no-op (нарратив пуст, дублей нет).
    migrate_self_narrative(&storage, profile);
    assert_eq!(
        storage
            .db()
            .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn self_notes_relevant_ranks_and_filters() {
    // Ярус 2: инъекция по релевантности — self_notes_relevant ранжирует
    // self-заметки по близости к запросу и не отдаёт обычные заметки.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    create_note(
        &ctx,
        "aaaa про краткость".into(),
        vec![SELF_NOTE_TAG.to_string()],
    )
    .await
    .unwrap();
    create_note(
        &ctx,
        "wwww про погоду".into(),
        vec![SELF_NOTE_TAG.to_string()],
    )
    .await
    .unwrap();
    // Обычная заметка (не @self) — не должна попадать в выборку наблюдений.
    create_note(&ctx, "aaaa обычная".into(), vec![])
        .await
        .unwrap();

    let rel = self_notes_relevant(&storage, ctx.embedder.as_ref(), profile, "aaaa", 3).await;
    assert!(!rel.is_empty());
    assert!(rel[0].content.contains("краткость")); // ближайшая к «aaaa»
    assert!(rel.iter().all(|n| n.content != "aaaa обычная")); // только @self
    // Пустой запрос → пусто (мягкая деградация к свежести у вызывающего).
    assert!(
        self_notes_relevant(&storage, ctx.embedder.as_ref(), profile, "  ", 3)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn save_surfaces_similar_notes_as_gate() {
    // MockEmbedder(16) — мешок символов: тексты с общими буквами близки.
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa bbbb"}))
        .await
        .unwrap();
    // Вторая заметка близка по символам → ворота должны показать первую.
    let out = NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaab"}))
        .await
        .unwrap();
    assert!(out.result.contains("Похожие заметки"));
    assert!(out.result.contains("aaaa bbbb"));
}

#[tokio::test]
async fn recall_semantic_finds_non_substring_match() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "wwww"}))
        .await
        .unwrap();
    // Запрос «aaab» не является подстрокой ни одной заметки, но семантически
    // ближе к «aaaa» → семантический путь его находит.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
        .await
        .unwrap();
    assert!(out.result.contains("aaaa"));
    assert!(!out.result.contains("wwww"));
}

#[tokio::test]
async fn save_gate_surfaces_legacy_note_without_vector() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    // «Старая» заметка без вектора (вставлена напрямую — как до фичи/при импорте).
    storage
        .db()
        .note_insert(&Note::new(profile, "aaaa bbbb", vec![]))
        .unwrap();
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        1
    );

    // Сохраняем похожую — ворота должны показать старую (бэкфилл в note_save).
    let out = NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaab"}))
        .await
        .unwrap();
    assert!(out.result.contains("Похожие заметки"));
    assert!(out.result.contains("aaaa bbbb"));
    // Бэкфилл проиндексировал старую заметку.
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        0
    );
}

#[tokio::test]
async fn recall_backfills_legacy_notes_without_vectors() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    // «Старые» заметки без векторов (вставлены напрямую — как до фичи/при импорте).
    storage
        .db()
        .note_insert(&Note::new(profile, "aaaa", vec![]))
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(profile, "wwww", vec![]))
        .unwrap();
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        2
    );

    // Семантический recall дотягивает вектора и находит не-подстрочное совпадение.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
        .await
        .unwrap();
    assert!(out.result.contains("aaaa"));
    // Бэкфилл выполнен — заметок без векторов больше нет.
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        0
    );
}

#[tokio::test]
async fn revise_rewrites_in_place() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "старое"}))
        .await
        .unwrap();
    let id = storage.db().note_list(profile, None, &[], None).unwrap()[0].id;

    let out = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": id.to_string(), "content": "новое"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("переписана"));
    // Содержимое заменено на месте (не добавлена новая заметка).
    let notes = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content, "новое");
}

#[tokio::test]
async fn revise_bad_and_missing_id() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    // Некорректный uuid → ошибка.
    assert!(
        NoteRevise
            .invoke(&ctx, serde_json::json!({"id": "not-uuid", "content": "x"}))
            .await
            .is_err()
    );
    // Корректный, но несуществующий → понятный текст, не паника.
    let out = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": Uuid::new_v4().to_string(), "content": "x"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("не найдена"));
}

/// id заметки по содержимому (для тестов графа).
fn id_by_content(storage: &crate::shared::storage::Storage, profile: Uuid, content: &str) -> Uuid {
    storage
        .db()
        .note_list(profile, None, &[], None)
        .unwrap()
        .into_iter()
        .find(|n| n.content == content)
        .unwrap()
        .id
}

#[tokio::test]
async fn link_then_neighbors() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "альфа"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "бета"}))
        .await
        .unwrap();
    let a = id_by_content(&storage, profile, "альфа");
    let b = id_by_content(&storage, profile, "бета");

    let out = NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "refines"}),
            )
            .await
            .unwrap();
    assert!(out.result.contains("Связь создана"));

    // Повтор той же связи — честный ответ «уже существовала» (без дубля в графе).
    let dup = NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "refines"}),
            )
            .await
            .unwrap();
    assert!(dup.result.contains("уже существовала"));

    let nb = NoteNeighbors
        .invoke(&ctx, serde_json::json!({"id": a.to_string()}))
        .await
        .unwrap();
    assert!(nb.result.contains("бета"));
    assert!(nb.result.contains("refines"));

    // Неизвестный тип связи и самосвязь → ошибки.
    assert!(
            NoteLink
                .invoke(
                    &ctx,
                    serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "foo"}),
                )
                .await
                .is_err()
        );
    assert!(
            NoteLink
                .invoke(
                    &ctx,
                    serde_json::json!({"from_id": a.to_string(), "to_id": a.to_string(), "relation": "relates"}),
                )
                .await
                .is_err()
        );
}

#[tokio::test]
async fn revise_warns_when_note_has_links() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "узел"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "другой"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "одиночка"}))
        .await
        .unwrap();
    let a = id_by_content(&storage, profile, "узел");
    let b = id_by_content(&storage, profile, "другой");
    let lone = id_by_content(&storage, profile, "одиночка");
    NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": b.to_string(), "to_id": a.to_string(), "relation": "contradicts"}),
            )
            .await
            .unwrap();

    // Ревизия узла со связью предупреждает про note_supersede.
    let out = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": a.to_string(), "content": "узел v2"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("переписана"));
    assert!(out.result.contains("note_supersede"));

    // Узел без связей — без предупреждения.
    let out2 = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": lone.to_string(), "content": "одиночка v2"}),
        )
        .await
        .unwrap();
    assert!(!out2.result.contains("note_supersede"));
}

#[tokio::test]
async fn supersede_hides_old_shows_new() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "первая версия"}))
        .await
        .unwrap();
    let old = id_by_content(&storage, profile, "первая версия");

    let out = NoteSupersede
        .invoke(
            &ctx,
            serde_json::json!({"old_id": old.to_string(), "content": "вторая версия"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("замещена"));

    let active = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].content, "вторая версия");
    assert!(!storage.db().note_is_active(profile, old).unwrap());
}

#[tokio::test]
async fn merge_consolidates_sources() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "кусок один"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "кусок два"}))
        .await
        .unwrap();
    let ids: Vec<String> = storage
        .db()
        .note_list(profile, None, &[], None)
        .unwrap()
        .into_iter()
        .map(|n| n.id.to_string())
        .collect();

    let out = NoteMerge
        .invoke(
            &ctx,
            serde_json::json!({"ids": ids, "content": "единая заметка"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("Объединено заметок: 2"));

    let active = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].content, "единая заметка");

    // Меньше двух существующих → ошибка.
    assert!(
        NoteMerge
            .invoke(
                &ctx,
                serde_json::json!({"ids": [Uuid::new_v4().to_string()], "content": "x"}),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn supersede_preserves_tags() {
    // Замещение self-заметки сохраняет тег @self — новая версия остаётся скрытой
    // из пользовательского recall (иначе «выпала» бы в выдачу).
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "версия 1",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    let old = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap()[0]
        .id;
    NoteSupersede
        .invoke(
            &ctx,
            serde_json::json!({"old_id": old.to_string(), "content": "версия 2"}),
        )
        .await
        .unwrap();
    let self_notes = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    assert_eq!(self_notes.len(), 1);
    assert_eq!(self_notes[0].content, "версия 2");
    // И не всплывает в обычном recall.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("не найдены"));
}

#[tokio::test]
async fn merge_unions_tags() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(profile, "aaa", vec!["x".into()]))
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(profile, "bbb", vec!["y".into()]))
        .unwrap();
    let ids: Vec<String> = storage
        .db()
        .note_list(profile, None, &[], None)
        .unwrap()
        .iter()
        .map(|n| n.id.to_string())
        .collect();
    NoteMerge
        .invoke(&ctx, serde_json::json!({"ids": ids, "content": "ccc"}))
        .await
        .unwrap();
    let merged = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(merged.len(), 1);
    assert!(merged[0].tags.contains(&"x".to_string()));
    assert!(merged[0].tags.contains(&"y".to_string()));
}

#[tokio::test]
async fn recall_spreads_to_linked_notes() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "zzzz"}))
        .await
        .unwrap();
    let a = id_by_content(&storage, profile, "aaaa");
    let z = id_by_content(&storage, profile, "zzzz");
    NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": z.to_string(), "relation": "relates"}),
            )
            .await
            .unwrap();

    // Запрос близок к «aaaa»; «zzzz» не похож, но связан → попадёт в «Связанные».
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
        .await
        .unwrap();
    assert!(out.result.contains("aaaa"));
    assert!(out.result.contains("Связанные заметки"));
    assert!(out.result.contains("zzzz"));
}

#[tokio::test]
async fn consolidate_notes_reports_dups_and_dangling() {
    let profile = Uuid::new_v4();
    let (_d, _s, ctx) = ctx_with_storage(profile);
    // Два почти-дубля (общие символы → высокий косинус на MockEmbedder).
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa bbbb"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa bbbbb"}))
        .await
        .unwrap();
    // Несвязанная непохожая заметка.
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "zzzz"}))
        .await
        .unwrap();

    let out = ConsolidateNotes
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("Обзор базы знаний"));
    // Похожая пара найдена (хотя бы одна).
    assert!(out.result.contains("Похожие пары (возможные дубли"));
    assert!(out.result.contains("aaaa bbbb"));
    assert!(out.result.contains("без связей"));
    assert!(out.result.contains("note_merge"));
}

#[tokio::test]
async fn merge_transfers_links_to_new_note() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "часть один"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "часть два"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "третья"}))
        .await
        .unwrap();
    // id берём ДО слияния (после источники замещаются и из списка исчезают).
    let s1 = id_by_content(&storage, profile, "часть один");
    let s2 = id_by_content(&storage, profile, "часть два");
    let other = id_by_content(&storage, profile, "третья");
    NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": s1.to_string(), "to_id": other.to_string(), "relation": "relates"}),
            )
            .await
            .unwrap();

    NoteMerge
        .invoke(
            &ctx,
            serde_json::json!({"ids": [s1.to_string(), s2.to_string()], "content": "единая"}),
        )
        .await
        .unwrap();

    // Связь источника перенесена на объединённую заметку: сосед «третьей» — «единая».
    let nb = NoteNeighbors
        .invoke(&ctx, serde_json::json!({"id": other.to_string()}))
        .await
        .unwrap();
    assert!(nb.result.contains("единая"));
    assert!(!nb.result.contains("часть один"));
}
