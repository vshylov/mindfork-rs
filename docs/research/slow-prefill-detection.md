# A slow prefill, detected on the fly — the batch a cancel waits for, told to the user

> **Status:** implemented (2026-09-08) — every fork at its recommendation
> (the user's decision, 2026-09-08); stage 0 was the measurement in §2.1
> (the engine's own `timings`), stage 1's live runs on the CPU build and the
> GPU stack are in §6.1. The item the batch track recorded
> ([cpu-batch.md](cpu-batch.md) §7): the batch is a launch flag, so a
> detection at runtime can only *advise* — and the advice is worth giving,
> because the app's automatic `-b 256` covers exactly one shape, a managed
> server with no GPU layers, and every other slow prefill is silent.

## 1. Why, precisely

The batch track measured the harm and fixed the case it could see: a
`llama-server` looks at its queue between batches of `-b` prompt tokens,
so a stream cancelled during its prefill — a displaced roll, a stopped
task — holds its slot for one batch: **23 s** at the default 2048 on the
CPU build, 6.5 s at 256. The fix was a launch-line default: `-ngl 0` →
`-b 256 -ub 256`. It covers a managed server with no GPU layers and
nothing else: a managed server with a partial offload on a small GPU, a
managed server on a slow GPU, an external `llama-server` the user launched
with the default batch, a CPU-only box the user runs in external mode —
each prefills slowly, each holds its slot for seconds on a cancel, and
none says so. The user learns it as the batch track did: a turn that waits
tens of seconds behind a roll they stopped.

The app cannot change a launch flag at runtime, and should not change a
setting the user did not type (the batch track's F5 drew that line). What
it can do is **measure and say**: how fast this server processes prompts,
how long a cancelled stream would therefore hold its slot, and the one
change that halves it — the *Batch (-b)* field for a managed server, the
launch line for an external one.

**Requirements.**

- **R1. The engine's own figure**, not a guess: prompt tokens processed
  per second net of the prefix cache, as the server reports it.
- **R2. The harm in the user's terms**: seconds a cancelled stream would
  hold its slot, computed from the batch the server was launched with.
- **R3. Once, and with the route**: one note per server session, naming
  the field or the flag, never a second time for the same server.
- **R4. Quiet where it does not apply**: no note on the clouds, on a server
  that reports no timings, on a batch already at the knee, on a prompt too
  short to measure.

## 2. What exists (inventory)

### 2.1 The signal is already on the wire

`llama-server`'s OpenAI-compatible stream ends with a chunk carrying
`usage` **and** `timings` (measured on the LAN stack, 2026-09-08):

```json
"timings": {"cache_n": 0, "prompt_n": 16, "prompt_ms": 104.1,
            "prompt_per_token_ms": 6.5, "prompt_per_second": 153.7,
            "predicted_n": 16, "predicted_ms": 366.4, ...}
```

`prompt_n` is the prompt tokens actually processed — **net of `cache_n`**,
the prefix reused from the slot — and `prompt_ms` the time they took: the
exact prefill throughput, cache-aware, no estimate. The app's client
(`shared/api/openai/client.rs`) parses `usage` into `ChatChunk::Usage`
and drops `timings`; `dialogue_probe.rs` reads them by hand for its own
measurement and says so. No other provider sends them: the clouds have no
batch to speak of, vLLM/Ollama/LM Studio report nothing — R4 by
construction.

### 2.2 The batch the server runs

Managed: `build_args` (`shared/api/managed.rs`) passes `-b n` when the
field is typed, `-b 256` when it is empty and `gpu_layers == 0`, nothing
otherwise — the server's default, 2048. External: `/props` does not expose
`n_batch` (checked: `default_generation_settings` carries `params` and
`n_ctx` only), so the app cannot know; the server's default is 2048 unless
the user passed `-b`.

### 2.3 Where a round's usage lands

The turn loop (`generation.rs`) reads `ChatChunk::Usage` per round into
`usage_prompt`/`usage_tokens` and folds the turn into `TurnUsage {
prompt_tokens, completion_tokens }` on `GenResult.usage`; `handle_done`
reads it for the compaction trigger. The silent loops read their usage in
`tool_loop.rs::record_round_usage`; the roll's stream drops it.

### 2.4 Notes and sessions

`AppEvent::Notice(String)` is a feed note (the compaction notices);
`engines.rs` sees every status transition (`set_chat_status`,
`note_recovery` on `Ready`) — a server session begins at `Ready`.

## 3. Design

### 3.1 The figure travels with the usage

`TokenUsage` gains `prefill: Option<Prefill { tokens: u32, ms: u32 }>` —
`timings.prompt_n` and `timings.prompt_ms` when the chunk carries them
(the wire struct gains an optional `timings`), `None` otherwise. The turn
loop keeps the round with the **largest `prompt_n`** — the first round of a
turn on a cold cache is the good sample, later rounds ride the cache —
and `TurnUsage` gains the same `prefill`.

### 3.2 The rule

A pure function, `slow_prefill(batch: u32, prefill: Prefill) -> Option<u32>`
(the hold in seconds when it is worth saying):

- the sample counts only when `tokens >= 256` — a batch's worth, so the
  per-request overhead does not pass for throughput (16 tokens at 104 ms
  on the LAN GPU read as 154 tok/s; a real prompt there runs in the
  thousands);
- `tps = tokens / ms · 1000`; the hold `= batch / tps`;
- worth saying when the hold is **above 5 s** and the batch is **above
  256** — at 256 the knee is already taken, and 128 was measured as +20 %
  prefill for little gain (cpu-batch §3.1).

`batch` is the managed launch line's (`build_args`' rule, lifted into a
function both it and the detector call) or **2048** for an external
server (fork F3). On the CPU build at the default that is 2048 / ~90 tok/s
≈ 23 s — the batch track's number, now derived from the engine's own
report; on the LAN GPU 2048 / ~2000 ≈ 1 s — quiet.

### 3.3 The note, once

`handle_done` hands the turn's `prefill` to `note_slow_prefill`: if the
rule says a hold, and no note went out **this server session**, one
`AppEvent::Notice` — the throughput, the hold, the batch, and the route:

- managed: *"This server processes prompts at N tokens/s: a background
  request stopped or displaced during its prompt would hold its slot for
  about S s (the server looks at its queue between batches of B tokens).
  Set Batch (-b) to 256 in Settings → Model → Performance — the server
  restarts."*
- external: *"… Launch llama-server with `-b 256 -ub 256`."*

The flag `prefill_noted` clears when the chat server reaches `Ready` again
(`note_recovery`'s neighbour): a relaunch with a new batch measures afresh
and stays quiet if the hold is fine; the same server nags nobody. The
note is also a `tracing::info!` with the raw figures, for the log.

### 3.4 What the user sees

On a slow host, after the first substantial turn: one note in the feed
with the numbers and the one change to make. On a GPU host: nothing. On
the clouds: nothing. After setting the batch: nothing, since the hold is
now under the bar — or the note again if it is still not, with the new
figure.

## 4. Difficult spots

- **The cache hides the prompt.** With a warm prefix only the new tokens
  are processed; `prompt_n` says so, and a turn whose processed tokens are
  under 256 is no sample. The first turn of a session processes the
  system prompt and the history — the sample the rule wants.
- **A batch the app does not know.** External servers: the assumption
  (2048) is the server's default, and a user who passed `-b` already
  knows the flag; the note names the assumption ("at the default batch").
- **The roll's stream is the best sample and drops its usage.** The roll
  prefills the whole conversation cold; reading its timings would need
  `CompactResult` to carry usage. Not in this track — the turn's sample
  is there on every session's first turn.
- **The figure's noise.** `prompt_ms` is the server's own clock over
  `prompt_n` tokens; at 256+ tokens the per-request overhead is a few
  percent. The bar (5 s) is far from the CPU build's 23 s and from the
  GPU's 1 s, so a noisy sample near the bar changes nothing that matters.
- **Concurrency.** A prompt processed while another stream decodes on a
  shared pool is slower than alone; the figure is then the throughput the
  user actually gets, which is the one the hold depends on.

## 5. Forks

- **F1. The signal.** (a) **`llama-server`'s `timings` — `prompt_n` over
  `prompt_ms`, net of the cache** *(recommended — R1; exact, and only the
  engine with the batch problem sends it)*. (b) Time to first token over
  the app's prompt estimate — any server, but the estimate is a byte
  ratio and the cache invisible. (c) Both, timings first.
- **F2. The rule.** (a) **The hold `batch / tps` above 5 s, on a sample of
  at least 256 processed tokens, with the batch above 256** *(recommended
  — R2, the harm in seconds)*. (b) A throughput floor alone (below 400
  tok/s) — ignores the batch the user already set. (c) The hold above
  10 s — the CPU build's 6.5 s at 256 would still be silent, but so would a
  13 s hold at 512.
- **F3. The batch of an external server.** (a) **Assume 2048, the server's
  default, and say so in the note** *(recommended — `/props` does not
  expose it)*. (b) Skip external servers — the CPU-only external box is
  exactly the user the batch track could not reach.
- **F4. The action.** (a) **One feed note per server session, numbers and
  route** *(recommended — R3; the batch track's line: advise, never set
  what the user did not type)*. (b) Set the field to 256 when it is
  empty and restart — a restart the user did not ask for, mid-session. (c)
  A hint on the settings field only — read by nobody at the moment it
  matters.
- **F5. The sample.** (a) **The turn's rounds, the largest `prompt_n`**
  *(recommended — every session's first turn)*. (b) Every stream, the
  silent loops and the roll included — the roll's usage is not carried
  today.
- **F6. Staging.** (a) **One PR, live-gated** *(recommended)*. (b) Two.

## 6. Tests and the live run

- `shared/api/openai/wire.rs` / `client.rs`: a chunk with `timings` yields
  `Usage` with `prefill: Some(Prefill { tokens, ms })`; without → `None`;
  a chunk with `usage` but no `timings` (a non-llama server) → `None`.
- `generation.rs`: a two-round turn whose first round processed 1200
  tokens and whose second processed 40 lands `TurnUsage.prefill` as the
  first; a turn with no timings lands `None`.
- The rule (pure): 2048 / 90 tok/s → `Some(23)`; 256 / 90 → `None` (the
  batch at the knee); 2048 / 2000 → `None`; 100 tokens in 5 s → `None`
  (too short a sample); 1024 / 150 → `Some(7)`.
- The orchestrator (`tests/generation.rs` or a new `tests/prefill.rs`): a
  turn landing with a slow prefill on a managed engine at the default
  batch → one `Notice` naming the field and the hold; the next turn → no
  second note; `Ready` again → the note once more; a managed engine with
  `batch_size: Some(256)` → none; external mode → the launch-line wording;
  a fast prefill → none.
- **Live — required** (the client parses a new field of the engine's
  stream, and the rule's numbers are the engine's): `slow_prefill_e2e_live`
  on the **CPU build** in external mode at the default batch (`-c 2048`, no
  `-b`): a seeded turn processes ≥ 256 tokens and the note arrives with a
  hold in the tens of seconds; the same build launched with `-b 256`: no
  note. On the **LAN stack**: a seeded turn, no note, the figure logged;
  and the regression trio.

### 6.1 The run (2026-09-08)

The unit suite: **2958 green, 147 `#[ignore]`** (+8: the wire's `timings`
beside `usage` and their absence; the client's `prefill` from them and
`None` without; `launched_batch` and the rule over the CPU build's figure,
the knee, a GPU's second, a short sample, a typed 1024; the note once per
managed session and again at `Ready`, nothing at the knee, the CPU auto,
a fast prefill, a cloud or no sample; the external wording; the whole
path from a scripted usage chunk to the note, and silence on the second
turn) — plus `slow_prefill_e2e_live`, the ninth, `#[ignore]`.

**Live.** `slow_prefill_e2e_live` (external mode, a forty-paragraph
seed, `MINDFORK_EXPECT_SLOW_PREFILL` saying which host it is):

- the **CPU build** (`llama-server` b10807, Gemma 3 4B Q8_0, `-ngl 0 -c
  2048`, no `-b` — the default 2048): the turn took 37.9 s, the reply
  *Lamp*, and the note came — **38 tokens/s, a hold of 54 s at the default
  batch of 2048**, the launch-line wording (`-b 256 -ub 256`). The batch
  track's own reading of this build ("a 1400-token prompt already takes
  38 s") is the same figure, now from the engine's clock.
- the **LAN stack** (Qwen 3.6 27B on the 4090, four slots over 16384): the
  turn took 4.4 s, the reply *lamp*, no note; with the regression trio
  (`stop_silent_task_e2e_live`, `silent_roll_e2e_live`,
  `background_subagent_e2e_live`) **4/4 in 49.8 s**.

## 7. Not in this track

- **The roll's timings** (F5b) — done: [roll-timings.md](roll-timings.md).
- **A figure on the settings screen** — the measured throughput beside
  the *Batch (-b)* field, once the field's hint has a place for a number.
- **Setting the batch for the user** (F4b).

## 8. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the note beside the batch rule), §6.3 (the hold, measured),
  §11.3 (the note in the feed).
- architecture §5 (`TokenUsage.prefill`, `TurnUsage.prefill`, the rule),
  §6 (the wire's `timings`).
- [cpu-batch.md](cpu-batch.md) §7 (done here).
- locales `en`/`ru`: `ui.notice.slow_prefill_managed`,
  `ui.notice.slow_prefill_external`.
- install.md §3 (the external server's line, the note that says so).
- CHANGELOG (Added), journal `engine.md`, CLAUDE.md's status line and
  count.
