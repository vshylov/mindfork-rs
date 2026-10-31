//! Replay from the call log (docs/research/fabula-engine.md §9, *Replay is
//! keyed by address, not by prompt*): every attempt is served from the record
//! under its run, address and attempt, and walks the same policy the live
//! call walked ([`super::port::run_json`]), so a replayed scene commits what
//! the recorded one committed. Nothing reaches a model.
//!
//! The prompt that would be sent now is hashed and compared with the one
//! recorded. A difference is **drift**: the reply is still served, since the
//! address is the key, and the drift is logged and kept for the caller to
//! judge. A replay test of an unchanged engine asserts there is none.

use std::sync::Mutex;

use serde_json::Value;

use super::port::{
    Answer, Attempt, Attempts, CallAddress, LlmPort, PortError, Prompt, Role, prompt_sha256,
    run_json, run_text,
};
use super::schema::Schema;
use crate::shared::storage::story::StoryDb;

/// A prompt that differs from the one recorded at its address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    /// The address.
    pub address: String,
    /// The attempt.
    pub attempt: u32,
    /// The hash recorded.
    pub recorded: String,
    /// The hash of the prompt sent now.
    pub now: String,
}

/// The port over a story's call log.
pub struct ReplayAdapter {
    db: StoryDb,
    drift: Mutex<Vec<Drift>>,
}

impl ReplayAdapter {
    /// Serves the calls recorded in `db`'s call log.
    pub fn new(db: StoryDb) -> Self {
        Self {
            db,
            drift: Mutex::new(Vec::new()),
        }
    }

    /// Every drift found so far, in the order found.
    pub fn drift(&self) -> Vec<Drift> {
        self.drift
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl LlmPort for ReplayAdapter {
    async fn json(
        &self,
        call: &CallAddress,
        _role: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Result<Value, PortError> {
        let mut recorded = Recorded {
            adapter: self,
            call,
            prompt_sha256: prompt_sha256(prompt, Some(schema)),
        };
        run_json(&mut recorded, &call.address, schema, prompt).await
    }

    async fn text(
        &self,
        call: &CallAddress,
        _role: Role,
        prompt: &Prompt,
    ) -> Result<String, PortError> {
        let mut recorded = Recorded {
            adapter: self,
            call,
            prompt_sha256: prompt_sha256(prompt, None),
        };
        run_text(&mut recorded, &call.address, prompt).await
    }
}

/// One call's attempts, read back.
struct Recorded<'a> {
    adapter: &'a ReplayAdapter,
    call: &'a CallAddress,
    prompt_sha256: String,
}

#[async_trait::async_trait]
impl Attempts for Recorded<'_> {
    async fn attempt(&mut self, attempt: &Attempt) -> Result<Answer, PortError> {
        let stored = self
            .adapter
            .db
            .call(&self.call.run, &self.call.address, attempt.number)
            .await?
            .ok_or_else(|| PortError::NotRecorded {
                run: self.call.run.clone(),
                address: self.call.address.clone(),
                attempt: attempt.number,
            })?;
        if stored.call.prompt_sha256 != self.prompt_sha256 {
            tracing::warn!(
                address = %self.call.address,
                attempt = attempt.number,
                "replay drift: the prompt differs from the one recorded"
            );
            self.adapter
                .drift
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Drift {
                    address: self.call.address.clone(),
                    attempt: attempt.number,
                    recorded: stored.call.prompt_sha256.clone(),
                    now: self.prompt_sha256.clone(),
                });
        }
        Ok(Answer::from_json(
            stored.call.status,
            stored.call.response.as_ref(),
        ))
    }
}
