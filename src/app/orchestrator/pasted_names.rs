//! A pasted image's name (spec §9.10, docs/research/pasted-image-names.md, stage 2):
//! the chat's own engine is asked what the picture shows, the image is renamed
//! `clipboard/<slug>.png` when the answer lands, and a turn that carries an image whose
//! name is still on its way is **parked** until it lands — at most
//! [`image_name::SEND_WAIT`] (fork F4).
//!
//! The park sits before the turn is built, not inside it: the request, the label the
//! model reads before the image and the chat's list of files in the system prompt are
//! all rendered from the chat at the start, so building the turn once the names are in
//! keeps every one of them on the same name. The turn is reserved at once — its id
//! begun, `GenerationStarted` sent — so the screen shows it starting, `Esc` stops it,
//! and nothing else starts beside it.
//!
//! Whether a name is still taken is one board's to say ([`NameBoard`]): a name lands
//! only on an image still waiting for it, and a turn that stops waiting settles the
//! rest. Every failure is quiet (F9) — the image keeps the name it was staged under.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message_image::MessageImage;
use crate::features::image_command::ImageProgress;
use crate::features::image_name::{self, Letters};
use crate::shared::api::{ChatChunk, ChatRequest, EngineBackend, FinishReason};
use crate::shared::session_budget::{SessionBudget, Shape};

use super::Orchestrator;

/// What the naming machinery tells the loop (an internal channel).
pub(super) enum NameEvent {
    /// A naming request answered: the slug its answer came to, `None` for every way of
    /// not bringing one.
    Answered {
        chat_id: Uuid,
        image_id: Uuid,
        slug: Option<String>,
    },
    /// A parked turn has waited its [`image_name::SEND_WAIT`] — its generation id.
    Deadline(Uuid),
}

/// A turn reserved but not yet built, waiting for the names of the images it carries.
pub(super) struct ParkedTurn {
    chat_id: Uuid,
    backend: Arc<dyn EngineBackend>,
    id: Uuid,
    cancel: CancellationToken,
    images: Vec<Uuid>,
}

impl Orchestrator {
    /// Asks the chat's engine for a just-staged pasted image's name. Quiet when the
    /// engine is not ready: the image keeps its name, as when the request fails.
    pub(super) fn start_naming(&mut self, chat_id: Uuid, image: &MessageImage) {
        let backend = match self.engines.backend_if_ready(self.ui_locale()) {
            Ok(backend) => backend,
            Err(why) => {
                tracing::info!(%chat_id, why, "a pasted image is not named: no engine is ready");
                return;
            }
        };
        let image_id = image.id;
        self.name_board.expect(image_id);
        let request = image_name::request(image);
        let floor = image.est_tokens() as u64;
        let sessions = self.session_budget();
        let tx = self.name_tx.clone();
        tracing::info!(%chat_id, %image_id, "asking for a pasted image's name");
        tokio::spawn(async move {
            let slug = ask_name(backend, request, floor, &sessions).await;
            let _ = tx.send(NameEvent::Answered {
                chat_id,
                image_id,
                slug,
            });
        });
    }

    /// Applies what the naming machinery reports: a name landed — made unique in its
    /// chat and given to the image if it is still waiting for one — or a parked turn's
    /// wait ran out. Either may release the parked turn.
    pub(super) fn handle_name_event(&mut self, event: NameEvent) {
        match event {
            NameEvent::Answered {
                chat_id,
                image_id,
                slug,
            } => {
                match slug {
                    Some(slug) => {
                        let wanted = image_name::pasted_name(&slug);
                        let name = self.free_image_name(chat_id, &wanted, Some(image_id));
                        if self.name_board.land(image_id, &name) {
                            self.rename_image(chat_id, image_id, name);
                        } else {
                            tracing::info!(%image_id, name, "a pasted image's name came too late");
                        }
                    }
                    None => self.name_board.settle(image_id),
                }
                self.name_board.forget(image_id);
                self.maybe_unpark();
            }
            NameEvent::Deadline(generation) => {
                if self.parked.as_ref().is_some_and(|p| p.id == generation) {
                    tracing::info!(
                        %generation,
                        "a turn stopped waiting for its pasted images' names"
                    );
                    self.unpark();
                }
            }
        }
    }

    /// The first name based on `wanted` that nothing else in the chat is called — staged
    /// images, sent images, stored files, attachments (F6). `except` is the image being
    /// named, whose current name is about to go.
    pub(super) fn free_image_name(
        &self,
        chat_id: Uuid,
        wanted: &str,
        except: Option<Uuid>,
    ) -> String {
        let same = crate::entities::chat_file::same_name;
        let staged = self.staged_images.get(&chat_id);
        let chat = self.chats.iter().find(|c| c.id == chat_id);
        let taken = |name: &str| {
            let image_holds = |i: &MessageImage| Some(i.id) != except && same(&i.name, name);
            staged.is_some_and(|v| v.iter().any(image_holds))
                || chat.is_some_and(|c| {
                    c.messages
                        .iter()
                        .flat_map(|m| m.images.iter())
                        .any(image_holds)
                        || c.files.iter().any(|f| same(&f.name, name))
                        || c.attachments.iter().any(|a| same(&a.name, name))
                })
        };
        image_name::unique_name(wanted, taken)
    }

    /// Gives image `image_id` of chat `chat_id` its name, wherever it is: still staged,
    /// or on the message a parked turn has not built yet. Said in the feed when the chat
    /// is open.
    fn rename_image(&mut self, chat_id: Uuid, image_id: Uuid, name: String) {
        let staged = self
            .staged_images
            .get_mut(&chat_id)
            .and_then(|v| v.iter_mut().find(|i| i.id == image_id));
        let from = if let Some(image) = staged {
            let from = std::mem::replace(&mut image.name, name.clone());
            self.emit_staged_images();
            from
        } else {
            let Some(chat) = self.chat_mut(chat_id) else {
                return;
            };
            let Some(image) = chat
                .messages
                .iter_mut()
                .rev()
                .flat_map(|m| m.images.iter_mut())
                .find(|i| i.id == image_id)
            else {
                return;
            };
            let from = std::mem::replace(&mut image.name, name.clone());
            self.mark_dirty(chat_id);
            from
        };
        if self.active_id == Some(chat_id) {
            let _ = self
                .evt_tx
                .send(AppEvent::ImageProgress(ImageProgress::Named {
                    from,
                    to: name,
                }));
        }
    }

    /// Reserves the turn of chat `chat_id`'s new message and parks it until the names of
    /// `images` are in, or [`image_name::SEND_WAIT`] has passed (F4).
    pub(super) fn park_turn(
        &mut self,
        chat_id: Uuid,
        backend: Arc<dyn EngineBackend>,
        images: Vec<Uuid>,
    ) {
        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let _ = self.evt_tx.send(AppEvent::GenerationStarted {
            generation_id: id,
            model: self.effective_model_name(),
            continuation: false,
        });
        self.gen_state.begin(id, cancel.clone());
        let tx = self.name_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(image_name::SEND_WAIT).await;
            let _ = tx.send(NameEvent::Deadline(id));
        });
        tracing::info!(%chat_id, waiting = images.len(), "a turn waits for its pasted images' names");
        self.parked = Some(ParkedTurn {
            chat_id,
            backend,
            id,
            cancel,
            images,
        });
    }

    /// Releases the parked turn once none of its images waits for a name any more.
    fn maybe_unpark(&mut self) {
        if self
            .parked
            .as_ref()
            .is_some_and(|p| self.name_board.pending(&p.images).is_empty())
        {
            self.unpark();
        }
    }

    /// Releases the parked turn when it was stopped while it waited — `Esc`, or a switch
    /// to another chat. Called after every place a turn is cancelled.
    pub(super) fn unpark_if_cancelled(&mut self) {
        if self
            .parked
            .as_ref()
            .is_some_and(|p| p.cancel.is_cancelled())
        {
            self.unpark();
        }
    }

    /// Ends the park: names still on their way are given up — the turn goes under the
    /// names its images have, and a late one is dropped — then the turn is built and
    /// started under the id it was reserved with, or ended there if it was stopped.
    fn unpark(&mut self) {
        let Some(parked) = self.parked.take() else {
            return;
        };
        self.name_board.give_up(&parked.images);
        let ParkedTurn {
            chat_id,
            backend,
            id,
            cancel,
            ..
        } = parked;
        if cancel.is_cancelled() || self.gen_state.current_id() != Some(id) {
            self.end_parked(id);
            return;
        }
        if self
            .start_reserved_generation(chat_id, backend, (id, cancel))
            .is_none()
        {
            self.end_parked(id);
            return;
        }
        self.title_after_user_message(chat_id);
    }

    /// A reserved turn that never made a request: the screen is told it ended, and the
    /// state machine is idle again.
    fn end_parked(&mut self, id: Uuid) {
        let _ = self.evt_tx.send(AppEvent::Finished {
            generation_id: id,
            reason: FinishReason::Cancelled,
            continuable: false,
        });
        self.gen_state.finish(id);
    }
}

/// One naming request, in the silent lane (docs/research/silent-tasks-budget.md): the
/// slug its answer comes to, or `None` for a lane that never opened, a request that
/// failed, ran past [`image_name::REQUEST_TIMEOUT`] or answered nothing a name can be
/// made of. One attempt, and it yields to a turn that needs its room: a turn reaches for
/// room only once it has stopped waiting for the names (the park sits before the turn is
/// built), and a name it no longer waits for would be dropped anyway.
async fn ask_name(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    image_tokens: u64,
    sessions: &SessionBudget,
) -> Option<String> {
    let cancel = CancellationToken::new();
    let estimate = super::generation::estimate_prompt_tokens(&request);
    // The text estimate cannot see the image, so its tokens are the floor.
    let need = sessions.price(
        Shape::Name,
        estimate,
        estimate + image_tokens,
        request.sampling.max_tokens.map(|m| m as u64),
    );
    let lane = sessions
        .acquire_silent(need, &cancel, "image name", true)
        .await?;
    let token = lane.stream_token();
    let collect = async {
        let mut stream = match backend.chat_stream(request, token).await {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!(error = %err, "a pasted image's name was not asked");
                return None;
            }
        };
        let (mut text, mut thoughts) = (String::new(), String::new());
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::Finished(FinishReason::Cancelled) => return None,
                ChatChunk::Finished(_) => break,
                ChatChunk::Error { message, .. } => {
                    tracing::warn!(error = %message, "engine error while naming a pasted image");
                }
                ChatChunk::Usage(u) => u.log_round("image name"),
                _ => {}
            }
        }
        Some(image_name::answer(&text, &thoughts))
    };
    let answer = match tokio::time::timeout(image_name::REQUEST_TIMEOUT, collect).await {
        Ok(answer) => answer?,
        Err(_) => {
            cancel.cancel();
            tracing::warn!("naming a pasted image timed out");
            return None;
        }
    };
    drop(lane);
    let slug = image_name::slug(&answer, Letters::Ascii);
    if slug.is_none() {
        tracing::info!(answer, "a pasted image's naming answer held no name");
    }
    slug
}
