//! What the engine calls the model it is running, when settings do not say.
//!
//! `EngineSettings::active_model_name()` answers for every mode whose model is a
//! *setting* — a managed server's GGUF, a cloud provider's required model name.
//! It cannot answer for `external`, where the "Model (opt.)" field is optional
//! and normally blank: connecting to a `llama-server` by URL is the one
//! configuration where the app genuinely does not know what it is talking to,
//! and the feed's caption and every message's metadata snapshot went empty
//! because of it (spec §11.3).
//!
//! So the engine is asked — [`model_id`](crate::shared::api::EngineBackend::model_id), answered out of
//! `GET /v1/models` and llama.cpp's `/props`. The state here is the same shape as
//! [`ContextDiscovery`](super::compaction::ContextDiscovery), for the same
//! reasons: an `epoch` so an answer that belongs to a replaced engine is dropped,
//! a `pending` flag so the question is asked once, an `answered` flag so "cannot
//! say" is not re-asked forever.
//!
//! Two differences from the context budget, both deliberate:
//!
//! - **the question is asked eagerly**, when the engine is applied and when its
//!   readiness flips, not lazily on first use. The budget is needed only as a
//!   conversation approaches its window; a name is needed by a caption that is on
//!   screen before the first message;
//! - **it is asked only when the configuration cannot name a model.** A name the
//!   user typed always wins, and asking anyway would spend a round trip to
//!   confirm what settings already say.
//!
//! Nothing is written to `settings.json`: the discovered name lives for as long
//! as the connection does. See docs/research/external-model-name.md §4, §7 (F2,
//! F3).

use super::Orchestrator;
use crate::app::events::AppEvent;

/// What the engine has said about the model it is running.
#[derive(Default)]
pub(super) struct ModelDiscovery {
    /// Bumped by [`Self::invalidate`]. An answer for an older epoch is dropped —
    /// switching servers mid-question must not label the new one with the old
    /// one's model.
    epoch: u64,
    /// A question is in flight — don't ask again.
    pending: bool,
    /// An answer arrived for the current epoch (possibly "cannot say").
    answered: bool,
    /// The name the engine reported, already display-shaped.
    known: Option<String>,
}

impl ModelDiscovery {
    /// Forgets what was learned: the engine changed, or its readiness flipped and
    /// a server that could not answer before may answer now.
    fn invalidate(&mut self) {
        self.epoch += 1;
        self.pending = false;
        self.answered = false;
        self.known = None;
    }

    /// The engine generation a pending answer would have to match.
    #[cfg(test)]
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Whether a question is in flight — the observable difference between "the
    /// engine was asked" and "settings already answered".
    #[cfg(test)]
    pub(super) fn pending(&self) -> bool {
        self.pending
    }

    /// Applies an answer if it belongs to the current engine.
    fn apply(&mut self, epoch: u64, name: Option<String>) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.pending = false;
        self.answered = true;
        self.known = name;
        true
    }
}

impl Orchestrator {
    /// The model name for the feed's caption and for a message's metadata
    /// snapshot: what the configuration says, or — when it says nothing — what
    /// the engine reported about itself.
    ///
    /// A pure read. The asking happens when the engine is applied
    /// ([`Self::refresh_model_name`]), so a turn never waits on a round trip and
    /// the header cannot name one model while the stored message claims another.
    pub(super) fn effective_model_name(&self) -> Option<String> {
        self.config
            .engine
            .active_model_name()
            .or_else(|| self.model.known.clone())
    }

    /// The engine changed, or its readiness flipped: drop what was learned, tell
    /// the UI the name is gone, and ask again if the configuration still cannot
    /// name a model.
    pub(super) fn refresh_model_name(&mut self) {
        self.model.invalidate();
        // The caption must not keep naming the previous server's model while the
        // new one is still connecting.
        self.emit_model_name();
        if self.config.engine.active_model_name().is_some() {
            // Settings answer it; the engine's opinion would never be read.
            return;
        }
        // Deliberately `backend` rather than a readiness gate, like the context
        // budget: a server still loading its weights answers both `/v1/models`
        // and `/props` perfectly well.
        let Some(backend) = self.engines.backend.clone() else {
            return;
        };
        self.model.pending = true;
        let epoch = self.model.epoch;
        let tx = self.model_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send((epoch, backend.model_id().await));
        });
    }

    /// Records what the engine answered, and tells the UI.
    pub(super) fn handle_model_result(&mut self, epoch: u64, name: Option<String>) {
        if !self.model.apply(epoch, name) {
            return;
        }
        if let Some(name) = &self.model.known {
            tracing::info!(model = %name, "engine reported the model it is running");
        }
        self.emit_model_name();
    }

    /// Sends the discovered name to the UI. Only the *discovered* half travels:
    /// the screen already holds the configuration and prefers it, so one value
    /// never has two meanings.
    fn emit_model_name(&self) {
        let _ = self
            .evt_tx
            .send(AppEvent::EngineModel(self.model.known.clone()));
    }
}
