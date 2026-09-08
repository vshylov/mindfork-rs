# The batch a cancel waits for — `-b`/`-ub` on the CPU build's launch line

> **Status:** implemented (2026-09-07) — stage 0 measured in §3.1, stage 1's
> live runs in §3.2; every fork at its recommendation (the user's decision,
> 2026-09-07), F5 restated in §4.4 once the stand's shape was checked.
> The item the silent-preemption track recorded and did not take
> ([silent-preemption.md](silent-preemption.md) §8): a displaced stream holds
> its slot until the server's current batch runs out, and on the CPU build
> that batch is twenty seconds. The launch line is the app's
> (`shared/api/managed.rs::build_args`); what has to be decided is whether it
> should say `-b`, when, and at what cost to prompt processing.

## 1. Why, precisely

The preemption track measured its own ceiling (silent-preemption §3.2):
llama.cpp honours a cancel **between batches**. A stream cancelled while it
decodes is released within a token; a stream cancelled during its
**prefill** keeps its slot until `update_slots` has run its current batch —
up to `n_batch` prompt tokens, 2048 by default — to the end. On the CPU
build (Gemma 3 4B Q8_0, four unified slots over 2048) the compaction roll,
displaced 1.1 s into its prefill, held its slot for 23 more seconds and the
user's turn launched the instant it was released; on the GPU stack the same
batch is about a second. The stop track ([stop-silent-task.md](stop-silent-task.md))
inherited the same ceiling for a stop: the token fires at once, the slot
frees when the batch does.

The batch is a launch-line number. `-b` (`--batch-size`) is the *logical*
batch — how many tokens one `llama_decode` call is handed; `-ub`
(`--ubatch-size`) the *physical* micro-batch it is split into for the
matmuls, clamped to `-b`. The app already sets both for the embedding server
(to the context size, since an embedding must fit one physical batch —
install.md §3) and neither for the chat server. A smaller `-b` would shorten
the batch a cancel waits for in proportion; the price is prompt-processing
throughput, since a matmul over 128 tokens is less efficient than one over
2048 — how much less, on a CPU, is what §3 measures, and it decides whether
this is a default, a setting, or nothing.

**Requirements.**

- **R1.** Measured before designed (lessons §3): on the CPU build, for each
  batch setting, the cold prefill of a seeded prompt (the throughput price)
  and the wait a displaced roll's batch imposes on the user's turn (the
  gain), both read off the app's own smokes and the server's log, on a fresh
  server per arm.
- **R2.** Whatever the launch line gains, a GPU host at the defaults keeps
  its line byte for byte: the change is for the case that measured slow.
- **R3.** The knob, if there is one, sits where the other launch flags sit —
  the managed engine's *Performance* group (`-ngl`, FlashAttn, `--no-mmap`)
  — with a hint that says what it trades, and an empty value that means the
  server's own default.
- **R4.** Nothing changes for the embedding server (its batch is the context
  size for a different reason) or for an external server (its line is not
  the app's).

## 2. What exists (inventory)

### 2.1 The launch line

`ManagedConfig` (`shared/api/managed.rs`) → `build_args`: `--host`,
`--port`, `-ngl`, `-c`, then `-np N --kv-unified` above one session, `-m`,
`--mmproj`, `--jinja`, `--reasoning-format`, `--embeddings` (and with it
`-ub <ctx> -b <ctx>`), `--no-mmap`, `--flash-attn`, the speculative
family, then `extra_args` (always empty from the supervisor). The settings
behind it are `ManagedSettings` (`shared/config.rs`): `gpu_layers`
(`DEFAULT_GPU_LAYERS`), `context_size`, `sessions`, `jinja`, `no_mmap`,
`flash_attn`, the draft fields — each `#[serde(default)]`-tolerant, so a
new field loads an old `settings.json` without a migration (the `--no-mmap`
entry of the engine journal is the precedent, field and hint together).
`app/supervisor.rs::managed_config` copies the settings into the config;
a change to any of them restarts the server.

### 2.2 The settings screen

`screens/settings/helpers.rs` builds the managed section's groups: *Server*
(binary, host, port), *Model* (GGUF, projector, context, `--jinja`),
*Performance* (`-ngl`, FlashAttn, `--no-mmap`), *Speculative*. A numeric
field is `num_field(id, label, value)` with an `int` handler in `spec.rs`;
an optional one (`draft_gpu_layers`) parses through `parse_opt_num` — empty
means *not passed*. A field's hint is `.describe(loc.t(key))`, shown at the
section's foot while the field is focused; the demo dumps pin the hint
panel's height to the longest hint (`ui.settings.desc.sessions` is the
ceiling, ~700 characters). The impersonation engine has the same section
with its own ids (`Ix*`).

### 2.3 What the server does with the numbers

`n_batch` bounds the tokens one `update_slots` pass hands `llama_decode`;
`n_ubatch` is the slice of that the backend computes at once, clamped to
`n_batch` (the log says `n_batch = N, n_ubatch = min(N, 512)`). The queue
of tasks — a cancel among them — is served between passes. So the batch a
cancel waits for is `n_batch`, not `n_ubatch`: §3's arm with `-b 2048
-ub 128` is the test of that reading. On KV pressure the server halves
`n_batch` on the fly (`failed to find free space in the KV cache, retrying
with smaller batch size`, seen in silent-preemption §3.2), so a smaller
batch also means fewer retried tokens when the pool is tight.

### 2.4 The instruments

`preemption_smoke` (`tests/live.rs`) over the hybrid `ScriptedParent`: a
scripted seed of 55 % of the pool, then one of three arms — the one-word
turn alone (`preemption_cold_e2e_live`, this track's addition: the cold
prefill reference), the roll displaced by the turn
(`preemption_wait_e2e_live`), a turn cancelled while decoding
(`preemption_floor_e2e_live`). The server's log stamps every slot's
`launch_slot_`, `cancel task` and `release`, so the batch a cancel waited
for is the gap between the cancel and the release. The CPU build is the
free gate (install.md §7.3), launched by hand with the launcher's own line
plus the setting under test.

## 3. Measurements to make (before designing further)

Five launch lines on the CPU build, each on a **fresh server per arm** (a
warm slot would hide the prefill):

| setting | `-b` | `-ub` | what it tests |
|---|---:|---:|---|
| default | 2048 | 512 | the line as it is |
| b512 | 512 | 512 | one physical batch per pass |
| b256 | 256 | 256 | a quarter of the default pass |
| b128 | 128 | 128 | the smallest worth a matmul |
| b2048-ub128 | 2048 | 128 | `-ub` alone — §2.3's reading |

Two arms each: the cold prefill of the seeded turn (`preemption_cold`, the
price) and the roll displaced during its prefill (`preemption_wait`, the
gain), reading the turn's first token from the smoke and the roll's
launch → cancel → release and the turn's launch off the server's log.

**GO** if a setting cuts the cancel-to-release gap by more than it costs the
cold prefill — the turn's first token in the wait arm falls, net of the
prefill's own change — and the prefill's cost is one a user would accept
for the latency (a fifth, say, not a half). **NO-GO** if every setting costs
about what it gains, or if `-ub` alone turns out to be the knob (then the
design is a different line); then the roadmap keeps the item as "the batch
is the server's".

### 3.1 Stage 0 — measured (2026-09-07)

**GO — and the knob is `-b`, not `-ub`.** Ten launches of the CPU build
(`llama-server` b10807, Gemma 3 4B Q8_0, `--host --port -ngl 0 -c 2048 -m …
--jinja`, no `-np`: four unified slots), one fresh server per arm, the seed
37 paragraphs of archive (55 % of the pool) plus a four-paragraph tail. The
turn's prompt is ~1400 tokens, the roll's ~890.

| setting (`-b`/`-ub`) | the turn alone: first token | the roll's batch: cancel → release | the turn behind the roll: first token | the roll, both attempts |
|---|---:|---:|---:|---:|
| default (2048/512) | 37.9 s | **23.3 s** | 63.7 s | 113.1 s |
| 512/512 | 39.7 s (+5 %) | **13.1 s** | 54.1 s | 106.8 s |
| 256/256 | 43.2 s (+14 %) | **6.5 s** | 50.4 s | 102.7 s |
| 128/128 | 45.6 s (+20 %) | **2.8 s** | 48.8 s | 104.2 s |
| 2048/128 | 45.8 s (+21 %) | **39.8 s** | 86.4 s | 142.4 s |

Read off the server's log, the roll was cancelled 1.0 s after its launch in
every arm (the turn's request half a second behind `/compact`, the client's
cancel at once), and released when its current batch had run out: the whole
888-token prompt at the default (23.3 s), one 512-token batch at `-b 512`
(13.1 s), one of 256 (6.5 s), one of 128 (2.8 s) — **linear in `-b`**, the
turn launched the instant the slot was released each time. `-b 2048 -ub
128` is the reading of §2.3 confirmed from the other side: the same
888-token batch, now computed in sixteen slices of 128, took 39.8 s to run
out, and the cold prefill paid the same +21 % as `-b 128` for nothing — the
micro-batch sets the matmul's width, the batch sets when the server looks
at its queue.

The price is the prefill: the turn alone is +5 % at 512, +14 % at 256,
+20 % at 128 — every turn's prompt processing, on a host where a 1400-token
prompt already takes 38 s. The gain is the batch a cancel waits for — a
displaced stream's (silent-preemption §3.2), a stopped task's
(stop-silent-task §3.2) — and the turn behind a displaced roll sees the
difference net of its own slower prefill: 63.7 → 54.1 → 50.4 → 48.8 s. The
retry's cost is unchanged (the roll's second attempt 49–55 s in every arm,
placed cold by LRU as before). The knee is **256**: a quarter of the
default's wait for a seventh of the prefill; 128 buys 3.7 s more for
another 6 %, 512 keeps most of the wait for the least cost.

Against the criterion: a setting exists that cuts the gap far more than it
costs — at 256, 16.8 s off a 23.3 s wait for 5.3 s on a 38 s prefill — and
`-ub` alone is not the knob, so the design is the line §4 describes. What
stays with the server: the retry's cold prefill, and the halving of the
batch under KV pressure (`failed to find free space in the KV cache`, seen
again in the default and the 2048/128 arms only — the settings whose batch
is larger than the room left).

### 3.2 Stage 1 — the line, launched by the app (2026-09-07)

`managed_cpu_line_launches_with_the_measured_batch_live` (`managed.rs`):
the app's own `build_args` for `-ngl 0` with no batch typed —
`--host 127.0.0.1 --port 18124 -ngl 0 -c 2048 -b 256 -ub 256 -m … --jinja` —
launched through `ServerHandle::launch` against the CPU build: ready,
four unified slots over 2048 on `/props`. The numbers that line buys are
§3.1's 256 row, measured on the same flags by hand. The LAN regression
(`silent_roll_e2e_live`, `background_subagent_e2e_live`, external mode —
the GPU stack's line is not the app's) 2/2 in 55 s. The unit suite: 2908
green, 146 `#[ignore]`; the settings screen's demo dumps and screenshots
regenerated for the new row.

## 4. Design

### 4.1 One optional field: the batch

`ManagedSettings.batch_size: Option<u32>` (`#[serde(default)]`: an old
`settings.json` reads as `None`), copied into `ManagedConfig.batch_size`
by `managed_config`, read by `build_args` for the **chat** server only (the
embedding server keeps `-ub <ctx> -b <ctx>` for its own reason, R4).
`Some(n)` passes `-b n` and `-ub min(n, 512)` — the micro-batch named too,
so the server's log says what the app meant and the launch line reads
whole (llama.cpp would clamp it the same way). `None` means **auto**: the
line stays byte for byte what it is (R2) unless the engine is being run on
the CPU by the user's own setting — `gpu_layers == 0` — in which case the
app passes **`-b 256 -ub 256`**, the knee §3.1 measured. A user who wants the
server's default on a CPU host types `2048`; one who wants 128 types it.

### 4.2 Why `gpu_layers == 0` is the trigger

The app cannot know the binary is a CPU build, but it does know what the
user told the server to do with the GPU: `-ngl 0` runs every layer, and so
every prefill, on the CPU — whether because the build has no CUDA or
because the VRAM is spoken for — and that is the case the batch trades
well in. A partial offload (`-ngl 20`) or the default (`99`) prefills on
the GPU, where the default batch is about a second and the prefill's price
is real; nothing changes there. The rule is one comparison in
`build_args`, next to the `-np` one.

### 4.3 The field and its hint

*Performance* group, after `-ngl`: **Batch (-b)**, an optional number —
empty reads *auto* (the hint says what auto does), a number is passed as
is. The hint, kept under the demo dumps' ceiling: *"Prompt tokens the
server processes per pass (-b). Empty: auto — 256 on the CPU (GPU layers
0), otherwise the server's default (2048). A smaller batch shortens the
wait for a stopped or displaced stream (measured: 23 s → 7 s at 256 on a
CPU-only host) and slows prompt processing (+14 %). Restarts the server."*
The impersonation engine's managed section gets the same field (F4).

### 4.4 What the user sees

Nothing new unless they look: a CPU-only host's server log says
`n_batch = 256`, and a turn typed while a reflection or a roll prefills
starts in seconds rather than half a minute. install.md §3 gets the number
and the reasoning in the paragraph on CPU hosts. The docker stand (F5, as
decided and then checked): its two `llama-server`s are **containers the
app talks to as external servers** — the seeded settings are external
mode, and the launcher never runs there — so "left to auto" would have
meant *no batch at all*; the chat container's line in `compose.yaml`
carries `-b 256 -ub 256` itself, with the reason beside it, which is the
same decision made where the line actually is.

### 4.5 What does not change

The default line on a GPU host; the embedding server's batch; the external
server (its line is the user's — install.md says what to pass); the lane,
the preemption and the stop, which now wait for a smaller batch and
otherwise do what they did; the retry's cold prefill; the pool rule.

## 5. Difficult spots

- **`-ngl -1`.** llama.cpp's own default is now `-1` (auto: offload what
  fits); the app's is `99`. Neither is `0`, so neither triggers auto — a
  user who sets `-1` on a CPU build gets the server's default batch, as
  today, and can type `256`. The rule is the user's number, not the
  hardware.
- **The impersonation engine's managed server** shares the struct and
  therefore the field and the rule; its `-ngl` is its own, so a CPU-only
  impersonation server gets the same auto.
- **The hint's length.** The demo dumps pin the settings hint panel to the
  longest hint; the batch hint must stay under `ui.settings.desc.sessions`
  (~700 characters) or the dumps change and the screenshots have to be
  regenerated (docs/lessons.md §2).
- **A number the server refuses.** `-b 0` or a value below what a prompt
  needs is llama.cpp's to reject; the field takes any positive integer and
  the server's startup failure is reported as any launch failure is
  (spec §3.4). No validation beyond "a number".
- **`-ub` above `-b`.** Not produced: the app passes `-ub min(n, 512)`, and
  llama.cpp clamps the same way, so the log's `n_ubatch` always equals what
  the line says.
- **The KV-pressure halving.** A batch of 256 is at or below the room the
  server usually has, so the `failed to find free space … retrying with
  smaller batch size` line — seen in the default and 2048/128 arms only —
  should also go quiet on the CPU build; not measured on purpose, noted.

## 6. Forks

- **F1. What the knob is.** (a) **An optional field, empty = auto (256 on
  `-ngl 0`, the server's default otherwise)** *(recommended — the CPU host
  gets the measured knee without knowing the flag exists, the GPU host's
  line is untouched, and either can type a number)*. (b) The field only,
  no auto: install.md recommends 256 for CPU hosts. (c) Auto only, no
  field: the number is the app's and cannot be overridden.
- **F2. The auto value.** (a) **256** *(recommended — §3.1's knee: a
  quarter of the wait for a seventh of the prefill)*. (b) 512: +5 % for a
  wait of 13 s. (c) 128: +20 % for 2.8 s.
- **F3. `-ub` on the line.** (a) **Passed as `min(n, 512)` whenever `-b` is**
  *(recommended — the line says what the app means; the server would clamp
  it the same way)*. (b) `-b` only.
- **F4. The impersonation engine.** (a) **The same field and rule** *(the
  same struct, the same section)*. (b) The chat engine only.
- **F5. The docker stand.** (a) **Left to auto** *(recommended — its
  containers run `-ngl 0`)*. (b) Pinned in the seeded settings.
- **F6. Staging.** (a) **One PR** *(recommended — one field, one rule, one
  hint, `build_args`' tests)*. (b) Two.

## 7. Stage and the test plan

Stage 0 is §3's probe, on this branch with the design (the third arm,
`preemption_cold_e2e_live`, is its addition to `tests/live.rs`). Stage 1,
one PR:

- `shared/api/managed.rs`: `build_args` — `gpu_layers: 0` and no field →
  `-b 256 -ub 256`; `gpu_layers: 99` and no field → neither flag, the line
  byte for byte as before; `Some(1024)` → `-b 1024 -ub 512`; `Some(128)` →
  `-b 128 -ub 128`; the embedding server unchanged at `-ub <ctx> -b <ctx>`
  whatever the field says.
- `shared/config.rs`: an old `settings.json` without the field reads
  `None`; the field round-trips.
- `screens/settings`: the field parses a number, an emptied field reads
  `None`, the hint is present and under the ceiling; the demo dumps
  unchanged.
- `app/supervisor.rs`: `managed_config` carries the field; a change to it
  restarts the server (the existing restart-on-change test's pattern).
- Live: the CPU build launched **by the app's own line** — `MINDFORK_LLAMA_BIN`
  with `MINDFORK_NGL=0` — reporting `n_batch = 256` in its log, and the
  probe's wait arm on it at the 256 numbers of §3.1; the LAN regression
  (`silent_roll_e2e_live`, `background_subagent_e2e_live`) on the GPU stack,
  whose line must not have changed.

## 8. Not in this track (recorded so they are not re-derived)

- **A per-request batch** — llama.cpp has none; the batch is the server's.
- **The embedding server's batch** — the context size, for a reason of its
  own (a non-causal model's input must fit one physical batch).
- **An external server's line** — not the app's to set; install.md §3 can
  say what to pass by hand.
- **Detecting a CPU build at runtime** (the prefill's measured tokens per
  second) — **done** as its own track
  ([slow-prefill-detection.md](slow-prefill-detection.md), 2026-09-08): the
  engine's own `timings`, a note once per server session naming the hold
  and the change; 38 tok/s and a 54 s hold measured on this build at the
  default batch. The batch is a launch-time argument, so a runtime reading could
  only advise a restart.

## 9. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the launch line gains `-b`/`-ub` on a CPU host, and why),
  §11.6 (the *Performance* group's new field and what empty means).
- architecture §5 or §7 wherever `build_args` is described (the batch rule
  beside the `-np` one), §3's module map line for `managed.rs`.
- install.md §3 (the CPU-host paragraph: the number, the trade, the log
  line to look for; what to pass to an external server by hand), §7.3 (the
  docker stand inherits auto).
- locales `en`/`ru`: the field's label and hint.
- journal engine.md (the launch line is the engine's) with §3.1's table;
  CHANGELOG `[Unreleased]` Changed (a CPU-only host's server now runs a
  smaller batch; a turn typed during the app's own request starts sooner)
  and Added (the field); roadmap; silent-preemption.md §8 and
  stop-silent-task.md §7 point here; CLAUDE.md's status line when the
  track ships.
