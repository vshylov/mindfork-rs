# A stop gives the window back — the silent task returns at the next landing

> **Status:** proposed (2026-09-08) — the forks in §5 await the user's
> decision; no stage-0 probe (nothing about a model's behaviour is in
> question). The item the stop track recorded and did not take
> ([stop-silent-task.md](stop-silent-task.md) §7, its fork F4b): a stopped
> task keeps the bookkeeping it advanced at spawn, so the window it was
> reading is **skipped** — a stop is a skip. The premise of this track is
> the other reading, the one the stop track named as the condition for
> revisiting: users stop a task to *postpone* it, not to lose what it was
> about to read, so the window should come back.

## 1. Why, precisely

Three of the four silent tasks advance their cadence **at spawn**, not at
landing: reflection moves `Chat.reflected_upto` (and `reflected_at`) to the
end of the history, the two consolidations reset their per-chat reply
counters to zero. The lane and preemption tracks kept every one of these at
spawn and argued why ([silent-tasks-budget.md](silent-tasks-budget.md) §4,
[silent-preemption.md](silent-preemption.md) §2.3): a task that fails after
its first round, or is displaced and retried, has *read* its window — the
advance is honest, and a failure that re-read the same window at every
landing would be a loop. The stop track inherited that and read a stop as
a skip (its §2.4): "nothing needs refunding, and re-reading the window at
the next cadence would make the stopped task come back sooner, which is the
opposite of what a stop asks."

That last clause is the premise this track reverses, on the user's
decision. The cases where a user reaches for the stop — a reflection queued
behind a turn they want the engine for, a consolidation starting while they
are about to close the laptop, a roll they did not expect — are cases of
*not now*, and the window the task was about to read is the very thing a
*not now* wants kept. Today it is lost: the next reflection begins where
the stopped one would have ended, and the replies in between are never
reflected on.

What has to stay true is the argument that put the advance at spawn in the
first place: a window the task has **acted on** must not be read twice.
Reflection's digest is over the window only, so that "every cycle would
not re-read what's already been reflected on and produce duplicate
insights" (`reflection.rs`); a reflection stopped after its first round has
already written its observations from that window, and giving the window
back would write them again — twice in the self-model, with nothing to
merge them unless self-consolidation is on (it is off by default).

**Requirements.**

- **R1. A stop gives the window back when the task had not yet acted on
  it** — reflection's watermark and stamp restored, a consolidation's
  counter restored — so the task is due again at the next landing, over the
  same replies and whatever came after.
- **R2. A window the task acted on stays advanced.** The fact comes from
  the task itself: whether a round of its tools ran before the stop.
- **R3. Nothing else lands differently.** The streak untouched, the slot
  cleared, `SelfModelChanged` announced — everything the stop track decided
  (its §3.3) stays; `Done`, `Failed` and the roll are untouched.
- **R4. Persistence follows the advance.** Reflection's watermark lives in
  the chat file; its refund is saved the way its advance was.

## 2. What exists (inventory)

### 2.1 The three advances and the roll

- `maybe_auto_reflect` (`reflection.rs`): every gate passed → `reflected_upto
  = Some(messages.len())`, `reflected_at = Some(now)`, `mark_dirty(chat)`,
  then the spawn. `reflect_window(messages, reflected_upto)` counts the
  assistant replies past the watermark; `tool_loop::due(count, every)` is
  `count >= every`. `reflected_at` feeds `behavior_markers` (the
  regenerations and deletions *since* the previous reflection).
- `maybe_auto_consolidate` / `maybe_auto_self_consolidate`: the per-chat
  counter (`consolidate_counts`, `self_consolidate_counts`; in memory, not
  persisted) is incremented on every landing, and `insert(chat, 0)` at
  spawn; every other gate leaves it accumulating.
- The compaction roll: the plan is cut at spawn; an automatic roll stopped
  is **already planned again** at the next landing if the conversation is
  still over the threshold (the stop track's F5), and a manual `/compact`
  is the user's to retype. Nothing here needs a refund.

### 2.2 The slot and the landing

`BgSlot { cancel, failures }` per kind; `begin_bg(kind, cancel)` marks the
slot at spawn (the three loops and the roll call it); `handle_bg_done(kind,
BgOutcome::{Done, Cancelled, Failed})` clears it. The slot knows nothing of
the chat the task was spawned for.

### 2.3 What the loop knows

`run_rounds` (`tool_loop.rs`) keeps `round`, incremented after a round's
tools ran; a stop returns `RoundsEnd::Cancelled` from two places — the
stream ended `Cancelled`, or its finish reason says so — and both come
**before** that round's tools. So at the moment a stop is read, `round` is
exactly the number of rounds whose tools ran: `0` when the task was stopped
while waiting for the lane, while waiting for room, or during its first
stream; `1` or more once a round of tools has run. `spawn_silent_loop` maps
`RoundsEnd::Cancelled` onto `BgOutcome::Cancelled` and drops the number.

### 2.4 The tests

`tests/reflection.rs` spawns through `maybe_auto_reflect` against a
scripted backend and asserts the watermark; `tests/self_consolidation.rs`
reads the counter after a spawn (`Some(&0)`) and after a gate skip;
`tests/silent.rs` stops a loop mid-stream, while waiting, and during a
retry, and reads `Cancelled` off `done_rx`; the stop test in
`tests/reflection.rs` drives `begin_bg` → `handle_stop_background_task` →
`handle_bg_done(Cancelled)` by hand.

## 3. Design

### 3.1 The window on the slot

`BgSlot` gains `window: Option<Window>` — what the spawn advanced, and how
to put it back:

```rust
enum Window {
    /// Reflection: the chat's watermark and stamp before the spawn.
    Reflection { chat: Uuid, upto: Option<usize>, at: Option<DateTime<Utc>> },
    /// A consolidation: the chat's reply count the spawn reset.
    Counter { chat: Uuid, count: u32 },
}
```

`begin_bg(kind, cancel, window: Option<Window>)`: the three loops pass
theirs, the roll passes `None`. The slot is the task's lifecycle record
already (running, streak); the window is the third fact about the run, and
it lives and dies with the slot rather than travelling through the loop
(fork F3).

### 3.2 The fact from the loop

`RoundsEnd::Cancelled { rounds: u32 }` — the number of rounds whose tools
ran before the stop — and `BgOutcome::Cancelled { consumed: bool }` with
`consumed = rounds > 0`; the roll's `CompactEnd::Cancelled` maps to
`consumed: false` (a cancelled roll wrote nothing and has no window). The
stop track's reading of a stop — the streak, the slot, the indicator, the
list, `SelfModelChanged` — is unchanged by the flag.

### 3.3 The refund

`handle_bg_done` takes the slot's `window` on every outcome; on
`Cancelled { consumed: false }` it puts it back:

- `Reflection { chat, upto, at }` → the chat's `reflected_upto` and
  `reflected_at` restored and `mark_dirty(chat)` (R4); a chat that is gone
  meanwhile is left alone.
- `Counter { chat, count }` → the kind's map gets `count` **added back**
  (fork F2): the landings during the run incremented it legitimately, and
  the sum is what the counter would read had the spawn never happened.

Then the ordinary cadence does the rest: at the next landing
`reflect_window` counts the old window plus the new replies, `due` is true,
the gates run, and the task spawns — over the same window it was stopped
on, extended by what came after. Nothing is re-scheduled by hand, no
"pending" flag exists, and a task stopped twice is refunded twice, since
each spawn records its own window.

On `Done`, `Failed` and `Cancelled { consumed: true }` the window is
dropped: the advance stands, as today.

### 3.4 What the user sees

Nothing new at the stop. The task's row goes *idle* and its chip clears;
after the next reply it runs again — the way any due task does — and the
tasks screen shows it running. No note says "it will come back": the feed
is not a log (the stop track's F3), and a task that returns at its cadence
is not news.

### 3.5 What does not change

The roll (§2.1); the outcome's other effects (R3); the preemption retry
(a displaced round is not a stop — the loop makes the request again and the
advance stays honest); a task that fails; `Quit` (fork F6); the tasks screen
and the two commands.

## 4. Difficult spots

- **"Acted on" is a round of tools, not a byte written.** A reflection's
  first round may call `get_self_model` and nothing else; the window is
  then kept although nothing was written. The honest criterion would read
  every tool result's effects; the cheap one — a round of tools ran — errs
  on the side the spawn-time rule already chose (never read twice), and
  the stop that matters most, the one seconds after the chip appears, is
  always before the first round ends.
- **A stop during the tools.** The tools of a round run to their end (the
  token is read at the next stream), so `round` is already incremented
  when the stop is read: `consumed`, correctly — a write may have happened.
- **The counter is added back, not restored**, because the map is live
  during the run: three landings while a consolidation streams leave it at
  three, and `prev + 3` is what it would read. Restoring `prev` would lose
  them.
- **The watermark is persisted; the counters are not.** Today a restart
  loses the counters anyway (they start at zero); the refund does not
  change that, and a restart mid-reflection keeps the spawn-time watermark
  as it does today (fork F6).
- **Two chats.** The window names its chat: a reflection is per chat, and
  a stop from another chat's tasks screen refunds the right one.

## 5. Forks

- **F1. The criterion.** (a) **The window comes back only when no round of
  the task's tools ran** — `Cancelled { consumed }` from the loop
  *(recommended — R2: a read window written twice is the defect the
  spawn-time rule exists to prevent)*. (b) Always — self-consolidation
  merges duplicates: it is off by default, and reflection's duplicates are
  visible on `F3` in the meantime. (c) Only when stopped while *waiting* —
  too narrow: the first stream is where most stops land, and nothing is
  written until it ends.
- **F2. The counters.** (a) **Added back** (`count += prev`) *(recommended
  — the landings during the run are real)*. (b) Restored (`count = prev`).
- **F3. Where the window lives.** (a) **On the slot**, set at spawn through
  `begin_bg`, taken at the landing *(recommended — the slot is the run's
  record; the loop stays ignorant of chats' watermarks)*. (b) Carried
  through the loop and returned with the outcome.
- **F4. The roll.** (a) **Unchanged** *(recommended — an automatic roll is
  already planned again; a manual one was typed and stopped by the same
  hand)*. (b) A stopped manual `/compact` planned again automatically.
- **F5. Feedback.** (a) **None new** *(recommended — §3.4)*. (b) A note at
  the stop saying the task returns after the next reply.
- **F6. `Quit`.** (a) **A quit is not a stop**: `cancel_all_bg` ends the
  tasks and nothing lands, so the advance stays, as today *(recommended —
  the loop's fact is not available at a quit, and the conservative side
  is the spawn-time rule's)*. (b) Refund every active window before the
  exit flush, blind to whether a round ran.
- **F7. Staging.** (a) **One PR** *(recommended)*. (b) Two.

## 6. Tests and the live run

- `tests/reflection.rs`: a reflection spawned through `maybe_auto_reflect`
  (watermark `Some(2)`, `reflected_at` set), stopped, landed `Cancelled {
  consumed: false }` → `reflected_upto` back to `None`, `reflected_at` back
  to `None`, the chat marked dirty; the next `maybe_auto_reflect` spawns
  again and advances; landed `Cancelled { consumed: true }` → the watermark
  stays; landed `Done` after a spawn → stays.
- `tests/self_consolidation.rs` / `tests/consolidation.rs`: a spawn resets
  the counter to `0`; a landing during the run makes it `1`; the stop
  refunds to `every + 1`; `consumed: true` leaves `1`.
- `tests/silent.rs`: a loop stopped mid-stream, while waiting, and during a
  retry lands `Cancelled { consumed: false }`; a loop whose first round
  called a tool and whose second stream is stopped lands `Cancelled {
  consumed: true }`; the roll's stop lands `consumed: false`.
- `tests/reflection.rs`' existing stop test: unchanged effects (streak,
  events) under the new shape.
- **Live: not required** — the engine path is the stop track's, measured
  (its §6.1); what changes is the orchestrator's bookkeeping after the
  landing, under unit test. The LAN regression trio is run once as the
  habit.

## 7. Not in this track

- **Refund on `Quit`** (F6b) — if a restart mid-reflection turns out to
  matter.
- **Effects-based "acted on"** — reading the tool results' effects rather
  than counting rounds; the cheap criterion first.
- **A note at the stop** (F5b).

## 8. Documentation touch list (AGENTS.md §4)

- spec §17.6 (a stopped reflection: the window given back or kept), §11.10
  (the stop's effect on the window), §6.7 unchanged (stated).
- architecture §5 (`RoundsEnd::Cancelled { rounds }`), §9/§11
  (`BgSlot.window`, `BgOutcome::Cancelled { consumed }`, the refund in
  `handle_bg_done`).
- [stop-silent-task.md](stop-silent-task.md) §2.4/§4/§7 (F4b — done here),
  [tasks-stop-command.md](tasks-stop-command.md) §7.
- CHANGELOG (Changed), journal `engine.md` (the silent-task family's
  file), CLAUDE.md's status line and count.
