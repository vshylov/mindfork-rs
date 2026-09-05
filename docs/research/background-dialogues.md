# Background dialogues — a directed scene that outlives its turn

**Status:** design, forks open (2026-09-05). The last of the three items
[background-subagents.md](background-subagents.md) §8 left for later, and the
one [two-agent-dialogue.md](two-agent-dialogue.md) named as *"the same shape
would serve"* when the background mechanism existed. It now does.

Related: [ADR 0011](../decisions/0011-dialogue-directed-run.md) (the dialogue
as a scripted multi-context run), [ADR 0010](../decisions/0010-subagent-nested-turn.md)
(amended twice — the nested turn, then the run outside it),
[spec §9.13](../../spec.md), [spec §9.3.2](../../spec.md).

---

## 1. Requirements

- **R1.** The assistant can stage a directed dialogue whose result it does
  not need for the current reply: the call returns at once with the
  transcript's address, the scene runs on, and the director's closing result
  arrives later as a task notification — the contract `start_subagent`
  already has.
- **R2.** Nothing about a foreground `run_dialogue` changes. A profile that
  never stages a background scene sends byte-identical requests.
- **R3.** The VRAM contract survives. ADR 0011 decision 3 bought it with
  sequentiality *inside one turn* ("at most one request in flight ever"). A
  scene that outlives its turn cannot keep that literal invariant — the
  parent's next turn is a second stream by construction — so the contract
  has to become the one the background sub-agent track already adopted: the
  **app-wide session budget**, where at `sessions = 1` the scene and the
  next turn take turns request by request, and under a shared KV pool the
  admission guard prices both.
- **R4.** A background scene is stoppable and lands honestly: `/subagents
  stop [n]`, `F6` on its open transcript, take-back of the spawning
  exchange, a deleted chat, `Quit` — each ending it and landing the partial
  transcript, exactly as a background sub-agent run does.
- **R5.** The live transcript keeps everything stage 2 of the dialogue track
  built: the line streaming on its speaker's side, the director's retake
  replacing the open view, the scene chip on the status bar.
- **R6.** No new confirmation surface. Participants have no tools (ADR 0011
  fork F4) and the director's verdicts are interpreted by the loop, so a
  background scene runs nothing that could ask — the fork the sub-agent
  track had to decide (its F3) does not arise here.
- **R7.** One cap over both kinds of background run, because what the cap
  protects is shared: permits, pool reservations and parked contexts.

## 2. What exists (inventory)

Anchors are `file.rs:line` at `e0525cd` plus the stage-2 merge.

### 2.1 The dialogue is driven from inside the turn's loop

`run_dialogue` is a **loop-executed** tool: the registry entry carries the
schema and refuses to execute (`features/tools/dialogue.rs:422`), and the
agentic loop recognises the name in `resolve_call_result`
(`generation.rs:2505`) and calls `TurnLoop::run_dialogue`
(`generation.rs:3285`). It is reached only from the round's **sequential**
branch — `is_group_call` names `call_subagent` alone
(`generation.rs:2565`), `is_background_call` names `start_subagent` alone
(`generation.rs:2573`) — so the parent's turn is parked inside
`execute_round` for the whole scene.

The driver takes a raw `&serde_json::Value`, parses `DialogueArgs`
(`dialogue.rs:85`) and owns `DialogueState` (`generation.rs:3205`) outside
the timed future, so a timeout still lands the partial transcript
(`generation.rs:3397`). The scene itself is `dialogue_loop`
(`generation.rs:3513`) over `dialogue_line` `:3579`, `dialogue_checkpoint`
`:3694` and the five verdict handlers `:3812`–`:3951`. The result is a
`CallResult` carrying a `SubagentRun` with `kind: Dialogue`
(`generation.rs:3441`, `:3502`), written into the call's record by
`record_call` (`generation.rs:2440`) and landed with the turn.

### 2.2 What it borrows from the turn

Already re-created for a background run by `SharedParts::of`
(`generation.rs:2826`) and `spawn_background_run` (`generation.rs:2930`):
the backend, the sub-agent limits, the locale, the engine mode and model
name, the compaction summary, the event sender, fresh counters, and a
generation id of the run's own.

**Not** re-created — the four turn-coupled inputs:

- **The cancellation token.** `let cancel = self.cancel.child_token()`
  (`generation.rs:3365`) binds the scene to the turn, which is why `Esc`
  ends it today. `start_background` instead mints a fresh token
  (`generation.rs:2590`).
- **The parent's persona.** The director's system starts with
  `ctx.system_message` (`generation.rs:3341`) — the chat's own persona.
  `ChildSpec` clones `ctx` but **replaces** that field with the sub-agent's
  persona (`generation.rs:2687`), so the value a background dialogue needs
  is preserved nowhere in the background path.
- **The conversation brief.** `conversation_brief` (`generation.rs:3235`)
  reads `self.request.messages` — the turn's live request tail — plus the
  rolling summary. Nothing in `SharedParts` or `ChildSpec` carries the
  parent conversation.
- **The depth guard** (`generation.rs:3301`) and the `TurnLoop` object
  itself: `run_dialogue` is a `&mut TurnLoop` method.

### 2.3 A defect the inventory turned up

`dialogue_stream` (`generation.rs:3967`) calls `stream_round` **directly**.
`TurnLoop::stream` prices and acquires a session first
(`generation.rs:1850`–`:1861`); the dialogue does neither. Inside a turn
that was invisible — the parked turn holds no permit, and the scene was the
only thing running. It stopped being invisible when background sub-agent
runs landed: a foreground dialogue's ~25 requests and a background run's
stream can now overlap on a one-session engine, and under a unified KV pool
that is precisely the collective failure
[admission-by-budget.md](admission-by-budget.md) §3 reproduced. So the fix
belongs to this track whichever way the background fork goes (F4).

### 2.4 What the background mechanism already generalises

Kind-agnostic today, needing no work: the seat (`background_runs.rs:43`)
and its `InflightChild`, which already carries `line_role` for the
dialogue's speaker side (`mod.rs:557`); the progress fallthrough
(`background_runs.rs:121` → `mod.rs:1247`) covering `ChildLineStarted`,
`ChildTranscript`, `ChildRoundFiled`, `ChildStep`, `ChildTokens` and
`ChildEnded`, and forwarding `TranscriptLine`/`TranscriptReset`/
`TranscriptGrew` through `forward_child` (`mod.rs:1395`); landing by id
(`background_runs.rs:137`, `:178`, `:206`); the read paths — `view()`
(`mod.rs:1140`), `activate_focused`'s background `LiveTurn` including
`role: seat.child.line_role` (`mod.rs:1571`), `emit_chat_list`
(`mod.rs:1781`), `ChildSummary::of`; the lifecycle — stop, orphan-cancel,
chat-delete, `Quit`, the count on the bar, the auto-title at landing; and
the stage-2 keys `F6`/`Esc`, which resolve any background run by id.

**Sub-agent-shaped, and therefore in the way:**

- `ChildSpec` (`generation.rs:2748`): `parsed: SubagentArgs`, one request,
  one user message, `allowed`, `depth`.
- `BackgroundStart` (`generation.rs:2773`) and its `placeholder()` `:2787`,
  which hard-codes `kind: RunKind::Subagent`, no participants and a single
  user message. A dialogue's placeholder is the shape at
  `generation.rs:3366`–`:3381`.
- `spawn_background_run` (`generation.rs:2930`), which ends in
  `run_child(&shared, loc, spec)` `:2994`.
- `is_background_call` (`generation.rs:2573`), keyed on the tool name.
- The notification's wording: `finish_landing` hard-codes
  `tool.start_subagent.notification` (`background_runs.rs:253`) with the
  sub-agent's final reply as the body. A dialogue has no `name`
  (`generation.rs:3446`) and no "final reply" — its closing result is the
  director's reason and summary.

## 3. Measurement (2026-09-05)

The model-facing fork was settled once by the sub-agent track (§3.1 there:
an optional boolean is silently omitted by Claude 0/3 while a second tool is
used 3/3 on every cloud and 5/5 locally), so what this track has to measure
is narrower: does a model *choose* to background a **dialogue** when the
scene's outcome is not what the user is waiting for, and does it stay in the
foreground when the outcome is the whole request?

Instrument: `dlg_probe.py`, the sub-agent probe's harness with the dialogue's
own two tools — the shipped `run_dialogue` description from `locales/en.json`
and a `start_dialogue` twin carrying the §4.1 draft text — and the app's own
*started* line and closing result as the tool answers. Two scenarios, the
sub-agent probe's shape:

- **S1** — "stage me a full sixteen-line negotiation, I will read it later,
  start it and move on" plus an arithmetic question for right now; a
  background call is answered with the *started* line, the turn is allowed
  to end, the next user message is the task notification alone, and one line
  later the model is asked what the two sides settled on.
- **S2** — "stage a short scene and tell me right now how it ends": the
  outcome is the point, so the call must stay in the foreground.

Five trials of each scenario, twice (the second run classifying what the
first could not), Qwen 3.6 27B Q4_K_M on the LAN stack, b10807, thinking on,
temperature 1, `max_tokens` 4096.

| run | S1: asked for the background | S2: stayed in the foreground | r3: used the notification, no re-call | r4: what they settled on |
|---|:-:|:-:|:-:|:-:|
| 1 (2026-09-05) | **5/5** | 5/5 | 3/5 *(see below)* | 5/5 |
| 2, with the empty-turn classification | **5/5** | 5/5 | **5/5** (2 of them after a muted re-ask) | 5/5 |

**Go**, above the bar — and the interesting number is the one that looked
like a failure. In run 1 two of five notification turns produced *nothing*:
no tool call, no text. The next turn answered the question correctly from
the same notification, which is what said the model had read it; run 2
instrumented the turn and named the mode — `finish_reason: length` with the
**whole 4096-token cap spent in `reasoning_content`**, the all-thinking
empty turn this project has met twice before ([lessons §9](../lessons.md),
[two-agent-dialogue.md](two-agent-dialogue.md) §5). A single re-ask with
thinking muted recovered both, 2/2, exactly as it does for a participant
line.

That has a consequence beyond this probe, and it belongs to the **shipped**
background sub-agent feature rather than to dialogues: the app's *wake* turn
— the turn it starts on a landed notification — is precisely this shape, a
turn whose only new input is the notification, and on this model it can
land an empty assistant bubble. The dialogue's own lines already have the
one-shot muted re-ask (spec §9.13); the wake turn does not (F11).

The rest of the reading is unremarkable, which is the point: every trial
that backgrounded the scene said so plainly and answered the arithmetic in
the same reply, no trial invented how the scene ended before the
notification arrived, and S2 — where the outcome *is* the request — stayed
in the foreground 10/10.


## 4. Design

### 4.1 The ask: a second tool

`start_dialogue` — the same arguments as `run_dialogue`, its own
description, present in the effective set only when the background switch is
on. Not a flag and not a `mode` enum, for the reason the sibling measured.
The description is the shipped one plus the background clauses the
sub-agent twin proved: the call returns at once with the `chat://` address,
the director's closing result arrives later as a task notification, use it
when the scene's outcome is not needed for this reply, never guess at how
the scene ended, say it is still running if asked.

### 4.2 The run: the same seat, a dialogue-shaped spec

`is_background_call` gains the name; `start_background` takes the same path
it takes for a sub-agent — the cap check (`BackgroundSlots`, name-agnostic
already), a fresh cancellation token, the *started* result, and a
placeholder run that lands with the turn — but the payload it hands over
becomes a two-variant spec (F5): the sub-agent's `ChildSpec` or a
`DialogueSpec` carrying the parsed `DialogueArgs`, the **snapshotted**
director inputs (the parent's persona and the conversation brief, F3), the
sampling, and the dialogue timeout. `BackgroundStart::placeholder` branches
on the variant, so a dialogue's placeholder is `kind: Dialogue` with its
`participants` and the opening line — the shape `run_dialogue` builds at
`generation.rs:3366` today.

In the spawned task the background run reconstructs a `TurnLoop` over the
`TurnShared` built from `SharedParts` — with the snapshotted `ctx` (persona
included) and a request whose `messages` are the snapshotted brief tail,
`depth: 0`, the run's own token — and calls the **existing**
`TurnLoop::run_dialogue` unchanged (F6). This is what `run_child` already
does for a sub-agent: build a loop and run it. No dialogue logic moves.

### 4.3 Landing, delivery, stopping

Unchanged from the sub-agent path, because it is already generic: the record
is re-found by id and filled in (deferred to `handle_done` when a turn is
running in that chat), the attachments — none for a dialogue — are applied,
the run is auto-titled, and the result is delivered as a task notification
appended to the parent chat, with the wake turn when the chat is open and
idle and the unread mark when it is not. What changes is the **wording**
(F8): a dialogue's notification names the two participants and carries the
director's closing result — the same compact text §9.13's foreground result
returns (ended by decision / cap / timeout / cancelled / failed, the reason,
the summary, the address).

Stopping: `/subagents stop [n]` and `F6` resolve any background run by id
and already work; only the help text needs to stop saying "sub-agent" where
it now means "background run" (F9).

### 4.4 The budget

Every dialogue stream — participant line, director checkpoint, the muted
re-ask — goes through the same price-and-acquire the turn's own stream uses,
so the scene competes for permits and pool room like everything else (F4).
At `sessions = 1` a background scene and the parent's next turn take turns
request by request; under a unified pool each request reserves its estimate
plus its cap and waits for room. This also closes §2.3's defect for the
**foreground** dialogue, which is the same code path.

### 4.5 Limits

The scene's own budgets are unchanged: `max_messages` (the cap and the retry
meter), `tools.subagent_max_tokens` per line, and
`tools.dialogue_run_timeout_secs` — 1800 s, which was already sized for a
scene a user waits through and is exactly right for one they do not. The
background cap is shared with sub-agent runs (F7): `subagent_background_max`
counts every run out, and a `start_dialogue` past it is refused with the
same localized result naming the setting.

### 4.6 On disk

Nothing new. A dialogue run already serializes with `kind: Dialogue` and
`participants` (`CHAT_SCHEMA` 3), and `SubagentRun.background` is the flag
stage 1 of the sub-agent track added. A stored dialogue with `background`
and no outcome reads *unfinished* by the same rule.

## 5. Difficult spots

- **The brief is a snapshot, and the doc has to say so.** A director judging
  a scene twenty minutes after the call sees the conversation as it was when
  the caller staged it. That is the honest shape (§4.2, F3), but it is a
  behavioural difference from a foreground dialogue, whose director reads
  the turn's live tail, and it belongs in the tool's description as well as
  the spec.
- **A turn parked in a scene versus a scene with no turn.** The status-bar
  chip (`RunProgressKind::DialogueLine`/`DialogueDirector`) exists so a
  parked turn does not read as stuck. A background scene has no parked turn:
  the bar's *"in background: n"* indicator is the right report, and the
  scene chip should stay the turn's — otherwise the bar claims a turn is
  working when nothing is.
- **`Esc` on the open transcript** — settled by stage 2 of the sub-agent
  track: a background run's transcript streams under no turn, so `Esc` goes
  back and `F6` stops. A dialogue transcript inherits this for free, and the
  key's note ("stopping the background run «A ↔ B»") reads correctly.
- **Two spec variants, one duplication gate.** The enum in F5 must not be
  paid for by a second copy of `spawn_background_run` — the duplication gate
  measures density on changed lines, and two same-shaped spawn paths are
  exactly the shape [lessons §2](../lessons.md) records failing four times.

## 6. Forks

Recommendations marked; nothing is decided until the user says so.

- **F1. How the model asks.** (a) **A second tool, `start_dialogue`**
  *(recommended — the sibling's measurement, plus §3 here)*. (b) A
  `background` boolean on `run_dialogue` — rejected by the sibling's
  measurement (0/3 on Claude). (c) A required `mode` enum on
  `run_dialogue` — works, but changes the shipped tool's schema for every
  model and every profile.
- **F2. The gate.** (a) **The existing `tools.subagent_background` gates
  both twins** *(recommended — one switch for "a run that outlives the
  turn"; the settings row is reworded from "Subagent: background runs" to
  cover both, no new knob)*. (b) A separate `tools.dialogue_background`.
- **F3. The director's brief.** (a) **Snapshotted at the call**
  *(recommended — matches `SharedParts`' frozen shape; the brief exists to
  give the director the context the delegation was made in)*. (b) Refreshed
  at each checkpoint from the live chat — needs a round trip to the
  orchestrator, the sole owner of `Chat`, and lets the scene drift with
  whatever the user said meanwhile.
- **F4. The session budget.** (a) **Every dialogue stream priced and
  admitted, foreground and background alike** *(recommended — R3, and it
  closes §2.3's defect at the same seam)*. (b) Background dialogues only.
  (c) Leave the dialogue outside the budget — rejected: it is the exact
  overlap the admission track exists to prevent.
- **F5. The spec shape.** (a) **`BackgroundStart` carries a two-variant
  spec** (`Subagent(ChildSpec)` / `Dialogue(DialogueSpec)`) *(recommended)*.
  (b) A parallel start type and a second spawn path.
- **F6. Where the dialogue's code runs.** (a) **Rebuild a `TurnLoop` in the
  background task and call the existing `run_dialogue`** *(recommended —
  what `run_child` does for a sub-agent; zero dialogue logic moves, so the
  foreground behaviour cannot drift)*. (b) Lift the ~700-line dialogue
  driver off `TurnLoop` into a free function over an explicit context — a
  large refactor mixed into a behaviour change, which AGENTS.md §2 says not
  to mix.
- **F7. The cap.** (a) **Shared `subagent_background_max`** *(recommended —
  R7)*. (b) A separate dialogue cap.
- **F8. The notification.** (a) **A dialogue-specific wording** naming the
  participants and carrying the director's closing result *(recommended)*.
  (b) Reuse the sub-agent key.
- **F9. The stop routes' wording.** (a) **Keep `/subagents stop [n]` and
  `F6`, reword the texts to "background run"** *(recommended — one route
  for one concept)*. (b) A `/dialogues` command family.
- **F11. The wake turn's empty reply** (measured above, and a defect of the
  **shipped** background sub-agent feature, not of dialogues). (a) **Give the
  wake turn the one-shot muted re-ask the dialogue's lines already have**
  *(recommended — the measurement is here, the fix is one place, and without
  it a background result can land as an empty bubble on the gate model)*.
  (b) Leave it and record the mode in the journal. (c) Split it into its own
  fix PR before this track.
- **F10. Staging.** (a) **One PR** *(recommended — the mechanism, the
  surfaces and the keys all exist; this track adds a tool, a spec variant,
  the budget seam and texts)*. (b) Two, splitting the budget fix out first.

## 7. Stages and the test plan

**Probe → go/no-go** (§3): the bar is the sibling's — ≥ 4/5 background on
S1, 0/5 on S2, ≥ 4/5 notification use without a re-call.

**The track** (`feat/background-dialogues`), assuming F10(a). Unit tests
over the keyed engine, alongside the existing dialogue and background
suites: the catalog is byte-identical with the switch off and carries
`start_dialogue` on; a `start_dialogue` call records the *started* line and
lands a `kind: Dialogue` placeholder with its participants; the scene's
lines land on the record by id when it ends, with the notification carrying
the director's reason and summary; `Esc` leaves the scene running and lands
the turn; `/subagents stop` and `F6` end it and land the partial transcript
as *cancelled*; a background scene's transcript opens streaming on the
current speaker's side and a retake resets it; with `sessions = 1` the
scene's requests and the next turn's take turns (the counting mock sees one
at a time — the test that pins R3, and the one §2.3's defect would fail);
the cap refuses a third run of either kind; `Quit` lands the scene.

If F11(a): a unit test that a wake turn whose first generation comes back
empty is re-asked once with thinking muted, and lands the second reply.

**Live**: `background_dialogue_e2e_live` — the parent stages a scene in the
background and answers something else at once, the scene runs to the
director's stop or the cap, the notification carries the closing result, and
the woken turn reports it — on the LAN stack, plus a cloud arm if the user
wants one.

## 8. Not in this track

- **Background sub-agents' leftovers** that stay open regardless: a tasks
  screen across chats, the silent background tasks under the app-wide
  budget.
- **Nested background**: a dialogue cannot stage a dialogue, and a sub-agent
  cannot start one — `withheld_from_subagent` already covers both names.
- **Participants with tools**: ADR 0011 fork F4, unchanged by this track.
- **Per-speaker TTS** over a two-persona transcript
  ([two-agent-dialogue.md](two-agent-dialogue.md) §8).

## 9. Documentation touch list (AGENTS.md §4)

- spec §9.13 (the background twin, the snapshotted brief, the budget),
  §9.3.2 (the cap and the notification now cover both kinds), §11.2 (the row
  and the stop key already say "run"), §11.6 (the reworded setting).
- architecture §5 (the spec variant and the second spawn path), §8 (the
  tool).
- ADR 0011 — an amendment: decision 3's "one request in flight" becomes the
  session budget's guarantee; ADR 0010's second amendment gains the sibling.
- journal: tools.md; CHANGELOG `[Unreleased]`; README (the tool list and the
  setting); roadmap (the item closes); locales `en`/`ru`.
