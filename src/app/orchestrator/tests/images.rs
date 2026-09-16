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

/// The image twin of the shared-name refusal (docs/research/remove-by-shared-name.md
/// F3a): two `chart.png` from two folders, where `/image remove chart.png` unstaged the
/// first without saying which. Now the name unstages neither; `#1` unstages the first,
/// and its note names the source.
#[tokio::test]
async fn a_name_two_staged_images_share_unstages_neither() {
    let (dir, cmd_tx, mut evt_rx, _handle) = spawn_orch(None);
    for (folder, width) in [("a", 32), ("b", 64)] {
        std::fs::create_dir_all(dir.path().join(folder)).unwrap();
        let path = write_png(&dir, &format!("{folder}/chart.png"), width, 16);
        cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
        wait_staged(&mut evt_rx).await;
    }
    let sources: Vec<String> = staged_list(&cmd_tx, &mut evt_rx)
        .await
        .into_iter()
        .map(|i| i.source)
        .collect();
    let outcome = |e: &AppEvent| {
        matches!(
            e,
            AppEvent::ImageProgress(ImageProgress::Removed { .. } | ImageProgress::Failed(_))
        )
    };

    cmd_tx
        .send(AppCommand::ImageRemove {
            target: "chart.png".into(),
        })
        .unwrap();
    match wait_for(&mut evt_rx, outcome).await {
        Some(AppEvent::ImageProgress(ImageProgress::Failed(msg))) => {
            assert!(msg.contains(&format!("#1 {}", sources[0])), "{msg}");
            assert!(msg.contains(&format!("#2 {}", sources[1])), "{msg}");
        }
        other => panic!("a shared name unstaged something: {other:?}"),
    }
    assert_eq!(
        staged_list(&cmd_tx, &mut evt_rx).await.len(),
        2,
        "nothing was unstaged"
    );

    cmd_tx
        .send(AppCommand::ImageRemove {
            target: "#1".into(),
        })
        .unwrap();
    match wait_for(&mut evt_rx, outcome).await {
        Some(AppEvent::ImageProgress(ImageProgress::Removed { name, source })) => {
            assert_eq!(name, "chart.png");
            assert_eq!(source.as_deref(), Some(sources[0].as_str()));
        }
        other => panic!("#1 was not unstaged: {other:?}"),
    }
    let left = staged_list(&cmd_tx, &mut evt_rx).await;
    assert_eq!(
        (left.len(), left[0].width),
        (1, 64),
        "exactly the first went"
    );

    // One `chart.png` is left: the name reaches it alone, and the note needs no source.
    cmd_tx
        .send(AppCommand::ImageRemove {
            target: "chart.png".into(),
        })
        .unwrap();
    match wait_for(&mut evt_rx, outcome).await {
        Some(AppEvent::ImageProgress(ImageProgress::Removed { source, .. })) => {
            assert_eq!(source, None)
        }
        other => panic!("the last chart.png was not unstaged: {other:?}"),
    }
}

/// An engine that captures the last request like [`CapturingBackend`], answers the image
/// question with whatever the test sets — the user switching models mid-chat — and counts
/// being asked.
struct SwitchableSight {
    inner: Arc<CapturingBackend>,
    unsupported: std::sync::atomic::AtomicBool,
    asked: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl EngineBackend for SwitchableSight {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::shared::api::contract::ChatStream> {
        self.inner.chat_stream(req, cancel).await
    }
    async fn vision(&self) -> crate::shared::api::VisionSupport {
        self.asked
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.unsupported.load(std::sync::atomic::Ordering::Relaxed) {
            crate::shared::api::VisionSupport::Unsupported
        } else {
            crate::shared::api::VisionSupport::Supported
        }
    }
}

/// Every language's rendering of the once-per-chat note for `n` images.
fn withheld_notes(n: usize) -> Vec<String> {
    crate::shared::i18n::Lang::ALL
        .iter()
        .map(|l| {
            crate::shared::i18n::locale(*l).tf("ui.chat.images_withheld", &[("n", &n.to_string())])
        })
        .collect()
}

/// A chat that got an image while the engine could see it, then switched to one that
/// cannot: the stored image went on every turn and the engine refused each — a `500` from
/// llama.cpp without a projector, a `404` from a gateway (docs/research/history-images-no-vision.md
/// §2.1). Now the request carries a marker naming it, the chat is told once, the next turn
/// is not told again, and the stored message keeps its pixels. A turn in a chat without
/// images does not ask the engine at all.
#[tokio::test]
async fn a_history_image_goes_as_a_marker_once_the_engine_takes_none() {
    let capturing = CapturingBackend::new();
    let backend = Arc::new(SwitchableSight {
        inner: Arc::clone(&capturing),
        unsupported: std::sync::atomic::AtomicBool::new(false),
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(
        Some(backend.clone() as Arc<dyn EngineBackend>),
        no_auto_cfg(),
    );

    // No images yet: the turn asks nothing.
    cmd_tx
        .send(AppCommand::SendMessage("hello".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let asked_before_attach = backend.asked.load(std::sync::atomic::Ordering::Relaxed);

    // A vision engine: the image rides the message.
    let path = write_png(&dir, "figure.png", 16, 16);
    cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
    wait_staged(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage("what is this?".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let seen: usize = capturing
        .last_request()
        .messages
        .iter()
        .map(|m| m.images.len())
        .sum();
    assert_eq!(seen, 1, "a vision engine gets the image");

    // The switch: the next turn replays the history to an engine that takes none.
    backend
        .unsupported
        .store(true, std::sync::atomic::Ordering::Relaxed);
    cmd_tx
        .send(AppCommand::SendMessage("and the corner?".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let req = capturing.last_request();
    assert!(
        req.messages.iter().all(|m| m.images.is_empty()),
        "no image may reach an engine that refuses them"
    );
    let carrier = req
        .messages
        .iter()
        .find(|m| m.content.starts_with("what is this?"))
        .expect("the message that carried the image");
    let markers: Vec<String> = crate::shared::i18n::Lang::ALL
        .iter()
        .map(|l| {
            let loc = crate::shared::i18n::locale(*l);
            let label = loc.tf("prompt.images.label", &[("n", "1"), ("name", "figure.png")]);
            loc.tf(
                "prompt.images.withheld",
                &[("image", label.trim_end_matches(':'))],
            )
        })
        .collect();
    assert!(
        markers
            .iter()
            .any(|m| carrier.content.ends_with(m.as_str())),
        "the image goes as a marker naming it: {:?}",
        carrier.content
    );
    let notes = withheld_notes(1);
    // Bounded (docs/lessons.md §2): a regression that never notes must fail, not hang.
    let told = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Notice(_))),
    )
    .await
    .ok()
    .flatten()
    .expect("the chat is told");
    let AppEvent::Notice(text) = told else {
        unreachable!()
    };
    assert!(notes.contains(&text), "{text}");

    // The next turn of the same chat, same engine: not told again.
    cmd_tx
        .send(AppCommand::SendMessage("thanks".into()))
        .unwrap();
    let mut repeated = false;
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Notice(t) if notes.contains(&t) => repeated = true,
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    while let Ok(ev) = evt_rx.try_recv() {
        if matches!(&ev, AppEvent::Notice(t) if notes.contains(t)) {
            repeated = true;
        }
    }
    assert!(!repeated, "told once per chat, not on every turn");
    assert_eq!(asked_before_attach, 0, "a chat without images asks nothing");

    let chats = crate::shared::storage::Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chats()
        .unwrap();
    let stored: usize = chats[0].messages.iter().map(|m| m.images.len()).sum();
    assert_eq!(stored, 1, "the stored message keeps its image");
}

/// The note is once per chat, not once per app: another chat is told on its own, a turn
/// that withheld nothing says nothing, and once the engine's facts are asked again — the
/// settings applied, the server's readiness flipped — the chat is told again, since the
/// new engine may be another model altogether.
#[tokio::test]
async fn the_withheld_note_is_once_a_chat_until_the_engine_is_asked_again() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let notices = |rx: &mut UnboundedReceiver<AppEvent>| {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Notice(t) = ev {
                out.push(t);
            }
        }
        out
    };
    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());

    orch.note_withheld_images(first, 2);
    assert_eq!(notices(&mut rx).len(), 1);
    orch.note_withheld_images(first, 2);
    orch.note_withheld_images(first, 0);
    orch.note_withheld_images(second, 0);
    assert!(notices(&mut rx).is_empty());
    orch.note_withheld_images(second, 1);
    assert_eq!(
        notices(&mut rx),
        [orch
            .ui_locale()
            .tf("ui.chat.images_withheld", &[("n", "1")])]
    );

    orch.refresh_engine_facts();
    orch.note_withheld_images(first, 2);
    assert_eq!(
        notices(&mut rx).len(),
        1,
        "told again after the engine changed"
    );
}
