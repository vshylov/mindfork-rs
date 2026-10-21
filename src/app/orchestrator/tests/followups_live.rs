//! Live `#[ignore]` smokes for the companion's follow-ups (spec §9.3.6;
//! docs/research/companion-platform.md C4 (a), §3.5). A companion profile with
//! *Follow-ups* and the time rows on is put on disk; two smokes run a real turn
//! and read what it planned, two start the app on a follow-up already due and
//! read what the woken turn wrote.
//!
//! `MINDFORK_ENGINE_URL=…/v1 cargo test followups_live -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::entities::followup::Followup;
use chrono::{Duration, Local, Timelike, Utc};

/// The probe's persona (§3.2, §3.5 there).
const PERSONA: &str = "You are Sol, Alex's companion. You talk with Alex the way a close \
friend would: warm, honest, curious, never syrupy. You remember what Alex tells you, and you \
use your tools when it helps. Keep replies conversational and fairly short.";

const INTERVIEW: &str = "Tomorrow at 10 I have the interview at the game studio I told you \
about. I'm so nervous, I keep rehearsing answers in my head.";

const PLAN_REPLY: &str = "You've prepared for this — trust that. Sleep well, and I'll check \
in tomorrow afternoon to hear how it went. Good luck!";

/// When the woken smokes' conversation happened, on the local wall clock — fixed
/// to days, so the story holds whatever hour the smoke runs at: the interview was
/// *yesterday* at 10, mentioned the evening before at 21:10.
fn evening_before() -> chrono::DateTime<Utc> {
    let day = Local::now().date_naive() - Duration::days(2);
    crate::features::followups::local_to_utc(day.and_hms_opt(21, 10, 0).unwrap())
}

/// What the follow-up says it is about — the interview, on yesterday's weekday.
fn about() -> String {
    let yesterday = (Local::now().date_naive() - Duration::days(1)).format("%A");
    format!("Ask Alex how the interview at the game studio went (it was on {yesterday} at 10).")
}

/// The app on a fresh root: a companion profile (follow-ups and time rows on, no
/// quiet hours, so the hour a smoke runs at never holds a delivery back) and one
/// conversation of `messages`, which may hold a follow-up due now. `None` without
/// an engine.
struct Live {
    dir: tempfile::TempDir,
    root: std::path::PathBuf,
    chat: Uuid,
    /// The follow-up planted due now, when there is one.
    planted: Option<Uuid>,
    cmd_tx: UnboundedSender<AppCommand>,
    evt_rx: UnboundedReceiver<AppEvent>,
    handle: tokio::task::JoinHandle<()>,
}

async fn start(messages: Vec<Message>, due: Option<String>) -> Option<Live> {
    let backend = live_backend()?;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(&root));
    let mut profile = Profile::new("Companion", PERSONA);
    profile.language = crate::shared::i18n::Lang::En;
    profile.enabled_tools = crate::features::tools::default_tool_ids();
    profile.known_tools = profile.enabled_tools.clone();
    profile.companion.followups = true;
    profile.companion.time_rows = true;
    profile.companion.quiet_from = 0;
    profile.companion.quiet_until = 0;
    json.upsert_profile(&profile).unwrap();
    let mut chat = Chat::from_profile(&profile, "Alex");
    let planned_at = messages.first().map(|m| m.timestamp);
    for m in messages {
        chat.push_message(m);
    }
    json.save_chat(&chat).unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());
    let mut planted = None;
    if let Some(about) = due.as_deref() {
        let mut f = Followup::new(
            profile.id,
            chat.id,
            about,
            Utc::now() - Duration::minutes(1),
        );
        f.created_at = planned_at.unwrap_or_else(Utc::now) + Duration::seconds(20);
        storage.db().followup_insert(&f).unwrap();
        planted = Some(f.id);
    }
    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend(Some(backend))),
        default_language: crate::shared::i18n::Lang::En,
        extra_tools: Vec::new(),
    }));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    Some(Live {
        dir,
        root,
        chat: chat.id,
        planted,
        cmd_tx,
        evt_rx,
        handle,
    })
}

impl Live {
    /// Ends the app; the data stays until the returned guard is dropped.
    async fn quit(self) -> (tempfile::TempDir, std::path::PathBuf) {
        self.cmd_tx.send(AppCommand::Quit).unwrap();
        let _ = self.handle.await;
        (self.dir, self.root)
    }
}

/// The text a woken turn streamed, up to its end.
async fn woken_reply(evt_rx: &mut UnboundedReceiver<AppEvent>) -> String {
    let mut out = String::new();
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Chunk { text, .. } => out.push_str(&text),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    out
}

fn pending(root: &std::path::Path, chat: Uuid) -> Vec<Followup> {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    storage.db().followups_pending_in_chat(chat).unwrap()
}

/// A message written `after` the evening before the interview.
fn at(after: Duration, text: &str, user: bool) -> Message {
    let mut m = if user {
        Message::user(text)
    } else {
        Message::assistant(text)
    };
    m.timestamp = evening_before() + after;
    m
}

fn says_any(text: &str, words: &[&str]) -> bool {
    let lower = text.to_lowercase();
    words.iter().any(|w| lower.contains(w))
}

/// Guilt, clinginess, reproach about silence — a failure beside relevance (§7,
/// rule 4 there).
const PRESSURE: [&str; 5] = [
    "haven't heard",
    "why didn't you",
    "you didn't tell",
    "i missed you",
    "waiting for you to",
];

/// A reminder the person asked for, on a named day: planned for that day at that
/// hour. The weekday beside the date is what corrects a model that miscounts.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn followup_planned_for_a_reminder_asked_for_e2e_live() {
    let Some(mut live) = start(Vec::new(), None).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let day = Local::now().date_naive() + Duration::days(2);
    let weekday = day.format("%A").to_string();
    let ask = format!("Can you remind me on {weekday} at 9 to call the bank about my card?");
    let (reply, calls) = run_turn_capture(&live.cmd_tx, &mut live.evt_rx, &ask).await;
    let chat = live.chat;
    let (_dir, root) = live.quit().await;
    let planned = pending(&root, chat);
    eprintln!("calls: {calls:?}\nreply: {reply}\nplanned: {planned:?}");
    let f = planned.last().expect("a follow-up planned");
    let due = f.due_at.with_timezone(&Local);
    assert_eq!(due.date_naive(), day, "planned for {weekday}: {due}");
    assert_eq!(due.hour(), 9, "at 9: {due}");
    assert!(says_any(&f.about, &["bank"]), "{}", f.about);
}

/// An event the person mentions, then good night: a follow-up planned after it.
/// Gemma 4 planned unprompted in 3 of 5 probe runs (§3.5), Qwen 3.8 in 5 of 5.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn followup_planned_after_an_event_mentioned_e2e_live() {
    let Some(mut live) = start(Vec::new(), None).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (first, calls1) = run_turn_capture(&live.cmd_tx, &mut live.evt_rx, INTERVIEW).await;
    let (second, calls2) = run_turn_capture(
        &live.cmd_tx,
        &mut live.evt_rx,
        "Ok, thanks. I should sleep. Good night!",
    )
    .await;
    let chat = live.chat;
    let (_dir, root) = live.quit().await;
    let planned = pending(&root, chat);
    eprintln!(
        "calls: {calls1:?} {calls2:?}\nreplies: {first}\n---\n{second}\nplanned: {planned:?}"
    );
    let f = planned
        .first()
        .expect("a follow-up planned for the interview");
    let tomorrow_ten = (Local::now().date_naive() + Duration::days(1))
        .and_hms_opt(10, 0, 0)
        .unwrap();
    let due = f.due_at.with_timezone(&Local).naive_local();
    assert!(
        due > tomorrow_ten && due < tomorrow_ten + Duration::hours(36),
        "after the interview, within a day and a half: {due}"
    );
    assert!(says_any(&f.about, &["interview", "studio"]), "{}", f.about);
}

/// A follow-up due now, nothing said since: the app wakes the companion, which
/// asks about the interview, with no pressure.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn followup_due_wakes_the_companion_e2e_live() {
    let messages = vec![
        at(Duration::zero(), INTERVIEW, true),
        at(Duration::seconds(30), PLAN_REPLY, false),
    ];
    let Some(mut live) = start(messages, Some(about())).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let reply = woken_reply(&mut live.evt_rx).await;
    let chat = live.chat;
    let (_dir, root) = live.quit().await;
    eprintln!("woken reply: {reply}");
    assert!(!crate::features::followups::is_silent(&reply), "{reply}");
    assert!(
        says_any(
            &reply,
            &["interview", "how did it go", "how'd it go", "how was it"]
        ),
        "the reply should follow up on the interview: {reply}"
    );
    assert!(!says_any(&reply, &PRESSURE), "pressure: {reply}");
    let storage = Storage::open(Paths::with_root(&root)).unwrap();
    let delivered = storage.db().followups_pending_in_chat(chat).unwrap();
    assert!(delivered.is_empty(), "delivered, once");
}

/// The person already told how it went: the woken turn stays silent, and the
/// conversation is as it was (Gemma 4 and Qwen 3.8 were silent 5 of 5, §3.5).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn followup_already_answered_stays_silent_e2e_live() {
    // Yesterday at 12:05 the person told how it went.
    let told = Duration::hours(14) + Duration::minutes(55);
    let messages = vec![
        at(Duration::zero(), INTERVIEW, true),
        at(Duration::seconds(30), PLAN_REPLY, false),
        at(
            told,
            "It's done!! They want me for a second round next week. I'm so relieved.",
            true,
        ),
        at(
            told + Duration::seconds(30),
            "That's wonderful — a second round means they liked you. Enjoy the relief today, \
             you earned it.",
            false,
        ),
    ];
    let Some(mut live) = start(messages, Some(about())).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let reply = woken_reply(&mut live.evt_rx).await;
    let (chat, planted) = (live.chat, live.planted.unwrap());
    let (_dir, root) = live.quit().await;
    let storage = Storage::open(Paths::with_root(&root)).unwrap();
    let stored = storage.json().load_chat(chat).unwrap().unwrap();
    // A new event the person mentioned — the second round — may be planned in
    // the same turn (DeepSeek v4.1 flash did): that is a follow-up of its own.
    let new = storage.db().followups_pending_in_chat(chat).unwrap();
    eprintln!(
        "woken reply: {reply}
planned meanwhile: {new:?}"
    );
    assert!(
        crate::features::followups::is_silent(&reply),
        "the person already told — silence expected: {reply}"
    );
    assert_eq!(
        storage.db().followup_get(planted).unwrap().unwrap().state,
        crate::entities::followup::FollowupState::Delivered
    );
    // No trace of the silence: no reply text after the four rows the conversation
    // had, and nothing at all unless the turn acted — a tool call stays, with the
    // notice that explains it.
    let added = &stored.messages[4..];
    assert!(
        added.iter().all(
            |m| m.role != crate::entities::message::MessageRole::Assistant
                || m.text.trim().is_empty()
        ),
        "no reply text: {added:?}"
    );
    assert!(
        added.is_empty() || added.iter().any(|m| !m.tool_calls.is_empty()),
        "no trace: {added:?}"
    );
}
