//! An assistant note (memory about the user/context). Isolated per profile.
//! See spec §5.1, §9.3, §9.5.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A note belonging to a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One link of a note as the notes screen shows it (spec §11.11): the relation,
/// its direction, and the note at the other end.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteLinkView {
    pub relation: String,
    /// The link points from this note to [`Self::other`].
    pub outgoing: bool,
    pub other: Note,
}

/// A note with what surrounds it, for the notes screen.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteCard {
    pub note: Note,
    /// Its links to active notes, either direction.
    pub links: Vec<NoteLinkView>,
    /// The notes it superseded — hidden everywhere else, kept as its trace.
    pub supersedes: Vec<Note>,
}

/// The notes screen's snapshot of the active profile (spec §11.11,
/// docs/research/companion-platform.md C3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NotesView {
    /// The profile's notes — not its `@self` observations — newest first.
    pub notes: Vec<NoteCard>,
    /// Which of them this conversation's prompt carries (`notes_in_prompt`, spec §6.6).
    pub shown: Vec<Uuid>,
    /// The observations this conversation's prompt carries (the self-model's
    /// picks, spec §17.4) — shown, not edited here: they are `F3`'s.
    pub observations: Vec<crate::entities::self_model::NarrativeSegment>,
    /// The profile has *Notes in the prompt* on — so an empty [`Self::shown`]
    /// can say whether nothing was near or the feature is off.
    pub notes_in_prompt: bool,
}

/// An edit made on the notes screen; the orchestrator applies it and sends a
/// fresh [`NotesView`].
#[derive(Debug, Clone, PartialEq)]
pub enum NoteEdit {
    SetText { id: Uuid, text: String },
    SetTags { id: Uuid, tags: Vec<String> },
    Delete(Uuid),
}

impl Note {
    pub fn new(profile_id: Uuid, content: impl Into<String>, tags: Vec<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            profile_id,
            content: content.into(),
            tags,
            created_at: now,
            updated_at: now,
        }
    }
}
