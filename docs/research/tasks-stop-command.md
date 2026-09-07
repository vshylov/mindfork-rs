# `/tasks stop <kind>` — the typed route to stopping a silent task

> **Status:** implemented (2026-09-08) — every fork at its recommendation
> (the user's decision, 2026-09-08); the regression run in §6.1; no stage-0
> probe, since nothing about a model's behaviour was in question.
> The item the stop track recorded and did not take
> ([stop-silent-task.md](stop-silent-task.md) §7, its fork F2b): `F6` on a
> task row of the tasks screen stops one of the app's own tasks, and that
> key has no typed twin — the one action in the interface whose only route
> is a function key on a screen. The command is one registry row, one
> runner and five notes; what has to be decided is its shape.

## 1. Why, precisely

Spec §11.7 states the rule the interface is built on since the
command-only-control track: **every action has a typed route as well as a
chord**, because a host that embeds the terminal claims chords before
crossterm sees them, and typing is the one input no host can take away. The
rule was kept on every surface that came after — `/subagents stop [n]` is the
typed twin of `F6` on a background run's transcript, written into the same
track as the key — until the stop of a silent task
([stop-silent-task.md](stop-silent-task.md), 2026-09-07). That track put the
stop on `F6` of the tasks screen and chose *no command in v1* (its F2a), on
the reading that the screen is the surface and a command would need
"localized kind names and a parser for a thing four rows away". Both halves
of that reading turned out lighter than they sounded, which is why the item
is back:

- **The names exist.** The tasks screen already names the four tasks in both
  locales (`ui.tasks.app.reflection` … `ui.tasks.app.compaction`), and the
  command's *words* are not localized at all — command words are protocol,
  like CLI flags (the registry's own rule, `features/ui_command.rs`).
- **The parser exists.** `Arity::Subcommand` already carries a short tail
  behind its word (`/subagents stop 2`), and the chat screen already keeps
  the one fact the command needs — whether each of the four tasks is running
  — for the status bar's chips.
- **The route matters more than it seemed.** From a host that eats `F6`, a
  silent task cannot be stopped at all: the screen opens by `/tasks`, the row
  selects by the arrows, and then the only key that acts is the one the host
  took. And from the chat, the hand that sees *reflection…* on the status
  bar and wants it gone has three steps where `/subagents stop` has one.

**Requirements.**

- **R1. A command is its key** (spec §11.7). The command ends in
  `AppCommand::StopBackgroundTask { kind }` — the very command `F6` on the
  task row sends — so the orchestrator gains no second path and the outcome
  (the third kind, *cancelled*, with everything the stop track decided about
  the streak and the window) is the same by construction.
- **R2. A command answers where a key may stay silent** (spec §11.7,
  lessons §4). Every precondition leaves a note that names a route: the
  named task idle, no task named while several run, a word the command does
  not know. `let x = thing?` never swallows one.
- **R3. Words are protocol, names are text.** The kind words are fixed
  ASCII, case-insensitive, the same in every locale; the note that answers
  names the task in the interface language — with the tasks screen's own
  words, so the two surfaces never call one task two things.
- **R4. `/tasks` bare is untouched** — the screen, as before — and the
  registry stays one: no parser module for a command with one word and one
  token behind it. The help tab's row comes from the registry, as every row
  does.

## 2. What exists (inventory)

### 2.1 The registry and the parser

`features/ui_command.rs` holds every typed route as one table row:
aliases, `UiCommand`, an `Arity`, a label (a literal, or a `ui.help.k.*`
key when the label names an argument — it doubles as the usage line in the
parser's own errors), a description key. `/tasks` is
`row(&["/tasks"], UiCommand::Tasks, Arity::None, "/tasks", "ui.help.cmd_tasks")`.
`Arity::Subcommand(words)` is the closed-set arity: `parse` normalizes the
head of the argument against the set, reports a head outside it with
`ui.cmd.bad_subcommand` (naming the usage label), and lets a **tail** travel
behind the word — `/subagents STOP 2` parses to `argument = "stop 2"`; bare
parses to an empty argument, which is how `/self` and `/subagents` keep
their bare meanings. The `Arity` doc comment says a command whose
subcommands take arguments of their own belongs in a module (`/profile`);
`/subagents stop [n]` drew the line one notch further — a *token* rides
with the word, a *free-text argument* earns a module — without saying so.

### 2.2 The runner

`screens/chat/commands.rs::run` matches on `UiCommand` exhaustively;
`UiCommand::Tasks => Some(ChatIntent::OpenTasks)`. The `/subagents` arm is
the pattern for a word with a tail: `typed_subagents` strips `stop`, and
`typed_subagents_stop(active, which)` resolves the target from the chat
list's cards — the open transcript's run, the n-th one out, the only one
out — and answers with `ui.cmd.subagents_stopped` (naming the run),
`ui.cmd.subagents_no_running`, or `ui.cmd.subagents_stop_which` (`/subagents
stop <n> (1–{n})`). It returns `ChatIntent::StopSubagentRun { id }`, which
`runtime/dispatch.rs::dispatch_chat` maps one to one onto
`AppCommand::StopSubagentRun { id }`.

### 2.3 What the chat screen knows

Four flags on `ChatScreen` — `reflecting`, `consolidating`,
`self_consolidating`, `compacting` — set by `apply_background_task` in the
runtime from `AppEvent::BackgroundTask { kind, active }`, which the
orchestrator sends with `active: true` as a slot is taken
(`background.rs::spawn_bg`) and `active: false` as its outcome lands,
whatever the outcome. The status bar's chips read them. A flag is therefore
on exactly while the slot is taken — streaming *or* waiting on the lane —
which is the same predicate the tasks screen's `Stoppable::Task` uses
(`AppTask.running`, built off the slots in `orchestrator/tasks.rs`; *waiting*
is a refinement of it, never a third state). The event reaches the chat
screen whatever screen is in front, so the flags are current when a
command is typed.

### 2.4 The orchestrator

`AppCommand::StopBackgroundTask { kind }` (on `works_on_the_open_chat`'s
false side — it changes nothing in the conversation) →
`handle_stop_background_task`: cancels the slot's token if the slot is
taken, ignores a kind with nothing running. The task lands
`BgOutcome::Cancelled`; a manual `/compact` answers with
`ui.compact.cancelled`, an automatic roll quietly. Nothing here changes.

### 2.5 The help tab and its gates

The Commands tab renders its rows off the registry (label + description in
the interface language), so a row cannot be undiscoverable; `every_ui_command_variant_has_exactly_one_row`
and `every_alias_parses_in_any_case_and_padding` are driven off the table,
and `errors_are_localized_for_all_langs` renders the parser's errors under
every bundled language. The command test harness (`Cmd` in
`screens/chat/tests.rs`: `run(line)`, `last_note()`) is what the
`/subagents stop` tests use.

## 3. Design

### 3.1 The syntax

```
/tasks                         the screen (unchanged)
/tasks stop <kind>             stop that task
/tasks stop                    stop the only one running; otherwise say which
```

The registry row becomes
`row(&["/tasks"], UiCommand::Tasks, Arity::Subcommand(&["stop"]), "ui.help.k.tasks", "ui.help.cmd_tasks")`
with `ui.help.k.tasks` = `/tasks [stop <kind>]`. The parser owes nothing
new: bare stays the screen, `stop` with a tail arrives as `"stop <tail>"`,
`/tasks halt` is reported by the parser naming the usage line. The `Arity`
doc comment states the line `/subagents stop [n]` drew: **a token rides
with the word; free text earns a module.**

The kind words (fork F2): **`reflection`, `notes`, `self`, `compact`** —
each the word of the command or screen the task belongs with (`/self` is
the self-model, `/compact` the roll, the notes are the notes), one spelling
each, matched case-insensitively. They are a table in the runner,
`TASK_KINDS: [(&str, BackgroundKind); 4]`, in the order the tasks screen
lists the rows, and the same table is what the notes quote.

### 3.2 The runner

`typed_tasks(argument)`:

- empty → `ChatIntent::OpenTasks`, as today;
- `stop <word>` → the word resolved against `TASK_KINDS`; unknown →
  `ui.cmd.tasks_bad_kind` (naming the four words) and no intent; known and
  its flag **off** → `ui.cmd.tasks_not_running` (naming the task in the
  interface language, and `/tasks` as the place that shows what is running)
  and no intent; known and **on** → `ui.cmd.tasks_stopping` (naming the
  task) and `ChatIntent::StopBackgroundTask { kind }`;
- `stop` bare (fork F3) → the running tasks are collected off the four
  flags: none → `ui.cmd.tasks_none_running`; one → stopped, as
  `/subagents stop` stops the only run out; several →
  `ui.cmd.tasks_stop_which` listing their **words**, so the answer is the
  next command.

No `ui.cmd.no_chat` gate: the tasks are the app's, not a chat's — `/tasks`
bare needs no chat either, and a compaction roll is stoppable from
whichever chat is open. The task's name in a note comes from the tasks
screen's own keys, `ui.tasks.app.*`, through one function on the enum
(`BackgroundKind::label_key`, in `app/events.rs` beside the enum both
screens already import) — the screen's private `app_label_key` moves there,
so the two surfaces cannot drift.

`ChatIntent::StopBackgroundTask { kind: BackgroundKind }` is the twin of
`TasksIntent::StopTask(BackgroundKind)`; `dispatch_chat` maps it onto
`AppCommand::StopBackgroundTask { kind }` in one line, exactly as
`StopSubagentRun` is mapped. The chat screen already imports `app::events`
types (`ChildView`), as the tasks screen imports `BackgroundKind`: the
events module is the UI contract, and the invariant architecture §3 states
is about `AppCommand` — a screen returns its intent and never the command.

### 3.3 What the user sees

One note in the feed: *Stopping reflection.* — the `subagents_stopped`
shape (fork F6). The status bar's chip clears when the outcome lands, as it
does after `F6`; a manual `/compact` stopped this way also gets the
orchestrator's *Compression stopped; nothing was folded.* — two lines, both
true, the second being the outcome the screen shows as *idle*. A stop typed
a moment after the task landed on its own reaches an idle slot and is
ignored, as `/subagents stop` on a run that just landed is: the note said
*stopping*, and nothing was running to stop — the window between the flag
and the slot is one event's flight.

### 3.4 What does not change

The orchestrator, the outcome, the bookkeeping, the tasks screen and its
`F6`, the four flags and their events, `/tasks` bare, `/stop` (the turn's,
and only the turn's — `Esc`'s split in §11.7 stands), the count of commands
(twenty-four rows).

## 4. Difficult spots

- **The registry's line.** `Arity`'s comment draws it at "subcommands with
  arguments belong in a module", and `/subagents stop [n]` already crosses
  it with a number. The honest statement is the one §3.1 makes — a token
  behind a closed-set word is the registry's, free text is a module's — and
  the comment is corrected rather than the precedent undone; a second
  command of the shape is what makes the line worth writing down.
- **The word `self`.** `/tasks stop self` reads oddly out of context; in
  the interface it is the word `/self` already made the self-model's, and
  the alternative (`selfmodel`, `self-consolidation`) is longer for no
  gain in clarity. Fork F2 has the alternatives.
- **A flag, not a snapshot.** The chat screen answers off the four flags,
  not off `TaskList`, which it never receives (the tasks screen asks for
  it). The flags are the bar's own source and carry no less: the stop
  needs *taken or not*, which is exactly what they say (§2.3).
- **`stop` bare with one running.** Mirroring `/subagents stop` means a
  bare `/tasks stop` acts without naming what it stops; the note names it
  after the fact, and a stop is per attempt — the window is skipped, the
  next cadence runs. Fork F3's (b) always asks instead.
- **The help row's length.** The description grows to name the four words;
  the help tab's width gate renders every row in both locales, so the
  wording is written under it rather than around it.

## 5. Forks

- **F1. The syntax.** (a) **`/tasks stop <kind>` on the existing registry
  row** *(recommended — R4; the `/subagents stop [n]` shape, bare
  untouched)*. (b) A module of its own (`features/tasks_command.rs`) — the
  `/profile` route, for a command with no free text in it. (c) `/stop
  <kind>` — no: `/stop` is the half of `Esc` that always means the turn.
- **F2. The kind words.** (a) **`reflection · notes · self · compact`**
  *(recommended — each the word of the command or screen the task belongs
  with; one spelling)*. (b) The lane labels `reflection · consolidation ·
  self_consolidation · compaction` — precise, and nobody types an
  underscore. (c) Both, as aliases — two spellings to document for a
  four-word set.
- **F3. Bare `/tasks stop`.** (a) **Mirror `/subagents stop`** — the only
  running task is stopped, several are listed by word, none is a note
  *(recommended — a sibling of the last, and the common case is one)*. (b)
  Always list, never act without a word.
- **F4. Where "is it running" is read.** (a) **The chat screen's four
  flags** *(recommended — the status bar's own source, the same predicate as
  the screen's `F6`, the answer immediate, no new event)*. (b) Sent
  unconditionally, the orchestrator answering with a notice — a round trip
  and a second wording for one fact.
- **F5. The task's name in a note.** (a) **The tasks screen's words**
  (`ui.tasks.app.*`, through `BackgroundKind::label_key`) *(recommended —
  R3, one name per task across surfaces)*. (b) The error family's
  (`ui.err.bg_*`: "Auto-reflection", "History compression") — a second
  set that exists for the failure alert.
- **F6. The feedback.** (a) **One note naming the task** *(recommended —
  R2; the `subagents_stopped` shape)*. (b) None — the chip's clearing is
  the feedback: a typed command that vanishes reads as a refusal.
- **F7. Staging.** (a) **One PR** *(recommended — one row, one runner, one
  intent, five notes)*. (b) Two.

## 6. Tests and the live run

- `features/ui_command.rs`: `/tasks` bare parses to `Tasks` with an empty
  argument; `/tasks stop reflection` (and `/tasks STOP Reflection`) to
  `"stop reflection"` / `"stop Reflection"` (the tail is the runner's to
  normalize); `/tasks halt` is reported naming `/tasks [stop <kind>]`;
  `errors_are_localized_for_all_langs` gains the `/tasks halt` row.
- `screens/chat/tests.rs` (`Cmd`): `/tasks` → `OpenTasks`; `/tasks stop
  reflection` with `set_reflecting(true)` → `StopBackgroundTask {
  Reflection }` and a note naming *reflection*; the same with the flag off
  → no intent and a note naming the task and `/tasks`; `/tasks stop compact`
  → `Compaction`; `/tasks stop` with one flag on → that kind; with two on →
  no intent and a note naming both words; with none → no intent and a note;
  `/tasks stop foo` → no intent and a note naming all four words; the case
  test; every note renders under both locales with no placeholder left.
- `runtime/tests.rs`: `ChatIntent::StopBackgroundTask { kind }` becomes
  `AppCommand::StopBackgroundTask { kind }`.
- The registry-driven gates (every variant one row, every alias parses, the
  help tab under its width in both locales) run unchanged over the new row.
- **Live: not required** (AGENTS.md §3 — a pure UI route). The command ends
  in the very `AppCommand` the stop track's `stop_silent_task_e2e_live`
  sends and measured (its §6.1); that smoke and the two beside it
  (`silent_roll_e2e_live`, `background_subagent_e2e_live`) are run once on
  the LAN stack as the regression, and the outcome recorded in the journal.

### 6.1 The run (2026-09-08)

The unit suite: **2920 green, 146 `#[ignore]`** (+12: one in the parser,
ten in `screens/chat/tests.rs::tasks_stop`, one in the runtime); the demo
dumps unchanged. The regression on the LAN stack (Qwen 3.6 27B, four slots
over 16384): `stop_silent_task_e2e_live`, `silent_roll_e2e_live`,
`background_subagent_e2e_live` — **3/3 in 58.7 s**, the stop's notice
0.00 s after the stop, the next `/compact` in 2.2 s.

## 7. Not in this track (recorded so they are not re-derived)

- **`/tasks stop all`** — four words typed four times is the rarest case;
  a fifth word is cheap to add when someone asks for it. **Asked for and
  done** the same day ([tasks-stop-all.md](tasks-stop-all.md)): one intent
  carrying the running kinds, the runtime sending this track's command once
  per kind.
- **Stopping the title** — no row on the screen, no flag on the bar, and
  a title is seconds (the stop track's §7).
- **A confirmation** — a stop is per attempt and undoes nothing written
  (the stop track's R3).
- **Refunding the window on a stop** (the stop track's F4b) — unchanged by
  the route.

## 8. Documentation touch list (AGENTS.md §4)

- spec §11.7 (the command list: `/tasks [stop <kind>]`; the registry's
  line), §11.10 ("there is no command for it" → the command), the keys
  table's `F7` row.
- architecture §10 (the registry row; the runner; `BackgroundKind::label_key`).
- [stop-silent-task.md](stop-silent-task.md) §7 (F2b — done here),
  [tasks-screen.md](tasks-screen.md) §8 if it names the route.
- locales `en`/`ru`: `ui.help.k.tasks`, `ui.help.cmd_tasks` (reworded),
  `ui.cmd.tasks_stopping`, `ui.cmd.tasks_not_running`,
  `ui.cmd.tasks_none_running`, `ui.cmd.tasks_stop_which`,
  `ui.cmd.tasks_bad_kind`.
- CHANGELOG (Added), roadmap, journal `ui-screens.md`, CLAUDE.md's status
  line and test count.
