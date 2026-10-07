# CLAUDE.md — project guide for mindfork-rs

A console (TUI) AI chat application in Rust. Local models **Gemma 3/4** and
**Qwen 3.5/3.6** via **llama.cpp `llama-server`** (an OpenAI-compatible server;
in external mode any such server works — vLLM/LM Studio/Ollama), plus the cloud
providers OpenAI, Gemini, Anthropic and xAI, and the OpenRouter gateway. UI on
**ratatui**. Platforms: Windows + Linux, macOS on Apple Silicon (a preview). Architecture — **Feature-Sliced Design
(FSD)**.

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

- **Read the section, not the file.** [spec.md](spec.md) (496 KB) and
  [docs/architecture.md](docs/architecture.md) (275 KB) are chaptered reference
  documents with stable numbering; every section is cited by number from the map
  and from the code. Loading either whole spends context on twelve chapters to
  use one.
- **The engineering journal is per area.** What was done, why, what was measured
  and what was rejected lives in `docs/journal/<area>.md` — 571 entries, split by
  subsystem. Read the file for the area you are touching; grep across them when
  hunting a specific past decision.

## Document map — what to read, and when

| When you are… | Read |
|---|---|
| **starting any task** | [AGENTS.md](AGENTS.md) — the mandatory workflow (design doc → branch → live run → docs → PR) |
| looking for the **documents a user reads** | [docs/README.md](docs/README.md) — the human index; [docs/manual.md](docs/manual.md) is how the app is used, [docs/install.md](docs/install.md) how it is set up |
| **about to implement anything** | [docs/lessons.md](docs/lessons.md) — the traps that recur across areas; short, and it will save you a repeat |
| working on the **engine**, a provider, generation, sampling, compaction | architecture §5–§6 · spec §3, §6–§8 · [docs/journal/engine.md](docs/journal/engine.md) |
| working on **storage**, config, backup, migrations, secrets | architecture §7 · spec §5, §12 · [docs/journal/storage.md](docs/journal/storage.md) |
| working on a **tool**, MCP, the Python sandbox | architecture §8 · spec §9 · [docs/journal/tools.md](docs/journal/tools.md) |
| working on the **self-model** — summary, goals, traits, reflection | architecture §9 · spec §17 · [docs/journal/self-model.md](docs/journal/self-model.md) |
| working on **notes** and their graph | architecture §9 · spec §9.3 · [docs/journal/notes.md](docs/journal/notes.md) |
| working on **RAG**, chat attachments or embeddings | architecture §6–§7 · spec §9.3 (§9.3.4–§9.3.5), §9.7 · [docs/journal/rag.md](docs/journal/rag.md) |
| working on the **feed**, markdown, rendering, the terminal, small windows | architecture §10 · spec §11.1.1, §11.3–§11.4 · [docs/journal/ui-feed.md](docs/journal/ui-feed.md) |
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
python tools/indexnow.py --check   # the site's IndexNow key file (+ --self-test)
python tools/site_llms_txt.py --check  # /llms.txt matches the site content
python tools/site_release_gate.py --self-test  # the gate that holds the site's deploy for a release
python tools/actions_pin_check.py  # every workflow action is pinned to a commit
python tools/release_guard.py --self-test  # the tag guard release.yml runs, against fixtures
python tools/aur_package.py --self-test    # the AUR renderer aur.yml runs, against fixtures
python tools/console_probe.py      # Windows: drive the TUI in a hidden console, read the screen back
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

## Status (2026-10-07, version 0.17.0)

The **M0–M9** plan is done, plus extensive post-M9 work — **3936 unit tests
green, 257 `#[ignore]`** (live smokes + a real-clipboard round trip + the
screenshot-dump and demo-reel regenerators).

**This list is pointers, not summaries.** One line per track, newest first: what
it is, and where the reasoning lives. What was decided, what was measured and
what was rejected belongs in the linked document and is not repeated here — a
paragraph per track is how this section grew to 18 KB inside a file that is
loaded in full at the start of every session and is capped at 30 000 bytes by
`tools/doc_index_check.py`. A new track adds a line; a line that has stopped
being recent is dropped, not shortened.

<!-- cyrillic-ok:start -->
- **LM Studio and local servers** (2026-10-06): LM Studio's window is read and its
  silent cut of a conversation's middle told, each note in its server's words; an
  engineless chat offers a local Ollama or LM Studio, `/local` on demand
  ([docs/research/local-servers.md](docs/research/local-servers.md)).
- **Ollama** (2026-10-05): the README's recipe, run live, found a slow-prefill note
  meant for llama-server (fixed) and Ollama's silent cut of an overlong prompt; its
  window is read from `/api/ps`, one too small is said once, and a prompt it cut is
  told from its `usage` ([docs/research/ollama-window.md](docs/research/ollama-window.md),
  [prompt-cut-detection.md](docs/research/prompt-cut-detection.md),
  [docs/journal/engine.md](docs/journal/engine.md)).
- **Scoop and the AUR** (2026-10-03): `vshylov/scoop-bucket` — the release's zip,
  data in `%APPDATA%`, Excavator for updates — is live; `mindfork-rs-bin` is built
  and checked by every release (`aur.yml`) and **blocked**: the AUR's registration
  is closed, so no user doc names it yet ([docs/research/package-managers.md](docs/research/package-managers.md),
  [docs/journal/release.md](docs/journal/release.md)).
- **Promotion — making mindfork findable** (opened 2026-10-03): the plan and its
  baseline ([docs/research/promotion.md](docs/research/promotion.md)). Its first
  code track: a ~25 s animated demo **generated from code** — a scripted turn
  played through the runtime's own path, drawn to GIF/WebP by
  `tools/demo_reel.py` in every release (English and Russian), played by mindfork.io
  ([docs/research/demo-reel.md](docs/research/demo-reel.md),
  [docs/journal/release.md](docs/journal/release.md)). macOS: researched, probe
  GO on Apple Silicon runners, forks decided; stages 1–2 built — secrets, the lock,
  the keys, CI on macOS, the `aarch64-macos` archive, `install.sh`, a Homebrew tap;
  stage 3, a rented Mac day, done — defects in §14.7 ([docs/research/macos.md](docs/research/macos.md)).
- **A pod's failures say what they are** — a managed server that dies shows the
  error it logged (a CUDA build newer than the driver names `cuda-12`), a port
  another program holds is refused, one death is one relaunch. Live **GO**
  ([docs/journal/engine.md](docs/journal/engine.md), spec §3.4).
- **The `max` effort is a stored value** — added with no schema step: 0.13.0,
  measured, refuses settings that hold it and leaves a chat that holds it out of
  its list in silence. `settings.json` and the chat files are stamped 4 → 5, and
  a new effort does not compile until its schema is named. Live **GO**, control
  red (spec §12.2, [docs/journal/storage.md](docs/journal/storage.md)).
- **Grok's efforts are read from xAI's list** — `OpenAiClient::for_xai` asks
  `GET /models/{name}` (an alias resolved): a muted turn is `none` on `grok-4.3`
  and `low` on 4.5–4.7, where it was the default `high`; `max` is `xhigh`; a
  model with no list refuses the parameter and is asked again without it. Live
  **GO**, seven models
  ([docs/research/effort-tiers.md](docs/research/effort-tiers.md) §9, spec §8.1,
  [docs/journal/engine.md](docs/journal/engine.md)).
- **Thinking on the Claude 4.5 generation** — it has no adaptive mode and thinking
  is on by default, so every turn on `claude-haiku-4-5` was a `400`. The refusal is
  asked once more as `{type:"enabled", budget_tokens}`: the effort as the budget,
  half the reply's cap at most. Live **GO**, three models (spec §8.1,
  [docs/journal/engine.md](docs/journal/engine.md)).
- **Reasoning effort: the nearest tier a model has, and `max` — track complete**
  (2026-10-01, two stages). Ten OpenAI models take five different sets, and
  `gemini-3.7/3.8-flash` refuse the `minimal` a muted turn asks for — titles
  failed there and on `gpt-6.1-sol`. A refused value is asked once more as the
  nearest the refusal lists (Gemini: the level above), remembered once accepted
  (`shared/api/effort.rs`). `ReasoningEffort::Max`; Claude gets real
  `xhigh`/`max`; xAI's top is `xhigh`. Live **GO**, controls red
  ([docs/research/effort-tiers.md](docs/research/effort-tiers.md), spec §8.1,
  [docs/journal/engine.md](docs/journal/engine.md)).
- **Small windows — track complete** (2026-09-30, three stages). A window below
  what the frame in front needs gets a notice — the size it is, the size it
  needs, the keys that work — instead of a frame with parts missing; under it
  only quit works, and `Esc` where something is open over the chat. A question
  is a `Prompt`, its keys never cut. The chat sheds its chrome one piece at a
  time (`ChatChrome::at`) and keeps four rows of the conversation down to 20×3.
  A screen's footer takes a third of the window at most and sheds past it, `F1`
  and `Esc` first — the chat bar's shedding, moved into `shared::ui`. Gate: 20
  states × 459 sizes. Live **GO**, 67 checks
  ([docs/research/small-terminal.md](docs/research/small-terminal.md), spec
  §11.1.1, [docs/journal/ui-feed.md](docs/journal/ui-feed.md),
  [docs/journal/ui-screens.md](docs/journal/ui-screens.md)).
- **OpenRouter as a provider of its own — track complete** (2026-09-29, four
  stages, one release). `openrouter` is a mode beside `external`: one key, a dialect
  on the client (`repetition_penalty`, the lowest listed effort on a muted turn), the
  key checked before `Ready`, the picker's rows with window and price, attribution
  behind a switch; `settings.json` 3 → 4. Stage 2 is every embedder's: requests of
  at most 64 texts, a retry for a cloud, and the guard and the convention applied
  by every road that installs an embedder. Stage 3: the audio format negotiated per
  model, the rate read from the answer's label, the voices listed — and a player
  that no longer trusts a streamed MP3's first frame, in every mode. Stage 4:
  `video.provider`; an answer with no video tokens is an error; the whole video is
  read, so a segment is named in words and the ceiling measures the whole. Live
  **GO**, each stage
  ([docs/research/openrouter-mode.md](docs/research/openrouter-mode.md) §7, §11–§14,
  spec §3.4, §9.9, §11.6, §11.9, [docs/journal/engine.md](docs/journal/engine.md)).
- **A settings file the typed parse refuses ends the start, not the settings** —
  one misspelt value reset the whole config, which the first save wrote over the
  file. The gate parses typed, no reader falls back to the defaults, a backup
  reads its password from the raw JSON
  ([docs/research/settings-typed-parse.md](docs/research/settings-typed-parse.md),
  spec §12.2, [docs/journal/storage.md](docs/journal/storage.md)).
- **Colour modes and user themes — track complete** (2026-09-28). Two passes over the
  finished frame (`ui::finish_frame`): `full` paints the theme's canvas, `mono` strips
  every colour and attribute — reverse video kept for a selection and a search match, a
  glyph for whatever else styling alone said. `NO_COLOR` starts the app in `mono` unless
  a mode was chosen. A theme of the user's is `data/themes/<name>.json`: named colours
  kept, the rest fitted to the contrast floors (`shared/user_theme.rs`, `mindfork themes
  export|check`). Measured on Windows 11's console host; other terminals are a look
  ([docs/history/theme-modes.md](docs/history/theme-modes.md), spec §11.6,
  [docs/journal/ui-feed.md](docs/journal/ui-feed.md)).
- **A fetched page is searchable in its birth turn — track complete** (2026-09-26).
  Stage 1: no search promised before it exists, a cut says where, ceiling 1 000 000;
  stage 2: the index starts at the round's end (`IndexBoard`) and a search waits, 6/6
  live birth turns found §17.5 with one search
  ([docs/research/attachment-birth-turn.md](docs/research/attachment-birth-turn.md),
  [docs/journal/rag.md](docs/journal/rag.md)).
- **Raw `llama-server` arguments in managed settings** — an *Extra arguments* field
  per managed section (`--n-cpu-moe` fits a 26B MoE in 8.4 GB); a field's flag, the
  connection's, `--tools`/`--agent` and `-hf` refused, their `LLAMA_*` scrubbed from
  the child, a refused launch said in llama.cpp's words. Live **GO**
  ([docs/research/managed-extra-args.md](docs/research/managed-extra-args.md),
  spec §3.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **A spinner repaints when its glyph changes, not every tick** — a frame per 50 ms
  tick never gave Windows Terminal the 100 ms of quiet it re-finds URLs in, so during
  indexing a hover underlined the line under a link (`Invalid URI`)
  ([docs/journal/ui-feed.md](docs/journal/ui-feed.md), spec §4.4.1).
- **One command from a bare GPU box to a chat — track complete** (2026-09-22).
  `install.sh` (a release asset, attested; the archive refused unless the
  checksums agree; whether the app starts decided by running it) hands over to
  `mindfork setup`, and the README's line ran on a RunPod pod end to end: CUDA
  llama.cpp with its runtime, the sandbox, a 31B Q8_0 `ready in 4 s — context
  131072`. The first real pod found the one defect that shipped — root without
  `CAP_CHOWN`, `tar` restoring the archive's owner — fixed in 0.11.1 with a
  `capsh` scenario and its control arm. Optional: `pod_probe.sh`'s JIT / volume
  / `machine-id` measurements
  ([docs/research/cloud-provisioning.md](docs/research/cloud-provisioning.md) §7–§8,
  [docs/journal/release.md](docs/journal/release.md), install.md §1, §3.3–§3.4).
- **`mindfork setup` — a working managed engine from one command** (stage 1 of
  the provisioning track). Sandbox + llama.cpp + the model, projector, embedder
  and context **written** into the settings (the env variables only override),
  `--set KEY=VALUE` guarded by a round trip through the config's types, and
  `--verify`, which starts what the app will start and reports `/props`.
  Validated before any download; a failed step does not stop the rest. Live **GO**
  on Windows/CPU; Linux and a GPU ride on the pod probe
  ([docs/research/cloud-provisioning.md](docs/research/cloud-provisioning.md) §4,
  spec §3.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **`llama setup` refused CUDA on Linux** — upstream has shipped it since
  2026-09-14, under a runtime-archive name unlike the Windows one; the name is
  derived now. The live smoke then caught `--version` logging ahead of itself,
  which had made the build check skip in silence. Plus backend families
  (`cuda-12`) and a scan that passes over half-uploaded builds. Live **GO** on
  Windows; the Linux GPU half rides on the pod probe
  ([docs/research/cloud-provisioning.md](docs/research/cloud-provisioning.md) §3.1,
  [docs/journal/engine.md](docs/journal/engine.md)).
- **The site waits for the release.** The release PR's merge deployed its own
  post 25 min 28 s before the release was public (measured on 0.10.2). `site.yml`
  now holds a deploy while `Cargo.toml` names an unpublished version and runs
  again when `crates.io` completes; `force` overrides. Rehearsed **GO**, and seen
  for real on 0.11.0: held on the merge, live 3 min 37 s *after* the publication
  ([docs/research/site-waits-for-release.md](docs/research/site-waits-for-release.md),
  [docs/journal/website.md](docs/journal/website.md)).
- **`mindfork stats` — which copy of the data is the newest, and what the other
  one holds** (both stages). A read-only summary of the live data or of a backup
  archive, encrypted ones included: nothing is created, migrated or unpacked.
  `--compare` takes a `--json` snapshot or an archive of the other copy and compares
  chats **by the ids of their messages**, so a chat continued on two machines reads as
  *diverged*, never as "newer there"; equal fingerprints mean an identical comparison.
  Live **GO**, both stages ([docs/history/data-stats.md](docs/history/data-stats.md), spec §12.4,
  [docs/journal/storage.md](docs/journal/storage.md)).
- **The model row asks the provider** (stage 4b, the last of stage 4). The settings
  hint recommended `gpt-4o` and `claude-opus-4-8` — a hint that names a model ages, so
  `Enter` on a model row now asks the provider what it serves and offers the list,
  filtered as you type, with **"type a name by hand" as its first row** so no failure
  traps the user. The first question the UI has ever asked the network
  (`AppCommand::ListModels`, carrying the slot and no key); measured, the four
  catalogues agree on nothing — OpenAI lists **132** entries with **no** capability
  field (chat mixed with `whisper-1` and `sora-2`) and 56 `shutdown_date`s, Gemini
  publishes `supportedGenerationMethods` under a `models/` prefix and pages at 50 of
  58, Anthropic lists 11 chat models and no embedder, and xAI's chat half is
  `/v1/language-models`, not `/v1/models`. So the list narrows on the **provider's**
  claim where there is one and shows everything where there is none — never ours, which
  would age the same way. Live **GO** on all five catalogues
  ([docs/research/model-picker.md](docs/research/model-picker.md),
  [docs/journal/ui-screens.md](docs/journal/ui-screens.md),
  [docs/journal/engine.md](docs/journal/engine.md)).
- **Robustness and the shipped defaults** (stage 4a; the model picker is 4b). A panic in
  the orchestrator left a live interface with nothing behind it — the loop now tells a
  closed channel from an empty one and ends the session saying where the log is; a managed
  build with **no GGUF** started a *router* whose `/health` says ok while every message is
  a 400 (measured) and which could fetch a model from Hugging Face, so an unset model is
  `NotConfigured`; the default reply budget rose 2048 → 16384 because it covers the
  model's reasoning (measured: 1697 thinking + 347 answer, cut) with a `settings.json`
  2→3 step; a refusal printed into a double-clicked console waits for Enter
  (`GetConsoleProcessList` = 1); a second instance exits 2; an external locale falls back
  to English; and the installer offers `PATH` as an unchecked box. Live **GO**
  ([docs/research/robustness-and-defaults.md](docs/research/robustness-and-defaults.md),
  [docs/journal/engine.md](docs/journal/engine.md), [docs/journal/release.md](docs/journal/release.md)).
- **The release pipeline — what a stranger downloads, and what produced it** (stage 3).
  The Windows binary needed a Visual C++ runtime nothing ships (`VCRUNTIME140.dll`,
  measured) — the C runtime is now static, in `.cargo/config.toml` so the tests run on the
  shipped linkage; a tag is checked against `Cargo.toml` and the CHANGELOG before anything
  builds and the release comes out a **draft** (`tools/release_guard.py`, its refusals
  self-tested in `lint`); the licences travel — `THIRD-PARTY-NOTICES.md` from the release's
  own lock file, the grammar licences, the policy in the Linux packages; `nfpm` and every
  action are pinned and hash- or SHA-verified with Dependabot keeping them current, and
  Sonar no longer fails a fork's or Dependabot's pull request
  ([docs/research/release-pipeline.md](docs/research/release-pipeline.md),
  [docs/journal/release.md](docs/journal/release.md), [docs/journal/ci.md](docs/journal/ci.md)).
- **Safe defaults — what an enabled tool may reach** (stages 2a and 2b, each live
  **GO**). 2a: an empty `fs_root` refuses (it meant the whole disk), no file or code tool
  reaches the data root or the binary's directory (`tools/reach.rs`), a dangling link no
  longer writes through the root check, `.git/` is not written, and a background run under
  confirmation gets no dangerous tools. 2b: the sandbox's network is public-only —
  wasmer's own rule list, generated from `net.rs`'s ranges, measured to leave the host's
  loopback and LAN reachable before — the local interpreter and workspace commands start
  without the environment's keys (`shared/child_env.rs`), a fetched page is read under a
  32 MB ceiling, and on unix the data root is `0700` with `0600` files
  ([docs/research/safe-defaults.md](docs/research/safe-defaults.md),
  [docs/journal/tools.md](docs/journal/tools.md)).
- **A headless launch (no TTY) exits with code 2** and a one-line reason — the
  TUI cannot be run from an agent's shell; how it *looks* needs a real terminal
  and a person. What a console *holds* can be measured on Windows:
  `tools/console_probe.py` (docs/lessons.md §6).
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
