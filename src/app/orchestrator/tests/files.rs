//! Orchestrator tests — stored files, the chat's side of the sandbox file exchange
//! (docs/history/sandbox-file-exchange.md §11 S5–S8, S11): a tool's `AddChatFile` lands once and is
//! mirrored into the turn, `/file list` and `/file remove` reach stored files, a copy that
//! cannot be deleted stays listed, the bootstrap adopts what a chat does not list, and an
//! image the model cannot take is withheld with a note, and `/file open`/`/file folder`
//! plan what reaches the shell (§13). Part of the [`super`] module
//! (fixtures in mod.rs; the scripted engine in subagent.rs).

use std::time::Duration;

use super::subagent::{Script, ScriptRecorder, load, text};
use super::*;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::entities::chat_file::{ChatFile, FileOrigin};
use crate::entities::profile::ToolId;
use crate::features::file_command::{FileProgress, OpenedInstead};
use crate::features::tools::meta::ToolGroup;
use crate::features::tools::{ChatEffect, Tool, ToolContext, ToolImage, ToolOutcome};
use crate::shared::api::VisionSupport;
use crate::shared::api::contract::{ChatStream, FinishReason, ToolCallDelta};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot-really-pixels";

fn listing(name: &str) -> ChatFile {
    ChatFile::new(name, FileOrigin::Sandbox, PNG)
}

/// A bare orchestrator's one open chat; returns its id.
fn open_chat(orch: &mut Orchestrator) -> Uuid {
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "t");
    let id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(id);
    id
}

fn files_of(orch: &Orchestrator, chat_id: Uuid) -> Vec<String> {
    orch.chats
        .iter()
        .find(|c| c.id == chat_id)
        .expect("the chat")
        .files
        .iter()
        .map(|f| f.name.clone())
        .collect()
}

/// What the bare orchestrator has emitted so far.
fn drain(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<AppEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn saved_notes(events: &[AppEvent]) -> Vec<Vec<String>> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::FileProgress(FileProgress::Saved { names, .. }) => Some(names.clone()),
            _ => None,
        })
        .collect()
}

fn failure(events: Vec<AppEvent>) -> Option<String> {
    events.into_iter().find_map(|e| match e {
        AppEvent::FileProgress(FileProgress::Failed(msg)) => Some(msg),
        _ => None,
    })
}

#[test]
fn a_stored_file_lands_once_and_one_note_names_it() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    orch.list_stored_files(chat_id, vec![listing("chart.png"), listing("totals.csv")]);
    // The same landing again — a repeated one — and a name that differs only in case.
    orch.list_stored_files(chat_id, vec![listing("chart.png"), listing("CHART.png")]);
    assert_eq!(files_of(&orch, chat_id), ["chart.png", "totals.csv"]);
    assert_eq!(
        saved_notes(&drain(&mut rx)),
        [vec!["chart.png".to_string(), "totals.csv".to_string()]]
    );
}

#[test]
fn a_landing_in_a_chat_that_is_not_open_lists_without_a_note() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    orch.active_id = None;
    orch.list_stored_files(chat_id, vec![listing("chart.png")]);
    assert_eq!(files_of(&orch, chat_id), ["chart.png"]);
    assert!(saved_notes(&drain(&mut rx)).is_empty());
}

/// The reply `/file list` sent, or nothing.
fn listed(events: Vec<AppEvent>) -> Option<(Vec<String>, Vec<String>, Vec<String>)> {
    events.into_iter().find_map(|e| match e {
        AppEvent::FileProgress(FileProgress::Listed {
            items,
            stored,
            images,
            ..
        }) => Some((
            items.iter().map(|a| a.name.clone()).collect(),
            stored.iter().map(|f| f.name.clone()).collect(),
            images.iter().map(|i| i.name.clone()).collect(),
        )),
        _ => None,
    })
}

/// One numbered list of the chat's three kinds (docs/history/sandbox-file-exchange.md §12 T2, T4):
/// attachments, then the stored files no attachment links, then the images its messages
/// carry — and a document that kept its original is **one** item, shown on its attachment's
/// line rather than twice (§12 T9).
#[test]
fn file_list_numbers_attachments_stored_files_and_images_as_one_list() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let original = ChatFile::new("report.pdf", FileOrigin::Attached, b"%PDF-1.7\n");
    let linked = Attachment::new(
        "report.pdf",
        "C:\\report.pdf",
        "the extracted text".into(),
        9,
        AttachMode::ByReference,
    )
    .with_file(original.id);
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    chat.attachments.push(linked);
    chat.files.push(original);
    chat.files.push(listing("chart.png"));
    let mut message = Message::user("look at this");
    message
        .images
        .push(crate::entities::message_image::MessageImage::new(
            "shot.png",
            "C:\\shot.png",
            "image/png",
            10,
            10,
            "AAAA".into(),
        ));
    chat.messages.push(message);

    orch.handle_file_list();
    let (items, stored, images) = listed(drain(&mut rx)).expect("a /file list reply");
    assert_eq!(items, ["report.pdf"]);
    // Not `report.pdf` again: the original is the attachment's own half.
    assert_eq!(stored, ["chart.png"]);
    assert_eq!(images, ["shot.png"]);
}

/// An image belongs to the message that carries it, so `/file remove` refuses it — and
/// names the command that *is* about images, rather than only saying no (lessons §4).
#[test]
fn removing_an_image_is_refused_with_the_way_out() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    let mut message = Message::user("look");
    message
        .images
        .push(crate::entities::message_image::MessageImage::new(
            "shot.png",
            "C:\\shot.png",
            "image/png",
            10,
            10,
            "AAAA".into(),
        ));
    chat.messages.push(message);

    orch.handle_file_remove("shot.png".into());
    let msg = failure(drain(&mut rx)).expect("a refusal");
    assert!(msg.contains("shot.png"), "{msg}");
    assert!(msg.contains("/image remove"), "{msg}");
}

/// Fork F9 (§13 U3): a type the shell may run is never handed to a handler — the folder
/// opens instead, and the note says which happened. The plan is asserted rather than the
/// launch: nothing opens a window on the machine running the tests (§13 U10).
#[test]
fn a_document_opens_and_a_script_the_call_wrote_opens_its_folder() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("chart.png"), PNG).unwrap();
    std::fs::write(dir.join("run.bat"), b"echo hi").unwrap();
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    chat.files.push(listing("chart.png"));
    chat.files
        .push(ChatFile::new("run.bat", FileOrigin::Sandbox, b"echo hi"));

    let (path, note) = orch.plan_open("#1").expect("the chart opens");
    assert_eq!(path, dir.join("chart.png"));
    assert!(
        matches!(&note, FileProgress::Opened { name, .. } if name == "chart.png"),
        "{note:?}"
    );

    let (path, note) = orch.plan_open("run.bat").expect("the folder opens instead");
    assert_eq!(path, dir, "a script must not reach a handler");
    assert!(
        matches!(
            &note,
            FileProgress::OpenedFolder {
                instead_of: Some(OpenedInstead { name, by_a_call: true }),
                ..
            } if name == "run.bat"
        ),
        "{note:?}"
    );
    assert_eq!(failure(drain(&mut rx)), None, "neither is a refusal");
}

/// The folder stands in for a refused type whoever wrote the file — the allowlist is by
/// type — but the reason the note gives is true only of a file a call wrote. A `.py` the
/// user attached and a binary `/file attach` kept are the user's own; a call's script and
/// a leftover adopted from the folder are a call's (§13 U3).
#[test]
fn the_reason_a_folder_opens_instead_names_whose_file_it_is() {
    let (root, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["setup.msi", "run.cmd", "old.sh"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    // The user's own script, attached from where it lives: plain text keeps no copy.
    let own = root.path().join("script.py");
    std::fs::write(&own, b"print(1)").unwrap();
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    chat.attachments.push(Attachment::new(
        "script.py",
        own.display().to_string(),
        "print(1)".into(),
        3,
        AttachMode::Inline,
    ));
    chat.files
        .push(ChatFile::new("setup.msi", FileOrigin::Attached, b"x"));
    chat.files
        .push(ChatFile::new("run.cmd", FileOrigin::Sandbox, b"x"));
    chat.files
        .push(ChatFile::new("old.sh", FileOrigin::Recovered, b"x"));

    let by_a_call = |target: &str| match orch.plan_open(target) {
        Some((
            _,
            FileProgress::OpenedFolder {
                instead_of: Some(instead),
                ..
            },
        )) => instead.by_a_call,
        other => panic!("{target}: expected its folder instead, got {other:?}"),
    };
    assert!(!by_a_call("script.py"), "the user attached it");
    assert!(
        !by_a_call("setup.msi"),
        "the user's binary, kept by /file attach"
    );
    assert!(by_a_call("run.cmd"), "a call wrote it");
    assert!(
        by_a_call("old.sh"),
        "adopted from the folder as a call's leftover"
    );
    assert_eq!(failure(drain(&mut rx)), None, "none of them is a refusal");
}

/// A stored name comes back from `chat.json` unvalidated. One that leaves the folder is
/// refused before anything is handed over — and the file it points at is **real**, so
/// the existence check alone would have let it through to a handler.
#[test]
fn a_stored_name_that_leaves_the_folder_is_refused_even_when_the_file_is_real() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    // Two levels above `files/<chat-id>/`: outside every chat's folder.
    std::fs::write(dir.join("..").join("..").join("escape.pdf"), b"%PDF").unwrap();
    // As `chat.json` would hand it back, not as `ChatFile::new` would build it.
    let mut file = ChatFile::new("escape.pdf", FileOrigin::Sandbox, b"%PDF");
    file.name = "../../escape.pdf".into();
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    chat.files.push(file);

    assert!(
        orch.plan_open("#1").is_none(),
        "nothing outside the folder is opened"
    );
    let msg = failure(drain(&mut rx)).expect("a refusal");
    assert!(msg.contains("escape.pdf"), "{msg}");
}

/// A launch runs off the loop and can outlast a switch of chats (§13 U5). Its success is
/// noted only in the chat it was asked in; its failure wherever the user is, because that
/// note is the only report of a command just given and a note is kept with no chat.
#[test]
fn a_launch_s_note_goes_to_its_chat_and_a_failure_is_never_lost() {
    use crate::app::orchestrator::attachments::OpenResult;
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let asked_in = open_chat(&mut orch);
    let opened = || FileProgress::Opened {
        name: "report.pdf".into(),
        path: "/data/files/a/report.pdf".into(),
    };
    let notes = |rx: &mut UnboundedReceiver<AppEvent>| {
        drain(rx)
            .iter()
            .filter(|e| matches!(e, AppEvent::FileProgress(_)))
            .count()
    };

    // Still in the chat: the note lands.
    orch.handle_open_result(OpenResult {
        chat_id: asked_in,
        progress: opened(),
    });
    assert_eq!(notes(&mut rx), 1);

    // The user moved on before the launch came back.
    let elsewhere = open_chat(&mut orch);
    assert_ne!(elsewhere, asked_in);
    orch.handle_open_result(OpenResult {
        chat_id: asked_in,
        progress: opened(),
    });
    assert_eq!(
        notes(&mut rx),
        0,
        "another chat's feed does not say this chat's file opened"
    );

    let why = "could not open /data/files/a/report.pdf";
    orch.handle_open_result(OpenResult {
        chat_id: asked_in,
        progress: FileProgress::Failed(why.into()),
    });
    assert_eq!(
        failure(drain(&mut rx)).as_deref(),
        Some(why),
        "a failure still reaches the user"
    );
}

/// The three shapes of "there is no file to open" — a pasted image, a handle nothing
/// answers to, and a listed copy the folder no longer holds — refuse before the shell is
/// reached, each naming what it looked for (§13 U2).
#[test]
fn opening_refuses_what_is_not_a_file_on_this_machine() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    // Listed, but its copy never reached the folder.
    chat.files.push(listing("chart.png"));
    let mut message = Message::user("look");
    message
        .images
        .push(crate::entities::message_image::MessageImage::new(
            "clipboard.png",
            "clipboard:9f2c",
            "image/png",
            10,
            10,
            "AAAA".into(),
        ));
    chat.messages.push(message);

    assert!(orch.plan_open("#2").is_none(), "a paste has no file");
    let msg = failure(drain(&mut rx)).expect("a refusal");
    assert!(
        msg.contains("clipboard.png") && msg.contains("clipboard:9f2c"),
        "{msg}"
    );

    assert!(orch.plan_open("#1").is_none(), "the copy is gone");
    let msg = failure(drain(&mut rx)).expect("a refusal");
    assert!(msg.contains("chart.png"), "{msg}");

    assert!(orch.plan_open("nothing.txt").is_none());
    let msg = failure(drain(&mut rx)).expect("a refusal");
    assert!(msg.contains("nothing.txt"), "{msg}");
}

/// A name two of the chat's files share opens nothing and lists each holder's `#N` and
/// source — the rule `/file remove` already had, now shared by both commands (§13 U1).
#[test]
fn opening_a_shared_name_is_refused_with_both_candidates() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    for source in ["C:\\a\\notes.md", "C:\\b\\notes.md"] {
        chat.attachments.push(Attachment::new(
            "notes.md",
            source,
            "text".into(),
            4,
            AttachMode::Inline,
        ));
    }

    assert!(orch.plan_open("notes.md").is_none());
    let msg = failure(drain(&mut rx)).expect("a refusal");
    assert!(msg.contains("#1") && msg.contains("#2"), "{msg}");
    assert!(
        msg.contains("C:\\a\\notes.md") && msg.contains("C:\\b\\notes.md"),
        "{msg}"
    );
}

/// `/file folder` on a chat that has saved nothing says so and prints the path — and
/// creates no empty directory on the way (§13 U7).
#[test]
fn the_folder_of_a_chat_that_saved_nothing_is_refused_with_its_path() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);

    orch.handle_file_folder();
    let msg = failure(drain(&mut rx)).expect("a refusal");
    assert!(msg.contains(&dir.display().to_string()), "{msg}");
    assert!(
        !dir.exists(),
        "the command created the folder it reported on"
    );
}

/// Removing an attached document that kept its original takes both halves — the listing
/// and our copy of the file — and never the user's own (§12 T9).
#[test]
fn removing_a_pair_deletes_our_copy_and_both_listings() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("report.pdf"), b"%PDF-1.7\n").unwrap();
    let original = ChatFile::new("report.pdf", FileOrigin::Attached, b"%PDF-1.7\n");
    let linked = Attachment::new(
        "report.pdf",
        "C:\\report.pdf",
        "the extracted text".into(),
        9,
        AttachMode::ByReference,
    )
    .with_file(original.id);
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    chat.attachments.push(linked);
    chat.files.push(original);

    orch.handle_file_remove("report.pdf".into());
    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    assert!(chat.attachments.is_empty(), "the attachment stayed");
    assert!(chat.files.is_empty(), "the listing stayed");
    assert!(!dir.join("report.pdf").exists(), "our copy stayed on disk");
    let note = drain(&mut rx).into_iter().find_map(|e| match e {
        AppEvent::FileProgress(FileProgress::RemovedPair { name }) => Some(name),
        _ => None,
    });
    assert_eq!(note.as_deref(), Some("report.pdf"));
}

/// Fork F8a (§12 T9): `/file attach` on a binary keeps the file with the chat and makes no
/// attachment of it — there is no text to attach. The note says so, and `/file list`
/// numbers the file like any other stored one. This is the refusal D3 asked to lift.
#[test]
fn attaching_a_binary_keeps_the_file_and_makes_no_attachment() {
    use crate::app::orchestrator::attachments::{AttachResult, ExtractedFile};
    const WORKBOOK: &[u8] = b"PK\x03\x04not-really-a-workbook";
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    orch.handle_attach_result(AttachResult {
        chat_id,
        outcome: Ok(ExtractedFile {
            name: "sales.xlsx".into(),
            source: "C:\\sales.xlsx".into(),
            text: String::new(),
            bytes: WORKBOOK.len(),
            encoding: None,
            original: Some(WORKBOOK.to_vec()),
        }),
    });
    assert_eq!(files_of(&orch, chat_id), ["sales.xlsx"]);
    let dir = orch.stored_files_dir(chat_id);
    assert_eq!(std::fs::read(dir.join("sales.xlsx")).unwrap(), WORKBOOK);
    assert!(
        orch.chats
            .iter()
            .find(|c| c.id == chat_id)
            .unwrap()
            .attachments
            .is_empty(),
        "a binary carries no text, so nothing is attached as text"
    );
    let note = drain(&mut rx).into_iter().find_map(|e| match e {
        AppEvent::FileProgress(FileProgress::StoredFile { name, .. }) => Some(name),
        _ => None,
    });
    assert_eq!(note.as_deref(), Some("sales.xlsx"));
}

/// The remedy the app itself prescribes has to work. A stored copy can go missing while
/// its listing stands — a pruned `data/files/`, a partial sync, a chat file restored
/// without its folder — `/file list` marks it, and `python_exec` refuses the file and tells
/// the model to *ask the user to attach it again*. Matching on the listing alone made that
/// a dead end: name and digest agreed, so re-attaching wrote nothing, the entry stayed
/// missing and the next call refused identically.
#[test]
fn reattaching_a_file_whose_copy_went_missing_puts_it_back() {
    use crate::app::orchestrator::attachments::{AttachResult, ExtractedFile};
    const WORKBOOK: &[u8] = b"PK\x03\x04not-really-a-workbook";
    let (_dir, mut orch, _rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let attach = || AttachResult {
        chat_id,
        outcome: Ok(ExtractedFile {
            name: "sales.xlsx".into(),
            source: "C:\\sales.xlsx".into(),
            text: String::new(),
            bytes: WORKBOOK.len(),
            encoding: None,
            original: Some(WORKBOOK.to_vec()),
        }),
    };

    orch.handle_attach_result(attach());
    let dir = orch.stored_files_dir(chat_id);
    let copy = dir.join("sales.xlsx");
    assert_eq!(std::fs::read(&copy).unwrap(), WORKBOOK);
    let listed_before = files_of(&orch, chat_id);

    // The copy goes; the chat goes on listing it.
    std::fs::remove_file(&copy).unwrap();
    assert!(!crate::features::chat_files::exists(&dir, "sales.xlsx"));

    // Attaching the very same file again is what the refusal tells the user to do.
    orch.handle_attach_result(attach());
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        WORKBOOK,
        "the copy was not put back"
    );
    assert_eq!(
        files_of(&orch, chat_id),
        listed_before,
        "one listing, not a second one beside it"
    );
}

/// A document an extractor read becomes the attachment **and** keeps its original, the two
/// linked as one item (§12 T9) — which is what lets `python_exec` open the file itself
/// while the model reads the text.
#[test]
fn attaching_a_document_links_its_original_to_the_attachment() {
    use crate::app::orchestrator::attachments::{AttachResult, ExtractedFile};
    let (_dir, mut orch, _rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    orch.handle_attach_result(AttachResult {
        chat_id,
        outcome: Ok(ExtractedFile {
            name: "report.pdf".into(),
            source: "C:\\report.pdf".into(),
            text: "the extracted text".into(),
            bytes: 9,
            encoding: None,
            original: Some(b"%PDF-1.7\n".to_vec()),
        }),
    });
    let chat = orch.chats.iter().find(|c| c.id == chat_id).unwrap();
    assert_eq!(chat.attachments.len(), 1);
    let linked = chat.attachments[0].file_id.expect("the pair is linked");
    assert!(
        chat.files
            .iter()
            .any(|f| f.id == linked && f.name == "report.pdf"),
        "the original is listed: {:?}",
        chat.files
    );
}

#[test]
fn file_list_shows_stored_files_and_marks_one_missing_from_the_folder() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("chart.png"), PNG).unwrap();
    orch.list_stored_files(chat_id, vec![listing("chart.png"), listing("gone.png")]);
    drain(&mut rx);
    orch.handle_file_list();
    let (stored, shown) = drain(&mut rx)
        .into_iter()
        .find_map(|e| match e {
            AppEvent::FileProgress(FileProgress::Listed { stored, dir, .. }) => Some((stored, dir)),
            _ => None,
        })
        .expect("a listing");
    let seen: Vec<(&str, bool)> = stored
        .iter()
        .map(|f| (f.name.as_str(), f.missing))
        .collect();
    assert_eq!(seen, [("chart.png", false), ("gone.png", true)]);
    assert_eq!(shown, dir.display().to_string());
}

#[test]
fn removing_a_stored_file_deletes_our_copy_and_then_its_listing() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("chart.png"), PNG).unwrap();
    orch.list_stored_files(chat_id, vec![listing("chart.png")]);
    drain(&mut rx);
    orch.handle_file_remove("#1".into());
    assert!(!dir.join("chart.png").exists());
    assert!(files_of(&orch, chat_id).is_empty());
    assert!(drain(&mut rx).iter().any(|e| matches!(
        e,
        AppEvent::FileProgress(FileProgress::RemovedStored { name }) if name == "chart.png"
    )));
}

/// The order of the two writes (docs/lessons.md §8): a copy that cannot be deleted keeps
/// its listing, so the removal can be retried and nothing is lost. A directory where the
/// file should be is what makes the delete fail on every platform.
#[test]
fn a_stored_file_whose_copy_cannot_be_deleted_stays_listed() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(dir.join("chart.png")).unwrap();
    orch.list_stored_files(chat_id, vec![listing("chart.png")]);
    drain(&mut rx);
    orch.handle_file_remove("chart.png".into());
    assert_eq!(files_of(&orch, chat_id), ["chart.png"]);
    let msg = failure(drain(&mut rx)).expect("the refusal");
    assert!(msg.contains("chart.png"), "{msg}");
}

#[test]
fn a_name_an_attachment_and_a_stored_file_share_removes_neither() {
    let (_dir, mut orch, mut rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    orch.chats[0].attachments.push(Attachment::new(
        "chart.png",
        "/tmp/chart.png",
        "x".into(),
        1,
        AttachMode::Inline,
    ));
    orch.list_stored_files(chat_id, vec![listing("chart.png")]);
    drain(&mut rx);
    orch.handle_file_remove("chart.png".into());
    let msg = failure(drain(&mut rx)).expect("the refusal");
    assert!(msg.contains("#1") && msg.contains("#2"), "{msg}");
    assert_eq!(orch.chats[0].attachments.len(), 1);
    assert_eq!(files_of(&orch, chat_id), ["chart.png"]);
}

#[test]
fn adopting_lists_a_file_the_chat_does_not_and_deletes_nothing() {
    let (_dir, mut orch, _rx) = bare_orch_rx();
    let chat_id = open_chat(&mut orch);
    let dir = orch.stored_files_dir(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("orphan.csv"), b"a,b\n").unwrap();
    orch.adopt_unlisted_files();
    orch.adopt_unlisted_files();
    let chat = &orch.chats[0];
    assert_eq!(files_of(&orch, chat_id), ["orphan.csv"]);
    assert_eq!(chat.files[0].origin, FileOrigin::Recovered);
    assert!(dir.join("orphan.csv").exists());
}

/// The call site, not only the method: a chat saved without the listing of a file in its
/// folder has it listed — and saved — once the app has started (§11 S6).
#[tokio::test]
async fn the_bootstrap_adopts_unlisted_files_and_saves_the_listing() {
    let root = tempfile::tempdir().unwrap();
    let chat = {
        let storage = Storage::open(Paths::with_root(root.path())).unwrap();
        let profile = Profile::new("P", "sys");
        storage.json().upsert_profile(&profile).unwrap();
        let chat = Chat::from_profile(&profile, "t");
        storage.json().save_chat(&chat).unwrap();
        chat
    };
    let dir = root.path().join("files").join(chat.id.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("chart.png"), PNG).unwrap();
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(root.path(), None, no_auto_cfg());
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })),
    )
    .await
    .expect("the bootstrap activates a chat");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let files = load(root.path(), chat.id).files;
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["chart.png"]);
    assert_eq!(files[0].origin, FileOrigin::Recovered);
}

#[test]
fn a_turns_next_round_sees_the_files_its_calls_stored() {
    let (_d, _s, mut ctx) = crate::features::tools::testkit::ctx_with_storage(Uuid::new_v4());
    let effects = vec![ChatEffect::AddChatFile(Box::new(listing("chart.png")))];
    super::super::generation::sync_files(&mut ctx, &effects);
    // The effects list is cumulative across rounds: mirroring it twice adds nothing.
    super::super::generation::sync_files(&mut ctx, &effects);
    let names: Vec<&str> = ctx.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["chart.png"]);
}

/// A decodable 4×4 PNG.
fn real_png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(4, 4, image::Rgb([255, 255, 0]))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// A tool that stores a chart the way `python_exec` does: the bytes in the chat's folder,
/// a listing effect, and `image` for the model.
struct Charting {
    image: Vec<u8>,
    /// Draw a different chart each round — a trailing byte per call. Off, every round
    /// stores the same bytes, which is the dedupe case; on, each round has an image of its
    /// own, which is what a tool called three times normally produces.
    vary: bool,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl Tool for Charting {
    fn id(&self) -> ToolId {
        "charting".into()
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "draws a chart".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    async fn invoke(
        &self,
        ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> anyhow::Result<ToolOutcome> {
        use base64::Engine as _;
        let dir = ctx.files_dir.clone().expect("a turn has a files folder");
        let nth = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut image = self.image.clone();
        if self.vary {
            image.push(nth as u8);
        }
        let stored = crate::features::chat_files::store(&dir, &ctx.files, "chart.png", &image)?;
        let file = match stored {
            crate::features::chat_files::Stored::New(file) => file,
            // Listed, but the copy had gone from the folder: the bytes are back and the
            // listing stands, so there is still nothing to add.
            crate::features::chat_files::Stored::Restored(_) => {
                return Ok(ToolOutcome::text("files:\n- chart.png — restored"));
            }
            // The same bytes under a name the turn already listed: nothing to add.
            crate::features::chat_files::Stored::Unchanged(_) => {
                return Ok(ToolOutcome::text("files:\n- chart.png — unchanged"));
            }
        };
        Ok(ToolOutcome::with_effects(
            "files:\n- chart.png — shown to you below",
            vec![ChatEffect::AddChatFile(Box::new(file))],
        )
        .with_images(vec![ToolImage {
            mime: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(&image),
        }]))
    }
    fn group(&self) -> ToolGroup {
        ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "chart"
    }
}

/// A scripted engine that says what it can see, and counts being asked.
struct Sighted {
    inner: Arc<ScriptRecorder>,
    vision: VisionSupport,
    /// How many times the turn asked. The answer belongs to the server and the model, so
    /// it cannot change inside a turn — and asking is an HTTP round trip on the critical
    /// path, paid once per round that returned an image until it was memoized.
    asked: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl EngineBackend for Sighted {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        self.inner.chat_stream(req, cancel).await
    }
    async fn vision(&self) -> VisionSupport {
        self.asked
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.vision
    }
}

/// What a `charting` turn left: the data root (kept alive), the stored call's record (its
/// result and image count), the chat's stored files, and their folder.
struct ChartingTurn {
    _root: tempfile::TempDir,
    record: crate::entities::message::ToolCallRecord,
    files: Vec<ChatFile>,
    folder: std::path::PathBuf,
    /// How many times the turn asked the engine about images.
    asked: usize,
    /// The names of the images the chat's messages ended up carrying, in order.
    image_names: Vec<String>,
}

/// One turn in which the model calls `charting` once in each of `rounds` rounds.
async fn charting_turn(vision: VisionSupport, image: Vec<u8>, rounds: usize) -> ChartingTurn {
    charting_turn_drawing(vision, image, rounds, false).await
}

/// [`charting_turn`], with `vary` deciding whether each round draws a new chart.
async fn charting_turn_drawing(
    vision: VisionSupport,
    image: Vec<u8>,
    rounds: usize,
    vary: bool,
) -> ChartingTurn {
    let mut scripts: Vec<Script> = (1..=rounds)
        .map(|round| Script {
            chunks: vec![
                ChatChunk::ToolCall(ToolCallDelta {
                    thought_signature: None,
                    index: 0,
                    id: Some(format!("c{round}")),
                    name: Some("charting".into()),
                    arguments: "{}".into(),
                }),
                ChatChunk::Finished(FinishReason::ToolCalls),
            ],
            hang: false,
        })
        .collect();
    scripts.push(text("done"));
    let recorder = ScriptRecorder::new(scripts);
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend: Arc<dyn EngineBackend> = Arc::new(Sighted {
        inner: recorder,
        vision,
        asked: Arc::clone(&asked),
    });
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_tools(
        Some(backend),
        no_auto_cfg(),
        vec![Arc::new(Charting {
            image,
            vary,
            calls: std::sync::atomic::AtomicUsize::new(0),
        })],
    );
    let (mut pid, mut chat_id) = (None, None);
    while pid.is_none() || chat_id.is_none() {
        match tokio::time::timeout(Duration::from_secs(5), evt_rx.recv())
            .await
            .expect("startup events")
        {
            Some(AppEvent::ProfileList(v)) if !v.is_empty() => pid = Some(v[0].id),
            Some(AppEvent::ChatActivated { id, .. }) => chat_id = Some(id),
            Some(_) => {}
            None => panic!("the orchestrator went away during startup"),
        }
    }
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid.unwrap(),
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(vec!["charting".into()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx.send(AppCommand::SendMessage("draw".into())).unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. })),
    )
    .await
    .expect("the turn finishes");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat_id = chat_id.unwrap();
    let chat = load(dir.path(), chat_id);
    let record = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|r| r.name == "charting")
        .cloned()
        .expect("the call's record");
    let folder = dir.path().join("files").join(chat_id.to_string());
    let image_names = chat
        .messages
        .iter()
        .flat_map(|m| m.images.iter())
        .map(|i| i.name.clone())
        .collect();
    ChartingTurn {
        _root: dir,
        record,
        files: chat.files,
        folder,
        asked: asked.load(std::sync::atomic::Ordering::Relaxed),
        image_names,
    }
}

fn profile_note(key: &str, n: &str) -> String {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::default()).tf(key, &[("n", n)])
}

/// Whether the engine takes images belongs to the server and the model behind it, so it
/// cannot change inside a turn — but asking is a real HTTP round trip on the turn's
/// critical path, and nothing memoized it: a turn whose every round returned an image paid
/// one probe per round, up to `max_tool_rounds` of them.
#[tokio::test]
async fn the_engine_is_asked_about_images_once_a_turn_however_many_rounds_return_one() {
    let ChartingTurn { asked, .. } =
        charting_turn_drawing(VisionSupport::Supported, real_png(), 3, true).await;
    assert_eq!(
        asked, 1,
        "three rounds, three images, and the answer cannot have changed between them"
    );
}

#[tokio::test]
async fn an_image_the_model_cannot_take_is_withheld_and_the_result_says_so() {
    let ChartingTurn {
        _root,
        record,
        files,
        folder,
        ..
    } = charting_turn(VisionSupport::Unsupported, real_png(), 1).await;
    let result = record.result.unwrap_or_default();
    assert_eq!(record.images, 0, "{result}");
    assert!(
        result.contains(&profile_note("loop.images_no_vision", "1")),
        "{result}"
    );
    // The file itself landed regardless: only the pixels are withheld.
    assert_eq!(files.len(), 1);
    assert!(folder.join("chart.png").exists());
}

#[tokio::test]
async fn an_image_a_seeing_model_takes_is_sent_without_a_note() {
    let ChartingTurn {
        _root,
        record,
        files,
        ..
    } = charting_turn(VisionSupport::Supported, real_png(), 1).await;
    let result = record.result.unwrap_or_default();
    assert_eq!(record.images, 1, "{result}");
    assert_eq!(result, "files:\n- chart.png — shown to you below");
    assert_eq!(files.len(), 1);
}

/// `prepare_tool_images` drops what it cannot decode — silently, until the result had to
/// say so (§11 S8), for MCP's images as much as for a chart.
#[tokio::test]
async fn an_image_that_cannot_be_prepared_is_dropped_and_the_result_says_so() {
    let ChartingTurn { _root, record, .. } = charting_turn(
        VisionSupport::Unknown,
        b"\x89PNG\r\n\x1a\ntruncated".to_vec(),
        1,
    )
    .await;
    let result = record.result.unwrap_or_default();
    assert_eq!(record.images, 0, "{result}");
    assert!(
        result.contains(&profile_note("loop.images_dropped", "1")),
        "{result}"
    );
}

/// A tool image is named for the chat, not for the call. Numbered per call, every round
/// handed its first image `tool-image-1.png`, and two of those in one chat are a name two
/// items share — which `resolve` refuses (`Resolved::Shared`), so the model naming its own
/// chart in the next call's `files` bought a refusal with the round.
///
/// Two rounds, each drawing a chart of its own: the same bytes twice would be deduplicated
/// into one file and could not collide (the test below is that case).
#[tokio::test]
async fn each_round_s_chart_gets_a_name_of_its_own() {
    let ChartingTurn {
        _root, image_names, ..
    } = charting_turn_drawing(VisionSupport::Supported, real_png(), 2, true).await;
    assert_eq!(
        image_names,
        vec![
            "tool-image-1.png".to_string(),
            "tool-image-2.png".to_string()
        ],
        "the second round's chart takes the next free number"
    );
}

/// The mirror's call site (§11 S5): a turn's second round stores against the files its
/// first listed, so the same chart twice stays one file. Without `sync_files` the second
/// call finds the first on disk and saves `chart (2).png`.
#[tokio::test]
async fn a_second_round_storing_the_same_chart_keeps_one_file() {
    let ChartingTurn {
        _root,
        files,
        folder,
        ..
    } = charting_turn(VisionSupport::Supported, real_png(), 2).await;
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["chart.png"]);
    assert!(!folder.join("chart (2).png").exists());
}
