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

## Status (2026-09-06, version 0.9.8)

The **M0–M9** plan is done, plus extensive post-M9 work — **2871 unit tests
green, 139 `#[ignore]`** (live smokes + a real-clipboard round trip + the
screenshot-dump regenerator).

**This list is pointers, not summaries.** One line per track, newest first: what
it is, and where the reasoning lives. What was decided, what was measured and
what was rejected belongs in the linked document and is not repeated here — a
paragraph per track is how this section grew to 18 KB inside a file that is
loaded in full at the start of every session and is capped at 30 000 bytes by
`tools/doc_index_check.py`. A new track adds a line; a line that has stopped
being recent is dropped, not shortened.

- **The tasks screen — everything the app is doing, on one surface** —
  `F7`/`/tasks`: every sub-agent and dialogue run of every chat, running
  first with its **position** (round · tool, line, director) and elapsed
  time, then landed with its outcome (50, the rest counted), then the four
  silent tasks as running/idle; `Enter` opens the transcript, `P` the
  parent, `F6` stops a running background run through the command
  `/subagents stop` sends, `Esc` from a chat opened there returns to the
  list. A projection and nothing else: `AppEvent::TaskList` built off the
  seats, the turn's children and `Chat::children()`, sent on request and
  unasked on every change — which is why the screen opens on the key and
  an event may only refresh it; the position reaches the orchestrator as a
  new `TurnProgress::ChildProgress` step stored on the run's mirror; the
  bar's count and the running rows share one predicate
  ([docs/research/tasks-screen.md](docs/research/tasks-screen.md), spec
  §11.10, [docs/journal/ui-screens.md](docs/journal/ui-screens.md)).
- **Background dialogues — a directed scene that outlives its turn** —
  `start_dialogue`, the scene's twin of `start_subagent` behind the same
  switch: the call answers with the transcript's address and the director's
  closing result arrives later as a task notification. Small because the
  background machinery was already keyed by run id; what it added is a
  two-variant `RunSpec`, a `DialogueSpec` whose director inputs are
  **snapshotted at the call**, and one branch in the spawn into the same
  driver the foreground scene uses — a driver the preceding refactor lifted
  off `TurnLoop` onto an explicit context. Two measured fixes rode along: the
  dialogue had been streaming **outside the session budget** (so a foreground
  scene could already overlap a background run), and the turn the app starts
  on a notification could spend its whole cap thinking and land empty — now
  re-asked once with thinking muted, 2/2 recovered in the probe
  ([docs/research/background-dialogues.md](docs/research/background-dialogues.md),
  [ADR 0011](docs/decisions/0011-dialogue-directed-run.md) amended, spec
  §9.13, [docs/journal/tools.md](docs/journal/tools.md)).
- **Sub-agents in the background — a run that outlives its turn, both
  stages** — `start_subagent` (opt-in, `tools.subagent_background`): the
  call returns at once, the run is owned by the orchestrator over the
  turn's cloneable parts and the **app-wide** session budget, its record
  lands with the turn as a placeholder and is filled in **by id** when it
  ends, and the result is a stored **task notification** the model reads
  as user text — with a turn the app starts itself when the chat is open
  and idle. Measured before code on the four clouds and Qwen 3.6: an
  optional flag is silently omitted by Claude (0/3), a second tool is used
  3/3 and 5/5. No confirmations in a background run (the user's decision:
  the tool set is the control). Stage 2 answered the one surface the
  footer rule left open: a feed that streams under **no** turn
  (`LiveTurn.background`) — `F6` stops the run there and `Esc` merely goes
  back, both advertised only while it streams — and a result landing in a
  chat nobody is looking at marks that chat **unread** in the list
  (`Chat.unread`, additive, cleared by opening it)
  ([docs/research/background-subagents.md](docs/research/background-subagents.md),
  [ADR 0010](docs/decisions/0010-subagent-nested-turn.md) amended, spec
  §9.3.2, [docs/journal/tools.md](docs/journal/tools.md)).
- **Admission by budget — the unified KV pool never overfilled by the app** —
  above one session a managed server's streams share one KV pool, and when
  they outgrew it together the server ended *every* running conversation
  at once (reproduced on the CPU build, both routes: two prompts that do
  not fit, two replies that grow into each other); now every stream of a
  turn **reserves** its calibrated prompt estimate plus its reply cap in
  `SessionBudget` and waits for room, a stream alone always admitted, the
  pool from `pool.rs` (managed `-c`; an external llama.cpp's `n_ctx` above
  one slot; none on the clouds). The live control arm found the client
  swallowing llama.cpp's in-stream error as an empty chunk — fixed
  ([docs/research/admission-by-budget.md](docs/research/admission-by-budget.md),
  spec §6.3, [docs/journal/tools.md](docs/journal/tools.md)).
- **Concurrent ordinary tools — the round's read-only calls run at once** —
  the reads the model issues in one reply (measured unprompted, 26/26, on
  Gemma 4 31B and four clouds) run as a *segment*: a maximal run of
  consecutive calls their authors marked `Tool::concurrent()` (the file,
  project, attachment, chat, history and introspection readers, `fetch_url`;
  default `false`, never dangerous), at most the engine section's
  `concurrent_calls` at once — **1** on managed/external, the sequential round
  bit for bit, **4** on the clouds — with results, records and effects in the
  model's order, so no read moves across a write; the sub-agent group
  unchanged; `fetch_url`'s summary under `sessions`
  ([docs/research/concurrent-tools.md](docs/research/concurrent-tools.md),
  [ADR 0012](docs/decisions/0012-concurrent-tool-calls.md), spec §6.3,
  [docs/journal/tools.md](docs/journal/tools.md)).
- **Parallel sub-agents — track complete, both stages** — the model's
  several `call_subagent` calls in one reply run **at once**: the round's
  ordinary calls first, then the group (`buffer_unordered`, at most
  `tools.subagent_parallel` — default 1, the old sequential behaviour), then
  the records in the model's order; `TurnShared` borrowed immutably by every
  loop with the confirmation round trip behind one lock (one popup at a
  time) and the token totals as atomics; the mirror keyed by run id, the
  chip a set, the description carrying the number above 1. Stage 1 put
  `sessions` on every engine section (a semaphore held for one stream), a
  managed server above 1 on `-np N --kv-unified` (the pool stays whole where
  `-np` alone splits it), and the `parallel_slots` hint. Rests on a fact:
  since December 2025 a `llama-server` without `-np` already runs **four
  slots over one unified KV pool**
  ([docs/research/parallel-subagents.md](docs/research/parallel-subagents.md),
  [ADR 0010](docs/decisions/0010-subagent-nested-turn.md) amended, spec
  §9.3.2, [docs/journal/tools.md](docs/journal/tools.md)).
- **The web tools became opt-in, and the last unhashed download got a hash** —
  writing a privacy policy for the code-signing track meant inventorying every
  outbound connection rather than trusting SECURITY.md's summary, and the
  summary lost twice: `web_search` shipped **on** although the engines it
  queries are picked by the app and the query is built from the conversation
  (every other address in the app is one the user chose), and
  `TAVILY_API_KEY` was assumed by default, so a key exported for an unrelated
  tool routed the model's searches and spent its credits. Both are decisions
  the user makes now. `python.webc`, fetched by `wasmer` and therefore
  coverable by no lock-list row, is verified against the digest that had sat
  quoted in a comment since August — re-verified live against a fresh registry
  download. Signing, donations and the policy itself:
  [docs/research/code-signing.md](docs/research/code-signing.md),
  [PRIVACY.md](PRIVACY.md),
  [docs/journal/tools.md](docs/journal/tools.md).
- **One hint grid, and footers that name only the keys that work** — the four
  screens the chat bar's two recent reworks never reached (chat list, search,
  `F3`, `F4`) were drawing their hints through a **second, left-aligned**
  implementation of the same layout; there is now one
  (`shared::ui::render_hint_grid`), right-aligned everywhere, with only the
  bar's capped/shedding column choice — the part the status pill's competition
  for the row makes chat-specific — left behind in `status_bar`. And every
  footer is built from the state its own key handler dispatches on, so the rule
  spec §11.2 has stated since the chat list shipped (*an advertised key that is
  a no-op is worse than a missing hint*) now holds on all six surfaces rather
  than one: an observation on `F3` no longer offers "Enter edit" against an
  editor that has always refused it, `R` goes with a file already gone, and
  `F1` — the door to the full list every hidden hint is on — is finally
  advertised everywhere
  ([docs/history/status-hints-unified.md](docs/history/status-hints-unified.md),
  spec §11.1, [docs/journal/ui-screens.md](docs/journal/ui-screens.md)).
- **A third model on the live gate: gpt-oss-120b, split across two files** —
  the gate had only ever run single-file models, so `shared/gguf.rs` (which
  exists to strip a `-00001-of-00002` tail off a name) had met nothing but
  fixtures; now `--chat-model gpt-oss-120b` deploys 63.39 GB of Q8_0 out of a
  **1010 GB** repository — the undocumented `variant` field is what keeps the
  other 946 GB on the Hub — onto an H200 the model's own record names, and the
  live chain reports `gpt-oss-120b-Q8_0` from `/v1/models` through the header
  to the `llm_history` record. Text-only and split are **declarations derived
  from what was deployed**, never flags, so the three vision smokes skip in
  writing and a new smoke checks a part number never reaches a header. On this
  platform the H100 is neither available (quota 0) nor cheap ($10/hr against
  the H200's $5)
  ([docs/research/e2e-gpt-oss-120b.md](docs/research/e2e-gpt-oss-120b.md),
  [docs/journal/ci.md](docs/journal/ci.md)).
- **`get_llm_name`/`get_llm_history` — the language model, named and dated** —
  the assistant can say which **LLM** is running it (from the turn's own
  frozen name — header, metadata and tool answer share one read) and read the
  profile's dated history of model changes, recorded in `data.db` after each
  landed exchange when the pair (name, mode) differs from the newest record;
  `llm_*` deliberately mirrors the `self_model` family it must never be
  confused with, and an engine that names no model records nothing and says so.
  A history that has **never** been written to is seeded once at startup from
  the model names the profile's stored replies already carry — one rule, *what
  the recorder would have written had it existed then*, so nothing about the
  seed needs its own semantics (and no "already seeded" flag exists)
  ([docs/research/language-model-history.md](docs/research/language-model-history.md)
  §9, spec §9.14, [docs/journal/tools.md](docs/journal/tools.md)).
- **`run_dialogue` — a dialogue of two personas, directed by the assistant** —
  the subagent track's promised next feature, **complete (both stages)**: a
  loop-executed
  sibling of `call_subagent` where two caller-composed personas talk (each
  sees the other as the user, the role-encoded transcript on the call's
  record, `CHAT_SCHEMA` 3) and a director — the parent persona with a
  `/compact`-style brief of the conversation — steers via notes/retries/
  rewrites and stops the scene, with the open transcript streaming each line
  on its speaker's side while it runs; probed GO on both gate models and two clouds
  before any product code, with the muted re-ask for the all-thinking empty
  turn discovered live
  ([docs/research/two-agent-dialogue.md](docs/research/two-agent-dialogue.md),
  [ADR 0011](docs/decisions/0011-dialogue-directed-run.md), spec §9.13,
  [docs/journal/tools.md](docs/journal/tools.md)).
- **The chat list counts messages as the conversation reads** — the row's
  `N msg` was `messages.len()`, and an agentic loop stores a row per round and
  per tool result, so one tool-assisted exchange said "34 msg"; now one rule,
  `entities::chat::visible_message_count`, counts the feed's bubbles for chat
  and transcript cards alike, pinned to the feed's projection by an
  every-prefix equality test
  ([docs/journal/ui-screens.md](docs/journal/ui-screens.md), spec §11.2).
- **The model's name in `external` mode — asked of the server, and sent to it** —
  connect by URL with the "Model (opt.)" field blank and the engine is now asked
  what it is running (`EngineBackend::model_id`: `GET /v1/models` when it lists
  exactly one, then `/props`), so the feed's caption and every reply's metadata
  stop being empty in the commonest local setup; a typed name always wins and
  nothing is written back to settings. Fixed on the way: that field had **never
  been sent to the server**, which is what every multi-model endpoint routes on.
  `RetryBackend`'s missing delegation — the third of its kind — was caught by the
  live gate and nothing else
  ([docs/research/external-model-name.md](docs/research/external-model-name.md),
  spec §11.3, [docs/journal/engine.md](docs/journal/engine.md)).
- **`/continue` — an interrupted reply resumes in place, track complete** —
  assistant prefill on managed/external, Gemini, and Claude ≤4.5 (an
  allowlist-by-version gate; OpenAI/Grok refuse with the route that works),
  the server's echo of the prefill stripped byte-exactly, `MessageFinish`
  recorded on every reply, a tool-result tail resuming the loop, and the
  interruption notes naming `/continue` where it applies — a length-cut reply
  finally gets a note at all; probes + live runs on Qwen 3.6/b10659 and the
  real cloud keys
  ([docs/research/continue-generation.md](docs/research/continue-generation.md),
  spec §6.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **`Theme::Auto` actually follows the terminal** — it claimed to "follow the
  system setting" and detected nothing, so code blocks and the *selection
  backdrop* (which `keycap_bg` turns out to drive app-wide) were dark on a light
  terminal; now OSC 11 asks the terminal at startup, measured across six hosts —
  everything but legacy conhost answers — tmux included, and it alone with BEL
  where the rest use ST though asked with BEL — and
  JupyterLab takes 382 ms cold, which is why the query is emitted before storage
  opens and collected once the UI is up
  ([docs/terminal-background-detection.md](docs/terminal-background-detection.md),
  [docs/research/auto-theme-detection.md](docs/research/auto-theme-detection.md),
  spec §11.6).
- **A containerised test environment — JupyterLab, the app, and a CPU stack** —
  `docker compose up` in `docker/` ends in a browser JupyterLab terminal running
  `mindfork` built from the working tree against two CPU `llama-server`
  containers; the app is configured by a seeded partial `settings.json` (not
  `MINDFORK_ENGINE_URL`, which would pin the mode to external forever), and the
  published server ports double as a local, free live-smoke gate that does not
  replace the rented one
  ([docs/research/docker-jupyter-env.md](docs/research/docker-jupyter-env.md),
  [docker/README.md](docker/README.md), install.md §7.3).
- **`F1` help by screen, from every screen** — the "Shortcuts" tab is
  per-screen sections with a "you are here" marker; `F1` opens the dialog —
  a runtime overlay now — over any screen, scrolled to that screen's
  section, and each screen owns its section table next to its key handler
  ([docs/history/help-hotkeys-context.md](docs/history/help-hotkeys-context.md),
  [docs/journal/ui-screens.md](docs/journal/ui-screens.md), spec §11.7).
- **The binary/command is `mindfork`** — one `[[bin]]` stanza over the
  unchanged package `mindfork-rs`; the Linux symlink/`.desktop`/icons, the
  Windows installer, CI and the docs' command examples follow, while data
  locations, package/artifact names and the encryption/lock identifiers
  deliberately keep the project id; pre-release, so no compat shims
  ([docs/research/binary-rename.md](docs/research/binary-rename.md)).
- **Commands — stage 3: `/autotitle`, the personas, the profile texts, and the
  command residue** — the JupyterLab pass's gaps closed: the model-written
  title, the impersonation profiles (`/impersonation list|new|delete|use|system`)
  and the profile texts (`/profile system|greeting`, reserved word `clear`)
  each got a typed route through the settings paths they mirror, and the
  draft flush now precedes the intent, so a spent command no longer resurfaces
  in the box ([docs/history/commands-stage3.md](docs/history/commands-stage3.md)).

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
