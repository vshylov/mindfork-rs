//! The self-model's history (spec §17.9; docs/research/companion-platform.md C9):
//! every version the store wrote, who wrote it, and what changed from the version
//! before. Pure types and the field-level comparison — the rows live in `data.db`
//! (`self_model_versions`), written by the storage layer under the same lock that
//! bumps the model's `version`.
//!
//! "Who wrote it" is recorded when the version is written, not derived later from
//! the profile's language-model history: the tools know the turn's model, the
//! `F3` screen knows it is the person, and a background reflection knows it is
//! one. A version found in the store without a record — written before the
//! history was kept, or by an older build — is [`VersionAuthor::Unknown`].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entities::profile::LlmChange;
use crate::entities::self_model::{GoalStatus, SelfModel};
use crate::shared::config::ServerMode;

/// Who wrote a version of the self-model.
#[derive(Debug, Clone, PartialEq)]
pub enum VersionAuthor {
    /// A language model, through one of the self-model tools.
    Model {
        /// The model's name as the turn knew it (`ToolContext::model_name`);
        /// `None` when the engine named none.
        name: Option<String>,
        mode: ServerMode,
        /// The conversation the write was made in, or reflected over.
        chat_id: Option<Uuid>,
        /// Written by a background task (a reflection), not in a turn.
        background: bool,
    },
    /// The person, on the `F3` screen.
    Person,
    /// The app's own housekeeping — moving the old narrative into notes.
    App,
    /// Found in the store with no record of who wrote it: a version from before
    /// the history was kept, or one an older build wrote.
    Unknown,
}

impl VersionAuthor {
    /// The stored `author` column. Values persist in `data.db` — do not rename.
    pub fn kind_key(&self) -> &'static str {
        match self {
            Self::Model {
                background: false, ..
            } => "turn",
            Self::Model {
                background: true, ..
            } => "background",
            Self::Person => "person",
            Self::App => "app",
            Self::Unknown => "unknown",
        }
    }

    /// The stored `model`, `mode` and `chat_id` columns.
    pub fn columns(&self) -> (Option<String>, Option<&'static str>, Option<String>) {
        match self {
            Self::Model {
                name,
                mode,
                chat_id,
                ..
            } => (
                name.clone(),
                Some(mode.key()),
                chat_id.map(|id| id.to_string()),
            ),
            _ => (None, None, None),
        }
    }

    /// A stored row back into the author. Lenient on purpose: a kind or a mode
    /// this build does not know reads as what is known of it, never as an error —
    /// the history is a record to read, and one odd row must not hide the rest.
    pub fn from_columns(
        kind: &str,
        model: Option<String>,
        mode: Option<&str>,
        chat_id: Option<&str>,
    ) -> Self {
        let background = match kind {
            "turn" => false,
            "background" => true,
            "person" => return Self::Person,
            "app" => return Self::App,
            _ => return Self::Unknown,
        };
        Self::Model {
            name: model,
            mode: mode.and_then(ServerMode::from_key).unwrap_or_default(),
            chat_id: chat_id.and_then(|id| Uuid::parse_str(id).ok()),
            background,
        }
    }
}

/// One version of a profile's self-model, as written.
#[derive(Debug, Clone, PartialEq)]
pub struct SelfModelVersion {
    pub version: u64,
    pub written_at: DateTime<Utc>,
    pub author: VersionAuthor,
    pub model: SelfModel,
}

/// What the `F3` screen's history view is handed (spec §17.9): the versions,
/// oldest first, and what it needs to say who wrote them — the titles of the
/// conversations they name, and the profile's language-model history for a
/// version whose author was not recorded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelfModelHistoryView {
    pub versions: Vec<SelfModelVersion>,
    pub chat_titles: HashMap<Uuid, String>,
    pub llm: Vec<LlmChange>,
}

impl SelfModelHistoryView {
    /// The model the profile was talking to at `at`, by its language-model
    /// history: the newest record at or before it. What a version with no
    /// recorded author can honestly be told — the profile's model then, not the
    /// version's writer.
    pub fn llm_at(&self, at: DateTime<Utc>) -> Option<&LlmChange> {
        self.llm.iter().rev().find(|rec| rec.changed_at <= at)
    }
}

/// One difference between two versions of the self-model, field by field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The self-description was written, rewritten or emptied.
    Summary,
    GoalAdded(String),
    GoalRemoved(String),
    GoalReworded {
        from: String,
        to: String,
    },
    GoalStatus {
        goal: String,
        from: GoalStatus,
        to: GoalStatus,
    },
    TraitsAdded(Vec<String>),
    TraitsRemoved(Vec<String>),
    InterestsAdded(Vec<String>),
    InterestsRemoved(Vec<String>),
    /// The relationship dynamic was written, rewritten or emptied.
    Relationship,
}

/// What changed from `prev` to `next`, in the order the screen shows the
/// fields: the self-description, the goals, then the user half. Goals are
/// matched by id, so a reworded goal reads as reworded rather than as one goal
/// gone and another come; list items are matched as text, so a reworded trait
/// reads as one removed and one added — which is what happened to the list.
pub fn changes(prev: &SelfModel, next: &SelfModel) -> Vec<Change> {
    let mut out = Vec::new();
    if prev.summary != next.summary {
        out.push(Change::Summary);
    }
    for goal in &next.goals {
        match prev.goals.iter().find(|g| g.id == goal.id) {
            None => out.push(Change::GoalAdded(goal.description.clone())),
            Some(old) => {
                if old.description != goal.description {
                    out.push(Change::GoalReworded {
                        from: old.description.clone(),
                        to: goal.description.clone(),
                    });
                }
                if old.status != goal.status {
                    out.push(Change::GoalStatus {
                        goal: goal.description.clone(),
                        from: old.status,
                        to: goal.status,
                    });
                }
            }
        }
    }
    for old in &prev.goals {
        if !next.goals.iter().any(|g| g.id == old.id) {
            out.push(Change::GoalRemoved(old.description.clone()));
        }
    }
    let (prev_user, next_user) = (&prev.user_model, &next.user_model);
    push_list(
        &mut out,
        &prev_user.perceived_traits,
        &next_user.perceived_traits,
        Change::TraitsAdded,
        Change::TraitsRemoved,
    );
    push_list(
        &mut out,
        &prev_user.current_interests,
        &next_user.current_interests,
        Change::InterestsAdded,
        Change::InterestsRemoved,
    );
    if prev_user.relationship_dynamic != next_user.relationship_dynamic {
        out.push(Change::Relationship);
    }
    out
}

/// The added items of a list (in the new list's order), then the removed ones
/// (in the old list's), each as one change when there are any.
fn push_list(
    out: &mut Vec<Change>,
    prev: &[String],
    next: &[String],
    added: fn(Vec<String>) -> Change,
    removed: fn(Vec<String>) -> Change,
) {
    let new: Vec<String> = next.iter().filter(|s| !prev.contains(s)).cloned().collect();
    let gone: Vec<String> = prev.iter().filter(|s| !next.contains(s)).cloned().collect();
    if !new.is_empty() {
        out.push(added(new));
    }
    if !gone.is_empty() {
        out.push(removed(gone));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::self_model::Goal;

    fn goal(text: &str, status: GoalStatus) -> Goal {
        Goal {
            id: Uuid::new_v4(),
            description: text.into(),
            status,
            created_at: Utc::now(),
            closed_at: None,
        }
    }

    #[test]
    fn identical_versions_have_no_changes() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "I am Sol.".into();
        m.goals.push(goal("learn chess", GoalStatus::Active));
        assert!(changes(&m, &m.clone()).is_empty());
    }

    #[test]
    fn every_field_is_compared_in_screen_order() {
        let mut prev = SelfModel::new(Uuid::new_v4());
        prev.summary = "I am Sol.".into();
        let kept = goal("learn chess", GoalStatus::Active);
        let reworded = goal("read more", GoalStatus::Active);
        let dropped = goal("walk daily", GoalStatus::Active);
        prev.goals = vec![kept.clone(), reworded.clone(), dropped.clone()];
        prev.user_model.perceived_traits = vec!["curious".into(), "shy".into()];
        prev.user_model.current_interests = vec!["chess".into()];
        prev.user_model.relationship_dynamic = "new friends".into();

        let mut next = prev.clone();
        next.summary = "I am Sol, a companion.".into();
        next.goals[0].status = GoalStatus::Completed;
        next.goals[1].description = "read a book a month".into();
        next.goals.retain(|g| g.id != dropped.id);
        let added = goal("cook together", GoalStatus::Active);
        next.goals.push(added.clone());
        next.user_model.perceived_traits = vec!["curious".into(), "warm".into()];
        next.user_model.current_interests = vec!["chess".into(), "go".into()];
        next.user_model.relationship_dynamic = "close friends".into();

        assert_eq!(
            changes(&prev, &next),
            vec![
                Change::Summary,
                Change::GoalStatus {
                    goal: "learn chess".into(),
                    from: GoalStatus::Active,
                    to: GoalStatus::Completed,
                },
                Change::GoalReworded {
                    from: "read more".into(),
                    to: "read a book a month".into(),
                },
                Change::GoalAdded("cook together".into()),
                Change::GoalRemoved("walk daily".into()),
                Change::TraitsAdded(vec!["warm".into()]),
                Change::TraitsRemoved(vec!["shy".into()]),
                Change::InterestsAdded(vec!["go".into()]),
                Change::Relationship,
            ]
        );
    }

    #[test]
    fn a_goal_reworded_and_closed_at_once_is_both() {
        let mut prev = SelfModel::new(Uuid::new_v4());
        prev.goals.push(goal("finish the book", GoalStatus::Active));
        let mut next = prev.clone();
        next.goals[0].description = "finish the novel".into();
        next.goals[0].status = GoalStatus::Abandoned;
        assert_eq!(
            changes(&prev, &next),
            vec![
                Change::GoalReworded {
                    from: "finish the book".into(),
                    to: "finish the novel".into(),
                },
                Change::GoalStatus {
                    goal: "finish the novel".into(),
                    from: GoalStatus::Active,
                    to: GoalStatus::Abandoned,
                },
            ]
        );
    }

    #[test]
    fn the_author_round_trips_through_its_columns() {
        let chat = Uuid::new_v4();
        let authors = [
            VersionAuthor::Model {
                name: Some("gemma-4-31b".into()),
                mode: ServerMode::External,
                chat_id: Some(chat),
                background: false,
            },
            VersionAuthor::Model {
                name: None,
                mode: ServerMode::Claude,
                chat_id: None,
                background: true,
            },
            VersionAuthor::Person,
            VersionAuthor::App,
            VersionAuthor::Unknown,
        ];
        for author in authors {
            let (model, mode, chat_id) = author.columns();
            let back =
                VersionAuthor::from_columns(author.kind_key(), model, mode, chat_id.as_deref());
            assert_eq!(back, author);
        }
    }

    #[test]
    fn an_unknown_kind_or_mode_reads_as_what_is_known() {
        assert_eq!(
            VersionAuthor::from_columns("fork", None, None, None),
            VersionAuthor::Unknown
        );
        assert_eq!(
            VersionAuthor::from_columns("turn", Some("m".into()), Some("warp"), Some("x")),
            VersionAuthor::Model {
                name: Some("m".into()),
                mode: ServerMode::default(),
                chat_id: None,
                background: false,
            }
        );
    }

    #[test]
    fn the_model_then_is_the_newest_record_at_or_before() {
        let at = |h: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap()
                .and_utc()
        };
        let rec = |h: u32, name: &str| LlmChange {
            changed_at: at(h),
            model: name.into(),
            mode: ServerMode::OpenAi,
        };
        let view = SelfModelHistoryView {
            llm: vec![rec(8, "a"), rec(12, "b")],
            ..Default::default()
        };
        assert_eq!(view.llm_at(at(7)), None);
        assert_eq!(view.llm_at(at(8)).map(|r| r.model.as_str()), Some("a"));
        assert_eq!(view.llm_at(at(11)).map(|r| r.model.as_str()), Some("a"));
        assert_eq!(view.llm_at(at(13)).map(|r| r.model.as_str()), Some("b"));
    }
}
