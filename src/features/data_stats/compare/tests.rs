//! Tests of `mindfork stats --compare` (docs/history/data-stats.md §5.4). The other
//! copy is a byte copy of the data root, changed by the **real** writers — one
//! change per test — so each test says what that change must move: one counter,
//! the verdict, and the fingerprint.

use std::fs;
use std::path::Path;

use super::super::tests::{PASSWORD, Root, backup, en, root};
use super::super::{OtherCopy, collect_archive, collect_root, read_snapshot, render_json};
use super::*;
use crate::entities::chat::{Chat, DeletedExchange};
use crate::entities::message::Message;
use crate::entities::note::Note;
use crate::shared::paths::Paths;
use crate::shared::storage::{JsonStore, Storage};

/// A data root and a byte copy of it: *here* and *there*.
struct Pair {
    here: Root,
    _there_dir: tempfile::TempDir,
    there: Paths,
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn pair() -> Pair {
    let here = root();
    let there_dir = tempfile::tempdir().unwrap();
    let there = Paths::with_root(there_dir.path().join("data"));
    copy_tree(here.paths.root(), there.root());
    Pair {
        here,
        _there_dir: there_dir,
        there,
    }
}

impl Pair {
    fn compared(&self) -> Comparison {
        let here = collect_root(&self.here.paths, en()).unwrap();
        let there = collect_root(&self.there, en()).unwrap();
        compare(&here, &there, None)
    }

    fn fingerprints(&self) -> (String, String) {
        let c = self.compared();
        (c.here.fingerprint, c.there.fingerprint)
    }
}

/// The fixture's chat that has messages, a deleted exchange and a sub-agent run.
fn busy(paths: &Paths) -> Chat {
    let chats = JsonStore::new(paths.clone()).load_chats().unwrap();
    chats.into_iter().find(|c| c.title == "busy").unwrap()
}

fn save(paths: &Paths, chat: &Chat) {
    JsonStore::new(paths.clone()).save_chat(chat).unwrap();
}

/// `[same, only here, only there, ahead here, ahead there, diverged, details]`.
fn chat_shape(c: &ChatComparison) -> [usize; 7] {
    [
        c.same,
        c.only_here.len(),
        c.only_there.len(),
        c.ahead_here.len(),
        c.ahead_there.len(),
        c.diverged.len(),
        c.details.len(),
    ]
}

/// `[same, only here, only there, newer here, newer there, differing]`.
fn keyed_shape(k: &KeyedComparison) -> [usize; 6] {
    [
        k.same,
        k.only_here.len(),
        k.only_there.len(),
        k.newer_here.len(),
        k.newer_there.len(),
        k.differing.len(),
    ]
}

/// Both sides carry the fixture's one corrupt chat file, and say so.
fn fixture_caveats() -> Vec<Caveat> {
    [Side::Here, Side::There]
        .map(|side| Caveat::UnreadableChats { side, count: 1 })
        .to_vec()
}

#[test]
fn a_copy_is_identical_to_itself_as_a_root_an_archive_and_a_snapshot() {
    let pair = pair();
    let here = collect_root(&pair.here.paths, en()).unwrap();

    let archive = backup(&pair.here, Some(PASSWORD));
    let from_archive = collect_archive(&archive, Some(PASSWORD), en()).unwrap();
    let snapshot = pair.here.dir.path().join("there.json");
    fs::write(&snapshot, render_json(&here)).unwrap();
    let from_snapshot = read_snapshot(&snapshot, en()).unwrap();

    for there in [
        collect_root(&pair.there, en()).unwrap(),
        from_archive,
        from_snapshot,
    ] {
        let c = compare(&here, &there, None);
        assert_eq!(c.verdict, Verdict::Identical);
        assert_eq!(chat_shape(&c.chats), [2, 0, 0, 0, 0, 0, 0]);
        assert_eq!(keyed_shape(&c.notes), [1, 0, 0, 0, 0, 0]);
        assert_eq!(c.caveats, fixture_caveats());
        assert_eq!(c.here.fingerprint, c.there.fingerprint);
    }
}

#[test]
fn a_chat_that_exists_on_one_side_only_is_listed_there() {
    let pair = pair();
    let mut extra = busy(&pair.there);
    extra.id = uuid::Uuid::new_v4();
    extra.title = "only on the laptop".into();
    save(&pair.there, &extra);

    let c = pair.compared();
    assert_eq!(c.verdict, Verdict::ThereHasAll);
    assert_eq!(chat_shape(&c.chats), [2, 0, 1, 0, 0, 0, 0]);
    assert_eq!(c.chats.only_there[0].title, "only on the laptop");
    assert_eq!(c.chats.only_there[0].messages_there, Some(4));
    assert_ne!(c.here.fingerprint, c.there.fingerprint);
}

#[test]
fn a_chat_continued_on_the_other_side_is_ahead_there() {
    let pair = pair();
    let mut chat = busy(&pair.there);
    chat.push_message(Message::user("and then?"));
    chat.push_message(Message::assistant("then this"));
    save(&pair.there, &chat);

    let c = pair.compared();
    assert_eq!(c.verdict, Verdict::ThereHasAll);
    assert_eq!(chat_shape(&c.chats), [1, 0, 0, 0, 1, 0, 0]);
    let diff = &c.chats.ahead_there[0];
    assert_eq!(
        (diff.here, diff.there, diff.later),
        (0, 2, Some(Side::There))
    );
}

/// The case counts and dates get wrong: `Ctrl+R` replaces the reply, so the
/// other side has a message this one lacks **and** lacks one this one has live.
/// The old reply's id lives on in the deleted archive there, so nothing is lost
/// by keeping that copy — *ahead*, not *diverged*.
#[test]
fn a_regenerated_reply_is_ahead_not_diverged() {
    let pair = pair();
    let mut chat = busy(&pair.there);
    let old_reply = chat.messages.pop().unwrap();
    chat.deleted.push(DeletedExchange {
        deleted_at: chrono::Utc::now(),
        messages: vec![old_reply],
        draft: String::new(),
        cause: None,
    });
    chat.push_message(Message::assistant("a better reply"));
    save(&pair.there, &chat);

    let c = pair.compared();
    assert_eq!(chat_shape(&c.chats), [1, 0, 0, 0, 1, 0, 0]);
    assert_eq!(
        (c.chats.ahead_there[0].here, c.chats.ahead_there[0].there),
        (0, 1)
    );
    assert_eq!(c.verdict, Verdict::ThereHasAll);
}

/// One chat continued on two machines from one base. By message count and
/// last-message time this is "newer there"; by ids it is what it is.
#[test]
fn a_chat_continued_on_both_sides_diverged() {
    let pair = pair();
    let mut mine = busy(&pair.here.paths);
    mine.push_message(Message::user("written on the desktop"));
    save(&pair.here.paths, &mine);
    let mut theirs = busy(&pair.there);
    theirs.push_message(Message::user("written on the laptop"));
    theirs.push_message(Message::assistant("and answered there"));
    save(&pair.there, &theirs);

    let c = pair.compared();
    assert_eq!(c.verdict, Verdict::EachHasSomething);
    assert_eq!(chat_shape(&c.chats), [1, 0, 0, 0, 0, 1, 0]);
    assert_eq!(
        (c.chats.diverged[0].here, c.chats.diverged[0].there),
        (1, 2)
    );
}

/// `/continue` appends to a message under its own id: the same ids on both
/// sides, and the longer text is the later one.
#[test]
fn a_message_continued_in_place_counts_for_the_side_that_has_more_of_it() {
    let pair = pair();
    let mut chat = busy(&pair.there);
    chat.messages
        .last_mut()
        .unwrap()
        .text
        .push_str(", continued");
    save(&pair.there, &chat);

    let c = pair.compared();
    assert_eq!(chat_shape(&c.chats), [1, 0, 0, 0, 1, 0, 0]);
    assert_eq!(
        (c.chats.ahead_there[0].here, c.chats.ahead_there[0].there),
        (0, 1)
    );
}

/// One detail at a time, so that each is known to reach both the comparison
/// and the fingerprint on its own: a change made with another would hide
/// behind it.
fn assert_one_detail(change: impl FnOnce(&mut Chat), expected: Detail) {
    let pair = pair();
    let mut chat = busy(&pair.there);
    change(&mut chat);
    save(&pair.there, &chat);

    let c = pair.compared();
    assert_eq!(c.verdict, Verdict::DetailsOnly, "{expected:?}");
    assert_eq!(chat_shape(&c.chats), [1, 0, 0, 0, 0, 0, 1], "{expected:?}");
    let diff = &c.chats.details[0];
    assert_eq!(diff.details, [expected]);
    // None of these moves `modified_at`, and the output must not pretend to know.
    assert_eq!(diff.later, None, "{expected:?}");
    assert_ne!(
        c.here.fingerprint, c.there.fingerprint,
        "{expected:?} is part of the identity"
    );
}

#[test]
fn a_rename_is_a_detail() {
    assert_one_detail(|chat| chat.title = "renamed".into(), Detail::Title);
}

#[test]
fn a_soft_delete_is_a_detail() {
    assert_one_detail(|chat| chat.is_hidden = true, Detail::DeletedMark);
}

#[test]
fn a_removed_attachment_is_a_detail() {
    assert_one_detail(|chat| chat.attachments.truncate(1), Detail::Attachments);
}

#[test]
fn a_removed_stored_file_is_a_detail() {
    assert_one_detail(|chat| chat.files.clear(), Detail::StoredFiles);
}

/// `Ctrl+E` with nothing written after it: the same ids, one of them moved
/// into the deleted archive. No message is lost on either side.
#[test]
fn a_deleted_exchange_is_a_detail_not_a_lost_message() {
    assert_one_detail(
        |chat| {
            let last = chat.messages.pop().unwrap();
            chat.deleted.push(DeletedExchange {
                deleted_at: chrono::Utc::now(),
                messages: vec![last],
                draft: String::new(),
                cause: None,
            });
        },
        Detail::Deletions,
    );
}

#[test]
fn notes_are_lined_up_by_id_and_ordered_by_their_own_time() {
    let pair = pair();
    let storage = Storage::open(pair.there.clone()).unwrap();
    let note = &pair.here.note;
    storage
        .db()
        .note_update(note.id, note.profile_id, "remember, revised")
        .unwrap();
    let added = Note::new(note.profile_id, "only there", Vec::new());
    storage.db().note_insert(&added).unwrap();
    drop(storage);

    let c = pair.compared();
    assert_eq!(keyed_shape(&c.notes), [0, 0, 1, 0, 1, 0]);
    assert_eq!(c.notes.newer_there[0].key, note.id.to_string());
    assert_eq!(c.notes.only_there[0].key, added.id.to_string());
    assert_eq!(c.verdict, Verdict::ThereHasAll);
    let (here, there) = pair.fingerprints();
    assert_ne!(here, there);
}

/// Two versions of one note under one timestamp cannot be ordered, and the
/// verdict must then refuse to say either copy has everything.
#[test]
fn different_content_under_one_time_counts_for_both_sides() {
    let row = |digest: &str| KeyedRow {
        key: "n1".into(),
        at: "2026-03-05T10:20:30+00:00".into(),
        digest: digest.into(),
    };
    let differing = compare_keyed(&[row("aaaa")], &[row("bbbb")]);
    assert_eq!(keyed_shape(&differing), [0, 0, 0, 0, 0, 1]);
    let empty = KeyedComparison::default();
    assert_eq!(
        verdict(&ChatComparison::default(), [&differing, &empty, &empty]),
        Verdict::EachHasSomething
    );
    // The same digest is the same note, whatever the clocks say.
    let same = compare_keyed(
        &[row("aaaa")],
        &[KeyedRow {
            at: "2030-01-01T00:00:00+00:00".into(),
            ..row("aaaa")
        }],
    );
    assert_eq!(keyed_shape(&same), [1, 0, 0, 0, 0, 0]);
}

/// A database that cannot be read is not "no notes": the notes are left
/// uncompared and the text says so **before** the verdict.
#[test]
fn an_unreadable_database_is_a_caveat_not_a_difference() {
    let pair = pair();
    fs::write(pair.there.data_db(), b"this is not sqlite").unwrap();

    let c = pair.compared();
    assert_eq!(keyed_shape(&c.notes), [0; 6]);
    assert!(
        c.caveats.iter().any(|caveat| matches!(
            caveat,
            Caveat::DatabaseNotCompared {
                side: Side::There,
                ..
            }
        )),
        "{:?}",
        c.caveats
    );
    let text = render_comparison_text(&c, en());
    let partial = text.find(en().t("cli.stats.cmp.partial")).unwrap();
    let verdict = text
        .find(en().t("cli.stats.cmp.verdict.identical"))
        .unwrap();
    assert!(partial < verdict, "{text}");
}

fn snapshot_file(dir: &Path, content: &[u8]) -> std::path::PathBuf {
    let path = dir.join("other.json");
    fs::write(&path, content).unwrap();
    path
}

/// Each refusal names the file and the way out — and a format 1 snapshot must
/// be refused rather than read: it would deserialize into empty id lists and
/// compare as "this copy has everything".
#[test]
fn a_snapshot_that_cannot_be_compared_is_refused_with_the_way_out() {
    let dir = tempfile::tempdir().unwrap();
    let refusal = |content: &[u8]| {
        let path = snapshot_file(dir.path(), content);
        read_snapshot(&path, en()).unwrap_err().to_string()
    };
    // Pinned to its own message: "not a snapshot" names the same command, and
    // a test that accepted either passed with the format check removed.
    let old = refusal(br#"{"format":1,"app_version":"0.10.1","chat_list":[]}"#);
    assert!(
        old.contains("other.json") && old.contains("format 1") && old.contains("no message ids"),
        "{old}"
    );
    assert!(old.contains("mindfork stats --json"), "the way out: {old}");
    let new = refusal(br#"{"format":99}"#);
    assert!(new.contains("newer version"), "{new}");
    for junk in [
        &b"not json at all"[..],
        &b"{\"no\":\"format\"}"[..],
        &[0u8, 159, 146, 150][..],
    ] {
        let err = refusal(junk);
        assert!(
            err.contains("neither a backup archive nor a snapshot"),
            "{err}"
        );
    }
}

/// Windows PowerShell's `>` writes UTF-16 with a byte-order mark, and
/// `stats --json > file` is the command the help shows.
#[test]
fn a_snapshot_redirected_by_windows_powershell_is_read() {
    let pair = pair();
    let here = collect_root(&pair.here.paths, en()).unwrap();
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(render_json(&here).encode_utf16().flat_map(u16::to_le_bytes));
    let path = snapshot_file(pair.here.dir.path(), &utf16);

    let there = read_snapshot(&path, en()).unwrap();
    assert_eq!(there.chat_list, here.chat_list);
    assert_eq!(compare(&here, &there, None).verdict, Verdict::Identical);
}

#[test]
fn a_zip_is_read_as_an_archive_and_anything_else_as_a_snapshot() {
    let pair = pair();
    let archive = backup(&pair.here, None);
    assert_eq!(other_copy(&archive), OtherCopy::Archive);
    let snapshot = snapshot_file(pair.here.dir.path(), b"{}");
    assert_eq!(other_copy(&snapshot), OtherCopy::Snapshot);
    // The name decides nothing: an archive called `.json` is still an archive.
    let disguised = pair.here.dir.path().join("really-a-zip.json");
    fs::copy(&archive, &disguised).unwrap();
    assert_eq!(other_copy(&disguised), OtherCopy::Archive);
}

fn other_copy(path: &Path) -> OtherCopy {
    super::super::other_copy(path, en()).unwrap()
}

#[test]
fn the_text_gives_the_verdict_the_counts_and_the_lists_behind_them() {
    let pair = pair();
    let mut mine = busy(&pair.here.paths);
    mine.push_message(Message::user("written on the desktop"));
    save(&pair.here.paths, &mine);
    let mut extra = busy(&pair.there);
    extra.id = uuid::Uuid::new_v4();
    extra.title = "only on the laptop".into();
    extra.is_hidden = true;
    save(&pair.there, &extra);

    let c = compare(
        &collect_root(&pair.here.paths, en()).unwrap(),
        &collect_root(&pair.there, en()).unwrap(),
        Some("laptop.json".into()),
    );
    let text = render_comparison_text(&c, en());
    for expected in [
        "Snapshot laptop.json, taken ",
        en().t("cli.stats.cmp.verdict.each_has_something"),
        "Chats: same: 1; only here: 0; only there: 1; more here: 1; more there: 0; diverged: 0; details differ: 0",
        "Notes: same: 1; only here: 0; only there: 0; newer here: 0; newer there: 0; differing: 0",
        "Chats, only there (1):",
        "messages: 4, deleted chat  only on the laptop",
        "Chats, more here (1):",
        "messages only here: 1; only there: 0  busy",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(
        !text.contains("Chats, diverged"),
        "an empty list has no heading:\n{text}"
    );
    assert!(text.contains(&extra.id.to_string()), "{text}");

    let json: serde_json::Value = serde_json::from_str(&render_comparison_json(&c)).unwrap();
    assert_eq!(json["verdict"], "each_has_something");
    assert_eq!(
        json["chats"]["only_there"][0]["title"],
        "only on the laptop"
    );
}
