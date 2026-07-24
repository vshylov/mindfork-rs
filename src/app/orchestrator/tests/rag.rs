//! Orchestrator tests — RAG: indexing/deletion/listing/rebuild. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

#[tokio::test]
async fn rag_add_indexes_files_and_reports_progress() {
    // No chat engine (RAG doesn't depend on it); MockSupervisor supplies the embedder.
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match active {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    // A folder with two supported files and one unsupported one.
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
            assert_eq!(chunks, 2, "one chunk per file");
            assert_eq!(errors, 0);
            assert!(!cancelled);
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Documents are written under the active chat's profile (isolation).
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

    // Index the same folder twice.
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
        "a repeat add replaces rather than duplicates"
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

    // Delete the whole folder — both files leave the base.
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
            assert_eq!(sources.len(), 2, "two sources in the base");
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

    // Delete the file from disk — reindexing must rely on the stored source text.
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
            assert_eq!(files, 1, "one source reindexed");
            assert_eq!(chunks, 1);
            assert_eq!(errors, 0, "the source is taken from the DB, not from disk");
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
