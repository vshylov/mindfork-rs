//! Image attachments (`/image attach|remove|list`, spec §9.10).
//!
//! An image is **staged for the next message**, not pinned to the chat (fork F1 of
//! docs/research/multimodal-images.md): every provider takes images as message content
//! parts, and a set that could change at the head of the prompt would invalidate the
//! prefix cache on every mutation. So staging lives here, in the orchestrator, keyed by
//! chat — and the turn that sends the message consumes it ([`Orchestrator::take_staged_images`]).
//!
//! Staging is deliberately **session-only**: it is not written to the chat file and does
//! not survive a restart. What the user staged but never sent is a half-finished thought,
//! not conversation state; persisting it would resurrect images into a message written
//! days later. Once sent, the image lives on the `Message` and is persisted with it.
//!
//! Reading, decoding and downscaling run in a **background task** (a 10 MB photo must not
//! block the command loop) and come back through an internal channel, the same shape
//! [`super::attachments`] uses. The capability probe rides along: asking the engine
//! whether it takes images at all is what lets an attach be *refused* with an answer
//! rather than silently failing at send time.

use std::collections::HashMap;

use base64::Engine as _;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::attachment::{Resolved, handle_number, handle_range, name_is_shared};
use crate::entities::message_image::{MessageImage, infos, resolve_target};
use crate::features::image_command::ImageProgress;
use crate::features::image_fetch::{self, FetchError, FetchedImage};
use crate::features::image_prepare::{PrepareError, PreparedImage, prepare, prepare_rgba};
use crate::shared::api::VisionSupport;
use crate::shared::config::{ImageSettings, ServerMode};
use crate::shared::i18n::Locale;

use super::Orchestrator;

/// An image read and prepared by the background task (internal channel).
pub(super) struct ImageAttachResult {
    /// The chat the command was issued in — it may no longer be active when the task
    /// finishes, and the image still belongs to it.
    pub(super) chat_id: Uuid,
    /// What the engine said about image support while the file was being prepared.
    /// Carried through so the note can be written once, next to the outcome.
    pub(super) vision: VisionSupport,
    pub(super) outcome: Result<MessageImage, String>,
}

/// What the user asked to stage, before anything has been read. The split exists because
/// exactly one of these needs the network, and therefore cannot run on the blocking pool
/// with the decoder (fork F1, docs/research/image-url-attach.md).
enum Staging {
    /// Pixels that are already on this machine.
    Local(ImageSource),
    /// A URL to download first (`/image attach <url>`).
    Url(String),
}

/// Where a staging request's pixels come from — the **only** thing that differs between
/// `/image attach <path>`, `/image attach <url>` and `/image paste`. Everything around it
/// (the cap, the capability probe, the background encode, the staging itself) is one path,
/// in [`Orchestrator::stage_image`].
enum ImageSource {
    /// A file the user named. Decoded and normalized by its own extension.
    File(String),
    /// Pixels off the system clipboard, plus the synthetic name they will carry (there is
    /// no file to take one from).
    Clipboard {
        image: Box<crate::app::events::ClipboardImage>,
        name: String,
    },
    /// Bytes that came off the network, plus the URL they came from — which is both the
    /// image's `source` (so re-attaching the same URL replaces it) and where its display
    /// name is taken from.
    Downloaded {
        fetched: Box<FetchedImage>,
        url: String,
    },
}

impl Staging {
    /// Resolves whatever the user named into pixels in hand. Async because a URL is the one
    /// source that has to be fetched; a local source passes straight through.
    async fn resolve(
        self,
        cfg: &ImageSettings,
        loc: &'static Locale,
    ) -> Result<ImageSource, String> {
        match self {
            Staging::Local(source) => Ok(source),
            Staging::Url(url) => {
                let fetched = image_fetch::fetch(&url, cfg.max_bytes)
                    .await
                    .map_err(|e| localize_fetch(&e, cfg.max_bytes, loc))?;
                Ok(ImageSource::Downloaded {
                    fetched: Box::new(fetched),
                    url,
                })
            }
        }
    }
}

impl ImageSource {
    /// Turns the source into an attachable image. Runs on the blocking pool.
    fn prepare(self, cfg: &ImageSettings, loc: &'static Locale) -> Result<MessageImage, String> {
        match self {
            ImageSource::File(path) => prepare_image(std::path::Path::new(&path), cfg, loc),
            ImageSource::Clipboard { image, name } => prepare_clipboard(*image, name, cfg, loc),
            ImageSource::Downloaded { fetched, url } => prepare_downloaded(*fetched, url, cfg, loc),
        }
    }
}

impl Orchestrator {
    /// Starts staging an image for the next message (`/image attach <path|url>`).
    ///
    /// The two forms are told apart by the scheme rather than by a separate subcommand
    /// (fork F3): no path on either platform begins with `http://` or `https://`.
    pub(super) fn handle_image_attach(&mut self, path: String) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        if image_fetch::looks_like_url(&path) {
            self.stage_image(Staging::Url(path));
        } else {
            self.stage_image(Staging::Local(ImageSource::File(path)));
        }
    }

    /// Stages an image taken off the system clipboard (`Ctrl+V`, `/image paste`).
    ///
    /// The name is chosen here rather than in the background task because it depends on
    /// what is *already* staged, which only the orchestrator knows.
    pub(super) fn handle_image_paste(&mut self, image: crate::app::events::ClipboardImage) {
        let Some(chat_id) = self.active_id else {
            self.fail_image(self.ui_locale().t("ui.err.image_no_active_chat"));
            return;
        };
        let name = self.free_clipboard_name(chat_id);
        self.stage_image(Staging::Local(ImageSource::Clipboard {
            image: Box::new(image),
            name,
        }));
    }

    /// The one staging path, whatever the pixels came from: refuse early if there is
    /// nowhere or no room to put them, ask the engine whether it takes images at all, then
    /// prepare off the command loop and hand the result back through [`ImageAttachResult`].
    fn stage_image(&mut self, source: Staging) {
        let Some(chat_id) = self.active_id else {
            self.fail_image(self.ui_locale().t("ui.err.image_no_active_chat"));
            return;
        };
        let cfg = self.config.images;
        // The cap is checked *before* the work, so a user who is already at the limit is
        // told immediately instead of after a multi-second decode.
        let staged = self.staged_images.get(&chat_id).map_or(0, Vec::len);
        if staged >= cfg.max_count {
            let msg = self.ui_locale().tf(
                "ui.err.image_too_many",
                &[("max", &cfg.max_count.to_string())],
            );
            self.fail_image(&msg);
            return;
        }

        let loc = self.ui_locale();
        // `Unknown` when the engine is not ready or cannot be asked — that is the
        // optimistic path (fork F4), not a failure.
        let backend = self.engines.backend_if_ready(loc).ok();
        let managed = self.config.engine.mode == ServerMode::Managed;
        let tx = self.image_tx.clone();
        tokio::spawn(async move {
            let vision = match &backend {
                Some(b) => b.vision().await,
                None => VisionSupport::Unknown,
            };
            if vision == VisionSupport::Unsupported {
                // Close the door: name what *would* work rather than only what does not.
                // For a managed server that is the projector setting; otherwise it is the
                // model/provider choice.
                let key = if managed {
                    "ui.err.image_no_vision_managed"
                } else {
                    "ui.err.image_no_vision"
                };
                let _ = tx.send(ImageAttachResult {
                    chat_id,
                    vision,
                    outcome: Err(loc.t(key).to_string()),
                });
                return;
            }
            // A URL is downloaded here, before the decoder ever runs; every other source
            // already has its pixels. The cap was checked above, so this cannot start a
            // download for a message that has no room for it.
            let outcome = match source.resolve(&cfg, loc).await {
                Ok(source) => tokio::task::spawn_blocking(move || source.prepare(&cfg, loc))
                    .await
                    .unwrap_or_else(|e| {
                        Err(loc.tf("ui.err.image_failed", &[("err", &e.to_string())]))
                    }),
                Err(err) => Err(err),
            };
            let _ = tx.send(ImageAttachResult {
                chat_id,
                vision,
                outcome,
            });
        });
    }

    /// The first unused `clipboard*.png` name among the chat's staged images, so two
    /// pastes are told apart in `/image list` and in the label the model reads.
    fn free_clipboard_name(&self, chat_id: Uuid) -> String {
        let staged = self.staged_images.get(&chat_id);
        let taken = |name: &str| {
            staged.is_some_and(|v| v.iter().any(|i| i.name.eq_ignore_ascii_case(name)))
        };
        if !taken(CLIPBOARD_NAME) {
            return CLIPBOARD_NAME.to_string();
        }
        // Bounded by `max_count`, so this always terminates well before the guard.
        (2..)
            .map(|n| format!("clipboard-{n}.png"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| CLIPBOARD_NAME.to_string())
    }

    /// Applies the result of a background prepare: stages the image and reports.
    pub(super) fn handle_image_result(&mut self, res: ImageAttachResult) {
        let image = match res.outcome {
            Ok(i) => i,
            Err(err) => {
                self.fail_image(&err);
                return;
            }
        };
        // The chat may have been deleted while the image was being decoded.
        if !self.chats.iter().any(|c| c.id == res.chat_id) {
            return;
        }
        let staged = self.staged_images.entry(res.chat_id).or_default();
        // Staging the same file twice replaces the earlier copy — idempotent, like
        // re-attaching a file.
        staged.retain(|i| i.source != image.source);
        let info = (&image).into();
        staged.push(image);
        let count = staged.len();

        self.emit_image_progress(ImageProgress::Attached {
            info,
            staged: count,
        });
        // Only for the *first* image of the message being composed. An engine without
        // `/props` cannot say on any attach, so repeating the caveat per image would
        // turn a useful warning into noise the user learns to skip past — and it is
        // worth saying again on the next message, which is exactly what this gives.
        if res.vision == VisionSupport::Unknown && count == 1 {
            self.emit_image_progress(ImageProgress::VisionUnknown);
        }
        self.emit_staged_images();
    }

    /// Tells the user that a turn sent `count` of the chat's images as markers, because the
    /// engine takes none — **once per chat** until the engine's facts are asked again
    /// (docs/research/history-images-no-vision.md, fork W1(a)). The model says it cannot see
    /// them when asked; this line is for the user, who still sees them in the feed.
    pub(super) fn note_withheld_images(&mut self, chat_id: Uuid, count: usize) {
        if count == 0 || !self.images_withheld_noted.insert(chat_id) {
            return;
        }
        let note = self
            .ui_locale()
            .tf("ui.chat.images_withheld", &[("n", &count.to_string())]);
        let _ = self.evt_tx.send(AppEvent::Notice(note));
    }

    /// Unstages an image by name/path/`#N` (`/image remove <target>`). A name two staged
    /// images share unstages neither, exactly as `/file remove` refuses one
    /// (docs/research/remove-by-shared-name.md F3a).
    pub(super) fn handle_image_remove(&mut self, target: String) {
        let Some(chat_id) = self.active_id else {
            self.fail_image(self.ui_locale().t("ui.err.image_no_active_chat"));
            return;
        };
        let loc = self.ui_locale();
        let staged = self.staged_images.entry(chat_id).or_default();
        let idx = match resolve_target(staged, &target) {
            Resolved::One(idx) => idx,
            Resolved::Shared(hits) => {
                let candidates =
                    Self::candidate_lines(hits.iter().map(|&i| (i, staged[i].source.as_str())));
                let msg = loc.tf(
                    "ui.err.image_name_shared",
                    &[("target", target.trim()), ("candidates", &candidates)],
                );
                self.fail_image(&msg);
                return;
            }
            Resolved::Nothing => {
                // The message says what `remove` can and cannot reach: an image already
                // sent is part of the conversation, and pretending otherwise sends the
                // user hunting for a command that does not exist.
                let msg = match handle_number(&target).filter(|_| !staged.is_empty()) {
                    Some(n) => loc.tf(
                        "ui.err.image_no_such_number",
                        &[
                            ("n", &n.to_string()),
                            ("range", &handle_range(staged.len())),
                        ],
                    ),
                    None => loc.tf("ui.err.image_not_staged", &[("target", target.trim())]),
                };
                self.fail_image(&msg);
                return;
            }
        };
        let shared = name_is_shared(staged, idx, |i| i.name.as_str());
        let removed = staged.remove(idx);
        self.emit_image_progress(ImageProgress::Removed {
            name: removed.name,
            source: shared.then_some(removed.source),
        });
        self.emit_staged_images();
    }

    /// Lists what is staged for the next message (`/image list`).
    pub(super) fn handle_image_list(&mut self) {
        let Some(chat_id) = self.active_id else {
            self.fail_image(self.ui_locale().t("ui.err.image_no_active_chat"));
            return;
        };
        let items = self
            .staged_images
            .get(&chat_id)
            .map(|v| infos(v))
            .unwrap_or_default();
        self.emit_image_progress(ImageProgress::Listed { items });
    }

    /// Hands the staged images to the message being sent and clears the staging slot.
    /// Called by the generation path exactly once per user message.
    pub(super) fn take_staged_images(&mut self, chat_id: Uuid) -> Vec<MessageImage> {
        let taken = self.staged_images.remove(&chat_id).unwrap_or_default();
        if !taken.is_empty() {
            // The chip has to go out the moment the images leave staging, or it keeps
            // advertising a cost that has already moved into the conversation.
            self.emit_staged_images();
        }
        taken
    }

    /// Drops a deleted chat's staging slot, so a new chat reusing the screen never
    /// inherits images the user staged elsewhere.
    pub(super) fn forget_staged_images(&mut self, chat_id: Uuid) {
        if self.staged_images.remove(&chat_id).is_some() {
            self.emit_staged_images();
        }
    }

    /// Sends the active chat's staged-image cards to the UI (the status-bar chip).
    pub(super) fn emit_staged_images(&self) {
        let items = self
            .active_id
            .and_then(|id| self.staged_images.get(&id))
            .map(|v| infos(v))
            .unwrap_or_default();
        let _ = self.evt_tx.send(AppEvent::StagedImages(items));
    }

    fn emit_image_progress(&self, progress: ImageProgress) {
        let _ = self.evt_tx.send(AppEvent::ImageProgress(progress));
    }

    fn fail_image(&self, msg: &str) {
        self.emit_image_progress(ImageProgress::Failed(msg.to_string()));
    }
}

/// The staging store: images waiting for the next message, per chat.
pub(super) type StagedImages = HashMap<Uuid, Vec<MessageImage>>;

/// The display name a pasted image gets when nothing else is staged under it. `.png`
/// is not a guess — [`prepare_rgba`] always encodes a clipboard image as png.
const CLIPBOARD_NAME: &str = "clipboard.png";

/// Encodes clipboard pixels into an attachable image (blocking — runs on the blocking
/// pool). Errors are already localized (axis B).
fn prepare_clipboard(
    image: crate::app::events::ClipboardImage,
    name: String,
    cfg: &ImageSettings,
    loc: &'static Locale,
) -> Result<MessageImage, String> {
    let prepared = prepare_rgba(image.width, image.height, &image.rgba, cfg.downscale_px).map_err(
        |e| match e {
            // The clipboard handed over a buffer that does not match the size it
            // reported — not the user's doing, and nothing they can fix by choosing a
            // different file, so it must not read like "your image is broken".
            PrepareError::Undecodable => loc.t("ui.err.image_clipboard_unusable").to_string(),
            PrepareError::Failed(err) => loc.tf("ui.err.image_failed", &[("err", &err)]),
        },
    )?;
    // The byte cap applies to what will actually be re-sent every turn. For a file it is
    // checked before decoding; here there is no file, so the encoded result is the
    // honest equivalent — the downscale normally keeps it far below.
    if prepared.bytes.len() as u64 > cfg.max_bytes {
        return Err(loc.tf(
            "ui.err.image_too_big",
            &[
                (
                    "size",
                    &crate::entities::attachment::format_bytes(prepared.bytes.len()),
                ),
                (
                    "max",
                    &crate::entities::attachment::format_bytes(cfg.max_bytes as usize),
                ),
            ],
        ));
    }
    Ok(staged(
        prepared,
        name,
        // A source that cannot collide with a file path or with another paste: dedupe is
        // by source, and two screenshots must not silently become one.
        format!("clipboard:{}", Uuid::new_v4()),
    ))
}

/// Turns downloaded bytes into an attachable image (blocking — runs on the blocking pool).
///
/// The size cap was already enforced *during* the download, so what is left here is the
/// decode — plus the one refusal that deserves its own wording (fork F6).
fn prepare_downloaded(
    fetched: FetchedImage,
    url: String,
    cfg: &ImageSettings,
    loc: &'static Locale,
) -> Result<MessageImage, String> {
    let prepared = prepare(&fetched.bytes, cfg.downscale_px).map_err(|e| match e {
        PrepareError::Undecodable => not_an_image(&fetched.content_type, loc),
        PrepareError::Failed(err) => loc.tf("ui.err.image_failed", &[("err", &err)]),
    })?;
    let name = image_fetch::display_name(&fetched.final_url, extension(prepared.mime));
    // The URL as typed is the source, so attaching it twice replaces rather than
    // duplicates — the behaviour a file attach already has.
    Ok(staged(prepared, name, url))
}

/// The refusal for downloaded bytes that would not decode. When the server *said* what it
/// was sending and it was not an image, say that: linking the page instead of the image is
/// the likeliest mistake in this feature, and "unsupported format" would send the user
/// looking in the wrong place (lessons §4 — a message has to close the door).
fn not_an_image(content_type: &str, loc: &'static Locale) -> String {
    if content_type.is_empty() || content_type.starts_with("image/") {
        loc.t("ui.err.image_undecodable").to_string()
    } else {
        loc.tf("ui.err.image_url_not_image", &[("type", content_type)])
    }
}

/// The file extension matching a prepared image's MIME type — only ever the two
/// [`prepare`] can produce.
fn extension(mime: &str) -> &'static str {
    if mime == "image/jpeg" { "jpg" } else { "png" }
}

/// Localizes a download failure (axis B). Every arm names what happened rather than
/// "could not attach": the user picked this URL and can act on the difference between a
/// 404, a redirect loop and a file that is simply too big.
fn localize_fetch(err: &FetchError, max_bytes: u64, loc: &'static Locale) -> String {
    match err {
        FetchError::Scheme => loc.t("ui.err.image_url_scheme").to_string(),
        FetchError::Malformed => loc.t("ui.err.image_url_malformed").to_string(),
        FetchError::TooManyRedirects => loc.tf(
            "ui.err.image_url_redirects",
            &[("max", &image_fetch::MAX_REDIRECTS.to_string())],
        ),
        FetchError::Request(e) => loc.tf("ui.err.image_url_request", &[("err", e)]),
        FetchError::Status(code) => {
            loc.tf("ui.err.image_url_status", &[("status", &code.to_string())])
        }
        FetchError::TooBig => loc.tf(
            "ui.err.image_url_too_big",
            &[(
                "max",
                &crate::entities::attachment::format_bytes(max_bytes as usize),
            )],
        ),
        FetchError::Empty => loc.t("ui.err.image_url_empty").to_string(),
    }
}

/// Builds the staged image from prepared pixels. Shared by every source, so a new one
/// cannot drift in how the payload is encoded or what the chip is told.
fn staged(prepared: PreparedImage, name: String, source: String) -> MessageImage {
    let data = base64::engine::general_purpose::STANDARD.encode(&prepared.bytes);
    MessageImage::new(
        name,
        source,
        prepared.mime,
        prepared.width,
        prepared.height,
        data,
    )
}

/// Reads an image file and prepares it for attachment (blocking — runs on the blocking
/// pool). Errors are already localized (axis B): they go straight into a feed note.
fn prepare_image(
    path: &std::path::Path,
    cfg: &ImageSettings,
    loc: &'static Locale,
) -> Result<MessageImage, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| loc.tf("ui.err.image_unavailable", &[("err", &e.to_string())]))?;
    if !meta.is_file() {
        return Err(loc.t("ui.err.image_not_a_file").to_string());
    }
    // Size is checked before reading, let alone decoding: the point of the ceiling is
    // that an enormous file never reaches the decoder at all.
    if meta.len() > cfg.max_bytes {
        return Err(loc.tf(
            "ui.err.image_too_big",
            &[
                (
                    "size",
                    &crate::entities::attachment::format_bytes(meta.len() as usize),
                ),
                (
                    "max",
                    &crate::entities::attachment::format_bytes(cfg.max_bytes as usize),
                ),
            ],
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| loc.tf("ui.err.image_unavailable", &[("err", &e.to_string())]))?;
    let prepared = prepare(&bytes, cfg.downscale_px).map_err(|e| match e {
        PrepareError::Undecodable => loc.t("ui.err.image_undecodable").to_string(),
        PrepareError::Failed(err) => loc.tf("ui.err.image_failed", &[("err", &err)]),
    })?;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Ok(staged(
        prepared,
        name,
        crate::features::rag_ingest::canonical_source(path),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let buf = image::ImageBuffer::from_fn(width, height, |x, _| {
            image::Rgb([(x % 256) as u8, 40, 90])
        });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(buf)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn en() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::En)
    }

    #[test]
    fn prepares_an_image_and_reports_its_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        std::fs::write(&path, png(64, 32)).unwrap();

        let image = prepare_image(&path, &ImageSettings::default(), en()).unwrap();
        assert_eq!(image.name, "shot.png");
        assert_eq!(image.mime, "image/png");
        assert_eq!((image.width, image.height), (64, 32));
        assert!(image.bytes > 0);
        // The payload is base64 of the encoded image, not of the path or the raw file.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .unwrap();
        assert_eq!(decoded.len(), image.bytes);
        assert_eq!(&decoded[1..4], b"PNG");
    }

    #[test]
    fn an_oversized_file_is_refused_before_it_is_decoded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        std::fs::write(&path, png(200, 200)).unwrap();
        let cfg = ImageSettings {
            max_bytes: 16,
            ..ImageSettings::default()
        };
        let err = prepare_image(&path, &cfg, en()).unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn a_non_image_and_a_missing_file_are_refused_with_their_own_messages() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("not-an-image.txt");
        std::fs::write(&text, b"just words").unwrap();
        let err = prepare_image(&text, &ImageSettings::default(), en()).unwrap_err();
        assert!(
            err.contains("png"),
            "the refusal must name what works: {err}"
        );

        let missing = dir.path().join("nope.png");
        let err = prepare_image(&missing, &ImageSettings::default(), en()).unwrap_err();
        assert!(err.contains("unavailable"), "{err}");

        let err = prepare_image(dir.path(), &ImageSettings::default(), en()).unwrap_err();
        assert!(err.contains("not a file"), "{err}");
    }

    #[test]
    fn a_large_image_is_downscaled_before_it_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.png");
        std::fs::write(&path, png(2400, 1200)).unwrap();
        let image = prepare_image(&path, &ImageSettings::default(), en()).unwrap();
        assert_eq!((image.width, image.height), (1568, 784));
        // And the stored payload really is the smaller copy, not the original file.
        assert!(image.bytes < std::fs::metadata(&path).unwrap().len() as usize);
    }

    /// A download failure is judged by *which* message comes back, not merely that one
    /// does: the difference between "404", "too many redirects" and "over the limit" is
    /// the only thing the user can act on (lessons §4).
    #[test]
    fn every_download_failure_names_what_happened() {
        let max = ImageSettings::default().max_bytes;
        let msg = |e: FetchError| localize_fetch(&e, max, en());
        assert!(msg(FetchError::Status(404)).contains("404"));
        assert!(msg(FetchError::Request("dns error".into())).contains("dns error"));
        assert!(
            msg(FetchError::TooManyRedirects).contains(&image_fetch::MAX_REDIRECTS.to_string())
        );
        assert!(msg(FetchError::TooBig).contains("10"), "the limit in MB");
        // The two refusals that have to point somewhere: a scheme this cannot fetch, and
        // an address that is not one — both name a route that does work.
        assert!(msg(FetchError::Scheme).contains("/image attach"));
        assert!(msg(FetchError::Malformed).contains("Copy image address"));
    }

    /// Fork F6: the bytes are what decide, and the served `Content-Type` is what explains.
    /// A server that served a page must be quoted back; anything else stays the honest
    /// "we cannot read this".
    #[test]
    fn an_undecodable_download_is_explained_by_what_the_server_said_it_sent() {
        let msg = not_an_image("text/html", en());
        assert!(msg.contains("text/html"), "{msg}");
        for ct in ["image/heic", ""] {
            assert_eq!(not_an_image(ct, en()), en().t("ui.err.image_undecodable"));
        }
    }

    /// Every new download message, in every bundled language, with nothing unsubstituted
    /// (i18n gate discipline, docs/lessons.md §7).
    #[test]
    fn download_errors_are_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let mut msgs: Vec<String> = [
                FetchError::Scheme,
                FetchError::Malformed,
                FetchError::TooManyRedirects,
                FetchError::Request("connection refused".into()),
                FetchError::Status(404),
                FetchError::TooBig,
                FetchError::Empty,
            ]
            .into_iter()
            .map(|e| localize_fetch(&e, ImageSettings::default().max_bytes, loc))
            .collect();
            msgs.push(not_an_image("text/html", loc));
            for msg in msgs {
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                if lang == crate::shared::i18n::Lang::En {
                    assert!(
                        !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en message: {msg}"
                    );
                }
            }
        }
    }

    /// The error paths a user actually hits must render in every bundled language with
    /// nothing unsubstituted (i18n gate discipline, docs/lessons.md §7).
    #[test]
    fn preparation_errors_are_localized_for_all_langs() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("not-an-image.txt");
        std::fs::write(&text, b"just words").unwrap();
        let missing = dir.path().join("nope.png");
        let tiny = ImageSettings {
            max_bytes: 4,
            ..ImageSettings::default()
        };

        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let mut msgs = vec![
                prepare_image(&text, &ImageSettings::default(), loc).unwrap_err(),
                prepare_image(&missing, &ImageSettings::default(), loc).unwrap_err(),
                prepare_image(dir.path(), &ImageSettings::default(), loc).unwrap_err(),
                prepare_image(&text, &tiny, loc).unwrap_err(),
            ];
            // The two capability refusals are built by the attach path, not here, but
            // they are the ones that must close the door — check them too.
            msgs.push(loc.t("ui.err.image_no_vision_managed").to_string());
            msgs.push(loc.t("ui.err.image_no_vision").to_string());
            msgs.push(loc.tf("ui.err.image_too_many", &[("max", "8")]));
            for msg in msgs {
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                if lang == crate::shared::i18n::Lang::En {
                    assert!(
                        !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en message: {msg}"
                    );
                }
            }
        }
    }
}
