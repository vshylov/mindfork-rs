//! The chat screen — images staged for the next message (`/image attach`): the
//! feed note for a command's outcome and the status-bar chip. Part of the
//! [`super`] module. See spec §9.10.
//!
//! The shape mirrors [`super::attachments`], but the *thing* being shown is
//! different and the wording has to say so: a file attachment is pinned to the
//! chat and travels with every turn, while an image is staged for the **next
//! message** and then becomes ordinary history. So the chip counts what is still
//! waiting to be sent, and `/image list` numbers exactly the set `/image remove`
//! can still reach — once a message goes out, its images are no longer here.

use super::attachments::format_tokens;
use super::*;
use crate::entities::attachment::format_bytes;
use crate::features::image_command::ImageProgress;

impl ChatScreen {
    /// Updates the cards for the images staged for the next message
    /// (`AppEvent::StagedImages`) — the source for the status-bar chip.
    pub fn set_staged_images(&mut self, items: Vec<ImageInfo>) {
        self.staged_images = items;
    }

    /// Reports the outcome of an `/image` command as a note in the feed.
    pub fn set_image_progress(&mut self, progress: ImageProgress) {
        match progress {
            ImageProgress::Attached { info, staged } => {
                let msg = self.loc.tf(
                    "ui.image.attached",
                    &[
                        ("name", &info.name),
                        ("size", &format_bytes(info.bytes)),
                        ("width", &info.width.to_string()),
                        ("height", &info.height.to_string()),
                        ("tokens", &format_tokens(info.est_tokens)),
                        ("n", &staged.to_string()),
                    ],
                );
                self.push_note(&msg);
            }
            ImageProgress::Removed { name } => {
                let msg = self.loc.tf("ui.image.removed", &[("name", &name)]);
                self.push_note(&msg);
            }
            ImageProgress::Listed { items } => {
                let text = format_staged_images(&items, self.loc);
                self.push_note(&text);
            }
            ImageProgress::VisionUnknown => {
                // A note, not an error: the image *is* staged. The engine simply
                // could not say whether it takes images, and the send will.
                self.push_note(self.loc.t("ui.image.vision_unknown"));
            }
            ImageProgress::Failed(err) => {
                self.push_error(&self.loc.tf("ui.image.failed", &[("err", &err)]));
            }
        }
    }

    /// The status-bar chip label for staged images (`None` — nothing staged).
    /// Unlike the attachments chip this one is transient: it appears when an
    /// image is staged and goes away when the message carrying it is sent — so
    /// what it shows is the cost the *next* turn is about to pay.
    pub(super) fn staged_images_hint(&self) -> Option<String> {
        if self.staged_images.is_empty() {
            return None;
        }
        let cost: usize = self.staged_images.iter().map(|i| i.est_tokens).sum();
        Some(self.loc.tf(
            "ui.image.chip",
            &[
                ("n", &self.staged_images.len().to_string()),
                ("tokens", &format_tokens(cost)),
            ],
        ))
    }
}

/// Formats the `/image list` reply: a header plus one line per staged image with
/// its `#N` handle (the same handle `/image remove #N` accepts). The empty case
/// is not "nothing here" but a statement of the model — images already sent are
/// part of the conversation and are not listed.
pub(super) fn format_staged_images(items: &[ImageInfo], loc: &'static Locale) -> String {
    if items.is_empty() {
        return loc.t("ui.image.list_empty").to_string();
    }
    let cost: usize = items.iter().map(|i| i.est_tokens).sum();
    let mut out = loc.tf(
        "ui.image.list_header",
        &[
            ("n", &items.len().to_string()),
            ("total", &format_tokens(cost)),
        ],
    );
    for (i, image) in items.iter().enumerate() {
        out.push_str(&loc.tf(
            "ui.image.list_item",
            &[
                ("i", &(i + 1).to_string()),
                ("name", &image.name),
                ("size", &format_bytes(image.bytes)),
                ("width", &image.width.to_string()),
                ("height", &image.height.to_string()),
                ("tokens", &format_tokens(image.est_tokens)),
            ],
        ));
    }
    out
}
