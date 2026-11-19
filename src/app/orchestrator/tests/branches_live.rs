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
