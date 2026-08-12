//! Orchestrator tests — image attachments (`/image attach|remove|list`).
//! Part of the [`super`] module (fixtures in mod.rs). See spec §9.10,
//! docs/research/multimodal-images.md.

use super::*;
use crate::app::events::ImageProgress;
use crate::shared::api::ChatRequest;
use crate::shared::api::contract::ChatStream;
use std::io::Cursor;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// An engine that remembers the last request it was given — lets a test assert what
/// actually reached the model, which for images is the whole point.
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
            yield ChatChunk::Text("ok".to_string());
            yield ChatChunk::Finished(crate::shared::api::FinishReason::Stop);
        };
        Ok(Box::pin(s))
    }
}

/// Writes a real PNG into the orchestrator's temp directory and returns its path as the
/// user would type it. A real encode, not a stub: the attach path decodes what it is
/// given, so a fake would test nothing.
fn write_png(dir: &tempfile::TempDir, name: &str, width: u32, height: u32) -> String {
    let buf = image::ImageBuffer::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 60])
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(buf)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

/// Waits for the `Attached` outcome of an `/image attach` (preparation runs in a
/// background task, so the reply is asynchronous).
async fn wait_staged(
    rx: &mut UnboundedReceiver<AppEvent>,
) -> crate::entities::message_image::ImageInfo {
    let ev = wait_for(rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Attached { .. }))
    })
    .await
    .expect("an Attached event");
    match ev {
        AppEvent::ImageProgress(ImageProgress::Attached { info, .. }) => info,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_staged_image_rides_the_next_message_and_persists_with_it() {
    let backend = Arc::new(CapturingBackend {
        last: Mutex::new(None),
    });
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend.clone()));
    let path = write_png(&dir, "chart.png", 64, 32);

    cmd_tx
        .send(AppCommand::ImageAttach { path: path.clone() })
        .unwrap();
    let info = wait_staged(&mut evt_rx).await;
    assert_eq!(info.name, "chart.png");
    assert_eq!((info.width, info.height), (64, 32));

    cmd_tx
        .send(AppCommand::SendMessage("what is this?".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    // The image reached the model on the *message*, not in the system prompt — which is
    // the whole difference from a file attachment.
    let req = backend.last.lock().unwrap().clone().expect("a request");
    assert_eq!(req.messages.len(), 1);
    let images = &req.messages[0].images;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime, "image/png");
    assert!(!images[0].data.is_empty());
    assert!(
        images[0]
            .label
            .as_deref()
            .is_some_and(|l| l.contains("chart.png")),
        "the label must name the file so the model can cite it: {:?}",
        images[0].label
    );
    assert!(
        req.system
            .as_deref()
            .is_none_or(|s| !s.contains("chart.png")),
        "an image must not leak into the system prompt"
    );

    // And it is persisted with the message.
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chats = crate::shared::storage::Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chats()
        .unwrap();
    let user = chats[0]
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .expect("the user message");
    assert_eq!(user.images.len(), 1);
    assert_eq!(user.images[0].name, "chart.png");
    assert!(!user.images[0].data.is_empty());
}

#[tokio::test]
async fn staging_is_consumed_by_the_send_and_does_not_repeat_on_the_next_turn() {
    let backend = Arc::new(CapturingBackend {
        last: Mutex::new(None),
    });
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(Some(backend.clone()));
    let path = write_png(&dir, "one.png", 32, 32);

    cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
    wait_staged(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage("first".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage("second".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    let req = backend.last.lock().unwrap().clone().expect("a request");
    // The first message still carries the image (history replay), the second carries
    // none — staging was emptied by the send rather than re-applied.
    let with_images: Vec<usize> = req
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.images.is_empty())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        with_images,
        vec![0],
        "only the message that was sent with the image may carry it"
    );
}

#[tokio::test]
async fn unstaging_takes_the_image_out_before_it_is_sent() {
    let backend = Arc::new(CapturingBackend {
        last: Mutex::new(None),
    });
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(Some(backend.clone()));
    let path = write_png(&dir, "gone.png", 32, 32);

    cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
    wait_staged(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::ImageRemove {
            target: "#1".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Removed { .. }))
    })
    .await
    .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage("anything there?".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let req = backend.last.lock().unwrap().clone().expect("a request");
    assert!(
        req.messages.iter().all(|m| m.images.is_empty()),
        "an unstaged image must not reach the model"
    );
}

#[tokio::test]
async fn restaging_the_same_file_replaces_the_previous_copy() {
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let path = write_png(&dir, "same.png", 40, 40);

    cmd_tx
        .send(AppCommand::ImageAttach { path: path.clone() })
        .unwrap();
    wait_staged(&mut evt_rx).await;
    cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
    let second = wait_staged(&mut evt_rx).await;
    assert_eq!(second.name, "same.png");

    cmd_tx.send(AppCommand::ImageList).unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Listed { .. }))
    })
    .await
    .unwrap();
    let AppEvent::ImageProgress(ImageProgress::Listed { items }) = ev else {
        unreachable!()
    };
    assert_eq!(items.len(), 1, "re-attaching the same file is idempotent");
}

#[tokio::test]
async fn the_count_cap_refuses_with_a_message_that_says_what_to_do() {
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    // The default cap is 8; stage that many distinct files, then one more.
    for i in 0..8 {
        let path = write_png(&dir, &format!("i{i}.png"), 16, 16);
        cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
        wait_staged(&mut evt_rx).await;
    }
    let extra = write_png(&dir, "ninth.png", 16, 16);
    cmd_tx
        .send(AppCommand::ImageAttach { path: extra })
        .unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Failed(_)))
    })
    .await
    .unwrap();
    let AppEvent::ImageProgress(ImageProgress::Failed(msg)) = ev else {
        unreachable!()
    };
    // Closing the door: the refusal names both ways out, not just the problem.
    assert!(msg.contains('8'), "{msg}");
    assert!(msg.contains("/image remove"), "{msg}");
}

#[tokio::test]
async fn a_non_image_file_is_refused_and_nothing_is_staged() {
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let path = dir.path().join("not-an-image.txt");
    std::fs::write(&path, b"not an image").unwrap();

    cmd_tx
        .send(AppCommand::ImageAttach {
            path: path.to_string_lossy().into_owned(),
        })
        .unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Failed(_)))
    })
    .await
    .unwrap();
    let AppEvent::ImageProgress(ImageProgress::Failed(msg)) = ev else {
        unreachable!()
    };
    assert!(
        msg.contains("png"),
        "the refusal must name what works: {msg}"
    );

    cmd_tx.send(AppCommand::ImageList).unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Listed { .. }))
    })
    .await
    .unwrap();
    let AppEvent::ImageProgress(ImageProgress::Listed { items }) = ev else {
        unreachable!()
    };
    assert!(items.is_empty());
}

#[tokio::test]
async fn a_message_with_only_an_image_is_still_sent() {
    let backend = Arc::new(CapturingBackend {
        last: Mutex::new(None),
    });
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(Some(backend.clone()));
    let path = write_png(&dir, "wordless.png", 32, 32);

    cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
    wait_staged(&mut evt_rx).await;
    // "Look at this" with no words is a complete request; an empty send with nothing
    // staged is still ignored, which the next assertion pins.
    cmd_tx.send(AppCommand::SendMessage(String::new())).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let req = backend.last.lock().unwrap().clone().expect("a request");
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].images.len(), 1);
    assert!(req.messages[0].content.is_empty());
}
