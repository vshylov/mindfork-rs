//! The notes screen's data (`/notes`, spec §11.11;
//! docs/research/companion-platform.md C3): the snapshot of the active
//! profile's notes, and the edits the person makes on it. The screen is a
//! projection; the orchestrator owns the storage and answers every request and
//! every edit with a fresh [`NotesView`].

use uuid::Uuid;

use super::{Orchestrator, request};
use crate::app::events::AppEvent;
use crate::entities::note::{NoteCard, NoteEdit, NoteLinkView, NotesView};
use crate::features::tools::notes::{SELF_NOTE_TAG, is_self_note};
use crate::shared::storage::Storage;

/// What the snapshot's second half needs, read off the orchestrator before the
/// task that embeds the opening is spawned.
struct PromptPicks {
    profile_id: Uuid,
    /// The conversation's opening (or its summary) — the query a turn picks by.
    query: String,
    cut: chrono::DateTime<chrono::Utc>,
    notes_in_prompt: bool,
    self_model_in_prompt: bool,
    params: crate::entities::self_model::SelfModelParams,
}

impl Orchestrator {
    /// `RequestNotes`: sends the notes screen its snapshot.
    pub(super) fn handle_request_notes(&self) {
        self.emit_notes_view(None);
    }

    /// `EditNote`: applies a change the person made on the notes screen and
    /// sends a fresh snapshot. The screen shows no `@self` observation, and none
    /// can be changed through it — they are `F3`'s; a tag the person types as
    /// `@self` is dropped, so a note cannot be turned into one either.
    pub(super) fn handle_edit_note(&self, edit: NoteEdit) {
        let Some(pid) = self.active_profile_id() else {
            return;
        };
        let id = match &edit {
            NoteEdit::SetText { id, .. } | NoteEdit::SetTags { id, .. } | NoteEdit::Delete(id) => {
                *id
            }
        };
        let editable = self
            .storage
            .db()
            .note_get(pid, id)
            .ok()
            .flatten()
            .is_some_and(|n| !is_self_note(&n));
        if !editable {
            self.emit_notes_view(None);
            return;
        }
        let mut reembed = None;
        match edit {
            NoteEdit::SetText { text, .. } => {
                let text = text.trim().to_string();
                if !text.is_empty()
                    && self
                        .storage
                        .db()
                        .note_update(id, pid, &text)
                        .unwrap_or(false)
                {
                    reembed = Some((id, text));
                }
            }
            NoteEdit::SetTags { tags, .. } => {
                let _ = self.storage.db().note_set_tags(pid, id, &clean_tags(tags));
            }
            NoteEdit::Delete(_) => {
                let _ = self.storage.db().note_delete(pid, id);
            }
        }
        self.emit_notes_view(reembed);
    }

    /// Builds the snapshot and sends it. The notes, their links and their trace
    /// are read here; what this conversation's prompt carries is picked the way
    /// a turn picks it — by embedding the opening — so the reply leaves from a
    /// task, which first re-embeds a note whose text was just edited
    /// (`reembed`), so the picks see the new words.
    fn emit_notes_view(&self, reembed: Option<(Uuid, String)>) {
        let Some(pid) = self.active_profile_id() else {
            let _ = self.evt_tx.send(AppEvent::NotesView(Box::default()));
            return;
        };
        let notes = self.note_cards(pid);
        let picks = self.prompt_picks(pid);
        let storage = self.storage.clone();
        let embedder = self.engines.embedder();
        let evt_tx = self.evt_tx.clone();
        tokio::spawn(async move {
            if let Some((id, text)) = reembed
                && let Ok(vecs) = embedder
                    .embed(vec![text], crate::shared::api::EmbedRole::Passage)
                    .await
                && let Some(emb) = vecs.into_iter().next()
            {
                let _ = storage.db().note_vector_upsert(id, pid, &emb);
            }
            let view = match picks {
                Some(p) => shown_in_prompt(&storage, embedder.as_ref(), notes, p).await,
                None => NotesView {
                    notes,
                    ..NotesView::default()
                },
            };
            let _ = evt_tx.send(AppEvent::NotesView(Box::new(view)));
        });
    }

    /// The profile's notes — not its `@self` observations — newest first, each
    /// with its links and the notes it superseded.
    fn note_cards(&self, pid: Uuid) -> Vec<NoteCard> {
        let db = self.storage.db();
        db.note_list(pid, None, &[], None)
            .unwrap_or_default()
            .into_iter()
            .filter(|n| !is_self_note(n))
            .map(|note| NoteCard {
                links: db
                    .note_neighbors(pid, note.id, None)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(other, relation, outgoing)| NoteLinkView {
                        relation,
                        outgoing,
                        other,
                    })
                    .collect(),
                supersedes: db.note_superseded_by(pid, note.id).unwrap_or_default(),
                note,
            })
            .collect()
    }

    /// What a turn of the active chat would pick its memory by — the same
    /// switches, query and cut [`Self::start_generation`] reads.
    fn prompt_picks(&self, pid: Uuid) -> Option<PromptPicks> {
        let chat = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))?;
        let profile = self.profiles.iter().find(|p| p.id == pid)?;
        Some(PromptPicks {
            profile_id: pid,
            query: request::observation_query(chat, self.config.compaction.enabled),
            cut: crate::features::companion_memory::memory_cut(
                chat,
                self.config.compaction.enabled,
            ),
            notes_in_prompt: profile.companion.notes_in_prompt,
            // The turn's own gate for the self-model block (`inject_enabled`).
            self_model_in_prompt: profile
                .enabled_tools
                .iter()
                .any(|t| t == crate::features::tools::self_model::GET_SELF_MODEL_ID),
            params: crate::entities::self_model::SelfModelParams::from_settings(
                &self.config.self_model,
            ),
        })
    }
}

/// The snapshot's second half: the notes and the observations this
/// conversation's prompt carries, picked exactly as a turn picks them
/// (`notes_for_prompt`, `injection_recent`).
async fn shown_in_prompt(
    storage: &Storage,
    embedder: &dyn crate::shared::api::Embedder,
    notes: Vec<NoteCard>,
    p: PromptPicks,
) -> NotesView {
    let shown = if p.notes_in_prompt {
        crate::features::tools::notes::notes_for_prompt(
            storage,
            embedder,
            p.profile_id,
            &p.query,
            p.cut,
            crate::features::companion_memory::NOTES_SHOWN,
        )
        .await
        .into_iter()
        .map(|n| n.id)
        .collect()
    } else {
        Vec::new()
    };
    let observations = if p.self_model_in_prompt {
        super::generation::injection_recent(
            storage,
            embedder,
            p.profile_id,
            true,
            &p.query,
            &p.params,
        )
        .await
        .0
    } else {
        Vec::new()
    };
    NotesView {
        notes,
        shown,
        observations,
        notes_in_prompt: p.notes_in_prompt,
    }
}

/// Tags as the person typed them: trimmed, empty ones and repeats dropped, and
/// never the reserved `@self` — that would move the note into the self-model.
fn clean_tags(tags: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim().to_string();
        if !t.is_empty() && t != SELF_NOTE_TAG && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_tags_are_cleaned_and_never_reserved() {
        let got = clean_tags(vec![
            " pref ".into(),
            String::new(),
            "pref".into(),
            SELF_NOTE_TAG.into(),
            "drink".into(),
        ]);
        assert_eq!(got, ["pref", "drink"]);
    }
}
