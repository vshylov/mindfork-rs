# The loops' timings — the silent tasks' cold prompts, sampled at the landing

> **Status:** implemented (2026-09-09) — every fork at its recommendation
> (the user's decision, 2026-09-09); stage 0 is the measurement in §2.1
> (the reflection loop's rounds on the GPU stack, with a scratch print in
> `record_round_usage`), stage 1's live runs on the CPU build and the GPU
> stack are in §6.1. The item the roll-timings track recorded
> ([roll-timings.md](roll-timings.md) §7, fork F2b): the three silent
> loops — reflection, the notes consolidation, the self-consolidation —
> stream through the same client as the turn and the roll, their first
> round is the largest cold prompt a session makes, and their `usage` is
> read for the budget's calibration and dropped.

## 1. Why, precisely

The slow-prefill note is computed from one sample per server session: the
prompt tokens `llama-server` actually processed, net of its prefix cache,
over the milliseconds they took. The turn gives that sample on a fresh
server; the roll gives it on a warm one, where every turn processes tens
of tokens under the rule's 256-token floor ([roll-timings.md](roll-timings.md)
§2.1). What neither covers is the warm server's *ordinary* day: the app
reconnecting to a `llama-server` that kept running, a conversation under
the compaction threshold — no roll — and every turn warm. The silent
loops fire there on their own cadence: reflection every N replies, the
consolidations every M. Each sends a **different prefix** — its own
instructions, its tool schemas, a digest — so each first round is
processed cold, and it is the largest cold prompt a session makes: the
reflection's first round is 2798 tokens (§2.1), nearly twice the roll's
1466 and the turn's 1430. Today `record_round_usage` reads that usage for
the budget's calibration and keeps nothing else; the loop lands as
`(BackgroundKind, BgOutcome)` and the figure is gone.

**Requirements.**

- **R1. The three loops' figures reach the rule** — the same `Prefill` the
  turn and the roll carry, offered to the same `note_slow_prefill`.
- **R2. The same note, once** — the one-per-server-session claim is not
  touched; a loop after a turn or a roll that was told changes nothing.
- **R3. Every ending offers what it measured** — a loop stopped in its
  third round, timed out in its second, or failed on a later stream still
  measured its first; the sample is the engine's fact, not the task's
  verdict (the roll-timings track's R3, kept for the loops).

## 2. What is there

### 2.1 Measured (stage 0, the GPU stack)

`auto_reflect_e2e_live` on the LAN `llama-server` (Qwen3.6-27B, 4 slots),
with a scratch `eprintln!` of each round's usage in `record_round_usage`
(reverted): one short turn, then the reflection loop —

| round | `prompt_tokens` | `prefill.tokens` (net of the cache) | `prefill.ms` | tok/s |
|---|---:|---:|---:|---:|
| reflection, round 1 | 2798 | **2798** | 1054 | 2655 |
| reflection, round 2 (the tool result appended) | 2839 | 45 | 183 | — |
| reflection, round 3 | — | (the stream cut by the quit; no usage) | | |

For comparison, the same server the day before ([roll-timings.md](roll-timings.md)
§2.1): a cold turn 1430 tokens, the roll 1466, a turn on a warm prefix 31.
Three readings. The loop's first round is processed whole and cold — its
prefix (the reflection's system prompt, its tool schemas, the digest) is
nobody else's. Its later rounds are warm, tens of tokens: the first is
the sample, and "the largest" picks it as it picks the turn's first round.
And a stream cut short — by the quit here, by a stop or the loop's limit
elsewhere — has no usage: the chunk is the stream's last.

### 2.2 The code

- `tool_loop::run_rounds` streams each round (`stream_round` →
  `read_round`, which keeps the `Usage` chunk), calls
  `record_round_usage(ctx, estimate, usage, &mut last_exact)` — the
  budget's calibration and the next round's floor — and returns
  `Result<RoundsEnd, Error>`: `Done` | `Cancelled { wrote }` | `TimedOut`,
  or `Err` on a stream error.
- `spawn_silent_loop` maps that to `BgOutcome::{Done, Cancelled {
  consumed }, Failed(String)}` and sends `(kind, outcome)` on `done_tx`;
  `run`'s loop and `settle_silent_tasks` receive it and call
  `handle_bg_done(kind, outcome)`: the slot cleared, the streak, the
  refund, the events.
- The roll lands elsewhere — `handle_compact_result` builds its outcome,
  calls `handle_bg_done`, then offers `CompactResult.prefill` to
  `note_slow_prefill` itself (roll-timings §3.2).
- The three spawns (`reflection.rs`, `consolidation.rs`,
  `self_consolidation.rs`) build one `SilentLoop` each; the loops' limits
  are constants — reflection 6 rounds under 120 s of streaming, the
  consolidations 8 under 180 s.

## 3. Design

### 3.1 The loop keeps its largest sample

`run_rounds` takes one more out-parameter, `prefill: &mut Option<Prefill>`
— the shape `run_tools` already uses for `wrote` — and
`record_round_usage` keeps the round's sample on it when its `tokens`
exceed what is there. An out-parameter, not a field of `RoundsEnd`: the
sample must survive every way out of the loop — `Done`, `Cancelled`,
`TimedOut`, and the `?` on a stream error — and a field on one variant
covers one of them (R3). A displaced round is made again and only the
retry's stream reaches its usage; a stream cut by the loop's own limit
never does.

### 3.2 The landing carries it

The channel's tuple becomes a small struct, `BgDone { kind, outcome,
prefill }`, and `handle_bg_done(kind, outcome, prefill)` takes the sample
beside the outcome. `spawn_silent_loop` fills it from the out-parameter
whatever the outcome; the roll's landing passes `CompactResult.prefill`
the same way.

### 3.3 One offer, at the one landing

`handle_bg_done` offers the sample to `note_slow_prefill` at its end, after
the slot's events, for every kind — and the explicit offer in
`handle_compact_result` goes: every silent task lands there, so the
rule is asked there, once. The order keeps the roll-timings reading: the
task's own landing first (`Compacted`, the task-list refresh), the note
after it. Nothing in the rule changes — the same batch reading, the same
floor, the same one claim per server session (R2).

### 3.4 What the user sees

On a warm external server with a conversation under the compaction
threshold: the first reflection or consolidation of the session says the
note, once, if the hold is worth saying. On a fresh server, or where a
roll came first: nothing new — the claim is already taken. A slow host
whose loops never end a stream inside their limit hears nothing from
them (§4): the turn's or the roll's sample is what the note has there.

## 4. Difficult spots

- **A stream cut by the loop's limit has no figure.** The usage chunk is
  the stream's last, and the loop's time limit runs over its streaming.
  On the CPU build at 38 tok/s the reflection's 2800-token first round is
  74 s of prefill under a 120 s limit: its sample exists only if the
  model's first reply is short. That is the host's fact, not a defect —
  and the very thing the note is for; the live arm (§6) is planned around
  it.
- **The loops' prompts are the tool schemas' size.** 2798 tokens after one
  short turn: the instructions and the schemas dominate, the digest is
  small. The sample is therefore stable across chats — and always above
  the floor.
- **The channel's shape.** `(BackgroundKind, BgOutcome)` is destructured by
  `run`, `settle_silent_tasks` and the tests' harness (`landed`, `quit`,
  `spawn_loop_with`); a struct in its place is mechanical there.
- **The roll's `Failed` now offers too.** With the sample beside the
  outcome, a manual roll whose summary was discarded (a vanished
  boundary) still offers its figure at the same landing — roll-timings
  R3, which its own path already honoured — and a stopped roll offers
  `None`, as before.

## 5. Forks

- **F1. The carrier.** (a) **Beside the outcome: `run_rounds` keeps the
  largest sample on an out-parameter, the landing carries `BgDone { kind,
  outcome, prefill }`, every ending included** *(recommended — R3; the
  shape `wrote` already has)*. (b) Inside the outcome — `BgOutcome::Done {
  prefill }`, the item's label: a stopped, timed-out or failed loop drops
  its sample, and keeping the roll's R3 would put the field on `Failed`
  too. (c) A cell on the slot, as `Acted` is — right for a fact read
  before the landing (the quit's refund); nothing reads this one early.
- **F2. The loop's sample.** (a) **The largest `prompt_n` across its
  rounds** *(recommended — the turn's rule; measured, the first round)*.
  (b) The first round's only — the same figure today, and a rule to
  explain. (c) Each round's as it lands — the loop reaches the
  orchestrator only through its landing.
- **F3. The offer site.** (a) **`handle_bg_done`, once for every silent
  task — the roll's sample rides the same landing and the explicit offer
  in `handle_compact_result` goes** *(recommended — one landing, one
  offer)*. (b) Each spawn's own landing — the loops in `handle_bg_done`,
  the roll where it is: two sites for one rule.
- **F4. Which loops.** (a) **All three** *(recommended — one `SilentLoop`,
  one change)*. (b) Reflection only — the most frequent, and the one
  measured; the consolidations would need the same line later.
- **F5. Staging.** (a) **One PR, live-gated with a two-phase smoke**
  *(recommended — §6)*. (b) Not gated: the unit tests script the figure;
  the live arm is what shows a loop's real first round reaching the note.

## 6. Tests and the live run

- `tests/silent.rs`: a scripted loop whose round streams carry usage with
  a sample — the landing carries the largest; a loop stopped in its second
  round still carries the first's; a loop whose stream ended short carries
  `None`; the harness's `landed`/`quit`/`spawn_loop_with` read `BgDone`.
- `tests/reflection.rs` (through the orchestrator, an external server):
  a reflection whose first round carries a slow sample lands, then the
  note; the same server is not told twice; a fast sample says nothing.
- `tests/compaction.rs`: the roll's tests pass unchanged — its offer moved,
  not its behaviour; a discarded summary's landing offers.
- `tests/live.rs`: `loop_prefill_e2e_live`, two phases on one data root.
  Phase 1 — a long turn (the forty-paragraph seed), reflection off; quit.
  Phase 2 — the app restarted on the same root against the same server
  (its cache warm), `auto_reflect_every = 1`, one short turn — under the
  floor — then the reflection: its landing, then the note within 3 s under
  `MINDFORK_EXPECT_SLOW_PREFILL=1` (the CPU build), none without it (the
  GPU stack). The CPU build's reflection may end its limit before its
  first stream ends (§4); if it does, the arm moves to the
  self-consolidation loop (two observations seeded in phase 1, a 180 s
  limit) and §6.1 says so.
- The LAN regression trio: `roll_prefill_e2e_live`,
  `slow_prefill_e2e_live`, `auto_reflect_e2e_live`.

### 6.1 Runs

`loop_prefill_e2e_live`, two phases on one data root: phase 1 a long turn
with the memory tools enabled, no reflection, a quit; phase 2 the app
restarted on the same root against the same server, a short turn, the
reflection at its landing.

| host | the phase-2 turn | the reflection | the note |
|---|---|---|---|
| the CPU build (gemma-3-4b-it Q8, `-ngl 0 -c 8192`, the default batch) | 35.2 s — **52 tokens** processed, under the floor | landed 54.3 s after the turn | **35 tok/s, a hold of about 58 s at 2048, `-b 256 -ub 256`** — from the reflection's landing |
| the GPU stack (Qwen3.6-27B, the 4090, 4 slots) | 3.0 s | landed 38.9 s after | none |

A second CPU run with a scratch print of every offer and every round's
usage (reverted) said who gave what: the phase-1 turn offered 6 tokens
(the seed still in the cache from the first run), the phase-2 turn 52
tokens in 16 s — the server was prefilling phase 1's cold title request
beside it, and 3 tok/s is the figure the floor exists to refuse — and the
reflection's first round 1664 prompt tokens, 514 of them processed (the
rest cached from the first run's reflection) in 15.0 s, 34 tok/s: the
landing's offer was that sample, and the note followed the landing. On
Gemma's tokenizer the reflection's prompt is 1664 tokens, not Qwen's
2798 — 47 s of prefill on this host, inside the loop's 120 s limit; §4's
risk did not bite and the fallback was not needed.

The LAN regression trio after it — `roll_prefill_e2e_live` (2.4 s, no
note), `slow_prefill_e2e_live` (4.8 s, *lamp*, no note),
`auto_reflect_e2e_live` — 3/3. One run the smoke lost while being written:
the reflection is gated on the profile's enabled tools, so the first run
waited 300 s for a loop that was never spawned; phase 1 now enables the
tools before its turn (the schemas are part of the prefix phase 2 must
find warm), and the smoke waits for the spawn before the landing, so
"never spawned" and "never landed" read differently.

Unit: 2967 green, 149 ignored (five tests and the smoke added).

## 7. Not in this track

- **The roll's usage for the budget's calibration**
  ([roll-timings.md](roll-timings.md) §7).
- **A figure on the settings screen** — beside the *Batch (-b)* field
  ([slow-prefill-detection.md](slow-prefill-detection.md) §7).
- **A loop's limit against a slow host** — a reflection that cannot end
  its first stream inside 120 s on a CPU build is its own question.
- **A tool's own request inside a round** — the page summary's sample,
  done: [page-summary-usage.md](page-summary-usage.md) §3.2.

## 8. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the sentence naming every prompt the app reads: the loops'
  included), §17.6 (the loops' landing carries the sample).
- architecture §6 (the `ChatChunk` bullet: the loops' sample, one offer
  at `handle_bg_done`).
- [roll-timings.md](roll-timings.md) §7 (done here).
- CHANGELOG (the unreleased note's entry: the loops named), journal
  `engine.md`, CLAUDE.md's status line and count.
