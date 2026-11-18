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

use super::*;
use crate::entities::chat_branch::ParkCause;
use crate::shared::config::CompactionSettings;

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
