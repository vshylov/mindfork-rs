# `/tasks stop all` — every running silent task, in one word

> **Status:** implemented (2026-09-08) — every fork at its recommendation
> (the user's decision, 2026-09-08); the regression run in §6.1; no stage-0
> probe (nothing about a model's behaviour is in question). The item [tasks-stop-command.md](tasks-stop-command.md) §7
> recorded: *four words typed four times is the rarest case; a fifth word is
> cheap to add when someone asks for it.* Someone asked. The design is a page
> because the mechanism is one of two, and the choice is the kind the
> registry's rule (*a command is its key*) decides rather than taste.

## 1. Why, precisely

`/tasks stop <kind>` ([tasks-stop-command.md](tasks-stop-command.md),
2026-09-08) stops one of the app's own tasks by word. With several running
— a reflection and a compaction roll both queued behind a turn, say — bare
`/tasks stop` refuses to guess and lists their words, so stopping both is
two commands typed from the list. The case is rare and the fix is a word;
what the track has to settle is how that word reaches the orchestrator.

**Requirements.**

- **R1. A command is its key** (spec §11.7). Whatever `all` does, each task
  it stops lands exactly as it does after `F6` on its row: the same
  `AppCommand`, the same *cancelled* outcome, the same manual-`/compact`
  notice, the same untouched streak.
- **R2. A command answers** (lessons §4): `all` with nothing running is the
  existing "none is running" note; with something running, one note that
  says what was stopped, by the tasks screen's names.
- **R3. The routes name each other.** The note that lists several running
  tasks and asks which now also names `all` — an answer that lists the
  choices and omits the one that takes them all is the dead end lessons §4
  is about.
- **R4. Nothing else moves.** `/tasks`, `/tasks stop`, `/tasks stop <kind>`
  keep their meanings; the orchestrator, the tasks screen and the four flags
  are untouched.

## 2. What exists (inventory)

- `typed_tasks_stop` (`screens/chat/commands.rs`): the tail matched against
  `TASK_KINDS` (four words, the tasks screen's order); the running set read
  off `ChatScreen::task_running` (the status bar's four flags, on while a
  slot is taken); bare `stop` collects the running kinds — none is
  `ui.cmd.tasks_none_running`, one is stopped, several are listed by word in
  `ui.cmd.tasks_stop_which`. The collection of the running kinds is the very
  code `all` needs.
- `ChatIntent::StopBackgroundTask { kind }` → `dispatch_chat` →
  `AppCommand::StopBackgroundTask { kind }`, one to one.
  `dispatch_chat` builds one command per intent with early returns for the
  intents that send nothing (`OpenHelp`, `CopyToClipboard`) — an intent that
  sends several is a third shape it does not yet have.
- The orchestrator: `handle_stop_background_task(kind)` cancels one slot's
  token; `cancel_all_bg()` cancels every slot's — `Quit`'s branch, the
  mechanism of "stop all" already written, for a different caller.
- Notes: `ui.cmd.tasks_stopping` (`{task}`), `ui.cmd.tasks_stop_which`
  (`{kinds}`), `ui.cmd.tasks_bad_kind` (`{arg}`, `{kinds}`); the help label
  `/tasks [stop <kind>]`.

## 3. Design

### 3.1 The word

`/tasks stop all` — `all` as `/tts all` spells it (fork F2), checked before
the kind table, case-insensitively; it is a modifier, not a kind, so it does
not enter `TASK_KINDS` (which the "which" and "bad kind" notes quote as the
set of *kinds*). The help label becomes `/tasks [stop <kind>|all]`, the
description names it, and the two notes that list the kinds end with
"— or `all`" (R3).

### 3.2 The mechanism (fork F1)

The chat screen collects the running kinds, as bare `stop` already does:
none → `ui.cmd.tasks_none_running`; otherwise one note listing the tasks by
the screen's names (`ui.cmd.tasks_stopping_all`, `{tasks}` joined with
", ") and **one intent carrying the kinds** —
`ChatIntent::StopBackgroundTasks { kinds: Vec<BackgroundKind> }`, which
replaces the single-kind variant (the named-kind and bare-`stop` routes send
a one-element list). `dispatch_chat` sends **one
`AppCommand::StopBackgroundTask { kind }` per kind**, in the table's order,
and returns — the third shape, beside "one command" and "none", and the one
that keeps R1 by construction: every stop reaches the orchestrator through
the very command `F6` sends, and nothing in `orchestrator/` changes. The
alternative — `AppCommand::StopAllBackgroundTasks` onto `cancel_all_bg()` —
is one line in the orchestrator and a second verb for the same act, and
the note would still need the flags to say what it stopped.

### 3.3 What the user sees

*Stopping the tasks: reflection, history compaction.* — then, as after
`F6` on each row, the status bar's chips clear as the outcomes land and a
manual `/compact` adds its own *Compression stopped* line. `all` with one
task running takes the same path and the same note with one name (fork
F4): one code path, and the word said *all*.

## 4. Difficult spots

- **A `Vec` in an intent.** `ChatIntent` carries ids, strings and structs
  today, no lists; a list of kinds is small and bounded (four) and the
  alternative — an intent per kind returned from one keypress — is not a
  shape `handle_key` has. The runtime's fan-out is three lines.
- **The order of the stops.** The table's order (the screen's rows), so the
  note and the commands agree; the orchestrator's handling is per slot and
  order-independent.
- **`all` in the kind notes.** `ui.cmd.tasks_bad_kind` and
  `ui.cmd.tasks_stop_which` quote `TASK_KINDS`; `all` is appended in the
  wording, not the table, so the table stays the set of kinds the tasks
  screen shows.

## 5. Forks

- **F1. The mechanism.** (a) **One intent carrying the running kinds; the
  runtime sends the existing `StopBackgroundTask { kind }` once per kind**
  *(recommended — R1 by construction, the orchestrator untouched)*. (b) A
  new `AppCommand::StopAllBackgroundTasks` onto `cancel_all_bg()` — `Quit`'s
  mechanism, a second verb for one act.
- **F2. The word.** (a) **`all`** *(recommended — `/tts all`'s word)*. (b)
  `*`. (c) `everything`.
- **F3. The note.** (a) **One note listing the tasks stopped, by the tasks
  screen's names** *(recommended)*. (b) One `tasks_stopping` note per task.
- **F4. `all` with one task running.** (a) **The list note with one name**
  *(recommended — one path)*. (b) The single-task note, as if the kind had
  been named.
- **F5. The "which" and "bad kind" notes.** (a) **Reworded to name `all`**
  *(recommended — R3)*. (b) Unchanged.
- **F6. Staging.** (a) **One PR** *(recommended)*. (b) Two.

## 6. Tests and the live run

- `screens/chat/tests.rs` (`mod tasks_stop`): `all` with two running → the
  intent carrying both kinds in the table's order and a note naming both by
  the screen's words; `all` with none → the "none running" note and no
  intent; `all` with one → the one-element intent; `ALL` as `all`; the
  existing named-kind and bare-`stop` tests now expect one-element lists;
  the "which" and "bad kind" notes name `all`; the locale gate covers the
  new key.
- `runtime/tests.rs`: an intent with two kinds becomes two commands, in
  order; with one, one.
- `ui_command.rs`: unchanged (the tail is the runner's).
- **Live: not required** — the same command the stop track's smoke
  measured, sent more than once; the LAN regression trio run once as the
  habit.

### 6.1 The run (2026-09-08)

The unit suite: **2925 green, 146 `#[ignore]`** (+5: four in
`screens/chat/tests.rs::tasks_stop`, one in the runtime); the demo dumps
unchanged. The regression on the LAN stack (Qwen 3.6 27B, four slots over
16384): `stop_silent_task_e2e_live`, `silent_roll_e2e_live`,
`background_subagent_e2e_live` — **3/3 in 54.0 s**.

## 7. Not in this track

- A key for "stop all" on the tasks screen — `F6` is per row, and the
  screen's footer names what works on the selected row.
- `/tasks stop` bare stopping everything — the design of the last track
  chose the `/subagents stop` mirror; `all` is the word for the whole set.

## 8. Documentation touch list (AGENTS.md §4)

- spec §11.7 (the list: `/tasks [stop <kind>|all]`), §11.10 (the sentence
  on the typed route).
- architecture §10 (the fan-out in `dispatch_chat`).
- [tasks-stop-command.md](tasks-stop-command.md) §7 (done here).
- locales `en`/`ru`: `ui.help.k.tasks`, `ui.help.cmd_tasks`,
  `ui.cmd.tasks_stopping_all`, `ui.cmd.tasks_stop_which`,
  `ui.cmd.tasks_bad_kind`.
- CHANGELOG (Added), journal `ui-screens.md`, CLAUDE.md's status line and
  count.
