//! Orchestrator tests — chat file attachments (`/file attach|remove|list`).
//! Part of the [`super`] module (fixtures in mod.rs).
//! See docs/file-attachments.md.

use super::*;
use crate::app::events::FileProgress;
use crate::entities::attachment::AttachMode;
use crate::shared::api::ChatRequest;
use crate::shared::api::contract::ChatStream;
use crate::shared::config::AttachmentSettings;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// An engine that remembers the last request it was given and replies with a
/// fixed text — lets a test assert what actually reached the model.
struct CapturingBackend {
    last: Mutex<Option<ChatRequest>>,
}

#[async_trait::async_trait]
impl EngineBackend for CapturingBackend {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        _cancel: CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        *self.last.lock().unwrap() = Some(req);
        let s = async_stream::stream! {
            yield ChatChunk::Text("ок".to_string());
            yield ChatChunk::Finished(crate::shared::api::FinishReason::Stop);
        };
        Ok(Box::pin(s))
    }
}

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
    let backend = Arc::new(CapturingBackend {
        last: Mutex::new(None),
    });
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend.clone()));
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
    let req = backend.last.lock().unwrap().clone().expect("a request");
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
    let backend = Arc::new(CapturingBackend {
        last: Mutex::new(None),
    });
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
    let req = backend.last.lock().unwrap().clone().unwrap();
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
        AppEvent::FileProgress(FileProgress::Listed { items }) => {
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
        AppEvent::FileProgress(FileProgress::Listed { items }) => {
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
            lang: crate::shared::i18n::Lang::Ru,
            cancel: tokio_util::sync::CancellationToken::new(),
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
