//! Orchestrator tests — stored files, the chat's side of the sandbox file exchange
//! (docs/sandbox-file-exchange.md §11 S5–S8, S11): a tool's `AddChatFile` lands once and is
//! mirrored into the turn, `/file list` and `/file remove` reach stored files, a copy that
//! cannot be deleted stays listed, the bootstrap adopts what a chat does not list, and an
//! image the model cannot take is withheld with a note. Part of the [`super`] module
//! (fixtures in mod.rs; the scripted engine in subagent.rs).

use std::time::Duration;

use super::subagent::{Script, ScriptRecorder, load, text};
use super::*;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::entities::chat_file::{ChatFile, FileOrigin};
use crate::entities::profile::ToolId;
use crate::features::file_command::FileProgress;
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
        let stored =
            crate::features::chat_files::store(&dir, &ctx.files, "chart.png", &self.image)?;
        let file = match stored {
            crate::features::chat_files::Stored::New(file) => file,
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
            data: base64::engine::general_purpose::STANDARD.encode(&self.image),
        }]))
    }
    fn group(&self) -> ToolGroup {
        ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "chart"
    }
}

/// A scripted engine that says what it can see.
struct Sighted {
    inner: Arc<ScriptRecorder>,
    vision: VisionSupport,
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
}

/// One turn in which the model calls `charting` once in each of `rounds` rounds.
async fn charting_turn(vision: VisionSupport, image: Vec<u8>, rounds: usize) -> ChartingTurn {
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
    let backend: Arc<dyn EngineBackend> = Arc::new(Sighted {
        inner: recorder,
        vision,
    });
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_tools(
        Some(backend),
        no_auto_cfg(),
        vec![Arc::new(Charting { image })],
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
    ChartingTurn {
        _root: dir,
        record,
        files: chat.files,
        folder,
    }
}

fn profile_note(key: &str, n: &str) -> String {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::default()).tf(key, &[("n", n)])
}

#[tokio::test]
async fn an_image_the_model_cannot_take_is_withheld_and_the_result_says_so() {
    let ChartingTurn {
        _root,
        record,
        files,
        folder,
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
