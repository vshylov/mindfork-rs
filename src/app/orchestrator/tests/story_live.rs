//! Live `#[ignore]` checks of a story driven from the chat (docs/fabula.md §4,
//! PR 9; docs/research/fabula-engine.md §14). The model is the chat's own and
//! every role's; the person is a script, which reads where the story stands
//! and answers as a person would.
//!
//! - `story_chat_live_canon_and_approval` — the gate models' smoke: a canon
//!   composed from attached sketches and stored with `story_create`; no
//!   approval without a message of the person's after the question; then the
//!   approval turn played `MINDFORK_FABULA_TRIALS` times (5) from that state,
//!   each counted as a call, a claim with no call, or neither.
//! - `story_chat_live_story` — a story end to end: the sketches, the canon,
//!   its approval, each chapter landed and accepted, until the story is
//!   finished or `MINDFORK_FABULA_CHAPTERS` chapters have landed. The chat's
//!   transcript, the story's markdown and its registers (PR 11) are written to
//!   `MINDFORK_FABULA_OUT` (`target/fabula/story-live-<time>`), whose `data/`, when
//!   a run left one there, is taken up where it stands; `MINDFORK_FABULA_BUDGET` is
//!   the chapter's budget in tokens (the default's). Gate G2 is
//!   this test on Claude Sonnet 5.5, in Russian (`MINDFORK_FABULA_LANG=ru`;
//!   docs/fabula.md §6, T9–T10). With `MINDFORK_FABULA_RERUN=<scene>` the
//!   person answers the first chapter's checkpoint by asking for that scene
//!   to be played again, and the chapter's take lands as a landing of its own
//!   (PR 10; with `MINDFORK_FABULA_CHAPTERS=2` the script ends at the take's
//!   checkpoint); the takes are reported with the story.
//! - `story_chat_live_map_mended` — the model's first `story_create` with a map
//!   has its routes dropped, the slip a live story found, and the story stored
//!   after the refusal must have a map every place of which is reachable from
//!   every other; `MINDFORK_FABULA_TRIALS` (3) fresh chats.
//!
//! Routes as the engine's own live checks take them
//! (`features::story::chapter_live_tests::route`): Claude with
//! `MINDFORK_ANTHROPIC_KEY` and `MINDFORK_FABULA_CLAUDE_MODEL`, or an
//! OpenAI-compatible engine, OpenRouter's with `MINDFORK_ENGINE_DIALECT=openrouter`.
//!
//! `cargo test story_chat_live -- --ignored --nocapture --test-threads=1`

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::entities::attachment::{AttachMode, Attachment};
use crate::entities::story::QuestionBody;
use crate::features::story::chapter_live_tests::route;
use crate::shared::config::ServerMode;
use crate::shared::i18n::Lang;
use crate::shared::storage::story::{StoryDb, StoryStatus, story_status_of};

const SKETCH_EN: &str = "Sketches for a story, \"The Last Ferry\"

- Place: a northern island; a village by the pier, a lighthouse on the cape, an abandoned \
cannery, a small chapel.
- Time: late October. A storm; the ferry to the mainland is cancelled for three days.
- Vera, 34, keeps the lighthouse and its weather log. She hides that three years ago she did \
not light the lamp on the night her brother's boat went down.
- Timur, 40, an inspector from the mainland, has come to close the lighthouse for good. He \
hides that he grew up on this island and is looking for his father's grave.
- Galina, 70, runs the only shop and knows everyone. She wants the lighthouse kept. She hides \
that she found the brother's logbook and kept it.
- Tone: quiet prose, no mysticism; close third person, past tense.
- Two chapters: in the first Timur arrives and runs into Vera; in the second the truth about \
that night comes out and the lighthouse's fate is decided.";

const SKETCH_RU: &str = "Наброски к повести «Последний паром»

- Место: северный остров; посёлок у причала, маяк на мысу, заброшенная консервная фабрика, \
маленькая часовня.
- Время: конец октября. Шторм, паром на материк отменён на три дня.
- Вера, 34 года, смотритель маяка, ведёт журнал погоды. Скрывает: три года назад она не зажгла \
маяк в ту ночь, когда разбился баркас её брата.
- Тимур, 40 лет, инспектор с материка, приехал закрыть маяк насовсем. Скрывает: он вырос на \
этом острове и ищет могилу отца.
- Галина Петровна, 70 лет, хозяйка единственной лавки, знает всех. Хочет, чтобы маяк остался. \
Скрывает: это она нашла журнал брата Веры и спрятала его.
- Тон: тихая проза, без мистики; близкое третье лицо, прошедшее время.
- Две главы: в первой Тимур приезжает и сталкивается с Верой; во второй правда о той ночи \
выходит наружу и решается судьба маяка.";

/// What the scripted person says, in the profile's language.
struct Lines {
    open: &'static str,
    build: &'static str,
    approve_canon: &'static str,
    accept: &'static str,
    accept_last: &'static str,
    scene: &'static str,
    resume: &'static str,
    /// A scene asked for again at the checkpoint: `{n}` is its number.
    rerun: &'static str,
}

const EN: Lines = Lines {
    open: "I attached my sketches for a story. Let's make a short story of two chapters from them: \
build the canon and create the story.",
    build: "Build the canon yourself from the sketches and create the story; the details are up to you.",
    approve_canon: "Yes, I approve the canon. Start the first chapter.",
    accept: "I accept this chapter. Go on to the next one.",
    accept_last: "I accept the chapter. Thank you, the story is done.",
    scene: "Try the scene again as you see fit.",
    resume: "Go on with the story.",
    rerun: "Play scene {n} again, please: this time the secret must not come out in it; let \
the scene end with it still kept.",
};

const RU: Lines = Lines {
    open: "Я приложил наброски к повести. Давай сделаем из них короткую историю на две главы: \
собери канон и создай историю.",
    build: "Собери канон сам по наброскам и создай историю; детали на твоё усмотрение.",
    approve_canon: "Да, канон одобряю. Начинай первую главу.",
    accept: "Главу принимаю, давай дальше.",
    accept_last: "Главу принимаю. Спасибо, история готова.",
    scene: "Попробуй сцену ещё раз, как считаешь нужным.",
    resume: "Продолжай историю.",
    rerun: "Переиграй, пожалуйста, сцену {n}: на этот раз тайна в ней не должна раскрыться, \
пусть сцена закончится, а секрет останется при герое.",
};

fn lang() -> Lang {
    match std::env::var("MINDFORK_FABULA_LANG")
        .as_deref()
        .map(str::trim)
    {
        Ok("ru") => Lang::Ru,
        _ => Lang::En,
    }
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// The route, its model, and the mode the app would be in on it.
fn live() -> Option<(Arc<dyn EngineBackend>, String, ServerMode)> {
    let (backend, model) = route()?;
    let claude = std::env::var("MINDFORK_ANTHROPIC_KEY").is_ok()
        && std::env::var("MINDFORK_FABULA_CLAUDE_MODEL").is_ok();
    let gateway = std::env::var("MINDFORK_ENGINE_DIALECT").is_ok_and(|d| d.trim() == "openrouter");
    let mode = if claude {
        ServerMode::Claude
    } else if gateway {
        ServerMode::OpenRouter
    } else {
        ServerMode::External
    };
    Some((backend, model, mode))
}

/// The app on `root`, over `backend`.
struct App {
    root: PathBuf,
    chat: Uuid,
    cmd_tx: UnboundedSender<AppCommand>,
    evt_rx: UnboundedReceiver<AppEvent>,
    handle: tokio::task::JoinHandle<()>,
    /// Background runs out, as the last event said.
    out: u32,
}

fn config(mode: ServerMode, model: &str) -> AppConfig {
    let mut config = AppConfig::default();
    config.engine.mode = mode;
    config.engine.claude.model_name = Some(model.to_owned());
    config.engine.openrouter.model_name = Some(model.to_owned());
    config.engine.external.model_name = Some(model.to_owned());
    config.tools.story = true;
    config.tools.subagent_background_wake = true;
    config.tools.story_run_timeout_secs = env_or("MINDFORK_FABULA_TIMEOUT", 3 * 60 * 60);
    config.tools.story_budget_tokens = env_or(
        "MINDFORK_FABULA_BUDGET",
        crate::shared::config::DEFAULT_STORY_BUDGET_TOKENS,
    );
    config.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    config
}

/// A fresh root: a profile in `lang` with every tool, and one chat carrying
/// the sketches as an attachment.
fn seed(root: &Path, lang: Lang) {
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(root));
    let persona = match lang {
        Lang::Ru => "Ты — соавтор, который помогает писать истории. Отвечай коротко и по делу.",
        _ => {
            "You are a co-writer who helps the user write stories. Keep replies short and to the point."
        }
    };
    let mut profile = Profile::new("Writer", persona);
    profile.language = lang;
    profile.enabled_tools = crate::features::tools::default_tool_ids();
    profile.known_tools = profile.enabled_tools.clone();
    json.upsert_profile(&profile).unwrap();
    let mut chat = Chat::from_profile(&profile, "Story");
    let sketch = match lang {
        Lang::Ru => SKETCH_RU,
        _ => SKETCH_EN,
    };
    chat.attachments.push(Attachment::new(
        "sketches.md",
        "sketches.md",
        sketch.to_owned(),
        sketch.len(),
        AttachMode::Inline,
    ));
    json.save_chat(&chat).unwrap();
}

impl App {
    async fn start(root: &Path, backend: Arc<dyn EngineBackend>, config: AppConfig) -> Self {
        let storage = Arc::new(Storage::open(Paths::with_root(root)).unwrap());
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, mut evt_rx) = unbounded_channel();
        let handle = tokio::spawn(run(OrchestratorDeps {
            cmd_rx,
            evt_tx,
            storage,
            config,
            supervisor: Arc::new(MockSupervisor::with_backend(Some(backend))),
            default_language: Lang::En,
            extra_tools: Vec::new(),
        }));
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let AppEvent::ChatActivated { id: chat, .. } = active else {
            unreachable!()
        };
        Self {
            root: root.to_path_buf(),
            chat,
            cmd_tx,
            evt_rx,
            handle,
            out: 0,
        }
    }

    /// The next turn's reply, to its end; `None` when nothing finished within
    /// `limit`.
    async fn reply(&mut self, limit: Duration) -> Option<String> {
        let deadline = Instant::now() + limit;
        let mut text = String::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = tokio::time::timeout(left, self.evt_rx.recv())
                .await
                .ok()??;
            match event {
                AppEvent::Chunk { text: t, .. } => text.push_str(&t),
                AppEvent::BackgroundRuns { out } => self.out = out,
                AppEvent::Finished { .. } => return Some(text),
                _ => {}
            }
        }
    }

    /// The person writes `line`; the assistant's reply.
    async fn say(&mut self, line: &str) -> String {
        eprintln!("\n>>> {line}");
        self.cmd_tx
            .send(AppCommand::SendMessage(line.to_owned()))
            .unwrap();
        let reply = self
            .reply(Duration::from_secs(15 * 60))
            .await
            .expect("the turn ends");
        eprintln!("<<< {reply}");
        reply
    }

    /// Reads what arrives within a moment of a turn's end: the background
    /// runs' count among it.
    async fn settle(&mut self) {
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(2), self.evt_rx.recv()).await
        {
            if let AppEvent::BackgroundRuns { out } = event {
                self.out = out;
            }
        }
    }

    async fn quit(self) -> (PathBuf, Uuid) {
        self.cmd_tx.send(AppCommand::Quit).unwrap();
        let _ = self.handle.await;
        (self.root, self.chat)
    }
}

/// The chat's story, when it has one, and where it stands.
fn story_of(root: &Path, chat: Uuid) -> Option<(String, StoryStatus)> {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    let json = storage.json();
    let chat = chat.to_string();
    let id = crate::features::tools::story::story_ids(&json.chat_stories_dir(&chat))
        .into_iter()
        .next()?;
    let status = story_status_of(&json.story_db(&chat, &id), &json.story_log(&chat, &id)).ok()?;
    Some((id, status))
}

/// Every call of the chat's assistant, by name, in order.
fn calls(chat: &Chat) -> Vec<String> {
    chat.messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .map(|c| c.name.clone())
        .collect()
}

/// Whether `reply` says the story went on: the words a model uses when it
/// narrates a start it never made.
fn claims_a_start(reply: &str) -> bool {
    let lower = reply.to_lowercase();
    [
        "started",
        "starting",
        "begun",
        "is being written",
        "writing chapter",
        "running",
        "запущ",
        "начал",
        "пишется",
        "приступ",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

fn out_dir(name: &str) -> PathBuf {
    let base = std::env::var("MINDFORK_FABULA_OUT").map_or_else(
        |_| {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("fabula")
                .join(format!("{name}-{stamp}"))
        },
        PathBuf::from,
    );
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The chat as markdown: every row, the calls with their results, the
/// notifications marked.
fn transcript(chat: &Chat) -> String {
    let mut out = String::new();
    for m in &chat.messages {
        let who = if m.is_notification() {
            "notification".to_owned()
        } else {
            format!("{:?}", m.role).to_lowercase()
        };
        out.push_str(&format!("### {who} — {}\n\n", m.timestamp.to_rfc3339()));
        if !m.text.trim().is_empty() {
            out.push_str(m.text.trim());
            out.push_str("\n\n");
        }
        for c in &m.tool_calls {
            out.push_str(&format!(
                "**call** `{}` `{}`\n\n",
                c.name,
                serde_json::to_string(&c.arguments).unwrap_or_default()
            ));
            if let Some(result) = &c.result {
                out.push_str(&format!("> {}\n\n", result.replace('\n', "\n> ")));
            }
        }
    }
    out
}

/// The story's markdown, read from its database once nothing holds it.
async fn story_markdown(root: &Path, chat: Uuid, id: &str) -> String {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    let json = storage.json();
    let (db, log) = (
        json.story_db(&chat.to_string(), id),
        json.story_log(&chat.to_string(), id),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let store = loop {
        match StoryDb::open(&db, &log).await {
            Ok(store) => break store,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => panic!("{e}"),
        }
    };
    let text = crate::features::story::export::markdown(&store)
        .await
        .unwrap();
    store.close().await.unwrap();
    text
}

/// The gate models' smoke: the canon from the sketches, no approval without
/// the person, and the approval turn counted.
#[tokio::test]
#[ignore = "needs a live route"]
async fn story_chat_live_canon_and_approval() {
    let Some((backend, model, mode)) = live() else {
        eprintln!("story_chat_live: no route; skipped");
        return;
    };
    let lang = lang();
    let lines = if lang == Lang::Ru { &RU } else { &EN };
    let base = out_dir("story-chat");
    let seeded = base.join("p1");
    seed(&seeded, lang);
    let mut cfg = config(mode, &model);
    // The trials' runs stop at once: what is counted is the turn, not the chapter.
    cfg.tools.story_run_timeout_secs = 1;
    let mut app = App::start(&seeded, backend.clone(), cfg.clone()).await;
    app.say(lines.open).await;
    for _ in 0..2 {
        if story_of(&app.root, app.chat).is_some() {
            break;
        }
        app.say(lines.build).await;
    }
    let (root, chat_id) = app.quit().await;
    let chat = super::subagent::load(&root, chat_id);
    let creates = calls(&chat).iter().filter(|c| *c == "story_create").count();
    let continues = calls(&chat)
        .iter()
        .filter(|c| *c == "story_continue")
        .count();
    let (id, status) = story_of(&root, chat_id).expect("a story was created from the sketches");
    eprintln!(
        "\n=== canon: story «{id}», {creates} story_create call(s), {continues} premature \
         story_continue call(s); P1 pending: {}; approved: {}",
        status.pending.is_some(),
        status.canon_approved
    );
    assert_eq!(
        status.pending.map(|q| q.body),
        Some(QuestionBody::Canon),
        "P1 waits"
    );
    assert!(!status.canon_approved, "no approval without the person");

    let trials: usize = env_or("MINDFORK_FABULA_TRIALS", 5);
    let (mut called, mut claimed, mut neither) = (0, 0, 0);
    for trial in 1..=trials {
        let copy = base.join(format!("trial-{trial}"));
        copy_dir(&root, &copy);
        let mut app = App::start(&copy, backend.clone(), cfg.clone()).await;
        let reply = app.say(lines.approve_canon).await;
        let (copy, chat_id) = app.quit().await;
        let chat = super::subagent::load(&copy, chat_id);
        let made = chat
            .messages
            .iter()
            .rev()
            .take_while(|m| m.text != lines.approve_canon)
            .flat_map(|m| &m.tool_calls)
            .any(|c| c.name == "story_continue");
        let approved = story_of(&copy, chat_id).is_some_and(|(_, s)| s.canon_approved);
        let verdict = match (made, claims_a_start(&reply)) {
            (true, _) => {
                called += 1;
                "called"
            }
            (false, true) => {
                claimed += 1;
                "CLAIMED WITHOUT A CALL"
            }
            (false, false) => {
                neither += 1;
                "no call, no claim"
            }
        };
        eprintln!("=== trial {trial}: {verdict}; canon approved: {approved}");
    }
    eprintln!(
        "\n=== {model}: {called} called, {claimed} claimed without a call, {neither} neither, of \
         {trials}; data in {}",
        base.display()
    );
}

/// Where a story stands, as the script compares it from turn to turn: the
/// question waiting, the chapters accepted, what the open chapter spent.
fn progress(state: Option<&(String, StoryStatus)>) -> Option<(Option<String>, usize, u64)> {
    state.map(|(_, s)| {
        (
            s.pending.as_ref().map(|q| q.id.to_string()),
            s.chapters.iter().filter(|c| c.accepted).count(),
            s.spend.tokens(),
        )
    })
}

/// What the person says next, from where the story stands; `None` when the
/// script is done. `rerun` is the scene to ask for again at the next
/// chapter's checkpoint, until it has been asked.
fn next_line(
    lines: &'static Lines,
    state: Option<&(String, StoryStatus)>,
    (landed, chapters): (u32, u32),
    rerun: &mut Option<u32>,
) -> Option<String> {
    Some(
        match state.map(|(_, s)| (s.pending.as_ref().map(|q| &q.body), s)) {
            None => lines.build,
            Some((Some(QuestionBody::Canon), _)) => lines.approve_canon,
            Some((Some(QuestionBody::Chapter { chapter, .. }), s)) => {
                if landed >= chapters {
                    return None;
                }
                if let Some(scene) = rerun.take() {
                    return Some(lines.rerun.replace("{n}", &scene.to_string()));
                }
                if *chapter >= s.planned {
                    lines.accept_last
                } else {
                    lines.accept
                }
            }
            Some((Some(QuestionBody::Scene { .. }), _)) => lines.scene,
            Some((None, s)) if s.open_chapter.is_none() && s.planned > 0 => return None,
            Some((None, _)) => lines.resume,
        }
        .to_owned(),
    )
}

/// Chapter 1's takes as the story holds them: each take's first scene, what
/// it carried and replaced, and the scenes the latest keeps (PR 10).
async fn takes_report(root: &Path, chat: Uuid, id: &str) -> String {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    let json = storage.json();
    let db = StoryDb::open(
        &json.story_db(&chat.to_string(), id),
        &json.story_log(&chat.to_string(), id),
    )
    .await
    .unwrap();
    let take = db.take_of(1).await.unwrap();
    let kept: Vec<String> = db
        .kept_scenes(1)
        .await
        .unwrap()
        .iter()
        .map(|k| k.scene.id.to_string())
        .collect();
    let versions: Vec<String> = db
        .chapter_versions(1)
        .await
        .unwrap()
        .iter()
        .map(|v| format!("{} {}", v.id, v.origin.as_str()))
        .collect();
    let spend = db.chapter_spend(1).await.unwrap();
    db.close().await.unwrap();
    format!(
        "take: {take:?}\nchapter 1 keeps: {kept:?}\nversions: {versions:?}\nthe latest take spent \
         {} tokens in {} calls",
        spend.tokens(),
        spend.calls
    )
}

/// Every version's register and summary, chapter by chapter (PR 11): the
/// threads opened and paid off, the facts stated.
async fn registers_report(root: &Path, chat: Uuid, id: &str) -> String {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    let json = storage.json();
    let db = StoryDb::open(
        &json.story_db(&chat.to_string(), id),
        &json.story_log(&chat.to_string(), id),
    )
    .await
    .unwrap();
    let mut out = String::new();
    for n in 1.. {
        let versions = db.chapter_versions(n).await.unwrap();
        if versions.is_empty() {
            break;
        }
        for v in versions {
            let Some(register) = db.register(&v.id).await.unwrap() else {
                continue;
            };
            out.push_str(&format!("\n## {}\n", v.id));
            for t in &register.opened {
                out.push_str(&format!(
                    "- opens {}: {} («{}»)\n",
                    t.id, t.question, t.quote
                ));
            }
            for p in &register.paid {
                out.push_str(&format!("- pays {}: {}\n", p.thread, p.how));
            }
            for f in &register.facts {
                out.push_str(&format!(
                    "- fact {} ({}): {} («{}»)\n",
                    f.id, f.stated_by, f.fact, f.quote
                ));
            }
            if let Some(summary) = db.summary(&v.id).await.unwrap() {
                out.push_str(&format!("- summary: {summary}\n"));
            }
        }
    }
    db.close().await.unwrap();
    out
}

/// A story end to end, the person scripted by where the story stands.
#[tokio::test]
#[ignore = "needs a live route"]
async fn story_chat_live_story() {
    let Some((backend, model, mode)) = live() else {
        eprintln!("story_chat_live: no route; skipped");
        return;
    };
    let lang = lang();
    let lines = if lang == Lang::Ru { &RU } else { &EN };
    let chapters: u32 = env_or("MINDFORK_FABULA_CHAPTERS", 99);
    let mut rerun = Some(env_or("MINDFORK_FABULA_RERUN", 0u32)).filter(|n| *n > 0);
    let base = out_dir("story-live");
    let root = base.join("data");
    // A root a run left behind is taken up where it stands.
    if !root.join("chats").is_dir() {
        seed(&root, lang);
    }
    let started = Instant::now();
    let mut app = App::start(&root, backend, config(mode, &model)).await;
    let mut landed = 0u32;
    let mut line = next_line(
        lines,
        story_of(&app.root, app.chat).as_ref(),
        (0, chapters),
        &mut rerun,
    )
    .unwrap_or_else(|| lines.open.to_owned());
    // A turn that moves the story nowhere twice running ends the script: a
    // spent budget, a model that will not call.
    let mut idle = 0;
    let mut last = progress(story_of(&app.root, app.chat).as_ref());
    for _turn in 0..16 {
        app.say(&line).await;
        app.settle().await;
        // A run went out: wait for its landing and the turn the app wakes.
        if app.out > 0 {
            eprintln!("--- a run is out ({:?} in)", started.elapsed());
            loop {
                // The run's own limit plus the landing and the woken turn: a
                // wait of the limit alone gave up as the limit cancelled the
                // run, before it landed (PR 10's live run).
                let limit = env_or("MINDFORK_FABULA_TIMEOUT", 3 * 60 * 60u64) + 15 * 60;
                let woke = app
                    .reply(Duration::from_secs(limit))
                    .await
                    .expect("the run lands within its time limit");
                eprintln!("<<< (woken) {woke}");
                if app.out == 0 {
                    break;
                }
            }
            // Only a chapter's checkpoint counts: a run the time limit
            // stopped lands too, and counted, it ended a two-chapter
            // script at chapter 1 (PR 11's live run).
            let at_chapter = matches!(
                story_of(&app.root, app.chat)
                    .as_ref()
                    .and_then(|(_, s)| s.pending.as_ref())
                    .map(|q| &q.body),
                Some(QuestionBody::Chapter { .. })
            );
            if at_chapter {
                landed += 1;
                eprintln!("--- landed {landed} ({:?} in)", started.elapsed());
            } else {
                eprintln!("--- a run stopped ({:?} in)", started.elapsed());
            }
        }
        let state = story_of(&app.root, app.chat);
        let now = progress(state.as_ref());
        idle = if now == last { idle + 1 } else { 0 };
        last = now;
        if idle >= 2 {
            eprintln!("--- the story did not move for two turns; the script ends");
            break;
        }
        let Some(next) = next_line(lines, state.as_ref(), (landed, chapters), &mut rerun) else {
            break;
        };
        line = next;
    }
    let (root, chat_id) = app.quit().await;
    let chat = super::subagent::load(&root, chat_id);
    std::fs::write(base.join("transcript.md"), transcript(&chat)).unwrap();
    let report = match story_of(&root, chat_id) {
        Some((id, status)) => {
            let text = story_markdown(&root, chat_id, &id).await;
            std::fs::write(base.join("story.md"), &text).unwrap();
            eprintln!("\n=== {}", takes_report(&root, chat_id, &id).await);
            let registers = registers_report(&root, chat_id, &id).await;
            std::fs::write(base.join("registers.md"), &registers).unwrap();
            eprintln!("\n=== the registers:{registers}");
            format!(
                "story «{id}»: {} of {} chapters accepted, finished: {}; {} words",
                status.chapters.iter().filter(|c| c.accepted).count(),
                status.planned,
                status.open_chapter.is_none(),
                text.split_whitespace().count()
            )
        }
        None => "no story".to_owned(),
    };
    eprintln!(
        "\n=== {model}: {report}; {:?}; transcript and story in {}",
        started.elapsed(),
        base.display()
    );
}

/// The engine with the slip a live story found (docs/journal/stories.md, *a
/// map with no roads*) put into the model's first `story_create` that holds
/// a map of two places or more: every route of it dropped. Every other answer
/// is the model's own.
struct Roadless {
    inner: Arc<dyn EngineBackend>,
    slipped: std::sync::atomic::AtomicBool,
}

/// `arguments` with every route of the canon's map dropped; `None` when
/// they hold no map of two places or more: a canon the model passed as a
/// string, one it could not write as JSON, a single place.
fn without_routes(arguments: &str) -> Option<String> {
    let mut args: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let places = args
        .pointer_mut("/canon/locations")?
        .as_array_mut()
        .filter(|places| places.len() > 1)?;
    for place in places
        .iter_mut()
        .filter_map(serde_json::Value::as_object_mut)
    {
        place.insert("routes".to_owned(), serde_json::json!([]));
    }
    Some(args.to_string())
}

/// Whether `arguments` hold a map of two places or more with no route at
/// all: the call [`Roadless`] slipped, as the chat stored it.
fn has_no_roads(arguments: &serde_json::Value) -> bool {
    arguments
        .pointer("/canon/locations")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|places| {
            places.len() > 1
                && places.iter().all(|p| {
                    p.get("routes")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(Vec::is_empty)
                })
        })
}

impl Roadless {
    /// `chunks` with the first `story_create`'s map left without routes.
    fn slip(&self, chunks: Vec<ChatChunk>) -> Vec<ChatChunk> {
        use std::sync::atomic::Ordering;
        let mut calls = crate::shared::api::ToolCallAccumulator::default();
        let mut rest = Vec::new();
        for chunk in chunks {
            match chunk {
                ChatChunk::ToolCall(delta) => calls.push(delta),
                other => rest.push(other),
            }
        }
        let mut deltas = Vec::new();
        for (index, mut call) in calls.finish().into_iter().enumerate() {
            if call.name == "story_create"
                && !self.slipped.load(Ordering::SeqCst)
                && let Some(arguments) = without_routes(&call.arguments)
            {
                call.arguments = arguments;
                self.slipped.store(true, Ordering::SeqCst);
            }
            deltas.push(ChatChunk::ToolCall(
                crate::shared::api::contract::ToolCallDelta {
                    index,
                    id: Some(call.id),
                    name: Some(call.name),
                    arguments: call.arguments,
                    thought_signature: call.thought_signature,
                },
            ));
        }
        // The calls go where the stream had them: before its counter and end.
        let at = rest
            .iter()
            .position(|c| matches!(c, ChatChunk::Usage(_) | ChatChunk::Finished(_)))
            .unwrap_or(rest.len());
        rest.splice(at..at, deltas);
        rest
    }
}

#[async_trait::async_trait]
impl EngineBackend for Roadless {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::shared::api::contract::ChatStream> {
        use futures_util::StreamExt;
        let stream = self.inner.chat_stream(req, cancel).await?;
        if self.slipped.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(stream);
        }
        let chunks = self.slip(stream.collect().await);
        Ok(Box::pin(futures_util::stream::iter(chunks)))
    }

    async fn context_budget(&self) -> Option<u32> {
        self.inner.context_budget().await
    }

    async fn model_capabilities(&self) -> Option<crate::shared::api::contract::ModelCapabilities> {
        self.inner.model_capabilities().await
    }

    async fn vision(&self) -> crate::shared::api::VisionSupport {
        self.inner.vision().await
    }

    async fn model_id(&self) -> Option<String> {
        self.inner.model_id().await
    }

    async fn parallel_slots(&self) -> Option<u32> {
        self.inner.parallel_slots().await
    }

    async fn server_kind(&self) -> crate::shared::api::ServerKind {
        self.inner.server_kind().await
    }

    async fn enforces_output_schema(&self) -> bool {
        self.inner.enforces_output_schema().await
    }
}

/// A map with no roads mended from the refusal: the model's first
/// `story_create` with a map has every route dropped, as Qwen 3.8's once came, and
/// the story the chat stores must have a map whose every place is reachable
/// from every other. `MINDFORK_FABULA_TRIALS` (3) fresh chats.
#[tokio::test]
#[ignore = "needs a live route"]
async fn story_chat_live_map_mended() {
    let Some((backend, model, mode)) = live() else {
        eprintln!("story_chat_live: no route; skipped");
        return;
    };
    let lang = lang();
    let lines = if lang == Lang::Ru { &RU } else { &EN };
    let base = out_dir("story-map");
    let trials: usize = env_or("MINDFORK_FABULA_TRIALS", 3);
    let mut mended = 0;
    for trial in 1..=trials {
        let root = base.join(format!("trial-{trial}"));
        seed(&root, lang);
        let roadless = Arc::new(Roadless {
            inner: backend.clone(),
            slipped: std::sync::atomic::AtomicBool::new(false),
        });
        let mut cfg = config(mode, &model);
        cfg.tools.story_run_timeout_secs = 1;
        let mut app = App::start(&root, roadless.clone(), cfg).await;
        app.say(lines.open).await;
        for _ in 0..2 {
            if story_of(&app.root, app.chat).is_some() {
                break;
            }
            app.say(lines.build).await;
        }
        let (root, chat_id) = app.quit().await;
        let chat = super::subagent::load(&root, chat_id);
        tokio::fs::write(base.join(format!("trial-{trial}.md")), transcript(&chat))
            .await
            .unwrap();
        let creates: Vec<&crate::entities::message::ToolCallRecord> = chat
            .messages
            .iter()
            .flat_map(|m| &m.tool_calls)
            .filter(|c| c.name == "story_create")
            .collect();
        let slipped = roadless.slipped.load(std::sync::atomic::Ordering::SeqCst);
        // The slipped call is found by its map, the one with no roads at all.
        let at = creates.iter().position(|c| has_no_roads(&c.arguments));
        let refusal = at
            .and_then(|i| creates[i].result.as_deref())
            .unwrap_or_default();
        let named = refusal.matches("no route leads from").count();
        let stored = story_of(&root, chat_id);
        if slipped {
            assert!(
                named > 0,
                "the roadless map is refused for its roads: {refusal}"
            );
        }
        if slipped && stored.is_some() {
            mended += 1;
        }
        eprintln!(
            "=== trial {trial}: slipped {slipped} (story_create call {}); the refusal named \
             {named} way(s) missing; {} story_create call(s); stored: {}",
            at.map_or_else(|| "none".to_owned(), |i| (i + 1).to_string()),
            creates.len(),
            stored.map_or_else(|| "nothing".to_owned(), |(id, _)| format!("«{id}»"))
        );
    }
    eprintln!(
        "\n=== {model}: {mended} of {trials} roadless maps mended; data in {}",
        base.display()
    );
    assert!(mended > 0, "no roadless map was mended");
}
