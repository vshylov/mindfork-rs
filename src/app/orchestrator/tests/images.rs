//! Orchestrator tests — image attachments (`/image attach|remove|list`).
//! Part of the [`super`] module (fixtures in mod.rs). See spec §9.10,
//! docs/research/multimodal-images.md.

use super::*;
use crate::app::events::ImageProgress;
use std::io::Cursor;

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

/// Asks `/image list` and returns what it reports — the staged set as the *user* sees it,
/// which is the only view that proves `#N` addressing lines up with the listing.
async fn staged_list(
    cmd_tx: &UnboundedSender<AppCommand>,
    rx: &mut UnboundedReceiver<AppEvent>,
) -> Vec<crate::entities::message_image::ImageInfo> {
    cmd_tx.send(AppCommand::ImageList).unwrap();
    let ev = wait_for(rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Listed { .. }))
    })
    .await
    .expect("a Listed event");
    match ev {
        AppEvent::ImageProgress(ImageProgress::Listed { items }) => items,
        _ => unreachable!(),
    }
}

/// Sends `text` and returns the images the backend actually received on the message — the
/// tail every "a staged image rides the next message" test shares, whatever put it there.
async fn sent_images(
    backend: &CapturingBackend,
    cmd_tx: &UnboundedSender<AppCommand>,
    rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> Vec<crate::shared::api::ApiImage> {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    wait_for(rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    backend.last_request().messages[0].images.clone()
}

/// Waits for a refusal and returns its message.
async fn wait_refusal(rx: &mut UnboundedReceiver<AppEvent>) -> String {
    let ev = wait_for(rx, |e| {
        matches!(e, AppEvent::ImageProgress(ImageProgress::Failed(_)))
    })
    .await
    .expect("a Failed event");
    match ev {
        AppEvent::ImageProgress(ImageProgress::Failed(msg)) => msg,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_staged_image_rides_the_next_message_and_persists_with_it() {
    let backend = CapturingBackend::new();
    // Titling off: the automatic title request would overwrite `backend.last`.
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend.clone()), no_auto_cfg());
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
    let req = backend.last_request();
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
    let backend = CapturingBackend::new();
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

    let req = backend.last_request();
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
    let backend = CapturingBackend::new();
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
    let req = backend.last_request();
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

    let items = staged_list(&cmd_tx, &mut evt_rx).await;
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
    let msg = wait_refusal(&mut evt_rx).await;
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
    let msg = wait_refusal(&mut evt_rx).await;
    assert!(
        msg.contains("png"),
        "the refusal must name what works: {msg}"
    );

    let items = staged_list(&cmd_tx, &mut evt_rx).await;
    assert!(items.is_empty());
}

/// Opaque RGBA pixels, the shape `arboard` hands over for a clipboard image.
fn rgba(width: u32, height: u32) -> crate::app::events::ClipboardImage {
    let pixels = (0..width as usize * height as usize)
        .flat_map(|i| [(i % 256) as u8, 70, 130, 255])
        .collect();
    crate::app::events::ClipboardImage {
        width,
        height,
        rgba: pixels,
    }
}

#[tokio::test]
async fn a_pasted_image_is_staged_and_travels_with_the_message() {
    let backend = CapturingBackend::new();
    // Titling off: the automatic title request would overwrite `backend.last`.
    let (_dir, cmd_tx, mut evt_rx, _handle) = spawn_orch_cfg(Some(backend.clone()), no_auto_cfg());

    cmd_tx
        .send(AppCommand::ImagePaste(Box::new(rgba(48, 24))))
        .unwrap();
    let info = wait_staged(&mut evt_rx).await;
    // No file exists, so the name is synthetic — and png, which is what the clipboard
    // path always encodes (a screenshot must not pick up jpeg artifacts on small text).
    assert_eq!(info.name, "clipboard.png");
    assert_eq!(info.mime, "image/png");
    assert_eq!((info.width, info.height), (48, 24));

    let images = sent_images(&backend, &cmd_tx, &mut evt_rx, "what is this?").await;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime, "image/png");
}

/// Two pastes must stage two images. Dedupe is by source, and a constant one would make
/// the second screenshot silently replace the first — the failure mode that would cost a
/// user the thing they just copied.
#[tokio::test]
async fn pasting_twice_stages_two_images_under_distinct_names() {
    let (_dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);

    cmd_tx
        .send(AppCommand::ImagePaste(Box::new(rgba(32, 32))))
        .unwrap();
    assert_eq!(wait_staged(&mut evt_rx).await.name, "clipboard.png");
    cmd_tx
        .send(AppCommand::ImagePaste(Box::new(rgba(32, 32))))
        .unwrap();
    assert_eq!(wait_staged(&mut evt_rx).await.name, "clipboard-2.png");

    let items = staged_list(&cmd_tx, &mut evt_rx).await;
    assert_eq!(items.len(), 2);
}

/// A clipboard that reports a size its buffer cannot back is refused with its own
/// message: it is not the user's file being wrong, so it must not read that way.
#[tokio::test]
async fn a_malformed_clipboard_buffer_is_refused_and_nothing_is_staged() {
    let (_dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let mut broken = rgba(16, 16);
    broken.rgba.truncate(10);

    cmd_tx
        .send(AppCommand::ImagePaste(Box::new(broken)))
        .unwrap();
    let msg = wait_refusal(&mut evt_rx).await;
    // *Which* refusal, not merely that one happened: the clipboard-specific message
    // rather than the generic "could not process" one — a user whose own file is fine
    // must not be told it is not. Compared through the key, so it holds in every locale.
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
    assert_eq!(msg, loc.t("ui.err.image_clipboard_unusable"));
    assert!(
        msg.contains("/image attach"),
        "the refusal must name the route that still works: {msg}"
    );

    let items = staged_list(&cmd_tx, &mut evt_rx).await;
    assert!(items.is_empty());
}

#[tokio::test]
async fn a_message_with_only_an_image_is_still_sent() {
    let backend = CapturingBackend::new();
    // Titling off: the automatic title request would overwrite `backend.last`.
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch_cfg(Some(backend.clone()), no_auto_cfg());
    let path = write_png(&dir, "wordless.png", 32, 32);

    cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
    wait_staged(&mut evt_rx).await;
    // "Look at this" with no words is a complete request; an empty send with nothing
    // staged is still ignored, which the next assertion pins.
    cmd_tx.send(AppCommand::SendMessage(String::new())).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let req = backend.last_request();
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].images.len(), 1);
    assert!(req.messages[0].content.is_empty());
}

/// An image named by URL goes through the same staging slot as a file: same cap, same
/// chip, same message. What differs is only where the bytes came from
/// (docs/research/image-url-attach.md, fork F1).
#[tokio::test]
async fn an_image_attached_by_url_is_staged_and_named_after_its_path() {
    use crate::features::image_fetch::stub::{ok_response, png_bytes, serve};

    let backend = CapturingBackend::new();
    // Titling off: the automatic title request would overwrite `backend.last`.
    let (_dir, cmd_tx, mut evt_rx, _handle) = spawn_orch_cfg(Some(backend.clone()), no_auto_cfg());
    let png = png_bytes(48, 24);
    let (base, _h) = serve(vec![ok_response("image/png", &png, true)]);

    cmd_tx
        .send(AppCommand::ImageAttach {
            path: format!("{base}/pics/remote.png"),
        })
        .unwrap();
    let info = wait_staged(&mut evt_rx).await;
    assert_eq!(info.name, "remote.png", "the name comes from the URL path");
    assert_eq!((info.width, info.height), (48, 24));

    // The downloaded pixels ride the message itself — the URL is never handed to the
    // provider, which is what makes this work on all five engines and survive link rot.
    let images = sent_images(&backend, &cmd_tx, &mut evt_rx, "what is this?").await;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime, "image/png");
    assert!(!images[0].data.is_empty());
}

/// Dedupe is by source, and a URL is its own source — so attaching the same address twice
/// replaces rather than duplicates, exactly as re-attaching a file does.
#[tokio::test]
async fn attaching_the_same_url_twice_replaces_the_previous_copy() {
    use crate::features::image_fetch::stub::{ok_response, png_bytes, serve};

    let (_dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let png = png_bytes(20, 20);
    let (base, _h) = serve(vec![
        ok_response("image/png", &png, true),
        ok_response("image/png", &png, true),
    ]);
    let url = format!("{base}/same.png");

    cmd_tx
        .send(AppCommand::ImageAttach { path: url.clone() })
        .unwrap();
    wait_staged(&mut evt_rx).await;
    cmd_tx.send(AppCommand::ImageAttach { path: url }).unwrap();
    wait_staged(&mut evt_rx).await;

    let items = staged_list(&cmd_tx, &mut evt_rx).await;
    assert_eq!(items.len(), 1, "re-attaching the same URL is idempotent");
}

/// The likeliest mistake in this feature: linking the *page* instead of the image on it.
/// The refusal has to name what came back and where the picture's own address is, or the
/// user re-types the same URL (lessons §4).
#[tokio::test]
async fn a_url_that_serves_a_page_is_refused_with_a_message_naming_what_came_back() {
    use crate::features::image_fetch::stub::{ok_response, serve};

    let (_dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let (base, _h) = serve(vec![ok_response(
        "text/html",
        b"<html><body>a page</body></html>",
        true,
    )]);

    cmd_tx
        .send(AppCommand::ImageAttach {
            path: format!("{base}/gallery"),
        })
        .unwrap();
    let msg = wait_refusal(&mut evt_rx).await;
    assert!(
        msg.contains("text/html"),
        "the refusal must name what the server actually served: {msg}"
    );
    let items = staged_list(&cmd_tx, &mut evt_rx).await;
    assert!(items.is_empty(), "nothing may be staged from a page");
}

/// A URL that answers with an error status must not silently stage nothing: the status is
/// the one thing the user can act on.
#[tokio::test]
async fn a_url_that_answers_with_an_error_status_reports_it() {
    use crate::features::image_fetch::stub::serve;

    let (_dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    let (base, _h) = serve(vec![
        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec(),
    ]);

    cmd_tx
        .send(AppCommand::ImageAttach {
            path: format!("{base}/private.png"),
        })
        .unwrap();
    let msg = wait_refusal(&mut evt_rx).await;
    assert!(
        msg.contains("403"),
        "the status belongs in the message: {msg}"
    );
    assert!(
        msg.contains("/image attach"),
        "and the route that still works: {msg}"
    );
}
