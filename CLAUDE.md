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
message speech (TTS) · 0010 the sub-agent as a nested turn.

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
python tools/wizard_rtf.py         # regenerate the Windows installer's disclaimer page
python tools/wizard_rtf.py --check # ...and the gate that it matches DISCLAIMER.md
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

## Status (2026-08-24, version 0.9.7)

The **M0–M9** plan is done, plus extensive post-M9 work — **2544 unit tests
green, 107 `#[ignore]`** (live smokes + a real-clipboard round trip + the
screenshot-dump regenerator).

**This list is pointers, not summaries.** One line per track, newest first: what
it is, and where the reasoning lives. What was decided, what was measured and
what was rejected belongs in the linked document and is not repeated here — a
paragraph per track is how this section grew to 18 KB inside a file that is
loaded in full at the start of every session and is capped at 30 000 bytes by
`tools/doc_index_check.py`. A new track adds a line; a line that has stopped
being recent is dropped, not shortened.

- **Commands — stage 3: `/autotitle`, the personas, the profile texts, and the
  command residue** — the JupyterLab pass's gaps closed: the model-written
  title, the impersonation profiles (`/impersonation list|new|delete|use|system`)
  and the profile texts (`/profile system|greeting`, reserved word `clear`)
  each got a typed route through the settings paths they mirror, and the
  draft flush now precedes the intent, so a spent command no longer resurfaces
  in the box ([docs/history/commands-stage3.md](docs/history/commands-stage3.md)).
- **Sub-agent chats, PRs 2–7 of 7 — the sub-agent with the agent's tools,
  the migration of old calls, the transcript in the list, in search, titled,
  and live while it runs** — `call_subagent` runs as a nested turn with the
  turn's tools (minus itself, `history_*`, the self-model), its transcript
  lives on the call's record, shows nested under the parent in the list,
  opens read-only, is a conversation of its own to every search surface
  (`cache.db` `sub_id`, `CACHE_SCHEMA` 2) and to `chat_search`/`chat_read`,
  is auto-titled at landing, and — through the turn's progress channel and an
  in-flight mirror — is listed, openable and streaming while it runs, with
  the parent ↔ transcript switch not cancelling the turn; `CHAT_SCHEMA` 2
  synthesizes one for every old record, and a tool call's card opens the
  moment the call starts. Go on Gemma 4 31B, 5/5; the track is complete
  ([docs/history/subagent-live.md](docs/history/subagent-live.md))
  ([docs/research/subagent-chats.md](docs/research/subagent-chats.md),
  [ADR 0010](docs/decisions/0010-subagent-nested-turn.md)).
- **Inno Setup 7 for the Windows installer** — a 64-bit setup, and a pinned,
  hash-verified compiler in CI instead of a chocolatey package frozen at 6.7.1
  ([docs/research/inno-setup-7.md](docs/research/inno-setup-7.md)).
- **The licence and the disclaimer in Russian** — unofficial translations under
  `docs/legal/`, on the `F1` tabs and the ru installer's wizard pages, chosen by
  interface language; the English originals govern
  ([docs/history/legal-ru-translations.md](docs/history/legal-ru-translations.md)).
- **A code project attached to a chat** — `/project attach <directory>`: list,
  read, search and change it, run the user's build/run/test command lines, `F4`
  shows every change as a diff with per-file revert. Attaching *is* the
  permission, and the model cannot compose a command
  ([docs/history/code-workspace.md](docs/history/code-workspace.md), spec §9.12).
- **The model's name on the assistant's header** —
  `interface.show_model_name` (off), from the **message's** own metadata
  snapshot rather than the current engine setting
  ([docs/journal/ui-feed.md](docs/journal/ui-feed.md), spec §11.3).
- **Automatic chat titling on the first exchange** — `interface.auto_title`, a
  tri-state defaulting to after the assistant's first *substantive* reply; a
  manual rename outranks it
  ([docs/history/auto-chat-title.md](docs/history/auto-chat-title.md), spec §11.2).
- **The external server's API key, entered in settings** — addressed by *slot*
  (`SecretKey::External`), since four `external` sub-sections are four
  independent URLs; having no key stays legitimate
  ([docs/history/external-api-key.md](docs/history/external-api-key.md), spec §11.6).
- **A second chat model on the live e2e gate** — `--chat-model`
  `gemma-4-31b`/`qwen-3.6-27b`, one per dispatch; the product needed no change,
  the test-side assumptions did
  ([docs/history/e2e-second-chat-model.md](docs/history/e2e-second-chat-model.md)).
- **`/export` — a conversation to a file** — `md` is byte-for-byte what `F5`
  copies; `json` is the `mindfork-import` v1 document, so an export imports back
  onto the same chat
  ([docs/history/chat-export-file.md](docs/history/chat-export-file.md), spec §11.7).
- **OSC 52 — copying to the client's clipboard** — `interface.clipboard_osc52`
  (`auto`/`always`/`off`); the beneficiary is plain SSH, and JupyterLab drops the
  sequence entirely
  ([docs/history/osc52-clipboard.md](docs/history/osc52-clipboard.md), spec §11.7).
- **Command-only control** — nineteen commands make the app operable without a
  chord, each through the same handler its chord uses
  ([docs/history/command-only-control.md](docs/history/command-only-control.md),
  spec §11.7).
- **Navigable `chat://` references** — one address per conversation: the tools
  print it, their descriptions teach the model to cite it, the feed resolves it
  and `Ctrl+L` (or a click) follows
  ([docs/research/chat-uri-links.md](docs/research/chat-uri-links.md), spec §11.3).
- **Cross-chat search for the assistant** — `chat_search`/`chat_read`, off by
  default per profile, scoped in one turn snapshot
  ([docs/research/cross-chat-search-tool.md](docs/research/cross-chat-search-tool.md),
  spec §9.11).
- **Images in a message** — `/image attach|remove|list|paste`, a path or a web
  address, to a local `--mmproj` server and all four clouds; capability is asked,
  never guessed, and a request with no images is byte-identical to before. Its
  SSRF follow-up closed `fetch_url`/`web_search` to non-routable addresses
  ([docs/research/multimodal-images.md](docs/research/multimodal-images.md),
  [mcp-tool-images.md](docs/research/mcp-tool-images.md),
  [image-url-attach.md](docs/research/image-url-attach.md),
  [fetch-url-address-policy.md](docs/research/fetch-url-address-policy.md),
  spec §9.10).
- **Cloud-error retry/backoff** — a `RetryBackend` decorator retries only while
  the turn is *uncommitted*, so no tool effect fires twice
  ([docs/research/cloud-retry-backoff.md](docs/research/cloud-retry-backoff.md),
  spec §6.8).
- **The mindfork.io website, S1–S4** — a Zola site under `site/`, one
  CloudFormation stack (`infra/website.cfn.yaml`), CI deploys through an OIDC
  role. **Live at https://mindfork.io, behind a temporary maintenance IP
  allowlist while the repository is private** — the stack's `AllowedIps`
  parameter; set it empty to reopen
  ([docs/research/mindfork-io-website.md](docs/research/mindfork-io-website.md)).
- **Demo screenshots + demo mode** — a private-data-free, rot-gated capture
  pipeline, plus the interactive `mindfork demo`
  ([docs/history/demo-screenshots.md](docs/history/demo-screenshots.md)).
- **Grok (xAI) as a fourth cloud provider** — the first added without a client of
  its own ([docs/research/grok-xai-provider.md](docs/research/grok-xai-provider.md)).
- **History compression** — a rolling summary keeps a long chat inside the
  context window: `/compact`, an automatic trigger, read-back tools for the
  folded range ([docs/journal/engine.md](docs/journal/engine.md)).

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
