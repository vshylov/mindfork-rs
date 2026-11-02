//! The story runs out in the background (docs/fabula.md §4, PR 8), as the
//! app, its tools and the runs themselves share them.
//!
//! A story has one writer at a time (spec §9), so a run **claims** its story
//! here before it opens the files, and a second run of the same story is
//! refused with the first one's address. While it runs, the run writes the
//! address of the call in flight, which is what `story_status` reports and
//! what a run cut short by a quit says it stopped at. The orchestrator holds
//! the one board; the tools read it through their context.

use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// One story run out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunView {
    /// The chat the story belongs to.
    pub chat: Uuid,
    /// The story's id within its chat.
    pub story: String,
    /// The run's id: its record's, its transcript's address.
    pub run: Uuid,
    /// When it started.
    pub started_at: DateTime<Utc>,
    /// The address of the call it made last, `ch1/sc2/b1/r3/...`; `None`
    /// before its first.
    pub step: Option<String>,
}

/// The board. See the module doc.
#[derive(Debug, Default)]
pub struct StoryBoard {
    runs: Mutex<Vec<RunView>>,
}

impl StoryBoard {
    /// An empty board.
    pub fn new() -> Self {
        Self::default()
    }

    fn runs(&self) -> std::sync::MutexGuard<'_, Vec<RunView>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims `story` of `chat` for the run `run`, or names the run that
    /// holds it already.
    pub fn claim(&self, chat: Uuid, story: &str, run: Uuid) -> Result<(), RunView> {
        let mut runs = self.runs();
        if let Some(held) = runs.iter().find(|r| r.chat == chat && r.story == story) {
            return Err(held.clone());
        }
        runs.push(RunView {
            chat,
            story: story.to_owned(),
            run,
            started_at: Utc::now(),
            step: None,
        });
        Ok(())
    }

    /// Records the call `run` is making.
    pub fn step(&self, run: Uuid, address: &str) {
        if let Some(view) = self.runs().iter_mut().find(|r| r.run == run) {
            view.step = Some(address.to_owned());
        }
    }

    /// Gives `run`'s story back; what the board held of it.
    pub fn release(&self, run: Uuid) -> Option<RunView> {
        let mut runs = self.runs();
        let at = runs.iter().position(|r| r.run == run)?;
        Some(runs.remove(at))
    }

    /// The run `run`, while it is out.
    pub fn of_run(&self, run: Uuid) -> Option<RunView> {
        self.runs().iter().find(|r| r.run == run).cloned()
    }

    /// The runs out in `chat`, oldest first.
    pub fn of_chat(&self, chat: Uuid) -> Vec<RunView> {
        self.runs()
            .iter()
            .filter(|r| r.chat == chat)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One run per story: a second claim names the first; another story of
    /// the chat, or the same id in another chat, is free; a release frees it.
    #[test]
    fn a_story_has_one_run_at_a_time() {
        let board = StoryBoard::new();
        let (chat, other) = (Uuid::new_v4(), Uuid::new_v4());
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        board.claim(chat, "s1", first).unwrap();
        let held = board.claim(chat, "s1", second).unwrap_err();
        assert_eq!(held.run, first);
        board.claim(chat, "s2", second).unwrap();
        board.claim(other, "s1", Uuid::new_v4()).unwrap();
        assert_eq!(board.of_chat(chat).len(), 2);

        assert_eq!(board.release(first).map(|v| v.story).as_deref(), Some("s1"));
        assert!(board.release(first).is_none());
        board.claim(chat, "s1", Uuid::new_v4()).unwrap();
    }

    /// The step is the run's last call, and only that run's.
    #[test]
    fn a_step_is_recorded_on_its_run() {
        let board = StoryBoard::new();
        let chat = Uuid::new_v4();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        board.claim(chat, "s1", a).unwrap();
        board.claim(chat, "s2", b).unwrap();
        assert_eq!(board.of_run(a).unwrap().step, None);
        board.step(a, "ch1/outline");
        board.step(a, "ch1/sc1/plan");
        board.step(Uuid::new_v4(), "nowhere");
        assert_eq!(
            board.of_run(a).unwrap().step.as_deref(),
            Some("ch1/sc1/plan")
        );
        assert_eq!(board.of_run(b).unwrap().step, None);
    }
}
