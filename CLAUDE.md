# CLAUDE.md — project guide for mindfork-rs

A console (TUI) AI chat application in Rust. Local models **Gemma 3/4** and
**Qwen 3.5/3.6** via **llama.cpp `llama-server`** (an OpenAI-compatible server;
in external mode any such server works — vLLM/LM Studio/Ollama), plus the cloud
providers OpenAI, Gemini and Anthropic. UI on **ratatui**. Platforms: Windows +
Linux. Architecture — **Feature-Sliced Design (FSD)**.

> **Engine:** originally designed around `xinfer`, which turned out too raw
> (incoherent output on Gemma 4, builds poorly on Windows) — switched to
> llama.cpp. Generic-OpenAI client (`OpenAiClient`), managed launcher
> `LlamaSupervisor`; the clouds are separate `EngineBackend` implementations.
> See [docs/install.md](docs/install.md) §3 and
> [ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md).

## How to use this file

**This file is a router, not a corpus.** It is loaded in full at the start of
every session, so it stays small on purpose: orientation, the standing
decisions, and a map to everything else. The map below is the important part —
follow it to the two or three documents your task actually needs.

Two rules that follow from that:

- **Read the section, not the file.** [spec.md](spec.md) (217 KB) and
  [docs/architecture.md](docs/architecture.md) (144 KB) are chaptered reference
  documents with stable numbering; every section is cited by number from the map
  and from the code. Loading either whole spends context on twelve chapters to
  use one.
- **The engineering journal is per area.** What was done, why, what was measured
  and what was rejected lives in `docs/journal/<area>.md` — 243 entries, split by
  subsystem. Read the file for the area you are touching; grep across them when
  hunting a specific past decision.

## Document map — what to read, and when

| When you are… | Read |
|---|---|
| **starting any task** | [AGENTS.md](AGENTS.md) — the mandatory workflow (design doc → branch → live run → docs → PR) |
| **about to implement anything** | [docs/lessons.md](docs/lessons.md) — the traps that recur across areas; short, and it will save you a repeat |
| working on the **engine**, a provider, generation, sampling, compaction | architecture §5–§6 · spec §3, §6–§8 · [docs/journal/engine.md](docs/journal/engine.md) |
| working on **storage**, config, backup, migrations, secrets | architecture §7 · spec §5, §12 · [docs/journal/storage.md](docs/journal/storage.md) |
| working on a **tool**, MCP, the Python sandbox | architecture §8 · spec §9 · [docs/journal/tools.md](docs/journal/tools.md) |
| working on the **self-model** — summary, goals, traits, reflection | architecture §9 · spec §17 · [docs/journal/self-model.md](docs/journal/self-model.md) |
| working on **notes** and their graph | architecture §9 · spec §9.5 · [docs/journal/notes.md](docs/journal/notes.md) |
| working on **RAG**, chat attachments or embeddings | architecture §9 · spec §9.5, §9.7 · [docs/journal/rag.md](docs/journal/rag.md) |
| working on the **feed**, markdown, rendering, the terminal | architecture §10 · spec §11.3–§11.4 · [docs/journal/ui-feed.md](docs/journal/ui-feed.md) |
| working on the **input box**, keys, spellcheck | architecture §10 · spec §11.5 · [docs/journal/ui-input.md](docs/journal/ui-input.md) |
| working on a **screen** — settings, chat list, search, help | architecture §10 · spec §11.6–§11.8 · [docs/journal/ui-screens.md](docs/journal/ui-screens.md) |
| working on **localization** | [docs/journal/i18n.md](docs/journal/i18n.md) · the i18n plans in `docs/history/` |
| working on **packaging, installers, releases, branding** | architecture §12 · AGENTS.md §6 · [docs/journal/release.md](docs/journal/release.md) |
| working on **CI workflows**, jobs, caches, the live-test gate | architecture §12 · AGENTS.md §6 · [docs/journal/ci.md](docs/journal/ci.md) |
| working on the **website** — mindfork.io, `site/` | [docs/research/mindfork-io-website.md](docs/research/mindfork-io-website.md) · [docs/journal/website.md](docs/journal/website.md) |
| working on **static analysis or a repository gate** | AGENTS.md §4, §6 · [docs/journal/quality.md](docs/journal/quality.md) |
| **moving code without changing behaviour** | architecture §3 · [docs/journal/refactors.md](docs/journal/refactors.md) |
| asking "how did it get this way" about M3–M9 | [docs/journal/milestones.md](docs/journal/milestones.md) · [docs/history/plan.md](docs/history/plan.md) |
| adopting an **architectural decision** | [docs/decisions/](docs/decisions/) — write a new ADR (list below) |
| **planning a track** | AGENTS.md §1 · prior art in [docs/research/](docs/research/) (pre-decision) and [docs/history/](docs/history/) (finished plans) |
| working on the **code workspace** — the project attached to a chat | architecture §8, §10 · spec §9.12 · [docs/journal/tools.md](docs/journal/tools.md) · the finished plan [docs/history/code-workspace.md](docs/history/code-workspace.md) |
| installing, running, configuring the engine | [docs/install.md](docs/install.md) |
| what is left to do | [docs/roadmap.md](docs/roadmap.md) |
| what the user sees as changed | [CHANGELOG.md](CHANGELOG.md) |

**ADRs** (`docs/decisions/`): 0001 UI crates for ratatui 0.30 · 0002 dedicated
embedding server · 0003 own markdown renderer · 0004 engine contract and
multi-provider inference · 0005 Python sandbox as a `wasmer`/WASIX sidecar ·
0006 data schema versioning and migrations · 0007 plugins — MCP tool host and a
neutral import format · 0008 API keys with machine-bound encryption · 0009
message speech (TTS) · 0010 the subagent as a nested turn · 0011 the
directed dialogue as a scripted multi-context run · 0012 concurrent tool calls
in a round.

## Key architectural decisions

- **Engine = an OpenAI-compatible HTTP server** (managed `llama-server` or any
  external one) behind the `EngineBackend` trait, with OpenAI Responses,
  Anthropic and native Gemini as sibling implementations (ADR 0004). Embedding
  as an rlib is deliberately NOT used (it drags in candle/CUDA).
- **The agentic loop is client-side** (in the orchestrator). Tools return a
  result + effects; the orchestrator (sole owner of `Chat`) applies them — no
  locks.
- **Sampling** (`SamplingConfig`, spec §8): the standard OpenAI fields plus the
  llama.cpp extensions `llama-server` accepts in the request body (dynatemp,
  min_p, top_n_sigma, typical_p, adaptive-p, repeat/DRY/XTC, mirostat, seed,
  sampler order). Every field is `Option` and is sent only when set; what a given
  provider actually accepts comes from the single source of truth
  `entities::sampling::supported_sampling_fields`.
- **EOS**: stopped by token-id on the server; the `stop` field is NOT sent
  (anti-self-cutoff).
- **"Thoughts" (CoT)**: `delta.reasoning_content` (llama.cpp), with per-provider
  equivalents; fallback — parsing `<think>` out of content.
- **Storage**: JSON (config/profiles/chats, atomic write + `.bak`) + SQLite
  (notes/RAG, sqlite-vec, isolation by `profile_id`), plus a disposable
  `cache.db` for the full-text index (delete it to rebuild — it is derived data).
- **Soft delete** everywhere (`is_hidden`, profile→chats cascade).
- **Unidirectional UI↔orchestrator flow**: `AppCommand` up, `AppEvent` down;
  `generation_id` drops stale chunks; state machine `Idle/Generating/Cancelling`.

## Structure (FSD, dependencies strictly downward)

`app → screens → widgets → features → entities → shared`. A binary crate.

- `src/app/` — orchestrator, events, TUI loop, tokio↔UI bridge.
- `src/entities/` — domain types (`chat`, `message`, `profile`, `note`, `rag`,
  `sampling`, `self_model`, `attachment`).
- `src/shared/api/` — the engine behind `EngineBackend` (`openai`, `anthropic`,
  `gemini`, `managed`, thoughts parser, mock).
- `src/shared/storage/` — `json` + `db` (SQLite) + the `Storage` facade.
- `src/shared/` — `config`, `paths` (portable, next to the binary), `i18n`,
  `markdown`, `logging`, `secrets`, `sandbox`, `mcp`, `error`.
- `src/{screens,widgets,features}/` — UI screens, reusable widgets, and the
  feature logic (tools, spellcheck, backup, compaction, search).

Full module map with invariants — architecture §3.

## Conventions

- Task workflow (design doc → branch → development → live run → docs → PR) —
  **[AGENTS.md](AGENTS.md)**; below are code conventions.
- Rust edition 2024. **Before every commit**: `cargo fmt`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test` — all green.
- Errors: `anyhow` in application layers, `thiserror` in library modules under
  `shared`.
- Logs — file only (`logs/`, stdout is taken by the TUI). No `println!`.
- Tests are written next to the code as it is built (`#[cfg(test)]`); anything
  needing a real server/model — `#[ignore]`, and run for real before the PR.
- **Source language is English** — comments, docs and commit messages; guarded
  in CI by `tools/cyrillic_scan.py`. User-facing text stays localizable
  (`locales/*.json`), and `ru` is a fully supported locale.
- Comments and docs reference spec/architecture sections by number.
- End commits with a trailer naming the **actual model** that wrote the code
  (AGENTS.md §5), e.g.:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

## Commands

```
cargo build
cargo test                         # unit tests (no server needed)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run                          # TUI (needs a REAL terminal — see Pitfalls)
python tools/cyrillic_scan.py      # source-language gate
python tools/link_check.py         # relative-link gate
python tools/doc_index_check.py    # documentation-structure gate
python tools/list_scroll_check.py  # one implementation of a list's scroll state
python tools/wizard_rtf.py         # regenerate the Windows installer's disclaimer page
python tools/wizard_rtf.py --check # ...and the gate that it matches DISCLAIMER.md
python tools/site_legal_pages.py    # regenerate the site's privacy page from PRIVACY.md
python tools/site_legal_pages.py --check  # ...and its gate
```

Running against a real server (smoke test, llama.cpp):

```
llama-server -m gemma-4-it.gguf   --host 0.0.0.0 --port 8000 -ngl 99 -c 16384 --jinja        # chat
llama-server -m bge-m3-Q8_0.gguf  --host 0.0.0.0 --port 8001 -ngl 99 -c 16384 --embeddings   # embedder
$env:MINDFORK_ENGINE_URL="http://127.0.0.1:8000/v1"   # PowerShell; URL includes /v1
$env:MINDFORK_EMBED_URL="http://127.0.0.1:8001/v1"    # memory/RAG paths
cargo run
# All live smokes (silently skipped without the needed env var):
cargo test -- --ignored --nocapture --test-threads=1
# Memory/self-model/notes only:  cargo test e2e_live -- --ignored --nocapture --test-threads=1
```

Backend selection: `MINDFORK_ENGINE_URL` (external, any OpenAI server) OR
`MINDFORK_LLAMA_BIN` (+ `MINDFORK_MODEL` GGUF, `MINDFORK_NGL`, `MINDFORK_CTX`,
`MINDFORK_PORT`) for a managed `llama-server`. The whole live gate can also be
rented instead of hosted — `python tools/e2e_hf.py run`, see
`docs/history/remote-e2e-hf.md`.

## Status (2026-09-09, version 0.9.8)

The **M0–M9** plan is done, plus extensive post-M9 work — **2973 unit tests
green, 150 `#[ignore]`** (live smokes + a real-clipboard round trip + the
screenshot-dump regenerator).

**This list is pointers, not summaries.** One line per track, newest first: what
it is, and where the reasoning lives. What was decided, what was measured and
what was rejected belongs in the linked document and is not repeated here — a
paragraph per track is how this section grew to 18 KB inside a file that is
loaded in full at the start of every session and is capped at 30 000 bytes by
`tools/doc_index_check.py`. A new track adds a line; a line that has stopped
being recent is dropped, not shortened.

- **The title's and impersonation's usage for the budget — and the
  ratio they would erase** — the item was "let the two record their usage
  too"; measured, they are the app's most over-counting requests (0.65,
  0.59) and a turn carrying 25 KB of a tool result's JSON under-counts by
  a third (1.34) — so under the budget's one rule, *the latest wins,
  floored at 1.0*, the next title or roll after such a turn stored 1.0
  and the following turn was priced a quarter under. Now `SessionBudget`
  keeps one ratio per kind of request (`Shape::{Turn, Run, Loop, Roll,
  Title, Impersonation, Summary}`), `price`/`record_usage`/`density`
  take the kind, and a turn's correction is erased by nothing but a turn;
  the title and impersonation record theirs at their usage chunk, a
  child run's rounds are their own kind. Measured after: impersonation
  over the JSON at 1.61 in its own slot, the prose turns at 0.9
  ([docs/research/title-impersonation-usage.md](docs/research/title-impersonation-usage.md),
  spec §6.3, [docs/journal/engine.md](docs/journal/engine.md)).
- **The roll's usage for the budget — and the estimate it would
  calibrate** — the item was "let the roll record its usage as the loops
  do"; measured first, it turned on its precondition: the prompt estimate
  never counted the tool schemas (24 of them, 18 116 bytes, about 4270
  tokens — a fresh chat's first request estimated 85 against 4358 exact),
  so the ratio the budget "calibrates" with was that overhead in disguise
  (51 falling to 6 across one conversation), the first request of a
  session was priced at a fiftieth of its size, and a request without
  schemas — the roll — nine times over; its own ratio recorded would have
  priced the next turn six times under. Now `estimate_prompt_tokens`
  counts the schemas as the wire sends them (`openai::tools_json`, one
  rendering behind both), and the roll records its usage in its task
  beside the estimate its reservation was priced from. Measured after:
  the turn's estimate a tenth over the exact, the roll's price 3109 for
  8722 ([docs/research/roll-usage-calibration.md](docs/research/roll-usage-calibration.md),
  spec §6.3, §11.1, [docs/journal/engine.md](docs/journal/engine.md)).
- **The loops' timings — the silent tasks' cold prompts, sampled at the
  landing** — the note's sample came from the turn or the roll, and the
  warm server's ordinary day has neither: a conversation under the
  compaction threshold, every turn processing tens of tokens. The three
  silent loops send their own prefix — instructions, tool schemas, a
  digest — so their first round is processed cold and whole, the largest
  prompt a session makes (measured: a reflection's first round 2798
  tokens against the roll's 1466); its usage was read for the budget and
  dropped. Now `run_rounds` keeps the loop's largest sample on an
  out-parameter, every silent task lands as `BgDone { kind, outcome,
  prefill }`, and `handle_bg_done` offers the sample once for every kind
  after the task's own landing — the roll's own offer folded in. Measured
  in two phases on the CPU build: the warm turn 52 tokens, under the
  floor; the reflection's first round the note
  ([docs/research/loop-timings.md](docs/research/loop-timings.md),
  spec §3.4, §17.6, [docs/journal/engine.md](docs/journal/engine.md)).
- **The roll's timings — the session's coldest prompt, sampled** — the
  slow-prefill note read the turn's rounds only, and a server that kept
  running gives no turn sample: the chat's prefix stays in a slot's cache
  and every turn processes tens of tokens, under the rule's floor
  (measured on the LAN stack: 31 against 1430 cold). The compaction roll
  sends a different prefix — the summarizer's system, a digest that never
  repeats — so it is processed cold (1466 tokens), the largest prompt a
  session makes and the very stream a turn displaces; its usage was
  matched and dropped. Now `collect_roll` keeps the `Usage` chunk's
  figure, `CompactResult.prefill` carries it, and `handle_compact_result`
  offers it to the one rule after the roll's own landing. Measured: the
  CPU build's roll as the session's first request — 37 tok/s, the note;
  the 4090, none
  ([docs/research/roll-timings.md](docs/research/roll-timings.md),
  spec §3.4, §6.7, [docs/journal/engine.md](docs/journal/engine.md)).
- **A slow prefill, detected on the fly — the batch a cancel waits for,
  told to the user** — the batch track's automatic `-b 256` covers a
  managed server with no GPU layers and nothing else; every other slow
  prefill held its slot for seconds on a cancel and said nothing. The
  signal was on the wire: `llama-server`'s stream ends with `timings` —
  `prompt_n` over `prompt_ms`, net of the prefix cache — which the client
  now carries as `TokenUsage.prefill`; the turn keeps its largest sample,
  and at the landing a pure rule (`prefill_hold`: the hold `batch / tps`
  above 5 s on a sample of 256+ tokens with the batch above 256, the batch
  the launch line's or 2048 assumed for an external server) sends **one
  feed note per server session** naming the throughput, the hold and the
  change — the *Batch (-b)* field, or `-b 256 -ub 256` on the launch line.
  Measured: the CPU build at the default batch, 38 tok/s and a 54 s hold,
  the note; the 4090, no note
  ([docs/research/slow-prefill-detection.md](docs/research/slow-prefill-detection.md),
  spec §3.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **The quit's settle hears the roll, and its cap is a setting** — the
  roll takes a silent slot but lands on `compact_rx`, so the settle of
  the previous track waited its whole cap during an automatic roll and
  dropped a roll that had just finished; now `settle_silent_tasks`
  selects over both channels — a cancelled roll clears its slot at once,
  a finished one is applied through `handle_compact_result` for the flush
  — and the cap is the user's: `tools.quit_settle_secs`, a *Tools* row
  beside the other time limits (the user's decision), **no cap by
  default** so a task caught mid-tool finishes (each task is bounded by
  its own run time limit), a number in seconds, `0` deciding at once by
  state ([docs/research/quit-settle-roll-and-cap.md](docs/research/quit-settle-roll-and-cap.md),
  spec §6.7, §17.6, [docs/journal/engine.md](docs/journal/engine.md)).
- **The quit waits for the landing — a stop's own path decides, the state
  rule only past a cap** — at a quit a silent task caught mid-tools kept
  its window because its round's write, if any, had not reported; but a
  cancelled loop lands on its own, and fast — its lane wait returns at
  once, its stream ends at the next chunk, its tools finish — on the very
  channel `run` polls. Now the `Quit` arm only cancels (`cancel_bg_all`),
  `run` listens on `bg_done_rx` a little longer (`settle_silent_tasks`,
  each landing through `handle_bg_done` — the stop's path, `consumed` from
  the loop — while any slot is active, 2 s in all), then
  `refund_unlanded` decides whatever has not landed by its state, then
  the flush; a token check before the stream keeps a cancelled loop from
  asking the engine once more where it has no session budget
  ([docs/research/quit-waits-for-the-landing.md](docs/research/quit-waits-for-the-landing.md),
  spec §17.6, §11.10, [docs/journal/engine.md](docs/journal/engine.md)).
- **"Acted on" by effect — a silent task's window is consumed by a write,
  not by a round** — the refund rule's criterion had been "a round of the
  task's tools was about to run", coarse for a reflection whose first
  round only reads its self-model and is stopped seconds after it starts.
  Now the fact is the writer's own: `ToolOutcome.wrote`, set on the
  success path of each of the nine memory writers (a refusal reports
  nothing, an `Err` counts as a write), ORed per round by the loop and
  reported at the landing as `Cancelled { wrote }`; because it arrives
  after the tools, the slot's flag became a three-valued state
  (`Acting::{Idle, InTools, Wrote}`) and a quit refunds `Idle` only.
  `Tool::concurrent()` could not be the criterion: `note_recall` is
  unmarked for a cache write, `note_neighbors` unmarked at all
  ([docs/research/acted-by-effect.md](docs/research/acted-by-effect.md),
  spec §9.2, §17.6, [docs/journal/engine.md](docs/journal/engine.md)).
- **A quit gives the window back too — the fact the loop keeps in the
  open** — the refund track's fork F6b: a quit cancelled every silent
  task and returned, nothing landed, and the spawn-time advance stayed, so
  a restart mid-reflection skipped the window. The missing piece was a
  fact, not a decision: "a round of my tools is about to run" was read
  only from the outcome. Now the loop stores it on a flag the spawn tail
  keeps beside the window (`Refund { window, acted }`, `SilentLoop.acted`)
  at the very line it counts a round, and `quit_bg` cancels every token
  and then gives back every window whose flag is unset, before the exit
  flush; a token check right after the store means a cancelled loop
  starts no tools into a window already given back
  ([docs/research/quit-refunds-window.md](docs/research/quit-refunds-window.md),
  spec §17.6, §11.10, [docs/journal/engine.md](docs/journal/engine.md)).
- **A stop gives the window back — the silent task returns at the next
  landing** — the stop track's fork F4b, taken on its premise's other
  reading: a stop postpones. Reflection advances its watermark and stamp
  at spawn and the two consolidations reset their counters there, so a
  stopped task skipped the window it was about to read; now the spawn
  records what it advanced on the task's slot (`BgSlot.window`), the loop
  reports whether a round of its tools ran before the stop
  (`RoundsEnd::Cancelled { rounds }` → `BgOutcome::Cancelled { consumed }`),
  and the landing puts the window back only when it had not been acted on
  — a read window written twice is the defect the spawn-time rule exists
  to prevent — the watermark restored and saved, a counter added back;
  the ordinary cadence then spawns the task again. The roll, `Quit` and
  the preemption retry are untouched
  ([docs/research/stop-refunds-window.md](docs/research/stop-refunds-window.md),
  spec §17.6, §11.10, [docs/journal/engine.md](docs/journal/engine.md)).
- **`/tasks stop all` — every running silent task, in one word** — the
  fifth word the previous track recorded as cheap to add when asked for.
  `all` is a modifier checked before the kind table, the chat screen
  collects the running kinds as bare `stop` already did, one note names
  them with the tasks screen's words, and one intent carries them —
  `ChatIntent::StopBackgroundTasks { kinds }`, which the runtime fans out
  into the very `StopBackgroundTask` command `F6` sends, once per kind, so
  the orchestrator is untouched (the alternative, a new command onto
  `Quit`'s `cancel_all_bg`, was a second verb for one act); the two notes
  that list the kinds now name `all` as the route for the whole set
  ([docs/research/tasks-stop-all.md](docs/research/tasks-stop-all.md),
  spec §11.7, §11.10, [docs/journal/ui-screens.md](docs/journal/ui-screens.md)).
- **`/tasks stop <kind>` — the typed route to stopping a silent task** —
  `F6` on a task row of the tasks screen was the one action whose only
  route was a function key on a screen, against spec §11.7's rule that
  every action has a typed route. The reading that deferred the command
  ("localized kind names and a parser") turned out light: the screen already
  names the four tasks in both locales, `Arity::Subcommand` already carries
  a token behind its word, and the chat screen already keeps the four
  running flags for the status bar. Now `/tasks [stop <kind>]` on the
  existing registry row — the words `reflection · notes · self · compact`,
  bare `stop` mirroring `/subagents stop` — ends in the very
  `StopBackgroundTask` the key sends; the note names the task with the
  screen's words through `BackgroundKind::label_key`, the one function both
  surfaces read ([docs/research/tasks-stop-command.md](docs/research/tasks-stop-command.md),
  spec §11.7, §11.10, [docs/journal/ui-screens.md](docs/journal/ui-screens.md)).
- **The batch a cancel waits for — `-b` on the CPU build's launch line** —
  llama.cpp looks at its queue between batches of `-b` prompt tokens, so a
  stream the app cancels during its prefill (a displaced roll, a stopped
  task) holds its slot for one: 23 s at the default on the CPU build.
  Measured on five lines, a fresh server per arm: the wait is **linear in
  `-b`** (13.1 / 6.5 / 2.8 s at 512 / 256 / 128 for +5 / +14 / +20 % on the
  prefill) and `-ub` alone makes both worse. Now `-ngl 0` launches with
  `-b 256 -ub 256` unless *Batch (-b)* names a number; a GPU host's line is
  byte for byte what it was; the docker stand's chat container carries the
  flags itself ([docs/research/cpu-batch.md](docs/research/cpu-batch.md),
  spec §3.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **Stopping a silent task from the tasks screen** — `F6` on a task row
  that reads *running* or *waiting* stops it: the slot's token is cancelled
  and the task lands as a **third outcome**, `BgOutcome::Cancelled`, which
  clears the slot and touches neither the failure streak nor the spawn-time
  watermark and counters (the window is skipped, as on a failure); a
  `/compact` the user typed answers with one notice, an automatic roll stops
  quietly and is planned again at the next landing. What was missing was
  the reading, not the mechanism: the loops landed a cancelled task as `Ok`
  and the roll as a timeout, since only `Quit` could reach those paths
  ([docs/research/stop-silent-task.md](docs/research/stop-silent-task.md),
  spec §11.10, [docs/journal/ui-screens.md](docs/journal/ui-screens.md)).
- **The silent stream yields to the turn — preemption on the budget** —
  an interactive stream (a turn's round, a run's, a dialogue's line) that
  does not fit beside the app's own open request no longer waits for it:
  a silent reservation carries a **child token** the holder streams on, an
  interactive waiter that would then fit cancels it and is admitted ahead
  of the task's retry, and the task makes the same request again inside
  its own spawn — at most three times, then it holds — so the spawn-time
  watermark and counters stay honest; impersonation and the in-loop
  summary never yield; the loops' timeout runs over their streaming, not
  their waits. Measured first on the CPU build: a one-word turn waited
  45.9 s behind the roll where a cancelled stream's room is free in
  0.79 s; after, the turn streams before the roll ends — but a cancel is
  honoured between the server's batches, so the wait fell to the roll's
  prefill batch (24 s on the CPU build, under two on the GPU), and the
  roll pays its prefill twice
  ([docs/research/silent-preemption.md](docs/research/silent-preemption.md),
  spec §6.3, [docs/journal/engine.md](docs/journal/engine.md)).
- **The silent tasks under the app-wide budget — the budget's silent lane** —
  the app's own requests (the automatic title, reflection, the two
  consolidations, the compaction roll, impersonation on the shared engine,
  a summary inside a silent loop) stream on a **second permit** of the one
  `SessionBudget`, over the same pool sum: one at a time among themselves,
  never overfilling the pool beside a turn or a run, a turn that does not
  fit beside an open silent round waiting for that one round. The pool is
  known at one session too: a `llama-server` launched without `-np` runs
  four unified slots, and the roll — sized by the conversation, fired when
  it is largest — landed on them beside the turn; measured before designing
  on the CPU build (both ended, "Context size has been exceeded"), then the
  same arms green after the lane. `handle_done` asks for the roll ahead of
  the loops; the tasks screen's third state is *waiting*
  ([docs/research/silent-tasks-budget.md](docs/research/silent-tasks-budget.md),
  spec §6.3, [docs/journal/engine.md](docs/journal/engine.md)).


For what exists and how it works, read architecture.md and spec.md — they are
the source of truth for the current state. For how any of it came to be, and
what was measured and rejected on the way, read the journal file for that area
(the map above). For what is still open, read `docs/roadmap.md`.

## Pitfalls

- **The TUI "hangs" on a headless launch** (no TTY) — this is expected; clean
  exit via `Esc`/`Ctrl+Q` is covered by unit tests. Live verification needs a
  real terminal.
- Pointwise `#[allow(dead_code)]` (with a comment) marks deliberate
  ahead-of-consumer API; there is no crate-wide allow, and adding one is not the
  fix.
- The app's data is portable: it lives next to the binary (in dev —
  `target/debug/data/`), unless `defaults.json` says otherwise.
- **Spellcheck dictionaries** (`dictionaries/*.aff` + `*.dic`) are committed to
  the repo; `build.rs` copies them into `target/<profile>/data/dictionaries/`, and
  the release workflow packs them into the archive.
- More traps — and every one that has already cost this project time —
  [docs/lessons.md](docs/lessons.md).
