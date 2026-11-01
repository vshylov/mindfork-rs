//! A run's steps (docs/research/fabula-engine.md §9, *Resumable steps*): each
//! finished step is recorded under the run that finished it, and a resume —
//! a new run — reads what every run of the story finished, so it starts at
//! the first missing step and pays only for the step that was in flight.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use super::engine::SharedLog;
use crate::entities::story::RunId;
use crate::shared::storage::story::{StepStatus, StoryDb, StoryDbError};

/// The steps of a story as one run sees and records them.
pub struct Steps {
    log: SharedLog,
    run: RunId,
    known: Mutex<BTreeMap<String, StepStatus>>,
}

impl Steps {
    /// The steps every run of the story recorded so far, for `run` to go on
    /// from; `log` records the new ones.
    pub async fn load(db: &StoryDb, log: SharedLog, run: RunId) -> Result<Self, StoryDbError> {
        Ok(Self {
            known: Mutex::new(db.all_steps().await?),
            log,
            run,
        })
    }

    /// The run recording.
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// How the step ended in any run: done where any finished it.
    pub fn status(&self, address: &str) -> Option<StepStatus> {
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(address)
            .copied()
    }

    /// Whether any run finished the step.
    pub fn done(&self, address: &str) -> bool {
        self.status(address) == Some(StepStatus::Done)
    }

    /// Records how the step ended in this run. A step done before stays
    /// done in what this run sees.
    pub async fn finish(&self, address: &str, status: StepStatus) -> Result<(), StoryDbError> {
        self.log
            .lock()
            .await
            .finish_step(&self.run, address, status)
            .await?;
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = known.entry(address.to_owned()).or_insert(status);
        if status == StepStatus::Done {
            *entry = StepStatus::Done;
        }
        Ok(())
    }
}
