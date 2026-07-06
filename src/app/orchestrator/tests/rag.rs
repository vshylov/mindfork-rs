//! Тесты оркестратора — RAG: индексация/удаление/список/реиндекс. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/refactoring-god-objects.md, этап 3.

use super::*;

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
