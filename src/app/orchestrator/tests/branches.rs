//! Orchestrator tests — a fork and a switch of the open chat's line
//! (docs/research/chat-branches.md §3.3–§3.4). Part of the [`super`] module
//! (fixtures in mod.rs).

use super::*;
use crate::entities::chat::FoldKey;
use crate::entities::chat_branch::{BranchView, ParkCause};
use crate::entities::followup::{Followup, FollowupState};

/// A minute apart, from an hour ago: the times a take-back's rules compare.
fn at(minutes: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::hours(1) + chrono::Duration::minutes(minutes)
}

fn said(mut message: Message, minutes: i64) -> Message {
    message.timestamp = at(minutes);
    message
}

/// The open chat `q1 a1 q2 a2 q3 a3`, a minute apart, and its message ids.
fn forkable() -> (
    tempfile::TempDir,
    Orchestrator,
    UnboundedReceiver<AppEvent>,
    Uuid,
    Vec<Uuid>,
) {
    let messages = vec![
        said(Message::user("q1"), 0),
        said(Message::assistant("a1"), 1),
        said(Message::user("q2"), 2),
        said(Message::assistant("a2"), 3),
        said(Message::user("q3"), 4),
        said(Message::assistant("a3"), 5),
    ];
    let ids = messages.iter().map(|m| m.id).collect();
    let (d, mut orch, rx, chat) = bare_with_chat(messages);
    orch.active_id = Some(chat);
    (d, orch, rx, chat, ids)
}

fn drain(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<AppEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn line(orch: &Orchestrator, chat: Uuid) -> Vec<String> {
    let chat = orch.chats.iter().find(|c| c.id == chat).unwrap();
    chat.messages.iter().map(|m| m.text.clone()).collect()
}

/// The feed the events rebuilt, and the branches sent after it.
fn activated(events: &[AppEvent]) -> (Vec<String>, BranchView) {
    let feed = events
        .iter()
        .find_map(|e| match e {
            AppEvent::ChatActivated { messages, .. } => {
                Some(messages.iter().map(|m| m.text.clone()).collect())
            }
            _ => None,
        })
        .expect("the feed is rebuilt");
    let branches = events
        .iter()
        .find_map(|e| match e {
            AppEvent::Branches(view) => Some((**view).clone()),
            _ => None,
        })
        .expect("the chat's branches are sent with the activation");
    (feed, branches)
}

/// Every set of branches the events carry, in the order they were sent.
fn branch_views(events: &[AppEvent]) -> Vec<BranchView> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::Branches(view) => Some((**view).clone()),
            _ => None,
        })
        .collect()
}

fn notices(events: &[AppEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::Notice(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A fork at a reply: the line ends on it, what followed is parked and
/// anchored to it, the feed is rebuilt with the branch at its end, and
/// nothing returns to the input box.
#[test]
fn a_fork_at_a_reply_parks_what_followed() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    orch.handle_fork_at(ids[3]);

    assert_eq!(line(&orch, chat), ["q1", "a1", "q2", "a2"]);
    let stored = orch.chats.iter().find(|c| c.id == chat).unwrap();
    assert_eq!(stored.branches.len(), 1);
    assert_eq!(stored.branches[0].after, Some(ids[3]));
    assert_eq!(stored.branches[0].cause, ParkCause::Fork);

    let events = drain(&mut rx);
    let (feed, branches) = activated(&events);
    assert_eq!(feed, ["q1", "a1", "q2", "a2"]);
    assert_eq!(branches.parked.len(), 1);
    assert!(branches.parked[0].at_end);
    assert_eq!(branches.parked[0].number, 5);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AppEvent::RestoreInput(_))),
        "a reply is not written again"
    );
}

/// A fork at the person's own message: the line ends before it and its text
/// returns to the input box, to be written again.
#[test]
fn a_fork_at_the_persons_message_hands_its_text_back() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    orch.handle_fork_at(ids[2]);

    assert_eq!(line(&orch, chat), ["q1", "a1"]);
    let events = drain(&mut rx);
    let restored: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AppEvent::RestoreInput(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(restored, ["q2"]);
    // The rebuild comes first, so the text lands in the box of the new line.
    let rebuilt = events
        .iter()
        .position(|e| matches!(e, AppEvent::ChatActivated { .. }));
    let returned = events
        .iter()
        .position(|e| matches!(e, AppEvent::RestoreInput(_)));
    assert!(rebuilt < returned);
}

/// What a fork cannot do is answered, and changes nothing: the line's last
/// reply has nothing after it, and a message the line no longer shows names
/// no place.
#[test]
fn a_fork_that_cannot_be_made_says_why() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    let loc = orch.ui_locale();
    let before = orch.chats[0].clone();

    orch.handle_fork_at(ids[5]);
    assert_eq!(notices(&drain(&mut rx)), [loc.t("ui.branch.nothing_after")]);
    orch.handle_fork_at(Uuid::new_v4());
    assert_eq!(notices(&drain(&mut rx)), [loc.t("ui.branch.message_gone")]);
    assert_eq!(orch.chats[0], before);
    assert_eq!(line(&orch, chat).len(), 6);
}

/// A switch back to a parked tail restores the line; a switch between two
/// variants parks the one it displaces, and says which variant the line
/// holds now, in the line's own numbers.
#[test]
fn a_switch_restores_the_tail_and_says_where_the_line_is() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    let loc = orch.ui_locale();
    orch.handle_fork_at(ids[4]);
    drain(&mut rx);

    // Nothing was written on the new line: the place has one variant again.
    orch.handle_switch_branch(ids[4]);
    assert_eq!(line(&orch, chat), ["q1", "a1", "q2", "a2", "q3", "a3"]);
    assert!(orch.chats[0].branches.is_empty());
    let events = drain(&mut rx);
    let (feed, branches) = activated(&events);
    assert_eq!(feed.len(), 6);
    assert!(branches.is_empty());
    assert_eq!(
        notices(&events),
        [loc.tf("ui.branch.returned", &[("number", "5")])]
    );

    // Another way from the same place, then back to the first.
    orch.handle_fork_at(ids[4]);
    let other = said(Message::user("another q3"), 10);
    let other_id = other.id;
    orch.chats[0].messages.push(other);
    drain(&mut rx);
    orch.handle_switch_branch(ids[4]);
    assert_eq!(line(&orch, chat), ["q1", "a1", "q2", "a2", "q3", "a3"]);
    let parked = &orch.chats[0].branches;
    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].messages[0].id, other_id);
    assert_eq!(parked[0].cause, ParkCause::Switch);
    let events = drain(&mut rx);
    assert_eq!(
        notices(&events),
        [loc.tf(
            "ui.branch.switched",
            &[("number", "5"), ("variant", "1"), ("of", "2")]
        )]
    );
    let (_, branches) = activated(&events);
    assert_eq!(branches.marks.len(), 1);
    assert_eq!(branches.marks[0].message, ids[4]);

    // A variant that is not parked is answered, and nothing moves.
    orch.handle_switch_branch(Uuid::new_v4());
    assert_eq!(notices(&drain(&mut rx)), [loc.t("ui.branch.variant_gone")]);
    assert_eq!(line(&orch, chat).len(), 6);
}

/// Where a take-back is ignored, a fork and a switch are: while a reply is
/// being generated, and on a sub-agent transcript, which is read-only.
#[test]
fn a_fork_and_a_switch_are_ignored_where_a_take_back_is() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    orch.handle_fork_at(ids[4]);
    drain(&mut rx);
    let before = orch.chats[0].clone();

    orch.gen_state
        .begin(Uuid::new_v4(), tokio_util::sync::CancellationToken::new());
    orch.handle_fork_at(ids[1]);
    orch.handle_switch_branch(ids[4]);
    assert_eq!(orch.chats[0], before);
    assert!(drain(&mut rx).is_empty());
    orch.gen_state = crate::app::gen_state::GenState::Idle;

    // An id that names no chat of the list — a transcript's.
    orch.active_id = Some(Uuid::new_v4());
    orch.handle_fork_at(ids[1]);
    orch.handle_switch_branch(ids[4]);
    assert_eq!(orch.chats[0], before);
    assert!(drain(&mut rx).is_empty());
    let _ = chat;
}

/// A fork cancels the follow-ups planned in what it parks and keeps the ones
/// the line still holds the planning of — the take-back's rule
/// (`cancel_orphaned_followups`), reached by the same call.
#[test]
fn a_fork_cancels_the_follow_ups_planned_in_what_it_parks() {
    let (_d, mut orch, _rx, chat, ids) = forkable();
    let profile = orch.chats[0].profile_id;
    let plan = |orch: &Orchestrator, minutes: i64| {
        let mut f = Followup::new(profile, chat, "come back", at(600));
        f.created_at = at(minutes);
        orch.storage.db().followup_insert(&f).unwrap();
        f.id
    };
    // Planned while a1 was the last word, and while a3 was.
    let kept = plan(&orch, 1);
    let parked = plan(&orch, 5);

    orch.handle_fork_at(ids[3]);

    let state = |id: Uuid| {
        orch.storage
            .db()
            .followup_get(id)
            .unwrap()
            .expect("the follow-up")
            .state
    };
    assert_eq!(state(parked), FollowupState::Cancelled);
    assert_eq!(state(kept), FollowupState::Pending);
}

/// A block folded by a click in a tail that is parked keeps its fold: the
/// prune at activation counts the parked tails among the chat's messages, so
/// the block is as it was left when the tail returns.
#[test]
fn a_fold_in_a_parked_tail_survives_the_fork() {
    let (_d, mut orch, mut rx, _chat, ids) = forkable();
    let on_line = FoldKey::Thoughts(ids[1]);
    let parked = FoldKey::Thoughts(ids[5]);
    let gone = FoldKey::Thoughts(Uuid::new_v4());
    orch.chats[0].feed_view.flipped = [on_line, parked, gone].into_iter().collect();

    orch.handle_fork_at(ids[3]);
    drain(&mut rx);

    let kept = &orch.chats[0].feed_view.flipped;
    assert!(
        kept.contains(&on_line) && kept.contains(&parked),
        "{kept:?}"
    );
    assert!(!kept.contains(&gone), "a message the chat does not hold");
}

/// The full-text index holds the line: a word said only in the parked tail is
/// not found after the fork, and is found again when the tail is back.
#[test]
fn a_parked_tail_is_not_in_the_search_index() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    orch.chats[0].messages[5].text = "the zanzibar answer".into();
    let query = crate::features::chat_search::to_fts_query("zanzibar").expect("a query");
    let found = |orch: &mut Orchestrator| {
        let stored = orch.chats.iter().find(|c| c.id == chat).unwrap().clone();
        orch.storage.json().save_chat(&stored).unwrap();
        orch.index_saved_chat(&stored);
        orch.storage.cache().search_chats(&query).unwrap()
    };
    assert_eq!(found(&mut orch), [chat]);

    orch.handle_fork_at(ids[3]);
    assert_eq!(found(&mut orch), Vec::<Uuid>::new());

    orch.handle_switch_branch(ids[4]);
    assert_eq!(found(&mut orch), [chat]);
    drain(&mut rx);
}

/// Opening a chat sends its branches; opening one without any sends none, so
/// the previous chat's marks do not stay on the screen.
#[test]
fn an_activation_sends_the_chats_own_branches() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    orch.handle_fork_at(ids[3]);
    drain(&mut rx);

    let profile = Profile::new("P", "sys");
    let plain = Chat::from_profile(&profile, "plain");
    let plain_id = plain.id;
    orch.chats.push(plain);

    orch.activate(plain_id);
    assert!(activated(&drain(&mut rx)).1.is_empty());
    orch.activate(chat);
    assert_eq!(activated(&drain(&mut rx)).1.parked.len(), 1);
}

/// A reflection that was out when the line was forked, and is then stopped
/// before it acted, gives its window back no higher than the fork left the
/// watermark: the index it started from may name a message the line no
/// longer holds, and raised to it the new line's messages below it would
/// never be reflected on (research §3.4).
#[test]
fn a_refund_after_a_fork_does_not_raise_the_watermark_past_the_cut() {
    use crate::app::orchestrator::background::{Acted, BgOutcome, Refund, Window};
    use crate::entities::task::BackgroundKind;

    let (_d, mut orch, _rx, chat, ids) = forkable();
    // The spawn read from 4 and advanced the watermark to the line's end.
    orch.chats[0].reflected_upto = Some(6);
    orch.begin_bg(
        BackgroundKind::Reflection,
        tokio_util::sync::CancellationToken::new(),
        Some(Refund {
            window: Window::Reflection {
                chat,
                upto: Some(4),
                at: None,
            },
            acted: Arc::new(Acted::default()),
        }),
    );
    orch.handle_fork_at(ids[1]);
    assert_eq!(orch.chats[0].reflected_upto, Some(2), "lowered to the cut");

    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Cancelled { consumed: false },
        None,
    );
    assert_eq!(orch.chats[0].reflected_upto, Some(2));

    // With no fork in between the refund is what it was: the window's start.
    let (_d, mut orch, _rx, chat, _ids) = forkable();
    orch.chats[0].reflected_upto = Some(6);
    orch.begin_bg(
        BackgroundKind::Reflection,
        tokio_util::sync::CancellationToken::new(),
        Some(Refund {
            window: Window::Reflection {
                chat,
                upto: Some(4),
                at: None,
            },
            acted: Arc::new(Acted::default()),
        }),
    );
    orch.handle_bg_done(
        BackgroundKind::Reflection,
        BgOutcome::Cancelled { consumed: false },
        None,
    );
    assert_eq!(orch.chats[0].reflected_upto, Some(4));
}

/// A message sent in this session can be forked at without opening the chat
/// again: its echo carries the stored message's id, which is what `/fork N`
/// sends. The fork is at the person's own message, so its text returns to the
/// input box and the exchange is parked whole — on disk too, at schema 9.
#[tokio::test]
async fn a_message_sent_in_this_session_can_be_forked_at() {
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("ответ".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };

    cmd_tx
        .send(AppCommand::SendMessage("первый вопрос".into()))
        .unwrap();
    let echo = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::UserMessage { .. }))
        .await
        .unwrap();
    let AppEvent::UserMessage { text, id } = echo else {
        unreachable!()
    };
    assert_eq!(text, "первый вопрос");
    let sent = id.expect("the echo names the stored message");
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::ForkAt { message: sent }).unwrap();
    let restore = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::RestoreInput(_)))
        .await
        .unwrap();
    assert!(matches!(restore, AppEvent::RestoreInput(t) if t == "первый вопрос"));

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.v, crate::shared::storage::schema::CHAT_SCHEMA);
    assert!(chat.messages.is_empty(), "{:?}", chat.messages);
    assert!(chat.deleted.is_empty(), "a fork archives nothing");
    assert_eq!(chat.branches.len(), 1);
    let tail = &chat.branches[0];
    assert_eq!(tail.after, None, "the tail starts the chat");
    assert_eq!(tail.cause, ParkCause::Fork);
    assert_eq!(tail.messages[0].id, sent);
    assert_eq!(tail.messages[1].text, "ответ");
}

/// A message sent at a place a branch is parked is the line's own variant of
/// that place: the branches are sent again with it, and its bubble carries
/// the mark — no activation happens when a message is sent. A regeneration
/// of the reply a branch continues from parks it rather than archive it, so
/// the branch is still reached.
#[tokio::test]
async fn the_line_growing_at_a_fork_refreshes_the_marks() {
    let backend = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("ответ".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    let send = |text: &str| {
        cmd_tx
            .send(AppCommand::SendMessage(text.to_string()))
            .unwrap()
    };
    let sent_id = |e: AppEvent| match e {
        AppEvent::UserMessage { id, .. } => id.expect("a stored message"),
        _ => unreachable!(),
    };

    send("первый вопрос");
    let first = sent_id(
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::UserMessage { .. }))
            .await
            .unwrap(),
    );
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::ForkAt { message: first }).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::RestoreInput(_)))
        .await
        .unwrap();

    // Another way from the same place.
    send("другой вопрос");
    let second = sent_id(
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::UserMessage { .. }))
            .await
            .unwrap(),
    );
    let branches = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Branches(_)))
        .await
        .unwrap();
    let AppEvent::Branches(view) = branches else {
        unreachable!()
    };
    assert_eq!(view.marks.len(), 1, "{view:?}");
    assert_eq!(
        (
            view.marks[0].message,
            view.marks[0].variant,
            view.marks[0].of
        ),
        (second, 2, 2)
    );
    assert!(
        !view.parked[0].at_end,
        "the line has its own continuation now"
    );
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.messages[0].id, second);
    assert_eq!(chat.branches[0].messages[0].id, first);
}

/// `Ctrl+R` keeps the reply it replaces as a branch (research §3.10). While
/// the new reply is on its way the old one stands at the line's end, behind
/// the question; once the new one lands it is variant 2 of that place and
/// carries the mark, with no activation in between. A regeneration that lands
/// nothing leaves the line on the question — and the branches are sent again,
/// so the screen can say where the replaced reply is. A switch returns to it.
#[tokio::test]
async fn a_regeneration_keeps_the_replaced_reply_as_a_variant() {
    let reply = |text: &str| {
        vec![
            ChatChunk::Text(text.into()),
            ChatChunk::Finished(FinishReason::Stop),
        ]
    };
    let backend = Arc::new(MockBackend::sequence(vec![
        reply("первый"),
        reply("второй"),
        vec![
            ChatChunk::Error {
                message: "overloaded_error: Overloaded".into(),
                transient: true,
            },
            ChatChunk::Finished(FinishReason::Error),
        ],
    ])) as Arc<dyn EngineBackend>;
    // No title is asked for: its request would take a reply of the script.
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), no_auto_cfg());
    let root = _d.path().to_path_buf();
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    // Bounded: a step that stops sending its event fails here by name.
    use super::generation::wait_for_bounded as within;
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    let branches = |e: AppEvent| match e {
        AppEvent::Branches(view) => *view,
        _ => unreachable!(),
    };
    let is_branches = |e: &AppEvent| matches!(e, AppEvent::Branches(_));
    let is_landed = |e: &AppEvent| matches!(e, AppEvent::TurnLanded { .. });

    cmd_tx
        .send(AppCommand::SendMessage("вопрос".into()))
        .unwrap();
    let landed = within(&mut evt_rx, "the first reply", is_landed).await;
    let AppEvent::TurnLanded { messages, .. } = landed else {
        unreachable!()
    };
    let first = messages[0].id;

    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    // The activation that rebuilt the feed without the old reply: it is
    // parked at the line's end, and no bubble carries a mark yet.
    let view = branches(within(&mut evt_rx, "the activation's branches", is_branches).await);
    assert!(view.marks.is_empty(), "{view:?}");
    assert_eq!(view.parked.len(), 1);
    assert!(view.parked[0].at_end);
    assert_eq!(view.parked[0].first, first);
    // The new reply landed: the mark is on it.
    let landed = within(&mut evt_rx, "the second reply", is_landed).await;
    let AppEvent::TurnLanded { messages, .. } = landed else {
        unreachable!()
    };
    let second = messages[0].id;
    let view = branches(within(&mut evt_rx, "the landing's branches", is_branches).await);
    assert_eq!(
        view.marks
            .iter()
            .map(|m| (m.message, m.variant, m.of))
            .collect::<Vec<_>>(),
        [(second, 2, 2)]
    );
    assert_eq!((view.parked[0].variant, view.parked[0].of), (1, 2));
    assert!(!view.parked[0].at_end);

    // A regeneration the engine fails before a word: nothing lands, and the
    // line stays on the question with both replies parked behind it.
    cmd_tx.send(AppCommand::RegenerateLast).unwrap();
    let view = branches(within(&mut evt_rx, "the second activation", is_branches).await);
    assert_eq!(view.parked.iter().filter(|b| b.at_end).count(), 2);
    within(&mut evt_rx, "the failed turn's end", |e| {
        matches!(e, AppEvent::Finished { .. })
    })
    .await;
    let again = branches(within(&mut evt_rx, "the branches after it", is_branches).await);
    assert_eq!(again, view, "said again once the turn ended with nothing");

    // Back to the first reply.
    cmd_tx.send(AppCommand::SwitchBranch { first }).unwrap();
    within(&mut evt_rx, "the switch's notice", |e| {
        matches!(e, AppEvent::Notice(_))
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    let texts: Vec<&str> = chat.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["вопрос", "первый"]);
    assert!(chat.deleted.is_empty(), "{:?}", chat.deleted);
    let parked: Vec<(&str, ParkCause)> = chat
        .branches
        .iter()
        .map(|b| (b.messages[0].text.as_str(), b.cause))
        .collect();
    assert_eq!(parked, [("второй", ParkCause::Regenerate)]);
}

/// A greeting is swapped only in a chat that holds nothing else, and a chat
/// with a parked branch holds something: a branch may continue from the
/// greeting, which a swap would replace under it.
#[test]
fn a_chat_with_a_branch_does_not_swap_its_greeting() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let mut profile = Profile::new("P", "sys");
    profile.greeting = Some("Здравствуйте!".into());
    profile.alternate_greetings = vec!["Привет!".into()];
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::assistant("Здравствуйте!"));
    chat.push_message(Message::user("вопрос"));
    let greeting = chat.messages[0].id;
    assert!(chat.fork_at(1, chrono::Utc::now()));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);

    orch.handle_swap_greeting(None);
    assert_eq!(
        notices(&drain(&mut rx)),
        [orch.ui_locale().t("ui.greeting.started")]
    );
    assert_eq!(orch.chats[0].messages[0].id, greeting);
    assert_eq!(orch.chats[0].branches[0].after, Some(greeting));
}

/// A take-back of the exchange a branch continues from parks the exchange,
/// as a take-back of any exchange does: the branch is then reached through
/// the parked exchange, and a switch to it puts the whole conversation back.
#[test]
fn a_take_back_of_an_exchange_a_branch_continues_from_parks_it() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    orch.handle_fork_at(ids[3]);
    drain(&mut rx);

    orch.handle_delete_last();
    assert_eq!(line(&orch, chat), ["q1", "a1"]);
    let stored = orch.chats[0].clone();
    assert!(stored.deleted.is_empty(), "nothing is archived");
    let causes: Vec<ParkCause> = stored.branches.iter().map(|b| b.cause).collect();
    assert_eq!(causes, [ParkCause::Fork, ParkCause::Takeback]);
    assert!(stored.branch_view().parked.iter().all(|b| !b.detached));
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AppEvent::RestoreInput(t) if t == "q2")),
        "the take-back still hands the text back"
    );

    orch.handle_switch_branch(ids[4]);
    assert_eq!(line(&orch, chat), ["q1", "a1", "q2", "a2", "q3", "a3"]);
    assert!(orch.chats[0].branches.is_empty());
}

/// `Ctrl+E` is `/fork N` at the person's last message under a name of its
/// own (research §3.10): the exchange is parked, never archived; its text
/// comes back; the screen is sent the branch, which stands at the line's end;
/// and a switch to it restores the conversation with nothing left parked.
#[test]
fn a_take_back_keeps_the_exchange_as_a_branch() {
    let (_d, mut orch, mut rx, chat, ids) = forkable();

    orch.handle_delete_last();
    assert_eq!(line(&orch, chat), ["q1", "a1", "q2", "a2"]);
    let stored = &orch.chats[0];
    assert!(stored.deleted.is_empty(), "nothing is archived");
    assert_eq!(stored.branches.len(), 1);
    assert_eq!(stored.branches[0].cause, ParkCause::Takeback);
    assert_eq!(stored.branches[0].after, Some(ids[3]));
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AppEvent::RestoreInput(t) if t == "q3"))
    );
    let view = branch_views(&events).pop().expect("the branches are sent");
    assert_eq!(view.parked.len(), 1);
    assert!(view.parked[0].at_end);
    assert_eq!((view.parked[0].number, view.parked[0].bubbles), (5, 2));

    // The same fork by its command lands in the same place.
    let (_d2, mut by_command, _rx2, forked, fork_ids) = forkable();
    by_command.handle_fork_at(fork_ids[4]);
    assert_eq!(line(&by_command, forked), line(&orch, chat));
    assert_eq!(by_command.chats[0].branches[0].after, Some(fork_ids[3]));

    orch.handle_switch_branch(ids[4]);
    assert_eq!(line(&orch, chat), ["q1", "a1", "q2", "a2", "q3", "a3"]);
    assert!(orch.chats[0].branches.is_empty());
}

/// What reflection reads as "the reply did not land" is the tails the two
/// keys parked (research §3.10). A reply the person **switched back to** is on
/// the line again and leaves no record: going back to a reply is not that
/// sign — and neither is the switch that displaced the other variant.
#[test]
fn a_reply_switched_back_to_is_not_a_signal_for_reflection() {
    use crate::app::orchestrator::reflection::behavior_markers;
    let (_d, mut orch, mut rx, chat, ids) = forkable();
    let loc = orch.ui_locale();
    assert!(behavior_markers(&orch.chats[0], None, loc).is_none());

    // What `Ctrl+R` does to the chat, without the engine: the reply is
    // parked under its cause and another is written in its place.
    let now = chrono::Utc::now();
    assert!(orch.chats[0].park_tail(5, ParkCause::Regenerate, now));
    orch.chats[0].push_message(Message::assistant("a3 again"));
    let signal = behavior_markers(&orch.chats[0], None, loc).expect("a regeneration is read");
    assert!(
        signal.contains(&loc.tf("reflect.behavior.regen", &[("n", "1")])),
        "{signal}"
    );

    // The person returns to the first reply.
    orch.handle_switch_branch(ids[5]);
    drain(&mut rx);
    assert_eq!(line(&orch, chat), ["q1", "a1", "q2", "a2", "q3", "a3"]);
    let causes: Vec<ParkCause> = orch.chats[0].branches.iter().map(|b| b.cause).collect();
    assert_eq!(causes, [ParkCause::Switch]);
    assert!(behavior_markers(&orch.chats[0], None, loc).is_none());

    // A take-back is read, and a fork is not.
    orch.handle_delete_last();
    orch.handle_fork_at(ids[2]);
    drain(&mut rx);
    let signal = behavior_markers(&orch.chats[0], None, loc).expect("a take-back is read");
    assert!(
        signal.contains(&loc.tf("reflect.behavior.deleted", &[("n", "1")])),
        "{signal}"
    );
    assert!(!signal.contains(&loc.tf("reflect.behavior.regen", &[("n", "1")])));
}
