# CLAUDE.md — project guide for mindfork-rs

A console (TUI) AI chat application in Rust. Local models **Gemma 3/4** and
**Qwen 3.5/3.6** via **llama.cpp `llama-server`** (an OpenAI-compatible server;
in external mode any such server works — vLLM/LM Studio/Ollama), plus the cloud
providers OpenAI, Gemini and Anthropic. UI on **ratatui**. Platforms: Windows +
Linux. Architecture — **Feature-Sliced Design (FSD)**.

> **Engine:** originally designed around `xinfer`, but it turned out too raw
> (incoherent output on Gemma 4, builds poorly on Windows) — switched to
> llama.cpp. Generic-OpenAI client (`OpenAiClient`), managed launcher
> `LlamaSupervisor`; the cloud providers are separate `EngineBackend`
> implementations. See [docs/install.md](docs/install.md) §3 and
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
| installing, running, configuring the engine | [docs/install.md](docs/install.md) |
| what is left to do | [docs/roadmap.md](docs/roadmap.md) |
| what the user sees as changed | [CHANGELOG.md](CHANGELOG.md) |

**ADRs** (`docs/decisions/`): 0001 UI crates for ratatui 0.30 · 0002 dedicated
embedding server · 0003 own markdown renderer · 0004 engine contract and
multi-provider inference · 0005 Python sandbox as a `wasmer`/WASIX sidecar ·
0006 data schema versioning and migrations · 0007 plugins — MCP tool host and a
neutral import format · 0008 API keys with machine-bound encryption · 0009
message speech (TTS).

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

## Status (2026-08-14, version 0.9.6)

The **M0–M9** plan is done, plus extensive post-M9 work — **2216 unit tests
green, 97 `#[ignore]`** (93 live smokes + a real-clipboard round trip + the
screenshot-dump regenerator). The
most recent tracks: **navigable `chat://` references — track complete**
(`chat://<short-id>` is now the one address a conversation has: the cross-chat
tools print it, their descriptions teach the model to **cite it when it mentions
a conversation to the user**, and the feed draws a resolvable one as a link that
`Ctrl+L` follows. The scheme was **observed before it was designed** — a model
invented it out of the bracketed address the tools used to print — so the load-
bearing half turned out to be the teaching, not the rendering. Only an address
that resolves is styled, against the current profile's chats, so a link is never
a dead end and the profile boundary needs no separate check; recognition runs in
the block builder *before* the wrap, because an address is 15 columns and a
narrow panel splits it where the post-render matching used by in-feed search
would miss it. A key rather than a click: feed clicks are a no-op and mouse
capture is off by default; stage 2 then added the click anyway, from a map
derived at render time for the viewport only — the one point where the wrap, the
scroll and the panel's origin have all been applied, so nothing stored can go
stale;
[docs/research/chat-uri-links.md](docs/research/chat-uri-links.md), spec §11.3),
**cross-chat search for the assistant — track complete**
(`chat_search`/`chat_read`, **off by default** per profile: the model can
search and read the profile's *other* chats over the full-text index the app
already keeps — results grouped by conversation and page-addressed through the
same renderer as `history_read`, so a hit's page is exactly what a read
returns; the index is profile-blind and includes the current chat, so the
scope lives in a single turn snapshot — current profile only, current chat
excluded, hidden dropped — and the SQL is scoped by chat ids so another
profile's rows cannot starve the cap;
[docs/research/cross-chat-search-tool.md](docs/research/cross-chat-search-tool.md),
spec §9.11), **images in a message — track complete**
(`/image attach|remove|list`
stages an image for the **next** message, which is what every provider's wire
format actually models; it reaches a local `llama-server --mmproj` and **all four
clouds** — one shared change served llama.cpp and Grok, whose request shapes are
byte-identical, and stage 2 added the Anthropic `image` block, Gemini
`inline_data` and Responses `input_image` dialects.
Decode/downscale/normalize happens once at attach time — xAI takes png/jpeg only,
and an unscaled photo would ride every later turn — while a png already within the
ceiling passes through byte for byte. Capability is *asked*
(`EngineBackend::vision`, llama.cpp's `/props` `modalities.vision`), never guessed
from a model name, and a request with no images stays byte-identical to what the
app sent before the feature — in **all four** wire formats, each with its own
test, which is what makes the change safe for every stored conversation.
`/image paste` then took a screenshot straight off the clipboard: the app never
had a `Ctrl+V` handler at all — pasting works because the *terminal* injects
text, and an image injects nothing — so the command is the route that works in
every terminal and the key is the convenience. An **MCP tool's** image reaches the
model too: inside the tool result on four engines, and — since Gemini answers a
hard `400` there — as user parts right after it on the fifth, chosen per provider
rather than by catching the error. `/image attach` finally learned a **web
address** as well as a path — the bytes are always downloaded client-side, so one
path serves all five engines and the pixels live in the chat, where a dead link
cannot break a stored conversation; designing it found that the SSRF care the
deferred note promised to inherit from `fetch_url` had never existed — which
became the **next** track: `fetch_url` and `web_search`'s page fetches now refuse
anything not publicly routable, with the check inside the client's DNS resolver
so the approved address is the connected one, an IP-literal check `hyper` would
otherwise skip, and `tools.web_allow_private` (off) as the way back in;
[docs/research/multimodal-images.md](docs/research/multimodal-images.md),
[docs/research/mcp-tool-images.md](docs/research/mcp-tool-images.md),
[docs/research/image-url-attach.md](docs/research/image-url-attach.md),
spec §9.10),
**cloud-error retry/backoff — track complete** (a transient
`429`/`5xx`/`529` no longer costs the turn: a `RetryBackend` decorator over the
cloud and external backends retries three times with jittered backoff, honouring
`Retry-After` up to 30 s, and only while the turn is *uncommitted* — a tool call
counts as content, so a round that emitted calls is never replayed and no tool
effect fires twice; stage 1 first made such failures visible at all, since a
mid-answer failure used to leave a silent fragment on screen and Anthropic's
in-stream `error` events were swallowed entirely;
[docs/research/cloud-retry-backoff.md](docs/research/cloud-retry-backoff.md), spec §6.8),
the **mindfork.io website, S1–S4 — track complete** (a
Zola site under `site/`, terminal-styled on the brand palette, **live at
https://mindfork.io** — currently behind a temporary maintenance IP
allowlist while the repository is private (the stack's `AllowedIps`
parameter; set it empty to reopen): one CloudFormation stack
`infra/website.cfn.yaml` —
private S3 + OAC, CloudFront, ACM, Route53 — with CI deploys on push to main
through the stack's OIDC role, no stored keys; the landing inlines
screenshots as generated SVG — the demo-screenshots stage 4, shipped from
the site side; Zola pinned to 0.22.1 for a Windows template-discovery
regression in 0.23.x;
[docs/research/mindfork-io-website.md](docs/research/mindfork-io-website.md)),
**demo screenshots + demo mode** (a private-data-free,
rot-gated capture pipeline — demo fixture → headless frame dumps →
`tools/screenshots.py` — plus the interactive `mindfork demo`: the real TUI on
seeded data with a scripted engine, nothing touched outside a temp folder;
[docs/history/demo-screenshots.md](docs/history/demo-screenshots.md)),
**Grok (xAI) as a
fourth cloud provider** (the first one added without a client of its own — see
[docs/research/grok-xai-provider.md](docs/research/grok-xai-provider.md)), the
**history compression** track (a rolling summary keeps a long chat inside the
model's context window — `/compact`, an automatic trigger, and read-back tools
for the folded range), and the documentation refactor.

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
