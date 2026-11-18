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
