//! Тесты хранилища. См. mod.rs.

use super::*;

fn db() -> Db {
    Db::open_in_memory().unwrap()
}

#[test]
fn notes_isolated_by_profile() {
    let db = db();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    db.note_insert(&Note::new(a, "secret of A", vec![]))
        .unwrap();
    db.note_insert(&Note::new(b, "secret of B", vec![]))
        .unwrap();

    let a_notes = db.note_list(a, None, &[], None).unwrap();
    assert_eq!(a_notes.len(), 1);
    assert_eq!(a_notes[0].content, "secret of A");
    // Профиль B не виден из A.
    assert!(a_notes.iter().all(|n| n.profile_id == a));
}

#[test]
fn note_update_only_own_profile() {
    let db = db();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let note = Note::new(a, "v1", vec![]);
    let id = note.id;
    db.note_insert(&note).unwrap();
    // Чужой профиль переписать не может.
    assert!(!db.note_update(id, b, "hacked").unwrap());
    // Свой — может.
    assert!(db.note_update(id, a, "v2").unwrap());
    assert_eq!(db.note_list(a, None, &[], None).unwrap()[0].content, "v2");
    // Несуществующая заметка.
    assert!(!db.note_update(Uuid::new_v4(), a, "x").unwrap());
}

#[test]
fn note_semantic_search_ranks_and_isolates() {
    let db = db();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let n1 = Note::new(a, "rust", vec![]);
    let n2 = Note::new(a, "banana", vec![]);
    let (id1, id2) = (n1.id, n2.id);
    db.note_insert(&n1).unwrap();
    db.note_insert(&n2).unwrap();
    db.note_vector_upsert(id1, a, &[1.0, 0.0, 0.0]).unwrap();
    db.note_vector_upsert(id2, a, &[0.0, 1.0, 0.0]).unwrap();
    // Заметка другого профиля с близким вектором — не должна попасть в выдачу a.
    let nb = Note::new(b, "other", vec![]);
    db.note_insert(&nb).unwrap();
    db.note_vector_upsert(nb.id, b, &[1.0, 0.0, 0.0]).unwrap();

    let hits = db.note_search_semantic(a, &[0.9, 0.1, 0.0], 5).unwrap();
    assert_eq!(hits.len(), 2); // только профиль a
    assert_eq!(hits[0].0.id, id1); // ближе к [1,0,0]
    assert!(hits[0].1 > hits[1].1);

    // k ограничивает выдачу.
    let top1 = db.note_search_semantic(a, &[0.9, 0.1, 0.0], 1).unwrap();
    assert_eq!(top1.len(), 1);
    assert_eq!(top1[0].0.id, id1);
}

#[test]
fn notes_missing_vectors_lists_unembedded() {
    let db = db();
    let a = Uuid::new_v4();
    let n1 = Note::new(a, "with vec", vec![]);
    let n2 = Note::new(a, "no vec", vec![]);
    db.note_insert(&n1).unwrap();
    db.note_insert(&n2).unwrap();
    db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
    let missing = db.notes_missing_vectors(a).unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].1, "no vec");
}

#[test]
fn note_vector_upsert_replaces() {
    let db = db();
    let a = Uuid::new_v4();
    let n = Note::new(a, "x", vec![]);
    let id = n.id;
    db.note_insert(&n).unwrap();
    db.note_vector_upsert(id, a, &[1.0, 0.0]).unwrap();
    db.note_vector_upsert(id, a, &[0.0, 1.0]).unwrap(); // замена
    let hits = db.note_search_semantic(a, &[0.0, 1.0], 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert!((hits[0].1 - 1.0).abs() < 1e-6);
}

#[test]
fn supersede_hides_note_from_list_and_search() {
    let db = db();
    let a = Uuid::new_v4();
    let old = Note::new(a, "old", vec![]);
    let new = Note::new(a, "new", vec![]);
    db.note_insert(&old).unwrap();
    db.note_insert(&new).unwrap();
    db.note_vector_upsert(old.id, a, &[1.0, 0.0]).unwrap();
    db.note_vector_upsert(new.id, a, &[1.0, 0.0]).unwrap();
    db.note_supersede_mark(a, old.id, new.id).unwrap();

    // Замещённая скрыта и из списка, и из семантики, и из is_active.
    let list = db.note_list(a, None, &[], None).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, new.id);
    let hits = db.note_search_semantic(a, &[1.0, 0.0], 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, new.id);
    assert!(!db.note_is_active(a, old.id).unwrap());
    assert!(db.note_is_active(a, new.id).unwrap());
}

#[test]
fn links_and_neighbors_both_directions_and_isolation() {
    let db = db();
    let a = Uuid::new_v4();
    let n1 = Note::new(a, "n1", vec![]);
    let n2 = Note::new(a, "n2", vec![]);
    let n3 = Note::new(a, "n3", vec![]);
    db.note_insert(&n1).unwrap();
    db.note_insert(&n2).unwrap();
    db.note_insert(&n3).unwrap();
    assert!(db.note_link_insert(a, n1.id, n2.id, "refines").unwrap());
    assert!(db.note_link_insert(a, n3.id, n1.id, "contradicts").unwrap());
    // Повтор той же связи не создаётся (false) — дубля в таблице нет.
    assert!(!db.note_link_insert(a, n1.id, n2.id, "refines").unwrap());

    let nb = db.note_neighbors(a, n1.id, None).unwrap();
    assert_eq!(nb.len(), 2); // исходящая на n2 + входящая от n3 (дубль не учтён)
    assert!(
        nb.iter()
            .any(|(n, r, out)| n.id == n2.id && r == "refines" && *out)
    );
    assert!(
        nb.iter()
            .any(|(n, r, out)| n.id == n3.id && r == "contradicts" && !*out)
    );

    // Фильтр по типу связи.
    let only = db.note_neighbors(a, n1.id, Some("refines")).unwrap();
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].0.id, n2.id);

    // Замещённый сосед исчезает из выдачи.
    let repl = Note::new(a, "n2b", vec![]);
    db.note_insert(&repl).unwrap();
    db.note_supersede_mark(a, n2.id, repl.id).unwrap();
    let nb2 = db.note_neighbors(a, n1.id, None).unwrap();
    assert!(!nb2.iter().any(|(n, _, _)| n.id == n2.id));
}

#[test]
fn notes_with_vectors_active_only() {
    let db = db();
    let a = Uuid::new_v4();
    let n1 = Note::new(a, "n1", vec![]);
    let n2 = Note::new(a, "n2", vec![]);
    db.note_insert(&n1).unwrap();
    db.note_insert(&n2).unwrap();
    db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
    db.note_vector_upsert(n2.id, a, &[0.0, 1.0]).unwrap();
    // Замещённая исключается из выдачи.
    let r = Note::new(a, "r", vec![]);
    db.note_insert(&r).unwrap();
    db.note_supersede_mark(a, n2.id, r.id).unwrap();

    let wv = db.notes_with_vectors(a).unwrap();
    assert_eq!(wv.len(), 1);
    assert_eq!(wv[0].0.id, n1.id);
    assert_eq!(wv[0].1, vec![1.0, 0.0]);
}

#[test]
fn merge_link_retarget_moves_dedups_and_drops_selfloop() {
    let db = db();
    let a = Uuid::new_v4();
    let s1 = Note::new(a, "s1", vec![]);
    let s2 = Note::new(a, "s2", vec![]);
    let x = Note::new(a, "x", vec![]);
    let merged = Note::new(a, "merged", vec![]);
    for n in [&s1, &s2, &x, &merged] {
        db.note_insert(n).unwrap();
    }
    // s1→x и s2→x (после переноса станут дублем); s1→s2 (станет самопетлёй).
    db.note_link_insert(a, s1.id, x.id, "contradicts").unwrap();
    db.note_link_insert(a, s2.id, x.id, "contradicts").unwrap();
    db.note_link_insert(a, s1.id, s2.id, "relates").unwrap();

    db.note_links_retarget(a, s1.id, merged.id).unwrap();
    db.note_links_retarget(a, s2.id, merged.id).unwrap();

    let all = db.note_links_all(a).unwrap();
    assert_eq!(all.len(), 1); // дубль схлопнут, самопетля отброшена
    assert_eq!(all[0].0, merged.id);
    assert_eq!(all[0].1, x.id);
    assert_eq!(all[0].2, "contradicts");
}

#[test]
fn self_model_round_trip_and_isolation() {
    use crate::entities::self_model::SelfModel;
    let db = db();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();

    // Изначально нет.
    assert!(db.self_model_get(a).unwrap().is_none());

    let mut m = SelfModel::new(a);
    m.summary = "о себе A".into();
    m.add_goal("цель A");
    db.self_model_upsert(&m).unwrap();

    let loaded = db.self_model_get(a).unwrap().unwrap();
    assert_eq!(loaded.summary, "о себе A");
    assert_eq!(loaded.goals.len(), 1);
    assert_eq!(loaded.version, 1); // версия выставлена хранилищем

    // Профиль B не видит модель A.
    assert!(db.self_model_get(b).unwrap().is_none());

    // Повторный upsert повышает версию и заменяет данные.
    let mut m2 = loaded.clone();
    m2.summary = "обновлено".into();
    db.self_model_upsert(&m2).unwrap();
    let reloaded = db.self_model_get(a).unwrap().unwrap();
    assert_eq!(reloaded.summary, "обновлено");
    assert_eq!(reloaded.version, 2);
}

#[test]
fn self_model_update_is_atomic_under_concurrency() {
    use std::sync::Arc;
    // Два потока параллельно дописывают инсайты в модель одного профиля. При
    // неатомарном read-modify-write часть записей терялась бы (гонка «прочитал →
    // другой записал → записал поверх»). `self_model_update` держит SELECT+upsert
    // под одним захватом мьютекса — ни одна запись не теряется.
    let db = Arc::new(db());
    let pid = Uuid::new_v4();
    let n: usize = 50;
    let handles: Vec<_> = ["A", "B"]
        .iter()
        .map(|prefix| {
            let db = db.clone();
            let prefix = prefix.to_string();
            std::thread::spawn(move || {
                for i in 0..n {
                    db.self_model_update(pid, |m| {
                        // Накапливаем цели — считаем ровно (add_goal без потолка).
                        m.add_goal(format!("{prefix}{i}"));
                        true
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let m = db.self_model_get(pid).unwrap().unwrap();
    assert_eq!(m.goals.len(), 2 * n);
    assert_eq!(m.version, (2 * n) as u64); // каждая правка = один upsert
}

#[test]
fn self_model_update_skips_write_when_unchanged() {
    let db = db();
    let pid = Uuid::new_v4();
    // mutate вернул false → записи и роста версии нет, строки в БД не появилось.
    let (model, changed) = db.self_model_update(pid, |_m| false).unwrap();
    assert!(!changed);
    assert_eq!(model.version, 0);
    assert!(db.self_model_get(pid).unwrap().is_none());
}

#[test]
fn note_delete_removes_and_is_profile_isolated() {
    let db = db();
    let p = Uuid::new_v4();
    let other = Uuid::new_v4();
    let n = Note::new(p, "наблюдение", vec![]);
    let id = n.id;
    db.note_insert(&n).unwrap();
    db.note_vector_upsert(id, p, &[1.0, 0.0]).unwrap();
    // Чужой профиль не удаляет.
    assert!(!db.note_delete(other, id).unwrap());
    assert_eq!(db.note_list(p, None, &[], None).unwrap().len(), 1);
    // Свой — удаляет заметку (и её вектор).
    assert!(db.note_delete(p, id).unwrap());
    assert!(db.note_list(p, None, &[], None).unwrap().is_empty());
    // Заметки без вектора нет (обе таблицы пусты) — вектор снят вместе с заметкой.
    assert!(db.notes_missing_vectors(p).unwrap().is_empty());
}

#[test]
fn note_query_and_tag_filter() {
    let db = db();
    let p = Uuid::new_v4();
    db.note_insert(&Note::new(p, "likes tea", vec!["pref".into()]))
        .unwrap();
    db.note_insert(&Note::new(
        p,
        "likes coffee",
        vec!["pref".into(), "drink".into()],
    ))
    .unwrap();

    assert_eq!(db.note_list(p, Some("tea"), &[], None).unwrap().len(), 1);
    assert_eq!(
        db.note_list(p, None, &["drink".to_string()], None)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.note_list(p, None, &[], Some(1)).unwrap().len(), 1);
}

#[test]
fn note_delete_works() {
    let db = db();
    let p = Uuid::new_v4();
    let note = Note::new(p, "x", vec![]);
    db.note_insert(&note).unwrap();
    assert!(db.note_delete(p, note.id).unwrap());
    assert!(db.note_list(p, None, &[], None).unwrap().is_empty());
}

#[test]
fn rag_knn_respects_profile_isolation() {
    let db = db();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    // Профиль B имеет вектор, идентичный запросу — он не должен «утечь» в поиск A.
    db.rag_insert(&RagDocument::new(b, "b", "B doc", vec![1.0, 0.0, 0.0, 0.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(
        a,
        "a1",
        "A near",
        vec![0.9, 0.1, 0.0, 0.0],
    ))
    .unwrap();
    db.rag_insert(&RagDocument::new(
        a,
        "a2",
        "A far",
        vec![0.0, 0.0, 1.0, 0.0],
    ))
    .unwrap();

    let hits = db.rag_search(a, &[1.0, 0.0, 0.0, 0.0], 5).unwrap();
    assert_eq!(hits.len(), 2, "only profile A docs");
    assert_eq!(hits[0].chunk_text, "A near");
    assert_eq!(hits[1].chunk_text, "A far");
    assert_eq!(db.rag_count(a).unwrap(), 2);
    assert_eq!(db.rag_count(b).unwrap(), 1);
}

#[test]
fn rag_search_empty_before_any_insert() {
    let db = db();
    let hits = db.rag_search(Uuid::new_v4(), &[1.0, 0.0], 5).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn rag_delete_by_source_removes_exact_only() {
    let db = db();
    let p = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c1", vec![1.0, 0.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c2", vec![0.0, 1.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(p, "/data/b.txt", "c3", vec![1.0, 1.0]))
        .unwrap();

    // Удаляются оба чанка источника a.txt, b.txt остаётся.
    assert_eq!(db.rag_delete_by_source(p, "/data/a.txt").unwrap(), 2);
    assert_eq!(db.rag_count(p).unwrap(), 1);
    // Поиск тоже больше их не находит (векторы удалены).
    let hits = db.rag_search(p, &[1.0, 0.0], 5).unwrap();
    assert!(hits.iter().all(|h| h.source == "/data/b.txt"));
}

#[test]
fn note_rag_links_bidirectional_and_isolated() {
    use crate::entities::note::Note;
    let db = db();
    let p = Uuid::new_v4();
    let other = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(p, "/kb/spec.md", "chunk", vec![1.0, 0.0]))
        .unwrap();
    let note = Note::new(p, "опирается на спеку", vec![]);
    let nid = note.id;
    db.note_insert(&note).unwrap();

    // Существование источника (изоляция по профилю).
    assert!(db.rag_source_exists(p, "/kb/spec.md").unwrap());
    assert!(!db.rag_source_exists(p, "/kb/missing.md").unwrap());
    assert!(!db.rag_source_exists(other, "/kb/spec.md").unwrap());

    // Связь идемпотентна.
    assert!(db.note_cite_source_insert(p, nid, "/kb/spec.md").unwrap());
    assert!(!db.note_cite_source_insert(p, nid, "/kb/spec.md").unwrap());

    // Прямое направление: источники заметки.
    assert_eq!(
        db.note_cited_sources(p, nid).unwrap(),
        vec!["/kb/spec.md".to_string()]
    );
    // Обратное направление: заметки источника (+ изоляция).
    let citing = db.notes_citing_source(p, "/kb/spec.md").unwrap();
    assert_eq!(citing.len(), 1);
    assert_eq!(citing[0].id, nid);
    assert!(
        db.notes_citing_source(other, "/kb/spec.md")
            .unwrap()
            .is_empty()
    );

    // Удаление заметки снимает её ссылки на источники.
    db.note_delete(p, nid).unwrap();
    assert!(db.notes_citing_source(p, "/kb/spec.md").unwrap().is_empty());
}

#[test]
fn notes_citing_source_hides_superseded() {
    use crate::entities::note::Note;
    let db = db();
    let p = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(p, "/kb/a.md", "c", vec![1.0, 0.0]))
        .unwrap();
    let n = Note::new(p, "старое", vec![]);
    let nid = n.id;
    db.note_insert(&n).unwrap();
    db.note_cite_source_insert(p, nid, "/kb/a.md").unwrap();
    // Замещённая заметка не всплывает в обратном пути.
    let new = Note::new(p, "новое", vec![]);
    let new_id = new.id;
    db.note_insert(&new).unwrap();
    db.note_supersede_mark(p, nid, new_id).unwrap();
    assert!(db.notes_citing_source(p, "/kb/a.md").unwrap().is_empty());
}

#[test]
fn rag_delete_under_removes_path_and_descendants() {
    let db = db();
    let p = Uuid::new_v4();
    let other = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(p, "/data/a.txt", "a", vec![1.0, 0.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(p, "/data/sub/b.txt", "b", vec![0.0, 1.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(p, "/other/c.txt", "c", vec![1.0, 1.0]))
        .unwrap();
    // Чужой профиль с тем же путём не должен затрагиваться (изоляция).
    db.rag_insert(&RagDocument::new(other, "/data/a.txt", "x", vec![1.0, 0.0]))
        .unwrap();

    // Удаление директории сносит файл и вложенные, но не «/other» и не чужой профиль.
    assert_eq!(db.rag_delete_under(p, "/data").unwrap(), 2);
    assert_eq!(db.rag_count(p).unwrap(), 1);
    assert_eq!(db.rag_count(other).unwrap(), 1);

    // Префикс не цепляет соседнюю директорию с общим началом имени.
    db.rag_insert(&RagDocument::new(p, "/x/file.txt", "f", vec![1.0, 0.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(
        p,
        "/x-extra/file.txt",
        "g",
        vec![0.0, 1.0],
    ))
    .unwrap();
    assert_eq!(db.rag_delete_under(p, "/x").unwrap(), 1);
}

#[test]
fn rag_delete_under_tolerates_separators_and_trailing_slash() {
    let db = db();
    let p = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(p, "/data/a.txt", "a", vec![1.0, 0.0]))
        .unwrap();
    // Обратные слэши и хвостовой слэш в запросе матчат сохранённый «/»-источник.
    assert_eq!(db.rag_delete_under(p, "\\data\\").unwrap(), 1);
}

#[test]
fn rag_list_sources_aggregates_chunks_per_source() {
    let db = db();
    let p = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c1", vec![1.0, 0.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c2", vec![0.0, 1.0]))
        .unwrap();
    db.rag_insert(&RagDocument::new(p, "/data/b.md", "c3", vec![1.0, 1.0]))
        .unwrap();
    // Чужой профиль не попадает в выдачу.
    db.rag_insert(&RagDocument::new(
        Uuid::new_v4(),
        "/o.txt",
        "x",
        vec![1.0, 0.0],
    ))
    .unwrap();

    let sources = db.rag_list_sources(p).unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].source, "/data/a.txt");
    assert_eq!(sources[0].chunks, 2);
    assert_eq!(sources[1].source, "/data/b.md");
    assert_eq!(sources[1].chunks, 1);
}

#[test]
fn rag_sources_store_and_delete_with_chunks() {
    let db = db();
    let p = Uuid::new_v4();
    db.rag_source_upsert(p, "/data/a.txt", "полный текст", Utc::now())
        .unwrap();
    db.rag_insert(&RagDocument::new(p, "/data/a.txt", "чанк", vec![1.0, 0.0]))
        .unwrap();
    // Повторный upsert заменяет содержимое, а не плодит дубликат.
    db.rag_source_upsert(p, "/data/a.txt", "новый текст", Utc::now())
        .unwrap();
    let stored = db.rag_stored_sources(p).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].content, "новый текст");

    // Удаление под путём снимает и чанки, и сохранённый исходник.
    assert_eq!(db.rag_delete_under(p, "/data").unwrap(), 1);
    assert!(db.rag_stored_sources(p).unwrap().is_empty());
}

#[test]
fn rag_rebuild_dimension_change_flow() {
    let db = db();
    let p = Uuid::new_v4();
    // Индексировано в размерности 2.
    db.rag_insert(&RagDocument::new(p, "a", "c", vec![1.0, 0.0]))
        .unwrap();
    assert_eq!(db.rag_dimension().unwrap(), Some(2));
    assert!(!db.rag_other_profiles_have_docs(p).unwrap());

    // Реиндексация в размерность 3 невозможна без сброса (mismatch).
    assert!(
        db.rag_insert(&RagDocument::new(p, "a", "c", vec![0.0, 1.0, 0.0]))
            .is_err()
    );
    // Сбрасываем векторы и чистим документы профиля, затем индексируем в новой размерности.
    db.rag_delete_all_for_profile(p).unwrap();
    db.rag_reset_vectors().unwrap();
    assert_eq!(db.rag_dimension().unwrap(), None);
    db.rag_insert(&RagDocument::new(p, "a", "c", vec![0.0, 1.0, 0.0]))
        .unwrap();
    assert_eq!(db.rag_dimension().unwrap(), Some(3));
    assert_eq!(db.rag_count(p).unwrap(), 1);
}

#[test]
fn rag_other_profiles_have_docs_detects_neighbors() {
    let db = db();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(a, "a", "c", vec![1.0, 0.0]))
        .unwrap();
    assert!(!db.rag_other_profiles_have_docs(a).unwrap());
    db.rag_insert(&RagDocument::new(b, "b", "c", vec![0.0, 1.0]))
        .unwrap();
    assert!(db.rag_other_profiles_have_docs(a).unwrap());
}

#[test]
fn rag_dim_mismatch_errors() {
    let db = db();
    let p = Uuid::new_v4();
    db.rag_insert(&RagDocument::new(p, "s", "t", vec![1.0, 0.0, 0.0, 0.0]))
        .unwrap();
    let err = db.rag_insert(&RagDocument::new(p, "s", "t", vec![1.0, 0.0]));
    assert!(err.is_err());
}
