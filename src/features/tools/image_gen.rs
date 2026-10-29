//! `image_generate` — a picture drawn by the image slot's provider (spec §9.15,
//! docs/history/image-generation.md §6.3–§6.4).
//!
//! The tool calls the configured provider out of band, as `youtube_watch` calls
//! its video model: the chat engine can be anything, a local model included. The
//! original is kept **as the provider sent it** in the chat's folder, listed as a
//! [`FileOrigin::Generated`] file (F5), and a copy goes back with the result for a
//! model that sees (F6) — the loop prepares it on the path every tool's picture
//! takes and says whether it was shown.
//!
//! Three things bound what a turn spends (§6.6, F7, F10), all of them the turn's
//! and kept in [`TurnDraws`], which a sub-agent shares with the turn that called
//! it: the switch that offers the tool at all, the ceiling of pictures per turn,
//! and the refusal — after one, the tool draws nothing more in that turn, so the
//! person chooses the softened version, not the model (§11.2 saw both gate models
//! redraw a refused request up to seven times, unasked).
//!
//! A refusal is a result, not an error, said in the directive form the
//! withheld-image notes were measured in (spec §9.10). The result's text is the
//! agent scaffold's (axis A).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use anyhow::Result;
use base64::Engine as _;

use super::{ChatEffect, Tool, ToolContext, ToolImage, ToolOutcome};
use crate::entities::attachment::{Resolved, format_bytes};
use crate::entities::chat_file::{ChatFile, FileOrigin, same_name, sniff_image};
use crate::entities::profile::ToolId;
use crate::features::chat_files::{self, Stored};
use crate::features::chat_inputs::{self, ChatInput};
use crate::shared::config::ImageSettings;
use crate::shared::i18n::Locale;
use crate::shared::image_gen::{
    Aspect, Drawing, ImageGenSlot, ImageGenerator, ImageRequest, InputImage, Missing, Picture,
    Spent,
};

/// The tool's name.
pub const IMAGE_GENERATE_ID: &str = "image_generate";

/// Most pictures one call hands the provider to change — a bound on the request,
/// which carries each of them whole. Every provider takes more (OpenAI 16, Gemini
/// 14, the gateway's models up to 16).
pub const MAX_INPUTS: usize = 8;

/// What this turn has drawn, shared by every call of it — a sub-agent's context
/// is a clone of the turn's, so its calls count against the same ceiling. A
/// fresh one comes with every [`ToolContext::new`], which is once a turn.
#[derive(Debug, Default)]
pub struct TurnDraws {
    drawn: AtomicU32,
    refused: AtomicBool,
}

impl TurnDraws {
    /// Pictures this turn has drawn.
    pub fn drawn(&self) -> u32 {
        self.drawn.load(Ordering::SeqCst)
    }

    /// Whether the provider refused a picture in this turn (F10).
    pub fn refused(&self) -> bool {
        self.refused.load(Ordering::SeqCst)
    }
}

/// `image_generate`.
pub struct ImageGenerate {
    slot: Option<ImageGenSlot>,
    client: Option<Arc<dyn ImageGenerator>>,
    /// The ceiling a picture handed back to the loop must be under: a tool image
    /// over `images.max_bytes` is dropped **before** it is downscaled (§6.4), so a
    /// larger original goes as a prepared copy.
    images: ImageSettings,
}

impl ImageGenerate {
    /// `slot` — the settings as resolved (`None` only for the catalogue's own
    /// registry, which never runs a tool); `client` — built from the slot's
    /// configuration when it has one.
    pub fn new(
        slot: Option<ImageGenSlot>,
        client: Option<Arc<dyn ImageGenerator>>,
        images: ImageSettings,
    ) -> Self {
        Self {
            slot,
            client,
            images,
        }
    }

    /// The pictures `images` names, as the chat lists them.
    fn inputs(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<Vec<InputImage>> {
        let loc = ctx.loc;
        let handles: Vec<String> = match args.get("images") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::String(one)) => vec![one.clone()],
            Some(serde_json::Value::Array(many)) => many
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Some(_) => anyhow::bail!(loc.t("tool.image_generate.err.images_shape").to_string()),
        };
        let handles: Vec<String> = handles
            .into_iter()
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .collect();
        if handles.len() > MAX_INPUTS {
            anyhow::bail!(loc.tf(
                "tool.image_generate.err.too_many_images",
                &[("max", &MAX_INPUTS.to_string())],
            ));
        }
        if handles.is_empty() {
            return Ok(Vec::new());
        }
        let dir = ctx.files_dir.clone().unwrap_or_default();
        // The list the pinned block numbered, when this turn has one; otherwise
        // the one `/file list` shows — the same derivation, fresh.
        let items: Vec<ChatInput> = if ctx.inputs.is_empty() {
            chat_inputs::items(
                &ctx.attachments,
                &ctx.files,
                &ctx.images.iter().collect::<Vec<_>>(),
                &dir,
            )
        } else {
            ctx.inputs.to_vec()
        };
        handles
            .iter()
            .map(|handle| self.input(ctx, &items, &dir, handle))
            .collect()
    }

    /// One picture by its handle: a stored file read from the chat's folder, or
    /// one of the chat's images.
    fn input(
        &self,
        ctx: &ToolContext,
        items: &[ChatInput],
        dir: &std::path::Path,
        handle: &str,
    ) -> Result<InputImage> {
        let loc = ctx.loc;
        let item = match chat_inputs::resolve(items, handle) {
            Resolved::One(i) => &items[i],
            Resolved::Shared(_) => {
                anyhow::bail!(loc.tf("tool.image_generate.err.image_shared", &[("name", handle)],))
            }
            // `tool-image-2` for `tool-image-2.png`: the probe's models named the
            // chat's images without their extension.
            Resolved::Nothing => items
                .iter()
                .find(|i| i.is_image() && same_name(stem(&i.name), handle))
                .ok_or_else(|| {
                    anyhow::anyhow!(loc.tf(
                        "tool.image_generate.err.image_not_found",
                        &[("name", handle)],
                    ))
                })?,
        };
        let not_a_picture = || {
            anyhow::anyhow!(loc.tf(
                "tool.image_generate.err.not_a_picture",
                &[("name", &item.name)],
            ))
        };
        if let Some(image) = item.image.and_then(|i| ctx.images.get(i)) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(image.data.as_bytes())
                .map_err(|_| not_a_picture())?;
            return Ok(InputImage {
                mime: image.mime.clone(),
                bytes,
            });
        }
        let Some(name) = &item.file else {
            return Err(not_a_picture());
        };
        let path = chat_files::confined(dir, name).map_err(|_| not_a_picture())?;
        let bytes = std::fs::read(&path).map_err(|e| {
            anyhow::anyhow!(loc.tf(
                "tool.image_generate.err.image_unreadable",
                &[("name", name), ("err", &e.to_string())],
            ))
        })?;
        let Some(mime) = sniff_image(&bytes) else {
            return Err(not_a_picture());
        };
        // A large original goes as the copy the chat would show — the request
        // carries it whole, and a provider bills its pixels.
        if bytes.len() as u64 > self.images.max_bytes {
            let prepared =
                crate::features::image_prepare::prepare(&bytes, self.images.downscale_px)
                    .map_err(|_| not_a_picture())?;
            return Ok(InputImage {
                mime: prepared.mime.to_string(),
                bytes: prepared.bytes,
            });
        }
        Ok(InputImage {
            mime: mime.to_string(),
            bytes,
        })
    }

    /// Keeps a drawn picture and says what became of it.
    fn keep(&self, ctx: &ToolContext, slot: &ImageGenSlot, picture: Picture) -> ToolOutcome {
        let loc = ctx.loc;
        let (width, height) = dimensions(&picture.bytes);
        let size = format_bytes(picture.bytes.len());
        let mut effects = Vec::new();
        let entry = match ctx
            .files_dir
            .as_deref()
            .map(|dir| store(ctx, dir, &picture))
        {
            Some(Ok((file, handle))) => {
                let entry = loc.tf(
                    "tool.image_generate.result.saved",
                    &[
                        ("n", &handle.to_string()),
                        ("name", &file.name),
                        ("w", &width.to_string()),
                        ("h", &height.to_string()),
                        ("size", &size),
                    ],
                );
                effects.push(ChatEffect::AddChatFile(Box::new(file)));
                entry
            }
            Some(Err(err)) => loc.tf(
                "tool.image_generate.result.not_kept",
                &[
                    ("w", &width.to_string()),
                    ("h", &height.to_string()),
                    ("err", &err.to_string()),
                ],
            ),
            None => loc.tf(
                "tool.image_generate.result.no_folder",
                &[("w", &width.to_string()), ("h", &height.to_string())],
            ),
        };
        let mut lines = vec![entry.clone()];
        if let Some(revised) = &picture.revised_prompt {
            lines.push(loc.tf("tool.image_generate.result.revised", &[("prompt", revised)]));
        }
        if let Some(spent) = spent_line(loc, &picture.spent) {
            lines.push(spent);
        }
        if !slot.show_to_model {
            lines.push(
                loc.t("tool.image_generate.result.not_shown_off")
                    .to_string(),
            );
            return ToolOutcome::with_effects(lines.join("\n"), effects);
        }
        let shown = self.hand_over(&picture);
        let outcome = ToolOutcome::with_effects(lines.join("\n"), effects);
        match shown {
            Some((mime, data)) => outcome.with_images(vec![ToolImage {
                mime,
                data,
                entry: Some(entry),
            }]),
            None => outcome,
        }
    }

    /// The copy the loop is handed: the original when it is under the loop's
    /// ceiling, a prepared copy otherwise (§6.4). `None` — the picture cannot
    /// be decoded at all, and the loop's own note says it was not shown.
    fn hand_over(&self, picture: &Picture) -> Option<(String, String)> {
        let b64 = base64::engine::general_purpose::STANDARD;
        if picture.bytes.len() as u64 <= self.images.max_bytes {
            return Some((picture.mime.to_string(), b64.encode(&picture.bytes)));
        }
        let prepared =
            crate::features::image_prepare::prepare(&picture.bytes, self.images.downscale_px)
                .ok()?;
        Some((prepared.mime.to_string(), b64.encode(&prepared.bytes)))
    }
}

/// A name without its extension.
fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// The picture's pixel size, read off its header; `0×0` for one that will not
/// say.
fn dimensions(bytes: &[u8]) -> (u32, u32) {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .unwrap_or((0, 0))
}

/// The extension a picture of `mime` is stored with.
fn extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

/// Stores a drawn picture in the chat's folder `dir` as `image-N.<ext>`, the
/// first `N` no file of the chat has taken under any extension — a slug of the
/// prompt would be a transliteration problem in half the prompts this audience
/// writes (§6.4). `listed` is what the chat lists. Returns the listing: the
/// tool's road and `/image generate`'s are one.
pub fn store_drawn(
    dir: &std::path::Path,
    listed: &[ChatFile],
    bytes: &[u8],
    mime: &str,
) -> std::io::Result<ChatFile> {
    let ext = extension(mime);
    // What the folder holds, listed or not: a file a call wrote whose chat was
    // not saved still owns its name (sandbox-file-exchange §11 S6).
    let on_disk: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    let name = (1u32..)
        .map(|n| format!("image-{n}.{ext}"))
        .find(|candidate| {
            let taken = |name: &str| same_name(stem(name), stem(candidate));
            !listed.iter().any(|f| taken(&f.name)) && !on_disk.iter().any(|n| taken(n))
        })
        .expect("an unbounded range has a free name");
    let stored = chat_files::store_as(
        dir,
        listed,
        &name,
        bytes,
        FileOrigin::Generated,
        // From the internet, like a call's output: Office must not open it as a
        // local, trusted file (sandbox-file-exchange §13 U11).
        Some(crate::shared::os_open::FROM_ELSEWHERE),
    )?;
    Ok(match stored {
        Stored::New(file) | Stored::Unchanged(file) | Stored::Restored(file) => file,
    })
}

/// Stores the original ([`store_drawn`]) and says the `#N` `/file list` will
/// show it under.
fn store(
    ctx: &ToolContext,
    dir: &std::path::Path,
    picture: &Picture,
) -> std::io::Result<(ChatFile, usize)> {
    let listed = ctx.files.to_vec();
    let file = store_drawn(dir, &listed, &picture.bytes, picture.mime)?;
    let mut files = listed;
    if !files.iter().any(|f| f.id == file.id) {
        files.push(file.clone());
    }
    let handle = chat_inputs::items(&ctx.attachments, &files, &[], dir)
        .iter()
        .find(|i| i.file.as_deref() == Some(file.name.as_str()))
        .map_or(files.len(), |i| i.handle);
    Ok((file, handle))
}

/// What the provider said the call cost, as one line — `None` when it said
/// nothing. The app keeps no price list of its own (§6.6).
fn spent_line(loc: &Locale, spent: &Spent) -> Option<String> {
    if let Some(cost) = spent.cost_usd {
        return Some(loc.tf(
            "tool.image_generate.result.spent_cost",
            &[("cost", &format!("{cost:.3}"))],
        ));
    }
    match (spent.input_tokens, spent.output_tokens) {
        (None, None) => None,
        (input, output) => Some(loc.tf(
            "tool.image_generate.result.spent_tokens",
            &[
                ("input", &input.unwrap_or(0).to_string()),
                ("output", &output.unwrap_or(0).to_string()),
            ],
        )),
    }
}

#[async_trait::async_trait]
impl Tool for ImageGenerate {
    fn id(&self) -> ToolId {
        IMAGE_GENERATE_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "generate images"
    }
    /// Offered only while `image_gen.enabled` is on (F7): nothing is drawn, or
    /// paid for, until the person says so.
    fn gate(&self) -> Option<super::meta::ToolGate> {
        Some(super::meta::ToolGate::ImageGen)
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.image_generate.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        let aspects: Vec<&str> = Aspect::ALL.iter().map(|a| a.ratio()).collect();
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": loc.t("tool.image_generate.param.prompt")},
                "aspect": {
                    "type": "string",
                    "enum": aspects,
                    "description": loc.t("tool.image_generate.param.aspect"),
                },
                "images": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": loc.t("tool.image_generate.param.images"),
                }
            },
            "required": ["prompt"]
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let loc = ctx.loc;
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!(loc.t("tool.image_generate.err.prompt_empty")))?;
        let aspect = match args.get("aspect").and_then(|v| v.as_str()) {
            None => Aspect::default(),
            Some(said) => Aspect::parse(said).ok_or_else(|| {
                anyhow::anyhow!(loc.tf("tool.image_generate.err.bad_aspect", &[("aspect", said)]))
            })?,
        };
        let Some(slot) = &self.slot else {
            return Ok(ToolOutcome::text(
                loc.t("tool.image_generate.result.off").to_string(),
            ));
        };
        // F10: after a refusal nothing more reaches the provider in this turn.
        if ctx.draws.refused() {
            return Ok(ToolOutcome::text(
                loc.t("tool.image_generate.result.stopped").to_string(),
            ));
        }
        if slot.max_per_turn > 0 && ctx.draws.drawn() >= slot.max_per_turn {
            return Ok(ToolOutcome::text(loc.tf(
                "tool.image_generate.result.cap",
                &[("max", &slot.max_per_turn.to_string())],
            )));
        }
        let provider = slot.provider.cloud_provider().display_name();
        let (Ok(_), Some(client)) = (&slot.config, &self.client) else {
            let key = match slot.config {
                Err(Missing::Model) => "tool.image_generate.result.no_model",
                _ => "tool.image_generate.result.no_key",
            };
            return Ok(ToolOutcome::text(loc.tf(key, &[("provider", provider)])));
        };
        let inputs = self.inputs(ctx, &args)?;
        let request = ImageRequest {
            prompt: prompt.to_string(),
            aspect,
            inputs,
        };
        match client.draw(request, &ctx.cancel).await {
            Ok(Drawing::Picture(picture)) => {
                ctx.draws.drawn.fetch_add(1, Ordering::SeqCst);
                Ok(self.keep(ctx, slot, picture))
            }
            Ok(Drawing::Refused { reason, spent }) => {
                ctx.draws.refused.store(true, Ordering::SeqCst);
                let mut out = loc.tf(
                    "tool.image_generate.result.refused",
                    &[("reason", reason.trim())],
                );
                if let Some(line) = spent_line(loc, &spent) {
                    out.push('\n');
                    out.push_str(&line);
                }
                Ok(ToolOutcome::text(out))
            }
            Err(err) => Ok(ToolOutcome::text(loc.tf(
                "tool.image_generate.result.failed",
                &[("err", &format!("{err:#}"))],
            ))),
        }
    }
}

#[cfg(test)]
#[path = "image_gen_tests.rs"]
mod tests;
