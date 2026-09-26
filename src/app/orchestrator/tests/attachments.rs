//! Orchestrator tests — chat file attachments (`/file attach|remove|list`).
//! Part of the [`super`] module (fixtures in mod.rs).
//! See docs/file-attachments.md.

use super::*;
use crate::app::events::FileProgress;
use crate::entities::attachment::AttachMode;
use crate::shared::api::EmbedRole;
use crate::shared::config::AttachmentSettings;
use tokio_util::sync::CancellationToken;

/// Writes a file into the orchestrator's temp directory and returns its path as
/// a string (as the user would type it).
fn write_file(dir: &tempfile::TempDir, name: &str, body: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    path.to_string_lossy().into_owned()
}

/// Waits for the `Attached` outcome of a `/file attach` (reading runs in a
/// background task, so the reply is asynchronous).
async fn wait_attached(
    rx: &mut UnboundedReceiver<AppEvent>,
) -> crate::entities::attachment::AttachmentInfo {
    let ev = wait_for(rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Attached { .. }))
    })
    .await
    .expect("an Attached event");
    match ev {
        AppEvent::FileProgress(FileProgress::Attached { info, .. }) => info,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn attached_file_reaches_the_model_and_persists_in_the_chat() {
    let backend = CapturingBackend::new();
    // Titling off: the automatic title request would overwrite `backend.last`.
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend.clone()), no_auto_cfg());
    let path = write_file(&dir, "notes.md", "секретное число 4242");

    cmd_tx
        .send(AppCommand::FileAttach { path: path.clone() })
        .unwrap();
    let info = wait_attached(&mut evt_rx).await;
    assert_eq!(info.name, "notes.md");
    assert_eq!(info.mode, AttachMode::Inline, "a small file is inlined");

    // The next turn carries the file's text in the system prompt, and the
    // conversation itself stays clean.
    cmd_tx
        .send(AppCommand::SendMessage("что в файле?".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let req = backend.last_request();
    let system = req.system.expect("a system prompt");
    assert!(system.contains("секретное число 4242"), "{system}");
    assert!(system.contains("notes.md"), "{system}");
    assert_eq!(req.messages.len(), 1, "the file isn't a message");

    // It is persisted: the chat file on disk carries the snapshot.
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chats = crate::shared::storage::Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chats()
        .unwrap();
    let saved = &chats[0].attachments;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "notes.md");
    assert!(saved[0].text.contains("4242"));
}

#[tokio::test]
async fn removing_an_attachment_takes_it_out_of_the_request() {
    let backend = CapturingBackend::new();
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(Some(backend.clone()));
    let path = write_file(&dir, "secret.txt", "содержимое-маркер");

    cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
    wait_attached(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::FileRemove {
            target: "secret.txt".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Removed { .. }))
    })
    .await
    .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let req = backend.last_request();
    let system = req.system.unwrap_or_default();
    assert!(
        !system.contains("содержимое-маркер"),
        "/file remove must take the text out of what the model sees: {system}"
    );
}

#[tokio::test]
async fn a_file_over_the_budget_is_attached_by_reference_not_refused() {
    let config = AppConfig {
        attachments: AttachmentSettings {
            max_file_tokens: 10, // ≈40 bytes
            excerpt_tokens: 5,
            ..Default::default()
        },
        ..Default::default()
    };
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch_cfg(None, config);
    let body = "начало файла ".to_string() + &"наполнитель ".repeat(50);
    let path = write_file(&dir, "big.txt", &body);

    cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
    let info = wait_attached(&mut evt_rx).await;
    assert_eq!(
        info.mode,
        AttachMode::ByReference,
        "a file over the budget switches mode instead of being refused"
    );

    // It is listed and addressable by `#N`.
    cmd_tx.send(AppCommand::FileList).unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Listed { .. }))
    })
    .await
    .unwrap();
    match ev {
        AppEvent::FileProgress(FileProgress::Listed { items, .. }) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].name, "big.txt");
        }
        _ => unreachable!(),
    }
    cmd_tx
        .send(AppCommand::FileRemove {
            target: "#1".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Removed { .. }))
    })
    .await
    .unwrap();
}

/// A name two attachments share (docs/research/remove-by-shared-name.md §3):
/// `a/notes.md` and `b/notes.md` were listed alike, and `/file remove notes.md` took the
/// first without saying which. Now the name removes nothing and the refusal names both
/// by `#N` and path; `#2` removes exactly the second, and its note names the source.
#[tokio::test]
async fn a_name_two_attachments_share_removes_nothing_and_names_both() {
    let backend = CapturingBackend::new();
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch_cfg(Some(backend.clone()), no_auto_cfg());
    for folder in ["a", "b"] {
        std::fs::create_dir_all(dir.path().join(folder)).unwrap();
        let path = write_file(
            &dir,
            &format!("{folder}/notes.md"),
            &format!("marker-from-{folder}"),
        );
        cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
        wait_attached(&mut evt_rx).await;
    }
    let is_listed = |e: &AppEvent| matches!(e, AppEvent::FileProgress(FileProgress::Listed { .. }));
    let listed = |ev: Option<AppEvent>| match ev {
        Some(AppEvent::FileProgress(FileProgress::Listed { items, .. })) => items,
        other => panic!("not a listing: {other:?}"),
    };
    let outcome = |e: &AppEvent| {
        matches!(
            e,
            AppEvent::FileProgress(FileProgress::Removed { .. } | FileProgress::Failed(_))
        )
    };
    cmd_tx.send(AppCommand::FileList).unwrap();
    let items = listed(wait_for(&mut evt_rx, is_listed).await);
    let sources: Vec<String> = items.iter().map(|i| i.source.clone()).collect();
    assert_eq!(sources.len(), 2);

    cmd_tx
        .send(AppCommand::FileRemove {
            target: "notes.md".into(),
        })
        .unwrap();
    match wait_for(&mut evt_rx, outcome).await {
        Some(AppEvent::FileProgress(FileProgress::Failed(msg))) => {
            assert!(msg.contains(&format!("#1 {}", sources[0])), "{msg}");
            assert!(msg.contains(&format!("#2 {}", sources[1])), "{msg}");
        }
        other => panic!("a shared name removed something: {other:?}"),
    }
    cmd_tx.send(AppCommand::FileList).unwrap();
    assert_eq!(
        listed(wait_for(&mut evt_rx, is_listed).await).len(),
        2,
        "nothing was removed"
    );

    cmd_tx
        .send(AppCommand::FileRemove {
            target: "#2".into(),
        })
        .unwrap();
    match wait_for(&mut evt_rx, outcome).await {
        Some(AppEvent::FileProgress(FileProgress::Removed { name, source })) => {
            assert_eq!(name, "notes.md");
            assert_eq!(
                source.as_deref(),
                Some(sources[1].as_str()),
                "the note says which one went"
            );
        }
        other => panic!("#2 was not removed: {other:?}"),
    }
    cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let system = backend.last_request().system.unwrap_or_default();
    assert!(
        system.contains("marker-from-a") && !system.contains("marker-from-b"),
        "exactly the second file left the request: {system}"
    );

    // One `notes.md` is left: the name reaches it alone, and the note needs no source.
    cmd_tx
        .send(AppCommand::FileRemove {
            target: "notes.md".into(),
        })
        .unwrap();
    match wait_for(&mut evt_rx, outcome).await {
        Some(AppEvent::FileProgress(FileProgress::Removed { source, .. })) => {
            assert_eq!(source, None)
        }
        other => panic!("the last notes.md was not removed: {other:?}"),
    }
}

#[tokio::test]
async fn reattaching_the_same_file_replaces_the_previous_snapshot() {
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let path = write_file(&dir, "draft.txt", "первая версия");
    cmd_tx
        .send(AppCommand::FileAttach { path: path.clone() })
        .unwrap();
    wait_attached(&mut evt_rx).await;

    std::fs::write(dir.path().join("draft.txt"), "вторая версия, длиннее").unwrap();
    cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
    wait_attached(&mut evt_rx).await;

    cmd_tx.send(AppCommand::FileList).unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Listed { .. }))
    })
    .await
    .unwrap();
    match ev {
        AppEvent::FileProgress(FileProgress::Listed { items, .. }) => {
            assert_eq!(items.len(), 1, "no duplicate entry for the same file");
        }
        _ => unreachable!(),
    }
}

/// The turn snapshot must carry the chat's attachments, otherwise
/// `attachment_read` would see nothing (the tool can't reach `Chat` — the
/// orchestrator owns it).
#[tokio::test]
async fn attachment_read_sees_the_chat_files_through_the_turn_snapshot() {
    use crate::features::tools::attachment::{ATTACHMENT_READ_ID, AttachmentRead};
    use crate::features::tools::{Tool, ToolParams, TurnInfo};

    let (dir, mut orch) = bare_orch();
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "t");
    let body = "страница один и её содержимое\n".repeat(20);
    chat.attachments
        .push(crate::entities::attachment::Attachment::new(
            "doc.txt",
            "/tmp/doc.txt",
            body.clone(),
            body.len(),
            AttachMode::ByReference,
        ));
    let chat_id = chat.id;
    orch.profiles.push(profile.clone());
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    // The same snapshot `start_generation` builds for the turn.
    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    let ctx = crate::features::tools::ToolContext::new(
        orch.tool_deps(Arc::new(MockBackend::scripted(vec![]))),
        ToolParams::from_config(&orch.config),
        TurnInfo {
            profile_id: profile.id,
            chat_id,
            system_message: chat.system_message.clone(),
            effective_sampling: Default::default(),
            last_user_message_at: None,
            attachments: Arc::from(chat.attachments.clone()),
            workspace: chat.workspace.clone(),
            workspace_journal: None,
            files_dir: None,
            files: Arc::from(Vec::new()),
            inputs: Arc::from(Vec::new()),
            images: Arc::from(Vec::new()),
            stages_files: false,
            history: None,
            other_chats: Arc::from(Vec::new()),
            lang: crate::shared::i18n::Lang::Ru,
            cancel: tokio_util::sync::CancellationToken::new(),
            model_name: None,
            engine_mode: Default::default(),
            sessions: None,
            silent_lane: false,
        },
    );

    let out = AttachmentRead
        .invoke(&ctx, serde_json::json!({"name": "doc.txt", "page": 1}))
        .await
        .unwrap()
        .result;
    assert!(out.contains("страница один"), "{out}");
    // And the tool is actually registered under its wire name.
    assert!(orch.registry.get(ATTACHMENT_READ_ID).is_some());
    drop(dir);
}

// ---------- the chat-scoped semantic index (stage 3) ----------

/// A config whose budget forces a by-reference attachment and chunks the text
/// finely enough that a short fixture yields several fragments.
fn indexing_config() -> AppConfig {
    AppConfig {
        attachments: AttachmentSettings {
            max_file_tokens: 10, // ≈40 bytes → anything real goes by reference
            excerpt_tokens: 5,
            ..Default::default()
        },
        rag: crate::shared::config::RagSettings {
            chunk_target_chars: 60,
            chunk_overlap_chars: 10,
            chunk_max_chars: 120,
        },
        ..Default::default()
    }
}

/// Waits for the outcome of background indexing: `Ok(chunks)` when the index was
/// built, `Err(reason)` when it was skipped.
async fn wait_indexed(rx: &mut UnboundedReceiver<AppEvent>) -> Result<usize, String> {
    let ev = wait_for(rx, |e| {
        matches!(
            e,
            AppEvent::FileProgress(FileProgress::Indexed { .. })
                | AppEvent::FileProgress(FileProgress::IndexSkipped { .. })
        )
    })
    .await
    .expect("an indexing outcome");
    match ev {
        AppEvent::FileProgress(FileProgress::Indexed { chunks, .. }) => Ok(chunks),
        AppEvent::FileProgress(FileProgress::IndexSkipped { reason, .. }) => Err(reason),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_by_reference_file_is_indexed_and_searchable_within_the_chat() {
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(None, indexing_config());
    // A distinctive fragment in the middle: it is past the excerpt, so only the
    // index (or page reading) can reach it.
    let body = format!(
        "{}\nрецепт борща со свёклой и капустой\n{}",
        "наполнитель наполнитель ".repeat(20),
        "прочий текст прочий текст ".repeat(20)
    );
    let path = write_file(&dir, "book.txt", &body);

    cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
    let info = wait_attached(&mut evt_rx).await;
    assert_eq!(info.mode, AttachMode::ByReference);
    let chunks = wait_indexed(&mut evt_rx).await.expect("an index was built");
    assert!(chunks > 1, "the fixture must produce several fragments");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let storage = crate::shared::storage::Storage::open(Paths::with_root(dir.path())).unwrap();
    let chat = &storage.json().load_chats().unwrap()[0];
    let attachment = &chat.attachments[0];
    assert_eq!(
        storage.db().attachment_indexed_ids(chat.id).unwrap(),
        vec![attachment.id]
    );
    // And the index actually answers by meaning (the same deterministic embedder
    // the orchestrator used).
    let embedder = crate::shared::api::mock::MockEmbedder::new(16);
    let query = embedder
        .embed(vec!["борщ со свёклой".into()], EmbedRole::Passage)
        .await
        .unwrap()
        .remove(0);
    let hits = storage.db().attachment_search(chat.id, &query, 3).unwrap();
    assert!(
        hits.iter().any(|h| h.text.contains("рецепт борща")),
        "the index must find the fragment: {hits:?}"
    );
    // Another chat sees nothing of it (chat scoping, fork F11).
    assert!(
        storage
            .db()
            .attachment_search(Uuid::new_v4(), &query, 3)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_inline_file_is_not_indexed() {
    // Fork F13: an inline file is already in the prompt in full — indexing it
    // would only return duplicates of what the model can see.
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let path = write_file(&dir, "small.txt", "короткая заметка");
    cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
    assert_eq!(wait_attached(&mut evt_rx).await.mode, AttachMode::Inline);

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let storage = crate::shared::storage::Storage::open(Paths::with_root(dir.path())).unwrap();
    let chat = &storage.json().load_chats().unwrap()[0];
    assert!(
        storage
            .db()
            .attachment_indexed_ids(chat.id)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn removing_an_attachment_drops_its_index() {
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(None, indexing_config());
    let path = write_file(&dir, "book.txt", &"текст документа ".repeat(40));
    cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
    wait_attached(&mut evt_rx).await;
    wait_indexed(&mut evt_rx).await.expect("an index was built");

    cmd_tx
        .send(AppCommand::FileRemove {
            target: "book.txt".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Removed { .. }))
    })
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let storage = crate::shared::storage::Storage::open(Paths::with_root(dir.path())).unwrap();
    let chat = &storage.json().load_chats().unwrap()[0];
    assert!(
        storage
            .db()
            .attachment_indexed_ids(chat.id)
            .unwrap()
            .is_empty(),
        "the removed file's fragments must go with it"
    );
}

#[tokio::test]
async fn attaching_a_missing_file_reports_an_error() {
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let path = dir.path().join("nope.txt").to_string_lossy().into_owned();
    cmd_tx.send(AppCommand::FileAttach { path }).unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Failed(_)))
    })
    .await
    .unwrap();
    match ev {
        AppEvent::FileProgress(FileProgress::Failed(msg)) => assert!(!msg.is_empty()),
        _ => unreachable!(),
    }
}

// ---------- an attachment produced by a tool (spec §9.9, docs/history/youtube-transcript.md) ----------

/// Builds the turn snapshot `start_generation` builds, for a chat that already
/// holds `attachments`.
fn turn_ctx(
    orch: &Orchestrator,
    profile_id: Uuid,
    chat_id: Uuid,
    attachments: Vec<crate::entities::attachment::Attachment>,
) -> crate::features::tools::ToolContext {
    use crate::features::tools::{ToolParams, TurnInfo};
    crate::features::tools::ToolContext::new(
        orch.tool_deps(Arc::new(MockBackend::scripted(vec![]))),
        ToolParams::from_config(&orch.config),
        TurnInfo {
            profile_id,
            chat_id,
            system_message: String::new(),
            effective_sampling: Default::default(),
            last_user_message_at: None,
            attachments: Arc::from(attachments),
            workspace: None,
            workspace_journal: None,
            files_dir: None,
            files: Arc::from(Vec::new()),
            inputs: Arc::from(Vec::new()),
            images: Arc::from(Vec::new()),
            stages_files: false,
            history: None,
            other_chats: Arc::from(Vec::new()),
            lang: crate::shared::i18n::Lang::Ru,
            cancel: CancellationToken::new(),
            model_name: None,
            engine_mode: Default::default(),
            sessions: None,
            silent_lane: false,
        },
    )
}

fn made_up_attachment(
    name: &str,
    source: &str,
    text: &str,
) -> crate::entities::attachment::Attachment {
    crate::entities::attachment::Attachment::new(
        name,
        source,
        text.to_string(),
        text.len(),
        AttachMode::ByReference,
    )
}

/// **The hole this stage exists to close.** Effects are applied to `Chat` only
/// when the whole turn ends, while `ToolContext.attachments` is a turn snapshot —
/// so without the loop mirroring the effect, a tool that says "attached as X, use
/// attachment_read" would be issuing an instruction its own turn cannot carry
/// out. The assertion is therefore the behaviour, not the field: the tool that
/// the result points the model at must find the file in the **next round**.
#[tokio::test]
async fn an_attachment_from_a_tool_is_readable_in_the_next_round_of_the_same_turn() {
    use crate::features::tools::Tool;
    use crate::features::tools::attachment::AttachmentRead;

    let (_d, orch) = bare_orch();
    let body = "расшифровка речи, строка за строкой\n".repeat(20);
    let attachment = made_up_attachment("видео — расшифровка.txt", "youtube:abc#transcript", &body);
    let effects = vec![crate::features::tools::ChatEffect::AddAttachment(Box::new(
        attachment.clone(),
    ))];

    let mut ctx = turn_ctx(&orch, Uuid::new_v4(), Uuid::new_v4(), vec![]);
    // Before the round's effects are mirrored the file does not exist yet…
    let before = AttachmentRead
        .invoke(&ctx, serde_json::json!({"name": attachment.name}))
        .await
        .unwrap()
        .result;
    assert!(!before.contains("строка за строкой"), "{before}");

    super::super::generation::sync_attachments(&mut ctx, &effects);

    // …and after them the very next round can read it.
    let after = AttachmentRead
        .invoke(
            &ctx,
            serde_json::json!({"name": attachment.name, "page": 1}),
        )
        .await
        .unwrap()
        .result;
    assert!(
        after.contains("строка за строкой"),
        "the next round must see the attachment: {after}"
    );

    // Mirroring twice (two rounds carrying the same accumulated effect list)
    // must not duplicate it — the effects vector is cumulative, not per round.
    super::super::generation::sync_attachments(&mut ctx, &effects);
    assert_eq!(ctx.attachments.len(), 1);
    // …and the file is known as this turn's own, which is what lets
    // `attachment_search` say "attached in this turn" instead of "no index"
    // (docs/research/attachment-birth-turn.md F2) — once, however many rounds pass.
    assert_eq!(ctx.born_this_turn.as_ref(), [attachment.id].as_slice());
}

/// The search half of the same hole, end to end through the loop's own mirroring:
/// a search in the round after the tool attached the file names it as born in this
/// turn — the index is built after the turn lands — rather than as a file with no
/// index, which is what the transcript behind attachment-birth-turn.md got.
#[tokio::test]
async fn a_search_in_the_birth_turn_names_the_file_as_just_attached() {
    use crate::features::tools::Tool;
    use crate::features::tools::attachment::AttachmentSearch;

    let (_d, orch) = bare_orch();
    let body = "страница спецификации, строка за строкой\n".repeat(20);
    let attachment = made_up_attachment("spec.md", "https://example.com/spec.md", &body);
    let effects = vec![crate::features::tools::ChatEffect::AddAttachment(Box::new(
        attachment.clone(),
    ))];
    let mut ctx = turn_ctx(&orch, Uuid::new_v4(), Uuid::new_v4(), vec![]);
    super::super::generation::sync_attachments(&mut ctx, &effects);

    let out = AttachmentSearch
        .invoke(&ctx, serde_json::json!({"query": "спецификация"}))
        .await
        .unwrap()
        .result;
    let pages = attachment
        .page_count(ctx.attachment_cfg.page_tokens)
        .to_string();
    let born = ctx.loc.tf(
        "tool.attachment_search.unindexed.born",
        &[("name", "spec.md"), ("pages", &pages)],
    );
    assert!(out.contains(&born), "{out}");
}

/// The other half of F1: what the model was told about is what gets stored —
/// through the same path `/file attach` takes, so the index, the feed note and
/// the status chip all follow.
#[tokio::test]
async fn a_tool_produced_attachment_is_persisted_and_replaces_the_previous_one() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "t");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    let source = "youtube:abc#transcript@0:40-1:20";
    let first = made_up_attachment("first.txt", source, "первый вариант расшифровки");
    let gen_id = Uuid::new_v4();
    orch.gen_state.begin(gen_id, CancellationToken::new());
    orch.handle_done(super::super::generation::GenResult {
        usage: None,
        continuation: None,
        images_withheld: 0,
        id: gen_id,
        chat_id,
        messages: vec![Message::assistant("готово")],
        effects: vec![crate::features::tools::ChatEffect::AddAttachment(Box::new(
            first.clone(),
        ))],
        deleted: vec![],
    });

    let stored = &orch
        .chats
        .iter()
        .find(|c| c.id == chat_id)
        .unwrap()
        .attachments;
    assert_eq!(stored.len(), 1);
    // The stored object is the one the model was told about — the `id` included,
    // since that is the key the background index is written under.
    assert_eq!(stored[0].id, first.id);
    assert_eq!(stored[0].mode, AttachMode::ByReference);
    // It went through the shared attach path, so the UI hears about it.
    let mut attached = false;
    let mut chip = false;
    while let Ok(e) = rx.try_recv() {
        match e {
            AppEvent::FileProgress(FileProgress::Attached { .. }) => attached = true,
            AppEvent::Attachments(items) => chip = !items.is_empty(),
            _ => {}
        }
    }
    assert!(attached, "the feed note is part of attaching");
    assert!(chip, "so is the status-bar chip");

    // Transcribing the same span again replaces it rather than piling up.
    let second = made_up_attachment("second.txt", source, "исправленная расшифровка");
    let gen_id = Uuid::new_v4();
    orch.gen_state.begin(gen_id, CancellationToken::new());
    orch.handle_done(super::super::generation::GenResult {
        usage: None,
        continuation: None,
        images_withheld: 0,
        id: gen_id,
        chat_id,
        messages: vec![],
        effects: vec![crate::features::tools::ChatEffect::AddAttachment(Box::new(
            second.clone(),
        ))],
        deleted: vec![],
    });
    let stored = &orch
        .chats
        .iter()
        .find(|c| c.id == chat_id)
        .unwrap()
        .attachments;
    assert_eq!(stored.len(), 1, "same source → replaced, not duplicated");
    assert_eq!(stored[0].id, second.id);
}

/// A tool that produces an attachment, standing in for `youtube_watch` with a
/// video provider (which a test cannot mock — the registry builds the real
/// Gemini client from config).
struct AttachingTool {
    text: String,
}

#[async_trait::async_trait]
impl crate::features::tools::Tool for AttachingTool {
    fn id(&self) -> String {
        "fake_attach".into()
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "attaches something".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "fake attach"
    }
    async fn invoke(
        &self,
        _ctx: &crate::features::tools::ToolContext,
        _args: serde_json::Value,
    ) -> anyhow::Result<crate::features::tools::ToolOutcome> {
        Ok(crate::features::tools::ToolOutcome::with_effects(
            "attached as fake.txt",
            vec![crate::features::tools::ChatEffect::AddAttachment(Box::new(
                made_up_attachment("fake.txt", "fake:source", &self.text),
            ))],
        ))
    }
}

/// The call site of the mirroring, end to end through the real agentic loop:
/// round 1 attaches, round 2 reads it back. Without
/// [`sync_attachments`](super::super::generation::sync_attachments) being called
/// in the loop, round 2's `attachment_read` looks into a turn snapshot taken
/// before the attachment existed — which is exactly what the tool result would
/// have told the model to do.
#[tokio::test]
async fn the_loop_lets_the_next_round_read_what_the_previous_one_attached() {
    use crate::features::tools::ToolRegistry;
    use crate::features::tools::attachment::{ATTACHMENT_READ_ID, AttachmentRead};
    use crate::shared::api::contract::ToolCallDelta;
    use crate::shared::server::ServerStatus;

    let call = |id: &str, name: &str, args: &str| {
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some(id.into()),
                name: Some(name.into()),
                arguments: args.into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ]
    };
    let backend = Arc::new(MockBackend::sequence(vec![
        call("c1", "fake_attach", "{}"),
        // The model reads back exactly what the first round's result named.
        call("c2", ATTACHMENT_READ_ID, r#"{"name":"fake.txt","page":1}"#),
        vec![
            ChatChunk::Text("готово".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;

    let (_d, mut orch, mut rx) = bare_orch_rx();
    let body = "строка расшифровки, слышимая в ролике\n".repeat(20);
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(AttachingTool { text: body.clone() }));
    registry.register(Arc::new(AttachmentRead));
    orch.registry = Arc::new(registry);
    orch.engines.backend = Some(backend.clone());
    orch.engines.server_status = ServerStatus::Ready;

    let mut profile = Profile::new("P", "sys");
    profile.enabled_tools = vec!["fake_attach".into(), ATTACHMENT_READ_ID.into()];
    let chat = Chat::from_profile(&profile, "t");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    orch.handle_send("посмотри ролик".into());

    // The second round's tool result is the assertion: the file the first round
    // attached must be readable now, not next turn.
    let mut read_back = None;
    while let Some(ev) = wait_for(&mut rx, |e| {
        matches!(e, AppEvent::ToolCall { .. } | AppEvent::Finished { .. })
    })
    .await
    {
        match ev {
            AppEvent::ToolCall { name, result, .. } if name == ATTACHMENT_READ_ID => {
                read_back = Some(result);
                break;
            }
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    let read_back = read_back.expect("the second round called attachment_read");
    assert!(
        read_back.contains("строка расшифровки"),
        "the next round must read what the previous one attached: {read_back}"
    );
}
