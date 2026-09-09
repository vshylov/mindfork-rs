# The roll's timings — the session's coldest prompt, sampled

> **Status:** implemented (2026-09-08) — every fork at its recommendation
> (the user's decision, 2026-09-08); stage 0 is the measurement in §2.1
> (a turn on a warm prefix against a roll's digest, on the GPU stack),
> stage 1's live runs on the CPU build and the GPU stack are in §6.1.
> The item the slow-prefill track recorded
> ([slow-prefill-detection.md](slow-prefill-detection.md) §7, fork F5b):
> the compaction roll prefills the whole folded conversation cold and
> drops its `usage` on the floor — the one stream whose figure is exactly
> the hold the note is about, and the one sample a warm server ever gives.

## 1. Why, precisely

The slow-prefill track reads `llama-server`'s own clock over every prompt
— `timings.prompt_n` over `prompt_ms`, net of the prefix cache — keeps the
turn's largest sample, and once per server session says in the feed how
long a stream cancelled during its prompt would hold its slot at the batch
the server runs, and the one change. Its sample is the turn's rounds
(F5a), on the reading that every session's first turn processes the
system prompt and the history cold. Two things that reading leaves out:

- **A warm server gives no turn sample.** The app reconnecting to a
  `llama-server` that kept running — the external mode's ordinary day —
  finds the chat's prefix still in a slot's cache: every turn of the
  session processes only its own new tokens, tens of them, under the
  rule's 256-token floor (§2.1: 31 tokens). The rule then stays silent
  for the whole session, on exactly the CPU-only external box the batch
  track could not reach and the detection track was built for.
- **The roll is the sample.** A compaction roll sends a *different*
  prefix — the summarizer's system prompt and a digest that is new every
  roll — so its prompt is processed cold, and it is the largest cold
  prompt a session makes (§2.1: 1466 tokens where the turn on the warm
  prefix processed 31). It is also the very stream the note is about: a
  roll displaced by a turn is the case the batch track measured (23 s at
  the default batch on the CPU build). Today `spawn_compact`'s collect
  loop matches `ChatChunk::Usage(_) => {}` and the figure is gone.

**Requirements.**

- **R1. The roll's figure reaches the rule** — the same `Prefill` the turn
  carries, offered to the same `note_slow_prefill`.
- **R2. The same note, once** — the one-per-server-session claim is not
  touched; a roll after a turn that was already told changes nothing.
- **R3. A fact, not a verdict** — the sample is the engine's; it is
  offered whenever the stream delivered it, whatever the app then made of
  the summary text.

## 2. What is there

### 2.1 Measured (stage 0, the GPU stack)

Qwen3.6-27B on the LAN `llama-server` (4 slots over 16384), streamed with
`stream_options.include_usage` as the app's client always sends it, the
`timings` read off the last chunk — five requests in a row:

| request | `prompt_n` | `cache_n` | `prompt_ms` | tok/s |
|---|---:|---:|---:|---:|
| turn 1, cold (system + a forty-paragraph seed) | 1430 | 0 | 608 | 2351 |
| turn 1 again (same prefix, warm) | 4 | 1426 | 105 | 38 |
| turn 2 on the warm prefix (one short reply and a question) | 31 | 1426 | 247 | 125 |
| **the roll**: the summarizer's system + a digest of the same text, cold | **1466** | 0 | 628 | **2335** |
| the roll again (a digest never repeats in practice) | 4 | 1462 | 104 | 38 |

Three readings. A turn on a warm prefix processes tens of tokens — under
the rule's floor, so no sample, and rightly so: its 125 tok/s (and the
38 of a four-token prompt) is the per-request overhead, not the server's
throughput. The roll's prompt is processed cold — a different system
prompt, a new digest — and its figure is the cold turn's figure. And the
signal arrives on the roll's stream as it does on a turn's: the roll goes
through the same client and the same `include_usage`.

### 2.2 The code

- `spawn_compact` (`orchestrator/compaction.rs`) collects the stream into
  `(text, thoughts, truncated, cancelled)` and drops `Usage`; the result
  crosses to the orchestrator as `CompactResult { chat_id, boundary_id,
  rolls, origin, text: Result<String, CompactEnd> }` on `compact_tx`.
- `handle_compact_result` applies the summary (`apply_compaction`:
  boundary re-validated, an empty summary discarded) and lands the slot
  through `handle_bg_done`.
- `note_slow_prefill(Option<Prefill>)` (`generation.rs`) is the rule's
  one entry — managed vs external batch, `prefill_hold`, the session's
  claim, the note — called from `handle_done` only.
- The client fills `TokenUsage.prefill` from the chunk's `timings`; the
  usage chunk is the stream's **last** (after the one carrying
  `finish_reason`), so a stream that ended short — cancelled, displaced,
  an error, the timeout — never receives it.

## 3. Design

### 3.1 The wire: the sample rides the result

The collect loop keeps the `Usage` chunk's `prefill` beside the text, and
`CompactResult` gains `prefill: Option<Prefill>`. The loop's four-tuple
becomes a small named struct (`Collected { text, thoughts, truncated,
cancelled, prefill }`) — a fifth positional field would make the landing's
match arms unreadable, and Sonar's complexity bar is near on this function
already. The sample is set on the path that reached the stream's end (a
summary, truncated or not); `Cancelled` and `Failed` carry `None` — the
usage chunk never came. A displaced roll's retry is a new collect, and the
retry's usage is the one that lands.

### 3.2 The landing: offered to the rule

`handle_compact_result` offers the sample to `note_slow_prefill` **after**
the roll's own landing (`handle_bg_done`), so the `Compacted` event and a
manual roll's refusal precede the note in the feed — the note reads as a
footnote to the roll, not a preface. Nothing in the rule changes: the same
batch reading, the same floor, the same one-per-session claim; a session
whose turn already claimed the note hears nothing from the roll (R2).

The sample is offered regardless of what the landing made of the text
(R3): a summary discarded for a vanished boundary or for being empty was
still a prompt the engine processed at its speed.

### 3.3 The sessions that change

- A session that starts with `/compact` — a long chat reopened, the
  command the first request: the roll is the first cold prompt, the note
  comes from it.
- A long chat on a warm external server: every turn under the floor, the
  automatic roll cold and large — the session's only sample.
- A managed server, or a cold external one: the first turn samples and
  claims; the roll changes nothing.

## 4. Difficult spots

- **The roll beside another stream.** The automatic roll fires from
  `handle_done`, after the turn's stream ended; a background run or a
  sub-agent may still decode beside it on a shared pool. The figure is
  then the throughput the user actually gets — the one the hold depends
  on, as the detection track already reads it.
- **A digest under the floor.** A first roll of a small chat with a short
  folded region can process fewer than 256 tokens (the summarizer's system
  prompt is short). The floor holds: no sample, as for a short turn.
- **The note lands late.** An automatic roll takes seconds to minutes; its
  note arrives mid-silence, after the turn it followed. It is a feed
  notice like the turn's, and one per session.
- **The result's literals.** `CompactResult` is built in two places in
  the task and in five tests; a new field is mechanical there.

## 5. Forks

- **F1. Where the sample is read.** (a) **In the roll's collect loop,
  carried on `CompactResult` as `prefill: Option<Prefill>`, offered at
  the landing** *(recommended — the landing is the one place the
  orchestrator handles a roll, and `note_slow_prefill` is the rule's
  single entry)*. (b) Carry the whole `TokenUsage` on the result — the
  roll's cost has no consumer today (the budget's calibration is fed by
  the turns and the loops; a roll's digest is prose, not the request
  shape the ratio is calibrated on), so the field would be dead weight
  until a consumer appears.
- **F2. Which silent streams sample.** (a) **The roll only** *(recommended
  — the item named; the roll is the session's largest cold prompt, and
  covers the two sessions the turn cannot, §3.3)*. (b) The three loops
  too — `RoundsEnd::Done` carrying the largest sample through
  `BgOutcome::Done` (four constructors and every test on them) for a
  reflection's first round, which prefills a window of messages the turn
  before it usually already processed.
- **F3. Which landings offer the sample.** (a) **Every landing whose
  stream delivered it, before the verdict on the text** *(recommended —
  R3)*. (b) Only an applied summary — ties the engine's fact to a check
  about the chat.
- **F4. Staging.** (a) **One PR, live-gated** *(recommended — the CPU
  build's roll gives the note first thing in a session, the GPU stack's
  does not)*. (b) Not gated: the unit tests script the figure, but the
  live arm is what shows the sample arriving on a *real* roll's stream.

## 6. Tests and the live run

- `tests/compaction.rs`: the recording engine gains an optional usage
  chunk; a manual roll on an external server whose stream carries a slow
  sample ends with `Compacted` **then** the note; a fast sample says
  nothing; a slow sample after the turn already claimed the session says
  nothing; a stream that ends in an error carries `None`; the five
  result literals gain the field.
- `tests/live.rs`: `roll_prefill_e2e_live` — external mode, a chat seeded
  on disk (a forty-paragraph seed over a few exchanges, `tail_tokens` 120)
  and reopened, `/compact` the session's first request, `Compacted`, then
  the note within 3 s under `MINDFORK_EXPECT_SLOW_PREFILL=1` (the CPU
  build), none without it (the GPU stack).
- The LAN regression trio: the previous track's `slow_prefill_e2e_live`,
  `compaction_preserves_a_planted_fact_e2e_live`, and the new smoke.

### 6.1 Runs

Both arms of `roll_prefill_e2e_live` — a chat of four exchanges seeded on
disk, the app started on it, `/compact` its first request:

| host | the roll | folded | the note |
|---|---:|---|---|
| the CPU build (gemma-3-4b-it Q8, `-ngl 0 -c 2048`, the default batch) | 78.7 s | 6 messages → 1457 chars | **37 tok/s, a hold of about 55 s at the default 2048, `-b 256 -ub 256`** |
| the GPU stack (Qwen3.6-27B, the 4090, 4 slots) | 2.7 s | 6 messages → 304 chars | none |

The CPU build's note came from the roll — no turn had run in the session
— with the figure the slow-prefill track's turn gave on the same server
(38 tok/s, a 54 s hold). The LAN regression pair after it: the previous
track's `slow_prefill_e2e_live` (the turn 5.6 s, *Lamp*, no note) and
`compaction_preserves_a_planted_fact_e2e_live` (the planted code answered
before and after the roll), 2/2. One run the smoke lost while being
written: a data root seeded with chats and no `data.db` makes the bootstrap
put its own notice in the feed, which a wait for "the first `Notice`"
took for the roll's — the smoke now opens the storage once before the
app starts, so the database is there.

Unit: 2962 green, 148 ignored (four tests and the smoke added).

## 7. Not in this track

- **The loops' samples** (F2b) — done: [loop-timings.md](loop-timings.md).
- **The roll's usage for the budget's calibration** (F1b's consumer) — done: [roll-usage-calibration.md](roll-usage-calibration.md), which found the estimate's missing term first.
- **A figure on the settings screen** — beside the *Batch (-b)* field
  ([slow-prefill-detection.md](slow-prefill-detection.md) §7).

## 8. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the sentence naming every prompt the app reads: the roll's
  included), §6.7 (the roll's result carries the sample).
- architecture §5 (`CompactResult.prefill`, the landing's offer), §6.
- [slow-prefill-detection.md](slow-prefill-detection.md) §7 (done here).
- CHANGELOG (the unreleased note's entry: the roll named), journal
  `engine.md`, CLAUDE.md's status line and count.
