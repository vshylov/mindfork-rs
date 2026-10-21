//! Orchestrator tests — the notes screen's snapshot and edits (`RequestNotes`,
//! `EditNote`; spec §11.11). Part of the [`super`] module (fixtures in mod.rs).

use super::*;
use crate::entities::note::{Note, NoteEdit, NotesView};
use crate::features::tools::notes::SELF_NOTE_TAG;

struct Seeded {
    chat: Uuid,
    profile: Uuid,
    near: Note,
    far: Note,
    observation: Note,
    old: Note,
}

/// A profile with *Notes in the prompt* on, a chat opened with "aaaa", and four
/// notes: one near the opening, one far from it (which superseded an older
/// one and links to the near one), and an `@self` observation. Under the test
/// embedder `a`, `q` and `1` share a bucket — see `tests/companion_memory.rs`.
fn seed(root: &std::path::Path, notes_in_prompt: bool) -> Seeded {
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(root));
    let mut profile = Profile::new("Companion", "You are Sol.");
    profile.language = crate::shared::i18n::Lang::En;
    profile.enabled_tools = crate::features::tools::default_tool_ids();
    profile.known_tools = profile.enabled_tools.clone();
    profile.companion.notes_in_prompt = notes_in_prompt;
    json.upsert_profile(&profile).unwrap();
    let mut chat = Chat::from_profile(&profile, "now");
    chat.push_message(Message::user("aaaa"));
    json.save_chat(&chat).unwrap();

    let storage = Storage::open(Paths::with_root(root)).unwrap();
    let at = |days: i64| chrono::Utc::now() - chrono::Duration::days(days);
    let mk = |content: &str, tags: Vec<String>, days: i64| Note {
        id: Uuid::new_v4(),
        profile_id: profile.id,
        content: content.into(),
        tags,
        created_at: at(days),
        updated_at: at(days),
    };
    let near = mk("aaaa qq1 near", vec!["work".into()], 3);
    let far = mk("zzzz far", vec![], 2);
    let observation = mk("aaaa qq1 self", vec![SELF_NOTE_TAG.into()], 1);
    let old = mk("zzzz older", vec![], 9);
    for n in [&near, &far, &observation, &old] {
        storage.db().note_insert(n).unwrap();
    }
    storage
        .db()
        .note_supersede_mark(profile.id, old.id, far.id)
        .unwrap();
    storage
        .db()
        .note_link_insert(profile.id, far.id, near.id, "relates")
        .unwrap();
    Seeded {
        chat: chat.id,
        profile: profile.id,
        near,
        far,
        observation,
        old,
    }
}

async fn start(
    root: &std::path::Path,
    chat: Uuid,
) -> (
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(root, None, no_auto_cfg());
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    if !matches!(activated, AppEvent::ChatActivated { id, .. } if id == chat) {
        cmd_tx.send(AppCommand::SwitchChat(chat)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == chat),
        )
        .await
        .unwrap();
    }
    (cmd_tx, evt_rx, handle)
}

async fn next_view(evt_rx: &mut UnboundedReceiver<AppEvent>) -> NotesView {
    match wait_for(evt_rx, |e| matches!(e, AppEvent::NotesView(_))).await {
        Some(AppEvent::NotesView(v)) => *v,
        other => panic!("expected NotesView, got {other:?}"),
    }
}

#[tokio::test]
async fn the_snapshot_holds_the_notes_their_graph_and_what_the_prompt_carries() {
    let dir = tempfile::tempdir().unwrap();
    let s = seed(dir.path(), true);
    let (cmd_tx, mut evt_rx, handle) = start(dir.path(), s.chat).await;
    cmd_tx.send(AppCommand::RequestNotes).unwrap();
    let v = next_view(&mut evt_rx).await;

    // The notes — never the observation, never the superseded one — newest first.
    let ids: Vec<Uuid> = v.notes.iter().map(|c| c.note.id).collect();
    assert_eq!(ids, [s.far.id, s.near.id]);
    // The far note's graph: its link to the near one, and the note it replaced.
    let far = &v.notes[0];
    assert_eq!(far.links.len(), 1);
    assert_eq!(
        (far.links[0].relation.as_str(), far.links[0].outgoing),
        ("relates", true)
    );
    assert_eq!(far.links[0].other.id, s.near.id);
    assert_eq!(
        far.supersedes.iter().map(|n| n.id).collect::<Vec<_>>(),
        [s.old.id]
    );
    // What this conversation's prompt carries: the near note, picked as a turn
    // picks it; the self-model is not on, so no observations.
    assert!(v.notes_in_prompt);
    assert_eq!(v.shown, [s.near.id]);
    assert!(v.observations.is_empty());
    let _ = s.observation;

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn with_the_switch_off_nothing_is_marked_and_the_screen_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let s = seed(dir.path(), false);
    let (cmd_tx, mut evt_rx, handle) = start(dir.path(), s.chat).await;
    cmd_tx.send(AppCommand::RequestNotes).unwrap();
    let v = next_view(&mut evt_rx).await;
    assert!(!v.notes_in_prompt);
    assert!(v.shown.is_empty());
    assert_eq!(v.notes.len(), 2);
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn edits_are_stored_and_answered_with_a_fresh_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let s = seed(dir.path(), true);
    let (cmd_tx, mut evt_rx, handle) = start(dir.path(), s.chat).await;

    // The text: stored, and re-embedded — "zzzz" moves next to the opening and
    // the prompt now carries it.
    cmd_tx
        .send(AppCommand::EditNote(NoteEdit::SetText {
            id: s.far.id,
            text: "  aaaa qq1 now near  ".into(),
        }))
        .unwrap();
    let v = next_view(&mut evt_rx).await;
    let far = v.notes.iter().find(|c| c.note.id == s.far.id).unwrap();
    assert_eq!(far.note.content, "aaaa qq1 now near");
    assert!(v.shown.contains(&s.far.id), "{:?}", v.shown);

    // The tags: cleaned, and never the reserved one.
    cmd_tx
        .send(AppCommand::EditNote(NoteEdit::SetTags {
            id: s.near.id,
            tags: vec![" life ".into(), SELF_NOTE_TAG.into(), "life".into()],
        }))
        .unwrap();
    let v = next_view(&mut evt_rx).await;
    let near = v.notes.iter().find(|c| c.note.id == s.near.id).unwrap();
    assert_eq!(near.note.tags, ["life"]);

    // An observation is F3's: the screen's edits do not reach it.
    cmd_tx
        .send(AppCommand::EditNote(NoteEdit::Delete(s.observation.id)))
        .unwrap();
    let _ = next_view(&mut evt_rx).await;

    // A delete: gone, and its link with it.
    cmd_tx
        .send(AppCommand::EditNote(NoteEdit::Delete(s.near.id)))
        .unwrap();
    let v = next_view(&mut evt_rx).await;
    assert_eq!(
        v.notes.iter().map(|c| c.note.id).collect::<Vec<_>>(),
        [s.far.id]
    );
    assert!(v.notes[0].links.is_empty());

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    assert!(
        storage
            .db()
            .note_get(s.profile, s.observation.id)
            .unwrap()
            .is_some(),
        "the observation is still there"
    );
}
