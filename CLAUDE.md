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

## Status (2026-09-11, version 0.9.8)

The **M0–M9** plan is done, plus extensive post-M9 work — **3065 unit tests
green, 159 `#[ignore]`** (live smokes + a real-clipboard round trip + the
screenshot-dump regenerator).

**This list is pointers, not summaries.** One line per track, newest first: what
it is, and where the reasoning lives. What was decided, what was measured and
what was rejected belongs in the linked document and is not repeated here — a
paragraph per track is how this section grew to 18 KB inside a file that is
loaded in full at the start of every session and is capped at 30 000 bytes by
`tools/doc_index_check.py`. A new track adds a line; a line that has stopped
being recent is dropped, not shortened.

<!-- cyrillic-ok:start -->
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
- **The page summary's usage — the one kind that under-counts, and a
  summary that came back empty** — the item was "let the page summary
  record its usage too", left unrecorded on a reading: a request of prose,
  over-counted. Measured, a page's text ran above the estimate on four
  pages of five (a docs page 1.04, an English article 1.16, a Rust source
  1.17, an API's JSON 1.28) — the one kind that under-counts as a rule —
  and on the thinking gate model the summary came back empty on every
  page, the whole 768-token cap spent on thoughts. Now the summary records
  under `Shape::Summary` at its `Usage` chunk, mutes reasoning as the
  title does (`reasoning_budget = 0`), and hands the engine's timing of
  its prompt — the coldest the turn makes, 3236 tokens for a
  12 000-character head — back on `ToolOutcome.prefill`, which the turn's
  loop (`keep_tool_sample`) and the silent loop (`ToolsReport`) fold into
  the largest sample they keep for the slow-prefill note. Measured after:
  the JSON page at 1.29 in its own slot on the 4090, 1.51 on the CPU build
  (36 tok/s, 4 s inside the summary's 90 s limit), a real summary on both
  ([docs/research/page-summary-usage.md](docs/research/page-summary-usage.md),
  spec §6.3, §9.3.1, [docs/journal/engine.md](docs/journal/engine.md)).
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
