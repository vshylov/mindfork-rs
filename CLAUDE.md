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

## Status (2026-09-12, version 0.9.8)

The **M0–M9** plan is done, plus extensive post-M9 work — **3171 unit tests
green, 169 `#[ignore]`** (live smokes + a real-clipboard round trip + the
screenshot-dump regenerator).

**This list is pointers, not summaries.** One line per track, newest first: what
it is, and where the reasoning lives. What was decided, what was measured and
what was rejected belongs in the linked document and is not repeated here — a
paragraph per track is how this section grew to 18 KB inside a file that is
loaded in full at the start of every session and is capped at 30 000 bytes by
`tools/doc_index_check.py`. A new track adds a line; a line that has stopped
being recent is dropped, not shortened.

<!-- cyrillic-ok:start -->
- **The local interpreter joins the file exchange, and the track closes** — `python_exec`
  in Local mode ran the code and nothing else: no files in, none out, and a schema without
  `files`, because a model must never be offered an argument its mode cannot honour. Now
  `LocalSandbox` answers the same `SandboxRunner` contract as the sandbox — one job
  directory per call with the script beside `in/` and `out/`, the interpreter started with
  that directory as its working directory, one collector under the same caps — so the tool
  has no second code path and ADR 0005 §3's "the schema is mode-independent" holds again.
  The folders are one source with two spellings (`/w/in`/`/w/out` in the guest, `in`/`out`
  on the host), and the Wasmer prompt is byte-identical to the text the earlier gates
  measured. Live GO on Gemma 4 31B with **no sandbox at all**: the chat's CSV named in
  `files`, read from `in/`, summed to 4706 and written back to `out/`, stored with the chat
  — and the sandbox's own smokes re-measured after the refactor
  ([docs/history/sandbox-file-exchange.md](docs/history/sandbox-file-exchange.md) §14,
  spec §13.2, [docs/journal/tools.md](docs/journal/tools.md)).
- **A chat's file opens in the system** — `/file open <name|#N>` hands one of the files
  `/file list` shows to the desktop's handler and `/file folder` opens the chat's folder,
  on the handle and the refusals `/file remove` already had. What may be **launched** is an
  allowlist of document types, and that is the point: the folder is written by
  `python_exec`, so a name in it is the model's, and a `run.bat`, a `.lnk` or a scripted
  `.html`/`.svg` would run under its handler — those open the folder they sit in instead,
  with the reason in the note. The launch is our own three calls (`ShellExecuteW` /
  `xdg-open` / `open`), one argument and no shell line, on the blocking pool; the path is
  printed on success and on failure, so a desktop that will not open a type leaves the user
  one copy-paste away. Gate manual by design — a viewer and a file manager opened on
  Windows, while the Linux launch is covered in CI by a stub launcher that records the one
  whole argument it was handed
  ([docs/history/sandbox-file-exchange.md](docs/history/sandbox-file-exchange.md) §13, spec §9.7,
  [docs/journal/tools.md](docs/journal/tools.md)).
- **Files into the Python sandbox — the chat's files reach the code** — a call names
  them in `files`, by the `#N` `/file list` shows or by name, and each is copied into
  `/w/in` under a name the pinned block states **before** the code is written; the
  numbering is one list — attachments, stored files, the chat's images — so `#3` means the
  same to the user and the model. `/file attach` stops refusing a binary (it is kept with
  the chat, no attachment made) and keeps a pdf/docx/html original beside its extracted
  text, the pair one item everywhere. An unknown handle, a shared name or a copy gone from
  the folder refuses the call **before** it runs, and the confirmation popup states the
  resolved names, sizes and the network — the compact view drops arrays, so `files` would
  otherwise be invisible there ([docs/history/sandbox-file-exchange.md](docs/history/sandbox-file-exchange.md)
  §12, spec §9.7, §9.8, §13.2, [docs/journal/tools.md](docs/journal/tools.md)).
- **Files out of the Python sandbox — a chart reaches the user and the model** —
  `python_exec` gains `/w/out`: once the process exits (any exit code, never after a
  timeout) its regular files are collected under caps, stored in `data/files/<chat-id>/`
  and listed in `Chat.files`, and a PNG or JPEG goes back to the model behind
  `tools.python_images`. Measured first on Qwen 3.6 27B and Gemma 4 31B: a plot colour
  only the image carries was named 4/5 and 5/5 with it, 0/5 blind — and blind, both
  described a chart they had not seen, so every withheld image is now said in the
  result, MCP's too. `/file list`/`remove` reach stored files, backups pack them, and an
  unlisted file is adopted at startup, never swept
  ([docs/history/sandbox-file-exchange.md](docs/history/sandbox-file-exchange.md) §10–§11, spec §9.7,
  §9.10, §13.2, [docs/journal/tools.md](docs/journal/tools.md)).
- **The sandbox's packages, packed read-only** — `site-packages` was mounted with a
  plain `--volume` (wasmer has no read-only one), and a `sitecustomize.py` one call
  wrote there ran in the next, in every chat. `sandbox setup` now packs CPython and
  the packages into one self-contained image and starts it once; the runtime runs
  nothing else, and an install from before is refused with the command that packs
  it. One package, because a dependency on `python/python` resolves through the
  registry and a fresh cache cannot start offline (ADR 0005 §5 amended, spec §13.2,
  [docs/journal/tools.md](docs/journal/tools.md)).
- **The sandbox's starter set grows, and `site-packages` turned out writable** —
  twenty wheels from a check of today's WASIX index: sympy (mpmath held below 1.4),
  networkx, tabulate, lxml (in the index now), openpyxl, pypdf, pyyaml, regex,
  feedparser (now on `feedparser-sgmllib`), pillow, matplotlib; the lock list became
  one parsed string. matplotlib needed a wrapper shim — the guest has no `HOME`, and
  its default `force_autohint` traps FreeType here — and the warmup `compileall`s
  the bytecode, most of a cold call. Found on the way: one call can write
  `/sp/sitecustomize.py` that the next call runs; its fix is a PR of its own
  (spec §13.2, [docs/journal/tools.md](docs/journal/tools.md)).
- **`/file remove` and `/image remove` refuse a name two items share** — with
  `a/notes.md` and `b/notes.md` attached, `/file remove notes.md` removed the first
  and said only "notes.md", and `/file list` showed two identical lines (measured
  through the orchestrator; `/image remove` the same). Now one `resolve_handle`
  serves both: `#N` and a path reach one item, a shared name removes nothing and lists
  each holder's `#N` and path, and the listings and the removal note show the source
  where a name is shared
  ([docs/research/remove-by-shared-name.md](docs/research/remove-by-shared-name.md),
  spec §9.7, §9.10, [docs/journal/rag.md](docs/journal/rag.md)).
- **A fetched page's attachment is named after the page, not its site** —
  `fetch_url` named an attachment `<h1>`-first, because docs.vlang.io repeats one
  `<title>`; sector.biz.ua repeats one banner `<h1>`, so its articles were named after
  the archive. Measured on 43 sites first: the `<h1>`-first rule named every page of six
  after the site, and `og:title` first would still have left three. Now
  `NameFields::name` takes what a second field confirms — `og:title`, an `<h1>` the
  `<title>` begins with, the `<title>` without its site segment — with no collision on
  the corpus ([docs/research/page-attachment-name.md](docs/research/page-attachment-name.md),
  spec §9.3.1, [docs/journal/tools.md](docs/journal/tools.md)).
- **Local files are read in their own encoding, and edits written back in it**
  — every path that read a user's file assumed UTF-8, and `code_edit` wrote the
  lossy text back: an ASCII edit in a windows-1251 source left `EF BF BD` for
  every letter while the changes screen showed one line. Now
  `text_decode::decode_file` serves `fs_read`, the code tools' `TextFile`,
  `/file attach`, `/rag` and the changes screen (a BOM'd UTF-16 file is text;
  the interface language hints the detector), and an edit is written in the
  file's own encoding behind a round-trip check, refused with nothing written
  when a byte would not come back
  ([docs/research/local-file-encoding.md](docs/research/local-file-encoding.md),
  spec §9.7, §9.12, [docs/journal/tools.md](docs/journal/tools.md)).
- **A fetched page is read in its own encoding** — reported from a chat: a
  windows-1251 article became an attachment of `U+FFFD`, because `reqwest`
  runs without its default features and `Response::text()` is then
  `from_utf8_lossy` whatever the page declares. Now `shared::http_text::read`
  serves `fetch_url` and `web_search`'s result fetches: an unasked
  `gzip`/`deflate` undone (www.163.com sends one), then BOM → the bytes when
  UTF-8 by majority → a declaration they do not refute (the header, or
  `<meta>` up to `<body>`) → `chardetng` with the TLD. Measured on fourteen
  real pages first: four declare only in `<meta>`, so the header-only
  `charset` feature would not have been the fix
  ([docs/research/page-charset.md](docs/research/page-charset.md),
  spec §9.3.1, [docs/journal/tools.md](docs/journal/tools.md)).
- **Spellcheck reads a stress mark as part of the word** — reported from
  the input box: `И́стинно так` typed, `стинно` underlined. One predicate:
  the segmenter grows a word while `is_alphabetic`, and `U+0301` — the
  sign every Russian source uses, what `Alt+0769` types and what a paste
  carries — is `Mn`, so a word ended at its stress and the halves were
  judged apart. Measured first, that is wrong in **both** directions:
  `чуде́сный ве́чер` drew three underlines under correct text, while
  `за́мок` passed as `за` + `мок` — not noisy on stressed text, off for
  it. Now `U+0300–U+036F` counts as in-word and a word that fails as
  typed is looked up once more stripped; safe because the three bundled
  `.dic` files hold **zero** combining marks, so the second lookup can
  only accept what the first rejected, and as-typed-first keeps `en_GB`'s
  precomposed `café` answering for itself. The mark is ignored, never the
  spelling under it (`харашо́` still flagged, whole, once); suggestions
  come from the unmarked word, and "add to dictionary" stores it unmarked
  so one add covers every placement. `е`/`ё` needed nothing, and that is
  a count, not a guess: **0 of 7347** `ё` stems in `ru_RU` lack an
  accepted `е` spelling, while `афёра` and `опёка` are still flagged — a
  fallback of our own would only have broken the second half
  ([docs/research/spellcheck-stress-marks.md](docs/research/spellcheck-stress-marks.md),
  spec §11.5, [docs/journal/ui-input.md](docs/journal/ui-input.md)).
<!-- cyrillic-ok:end -->
- **The engine, downloaded — llama.cpp's backends named, fetched and
  pointed at** — install.md §3 named `llama-server` the recommended
  backend and assumed it existed, leaving the user to know that the newest
  *tagged* release is not the newest build and that a CUDA build without
  its separate `cudart-` archive does not error, it loads no device and
  runs on the CPU. Now `mindfork llama backends | setup --backend <id> |
  installed`, on the shape of `sandbox setup` — except that a pinned table
  is what this cannot be: upstream ships ~13 nightlies a day and the names
  drifted twice in fourteen months (Linux `.zip` → `.tar.gz`,
  `win-hip-radeon` → `win-rocm-10.0`), so the backends are derived by a
  parse anchored on both ends, an empty middle meaning `cpu`, and anything
  that does not match is skipped rather than guessed. The API, not the
  releases page, because only it carries a per-asset sha256 (63 KB against
  490 KB); one directory per install under `data/llama/<backend>-<tag>/`,
  resumable downloads, and the binary is run before the rename — the build
  number must equal the tag's, and `--list-devices` says whether the GPU
  backend found anything. `--set-binary` then writes the path — the
  assistant's engine always, impersonation and embeddings only where they
  were empty, the mode never switched behind the user (the fork turned on
  reading the code: `managed` is already the default). Measured after:
  b10883 cpu, cpu-b10871 beside it, vulkan reporting a real adapter, and
  the settings written on a config whose embedder path was left alone; the
  downloaded build served the app's live smoke set identically to a
  hand-built one. A follow-up closed the half of spec §3.4 that had never
  been implemented: the binary field is now **resolved**, not taken
  literally — a typed path as written, a bare name beside the application
  then `PATH`, and an **empty** field meaning the build installed last
  under `data/llama/` (not the newest tag, which would move a user off the
  GPU because a CPU build shipped later), so a download or an archive
  unpacked beside `mindfork` needs nothing typed; and `llama remove <id>`
  takes one back off disk, refusing while a managed field points at it
  and reporting what an empty field resolves to afterwards
  ([docs/research/llama-cpp-download.md](docs/research/llama-cpp-download.md),
  spec §3.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **The dialogue's director on Gemma's template — the checkpoint's
  history must alternate too** — the question the Gemma impersonation
  track left: does a scene's first line go out as a system prompt with
  no turns? Measured on Gemma 4 31B and Gemma 3 4B, it does not — the
  participants' views were built for the strict template (a user-side
  prologue, same-role lines merged) — but the director's conversation
  kept its verdicts as tool-call turns with a `tool` "noted" result,
  which Gemma 3's template, having no tool role, renders as a second
  user turn in a row and refuses: a `400` at the second checkpoint of
  every scene longer than one exchange (the clouds' wires merge the
  pair; the dialogue track's Gemma arm was Gemma 4). Now
  `verdict_turn` renders the verdicts as the director's own text turn —
  `name(arguments)` per call, no `tool` messages — the conversation
  still persistent and append-only, accepted by both templates with the
  same verdict. Measured after: two café scenes on Gemma 4 Completed at
  5 lines, stopped by the director past the second checkpoint
  ([docs/research/dialogue-director-history.md](docs/research/dialogue-director-history.md),
  spec §9.13, [docs/journal/tools.md](docs/journal/tools.md)).
- **Impersonation on Gemma's template — the swapped conversation must
  open with the assistant's silence** — the defect the one-shot samples
  track found: impersonation swaps the roles of the history, so a chat
  the user opened became a conversation opening with an assistant turn,
  and Gemma 3's chat template refused it with a `400` before any prefill.
  Measured on Gemma 4 31B and Gemma 3 4B: Gemma 4's template takes every
  shape; Gemma 3's refuses a leading assistant turn and two same-role
  turns in a row, and delivers the system prompt only inside a leading
  user turn (4 tokens for a 54-token system without one). Now
  `alternate_for_template` merges adjacent same-role turns and folds a
  leading assistant turn that a user turn follows into the persona as one
  sentence (`prompt.impersonation.opening`), for every provider — the
  same reply on Gemma 4 for five tokens more, a reply instead of a `400`
  on Gemma 3; the opening-only chat stays the lone turn both accept.
  Measured after: the user-opened seed impersonated on Gemma 4 in 1.5 s
  and on Gemma 3 in 69.8 s with the note, no `400` in the log
  ([docs/research/gemma-impersonation.md](docs/research/gemma-impersonation.md),
  spec §11.8, [docs/journal/engine.md](docs/journal/engine.md)).
- **The one-shot requests' samples — impersonation's prompt is the
  session's largest, and its own twice** — the item was "the title's and
  impersonation's timings for the slow-prefill note"; measured,
  impersonation's prompt is the largest a session makes — the whole
  conversation, roles swapped under its own system, nothing of it in any
  slot's cache: 10 642 tokens where the fresh chat's first turn had 4349,
  and 4 the second time, the cache's — while the title's is 200 on an
  ordinary opening, under the rule's floor, a thousand at most. Now the
  impersonation task keeps the sample under the record's own condition
  (a budget, i.e. the shared engine — the one server the rule knows) and
  lands `ImpDone { id, reason, prefill }` for `handle_imp_done` to offer
  after `ImpersonationFinished`; the title's rides `TitleResult.prefill`,
  offered after its landing whatever the text. Measured after on the CPU
  build, a seeded chat and no turn before the request: impersonation
  1246 tokens at 36 tok/s, the title 1119 at 35 — the note from either;
  the 4090 none. Found in passing: Gemma's template refuses the swapped
  conversation of a chat that opens with the user (a `400` before any
  prefill)
  ([docs/research/oneshot-samples.md](docs/research/oneshot-samples.md),
  spec §3.4, §11.2, §11.8, [docs/journal/engine.md](docs/journal/engine.md)).
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
