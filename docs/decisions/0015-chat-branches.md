# ADR 0015 — Branches inside a chat: the line, and the tails kept beside it

**Status:** accepted (2026-11-18). Design, the alternatives and the decided forks —
[docs/research/chat-branches.md](../research/chat-branches.md). Implemented on
`feat/chat-branches`:
[src/entities/chat_branch.rs](../../src/entities/chat_branch.rs) (`Branch`,
`Chat::fork_at`, `Chat::switch_to`, `Chat::branch_view`),
[src/app/orchestrator/branches.rs](../../src/app/orchestrator/branches.rs)
(`handle_fork_at`, `handle_switch_branch`),
[src/shared/storage/chat_steps.rs](../../src/shared/storage/chat_steps.rs)
(`chat_to_v9`, `message_lists_mut`), and — the reader of point 9 —
[src/features/tools/branches.rs](../../src/features/tools/branches.rs)
(`BranchRead`, `BranchShelf`). Related:
[ADR 0006](0006-data-schema-versioning.md) (the bump policy this decision reads
one way for one field).

## Context

A conversation could be changed only at its end: `Ctrl+E` takes the last exchange
back, `Ctrl+R` redoes the last reply, and both put what they remove into
`Chat.deleted`, an archive nothing reads. `/clone` copies a whole chat. There was no
way to go back to a message in the middle and continue from it, and no way back to
what a take-back removed.

`Chat.messages` is read as a linear conversation across the code — the request, the
rolling summary, reflection, the feed, search, export, the tools. Anything that
changes what that field *means* changes every one of those readers, every chat file,
and every test that builds a chat.

## Decision

1. **A chat is a line, with the tails that left it kept beside it.**
   `Chat.messages` stays the conversation in front of the person. A new field,
   `Chat.branches`, holds every tail that is off that line; each names the last
   message it shares with the line it left (`Branch::after`, `None` for a tail that
   starts the chat). The tree is implied by the anchors: message ids are unique
   within a chat, so an anchor names one place, on the line or inside another parked
   tail.
2. **A variant is named by its first message.** No branch id is stored. The variants
   of a place — the tails anchored there and whatever continues past it where it
   stands — are ordered by their first message's time, so "variant 2 of 3" means the
   same before and after any switch.
3. **Two operations, both pure functions of the chat.** A *fork* cuts the line at a
   bubble's edge and parks the tail. A *switch* puts a parked tail back and parks
   what it displaces; a tail anchored inside another is reached through it, the
   outer one brought in first and split around the inner. A parked tail is the line
   as it was left, so returning to it restores the conversation to its last message
   with no bookkeeping of "the last active child".
4. **What hangs off a place on the line travels with the tail.** A parked tail keeps
   a copy of the rolling summary and the reflection watermark in force when it left
   and gets them back. On the line, a summary whose boundary left is dropped and the
   watermark is lowered to the cut.
5. **To everything outside the chat file, a fork and a switch are a truncation.**
   The orchestrator makes the calls a take-back makes — the feed rebuilt, a
   background run whose record left the line cancelled, a follow-up planned in what
   left cancelled. No reader of `messages` is given a filter for branches, because
   none needs one: the line is all it ever sees.
6. **The assistant reads the line.** The request, the tools' snapshots and the
   full-text index are built from `messages`. Nothing about a fork enters the
   prompt. (A read of a line that says a chat has branches, and the tool that
   reads them, are point 9.)
7. **The field takes a schema step, 8 → 9, though it is additive in shape.** ADR
   0006 §8 exempts a new field with a default. The exemption rests on what an older
   build loses when it ignores such a field, and here that is the conversation: it
   would write the file back without the branches at its first save. The stamp makes
   it refuse the data as a newer version's instead.
8. **`Ctrl+E` and `Ctrl+R` park what they cut.** `Chat.deleted` is where what is
   gone goes, and a branch anchored on a message there could never return to the
   line — so the tree's first build already parked a removal of what a branch
   continued from. The track's third stage (2026-11-18) made it the rule: both keys
   are a fork's cut at the line's last exchange, under causes of their own
   (`Regenerate`, `Takeback`), and the replaced reply is the first variant of its
   place. `Chat.deleted` grows only by a reply the assistant itself rewrote — its own
   draft, not a continuation the person may return to. **Reflection's behavioural
   signal moved with the causes**: it counts the tails parked under those two since
   it last ran, plus the archive's older records; a reply the person switched back to
   leaves none.
9. **A parked tail is read by one tool, which the person switches on.** The
   track's fourth stage (2026-11-19). `branch_read` lists a conversation's parked
   tails and reads one as a transcript, addressed as `/branch N K` addresses it; a
   read of a line — `history_read`, `chat_read` — says that branches stand beside
   it. Both only in a profile that has the tool, and it is off by default: a tail
   is outside what the person sees in a feed, and since point 8 it holds what they
   took back. The tool is not gated on the chat having branches: appearing with
   the first fork, it would tell the model of the fork (point 6) and change the
   tool list a cached prefix begins with. The searches hold the line only, as
   before.
10. **A parked word is found by a tool of its own, which scans.** Added after the
    track closed (2026-11-19). `branch_search` finds the tail that says a query's
    words — what the reader's list, which shows first lines, cannot — and names
    the page `branch_read` returns it on. It is a second switch of the profile,
    off, beside the reader's: the fourth pair of one pattern (`history_*`,
    `chat_*`, `attachment_*`), not a mode of the reader. It reads the turn's
    snapshot of the tails, not the full-text index: the index holds the lines,
    which is what keeps `chat_search` and `history_search` from ever returning
    what a fork cut. Each of the pair names the other only on a turn that has it.

## Alternatives rejected

- **A pool of nodes with parent pointers** — the shape ChatGPT's and DeepSeek's
  exports have. The same tree, stored so that `messages` is no longer the
  conversation: every reader changes, every file is rewritten by a real migration,
  and a damaged pointer damages the line the person is reading. The clients built on
  that shape report exactly this class of defect.
- **A copy of the chat per fork.** The shared prefix is stored twice, found twice by
  search and reflected on twice, and the chat's files and its attached project belong
  to one of the copies. It stays available as `/clone`, then `/fork N` in the copy.
- **Anchors on `Chat.deleted`'s records.** One container fewer, but its readers take
  `deleted` to mean "gone", and holding the person's live branches under that name
  makes the file say the opposite of what it holds.

## Consequences

- A chat file can hold several times the conversation the person sees. A parked tail
  keeps its messages whole, images included, as `deleted` already does; nothing
  bounds either.
- A chat file now has **three** lists of messages. Typed walks go through
  `Chat::stored_messages_mut`; migration steps through `chat_steps::message_lists_mut`.
  A walk that names the lists itself is the defect to look for in review.
- The person's own search does not reach a parked tail: a word said only in a branch
  is found once that branch is on the line. The assistant's searches of a line do
  not either; what it can do is read a tail it was asked about, where the profile
  has `branch_read`, and find the tail that says a word, where it has
  `branch_search`.
- A fork does not rewind what the conversation did outside itself — notes, the
  self-model, the chat's files, the attached project.
- A live feed can number a bubble the chat did not keep. A command that names a
  message by its number therefore resolves it on the screen and sends the message's
  id; a sent message's echo carries its id from its first frame.
- 0.27.0 and older refuse data this version wrote. The way back is the backup made
  before the step.
