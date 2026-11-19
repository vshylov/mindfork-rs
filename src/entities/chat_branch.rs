//! Branches inside a chat: the tails that are off the line, and the two
//! operations that move them (docs/research/chat-branches.md §3.1–§3.3,
//! spec §5.1).
//!
//! **The chat stays a line.** [`Chat::messages`] is the conversation in front
//! of the person, as it always was, and everything that reads it goes on
//! reading a linear conversation. [`Chat::branches`] holds every tail that
//! left that line, each with the id of the last message it shares with the
//! line it left ([`Branch::after`]). The tree is implied by those anchors:
//! message ids are unique within a chat, so an anchor names one place — on the
//! line, or inside another parked tail — and the tails with one anchor,
//! together with whatever continues past it where it stands, are the
//! **variants** of that place.
//!
//! - A **fork** cuts the line at a bubble's edge and parks the tail
//!   ([`Chat::fork_at`]). `Ctrl+R` and `Ctrl+E` are the same cut at the line's
//!   last exchange, parked under a cause of their own ([`Chat::park_tail`],
//!   research §3.10): nothing a person wrote or was answered leaves the chat.
//! - A **switch** puts a parked tail back on the line and parks what it
//!   displaces ([`Chat::switch_to`]). A parked tail is the line as it was
//!   left, so returning to it restores the conversation the person walked
//!   away from, to its last message.
//!
//! A variant is named by **its first message**: no branch id is stored, and a
//! tail a switch split in two needs none invented.
//!
//! Both operations are pure functions of the chat; the orchestrator, which
//! owns it, tells the rest of the app what a truncation tells it
//! (`app/orchestrator/branches.rs`).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::chat::{Chat, Compaction, visible_message_count, visible_rows};
use crate::entities::message::{Message, MessageRole};

/// A tail of the conversation that is off the line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    /// The last message this tail shares with the line it left — the place it
    /// continues from. `None` — the tail starts the chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Uuid>,
    /// The tail, in order. Never empty: a cut that takes nothing parks nothing.
    pub messages: Vec<Message>,
    /// When it left the line.
    pub parked_at: DateTime<Utc>,
    /// What took it off the line.
    pub cause: ParkCause,
    /// The rolling summary in force when it was parked, to have again when it
    /// is back: a summary whose boundary left with the tail describes what the
    /// line no longer holds, and making one anew costs a model call
    /// (spec §6.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<Compaction>,
    /// The reflection watermark in force when it was parked
    /// ([`Chat::reflected_upto`]), so what was reflected on is not read again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflected_upto: Option<usize>,
}

impl Branch {
    /// The id of the tail's first message — what names the variant.
    pub fn first(&self) -> Option<Uuid> {
        self.messages.first().map(|m| m.id)
    }
}

/// What took a tail off the line. A strict stored enum: all four values
/// arrived with schema 9, so making a regeneration and a take-back park their
/// tail (research §3.10) needed no step of its own.
///
/// Two of the causes are also a **signal**: a reply regenerated and an
/// exchange taken back are what reflection reads as "the reply did not land"
/// (`behavior_markers`, spec §17). A switch back to such a tail puts it on the
/// line and its record is gone — going back to a reply is not that sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParkCause {
    /// `/fork N`: the person went back to a message.
    Fork,
    /// A switch to another variant displaced it.
    Switch,
    /// `Ctrl+R` replaced the reply: the old one is variant 1 of its place.
    Regenerate,
    /// `Ctrl+E` took the exchange back: `/fork N` at the person's last
    /// message, under its own name.
    Takeback,
}

/// Where a fork at a message cuts the line ([`Chat::fork_cut`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkCut {
    /// The line keeps `messages[..cut]`; the rest is parked.
    pub cut: usize,
    /// The fork is at the person's own message: its text, to write again.
    pub restore: Option<String>,
}

/// Why a switch did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchError {
    /// No parked tail starts with that message.
    Unknown,
    /// The tail's anchor is neither on the line nor in a tail that can be
    /// brought to it — a file edited by hand. The tail is kept and listed.
    Detached,
}

/// The mark a bubble of the line carries when the place it continues from has
/// other variants: "variant 2 of 3".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMark {
    /// The first message of the line's continuation at that place — the first
    /// message of the bubble the mark is drawn on.
    pub message: Uuid,
    pub variant: u16,
    pub of: u16,
}

/// One parked tail, as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// The tail's first message: what a switch is asked for by.
    pub first: Uuid,
    /// The number its first bubble has on its own line — the number a
    /// message of the line shows (spec §11.3). 0 for a detached tail.
    pub number: u32,
    /// Which variant of its place it is, among `of`: the variants are ordered
    /// by their first message's time, so the figure means the same before and
    /// after any switch.
    pub variant: u16,
    pub of: u16,
    /// Its messages, counted as the feed draws them.
    pub bubbles: usize,
    /// Who wrote its first message.
    pub role: MessageRole,
    /// The first line of its first message that has text.
    pub preview: String,
    /// When it was last written to: its last message's time.
    pub at: DateTime<Utc>,
    /// How many parked tails stand between it and the line: 0 — it continues
    /// from a message of the line.
    pub depth: u16,
    /// It continues from the line's **last** message, which has no
    /// continuation of its own: the state a fork leaves until the person
    /// writes.
    pub at_end: bool,
    /// Its anchor names no message that can reach the line.
    pub detached: bool,
}

/// The chat's branches as the screen needs them: the marks on the line's
/// bubbles and the parked tails, in tree order — a tail, then the tails that
/// continue from inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchView {
    pub marks: Vec<LineMark>,
    pub parked: Vec<BranchInfo>,
}

impl BranchView {
    pub fn is_empty(&self) -> bool {
        self.parked.is_empty()
    }

    /// The address of every parked tail, in [`Self::parked`]'s order: the
    /// `(number, variant)` of each place from the line down to the tail — one
    /// pair for a tail that continues from the line, the path through the
    /// tails that hold it for one inside another. A detached tail has no
    /// place to be named by, and is `(0, n)`: its turn among the detached.
    ///
    /// Tree order is what makes this one pass: a row's parent is the nearest
    /// row above it that is one level out.
    pub fn paths(&self) -> Vec<BranchPath> {
        let mut trail: Vec<BranchPath> = Vec::new();
        let mut detached = 0u16;
        self.parked
            .iter()
            .map(|info| {
                if info.detached {
                    detached += 1;
                    return vec![(0, detached)];
                }
                trail.truncate(usize::from(info.depth));
                let mut path = trail.last().cloned().unwrap_or_default();
                path.push((info.number, info.variant));
                trail.push(path.clone());
                path
            })
            .collect()
    }

    /// The address of the tail that starts with `first`, when one is parked.
    pub fn path_of(&self, first: Uuid) -> Option<BranchPath> {
        let at = self.parked.iter().position(|info| info.first == first)?;
        self.paths().into_iter().nth(at)
    }
}

/// A parked tail's address: the `(number, variant)` of every place from the
/// line down to it ([`BranchView::paths`]). The last pair is what
/// `/branch N K` takes for a tail that continues from the line.
pub type BranchPath = Vec<(u32, u16)>;

/// An address as it is written wherever a branch is named — the assistant's
/// tools, a search result, a note: `11/1`, and `5/1 > 7/1` for a tail inside
/// another.
pub fn branch_address(path: &[(u32, u16)]) -> String {
    path.iter()
        .map(|(number, variant)| format!("{number}/{variant}"))
        .collect::<Vec<_>>()
        .join(" > ")
}

/// Where a parked message stands, for something that found it and has to show
/// the person where it is without moving the line ([`Chat::parked_place`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParkedPlace {
    /// The first message of the tail that holds it — the tail's name.
    pub first: Uuid,
    /// The message of the line to look at: the one that stands where the
    /// tail — or, for a tail inside another, the outermost tail on its way
    /// to the line — leaves it, which is the bubble the variants mark is
    /// drawn on; the line's last message when the tail continues from the
    /// line's end. `None` — the tail is detached, or the line is empty.
    pub leaves_at: Option<Uuid>,
    /// The tail's anchor reaches no message of the line.
    pub detached: bool,
}

/// Where a tail's anchor stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    /// On the line: the tail continues from `messages[..cut]`.
    Line(usize),
    /// Inside the parked tail `branch`, after its message `pos`.
    Tail { branch: usize, pos: usize },
    /// Nowhere.
    Nowhere,
}

impl Chat {
    /// Where a fork at the bubble that shows `message` cuts the line
    /// (research §3.6). One rule: **after a fork the next thing in the
    /// conversation is the person's message.** So a fork at a reply of the
    /// assistant cuts after it — its tool results included; a fork at the
    /// person's own message cuts before it and hands its text back, to be
    /// written again; a fork at a note row (a task notification, a director's
    /// line) cuts before it, as a take-back does with a notification.
    ///
    /// `None` when the line shows no such message.
    pub fn fork_cut(&self, message: Uuid) -> Option<ForkCut> {
        let at = self.messages.iter().position(|m| m.id == message)?;
        let bubble = visible_rows(&self.messages)
            .into_iter()
            .find(|r| r.contains(&at))?;
        let first = &self.messages[bubble.start];
        Some(match first.role {
            MessageRole::Assistant => ForkCut {
                cut: bubble.end,
                restore: None,
            },
            MessageRole::User => ForkCut {
                cut: bubble.start,
                restore: Some(first.text.clone()),
            },
            MessageRole::System | MessageRole::Tool => ForkCut {
                cut: bubble.start,
                restore: None,
            },
        })
    }

    /// Forks the line at `cut`: `messages[cut..]` is parked, anchored to the
    /// message before it. `false` — nothing follows the cut, so there is
    /// nothing to fork from.
    pub fn fork_at(&mut self, cut: usize, now: DateTime<Utc>) -> bool {
        self.park_tail(cut, ParkCause::Fork, now)
    }

    /// Takes `messages[cut..]` off the line and keeps it as a branch, under
    /// the name of what took it: a fork, a regeneration (`Ctrl+R`: the reply
    /// that is being replaced), a take-back (`Ctrl+E`: the last exchange).
    /// `false` — nothing follows the cut, and nothing was parked.
    ///
    /// The `deleted` archive used to be where the two keys put what they
    /// removed (spec §11.7), and what it holds is gone to every reader: no
    /// screen shows it, nothing returns from it, and a branch that continued
    /// from a message there had no place left. Parked, the removed messages
    /// are a tail like any other — a variant of their place, one switch away
    /// (docs/research/chat-branches.md §3.10).
    pub fn park_tail(&mut self, cut: usize, cause: ParkCause, now: DateTime<Utc>) -> bool {
        if cut >= self.messages.len() {
            return false;
        }
        self.park_from(cut, cause, now);
        true
    }

    /// Takes `messages[cut..]` off the line and parks it with what was in
    /// force. The line's own summary and watermark are then cut to what the
    /// line still holds:
    ///
    /// - the watermark is lowered to the cut. Its clamp on read
    ///   (`reflect_window`) is enough for a take-back, which removes one
    ///   exchange; after a deep fork it would leave the new line's messages up
    ///   to the old watermark never reflected on (research §3.4);
    /// - a summary whose boundary left with the tail goes with it: it
    ///   describes what the line no longer holds.
    fn park_from(&mut self, cut: usize, cause: ParkCause, now: DateTime<Utc>) {
        if cut >= self.messages.len() {
            return;
        }
        let tail = self.messages.split_off(cut);
        self.branches.push(Branch {
            after: self.messages.last().map(|m| m.id),
            messages: tail,
            parked_at: now,
            cause,
            compaction: self.compaction.clone(),
            reflected_upto: self.reflected_upto,
        });
        self.reflected_upto = self.reflected_upto.map(|wm| wm.min(cut));
        if self
            .compaction
            .as_ref()
            .is_some_and(|c| !self.on_line(c.boundary_id))
        {
            self.compaction = None;
        }
        self.modified_at = now;
    }

    /// Puts the variant that starts with `first` on the line, parking what it
    /// displaces. A tail that continues from inside another parked tail needs
    /// that one on the line first, so the tails between it and the line are
    /// brought in outermost first; bringing a tail in under its own
    /// continuation splits the tail around it. On an error nothing is changed.
    pub fn switch_to(&mut self, first: Uuid, now: DateTime<Utc>) -> Result<(), SwitchError> {
        let mut path = vec![first];
        let mut at = self.branch_index(first).ok_or(SwitchError::Unknown)?;
        loop {
            match self.home(self.branches[at].after) {
                Home::Line(_) => break,
                Home::Nowhere => return Err(SwitchError::Detached),
                Home::Tail { branch, .. } => {
                    let parent = self.branches[branch].first().ok_or(SwitchError::Detached)?;
                    // A tail anchored inside a tail anchored inside it: a file
                    // edited by hand. The walk must end (research §3.1).
                    if path.contains(&parent) {
                        return Err(SwitchError::Detached);
                    }
                    path.push(parent);
                    at = branch;
                }
            }
        }
        for first in path.into_iter().rev() {
            self.bring_in(first, now);
        }
        Ok(())
    }

    /// One step of a switch: the tail starting with `first`, whose anchor is
    /// on the line, replaces the line's continuation past that anchor.
    fn bring_in(&mut self, first: Uuid, now: DateTime<Utc>) {
        let Some(at) = self.branch_index(first) else {
            return;
        };
        let Home::Line(cut) = self.home(self.branches[at].after) else {
            return;
        };
        // Pushed at the list's end: `at` still names the incoming tail.
        self.park_from(cut, ParkCause::Switch, now);
        let incoming = self.branches.remove(at);
        let kept = self.compaction.take();
        self.messages.extend(incoming.messages);
        // The summary the tail had and the one the line kept: whichever the
        // new line can place, the one that covers more of it. The boundary is
        // found by id, as `compaction_view` finds it.
        self.compaction = [incoming.compaction, kept]
            .into_iter()
            .flatten()
            .filter_map(|mut c| {
                c.upto = self.messages.iter().position(|m| m.id == c.boundary_id)?;
                Some(c)
            })
            .max_by_key(|c| c.upto);
        // The tail's own watermark is an index on the line it left, which is
        // this line again; the line's was just lowered to the shared prefix.
        self.reflected_upto = incoming
            .reflected_upto
            .into_iter()
            .chain(self.reflected_upto)
            .max()
            .map(|wm| wm.min(self.messages.len()));
        self.modified_at = now;
    }

    /// Every message the chat holds on the line and in its parked tails — the
    /// messages a reader of "is this message still the chat's" means. The
    /// `deleted` archive is not among them: what it holds is gone.
    pub fn branch_messages(&self) -> impl Iterator<Item = &Message> {
        self.branches.iter().flat_map(|b| b.messages.iter())
    }

    /// Where the parked message `message` stands; `None` when no parked tail
    /// holds it — it is on the line, or not the chat's at all. Changes
    /// nothing: the walk to the line is [`Self::switch_to`]'s, without the
    /// switch.
    pub fn parked_place(&self, message: Uuid) -> Option<ParkedPlace> {
        let holder = self
            .branches
            .iter()
            .position(|b| b.messages.iter().any(|m| m.id == message))?;
        let first = self.branches[holder].first()?;
        let detached = ParkedPlace {
            first,
            leaves_at: None,
            detached: true,
        };
        let mut seen = HashSet::from([holder]);
        let mut at = holder;
        let cut = loop {
            match self.home(self.branches[at].after) {
                Home::Line(cut) => break cut,
                Home::Nowhere => return Some(detached),
                Home::Tail { branch, .. } => {
                    // A ring of tails — a file edited by hand — ends the walk
                    // (research §3.1).
                    if !seen.insert(branch) {
                        return Some(detached);
                    }
                    at = branch;
                }
            }
        };
        Some(ParkedPlace {
            first,
            leaves_at: self
                .messages
                .get(cut)
                .or_else(|| self.messages.last())
                .map(|m| m.id),
            detached: false,
        })
    }

    fn on_line(&self, id: Uuid) -> bool {
        self.messages.iter().any(|m| m.id == id)
    }

    fn branch_index(&self, first: Uuid) -> Option<usize> {
        self.branches.iter().position(|b| b.first() == Some(first))
    }

    fn home(&self, after: Option<Uuid>) -> Home {
        let Some(id) = after else {
            return Home::Line(0);
        };
        if let Some(at) = self.messages.iter().position(|m| m.id == id) {
            return Home::Line(at + 1);
        }
        self.branches
            .iter()
            .enumerate()
            .find_map(|(branch, b)| {
                b.messages
                    .iter()
                    .position(|m| m.id == id)
                    .map(|pos| Home::Tail { branch, pos })
            })
            .unwrap_or(Home::Nowhere)
    }

    /// The chat's branches for the screen ([`BranchView`]).
    pub fn branch_view(&self) -> BranchView {
        if self.branches.is_empty() {
            return BranchView::default();
        }
        let homes: Vec<Home> = self.branches.iter().map(|b| self.home(b.after)).collect();
        // Each tail's number and depth, resolved from the line outwards: a
        // tail inside another is numbered once that one is. What a pass
        // cannot reach — an anchor that names nothing, a ring of tails — stays
        // unnumbered, and is listed as detached.
        let mut placed: Vec<Option<(u32, u16)>> = vec![None; self.branches.len()];
        loop {
            let mut progressed = false;
            for (i, home) in homes.iter().enumerate() {
                if placed[i].is_some() {
                    continue;
                }
                let at = match *home {
                    Home::Line(cut) => {
                        Some((visible_message_count(&self.messages[..cut]) as u32 + 1, 0))
                    }
                    Home::Tail { branch, pos } => placed[branch].map(|(number, depth)| {
                        let before = &self.branches[branch].messages[..=pos];
                        (number + visible_message_count(before) as u32, depth + 1)
                    }),
                    Home::Nowhere => None,
                };
                if at.is_some() {
                    placed[i] = at;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        // The variants of a place: the parked tails that continue from it and
        // what continues past it where it stands, by their first message's
        // time. A variant is its first message's id.
        let variants_of = |after: Option<Uuid>| -> Vec<Uuid> {
            let standing = match self.home(after) {
                Home::Line(cut) => self.messages.get(cut),
                Home::Tail { branch, pos } => self.branches[branch].messages.get(pos + 1),
                Home::Nowhere => None,
            };
            let mut firsts: Vec<&Message> = self
                .branches
                .iter()
                .filter(|b| b.after == after)
                .filter_map(|b| b.messages.first())
                .chain(standing)
                .collect();
            firsts.sort_by_key(|m| (m.timestamp, m.id));
            firsts.into_iter().map(|m| m.id).collect()
        };
        let rank = |variants: &[Uuid], first: Uuid| -> u16 {
            variants
                .iter()
                .position(|v| *v == first)
                .map_or(0, |i| i + 1) as u16
        };

        let info = |i: usize| -> Option<BranchInfo> {
            let b = &self.branches[i];
            let first = b.first()?;
            let variants = variants_of(b.after);
            let (number, depth) = placed[i].unwrap_or((0, 0));
            Some(BranchInfo {
                first,
                number,
                variant: rank(&variants, first),
                of: variants.len() as u16,
                bubbles: visible_message_count(&b.messages),
                role: b.messages[0].role,
                preview: preview_of(&b.messages),
                at: b.messages.last().map_or(b.parked_at, |m| m.timestamp),
                depth,
                at_end: homes[i] == Home::Line(self.messages.len()),
                detached: placed[i].is_none(),
            })
        };

        // Tree order: the tails that continue from the line, by number and
        // variant; under each, the tails that continue from inside it.
        let mut parked: Vec<BranchInfo> = Vec::with_capacity(self.branches.len());
        let mut listed: HashSet<usize> = HashSet::new();
        let by_place = |ids: &mut Vec<usize>| {
            ids.sort_by_key(|&i| {
                let (number, _) = placed[i].unwrap_or((0, 0));
                (
                    number,
                    self.branches[i].messages.first().map(|m| m.timestamp),
                )
            });
        };
        let mut roots: Vec<usize> = (0..self.branches.len())
            .filter(|&i| matches!(homes[i], Home::Line(_)))
            .collect();
        by_place(&mut roots);
        // Depth first, with an explicit stack: children pushed in reverse so
        // they come off in order.
        let mut stack: Vec<usize> = roots.into_iter().rev().collect();
        while let Some(i) = stack.pop() {
            if !listed.insert(i) {
                continue;
            }
            parked.extend(info(i));
            let mut children: Vec<usize> = (0..self.branches.len())
                .filter(|&c| {
                    placed[c].is_some()
                        && matches!(homes[c], Home::Tail { branch, .. } if branch == i)
                })
                .collect();
            by_place(&mut children);
            stack.extend(children.into_iter().rev());
        }
        // What the walk did not reach: detached tails, last.
        parked.extend(
            (0..self.branches.len())
                .filter(|i| !listed.contains(i))
                .filter_map(info),
        );

        // The line's own marks: at every place on the line that has parked
        // variants and a continuation of the line's own.
        let mut seen: HashSet<Option<Uuid>> = HashSet::new();
        let mut marks: Vec<LineMark> = Vec::new();
        for (i, b) in self.branches.iter().enumerate() {
            let Home::Line(cut) = homes[i] else {
                continue;
            };
            if !seen.insert(b.after) {
                continue;
            }
            let Some(standing) = self.messages.get(cut) else {
                continue;
            };
            let variants = variants_of(b.after);
            marks.push(LineMark {
                message: standing.id,
                variant: rank(&variants, standing.id),
                of: variants.len() as u16,
            });
        }
        BranchView { marks, parked }
    }
}

/// The first line of the first message of `messages` that has text — what a
/// list shows of a tail. A reply that opened with tool calls has none of its
/// own; the next message with text speaks for it.
fn preview_of(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role != MessageRole::Tool)
        .find_map(|m| m.text.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::ToolCallRecord;
    use crate::entities::profile::Profile;
    use chrono::{Duration, TimeZone};

    fn t(minutes: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 9, 0, 0).unwrap() + Duration::minutes(minutes)
    }

    fn msg(role: MessageRole, text: &str, minutes: i64) -> Message {
        let mut m = Message::new(role, text);
        m.timestamp = t(minutes);
        m
    }

    fn user(text: &str, minutes: i64) -> Message {
        msg(MessageRole::User, text, minutes)
    }

    fn reply(text: &str, minutes: i64) -> Message {
        msg(MessageRole::Assistant, text, minutes)
    }

    /// Three exchanges: `q1 a1 q2 a2 q3 a3`, a minute apart.
    fn three_exchanges() -> Chat {
        let mut chat = Chat::from_profile(&Profile::new("P", "sys"), "t");
        chat.messages = vec![
            user("q1", 0),
            reply("a1", 1),
            user("q2", 2),
            reply("a2", 3),
            user("q3", 4),
            reply("a3", 5),
        ];
        chat
    }

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|m| m.text.as_str()).collect()
    }

    /// The invariants of research §3.3, checked after every step of a test.
    fn assert_sound(chat: &Chat) {
        let mut ids: HashSet<Uuid> = HashSet::new();
        for m in chat.messages.iter().chain(chat.branch_messages()) {
            assert!(ids.insert(m.id), "a message is held twice: {:?}", m.text);
        }
        for b in &chat.branches {
            assert!(!b.messages.is_empty(), "an empty tail is parked");
            if let Some(after) = b.after {
                assert!(ids.contains(&after), "an anchor names no message");
            }
        }
    }

    /// A fork at a reply cuts after it, tool results included; at the
    /// person's message, before it, with the text to write again; at a note
    /// row, before it. A message the line does not show cuts nothing.
    #[test]
    fn a_fork_cuts_so_that_the_person_speaks_next() {
        let mut chat = three_exchanges();
        let mut round = reply("", 6);
        round.tool_calls = vec![ToolCallRecord {
            id: "c1".into(),
            name: "web_search".into(),
            arguments: serde_json::json!({}),
            result: Some("found".into()),
            thought_signature: None,
            images: 0,
            subagent: None,
        }];
        let mut result = msg(MessageRole::Tool, "found", 7);
        result.tool_call_id = Some("c1".into());
        let notice = Message::notification(Uuid::new_v4(), "run finished");
        chat.messages.extend([
            user("q4", 6),
            round,
            result,
            reply("a4", 8),
            notice,
            reply("woken", 10),
        ]);
        let id = |i: usize| chat.messages[i].id;

        // a1 — the second bubble: the line keeps q1 a1.
        assert_eq!(
            chat.fork_cut(id(1)),
            Some(ForkCut {
                cut: 2,
                restore: None
            })
        );
        // The reply made of two rounds and a tool result is one bubble: any of
        // its messages names it, and the cut is after its last.
        for i in [7, 8, 9] {
            assert_eq!(chat.fork_cut(id(i)).map(|c| c.cut), Some(10), "message {i}");
        }
        // The person's own message: before it, and its text comes back.
        assert_eq!(
            chat.fork_cut(id(4)),
            Some(ForkCut {
                cut: 4,
                restore: Some("q3".into())
            })
        );
        // A notification: before it, and nothing to write again.
        assert_eq!(
            chat.fork_cut(id(10)),
            Some(ForkCut {
                cut: 10,
                restore: None
            })
        );
        assert_eq!(chat.fork_cut(Uuid::new_v4()), None);
    }

    /// The round trip the whole design stands on: a fork, then a switch back,
    /// restores the line message for message and leaves nothing parked.
    #[test]
    fn a_fork_and_a_switch_back_restore_the_line() {
        let mut chat = three_exchanges();
        let before = chat.messages.clone();
        let q3 = chat.messages[4].id;

        assert!(chat.fork_at(4, t(10)));
        assert_sound(&chat);
        assert_eq!(texts(&chat.messages), ["q1", "a1", "q2", "a2"]);
        assert_eq!(chat.branches.len(), 1);
        assert_eq!(chat.branches[0].after, Some(before[3].id));
        assert_eq!(chat.branches[0].cause, ParkCause::Fork);
        assert_eq!(chat.modified_at, t(10));

        // The line has no continuation of its own: the switch parks nothing.
        chat.switch_to(q3, t(11)).unwrap();
        assert_sound(&chat);
        assert_eq!(chat.messages, before);
        assert!(chat.branches.is_empty());
    }

    /// Nothing follows the line's last reply, so there is nothing to fork
    /// from — and a cut past the end is the same.
    #[test]
    fn a_fork_with_nothing_after_it_is_not_one() {
        let mut chat = three_exchanges();
        let before = chat.clone();
        assert!(!chat.fork_at(6, t(10)));
        assert!(!chat.fork_at(99, t(10)));
        assert_eq!(chat, before);
    }

    /// A fork at the chat's first message parks the whole line: the tail
    /// starts the chat, and has no anchor.
    #[test]
    fn a_fork_at_the_first_message_parks_the_line() {
        let mut chat = three_exchanges();
        let before = chat.messages.clone();
        assert!(chat.fork_at(0, t(10)));
        assert!(chat.messages.is_empty());
        assert_eq!(chat.branches[0].after, None);
        assert_sound(&chat);

        let view = chat.branch_view();
        assert_eq!(view.marks, vec![]);
        assert_eq!(view.parked.len(), 1);
        assert_eq!((view.parked[0].number, view.parked[0].at_end), (1, true));

        chat.messages.push(user("another start", 20));
        chat.switch_to(before[0].id, t(21)).unwrap();
        assert_eq!(chat.messages, before);
        assert_eq!(texts(&chat.branches[0].messages), ["another start"]);
        assert_eq!(chat.branches[0].cause, ParkCause::Switch);
    }

    /// Two forks, the second above the first: the first tail's anchor is then
    /// inside the second. A switch to it brings the outer tail in first and
    /// splits it around the inner one — and the line is the original again.
    #[test]
    fn a_nested_tail_is_reached_through_the_tail_that_holds_its_anchor() {
        let mut chat = three_exchanges();
        let original = chat.messages.clone();
        let (q2, q3) = (original[2].id, original[4].id);

        // Back to q3, and another way from there.
        assert!(chat.fork_at(4, t(10)));
        chat.messages.extend([user("q3'", 11), reply("a3'", 12)]);
        // Back to q2, and another way from there: the tail now holds a2,
        // which the first tail continues from.
        assert!(chat.fork_at(2, t(20)));
        chat.messages.extend([user("q2''", 21), reply("a2''", 22)]);
        assert_sound(&chat);
        assert_eq!(texts(&chat.messages), ["q1", "a1", "q2''", "a2''"]);

        let view = chat.branch_view();
        let listed: Vec<(&str, u32, u16, u16, u16)> = view
            .parked
            .iter()
            .map(|b| (b.preview.as_str(), b.number, b.variant, b.of, b.depth))
            .collect();
        assert_eq!(
            listed,
            [
                // Continues from a1, on the line: the older of two variants.
                ("q2", 3, 1, 2, 0),
                // Continues from a2, inside it: the older of two there.
                ("q3", 5, 1, 2, 1),
            ]
        );
        assert_eq!(
            view.marks,
            vec![LineMark {
                message: chat.messages[2].id,
                variant: 2,
                of: 2
            }]
        );

        chat.switch_to(q3, t(30)).unwrap();
        assert_sound(&chat);
        assert_eq!(chat.messages, original);
        let parked: Vec<(Vec<&str>, Option<Uuid>)> = chat
            .branches
            .iter()
            .map(|b| (texts(&b.messages), b.after))
            .collect();
        assert_eq!(
            parked,
            [
                (vec!["q2''", "a2''"], Some(original[1].id)),
                (vec!["q3'", "a3'"], Some(original[3].id)),
            ]
        );

        // The marks follow: both places are on the line now, and the line
        // holds the older variant of each.
        let marks = chat.branch_view().marks;
        assert_eq!(
            marks,
            vec![
                LineMark {
                    message: q2,
                    variant: 1,
                    of: 2
                },
                LineMark {
                    message: q3,
                    variant: 1,
                    of: 2
                },
            ]
        );
    }

    /// A tail is addressed by the places from the line down to it, and a
    /// parked message is placed without moving anything: the tail that holds
    /// it, and the message of the line that stands where that tail — or the
    /// outermost tail on its way to the line — leaves it.
    #[test]
    fn a_parked_message_is_addressed_and_placed_without_a_switch() {
        let mut chat = three_exchanges();
        let original = chat.messages.clone();
        assert!(chat.fork_at(4, t(10)));
        chat.messages.extend([user("q3'", 11), reply("a3'", 12)]);
        assert!(chat.fork_at(2, t(20)));
        chat.messages.extend([user("q2''", 21), reply("a2''", 22)]);
        let before = chat.clone();

        let view = chat.branch_view();
        assert_eq!(view.paths(), vec![vec![(3, 1)], vec![(3, 1), (5, 1)]]);
        let addresses: Vec<String> = view.paths().iter().map(|p| branch_address(p)).collect();
        assert_eq!(addresses, ["3/1", "3/1 > 5/1"]);
        assert_eq!(view.path_of(original[4].id), Some(vec![(3, 1), (5, 1)]));
        assert_eq!(view.path_of(original[5].id), None, "not a tail's first");

        // a2 is in the tail that continues from the line after a1: the line's
        // own variant of that place is q2''.
        let standing = chat.messages[2].id;
        assert_eq!(
            chat.parked_place(original[3].id),
            Some(ParkedPlace {
                first: original[2].id,
                leaves_at: Some(standing),
                detached: false,
            })
        );
        // a3 is in the tail inside it — named by its own first message, and
        // looked at where the outer tail leaves the line.
        assert_eq!(
            chat.parked_place(original[5].id),
            Some(ParkedPlace {
                first: original[4].id,
                leaves_at: Some(standing),
                detached: false,
            })
        );
        // A message of the line, and one that is not the chat's.
        assert_eq!(chat.parked_place(chat.messages[0].id), None);
        assert_eq!(chat.parked_place(Uuid::new_v4()), None);
        assert_eq!(chat, before, "a look moves nothing");

        // A tail that continues from the line's end: there is no variant
        // standing there, so the place to look at is the line's last message.
        let mut fresh = three_exchanges();
        assert!(fresh.fork_at(4, t(10)));
        let last = fresh.messages.last().unwrap().id;
        let parked = fresh.branches[0].messages[1].id;
        assert_eq!(
            fresh.parked_place(parked).and_then(|p| p.leaves_at),
            Some(last)
        );

        // A detached tail is named and has no place; so is a ring of tails.
        let mut lost = three_exchanges();
        lost.branches.push(Branch {
            after: Some(Uuid::new_v4()),
            messages: vec![user("lost", 30)],
            parked_at: t(31),
            cause: ParkCause::Fork,
            compaction: None,
            reflected_upto: None,
        });
        let id = lost.branches[0].messages[0].id;
        assert_eq!(
            lost.parked_place(id),
            Some(ParkedPlace {
                first: id,
                leaves_at: None,
                detached: true,
            })
        );
        assert_eq!(lost.branch_view().paths(), vec![vec![(0, 1)]]);
        assert_eq!(branch_address(&[(0, 1)]), "0/1");
    }

    /// A variant's figure is its place among the variants by their first
    /// message's time, so it does not change when the line moves between them.
    #[test]
    fn a_variants_figure_survives_a_switch() {
        let mut chat = three_exchanges();
        let q3 = chat.messages[4].id;
        assert!(chat.fork_at(4, t(10)));
        chat.messages.extend([user("second", 11), reply("x", 12)]);
        let second = chat.messages[4].id;
        assert!(chat.fork_at(4, t(20)));
        chat.messages.extend([user("third", 21)]);
        let third = chat.messages[4].id;

        let figure = |chat: &Chat, first: Uuid| -> (u16, u16) {
            let view = chat.branch_view();
            view.parked
                .iter()
                .find(|b| b.first == first)
                .map(|b| (b.variant, b.of))
                .or_else(|| {
                    view.marks
                        .iter()
                        .find(|m| m.message == first)
                        .map(|m| (m.variant, m.of))
                })
                .expect("the variant is parked or on the line")
        };
        for line in [third, q3, second, third] {
            if chat.messages.get(4).map(|m| m.id) != Some(line) {
                chat.switch_to(line, t(30)).unwrap();
            }
            assert_sound(&chat);
            assert_eq!(figure(&chat, q3), (1, 3));
            assert_eq!(figure(&chat, second), (2, 3));
            assert_eq!(figure(&chat, third), (3, 3));
        }
    }

    /// A fork leaves the line ending on a message a tail continues from:
    /// that tail is `at_end`, and the line carries no mark for it — there is
    /// no bubble to draw one on until the person writes.
    #[test]
    fn a_fresh_fork_is_at_the_lines_end() {
        let mut chat = three_exchanges();
        assert!(chat.fork_at(4, t(10)));
        let view = chat.branch_view();
        assert_eq!(view.marks, vec![]);
        let tail = &view.parked[0];
        assert!(tail.at_end);
        assert_eq!((tail.number, tail.variant, tail.of), (5, 1, 1));
        assert_eq!((tail.bubbles, tail.role), (2, MessageRole::User));
        assert_eq!(tail.at, t(5), "its last message's time");

        chat.messages.push(user("another way", 11));
        let view = chat.branch_view();
        assert!(!view.parked[0].at_end);
        assert_eq!(view.marks.len(), 1);
    }

    /// A summary whose boundary leaves with the tail goes with it and comes
    /// back with it; one whose boundary stays on the line stays, and is the
    /// line's on both sides of a switch.
    #[test]
    fn the_summary_follows_its_boundary() {
        let summary = |chat: &Chat, upto: usize| Compaction {
            summary: format!("covers {upto}"),
            upto,
            boundary_id: chat.messages[upto].id,
            compacted_at: t(6),
            rolls: 1,
        };

        // The boundary is q3; the fork is at q2, before it.
        let mut chat = three_exchanges();
        let q2 = chat.messages[2].id;
        let whole = summary(&chat, 4);
        chat.compaction = Some(whole.clone());
        assert!(chat.fork_at(2, t(10)));
        assert_eq!(chat.compaction, None, "it describes what the line lost");
        assert_eq!(chat.compaction_view(true), None);
        assert_eq!(chat.branches[0].compaction, Some(whole.clone()));
        chat.switch_to(q2, t(11)).unwrap();
        assert_eq!(chat.compaction, Some(whole));
        assert_eq!(chat.compaction_view(true).map(|(_, upto)| upto), Some(4));

        // The boundary is q2; the fork is at q3, after it.
        let mut chat = three_exchanges();
        let q3 = chat.messages[4].id;
        let prefix = summary(&chat, 2);
        chat.compaction = Some(prefix.clone());
        assert!(chat.fork_at(4, t(10)));
        assert_eq!(chat.compaction, Some(prefix.clone()), "still the line's");
        // The new line grows and its summary rolls forward, past the fork.
        chat.messages
            .extend([user("q3'", 11), reply("a3'", 12), user("q4'", 13)]);
        let rolled = summary(&chat, 6);
        chat.compaction = Some(rolled.clone());
        chat.switch_to(q3, t(20)).unwrap();
        assert_eq!(
            chat.compaction,
            Some(prefix),
            "the returning tail's copy: the rolled one's boundary is parked"
        );
        assert_eq!(chat.branches[0].compaction, Some(rolled));
    }

    /// The watermark is lowered to the cut, so the new line is reflected on
    /// from there; the parked tail keeps its own and gets it back, so what
    /// was reflected on is not read again.
    #[test]
    fn the_watermark_is_cut_with_the_line_and_returns_with_the_tail() {
        let mut chat = three_exchanges();
        let q2 = chat.messages[2].id;
        chat.reflected_upto = Some(6);
        assert!(chat.fork_at(2, t(10)));
        assert_eq!(chat.reflected_upto, Some(2));
        assert_eq!(chat.branches[0].reflected_upto, Some(6));

        // The new line is written and reflected on.
        chat.messages.extend([user("q2'", 11), reply("a2'", 12)]);
        chat.reflected_upto = Some(4);
        chat.switch_to(q2, t(20)).unwrap();
        assert_eq!(chat.reflected_upto, Some(6), "the old line's own");
        assert_eq!(chat.branches[0].reflected_upto, Some(4));

        // A fork past the watermark leaves it alone.
        let mut chat = three_exchanges();
        chat.reflected_upto = Some(2);
        assert!(chat.fork_at(4, t(10)));
        assert_eq!(chat.reflected_upto, Some(2));
    }

    /// A tail whose anchor names nothing — a file edited by hand — is kept,
    /// listed as detached, and a switch to it changes nothing. So does a ring
    /// of tails.
    #[test]
    fn a_detached_tail_is_listed_and_not_switched_to() {
        let mut chat = three_exchanges();
        assert!(chat.fork_at(4, t(10)));
        let lost = chat.branches[0].first().unwrap();
        chat.branches[0].after = Some(Uuid::new_v4());
        let before = chat.clone();

        assert_eq!(chat.switch_to(lost, t(11)), Err(SwitchError::Detached));
        assert_eq!(chat, before);
        let view = chat.branch_view();
        assert_eq!(view.parked.len(), 1);
        assert!(view.parked[0].detached);
        assert_eq!(view.parked[0].number, 0);
        assert_eq!(view.marks, vec![]);

        assert_eq!(
            chat.switch_to(Uuid::new_v4(), t(11)),
            Err(SwitchError::Unknown)
        );

        // Two tails, each anchored inside the other.
        let mut chat = three_exchanges();
        assert!(chat.fork_at(4, t(10)));
        chat.messages.push(user("x", 11));
        assert!(chat.fork_at(4, t(12)));
        let (a, b) = (
            chat.branches[0].messages[0].id,
            chat.branches[1].messages[0].id,
        );
        chat.branches[0].after = Some(b);
        chat.branches[1].after = Some(a);
        let before = chat.clone();
        assert_eq!(chat.switch_to(a, t(13)), Err(SwitchError::Detached));
        assert_eq!(chat, before);
        assert!(chat.branch_view().parked.iter().all(|b| b.detached));
    }

    /// A chat with no branch writes the bytes it wrote before; one with
    /// branches round-trips, the cause in its wire form.
    #[test]
    fn branches_are_stored_only_where_there_are_some() {
        let mut chat = three_exchanges();
        let json = serde_json::to_string(&chat).unwrap();
        assert!(!json.contains("branches"), "{json}");

        chat.reflected_upto = Some(6);
        assert!(chat.fork_at(4, t(10)));
        let value = serde_json::to_value(&chat).unwrap();
        let tail = &value["branches"][0];
        assert_eq!(tail["cause"], serde_json::json!("fork"));
        assert_eq!(
            tail["after"],
            serde_json::json!(chat.messages[3].id.to_string())
        );
        assert_eq!(tail["reflected_upto"], serde_json::json!(6));
        assert!(tail.get("compaction").is_none());
        let back: Chat = serde_json::from_value(value).unwrap();
        assert_eq!(back, chat);
    }

    /// The walks that are about the file, not the conversation, reach a
    /// parked tail: a background run lands on its record wherever the
    /// exchange went, and a clone gives a parked transcript an id of its own.
    /// The line's own list of transcripts does not hold it — it is off the
    /// line.
    #[test]
    fn a_transcript_in_a_parked_tail_is_reached_by_the_files_walks() {
        let mut chat = three_exchanges();
        let run = crate::entities::subagent::SubagentRun::fixture("run", &["do", "done"]);
        let run_id = run.id;
        chat.messages[5].tool_calls = vec![run.on_record()];
        assert_eq!(chat.children().count(), 1);

        assert!(chat.fork_at(4, t(10)));
        assert_eq!(chat.children().count(), 0, "off the line");
        assert!(chat.child(run_id).is_none());
        assert!(chat.child_mut_including_deleted(run_id).is_some());

        chat.reid_children();
        assert!(chat.child_mut_including_deleted(run_id).is_none());
        let parked = chat.branches[0].messages[1].tool_calls[0]
            .subagent
            .as_deref()
            .unwrap();
        assert_ne!(parked.id, run_id);
        assert!(!chat.is_pristine());
    }

    /// A regeneration and a take-back cut the line's end as a fork does, and
    /// what they cut is parked under their own cause — never archived. The
    /// replaced reply is the first variant of its place, and a tail that
    /// continued from what was cut is still reached, through it.
    #[test]
    fn a_regeneration_and_a_take_back_park_what_they_cut() {
        let mut chat = three_exchanges();
        let original = chat.messages.clone();
        let (a3, q3) = (original[5].id, original[4].id);

        // `Ctrl+R`: the last reply leaves, the question stays.
        assert!(chat.park_tail(5, ParkCause::Regenerate, t(10)));
        assert_sound(&chat);
        assert_eq!(texts(&chat.messages), ["q1", "a1", "q2", "a2", "q3"]);
        assert_eq!(chat.branches[0].cause, ParkCause::Regenerate);
        assert_eq!(chat.branches[0].after, Some(q3));
        assert_eq!(texts(&chat.branches[0].messages), ["a3"]);
        assert!(chat.deleted.is_empty(), "nothing is archived");

        // The new reply is variant 2 of the place; the old one is variant 1.
        chat.messages.push(reply("a3 again", 11));
        let again = chat.messages[5].id;
        let view = chat.branch_view();
        assert_eq!(
            view.marks,
            [LineMark {
                message: again,
                variant: 2,
                of: 2
            }]
        );
        assert_eq!((view.parked[0].variant, view.parked[0].of), (1, 2));

        // `Ctrl+E`: the exchange leaves whole, and the replaced reply is now
        // inside what left — still reached, through the exchange.
        assert!(chat.park_tail(4, ParkCause::Takeback, t(12)));
        assert_sound(&chat);
        assert_eq!(texts(&chat.messages), ["q1", "a1", "q2", "a2"]);
        let causes: Vec<ParkCause> = chat.branches.iter().map(|b| b.cause).collect();
        assert_eq!(causes, [ParkCause::Regenerate, ParkCause::Takeback]);
        let view = chat.branch_view();
        assert!(view.parked.iter().all(|b| !b.detached), "{view:?}");
        assert!(chat.deleted.is_empty());

        // One switch to the first reply puts the conversation back as it was.
        chat.switch_to(a3, t(13)).unwrap();
        assert_sound(&chat);
        assert_eq!(chat.messages, original);

        // Nothing follows the cut: nothing is parked, and the answer says so.
        let mut chat = three_exchanges();
        assert!(!chat.park_tail(6, ParkCause::Regenerate, t(10)));
        assert!(chat.branches.is_empty());
        assert_eq!(chat.messages.len(), 6);
    }

    /// What a list shows of a tail: the first line with text, and a reply
    /// that opened with tool calls is spoken for by what follows.
    #[test]
    fn a_tails_preview_is_its_first_line_with_text() {
        let mut call = reply("", 0);
        call.tool_calls = vec![ToolCallRecord {
            id: "c".into(),
            name: "t".into(),
            arguments: serde_json::json!({}),
            result: None,
            thought_signature: None,
            images: 0,
            subagent: None,
        }];
        let result = msg(MessageRole::Tool, "tool output", 1);
        let text = reply("\n  The answer.  \nMore.", 2);
        assert_eq!(preview_of(&[call, result, text]), "The answer.");
        assert_eq!(preview_of(&[]), "");
    }
}
