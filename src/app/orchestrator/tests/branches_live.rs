//! Live smoke of a forked line — docs/research/chat-branches.md §6, stage 2.
//! Part of the [`super`] module.
//!
//! What only a real model can answer: that a turn on a forked line is asked
//! from what the line holds and nothing else. The parked tail is not in the
//! request, and neither is the summary that described it; the kept prefix is
//! there verbatim; and the tail, put back, is the conversation it was.
//!
//! One chat, one script, every step a turn against the engine:
//!
//! 1. Two facts are planted in two exchanges, and a **control** asks for the
//!    second while it is on the line — a model that cannot answer then says
//!    nothing about what follows (lessons §2).
//! 2. Filler turns push both behind the verbatim tail and the conversation is
//!    compacted: the second fact is now reachable through the summary too.
//! 3. A fork at the second fact's message — **before the summary's boundary**.
//!    The summary describes what the line no longer holds and must go with
//!    the tail.
//! 4. The second fact is asked for again: the answer must not hold it. The
//!    first is asked for: the answer must, from the prefix sent verbatim.
//! 5. The new line is compacted in its turn.
//! 6. A switch back: the summary returns with its tail, and the second fact
//!    is answered again.
//!
//! The first turn after the fork logs its cache share. On a local server —
//! one slot, the same prefix — it must have read some; a cloud's hit is the
//! provider's routing, logged and not asserted.
//!
//! Stage 3 (§3.10) adds a second script,
//! [`a_reflection_after_a_regeneration_reads_the_signal_e2e_live`]: `Ctrl+R`
//! parks the reply it replaces instead of archiving it, and the reflection
//! that follows must still be told of it — and must still land, on a real
//! model, with that line in its prompt.
//!
//! Stage 4 (§9) adds a third,
//! [`a_parked_branch_is_read_when_asked_and_left_alone_when_not_e2e_live`]:
//! a model handed `branch_read` reads a parked tail when the person asks
//! about it, and leaves the tails alone when nobody did.
//!
//! The search of parked branches (§10.2) adds a fourth,
//! [`a_parked_word_is_found_when_asked_and_left_alone_when_not_e2e_live`]:
//! a model handed `branch_search` finds the variant that said a word no
//! first line shows, and searches nothing when nobody asked.

use super::*;
use crate::entities::chat_branch::ParkCause;
use crate::shared::config::CompactionSettings;
use crate::shared::i18n::{Lang, locale};

const FIRST: &str = "HERON-17";
const SECOND: &str = "ZANZIBAR-42";

/// The typographic dashes a model may render a code with, folded to `-`: the
/// fact under test is the code, not the glyph.
fn holds(text: &str, code: &str) -> bool {
    let fold = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{FE58}' | '\u{FE63}' | '\u{FF0D}' => '-',
                other => other,
            })
            .collect()
    };
    fold(text).contains(&fold(code))
}

/// One turn: what was sent, what came back, and what it cost.
struct Turn {
    /// The stored message's id — what a fork is asked for by.
    sent: Uuid,
    reply: String,
    context: Option<u64>,
    cached: Option<u32>,
}

async fn turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> Turn {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = Turn {
        sent: Uuid::nil(),
        reply: String::new(),
        context: None,
        cached: None,
    };
    while let Some(event) = evt_rx.recv().await {
        match event {
            AppEvent::UserMessage { id, .. } => out.sent = id.expect("a stored message"),
            AppEvent::Chunk { text, .. } => out.reply.push_str(&text),
            AppEvent::TokenUsage {
                context, cached, ..
            } => {
                if context.is_some() {
                    out.context = context;
                    out.cached = cached;
                }
            }
            AppEvent::Finished { reason, .. } => {
                eprintln!(
                    "turn [{reason:?}] {text:?}\n  -> {}",
                    out.reply.trim().chars().take(160).collect::<String>()
                );
                break;
            }
            AppEvent::Error(message) => eprintln!("  engine note: {message}"),
            _ => {}
        }
    }
    assert!(!out.sent.is_nil(), "the turn was never echoed");
    out
}

async fn compact(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
) -> (String, usize) {
    cmd_tx.send(AppCommand::Compact).unwrap();
    let landed = wait_for(evt_rx, |e| {
        matches!(
            e,
            AppEvent::Compacted { .. } | AppEvent::Error(_) | AppEvent::Notice(_)
        )
    })
    .await
    .unwrap();
    let AppEvent::Compacted {
        summary, folded, ..
    } = landed
    else {
        panic!("the conversation was not compacted: {landed:?}");
    };
    eprintln!(
        "compacted {folded} messages into {} characters",
        summary.chars().count()
    );
    (summary, folded)
}

const FILLERS: [&str; 3] = [
    "In two sentences: why do databases have indexes?",
    "In two sentences: how does a cache differ from a buffer?",
    "In two sentences: what makes a request idempotent?",
];

#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn a_forked_line_is_asked_from_what_it_holds_e2e_live() {
    let mut config = no_auto_cfg();
    config.compaction = CompactionSettings {
        enabled: true,
        summary_words: 200,
        // A small verbatim tail, so a handful of exchanges is enough to push
        // the planted facts behind the boundary.
        tail_tokens: 120,
        ..Default::default()
    };
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // No tools: the answers must come from the request, and the summary must
    // be the one route to what was compacted away.
    let profile = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .and_then(|e| match e {
            AppEvent::ProfileList(ps) => ps.first().map(|p| p.id),
            _ => None,
        })
        .expect("the bootstrap profile");
    let chat_id = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .expect("the bootstrap chat");
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(Vec::new()),
                ..Default::default()
            }),
        })
        .unwrap();

    // 1. The two facts, and the control.
    turn(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Remember this for our conversation: the project's codename is {FIRST}. \
             Confirm in one short sentence."
        ),
    )
    .await;
    let second = turn(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "A second fact to remember: the launch password is {SECOND}. \
             Confirm in one short sentence."
        ),
    )
    .await;
    let control = turn(
        &cmd_tx,
        &mut evt_rx,
        "What is the launch password? Answer with the password only.",
    )
    .await;
    assert!(
        holds(&control.reply, SECOND),
        "control failed — the model cannot answer with the fact on the line, so this \
         run says nothing about a fork: {}",
        control.reply
    );

    // 2. Both facts go behind the boundary.
    for filler in FILLERS {
        turn(&cmd_tx, &mut evt_rx, filler).await;
    }
    let (summary, folded) = compact(&cmd_tx, &mut evt_rx).await;
    assert!(
        folded >= 4,
        "both facts' exchanges must be behind the boundary, folded {folded}"
    );
    eprintln!(
        "the summary holds the password: {}",
        holds(&summary, SECOND)
    );

    // 3. The fork, before the boundary.
    cmd_tx
        .send(AppCommand::ForkAt {
            message: second.sent,
        })
        .unwrap();
    let restored = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::RestoreInput(_)))
        .await
        .unwrap();
    assert!(
        matches!(&restored, AppEvent::RestoreInput(t) if holds(t, SECOND)),
        "the person's message returns to be written again: {restored:?}"
    );

    // 4. The line is asked from what it holds.
    let blind = turn(
        &cmd_tx,
        &mut evt_rx,
        "What is the launch password? If nobody has told you one in this conversation, \
         answer exactly: UNKNOWN.",
    )
    .await;
    eprintln!(
        "the first turn after the fork: context {:?}, cached {:?}",
        blind.context, blind.cached
    );
    assert!(
        !holds(&blind.reply, SECOND),
        "the parked tail or its summary reached the request: {}",
        blind.reply
    );
    let local = std::env::var("MINDFORK_ENGINE_URL")
        .is_ok_and(|u| u.contains("127.0.0.1") || u.contains("localhost"));
    if local {
        assert!(
            blind.cached.is_some_and(|c| c > 0),
            "a local server keeps the prefix a fork leaves: {:?}",
            blind.cached
        );
    }
    let kept = turn(
        &cmd_tx,
        &mut evt_rx,
        "What is the project's codename? Answer with the codename only.",
    )
    .await;
    assert!(
        holds(&kept.reply, FIRST),
        "the prefix the fork kept is sent verbatim: {}",
        kept.reply
    );

    // 5. The new line compacts in its turn.
    for filler in FILLERS {
        turn(&cmd_tx, &mut evt_rx, filler).await;
    }
    let (resummary, refolded) = compact(&cmd_tx, &mut evt_rx).await;
    assert!(refolded >= 2, "{refolded}");
    assert!(
        !holds(&resummary, SECOND),
        "the new line's summary is of the new line: {resummary}"
    );

    // 6. The tail returns, with its summary.
    cmd_tx
        .send(AppCommand::SwitchBranch { first: second.sent })
        .unwrap();
    let back = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated {
        messages,
        compaction,
        ..
    } = back
    else {
        unreachable!()
    };
    assert_eq!(
        messages[2].id, second.sent,
        "the second fact is on the line"
    );
    assert!(
        compaction.is_some(),
        "the tail's own summary is in force again"
    );
    let again = turn(
        &cmd_tx,
        &mut evt_rx,
        "Once more: what is the launch password? Answer with the password only.",
    )
    .await;
    assert!(
        holds(&again.reply, SECOND),
        "the tail is back as it was left: {}",
        again.reply
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let stored = Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.v, crate::shared::storage::schema::CHAT_SCHEMA);
    assert_eq!(stored.branches.len(), 1, "the line the switch displaced");
    assert_eq!(stored.branches[0].cause, ParkCause::Switch);
    assert_eq!(stored.branches[0].after, Some(stored.messages[1].id));
    assert!(
        stored.branches[0].compaction.is_some(),
        "the displaced line keeps its own summary"
    );
    eprintln!(
        "GO: the line holds {} messages, the parked tail {}",
        stored.messages.len(),
        stored.branches[0].messages.len()
    );
}

/// A live engine that also keeps every request it was handed — what a
/// background task asked the model is seen nowhere else: a reflection is
/// silent, and its prompt is not an event.
struct Recording {
    inner: Arc<dyn EngineBackend>,
    asked: std::sync::Mutex<Vec<String>>,
}

impl Recording {
    fn over(inner: Arc<dyn EngineBackend>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The line of the first request that holds one of `needles`.
    fn told_of(&self, needles: &[String]) -> Option<String> {
        let asked = self.asked.lock().unwrap();
        asked
            .iter()
            .flat_map(|request| request.lines())
            .find(|line| needles.iter().any(|n| line.contains(n.as_str())))
            .map(str::to_string)
    }
}

#[async_trait::async_trait]
impl EngineBackend for Recording {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::shared::api::contract::ChatStream> {
        let mut told = req.system.clone().unwrap_or_default();
        for m in &req.messages {
            told.push('\n');
            told.push_str(&m.content);
        }
        self.asked.lock().unwrap().push(told);
        self.inner.chat_stream(req, cancel).await
    }

    async fn context_budget(&self) -> Option<u32> {
        self.inner.context_budget().await
    }

    async fn model_capabilities(&self) -> Option<crate::shared::api::contract::ModelCapabilities> {
        self.inner.model_capabilities().await
    }

    async fn vision(&self) -> crate::shared::api::contract::VisionSupport {
        self.inner.vision().await
    }

    async fn model_id(&self) -> Option<String> {
        self.inner.model_id().await
    }

    async fn parallel_slots(&self) -> Option<u32> {
        self.inner.parallel_slots().await
    }

    async fn server_kind(&self) -> crate::shared::api::contract::ServerKind {
        self.inner.server_kind().await
    }

    async fn enforces_output_schema(&self) -> bool {
        self.inner.enforces_output_schema().await
    }
}

/// The reflection task's state in the app's task list, when an event says it.
fn reflection_running(event: &AppEvent) -> Option<bool> {
    let AppEvent::TaskList(list) = event else {
        return None;
    };
    list.app
        .iter()
        .find(|t| t.kind == BackgroundKind::Reflection)
        .map(|t| t.running)
}

/// `Ctrl+R` keeps the reply it replaces as a branch (research §3.10), and the
/// behavioural signal reflection reads moved with it: from the `deleted`
/// archive's records to the parked tails' causes. What only a run against a
/// real engine shows is the whole path — the key, the parked tail, the
/// cadence, the prompt — and that a model given that prompt still does the
/// reflection's work.
///
/// 1. An exchange, and **no** reflection yet (the cadence is two replies): a
///    request that holds the signal now would be a false one.
/// 2. `Ctrl+R`. The reply is parked under `Regenerate`; nothing is archived.
/// 3. A second exchange makes the reflection due. Its request — recorded on
///    the way to the engine — must say the reply was regenerated once.
/// 4. The reflection lands: the task ends, and what it wrote is printed for
///    the go/no-go (a model's wording is not asserted).
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn a_reflection_after_a_regeneration_reads_the_signal_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let Some(live) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let engine = Recording::over(live);
    let mut config = no_auto_cfg();
    config.self_model.auto_reflect_every = 2;
    let (dir, cmd_tx, mut evt_rx, handle) =
        spawn_orch_cfg(Some(engine.clone() as Arc<dyn EngineBackend>), config);
    let root = dir.path().to_path_buf();
    // The reflection's own tools and no others: the gate it checks is the
    // profile's, and the conversation's turns need none.
    let pid = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .and_then(|e| match e {
            AppEvent::ProfileList(ps) => ps.first().map(|p| p.id),
            _ => None,
        })
        .expect("the bootstrap profile");
    let chat_id = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .expect("the bootstrap chat");
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(
                    crate::app::orchestrator::reflection::REFLECT_TOOL_IDS
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                ),
                ..Default::default()
            }),
        })
        .unwrap();
    // The signal is worded in the profile's own language, whichever it is.
    let signals: Vec<String> = [Lang::En, Lang::Ru]
        .into_iter()
        .map(|lang| locale(lang).tf("reflect.behavior.regen", &[("n", "1")]))
        .collect();

    // 1. One exchange: one reply in the window, the reflection not due.
    turn(
        &cmd_tx,
        &mut evt_rx,
        "I am choosing a name for a small sailing boat. Suggest exactly one name, \
         in one short sentence.",
    )
    .await;

    // 2. The reply did not land: another is asked for.
    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    let mut second = String::new();
    while let Some(event) = evt_rx.recv().await {
        match event {
            AppEvent::Chunk { text, .. } => second.push_str(&text),
            AppEvent::Finished { reason, .. } => {
                eprintln!(
                    "regenerated [{reason:?}] -> {}",
                    second.trim().chars().take(160).collect::<String>()
                );
                break;
            }
            _ => {}
        }
    }
    assert_eq!(
        engine.told_of(&signals),
        None,
        "nothing was reflected on yet, so nothing may have been told of the signal"
    );

    // 3. The second exchange makes the reflection due; it is told.
    turn(
        &cmd_tx,
        &mut evt_rx,
        "Thanks, that one is better. In one short sentence: why does it suit a boat?",
    )
    .await;
    let mut told = None;
    for _ in 0..240 {
        told = engine.told_of(&signals);
        if told.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let told = told
        .unwrap_or_else(|| panic!("no request told the model of the regeneration ({signals:?})"));
    eprintln!("the reflection was told: {told}");

    // 4. It lands. The task list says a reflection is running and then that
    //    it is not; an earlier list, from before it was spawned, says "not
    //    running" too, and a quit on that one would stop the reflection and
    //    give its window back. The limit is for a CPU-only engine.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(900), async {
        let mut started = false;
        while let Some(event) = evt_rx.recv().await {
            match reflection_running(&event) {
                Some(true) => started = true,
                Some(false) if started => return true,
                _ => {}
            }
        }
        false
    })
    .await;
    assert!(matches!(ended, Ok(true)), "the reflection did not end");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let storage = Storage::open(Paths::with_root(&root)).unwrap();
    let model = storage.db().self_model_get(pid).unwrap();
    let observations = storage
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    eprintln!(
        "the reflection wrote: self-model = {model:#?}\nobservations = {:#?}",
        observations.iter().map(|n| &n.content).collect::<Vec<_>>()
    );
    let stored = storage.json().load_chat(chat_id).unwrap().unwrap();
    assert!(stored.deleted.is_empty(), "nothing is archived");
    let causes: Vec<ParkCause> = stored.branches.iter().map(|b| b.cause).collect();
    assert_eq!(causes, [ParkCause::Regenerate]);
    assert_eq!(stored.branches[0].after, Some(stored.messages[0].id));
    // Two exchanges, as the feed draws them: a reply that looked something up
    // on its way is still one bubble.
    assert_eq!(
        crate::entities::chat::visible_message_count(&stored.messages),
        4,
        "{:?}",
        stored
            .messages
            .iter()
            .map(|m| (m.role, m.text.chars().take(40).collect::<String>()))
            .collect::<Vec<_>>()
    );
    assert!(stored.reflected_at.is_some(), "the reflection was spawned");
}

/// One turn of the reader's smoke: the reply, and the tools the turn called.
struct Asked {
    reply: String,
    tools: Vec<String>,
}

impl Asked {
    fn called(&self, tool: &str) -> bool {
        self.tools.iter().any(|t| t == tool)
    }
}

/// Collects a turn that has already been started — by a message or by
/// `Ctrl+R` — to its end.
async fn collect(evt_rx: &mut UnboundedReceiver<AppEvent>, what: &str) -> Asked {
    let mut out = Asked {
        reply: String::new(),
        tools: Vec::new(),
    };
    while let Some(event) = evt_rx.recv().await {
        match event {
            AppEvent::Chunk { text, .. } => out.reply.push_str(&text),
            AppEvent::ToolCall {
                name,
                arguments,
                result,
                ..
            } => {
                // What was asked for and how the answer began: a run's log is
                // where a second call to one tool is explained.
                eprintln!(
                    "  call: {name} {arguments}\n     => {}",
                    result
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect::<String>()
                );
                out.tools.push(name);
            }
            AppEvent::Finished { reason, .. } => {
                eprintln!(
                    "turn [{reason:?}] {what:?}\n  tools: {:?}\n  -> {}",
                    out.tools,
                    out.reply.trim().chars().take(240).collect::<String>()
                );
                break;
            }
            AppEvent::Error(message) => eprintln!("  engine note: {message}"),
            _ => {}
        }
    }
    out
}

async fn ask(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> Asked {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    collect(evt_rx, text).await
}

/// Stage 4 (research §9): the branches told to the tools, and their reader.
/// What only a real model shows is the judgement the design rests on — that
/// a model handed `branch_read` reads a parked tail **when asked to** and
/// leaves the tails alone when it is not.
///
/// The chats are put on disk before the app starts, so the parked words are
/// the test's own: "boat", whose only reply was redone — the first variant,
/// with the password, is parked; the one on the line says it does not know —
/// and "dock", forked after its first exchange, the fuel's berth in the tail.
///
/// 1. *Left alone:* an unrelated question, then `Ctrl+R` on its reply. No
///    call to the reader in either turn.
/// 2. *Read when asked:* the earlier variant of the reply about the password
///    is asked for. The turn calls the reader and answers with the password —
///    which is in no request until the reader's answer brings it.
/// 3. *Told by a read:* asked what the other chat says about the dock, the
///    turn reads its line and goes no further — the notice under the read
///    names the branches, and nobody asked about them. Asked then for the
///    branch, it reads it and answers from the tail.
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn a_parked_branch_is_read_when_asked_and_left_alone_when_not_e2e_live() {
    use crate::features::tools::branches::BRANCH_READ_ID;
    use crate::features::tools::chats::CHAT_READ_ID;
    const BERTH: &str = "KESTREL-9";
    let Some(live) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let hour_ago = |minutes: i64| {
        chrono::Utc::now() - chrono::Duration::hours(1) + chrono::Duration::minutes(minutes)
    };
    let said = |mut m: Message, minutes: i64| {
        m.timestamp = hour_ago(minutes);
        m
    };
    let boat_id;
    {
        let json = crate::shared::storage::JsonStore::new(Paths::with_root(root));
        let mut profile = Profile::new("Assistant", "You are a helpful assistant.");
        profile.language = Lang::En;
        // The reader, the read it reaches another chat through, and two
        // tools that have nothing to do with either: "left alone" is a choice
        // only where there is something else to call. No `chat_search`: a
        // model that has it answers "where is the dock" from a hit, and a
        // search says nothing of branches (research §9.6, G1) — the notice
        // under test is the read's.
        let tools: Vec<String> = [BRANCH_READ_ID, CHAT_READ_ID, "calculate", "current_time"]
            .into_iter()
            .map(String::from)
            .collect();
        profile.enabled_tools = tools.clone();
        profile.known_tools = tools;
        json.upsert_profile(&profile).unwrap();

        let mut dock = Chat::from_profile(&profile, "dock");
        for (i, m) in [
            Message::user("Where is the dock?"),
            Message::assistant("The dock is north of the old pier."),
            Message::user("And where do I get fuel?"),
            Message::assistant(format!("Fuel is sold at berth {BERTH}.")),
        ]
        .into_iter()
        .enumerate()
        {
            dock.push_message(said(m, i as i64));
        }
        assert!(dock.fork_at(2, hour_ago(10)));
        dock.modified_at = hour_ago(10);
        json.save_chat(&dock).unwrap();

        let mut boat = Chat::from_profile(&profile, "boat");
        boat.push_message(said(
            Message::user("What is the boat's launch password?"),
            20,
        ));
        boat.push_message(said(
            Message::assistant(format!("The launch password is {SECOND}.")),
            21,
        ));
        assert!(boat.park_tail(1, ParkCause::Regenerate, hour_ago(22)));
        boat.push_message(said(
            Message::assistant("I am sorry, I do not know the launch password."),
            23,
        ));
        boat_id = boat.id;
        json.save_chat(&boat).unwrap();
    }
    let engine = Recording::over(live);
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(
        root,
        Some(engine.clone() as Arc<dyn EngineBackend>),
        no_auto_cfg(),
    );
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    if !matches!(activated, AppEvent::ChatActivated { id, .. } if id == boat_id) {
        cmd_tx.send(AppCommand::SwitchChat(boat_id)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == boat_id),
        )
        .await
        .unwrap();
    }

    // 1. Left alone: nothing in the question is about a branch.
    let plain = ask(
        &cmd_tx,
        &mut evt_rx,
        "In one short sentence: why do databases have indexes?",
    )
    .await;
    assert!(!plain.reply.trim().is_empty(), "the control turn answered");
    assert!(
        !plain.called(BRANCH_READ_ID),
        "an unrelated question read a branch: {:?}",
        plain.tools
    );
    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    let redone = collect(&mut evt_rx, "Ctrl+R").await;
    assert!(!redone.reply.trim().is_empty(), "the regeneration answered");
    assert!(
        !redone.called(BRANCH_READ_ID),
        "a regeneration read the reply it replaces: {:?}",
        redone.tools
    );
    assert_eq!(
        engine.told_of(&[SECOND.to_string()]),
        None,
        "the parked reply reached a request nobody asked for it in"
    );

    // 2. Read when asked.
    let compared = ask(
        &cmd_tx,
        &mut evt_rx,
        "Earlier in this conversation I regenerated your reply about the boat's launch \
         password. Look at the earlier variant of that reply and tell me what it said.",
    )
    .await;
    assert!(
        compared.called(BRANCH_READ_ID),
        "asked for the earlier variant, the turn did not read it: {:?}",
        compared.tools
    );
    assert!(
        holds(&compared.reply, SECOND),
        "the earlier variant's password is not in the answer: {}",
        compared.reply
    );

    // 3. Told by a read, and no further than asked.
    let line = ask(
        &cmd_tx,
        &mut evt_rx,
        "I have another conversation titled \"dock\". Read it and tell me in one \
         sentence where the dock is.",
    )
    .await;
    assert!(line.called(CHAT_READ_ID), "{:?}", line.tools);
    assert!(
        !line.called(BRANCH_READ_ID),
        "told of the other chat's branches, the turn read them unasked: {:?}",
        line.tools
    );
    assert!(
        line.reply.to_lowercase().contains("north"),
        "the line's answer: {}",
        line.reply
    );
    assert!(
        !holds(&line.reply, BERTH),
        "the parked tail reached the answer: {}",
        line.reply
    );
    let branch = ask(
        &cmd_tx,
        &mut evt_rx,
        "That \"dock\" conversation has a branch I set aside. Read that branch and tell \
         me where it says fuel is sold.",
    )
    .await;
    assert!(branch.called(BRANCH_READ_ID), "{:?}", branch.tools);
    assert!(
        holds(&branch.reply, BERTH),
        "the other chat's branch is not in the answer: {}",
        branch.reply
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// The search of parked branches (research §10.2). What only a real model
/// shows is the same judgement the reader's smoke measures, for the tool that
/// finds instead of reading: handed `branch_search`, a model searches the
/// tails **when asked about an earlier variant** and not otherwise.
///
/// The chat is put on disk before the app starts: one question, its reply
/// redone four times. Every variant opens with a line that says nothing, and
/// the third names a call sign **past that line** — where the reader's list,
/// which shows first lines, does not reach. The reply on the line knows
/// nothing.
///
/// The profile has the search and not the reader: the assertion is about
/// which tool is called, and a turn that also had the reader could list the
/// tails and read them one by one instead (docs/lessons.md §9).
///
/// 1. *Left alone:* an unrelated question, then `Ctrl+R` on its reply. No
///    search in either turn, and the call sign in no request.
/// 2. *Found when asked:* which earlier variant mentioned a call sign. The
///    turn searches, and the call sign — which no request held until the
///    search's answer brought it — is in the reply.
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn a_parked_word_is_found_when_asked_and_left_alone_when_not_e2e_live() {
    use crate::features::tools::branches::BRANCH_SEARCH_ID;
    const CALL_SIGN: &str = "KESTREL-9";
    let Some(live) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let hour_ago = |minutes: i64| {
        chrono::Utc::now() - chrono::Duration::hours(1) + chrono::Duration::minutes(minutes)
    };
    let said = |mut m: Message, minutes: i64| {
        m.timestamp = hour_ago(minutes);
        m
    };
    let harbour_id;
    {
        let json = crate::shared::storage::JsonStore::new(Paths::with_root(root));
        let mut profile = Profile::new("Assistant", "You are a helpful assistant.");
        profile.language = Lang::En;
        // The search, and two tools that have nothing to do with it: "left
        // alone" is a choice only where there is something else to call.
        let tools: Vec<String> = [BRANCH_SEARCH_ID, "calculate", "current_time"]
            .into_iter()
            .map(String::from)
            .collect();
        profile.enabled_tools = tools.clone();
        profile.known_tools = tools;
        json.upsert_profile(&profile).unwrap();

        let mut harbour = Chat::from_profile(&profile, "harbour");
        harbour.push_message(said(
            Message::user("Who should I ask about the mooring fees here?"),
            0,
        ));
        let variants = [
            "Let me think about this.\n\nYou could ask at the marina office by the gate."
                .to_string(),
            "Good question.\n\nThe yacht club's secretary usually knows the fees.".to_string(),
            format!(
                "Here is my suggestion.\n\nAsk the harbour master on the radio; her call \
                 sign is {CALL_SIGN}."
            ),
            "A short answer.\n\nThe ferry terminal's desk keeps the price list.".to_string(),
            "I am sorry, I do not know who handles the mooring fees here.".to_string(),
        ];
        for (n, text) in variants.into_iter().enumerate() {
            let n = n as i64;
            if n > 0 {
                assert!(harbour.park_tail(1, ParkCause::Regenerate, hour_ago(2 * n)));
            }
            harbour.push_message(said(Message::assistant(text), 2 * n + 1));
        }
        assert_eq!(harbour.branches.len(), 4, "four variants are parked");
        harbour_id = harbour.id;
        json.save_chat(&harbour).unwrap();
    }
    let engine = Recording::over(live);
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(
        root,
        Some(engine.clone() as Arc<dyn EngineBackend>),
        no_auto_cfg(),
    );
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == harbour_id),
    )
    .await
    .unwrap();

    // 1. Left alone: nothing in the question is about a branch.
    let plain = ask(
        &cmd_tx,
        &mut evt_rx,
        "In one short sentence: why do databases have indexes?",
    )
    .await;
    assert!(!plain.reply.trim().is_empty(), "the control turn answered");
    assert!(
        !plain.called(BRANCH_SEARCH_ID),
        "an unrelated question searched the branches: {:?}",
        plain.tools
    );
    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    let redone = collect(&mut evt_rx, "Ctrl+R").await;
    assert!(!redone.reply.trim().is_empty(), "the regeneration answered");
    assert!(
        !redone.called(BRANCH_SEARCH_ID),
        "a regeneration searched for the reply it replaces: {:?}",
        redone.tools
    );
    assert_eq!(
        engine.told_of(&[CALL_SIGN.to_string()]),
        None,
        "the parked reply reached a request nobody asked for it in"
    );

    // 2. Found when asked.
    let found = ask(
        &cmd_tx,
        &mut evt_rx,
        "Earlier in this conversation I regenerated your reply about the mooring fees \
         several times. One of the earlier variants mentioned a call sign. Find that \
         variant and tell me the call sign.",
    )
    .await;
    assert!(
        found.called(BRANCH_SEARCH_ID),
        "asked which variant said it, the turn did not search: {:?}",
        found.tools
    );
    assert!(
        holds(&found.reply, CALL_SIGN),
        "the call sign is not in the answer: {}",
        found.reply
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}
