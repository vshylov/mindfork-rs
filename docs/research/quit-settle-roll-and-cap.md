# The quit's settle hears the roll, and its cap is a setting

> **Status:** proposed (2026-09-08) — the forks in §5 await the user's
> decision; no stage-0 probe (nothing about a model's behaviour is in
> question). The two items the settle track recorded
> ([quit-waits-for-the-landing.md](quit-waits-for-the-landing.md) §7):
> draining `compact_rx` at the quit, and a configurable cap. The first
> turned out not to be cosmetic — the roll lands on a channel the settle
> does not listen to, so a quit during a roll waits the whole cap for a
> landing that never arrives on the channel it watches.

## 1. Why, precisely

**The roll.** `settle_silent_tasks` listens on `bg_done_rx` while any
silent slot is active. The compaction roll takes a slot like the three
loops (`begin_bg(Compaction, …)`), but it does not land on `bg_done_rx`:
`spawn_compact` sends a `CompactResult` on `compact_rx`, and it is
`handle_compact_result` — a `select!` arm of `run` — that applies a
finished summary to the chat and then calls `handle_bg_done(Compaction,
…)` to clear the slot. At a quit the arm is no longer running: the settle
sees the compaction slot active, waits on `bg_done_rx`, and nothing
clears the slot until the cap runs out. Two consequences, one visible:

- a quit while an automatic roll streams — the commonest silent task on a
  long conversation, and the one that fires exactly when the conversation
  is largest — takes the full two seconds instead of the milliseconds the
  cancelled stream needs to end;
- a roll that **finished** just before the quit, its `CompactResult`
  sitting in `compact_rx` unread, is dropped: the summary the engine spent
  seconds producing is never applied, and the next launch plans the roll
  again.

Neither was true before the settle track (a quit simply left), so the
first is a regression of that track, recorded there as a "not in this
track" and wrong to leave.

**The cap.** `QUIT_SETTLE` is a constant of two seconds, chosen for a GPU
host's embedding with a CPU host's in mind. A user who runs the embedder
on a slow CPU, or one who wants the app gone the instant they say so,
has no say. The item was recorded as a possible setting; the user asked
for it.

**Requirements.**

- **R1. A cancelled roll lands at the quit** like the three loops: the
  settle is over as soon as every slot is clear, the roll's included.
- **R2. A finished roll is not lost**: a summary already in the channel
  at the quit is applied before the flush, as it would have been a tick
  later.
- **R3. The cap is the user's**: a number in the settings, in the
  interface section where the quit's other behaviour lives, read at the
  quit; zero means no wait.
- **R4. An old `settings.json` reads the default**: the field is
  `#[serde(default)]`, two seconds.

## 2. What exists (inventory)

- **`run`** (`orchestrator/mod.rs`): `compact_rx` (`CompactResult`) and
  `bg_done_rx` (`(BackgroundKind, BgOutcome)`) are locals, each a `select!`
  arm; after the loop, `settle_silent_tasks(&mut bg_done_rx, QUIT_SETTLE)`,
  `refund_unlanded`, `flush_saves`.
- **`handle_compact_result`** (`compaction.rs`): destructures the result,
  applies `Ok(summary)` through `apply_compaction` (the chat edited and
  marked dirty), answers a manual roll's `Cancelled` with a notice, maps
  the rest onto `BgOutcome`, and calls `handle_bg_done(Compaction, …)`.
- **`InterfaceSettings`** (`shared/config.rs`): the theme, spellcheck,
  the two confirmations, terminal compatibility, the renderers — bools and
  enums; the settings screen's *Interface* group renders them
  (`screens/settings/spec.rs`, the `I*` field ids). A plain numeric field
  follows `XSessions`: `int(|c, t| if let Ok(v) = t.parse::<u32>() { … })`
  — an unparsable edit leaves the value. The demo dumps do not show the
  *Interface* group, so a row there changes no dump.
- **Tests**: the settle track's `quit` helper (cancel → settle → refund)
  over a slow test tool; `tests/compaction.rs` builds rolls and reads
  their results; the settings tests edit a field and read the config back.

## 3. Design

### 3.1 The settle hears both channels

`settle_silent_tasks(&mut bg_done_rx, &mut compact_rx, cap)`: while any
slot is active and the deadline holds,

```
select! {
    landed = bg_done_rx.recv()  => handle_bg_done(kind, outcome),
    result = compact_rx.recv()  => handle_compact_result(result),
}
```

each under `timeout_at(deadline, …)`. A cancelled roll's stream ends on
its token, `spawn_compact` sends `Err(CompactEnd::Cancelled)`,
`handle_compact_result` clears the slot (R1) — the manual roll's notice
goes to a UI that has stopped reading, a `let _ =` send. A finished roll's
`Ok(summary)` in the channel is applied and marked dirty (R2), and the
flush after the settle writes it. `refund_unlanded` is unchanged: the roll
has no window.

### 3.2 The cap as a setting

`InterfaceSettings.quit_settle_ms: u32`, `#[serde(default =
"default_quit_settle_ms")]` = 2000 (R4). A row in the *Interface* group,
*Quit: wait for background work (ms)*, the `XSessions` shape — a typed
number stored as is, an unparsable edit ignored, `0` accepted — with a
hint that says what it trades: a task caught mid-work gets that long to
finish so its window is decided exactly; zero decides at once by state
(a round of tools still running keeps its window). `run` reads
`Duration::from_millis(orch.config.interface.quit_settle_ms.into())` at
the quit; `QUIT_SETTLE` becomes the default's home (fork F3: no ceiling —
a large value costs its author their own quit, and the hint names it).

### 3.3 What the user sees

A quit during an automatic roll returns at once instead of after two
seconds; a roll that had just finished is in the chat on the next launch;
a new row in *Settings → Interface* with the wait, and `0` for none.

## 4. Difficult spots

- **Two channels, one deadline.** `select!` under one `timeout_at`: the
  first of the two to deliver is handled, the loop re-checks the slots;
  the deadline is absolute, so the cap is the whole wait as before.
- **A roll that finished and a stop that refunded, in one settle.** Each
  is its own slot and its own channel; the order between them does not
  matter — the flush writes both.
- **`handle_compact_result` at exit.** Its notice and its events go to a
  UI that has left; `apply_compaction` edits the chat and marks it dirty,
  which is the point. Nothing in it spawns.
- **The field's home.** The interface section is where the quit's other
  behaviour would sit if it had any; the engine sections are per engine
  and the wait is not.
- **Zero.** `settle_silent_tasks` with a zero cap returns at once (the
  deadline is now); `refund_unlanded` then decides everything by state —
  the pre-settle behaviour, by choice.

## 5. Forks

- **F1. The roll at the quit.** (a) **The settle drains `compact_rx` too,
  through `handle_compact_result`** *(recommended — R1 and R2 in one
  arm)*. (b) Mark the compaction slot landed at the cancel without
  reading the channel — R1 only; a finished summary is lost.
- **F2. Where the cap lives.** (a) **`interface.quit_settle_ms`, a row in
  the settings screen's *Interface* group** *(recommended — every setting
  is on the screen)*. (b) The config file only.
- **F3. The value.** (a) **Milliseconds, `u32`, default 2000, `0` = decide
  at once, no ceiling** *(recommended — the hint names the trade)*. (b) A
  ceiling of 30 s. (c) Seconds with one decimal.
- **F4. Staging.** (a) **One PR** *(recommended)*. (b) Two.

## 6. Tests and the live run

- `tests/silent.rs` (the settle track's helpers): a roll running at the
  quit — the settle is over within milliseconds and the compaction slot is
  clear; a finished roll's `CompactResult` sitting in `compact_rx` at the
  quit — the chat carries the summary after the settle and is dirty; a
  zero cap — an `Idle` slot refunded at once, a mid-tools one kept, the
  settle over at once.
- `shared/config.rs`: an old `settings.json` without the field reads 2000.
- `screens/settings/tests.rs`: the field stores a typed number, `0`
  included; an unparsable edit leaves the value; the hint renders in both
  locales.
- `run` reads the setting: the quit-track tests through `run` pass a
  config with the field, the exit still under the cap.
- **Live: not required** — the exit path and a setting; the LAN regression
  trio run once as the habit.

## 7. Not in this track

- A per-kind cap.
- Draining `title_rx` / `imp_done_rx` at the quit — neither holds a slot
  nor a window.

## 8. Documentation touch list (AGENTS.md §4)

- spec §11.6 (the *Interface* field), §17.6 (the roll at the quit, the
  cap a setting), §6.7 (a finished roll at the quit).
- architecture §9/§11 (`settle_silent_tasks` over both channels), §7
  (the config field).
- [quit-waits-for-the-landing.md](quit-waits-for-the-landing.md) §7.
- locales `en`/`ru`: `ui.settings.field.quit_settle`,
  `ui.settings.desc.quit_settle`.
- CHANGELOG (Added: the setting; Changed: a quit during a roll), journal
  `engine.md`, CLAUDE.md's status line and count.
