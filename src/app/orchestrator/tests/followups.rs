//! Orchestrator tests — the companion's follow-ups (spec §9.3.6;
//! docs/research/companion-platform.md C4 (a)): a due one is delivered once as a
//! notice row and wakes the open conversation; its `<silent>` leaves no trace;
//! quiet hours, the day's cap and the profile's switch hold it back; a
//! conversation not in view waits for the person; a take-back cancels what its
//! exchange planned. Part of the [`super`] module (fixtures in mod.rs).

use super::*;
use crate::entities::followup::{Followup, FollowupState};

const NOTICE: &str = "[Follow-up — not a message from the user]";
const SILENCE: &str = "reply with exactly <silent>";

/// An engine that answers every request with one reply and keeps them all.
struct Replying {
    reply: String,
    requests: std::sync::Mutex<Vec<crate::shared::api::ChatRequest>>,
}

impl Replying {
    fn new(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            reply: reply.into(),
            requests: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<crate::shared::api::ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl EngineBackend for Replying {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::shared::api::contract::ChatStream> {
        self.requests.lock().unwrap().push(req);
        let reply = self.reply.clone();
        let s = async_stream::stream! {
            yield ChatChunk::Text(reply);
            yield ChatChunk::Finished(crate::shared::api::FinishReason::Stop);
        };
        Ok(Box::pin(s))
    }
}

/// The profile, the conversation the follow-up returns to (opened at start) and
/// another one.
struct World {
    profile: Uuid,
    interview: Uuid,
    other: Uuid,
}

/// A companion profile — the switch as given, no quiet hours unless a test sets
/// them, so the hour the test runs at never holds a delivery back — and two
/// conversations: the interview one, opened at start, and another.
fn seed(root: &std::path::Path, switch: bool, edit: impl FnOnce(&mut Profile)) -> World {
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(root));
    let mut profile = Profile::new("Companion", "You are Sol.");
    profile.language = crate::shared::i18n::Lang::En;
    profile.enabled_tools = crate::features::tools::default_tool_ids();
    profile.known_tools = profile.enabled_tools.clone();
    profile.companion.followups = switch;
    profile.companion.quiet_from = 0;
    profile.companion.quiet_until = 0;
    edit(&mut profile);
    json.upsert_profile(&profile).unwrap();
    let hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);
    let mut other = Chat::from_profile(&profile, "Other");
    other.modified_at = hour_ago - chrono::Duration::hours(1);
    json.save_chat(&other).unwrap();
    let mut chat = Chat::from_profile(&profile, "Interview");
    let mut u = Message::user("Tomorrow at 10 is the interview, I'm nervous");
    u.timestamp = hour_ago;
    chat.push_message(u);
    let mut a = Message::assistant("You'll do great.");
    a.timestamp = hour_ago;
    chat.push_message(a);
    chat.modified_at = hour_ago;
    json.save_chat(&chat).unwrap();
    World {
        profile: profile.id,
        interview: chat.id,
        other: other.id,
    }
}

/// A pending follow-up of `chat`, due `minutes` from now (negative: overdue).
fn plant(root: &std::path::Path, w: &World, chat: Uuid, minutes: i64) -> Uuid {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    let mut f = Followup::new(
        w.profile,
        chat,
        "Ask how the interview went",
        chrono::Utc::now() + chrono::Duration::minutes(minutes),
    );
    // Planned before the exchange the conversation ends on (an hour ago).
    f.created_at = chrono::Utc::now() - chrono::Duration::minutes(90);
    storage.db().followup_insert(&f).unwrap();
    f.id
}

fn state(root: &std::path::Path, id: Uuid) -> FollowupState {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    storage.db().followup_get(id).unwrap().unwrap().state
}

fn chat_on_disk(root: &std::path::Path, id: Uuid) -> Chat {
    let storage = Storage::open(Paths::with_root(root)).unwrap();
    storage.json().load_chat(id).unwrap().unwrap()
}

async fn quit(cmd_tx: UnboundedSender<AppCommand>, handle: tokio::task::JoinHandle<()>) {
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// Waits for the start-up's activation, then lets the loop settle — for tests
/// where nothing more is expected to happen.
async fn settle(evt_rx: &mut UnboundedReceiver<AppEvent>) {
    let _ = wait_for(evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    pause().await;
}

/// Lets the loop run whatever it was handed.
async fn pause() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_due_follow_up_is_delivered_once_and_wakes_the_open_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |_| {});
    let id = plant(dir.path(), &w, w.interview, -1);
    let backend = Replying::new("Hey — how did the interview go?");
    let (cmd_tx, mut evt_rx, handle) =
        spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    // The woken request ends with the notice standing alone, and the choice to
    // write first or stay silent after it.
    let req = backend.requests().pop().unwrap();
    let last = req.messages.last().unwrap();
    assert_eq!(last.role, crate::shared::api::contract::ApiRole::User);
    assert!(last.content.starts_with(NOTICE), "{}", last.content);
    assert!(
        last.content.contains("«Ask how the interview went»"),
        "{}",
        last.content
    );
    assert!(
        last.content.ends_with("the user will not see it."),
        "{}",
        last.content
    );
    // The follow-up tools are offered with the switch on.
    assert!(
        req.tools.iter().any(|t| t.name == "followup_plan"),
        "{:?}",
        req.tools
    );

    quit(cmd_tx, handle).await;
    assert_eq!(state(dir.path(), id), FollowupState::Delivered);
    let chat = chat_on_disk(dir.path(), w.interview);
    let n = chat.messages.len();
    assert!(chat.messages[n - 2].is_followup_notice());
    // The stored row is the notice alone — the choice is added on the wire only.
    assert!(!chat.messages[n - 2].text.contains(SILENCE));
    assert_eq!(chat.messages[n - 1].text, "Hey — how did the interview go?");
    assert_eq!(backend.requests().len(), 1, "one wake, once");
}

#[tokio::test]
async fn silence_leaves_no_trace() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |_| {});
    let id = plant(dir.path(), &w, w.interview, -1);
    let backend = Replying::new("<silent>");
    let (cmd_tx, mut evt_rx, handle) =
        spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    pause().await;
    quit(cmd_tx, handle).await;
    assert_eq!(state(dir.path(), id), FollowupState::Delivered);
    let chat = chat_on_disk(dir.path(), w.interview);
    assert_eq!(chat.messages.len(), 2, "{:?}", chat.messages);
    assert_eq!(chat.messages[1].text, "You'll do great.");
    assert_eq!(backend.requests().len(), 1, "silence is not asked again");
}

#[tokio::test]
async fn a_conversation_not_in_view_is_marked_unread_and_nobody_is_woken() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |_| {});
    let id = plant(dir.path(), &w, w.other, -1);
    let backend = Replying::new("ok");
    let (cmd_tx, mut evt_rx, handle) =
        spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
    settle(&mut evt_rx).await;
    quit(cmd_tx, handle).await;
    assert!(backend.requests().is_empty(), "no reply into the void");
    assert_eq!(state(dir.path(), id), FollowupState::Delivered);
    let chat = chat_on_disk(dir.path(), w.other);
    assert!(chat.unread);
    assert!(chat.messages.last().unwrap().is_followup_notice());
}

#[tokio::test]
async fn a_conversation_not_in_view_is_woken_when_the_person_opens_it() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |_| {});
    plant(dir.path(), &w, w.other, -1);
    let backend = Replying::new("How did it go?");
    let (cmd_tx, mut evt_rx, handle) =
        spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
    settle(&mut evt_rx).await;
    assert!(backend.requests().is_empty(), "no reply into the void");
    cmd_tx.send(AppCommand::SwitchChat(w.other)).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let last = backend.requests().pop().unwrap();
    assert!(last.messages.last().unwrap().content.starts_with(NOTICE));
    quit(cmd_tx, handle).await;
    let chat = chat_on_disk(dir.path(), w.other);
    assert!(!chat.unread, "opening it read it");
    assert_eq!(chat.messages.last().unwrap().text, "How did it go?");
}

#[tokio::test]
async fn the_switch_quiet_hours_and_the_cap_each_hold_a_due_follow_up() {
    let now = chrono::Local::now();
    let this_hour = chrono::Timelike::hour(&now) as u8;
    type Edit = fn(&mut Profile, u8);
    let cases: [(&str, bool, Edit); 3] = [
        ("switch off", false, |_, _| {}),
        // Quiet hours are judged at the follow-up's due moment, a minute ago,
        // which in an hour's first minute is the hour before (runs at 04:00:35
        // and 12:00:24 delivered it), so they start an hour back; and they end
        // two hours on, so a run that starts as the hour turns is still inside
        // them (a full run at 23:59:5x once delivered it).
        ("quiet hours", true, |p, h| {
            p.companion.quiet_from = (h + 23) % 24;
            p.companion.quiet_until = (h + 2) % 24;
        }),
        ("cap reached", true, |p, _| {
            p.companion.followups_per_day = 1
        }),
    ];
    for (what, switch, edit) in cases {
        let dir = tempfile::tempdir().unwrap();
        let w = seed(dir.path(), switch, |p| edit(p, this_hour));
        if what == "cap reached" {
            // One delivered already on the day the follow-up falls due, and one
            // today. The cap is counted on the due day first, and in a day's
            // first minute "a minute ago" is yesterday; the delivery that day
            // holds it, today's holds the step to the next day.
            let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
            let now = chrono::Utc::now();
            for at in [now - chrono::Duration::minutes(1), now] {
                let f = Followup::new(w.profile, w.interview, "earlier", at);
                storage.db().followup_insert(&f).unwrap();
                storage
                    .db()
                    .followup_close(f.id, FollowupState::Delivered, at)
                    .unwrap();
            }
        }
        let id = plant(dir.path(), &w, w.interview, -1);
        let backend = Replying::new("hi");
        let (cmd_tx, mut evt_rx, handle) =
            spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
        settle(&mut evt_rx).await;
        quit(cmd_tx, handle).await;
        assert_eq!(state(dir.path(), id), FollowupState::Pending, "{what}");
        assert!(backend.requests().is_empty(), "{what}");
        assert_eq!(
            chat_on_disk(dir.path(), w.interview).messages.len(),
            2,
            "{what}"
        );
    }
}

#[tokio::test]
async fn taking_an_exchange_back_cancels_what_it_planned() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |_| {});
    let backend = Replying::new("ok");
    let (cmd_tx, mut evt_rx, handle) =
        spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
    settle(&mut evt_rx).await;
    // An earlier plan, and one the exchange about to be taken back made.
    let earlier = plant(dir.path(), &w, w.interview, 600);
    cmd_tx
        .send(AppCommand::SendMessage("remind me tomorrow".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    let planned = Followup::new(
        w.profile,
        w.interview,
        "the reminder",
        chrono::Utc::now() + chrono::Duration::days(1),
    );
    storage.db().followup_insert(&planned).unwrap();
    cmd_tx.send(AppCommand::DeleteLastExchange).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::RestoreInput(_)))
        .await
        .unwrap();
    quit(cmd_tx, handle).await;
    assert_eq!(state(dir.path(), planned.id), FollowupState::Cancelled);
    assert_eq!(state(dir.path(), earlier), FollowupState::Pending);
}

#[tokio::test]
async fn the_tasks_screen_lists_a_pending_follow_up_and_cancels_it() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |_| {});
    let id = plant(dir.path(), &w, w.interview, 600);
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), None, no_auto_cfg());
    settle(&mut evt_rx).await;
    cmd_tx.send(AppCommand::RequestTasks).unwrap();
    let Some(AppEvent::TaskList(list)) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::TaskList(_))).await
    else {
        panic!("no task list");
    };
    assert_eq!(list.followups.len(), 1);
    assert_eq!(list.followups[0].id, id);
    assert_eq!(list.followups[0].chat_title, "Interview");
    assert!(!list.followups[0].paused);
    cmd_tx.send(AppCommand::CancelFollowup(id)).unwrap();
    let Some(AppEvent::TaskList(list)) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::TaskList(_))).await
    else {
        panic!("no task list");
    };
    assert!(list.followups.is_empty());
    quit(cmd_tx, handle).await;
    assert_eq!(state(dir.path(), id), FollowupState::Cancelled);
}

#[tokio::test]
async fn deleting_the_conversation_cancels_its_follow_ups() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |_| {});
    let id = plant(dir.path(), &w, w.other, 600);
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), None, no_auto_cfg());
    settle(&mut evt_rx).await;
    cmd_tx.send(AppCommand::DeleteChat(w.other)).unwrap();
    pause().await;
    quit(cmd_tx, handle).await;
    assert_eq!(state(dir.path(), id), FollowupState::Cancelled);
}

#[tokio::test]
async fn turning_the_switch_on_delivers_what_was_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), false, |_| {});
    let id = plant(dir.path(), &w, w.interview, -1);
    let backend = Replying::new("How did it go?");
    let (cmd_tx, mut evt_rx, handle) =
        spawn_orch_at(dir.path(), Some(backend.clone()), no_auto_cfg());
    settle(&mut evt_rx).await;
    assert!(backend.requests().is_empty());
    let companion = crate::entities::profile::CompanionFeatures {
        followups: true,
        quiet_from: 0,
        quiet_until: 0,
        ..Default::default()
    };
    let edit = crate::features::profiles::ProfileEdit {
        companion: Some(companion),
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: w.profile,
            edit: Box::new(edit),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    quit(cmd_tx, handle).await;
    assert_eq!(state(dir.path(), id), FollowupState::Delivered);
    assert_eq!(backend.requests().len(), 1);
}

// ---- notifications for someone not looking (spec §11.12) ----

/// The test config with the interface in English — the language a notification
/// is worded in (axis B).
fn en_cfg() -> AppConfig {
    let mut cfg = no_auto_cfg();
    cfg.interface.language = crate::shared::i18n::Lang::En;
    cfg
}

/// Every event the loop has sent so far, drained.
fn drain(evt_rx: &mut UnboundedReceiver<AppEvent>) -> Vec<AppEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = evt_rx.try_recv() {
        out.push(ev);
    }
    out
}

fn attention(events: &[AppEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::TerminalAttention(seq) => Some(seq.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_woken_reply_notifies_when_the_terminal_is_not_known_to_be_in_focus() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |p| p.companion.notify = true);
    plant(dir.path(), &w, w.interview, -1);
    let backend = Replying::new("Hey — how did the interview go?");
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), Some(backend), en_cfg());
    let Some(AppEvent::TerminalAttention(seq)) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::TerminalAttention(_))).await
    else {
        panic!("no notification");
    };
    // OSC 777 with the profile and the conversation — not the message's words,
    // which stay out by default — and the bell after it.
    assert_eq!(
        seq,
        "\x1b]777;notify;Companion · mindfork;Wrote to you in «Interview»\x07\x07"
    );
    quit(cmd_tx, handle).await;
}

#[tokio::test]
async fn a_reply_the_person_is_looking_at_or_a_profile_that_did_not_ask_sends_nothing() {
    for (notify, focused) in [(true, true), (false, false)] {
        let dir = tempfile::tempdir().unwrap();
        let w = seed(dir.path(), true, |p| p.companion.notify = notify);
        plant(dir.path(), &w, w.other, -1);
        let backend = Replying::new("How did it go?");
        let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), Some(backend), en_cfg());
        settle(&mut evt_rx).await;
        cmd_tx.send(AppCommand::TerminalFocus(focused)).unwrap();
        // Opening the conversation wakes the companion in front of the person.
        drain(&mut evt_rx);
        cmd_tx.send(AppCommand::SwitchChat(w.other)).unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        pause().await;
        let seen = attention(&drain(&mut evt_rx));
        assert!(
            seen.is_empty(),
            "notify={notify} focused={focused}: {seen:?}"
        );
        quit(cmd_tx, handle).await;
    }
}

/// A log writer the test reads back.
#[derive(Clone, Default)]
struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl LogBuf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// The log says what a notification did — sent, and how, or held because the
/// person was looking — so one that never appeared on screen is told from one
/// that was never sent (spec §11.12; the owner's look on Windows Terminal 1.24,
/// which ignores OSC 777, found nothing in the log either way).
#[tokio::test]
async fn the_log_says_whether_a_notification_was_sent_or_held() {
    let log = LogBuf::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    // tracing decides once, for every thread, whether a log line is wanted; while
    // a single scoped subscriber is alive it asks only the default of the thread
    // that reaches the line first, so a parallel test's thread, with none, would
    // switch these lines off here too. A second one kept alive makes it ask all.
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    // The orchestrator runs on this test's thread (a current-thread runtime).
    let _guard = tracing::subscriber::set_default(subscriber);

    // No focus report yet: sent, with how.
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |p| p.companion.notify = true);
    plant(dir.path(), &w, w.interview, -1);
    let backend = Replying::new("Hey — how did the interview go?");
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), Some(backend), en_cfg());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::TerminalAttention(_)))
        .await
        .expect("no notification");
    quit(cmd_tx, handle).await;
    let out = log.text();
    let sent = out
        .lines()
        .find(|l| l.contains("notification sent"))
        .unwrap_or_else(|| panic!("no sent line: {out}"));
    for field in [
        "kind=\"reply\"",
        "escape=\"osc777\"",
        "bell=true",
        "push=false",
        "focused=None",
    ] {
        assert!(sent.contains(field), "{field} missing: {sent}");
    }

    // In focus, the conversation open: held — and the focus report is logged.
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |p| p.companion.notify = true);
    plant(dir.path(), &w, w.other, -1);
    let backend = Replying::new("How did it go?");
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), Some(backend), en_cfg());
    settle(&mut evt_rx).await;
    cmd_tx.send(AppCommand::TerminalFocus(true)).unwrap();
    cmd_tx.send(AppCommand::SwitchChat(w.other)).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    pause().await;
    quit(cmd_tx, handle).await;
    let out = log.text();
    assert!(
        out.lines()
            .any(|l| l.contains("terminal focus") && l.contains("focused=true")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.contains("notification held") && l.contains("kind=\"reply\"")),
        "{out}"
    );
}

/// A follow-up that comes due in a conversation the person is not in is said
/// whatever the focus: they are elsewhere in the app, or away from it.
#[tokio::test]
async fn a_follow_up_waiting_elsewhere_notifies_even_in_focus() {
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |p| p.companion.notify = true);
    let mut cfg = en_cfg();
    cfg.notifications.escape = crate::shared::notify::NotifyEscape::Osc9;
    cfg.notifications.bell = false;
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), None, cfg);
    settle(&mut evt_rx).await;
    cmd_tx.send(AppCommand::TerminalFocus(true)).unwrap();
    pause().await;
    plant(dir.path(), &w, w.other, -1);
    // A turn's end looks at the follow-ups again; a settings edit of the profile
    // does too — the cheapest way to make the loop look now.
    let edit = crate::features::profiles::ProfileEdit {
        name: Some("Companion".into()),
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: w.profile,
            edit: Box::new(edit),
        })
        .unwrap();
    let Some(AppEvent::TerminalAttention(seq)) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::TerminalAttention(_))).await
    else {
        panic!("no notification");
    };
    assert_eq!(
        seq,
        "\x1b]9;Companion · mindfork: Has something for you in «Other»\x07"
    );
    quit(cmd_tx, handle).await;
}

/// The push: a `POST` to the topic with the title in the query — the message's
/// words in it only when the settings say so.
#[tokio::test]
async fn a_woken_reply_is_pushed_to_the_ntfy_topic_with_its_words_when_asked() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut got = Vec::new();
        let mut buf = vec![0u8; 4096];
        while !String::from_utf8_lossy(&got).ends_with("how did it go?") {
            let n = sock.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
            .await
            .unwrap();
        String::from_utf8_lossy(&got).to_string()
    });
    let dir = tempfile::tempdir().unwrap();
    let w = seed(dir.path(), true, |p| p.companion.notify = true);
    plant(dir.path(), &w, w.interview, -1);
    let mut cfg = en_cfg();
    cfg.notifications.ntfy_url = format!("http://{addr}/mindfork-test");
    cfg.notifications.with_text = true;
    let backend = Replying::new("Hey Alex, how did it go?");
    let (cmd_tx, _evt_rx, handle) = spawn_orch_at(dir.path(), Some(backend), cfg);
    let request = tokio::time::timeout(std::time::Duration::from_secs(20), server)
        .await
        .expect("the push arrives")
        .unwrap();
    assert!(
        request.starts_with("POST /mindfork-test?title=Companion+%C2%B7+mindfork HTTP/1.1"),
        "{request}"
    );
    assert!(request.ends_with("Hey Alex, how did it go?"), "{request}");
    quit(cmd_tx, handle).await;
}
