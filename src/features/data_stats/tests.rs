//! Tests of `mindfork stats` (docs/history/data-stats.md §4.4). The data root under
//! test is written by the **real** writers — `JsonStore::save_chat`, the `Db`
//! API, `create_backup` — so the projection is exercised against the shape the
//! app leaves on disk, not against a fixture that agrees with it by design.

use std::fs;
use std::path::PathBuf;

use chrono::TimeZone;

use super::*;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::entities::chat::{Chat, DeletedExchange, visible_message_count};
use crate::entities::chat_file::{ChatFile, FileOrigin};
use crate::entities::message::Message;
use crate::entities::message_image::MessageImage;
use crate::entities::note::Note;
use crate::entities::profile::Profile;
use crate::entities::subagent::SubagentRun;
use crate::entities::workspace::Workspace;
use crate::shared::i18n::{self, Lang};
use crate::shared::storage::Storage;

pub(super) const PASSWORD: &str = "correct horse";

pub(super) fn en() -> &'static Locale {
    i18n::locale(Lang::En)
}

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, day, hour, 0, 0).unwrap()
}

fn message(mut m: Message, when: DateTime<Utc>) -> Message {
    m.timestamp = when;
    m
}

/// The chat that holds one of everything the summary counts.
///
/// Rows: user · assistant (a tool call carrying a sub-agent run of three
/// messages) · tool · assistant (stitched onto the round before it) · user
/// with an image · assistant in a bubble of its own. Six rows, **four**
/// bubbles, with the tool row and the stitched round being the difference.
fn busy_chat(profile: &Profile) -> Chat {
    let mut chat = Chat::from_profile(profile, "busy");
    let mut round = message(Message::assistant("calling"), at(2, 10));
    round
        .tool_calls
        .push(SubagentRun::fixture("helper", &["task", "answer", "thanks"]).on_record());
    let mut own_bubble = message(Message::assistant("one more thing"), at(4, 9));
    own_bubble.new_bubble = true;
    let image = MessageImage::new("pixel.png", "pixel.png", "image/png", 1, 1, "AAAA".into());
    chat.messages = vec![
        message(Message::user("question"), at(2, 9)),
        round,
        message(Message::new(MessageRole::Tool, "result"), at(2, 11)),
        message(Message::assistant("answer"), at(2, 12)),
        message(Message::user("look").with_images(vec![image]), at(4, 8)),
        own_bubble,
    ];
    chat.attachments = vec![
        Attachment::new("a.txt", "a.txt", "alpha".into(), 5, AttachMode::Inline),
        Attachment::new("b.txt", "b.txt", "beta".into(), 4, AttachMode::Inline),
    ];
    chat.files = vec![ChatFile::new("plot.png", FileOrigin::Sandbox, b"png")];
    chat.deleted = vec![DeletedExchange {
        deleted_at: at(3, 0),
        messages: vec![Message::user("never mind"), Message::assistant("fine")],
        draft: String::new(),
        cause: None,
    }];
    chat.workspace = Some(Workspace::new("/code/project"));
    chat.modified_at = at(4, 9);
    chat
}

/// A soft-deleted chat on the **same** project, changed later than any message.
fn hidden_chat(profile: &Profile) -> Chat {
    let mut chat = Chat::from_profile(profile, "hidden");
    chat.messages = vec![
        message(Message::user("old"), at(1, 9)),
        message(Message::assistant("older"), at(1, 10)),
    ];
    chat.workspace = Some(Workspace::new("/code/project"));
    chat.is_hidden = true;
    chat.modified_at = at(5, 0);
    chat
}

pub(super) struct Root {
    pub(super) dir: tempfile::TempDir,
    pub(super) paths: Paths,
    pub(super) note: Note,
}

/// A data root the app's own writers produced: two profiles (one
/// soft-deleted), the two chats above, a corrupt chat file, two files that are
/// not chats, and a database holding one note.
pub(super) fn root() -> Root {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(dir.path().join("data"));
    fs::create_dir_all(paths.chats_dir()).unwrap();
    let storage = Storage::open(paths.clone()).unwrap();

    let profile = Profile::new("main", "system");
    let mut gone = Profile::new("gone", "system");
    gone.is_hidden = true;
    storage
        .json()
        .save_profiles(&[profile.clone(), gone])
        .unwrap();
    storage.json().save_chat(&busy_chat(&profile)).unwrap();
    storage.json().save_chat(&hidden_chat(&profile)).unwrap();

    let chats = paths.chats_dir();
    fs::write(chats.join(format!("{}.json", Uuid::nil())), b"{ not json").unwrap();
    fs::write(chats.join("notes-to-self.json"), b"{}").unwrap();
    fs::write(chats.join(format!("{}.bak", Uuid::new_v4())), b"{}").unwrap();

    let note = Note::new(profile.id, "remember", Vec::new());
    storage.db().note_insert(&note).unwrap();
    Root { dir, paths, note }
}

fn expected_chats() -> ChatTotals {
    ChatTotals {
        total: 2,
        deleted: 1,
        unreadable: vec![format!("{}.json", Uuid::nil())],
        messages: 6,
        messages_in_deleted_chats: 2,
        message_rows: 8,
        deleted_messages: 2,
        deleted_exchanges: 1,
        attachments: 2,
        stored_files: 1,
        images: 1,
        projects: 1,
        chats_with_project: 2,
        subagent_runs: 1,
        subagent_messages: 3,
        last_message_at: Some(at(4, 9)),
        last_change_at: Some(at(5, 0)),
        newest_schema: CHAT_SCHEMA,
    }
}

/// The database half: the one note, counted and listed under its own id.
fn assert_database(database: &DatabaseStats, root: &Root) {
    let DatabaseStats::Ok(db) = database else {
        panic!("the database was not read: {database:?}");
    };
    assert_eq!((db.notes, db.notes_superseded, db.note_links), (1, 0, 0));
    assert_eq!(db.last_note_change, Some(root.note.updated_at));
    let keys: Vec<&str> = db.note_list.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(keys, [root.note.id.to_string()]);
    assert!(db.source_list.is_empty() && db.self_model_list.is_empty());
}

/// Every file under `dir`, with its size and content hash stand-in (the bytes
/// themselves) — "untouched" means this is equal before and after.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.push((path.clone(), Vec::new()));
                pending.push(path);
            } else {
                out.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_live_root_is_counted_and_left_exactly_as_it_was() {
    let root = root();
    let before = snapshot(root.dir.path());
    let stats = collect_root(&root.paths, en()).unwrap();

    assert_eq!(stats.chats, expected_chats());
    assert_eq!(
        stats.profiles,
        ProfileTotals {
            total: 2,
            deleted: 1
        }
    );
    assert_database(&stats.database, &root);
    assert!(stats.sizes.chats_bytes > 0 && stats.sizes.database_bytes > 0);
    assert_eq!(snapshot(root.dir.path()), before);
}

/// The projection restates no rule: for the chat the app wrote, it has to land
/// on the number the chat list's card shows.
#[test]
fn the_projection_counts_what_the_chat_list_counts() {
    let chat = busy_chat(&Profile::new("main", "system"));
    let shown = visible_message_count(&chat.messages);
    assert_eq!(
        shown, 4,
        "the fixture must exercise stitching and the tool row"
    );

    let mut tally = Tally::default();
    tally.add("x.json", chat.id, &serde_json::to_vec(&chat).unwrap());
    let (totals, rows, _) = tally.finish();
    assert_eq!(totals.messages, shown);
    assert_eq!(
        (
            rows[0].messages,
            rows[0].message_rows,
            rows[0].deleted_messages
        ),
        (shown, chat.messages.len(), 2)
    );
}

#[test]
fn a_root_that_does_not_exist_is_an_empty_summary_and_stays_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(dir.path().join("never-created"));
    let stats = collect_root(&paths, en()).unwrap();
    assert!(stats.is_empty());
    assert!(!paths.root().exists());
    assert!(render_text(&stats, en()).contains(en().t("cli.stats.empty")));
}

/// A file written before any of the additive fields existed, and one from a
/// future schema with a role this binary has never heard of: both count.
#[test]
fn files_from_other_schema_versions_are_still_counted() {
    const OLD: &str = r#"{"id":"x","title":"old","messages":[
        {"role":"user","text":"q"},{"role":"assistant","text":"a"}]}"#;
    const NEW: &str = r#"{"v":99,"messages":[
        {"role":"user","timestamp":"2030-01-01T00:00:00Z","shiny":{"deep":[1,2]}},
        {"role":"narrator"},{"role":"assistant"}],"workspace":null,"extra":true}"#;
    let mut tally = Tally::default();
    tally.add("old.json", Uuid::new_v4(), OLD.as_bytes());
    tally.add("new.json", Uuid::new_v4(), NEW.as_bytes());
    let (totals, _, _) = tally.finish();

    assert_eq!(totals.unreadable, Vec::<String>::new());
    assert_eq!((totals.total, totals.messages), (2, 5));
    assert_eq!(totals.newest_schema, 99);
    assert_eq!(
        totals.last_message_at,
        Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap())
    );
}

pub(super) fn backup(root: &Root, password: Option<&str>) -> PathBuf {
    let out = root.dir.path().join("copy.zip");
    backup::create_backup(&root.paths, Some(out), 1, None, password, en(), |_| {}).unwrap()
}

/// The archive is summarized to the same numbers as the root it was made
/// from — plain and encrypted — and nothing appears next to it on disk.
#[test]
fn an_archive_counts_the_same_as_the_root_it_was_made_from() {
    let root = root();
    let live = collect_root(&root.paths, en()).unwrap();
    for password in [None, Some(PASSWORD)] {
        let archive = backup(&root, password);
        let before = snapshot(root.dir.path());
        let stats = collect_archive(&archive, password, en()).unwrap();

        assert_eq!(stats.chats, expected_chats());
        assert_eq!(stats.chat_list, live.chat_list);
        assert_eq!(stats.profiles, live.profiles);
        assert_database(&stats.database, &root);
        assert_eq!(stats.sizes.chats_bytes, live.sizes.chats_bytes);
        assert!(matches!(
            &stats.source,
            Source::Archive { manifest: Some(m), newer_than_this_app: false, .. }
                if m.app_version == env!("CARGO_PKG_VERSION")
        ));
        assert_eq!(snapshot(root.dir.path()), before);
        fs::remove_file(archive).unwrap();
    }
}

#[test]
fn an_encrypted_archive_refuses_a_missing_or_wrong_password_in_the_restore_words() {
    let root = root();
    let archive = backup(&root, Some(PASSWORD));
    let refusal = |password| {
        collect_archive(&archive, password, en())
            .unwrap_err()
            .to_string()
    };
    assert_eq!(refusal(None), en().t("backup.err.password_required"));
    assert_eq!(refusal(Some("")), en().t("backup.err.password_required"));
    assert_eq!(refusal(Some("wrong")), en().t("backup.err.wrong_password"));
}

#[test]
fn a_zip_that_is_not_a_backup_is_refused_rather_than_summarized_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holiday.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    zip.start_file("photos/beach.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut zip, b"sand").unwrap();
    zip.finish().unwrap();

    let err = collect_archive(&path, None, en()).unwrap_err().to_string();
    assert!(err.contains("holiday.zip"), "{err}");
    assert_eq!(
        err,
        en().tf(
            "cli.stats.err.not_a_backup",
            &[("path", &path.display().to_string())]
        )
    );
}

/// A database that cannot be read costs the database rows and nothing else.
#[test]
fn an_unreadable_database_leaves_the_chat_half_standing() {
    let root = root();
    fs::write(root.paths.data_db(), b"this is not sqlite").unwrap();
    let stats = collect_root(&root.paths, en()).unwrap();
    assert_eq!(stats.chats, expected_chats());
    assert!(matches!(stats.database, DatabaseStats::Unreadable { .. }));

    let text = render_text(&stats, en());
    assert!(text.contains(en().t("cli.stats.label.database")), "{text}");
    assert!(!text.contains(en().t("cli.stats.label.notes")), "{text}");
}

fn rendered(lang: Lang) -> String {
    let root = root();
    let stats = collect_root(&root.paths, i18n::locale(lang)).unwrap();
    render_text(&stats, i18n::locale(lang))
}

#[test]
fn the_text_names_every_figure_and_the_file_it_could_not_read() {
    let text = rendered(Lang::En);
    for line in [
        "Last message:      2026-03-04 09:00:00 UTC",
        "Last change:       2026-03-05 00:00:00 UTC",
        "Profiles:          2 (deleted: 1)",
        "Chats:             2 (deleted: 1)",
        "Messages:          6 (in deleted chats: 2; stored rows: 8)",
        "Deleted messages:  2 (exchanges: 1)",
        "Attached files:    2",
        "Stored files:      1",
        "Images:            1",
        "Projects:          1 (chats: 2)",
        "Sub-agent runs:    1 (messages: 3)",
        "Notes:             1 (superseded: 0; links: 0)",
        "Knowledge base:    sources: 0; chunks: 0",
        "Self-models:       0",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    assert!(text.contains(&format!("{}.json", Uuid::nil())), "{text}");
}

/// Labels share one column, so their width is a budget only one locale finds
/// out about (docs/lessons.md §7): every bundled locale must come out aligned.
#[test]
fn the_value_column_is_aligned_in_every_bundled_locale() {
    for lang in [Lang::En, Lang::Ru] {
        let text = rendered(lang);
        let columns: BTreeSet<usize> = text
            .lines()
            .skip_while(|line| !line.is_empty())
            .skip(1)
            .take_while(|line| !line.is_empty())
            .map(|line| {
                let (label, rest) = line.split_once(':').unwrap();
                label.width() + 1 + rest.width() - rest.trim_start().width()
            })
            .collect();
        assert_eq!(columns.len(), 1, "{lang:?} is ragged: {columns:?}\n{text}");
    }
}

#[test]
fn the_json_form_is_versioned_sorted_and_carries_a_row_per_chat() {
    let root = root();
    let stats = collect_root(&root.paths, en()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&render_json(&stats)).unwrap();

    assert_eq!(json["format"], SNAPSHOT_FORMAT);
    assert_eq!(json["source"]["kind"], "data_root");
    assert_eq!(json["chats"]["messages"], 6);
    assert_eq!(json["database"]["status"], "ok");
    assert_eq!(json["database"]["notes"], 1);
    let ids: Vec<&str> = json["chat_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.is_sorted(), "{ids:?}");
    let titles: BTreeSet<&str> = json["chat_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, BTreeSet::from(["busy", "hidden"]));
}

/// An archive from a newer version is summarized, and says what that means.
#[test]
fn an_archive_from_a_newer_version_is_counted_and_flagged() {
    let root = root();
    let plain = backup(&root, None);
    let newer = root.dir.path().join("newer.zip");
    let mut from = zip::ZipArchive::new(fs::File::open(&plain).unwrap()).unwrap();
    let mut to = zip::ZipWriter::new(fs::File::create(&newer).unwrap());
    for i in 0..from.len() {
        let entry = from.by_index_raw(i).unwrap();
        if entry.name() != "manifest.json" {
            to.raw_copy_file(entry).unwrap();
        }
    }
    to.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    let manifest = r#"{"app_version":"99.0.0","created_at":"2031-01-01T03:00:00+03:00",
        "schemas":{"settings":99,"profiles":99,"chat":99,"db":99}}"#;
    std::io::Write::write_all(&mut to, manifest.as_bytes()).unwrap();
    to.finish().unwrap();

    let stats = collect_archive(&newer, None, en()).unwrap();
    assert_eq!(stats.chats, expected_chats());
    let text = render_text(&stats, en());
    assert!(text.contains(en().t("cli.stats.newer_data")), "{text}");
    assert!(text.contains("2031-01-01 00:00:00 UTC"), "{text}");
    assert!(text.contains("99.0.0"), "{text}");
}
