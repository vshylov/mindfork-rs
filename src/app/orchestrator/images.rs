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
use crate::entities::message_image::{MessageImage, infos, resolve_target};
use crate::features::image_command::ImageProgress;
use crate::features::image_prepare::{PrepareError, prepare};
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

impl Orchestrator {
    /// Starts staging an image for the next message (`/image attach <path>`).
    pub(super) fn handle_image_attach(&mut self, path: String) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
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
            let outcome = tokio::task::spawn_blocking(move || {
                prepare_image(std::path::Path::new(&path), &cfg, loc)
            })
            .await
            .unwrap_or_else(|e| Err(loc.tf("ui.err.image_failed", &[("err", &e.to_string())])));
            let _ = tx.send(ImageAttachResult {
                chat_id,
                vision,
                outcome,
            });
        });
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

    /// Unstages an image by name/path/`#N` (`/image remove <target>`).
    pub(super) fn handle_image_remove(&mut self, target: String) {
        let Some(chat_id) = self.active_id else {
            self.fail_image(self.ui_locale().t("ui.err.image_no_active_chat"));
            return;
        };
        let staged = self.staged_images.entry(chat_id).or_default();
        let Some(idx) = resolve_target(staged, target.trim().trim_matches(['"', '\''])) else {
            // The message says what `remove` can and cannot reach: an image already sent
            // is part of the conversation, and pretending otherwise sends the user
            // hunting for a command that does not exist.
            let msg = self
                .ui_locale()
                .tf("ui.err.image_not_staged", &[("target", target.trim())]);
            self.fail_image(&msg);
            return;
        };
        let removed = staged.remove(idx);
        self.emit_image_progress(ImageProgress::Removed { name: removed.name });
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
    let data = base64::engine::general_purpose::STANDARD.encode(&prepared.bytes);
    Ok(MessageImage::new(
        name,
        crate::features::rag_ingest::canonical_source(path),
        prepared.mime,
        prepared.width,
        prepared.height,
        data,
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
