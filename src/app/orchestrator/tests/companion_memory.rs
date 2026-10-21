//! Orchestrator tests — the companion's memory in the system prompt
//! (docs/research/companion-platform.md C2, spec §6.6): the conversations
//! before this one and the notes nearest it, each a profile switch, off by
//! default. Part of the [`super`] module (fixtures in mod.rs).

use super::*;
use crate::entities::note::Note;
use crate::features::tools::notes::SELF_NOTE_TAG;

const EARLIER: &str = "[Conversations before this one]";
const NOTES: &str = "[Your notes nearest this conversation]";

/// What goes on disk before the app starts.
struct Seed {
    notes_in_prompt: bool,
    earlier_chats: bool,
    /// Whether the profile offers `chat_read` (and so the addresses).
    chat_read: bool,
}

/// Puts a profile, an earlier conversation, a conversation of another profile,
/// four notes and an empty current chat on disk; returns the current chat's id.
/// The opening the tests send is "aaaa". The test embedder counts characters
/// into 16 buckets, and `a`, `q` and `1` share one, so a note spelt in them is
/// as near the opening as it gets — it has to be: the app's embedding guard
/// calibrates the similarity scale against this embedder and raises the floor.
/// A note of z's is as far as it gets.
fn seed(root: &std::path::Path, s: &Seed) -> (Uuid, Uuid) {
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(root));
    let mut profile = Profile::new("Companion", "You are Sol.");
    profile.language = crate::shared::i18n::Lang::En;
    let mut tools = crate::features::tools::default_tool_ids();
    if s.chat_read {
        tools.push(crate::features::tools::chats::CHAT_READ_ID.into());
    }
    profile.enabled_tools = tools.clone();
    profile.known_tools = tools;
    profile.companion.notes_in_prompt = s.notes_in_prompt;
    profile.companion.earlier_chats = s.earlier_chats;
    json.upsert_profile(&profile).unwrap();
    let stranger = Profile::new("Stranger", "s");
    json.upsert_profile(&stranger).unwrap();

    let two_days_ago = chrono::Utc::now() - chrono::Duration::days(2);
    let mut earlier = Chat::from_profile(&profile, "Taper plan");
    let mut m = Message::user("my knee hurt after the long run");
    m.timestamp = two_days_ago;
    earlier.push_message(m);
    earlier.modified_at = two_days_ago;
    json.save_chat(&earlier).unwrap();
    let mut foreign = Chat::from_profile(&stranger, "Not yours");
    let mut m = Message::user("hello");
    m.timestamp = two_days_ago;
    foreign.push_message(m);
    foreign.modified_at = two_days_ago;
    json.save_chat(&foreign).unwrap();
    let current = Chat::from_profile(&profile, "now");
    let current_id = current.id;
    json.save_chat(&current).unwrap();

    let storage = Storage::open(Paths::with_root(root)).unwrap();
    let week_ago = chrono::Utc::now() - chrono::Duration::days(7);
    let note = |content: &str, tags: Vec<String>, at: chrono::DateTime<chrono::Utc>| Note {
        id: Uuid::new_v4(),
        profile_id: profile.id,
        content: content.into(),
        tags,
        created_at: at,
        updated_at: at,
    };
    for n in [
        note("aaaa qq1 person", vec![], week_ago),
        note("zzzz", vec![], week_ago),
        note("aaaa qq1 self", vec![SELF_NOTE_TAG.into()], week_ago),
        // Written after the conversation began: not something it knew before.
        note(
            "aaaa qq1 later",
            vec![],
            chrono::Utc::now() + chrono::Duration::days(1),
        ),
    ] {
        storage.db().note_insert(&n).unwrap();
    }
    (profile.id, current_id)
}

/// Starts the app on the seeded root with a capturing engine and the test
/// embedder, switched to the current chat.
async fn start(
    root: &std::path::Path,
    current: Uuid,
    backend: Arc<CapturingBackend>,
) -> (
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(root, Some(backend), no_auto_cfg());
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    // Switching to the chat already in front is a no-op that says nothing.
    if !matches!(activated, AppEvent::ChatActivated { id, .. } if id == current) {
        cmd_tx.send(AppCommand::SwitchChat(current)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == current),
        )
        .await
        .unwrap();
    }
    (cmd_tx, evt_rx, handle)
}

async fn turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    backend: &CapturingBackend,
    text: &str,
) -> String {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    wait_for(evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    backend.last_request().system.unwrap_or_default()
}

#[tokio::test]
async fn both_blocks_reach_the_model_and_hold_on_the_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    let (_, current) = seed(
        dir.path(),
        &Seed {
            notes_in_prompt: true,
            earlier_chats: true,
            chat_read: true,
        },
    );
    let backend = CapturingBackend::new();
    let (cmd_tx, mut evt_rx, handle) = start(dir.path(), current, backend.clone()).await;

    let first = turn(&cmd_tx, &mut evt_rx, &backend, "aaaa").await;
    // The earlier conversation, aged and addressed; never another profile's.
    assert!(first.contains(EARLIER), "{first}");
    assert!(first.contains("- (2d) Taper plan — chat://"), "{first}");
    assert!(first.contains("Read one with chat_read."), "{first}");
    assert!(!first.contains("Not yours"), "{first}");
    // The note near the opening — not the far one, not an observation, not one
    // written after the conversation began.
    assert!(first.contains(NOTES), "{first}");
    assert!(first.contains("aaaa qq1 person"), "{first}");
    for absent in ["zzzz", "aaaa qq1 self", "aaaa qq1 later"] {
        assert!(!first.contains(absent), "{absent}: {first}");
    }
    assert!(
        first.contains("More notes are found with note_recall."),
        "{first}"
    );
    // The earlier conversations come first, the notes after them.
    assert!(first.find(EARLIER) < first.find(NOTES), "{first}");

    // A follow-up on another subject: the system prompt is the same text, so the
    // prefix holds (spec §6.6).
    let second = turn(&cmd_tx, &mut evt_rx, &backend, "zzzz something else").await;
    assert_eq!(first, second);

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn without_chat_read_the_earlier_block_names_no_address() {
    let dir = tempfile::tempdir().unwrap();
    let (_, current) = seed(
        dir.path(),
        &Seed {
            notes_in_prompt: false,
            earlier_chats: true,
            chat_read: false,
        },
    );
    let backend = CapturingBackend::new();
    let (cmd_tx, mut evt_rx, handle) = start(dir.path(), current, backend.clone()).await;
    let system = turn(&cmd_tx, &mut evt_rx, &backend, "aaaa").await;
    assert!(system.contains("- (2d) Taper plan"), "{system}");
    assert!(
        !system.contains("chat://") && !system.contains("chat_read"),
        "{system}"
    );
    assert!(
        !system.contains(NOTES),
        "the notes are a switch of their own: {system}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn off_by_default_the_request_carries_neither_block() {
    let dir = tempfile::tempdir().unwrap();
    let (_, current) = seed(
        dir.path(),
        &Seed {
            notes_in_prompt: false,
            earlier_chats: false,
            chat_read: true,
        },
    );
    let backend = CapturingBackend::new();
    let (cmd_tx, mut evt_rx, handle) = start(dir.path(), current, backend.clone()).await;
    let system = turn(&cmd_tx, &mut evt_rx, &backend, "aaaa").await;
    assert_eq!(system, "You are Sol.");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn an_opening_near_no_note_gets_no_notes_block() {
    let dir = tempfile::tempdir().unwrap();
    let (_, current) = seed(
        dir.path(),
        &Seed {
            notes_in_prompt: true,
            earlier_chats: false,
            chat_read: false,
        },
    );
    let backend = CapturingBackend::new();
    let (cmd_tx, mut evt_rx, handle) = start(dir.path(), current, backend.clone()).await;
    // "bbbb" shares no letter with any note.
    let system = turn(&cmd_tx, &mut evt_rx, &backend, "bbbb").await;
    assert_eq!(system, "You are Sol.");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}
