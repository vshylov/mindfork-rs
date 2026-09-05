//! Sub-agent runs out in the **background** (spec §9.3.2,
//! docs/research/background-subagents.md): a `start_subagent` call hands the
//! loop's [`generation::BackgroundStart`] to the orchestrator, which spawns
//! the run as a task of its own — outside the turn's token, under the app's
//! session budget — and keeps a seat for it here: the run's mirror (the same
//! [`InflightChild`] a turn's child gets, so the list, the read-only screen
//! and the stream forwarding need no second path), its cancellation token,
//! and its parent chat. The parent's record landed with the turn as a
//! placeholder; when the run ends, the orchestrator — sole owner of `Chat` —
//! re-finds that record **by id** (the auto-title's route, research §2.4)
//! and fills it in, then delivers the result as a **task notification**: a
//! stored row of the parent chat (`Message::notification`) that the model
//! reads as user text, and a turn the app starts itself when the chat is
//! open and idle (research §4.4). Nothing here asks the user anything: a
//! background run runs without confirmations (research fork F3, the user's
//! decision); the profile's tool set is the control.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageRole};
use crate::entities::subagent::{RunOutcome, SubagentRun};
use crate::features::tools::ChatEffect;

use super::generation::{self, BackgroundMessage, BackgroundSpawn, BackgroundStart, TurnProgress};
use super::{InflightChild, Orchestrator};

/// A run that ended while a turn was still running in its chat: the
/// record it lands on comes with that turn, so the landing waits for it
/// (research §5, the ordering); the seat stays meanwhile, ended, so the
/// list keeps the row.
pub(super) struct PendingLanding {
    generation: Uuid,
    run: SubagentRun,
    result: String,
    effects: Vec<ChatEffect>,
}

/// One run out in the background, as the orchestrator holds it.
pub(super) struct BackgroundRun {
    /// The run's own generation id — what its progress and its end are keyed
    /// by on the wire from the task (never the turn's, which may be over).
    pub(super) generation: Uuid,
    /// The run's id: the placeholder's, the mirror's, the landed record's.
    pub(super) run_id: Uuid,
    /// The chat whose exchange started it.
    pub(super) chat: Uuid,
    /// The run's own token: `/subagents stop`, the deletion of the spawning
    /// exchange, a profile switch and `Quit` cancel it; `Esc` does not.
    pub(super) cancel: CancellationToken,
    /// The mirror: the run as it will land, its round in progress, the stream
    /// id its open transcript accepts.
    pub(super) child: InflightChild,
}

impl Orchestrator {
    /// Spawns a background run the turn in flight handed over
    /// (research §4.2): a seat here, a fresh generation id, the app's budget,
    /// and the task. The parent's turn has already recorded the *started*
    /// result and goes on.
    pub(super) fn spawn_background_run(&mut self, start: Box<BackgroundStart>, chat: Uuid) {
        let generation = Uuid::new_v4();
        let run_id = start.run_id();
        let placeholder = start.placeholder();
        let sessions = self.session_budget();
        let cancel = generation::spawn_background_run(
            *start,
            BackgroundSpawn {
                generation,
                sessions,
                evt_tx: self.evt_tx.clone(),
                bg_tx: self.bg_run_tx.clone(),
            },
        );
        self.background_runs.push(BackgroundRun {
            generation,
            run_id,
            chat,
            cancel,
            child: InflightChild {
                run: placeholder,
                stream: Uuid::new_v4(),
                partial: Default::default(),
                line_role: MessageRole::Assistant,
            },
        });
        self.emit_chat_list();
        self.emit_background_runs();
    }

    /// The background runs' word to the orchestrator (see
    /// [`BackgroundMessage`]): a step of a run's stream or rounds goes to its
    /// mirror exactly as a turn child's does; its end lands it.
    pub(super) fn handle_background_message(&mut self, message: BackgroundMessage) {
        match message {
            BackgroundMessage::Progress {
                generation,
                progress,
            } => {
                let Some(seat) = self
                    .background_runs
                    .iter_mut()
                    .find(|b| b.generation == generation)
                else {
                    return;
                };
                match progress {
                    // The seat already holds the placeholder; the run's own
                    // start carries the same fields, timed by the task.
                    TurnProgress::ChildStarted(run) => {
                        let mut run = *run;
                        run.background = true;
                        run.title = seat.child.run.title.clone();
                        run.renamed_manually = seat.child.run.renamed_manually;
                        seat.child.run = run;
                        self.emit_chat_list();
                    }
                    other => self.handle_child_progress(other),
                }
            }
            BackgroundMessage::Done {
                generation,
                run,
                result,
                effects,
            } => self.land_background_run(generation, *run, result, effects),
        }
    }

    /// The run ended (research §4.3–§4.4). While a turn is still running in
    /// its chat the record it lands on is not in `Chat` yet — it comes with
    /// that turn — so the landing waits for `handle_done`
    /// ([`Self::land_pending_runs`]); otherwise it lands now.
    fn land_background_run(
        &mut self,
        generation: Uuid,
        run: SubagentRun,
        result: String,
        effects: Vec<ChatEffect>,
    ) {
        let Some(pos) = self
            .background_runs
            .iter()
            .position(|b| b.generation == generation)
        else {
            return;
        };
        let chat_id = self.background_runs[pos].chat;
        if self.inflight.as_ref().is_some_and(|t| t.chat == chat_id) {
            // The seat's row reads as ended meanwhile — and its transcript
            // shows the whole run, last reply included: the mirror is what
            // every read path sees until the record lands (research §5, the
            // list and the transcript honestly ahead of the feed).
            let mirror = &mut self.background_runs[pos].child.run;
            mirror.outcome = run.outcome;
            mirror.finished_at = run.finished_at;
            mirror.tokens = run.tokens;
            mirror.messages = run.messages.clone();
            self.pending_landings.push(PendingLanding {
                generation,
                run,
                result,
                effects,
            });
            self.emit_background_runs();
            self.emit_chat_list();
            return;
        }
        let seat = self.background_runs.remove(pos);
        self.finish_landing(seat, run, result, effects);
    }

    /// Lands the runs whose turn just landed in `chat_id` — the records are
    /// in `Chat` now. Called by `handle_done` after the turn's own rows.
    pub(super) fn land_pending_runs(&mut self, chat_id: Uuid) {
        let (pending, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_landings)
            .into_iter()
            .partition(|landing| {
                self.background_runs
                    .iter()
                    .any(|b| b.generation == landing.generation && b.chat == chat_id)
            });
        self.pending_landings = kept;
        for landing in pending {
            let Some(pos) = self
                .background_runs
                .iter()
                .position(|b| b.generation == landing.generation)
            else {
                continue;
            };
            let seat = self.background_runs.remove(pos);
            self.finish_landing(seat, landing.run, landing.result, landing.effects);
        }
    }

    /// The landing itself: the record re-found by id and filled in, the
    /// attachments to the parent, the result as a notification row with a
    /// wake (research §4.3–§4.4). A record gone with its chat lands nowhere;
    /// one that moved to the deleted archive with its exchange is filled in
    /// there and announces nothing.
    fn finish_landing(
        &mut self,
        seat: BackgroundRun,
        mut run: SubagentRun,
        result: String,
        effects: Vec<ChatEffect>,
    ) {
        run.background = true;
        if seat.child.run.renamed_manually {
            run.title = seat.child.run.title.clone();
            run.renamed_manually = true;
        }
        let chat_id = seat.chat;
        let run_id = run.id;
        let label = run.name.clone().unwrap_or_else(|| run.title.clone());
        let mut attached = Vec::new();
        let mut live = false;
        let mut landed = false;
        if let Some(chat) = self.chat_mut(chat_id) {
            live = chat.child(run_id).is_some();
            if let Some(record) = chat.child_mut_including_deleted(run_id) {
                *record = run;
                landed = true;
            }
            for effect in effects {
                if let ChatEffect::AddAttachment(a) = effect {
                    attached.push(*a);
                }
            }
        }
        self.emit_background_runs();
        if !landed {
            self.emit_chat_list();
            return;
        }
        self.mark_dirty(chat_id);
        for a in attached {
            self.insert_attachment(chat_id, a);
        }
        self.emit_chat_list();
        // Titled at landing like a turn child (spec §9.3.2): the run's
        // question and reply arrive together here too.
        self.maybe_auto_title_run(run_id);
        if !live {
            return;
        }
        // The notification, in the profile's language: the model reads it.
        let loc = self.profile_locale_of(chat_id);
        let text = loc.tf(
            "tool.start_subagent.notification",
            &[
                ("name", &label),
                ("address", &crate::features::chat_links::uri(run_id)),
                ("body", &result),
            ],
        );
        let message = Message::notification(run_id, text);
        self.deliver_notification(chat_id, message);
    }

    /// Appends the notification row to the chat — the feed rebuilt when it
    /// is the open one, so the note shows at once — and wakes the assistant
    /// (research §4.4). A chat the user is not looking at is marked
    /// **unread** instead (spec §11.2): the list says a result is waiting
    /// there, and opening the chat clears the mark
    /// ([`Orchestrator::activate_focused`]).
    fn deliver_notification(&mut self, chat_id: Uuid, message: Message) {
        let open = self.active_id == Some(chat_id);
        let Some(chat) = self.chat_mut(chat_id) else {
            return;
        };
        chat.push_message(message);
        if !open {
            chat.unread = true;
        }
        self.mark_dirty(chat_id);
        if open {
            self.activate(chat_id);
        }
        self.emit_chat_list();
        self.maybe_wake(chat_id);
    }

    /// Starts a turn with no new user message — the notification is the
    /// last user-side row — when the chat is the open one, nothing is
    /// generating, and `tools.subagent_background_wake` says so. A chat the
    /// user is not looking at keeps its note for their return: no reply
    /// into the void (research fork F2).
    fn maybe_wake(&mut self, chat_id: Uuid) {
        if !self.config.tools.subagent_background_wake
            || self.active_id != Some(chat_id)
            || !self.gen_state.is_idle()
        {
            return;
        }
        // Quietly: a server that is not ready keeps the note for the next
        // message, and an error bubble for a turn nobody asked for is noise.
        let Ok(backend) = self.engines.backend_if_ready(self.ui_locale()) else {
            return;
        };
        self.start_generation(chat_id, backend, None);
    }

    /// `/subagents stop` (spec §9.3.2): cancels the run's token; the run
    /// lands `cancelled` through the same route every end takes. An id that
    /// names no run out is ignored.
    pub(super) fn handle_stop_subagent_run(&mut self, id: Uuid) {
        if let Some(seat) = self.background_runs.iter().find(|b| b.run_id == id) {
            seat.cancel.cancel();
        }
    }

    /// Cancels every background run of `chat_id` whose record is no longer
    /// among the chat's live messages — the exchange that started it was
    /// taken back or regenerated (research fork F8). The run lands onto the
    /// archived copy of the record and announces nothing.
    pub(super) fn cancel_orphaned_background_runs(&mut self, chat_id: Uuid) {
        let live: Vec<Uuid> = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .map(|c| c.children().map(|r| r.id).collect())
            .unwrap_or_default();
        for seat in self
            .background_runs
            .iter()
            .filter(|b| b.chat == chat_id && !live.contains(&b.run_id))
        {
            seat.cancel.cancel();
        }
    }

    /// Cancels every background run of `chat_id` — the chat is being deleted
    /// or is leaving the profile in front of the user. The task's own landing
    /// finds no chat and drops the run.
    pub(super) fn cancel_background_runs_of(&mut self, chat_id: Uuid) {
        for seat in self.background_runs.iter().filter(|b| b.chat == chat_id) {
            seat.cancel.cancel();
        }
    }

    /// `Quit` (research fork F7): every run out is cancelled and landed
    /// **now**, from its mirror — the rounds filed so far, `cancelled`, the
    /// time — onto its record, so the file the exit flush writes says what
    /// happened instead of reading as unfinished. The tasks' own landings
    /// arrive after the loop has ended and are dropped.
    pub(super) fn stop_all_background_runs(&mut self) {
        let seats = std::mem::take(&mut self.background_runs);
        for seat in seats {
            seat.cancel.cancel();
            let mut run = seat.child.run.clone();
            run.background = true;
            run.outcome = Some(RunOutcome::Cancelled);
            run.finished_at = Some(chrono::Utc::now());
            if let Some(record) = self
                .chat_mut(seat.chat)
                .and_then(|c| c.child_mut_including_deleted(run.id))
            {
                *record = run;
                self.mark_dirty(seat.chat);
            }
        }
    }

    /// The mirror of a run out in the background, by the run's id.
    pub(super) fn background_child(&self, run: Uuid) -> Option<&InflightChild> {
        self.background_runs
            .iter()
            .find(|b| b.run_id == run)
            .map(|b| &b.child)
    }

    /// The seat of a run out in the background, by the run's id.
    pub(super) fn background_run(&self, run: Uuid) -> Option<&BackgroundRun> {
        self.background_runs.iter().find(|b| b.run_id == run)
    }

    /// A run's mirror wherever it is — the turn's children first, then the
    /// background seats. `None` for a run that is not in flight.
    pub(super) fn child_any(&self, run: Uuid) -> Option<&InflightChild> {
        self.inflight
            .as_ref()
            .and_then(|t| t.child(run))
            .or_else(|| self.background_child(run))
    }

    /// [`Self::child_any`], mutable.
    pub(super) fn child_mut_any(&mut self, run: Uuid) -> Option<&mut InflightChild> {
        if self
            .inflight
            .as_ref()
            .is_some_and(|t| t.child(run).is_some())
        {
            return self.inflight.as_mut().and_then(|t| t.child_mut(run));
        }
        self.background_runs
            .iter_mut()
            .find(|b| b.run_id == run)
            .map(|b| &mut b.child)
    }

    /// The status bar's count of runs out in the background — the seats
    /// whose run has not ended (an ended one waiting for its turn to land
    /// is no longer out).
    pub(super) fn emit_background_runs(&self) {
        let out = self
            .background_runs
            .iter()
            .filter(|b| b.child.run.outcome.is_none())
            .count() as u32;
        let _ = self.evt_tx.send(AppEvent::BackgroundRuns { out });
    }

    /// The language the chat's profile speaks — what a notification is
    /// worded in, since the model reads it (axis A).
    fn profile_locale_of(&self, chat_id: Uuid) -> &'static crate::shared::i18n::Locale {
        let lang = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| self.profiles.iter().find(|p| p.id == c.profile_id))
            .map(|p| p.language)
            .unwrap_or_default();
        crate::shared::i18n::locale(lang)
    }
}

/// The session budget shared by the turns and the background runs
/// (research §4.7): one `Arc` per active engine section, rebuilt when the
/// section, its `sessions` or its pool change, cloned into every turn and
/// every run — so a run out in the background and the next turn take turns
/// under one permit count and one KV pool.
pub(super) type BudgetKey = (crate::shared::config::ServerMode, u32, Option<u64>);

impl Orchestrator {
    /// The current budget: the memoized one when nothing about it changed,
    /// else a fresh one — a run holding the old `Arc` keeps streaming under
    /// it, and the next stream takes the new one.
    pub(super) fn session_budget(&mut self) -> Arc<crate::shared::session_budget::SessionBudget> {
        let key: BudgetKey = (
            self.config.engine.mode,
            self.config.engine.active_sessions(),
            self.session_pool(),
        );
        if let Some((k, budget)) = &self.session_budget_memo
            && *k == key
        {
            return budget.clone();
        }
        let budget = Arc::new(crate::shared::session_budget::SessionBudget::new(
            key.1, key.2,
        ));
        self.session_budget_memo = Some((key, budget.clone()));
        budget
    }
}
