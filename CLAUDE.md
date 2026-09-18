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
| looking for the **documents a user reads** | [docs/README.md](docs/README.md) — the human index; [docs/manual.md](docs/manual.md) is how the app is used, [docs/install.md](docs/install.md) how it is set up |
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
python tools/actions_pin_check.py  # every workflow action is pinned to a commit
python tools/release_guard.py --self-test  # the tag guard release.yml runs, against fixtures
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

## Status (2026-09-19, version 0.10.0)

The **M0–M9** plan is done, plus extensive post-M9 work — **3326 unit tests
green, 196 `#[ignore]`** (live smokes + a real-clipboard round trip + the
screenshot-dump regenerator).

**This list is pointers, not summaries.** One line per track, newest first: what
it is, and where the reasoning lives. What was decided, what was measured and
what was rejected belongs in the linked document and is not repeated here — a
paragraph per track is how this section grew to 18 KB inside a file that is
loaded in full at the start of every session and is capped at 30 000 bytes by
`tools/doc_index_check.py`. A new track adds a line; a line that has stopped
being recent is dropped, not shortened.

<!-- cyrillic-ok:start -->
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
- **Before the first public release — the track is closed** (stage 6, 2026-09-19).
  An audit of the flip, the defaults and the first run found twelve blockers and
  staged them; stage 1 made a terminal-less launch refuse (it hung), gave an engineless
  chat the ways to connect one, and rewrote PRIVACY.md from the code (16 corrections;
  `llama setup` reads no `GITHUB_TOKEN`). The flip itself was measured before it was
  taken: **9 229** blobs and **582** pull requests scanned for nine credential shapes,
  **zero** hits, and the Hugging Face namespace the audit feared is the owner's own
  GitHub login; 59 release assets and **183 CI artifacts** deleted for carrying the
  spellcheck dictionaries without their licences; v0.10.0 published and **immutable**
  (a draft created before the setting still becomes immutable at publish — measured,
  the documentation is silent); the settings SECURITY.md had been promising, minus the
  two paid secret-scanning options GitHub refuses in silence. What is left is the
  owner's: one press of the `crates.io` workflow at the next bump
  ([docs/research/public-release-readiness.md](docs/research/public-release-readiness.md) §5,
  [docs/journal/release.md](docs/journal/release.md)).
- **A chart's line said "shown" to a model that takes no images** — `python_exec`
  wrote the claim before the loop knew, and the loop's no-vision note contradicted it in
  the same result. The loop, which alone has the vision answer and the prepared pixels,
  now ends the image's line (`ToolImage.entry`) with its fate; the shown text is
  byte-identical. Live **GO** on OpenRouter, both arms (spec §9.10,
  [docs/journal/tools.md](docs/journal/tools.md)).
- **A chat's history images, on an engine that takes none** — replayed every turn,
  they got each turn refused (`500` from llama.cpp without a projector, `404` from a
  gateway). On the engine's "no" the request carries a marker per image instead —
  a silent drop made models deny what was there, 10/10; the marker, 10/10 "I cannot
  see it" — and the chat is told once. Live **GO**, control red
  ([docs/research/history-images-no-vision.md](docs/research/history-images-no-vision.md),
  spec §9.10, [docs/journal/engine.md](docs/journal/engine.md)).
- **A gateway's catalogue says whether the model takes images** — a text-only
  model's image was a `404` from the gateway itself (24/24), and every later turn
  of that chat too, since images replay as history. `vision()` reads
  `architecture.input_modalities` after `/props`, both ways, so attach refuses and
  a tool's picture is withheld and said; live **GO**, and a chat already holding an
  image is stage 2
  ([docs/research/gateway-vision-catalogue.md](docs/research/gateway-vision-catalogue.md),
  spec §9.10, [docs/journal/engine.md](docs/journal/engine.md)).
- **A reply the content filter stopped says so** — every provider reports it and
  every client's fallback arm read it as a finished answer; Responses read it as a
  length cut and offered `/continue` into the filter, and Gemini wrote an English
  note into the reply. One `FinishReason::Filtered` now, a localised note, stored as
  `stop` so the chat format holds
  ([docs/research/content-filter-finish.md](docs/research/content-filter-finish.md),
  spec §6.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **The thinking switch reaches a gateway** — a gateway never reads `thinking` (a
  wrong type in it is a `200`), so "on" left Haiku 4.5 at 0 reasoning tokens and
  "off" left Qwen 3.6 reasoning. Where the catalogue lists `reasoning`, a request
  with no effort also carries `reasoning: {enabled}`; "off" only on a stated
  `mandatory: false`, a zero budget reads as off, and the F2 recovery covers a
  refused `enabled: false`. Default is "on", so such models now reason. Live **GO**
  ([docs/history/gateway-thinking-switch.md](docs/history/gateway-thinking-switch.md), spec §8.1,
  [docs/journal/engine.md](docs/journal/engine.md)).
- **`/file open 1` — a bare number is the listed `#N`** — reported from a chat:
  `/file list` showed `#1`, and `/file open 1` was refused as "not attached". One
  resolver now reads a bare number as `#N` unless an item is called that, for `/file`,
  `/image remove` and `python_exec`'s `files` alike, and a miss answers with the numbers
  there are (spec §9.7, [docs/journal/rag.md](docs/journal/rag.md)).
- **Through a gateway, a tool's image and `/continue` belong to the route** — M3 ran
  the app itself through OpenRouter (GO on Bedrock-routed Haiku: a chart only the
  image could answer for, the `/file` round trip), and per pinned route with blind
  controls both halves of F6 turned out real: of 29 route-and-model pairs 3 answer
  about a tool image they never received (in a user message 28 of 28 see it, so
  stage H1 re-homes them there on a gateway), and `/continue` restarts on OpenAI and every
  open-weight route, which the echo filter stored glued onto the partial. Stage H2
  gates it: on an `external` endpoint whose catalogue answered, the slug's vendor
  is read against the spec §6.4 table (Anthropic ≤ 4.5, Gemini continue; the rest
  refuse with a gateway note), one answer feeds the gate, `Finished.continuable`
  and the notes, and the catalogue is asked when the engine is applied so the
  first command after a restart is not answered blind. **Silence is never a
  claim**: no catalogue, no change. F5 closed on a measurement — a garbage
  signature is a `400` through the gateway and direct, sending no blocks never is,
  so there is nothing to build
  ([docs/history/gateway-images-and-continue.md](docs/history/gateway-images-and-continue.md),
  spec §6.4, [docs/journal/engine.md](docs/journal/engine.md)).
- **The endpoint is asked what the model can do** — a gateway serves no `/props`,
  so automatic compaction had no window and never fired (measured: 22 567 tokens,
  nothing folded), while the sampling screen offered the whole llama.cpp set of
  which a gateway drops half — `repeat_penalty` worst of all, spelled
  `repetition_penalty` there and so set but inert. The catalogue the app already
  fetches for the model's *name* carries both answers, so one request now feeds
  both: `EngineBackend::model_capabilities` (default `None`, overridden only by
  `OpenAiClient`, delegated by the retry decorator with the test lessons §9
  demands), one background task landing one `EngineFacts`, and the registry
  rebuilt only when the published set really changed. **Silence is never a
  claim** — no catalogue, an empty list, a blank model field, a llama.cpp that
  lists ids and nothing else all leave behaviour exactly as it shipped; the
  window sits *after* `/props` (a running server describes this turn's process, a
  catalogue the model in the abstract) and an explicit setting still beats both.
  The narrowing reaches the settings screen, `set_sampling`'s schema and the
  metadata snapshot — **not** the wire, since the list is per model while the
  route that serves the request is per provider, and dropping a field ourselves
  on that evidence would trade their silent drop for ours. Live **GO** on R1: 64 000 for the
  window a gateway never had, and thirty offered fields down to ten — with
  `repeat_penalty` surviving under the catalogue's own `repetition_penalty`, which
  is the alias table earning its place. The local stack is measured unchanged:
  `/props` first, the catalogue not even asked
  ([docs/history/gateway-capabilities.md](docs/history/gateway-capabilities.md), spec §6.7, §8.1,
  [docs/journal/engine.md](docs/journal/engine.md)).
- **`external` against a gateway — a thought under a second name** — the mode we
  recommend for OpenRouter dropped every thought it sent: the client read
  `delta.reasoning_content`, a gateway writes `delta.reasoning`, and an unknown
  field deserializes away in silence, so a reasoning model there answered with
  its thinking invisible and the `<think>` fallback could not help (the gateway
  has already lifted the reasoning out of `content`). Both names are read now,
  `reasoning_content` first, taking **both** fields — one trace under two names
  must not double — so the local stack is byte-identical to before. The review's
  other "change nothing" — `reasoning_effort:"none"` on the three silent turns —
  **was refuted by the measurement that followed**: that body is answered `400
  "Reasoning is mandatory for this endpoint and cannot be disabled"`, so on an
  always-reasoning model the title, the compaction roll and impersonation fail
  while chat works (the xAI failure by another road). Fixed on the user's choice: the
  client re-sends once without the field and **remembers** the refusal per server,
  feeding the same `omit_effort_none` switch xAI is configured with — narrow by
  construction (a `400` only, only when the turn asked, only when the message
  names reasoning *and* its disabling), so a `503` stays the retry decorator's, and
  **GO** on R1: the title comes back where the same request used to be the `400` —
  with 772 characters of reasoning still spent, because that endpoint cannot be
  asked to stop, which is the residual the fix does not claim to remove.
  The catalogue itself is measured
  and carries `context_length` **and** `supported_parameters` per model, so the
  window and the honest sampling list are one fetch away.
  Live **GO** on `deepseek/deepseek-r1` through OpenRouter (the session had no
  route to the service; the author ran it): thoughts and answer both arrive, and
  the same run turned F3 from prediction into measurement — 22 567 tokens, nothing
  folded, because a gateway reports no window — while proving `usage` parses.
  Writing those commands found two more things: `live_client` sent no `model`, so
  **no** live smoke could have been pointed at a gateway (it now derives
  `MINDFORK_ENGINE_MODEL`), and "run the whole `--ignored` set" is local-stack
  advice that costs hours and real money on a metered one (install.md §7.1)
  ([docs/research/openrouter-external.md](docs/research/openrouter-external.md),
  spec §6.5, [docs/journal/engine.md](docs/journal/engine.md)).
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
- **A headless launch (no TTY) exits with code 2** and a one-line reason — the
  TUI cannot be run from an agent's shell; live verification needs a real
  terminal.
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
