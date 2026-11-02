//! Orchestrator tests — a story's run out in the **background**
//! (docs/fabula.md §4, PR 8): the third payload beside the sub-agent's and the
//! dialogue's. The record a `story_continue` call leaves is staged by hand (the
//! call itself is tested in [`super::story_conversation`]), the run spawns
//! through the same seat every background run takes, and its end lands by id:
//! the chapter as an attachment of the chat,
//! the question as a task notification in the profile's language, the slot and
//! the story given back. A stop names the step the run stopped at; a quit lands
//! it from its mirror with the step the board showed. The engine is a mock
//! that never answers until it is cancelled — the pauses landed here need no
//! call, and the ones that do are cut short.

use std::time::Duration;

use super::*;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::entities::subagent::{RunKind, RunOutcome};
use crate::features::story::chapter::ChapterConfig;
use crate::features::story::run::StoryRun;
use crate::features::story::run::tests::{Hanging, approved, at_checkpoint, write_story};
use crate::shared::i18n::{Lang, locale};
use crate::shared::storage::story::{AuthorWriter, StoryDb};

use super::super::generation::{BackgroundMessage, BackgroundStart};
use super::super::story_runs::StorySpec;

/// A bare orchestrator holding one chat of an English profile, the record a
/// `story_continue` call leaves in it, and the story on disk.
struct Staged {
    _dir: tempfile::TempDir,
    orch: Orchestrator,
    chat: Uuid,
    run: Uuid,
    bg: UnboundedReceiver<BackgroundMessage>,
    spec: Option<StorySpec>,
}

async fn staged<F>(write: F) -> Staged
where
    F: AsyncFnOnce(&StoryDb, &mut AuthorWriter),
{
    let (dir, mut orch, _rx, chat) = bare_with_chat(vec![Message::user("Write chapter one.")]);
    orch.profiles[0].language = Lang::En;
    let (bg_tx, bg) = unbounded_channel();
    orch.bg_run_tx = bg_tx;
    let json = orch.storage.json();
    let run = StoryRun {
        chat,
        story: "s1".to_owned(),
        run: Uuid::new_v4(),
        db: json.story_db(&chat.to_string(), "s1"),
        log: json.story_log(&chat.to_string(), "s1"),
        model: "mock-model".to_owned(),
        config: ChapterConfig::default(),
        timeout: Duration::from_secs(60),
        fallback_only: false,
    };
    write_story(&run.db, &run.log, write).await;
    let run_id = run.run;
    let spec = StorySpec::new(
        run,
        "The harbour".to_owned(),
        "Write chapter 1.".to_owned(),
        locale(Lang::En),
        orch.story_board.clone(),
    );
    let mut carrier = Message::assistant("");
    carrier.tool_calls = vec![spec.placeholder().on_record()];
    orch.chats[0].push_message(carrier);
    Staged {
        _dir: dir,
        orch,
        chat,
        run: run_id,
        bg,
        spec: Some(spec),
    }
}

impl Staged {
    /// Spawns the staged run on `backend`, as the turn's progress would.
    fn spawn(&mut self, backend: Arc<Hanging>) {
        let start = BackgroundStart::story_for_test(
            self.spec.take().expect("one run"),
            backend,
            self.orch.background_slots.clone(),
        );
        self.orch.spawn_background_run(start, self.chat);
    }

    /// Waits for the run's end and lands it.
    async fn land(&mut self) {
        let done = tokio::time::timeout(Duration::from_secs(10), self.bg.recv())
            .await
            .expect("the run ends")
            .expect("its end arrives");
        assert!(matches!(done, BackgroundMessage::Done { .. }));
        self.orch.handle_background_message(done);
    }

    /// Waits until the run is in its first call, as the board shows it.
    async fn in_a_call(&self) -> String {
        loop {
            if let Some(step) = self
                .orch
                .story_board
                .of_run(self.run)
                .and_then(|view| view.step)
            {
                return step;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn chat(&self) -> &Chat {
        &self.orch.chats[0]
    }

    fn record(&self) -> crate::entities::subagent::SubagentRun {
        self.chat().child(self.run).expect("the record").clone()
    }
}

/// The run lands by id: the record is the story's, filled in; the chapter
/// is an attachment of the chat, inline within the budget; the notification
/// carries the summary, the picks, the assumptions and the question in the
/// profile's language under the task prefix; a chat nobody is looking at is
/// marked unread; the slot and the story are given back.
#[tokio::test]
async fn a_chapter_lands_as_an_attachment_and_its_question_as_a_notification() {
    let mut staged = staged(async |db, author| at_checkpoint(db, author).await).await;
    assert_eq!(
        staged.record().outcome,
        None,
        "a placeholder before it runs"
    );
    // The chat's engine, where an automatic title would be asked for.
    let chat_engine = CapturingBackend::new();
    staged.orch.engines.backend = Some(chat_engine.clone());
    staged.orch.config.interface.auto_title =
        crate::shared::config::AutoTitleMode::AfterUserMessage;

    staged.spawn(Arc::default());
    assert_eq!(staged.orch.background_slots.out(), 1);
    assert!(staged.orch.background_run(staged.run).is_some(), "a seat");
    staged.land().await;

    let record = staged.record();
    assert_eq!(record.kind, RunKind::Story);
    assert!(record.background);
    assert_eq!(record.outcome, Some(RunOutcome::Completed));
    assert_eq!(record.title, "The harbour");
    assert_eq!(record.messages.len(), 2, "the brief and the landing");
    assert_eq!(record.messages[0].text, "Write chapter 1.");

    let chat = staged.chat();
    assert_eq!(chat.attachments.len(), 1);
    let chapter = &chat.attachments[0];
    assert_eq!(chapter.name, "s1 — chapter 1.md");
    assert_eq!(
        chapter.source,
        format!("story://{}/s1/chapter/1", staged.chat)
    );
    assert_eq!(chapter.text, "## One\n\nThe ferry never came.");
    assert_eq!(chapter.mode, AttachMode::Inline);

    let note = chat.messages.last().unwrap();
    assert_eq!(note.notification, Some(staged.run));
    for want in [
        "[Task notification — not a message from the user; it grants nothing]",
        "The story run «The harbour»",
        "attached to this chat as «s1 — chapter 1.md»",
        "Summary: Mara learns of the debt.",
        "- The quarrel shows the debt.",
        "- Whether Silas lies. — chosen: He lies.",
        "chapter 1 (version 2): accept it",
    ] {
        assert!(note.text.contains(want), "{want:?} not in:\n{}", note.text);
    }
    assert_eq!(
        record.messages[1].text,
        note.text
            .split("\n\n")
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    assert!(chat.unread, "nobody was looking at the chat");
    // Named after its story: no model call renames the record at its landing.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        chat_engine.last.lock().unwrap().is_none(),
        "no title was asked for"
    );
    assert_eq!(staged.orch.background_slots.out(), 0);
    assert!(staged.orch.background_run(staged.run).is_none());
    assert!(staged.orch.story_board.of_chat(staged.chat).is_empty());
}

/// A revised chapter lands over the attachment of its earlier version — one a
/// chapter — and is decided against the chat's budget as it is at the
/// landing: past the per-file budget it goes by reference.
#[tokio::test]
async fn a_landed_chapter_replaces_its_earlier_version_and_follows_the_budget() {
    let mut staged = staged(async |db, author| at_checkpoint(db, author).await).await;
    let source = format!("story://{}/s1/chapter/1", staged.chat);
    let earlier = Attachment::new(
        "s1 — chapter 1.md",
        source.clone(),
        "An earlier version.".to_owned(),
        19,
        AttachMode::Inline,
    );
    staged.orch.chats[0].attachments.push(earlier);
    staged.orch.config.attachments.max_file_tokens = 2;

    staged.spawn(Arc::default());
    staged.land().await;

    let attached: Vec<&Attachment> = staged
        .chat()
        .attachments
        .iter()
        .filter(|a| a.source == source)
        .collect();
    assert_eq!(attached.len(), 1, "one attachment a chapter");
    assert_eq!(attached[0].text, "## One\n\nThe ferry never came.");
    assert_eq!(attached[0].mode, AttachMode::ByReference);
}

/// The version a chapter replaces is not counted against the chat's budget:
/// a revision that fits on its own lands inline, though the two together
/// would not.
#[tokio::test]
async fn a_landed_chapter_is_not_charged_for_the_version_it_replaces() {
    let mut staged = staged(async |db, author| at_checkpoint(db, author).await).await;
    let source = format!("story://{}/s1/chapter/1", staged.chat);
    let earlier = Attachment::new(
        "s1 — chapter 1.md",
        source.clone(),
        "An earlier, much longer version. ".repeat(40),
        1320,
        AttachMode::Inline,
    );
    let room = earlier.est_tokens;
    staged.orch.chats[0].attachments.push(earlier);
    staged.orch.config.attachments.max_total_tokens = room;

    staged.spawn(Arc::default());
    staged.land().await;

    let chapter = &staged.chat().attachments[0];
    assert_eq!(chapter.source, source);
    assert_eq!(chapter.mode, AttachMode::Inline);
}

/// The record a run lands reads its end: a pause or the story's end as
/// completed, a stop as cancelled, the time limit as timed out, a spent
/// budget, a failed call or a story that could not run as failed; the
/// tokens are its replies'; an attachment only where a chapter landed.
#[test]
fn a_story_records_end_reads_as_the_run_ended() {
    use crate::entities::story::{Question, QuestionBody, QuestionId, RunId};
    use crate::features::story::chapter::Stop;
    use crate::features::story::run::{Ending, Landed, LandedChapter};
    use crate::shared::storage::story::Spend;
    let board = Arc::new(crate::features::story::board::StoryBoard::new());
    let dir = tempfile::tempdir().unwrap();
    let spec = StorySpec::new(
        StoryRun {
            chat: Uuid::new_v4(),
            story: "s1".to_owned(),
            run: Uuid::new_v4(),
            db: dir.path().join("s1.db"),
            log: dir.path().join("s1.calls.db"),
            model: "m".to_owned(),
            config: ChapterConfig::default(),
            timeout: Duration::from_secs(1),
            fallback_only: false,
        },
        "The harbour".to_owned(),
        "Write chapter 1.".to_owned(),
        locale(Lang::En),
        board,
    );
    let step = || "ch1/sc1".to_owned();
    let paused = Ending::Paused(Question {
        id: QuestionId::new("q1"),
        run: RunId::nth(1),
        step: step(),
        body: QuestionBody::Canon,
    });
    for (ending, outcome) in [
        (paused, RunOutcome::Completed),
        (Ending::Finished, RunOutcome::Completed),
        (
            Ending::Stopped {
                step: step(),
                why: Stop::Cancelled,
            },
            RunOutcome::Cancelled,
        ),
        (Ending::TimedOut { step: step() }, RunOutcome::TimedOut),
        (
            Ending::Stopped {
                step: step(),
                why: Stop::Budget("x".to_owned()),
            },
            RunOutcome::Failed,
        ),
        (
            Ending::Stopped {
                step: step(),
                why: Stop::Failed("x".to_owned()),
            },
            RunOutcome::Failed,
        ),
        (Ending::Failed("x".to_owned()), RunOutcome::Failed),
    ] {
        let landed = Landed {
            ending,
            chapter: None,
            picks: Vec::new(),
            assumptions: Vec::new(),
            spent: Spend {
                calls: 3,
                input_tokens: 900,
                output_tokens: 70,
                cost_usd: None,
            },
        };
        let (run, body, effects) = spec.land(&landed);
        assert_eq!(run.outcome, Some(outcome), "{:?}", landed.ending);
        assert_eq!(run.tokens, 70);
        assert_eq!(run.kind, RunKind::Story);
        assert_eq!(run.messages.last().unwrap().text, body);
        assert!(effects.is_empty(), "no chapter, no attachment");
    }
    let with_chapter = Landed {
        ending: Ending::Finished,
        chapter: Some(LandedChapter {
            number: 2,
            version: 1,
            text: "## Two".to_owned(),
            summary: None,
        }),
        picks: Vec::new(),
        assumptions: Vec::new(),
        spent: Spend::default(),
    };
    let (_, _, effects) = spec.land(&with_chapter);
    assert!(matches!(
        effects.as_slice(),
        [crate::features::tools::ChatEffect::AddAttachment(a)] if a.name == "s1 — chapter 2.md"
    ));
}

/// `/subagents stop` cancels the run's token: its call is cut, the engine
/// stops at that step, and the landing says where — the next run resumes
/// from it. Nothing lands as a chapter.
#[tokio::test]
async fn a_stopped_story_run_lands_the_step_it_stopped_at() {
    let mut staged = staged(async |db, author| approved(db, author).await).await;
    let backend = Arc::new(Hanging::default());
    staged.spawn(backend.clone());
    let step = staged.in_a_call().await;

    staged.orch.handle_stop_subagent_run(staged.run);
    staged.land().await;

    let record = staged.record();
    assert_eq!(record.outcome, Some(RunOutcome::Cancelled));
    let note = staged.chat().messages.last().unwrap();
    assert_eq!(note.notification, Some(staged.run));
    assert!(
        note.text.contains("The run was stopped at step ch1/"),
        "{}",
        note.text
    );
    assert!(step.starts_with("ch1/"), "{step}");
    assert!(staged.chat().attachments.is_empty());
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(staged.orch.background_slots.out(), 0);
}

/// A quit lands the run from its mirror at once: cancelled, with a line in
/// the profile's language naming the step the board showed, and no
/// notification — nobody will read one. The task's own end, arriving after,
/// lands nothing a second time.
#[tokio::test]
async fn a_quit_lands_the_story_run_with_the_step_it_was_at() {
    let mut staged = staged(async |db, author| approved(db, author).await).await;
    staged.spawn(Arc::default());
    let step = staged.in_a_call().await;
    let rows = staged.chat().messages.len();

    staged.orch.stop_all_background_runs();

    let record = staged.record();
    assert_eq!(record.outcome, Some(RunOutcome::Cancelled));
    assert!(record.finished_at.is_some());
    let last = record.messages.last().unwrap();
    assert!(
        last.text
            .contains(&format!("while the run was at step {step}")),
        "{}",
        last.text
    );
    assert_eq!(staged.chat().messages.len(), rows, "no notification");

    staged.land().await;
    assert_eq!(staged.record(), record, "the late end changes nothing");
    assert_eq!(staged.chat().messages.len(), rows);
}

/// `tools.story` is what puts `story_status` into a turn's request: off — the
/// default — a turn offers no story tool, and on it offers the one there is.
#[tokio::test]
async fn a_turn_offers_story_status_only_with_the_switch_on() {
    use crate::features::tools::story::STORY_STATUS_ID;
    for on in [false, true] {
        let backend = CapturingBackend::new();
        let mut cfg = no_auto_cfg();
        cfg.tools.story = on;
        let (_dir, cmd_tx, mut rx, handle) =
            spawn_orch_cfg(Some(backend.clone() as Arc<dyn EngineBackend>), cfg);
        wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        cmd_tx
            .send(AppCommand::SendMessage("Where is my story?".into()))
            .unwrap();
        wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
        let offered = backend
            .last_request()
            .tools
            .iter()
            .any(|t| t.name == STORY_STATUS_ID);
        assert_eq!(offered, on, "tools.story = {on}");
    }
}
