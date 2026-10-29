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

use super::ChatScreen;
use super::attachments::format_tokens;
use crate::entities::attachment::format_bytes;
use crate::entities::message_image::ImageInfo;
use crate::features::image_command::ImageProgress;
use crate::shared::i18n::Locale;

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
            ImageProgress::Removed { name, source } => {
                let msg = match source {
                    Some(source) => self.loc.tf(
                        "ui.image.removed_source",
                        &[("name", &name), ("source", &source)],
                    ),
                    None => self.loc.tf("ui.image.removed", &[("name", &name)]),
                };
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
            ImageProgress::Drawing { provider } => {
                self.push_note(&self.loc.tf("ui.image.drawing", &[("provider", &provider)]));
            }
            ImageProgress::Drawn {
                provider,
                drawn,
                info,
                staged,
            } => {
                let msg = self.loc.tf(
                    "ui.image.drawn",
                    &[
                        ("provider", &provider),
                        ("name", &drawn.name),
                        ("n", &drawn.handle.to_string()),
                        ("picture", &drawn_facts(&drawn, self.loc)),
                        ("tokens", &format_tokens(info.est_tokens)),
                        ("staged", &staged.to_string()),
                    ],
                );
                self.push_note(&msg);
            }
            ImageProgress::Kept {
                provider,
                drawn,
                why,
            } => {
                let msg = self.loc.tf(
                    "ui.image.kept",
                    &[
                        ("provider", &provider),
                        ("name", &drawn.name),
                        ("n", &drawn.handle.to_string()),
                        ("picture", &drawn_facts(&drawn, self.loc)),
                        ("why", &why),
                    ],
                );
                self.push_note(&msg);
            }
            ImageProgress::Refused {
                provider,
                reason,
                spent,
            } => {
                let charged = spent
                    .cost_usd
                    .map(|cost| {
                        self.loc.tf(
                            "ui.image.refused_charged",
                            &[("cost", &format!("{cost:.3}"))],
                        )
                    })
                    .unwrap_or_default();
                let msg = self.loc.tf(
                    "ui.image.refused",
                    &[
                        ("provider", &provider),
                        ("reason", reason.trim()),
                        ("charged", &charged),
                    ],
                );
                self.push_error(&msg);
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

/// What a note says about a drawn original: its size, its weight, and what the
/// provider said it cost — the price where it states one, the tokens where it
/// counts them, nothing where it says nothing (the app keeps no price list).
fn drawn_facts(drawn: &crate::features::image_command::DrawnFile, loc: &Locale) -> String {
    let mut facts = format!(
        "{}×{}, {}",
        drawn.width,
        drawn.height,
        format_bytes(drawn.bytes)
    );
    let spent = &drawn.spent;
    let said = match (spent.cost_usd, spent.input_tokens, spent.output_tokens) {
        (Some(cost), _, _) => {
            Some(loc.tf("ui.image.spent_cost", &[("cost", &format!("{cost:.3}"))]))
        }
        (None, None, None) => None,
        (None, input, output) => Some(loc.tf(
            "ui.image.spent_tokens",
            &[
                ("input", &input.unwrap_or(0).to_string()),
                ("output", &output.unwrap_or(0).to_string()),
            ],
        )),
    };
    if let Some(said) = said {
        facts.push_str(", ");
        facts.push_str(&said);
    }
    facts
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
        let n = (i + 1).to_string();
        let size = format_bytes(image.bytes);
        let width = image.width.to_string();
        let height = image.height.to_string();
        let tokens = format_tokens(image.est_tokens);
        let mut args = vec![
            ("i", n.as_str()),
            ("name", image.name.as_str()),
            ("size", size.as_str()),
            ("width", width.as_str()),
            ("height", height.as_str()),
            ("tokens", tokens.as_str()),
        ];
        // As in `/file list`: a shared name is told apart by the source `remove` accepts.
        let key =
            if crate::entities::attachment::name_is_shared(items, i, |other| other.name.as_str()) {
                args.push(("source", image.source.as_str()));
                "ui.image.list_item_source"
            } else {
                "ui.image.list_item"
            };
        out.push_str(&loc.tf(key, &args));
    }
    out
}
