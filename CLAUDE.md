# CLAUDE.md — project guide for mindfork-rs

A console (TUI) AI chat application in Rust. Local models **Gemma 3/4** and
**Qwen 3.5/3.6** via **llama.cpp `llama-server`** (an OpenAI-compatible server; in
external mode any such server works — vLLM/LM Studio/Ollama). UI on **ratatui**.
Platforms: Windows + Linux. Architecture — **Feature-Sliced Design (FSD)**.

> **Engine:** originally designed around `xinfer`, but it turned out too raw
> (incoherent output on Gemma 4, builds poorly on Windows) — switched to
> llama.cpp. Generic-OpenAI client (`OpenAiClient`), managed launcher — `llama-server`
> (`LlamaSupervisor`). See [docs/install.md §3](docs/install.md).

## Key documents (read before working)
- **[AGENTS.md](AGENTS.md)** — **mandatory task workflow**: design doc
  for complex tasks, branch before the first commit, live run, documentation-update
  checklist at finalization, model attribution in commits and PRs.
- **[spec.md](spec.md)** — full engineering specification (domain model, engine,
  tools, profiles, UI, testing). Source of truth for "what" and "why".
- **[docs/install.md](docs/install.md)** — installation/launch (llama.cpp `llama-server`
  managed/external, env, OpenAI-compatible protocol, dictionaries, import).
  **Up to date on the engine.**
- **[docs/decisions/](docs/decisions/)** — ADR: UI crates for ratatui 0.30 (0001),
  dedicated embedding server (0002), own markdown renderer (0003),
  engine contract and multi-provider inference without a crate split (0004),
  Python sandbox as a `wasmer`/WASIX sidecar behind `shared/sandbox.rs` (0005),
  data schema versioning and migrations (0006), plugins: MCP tool host +
  a neutral import exchange format (0007), API keys in settings with
  machine-bound encryption (0008), message speech (TTS): cloud
  provider + own speech extractor + `rodio` player, in progress (0009).
- **[docs/history/](docs/history/)** — archive of what's been implemented: the original
  spec (`request.md`), the completed step-by-step plan M0–M9 (`plan.md`), and **design
  plans of completed tracks** (below). Historical reference, not a source of truth.
- **Track design plans** (idea bank + implemented MVP probe; the tracks are
  **implemented**, plans live in `docs/history/`): "self-model"
  ([self-model.md](docs/history/self-model.md), [self-model-mvp.md](docs/history/self-model-mvp.md))
  + refinements ([refinements.md](docs/history/refinements.md));
  **notes connectivity** ([notes-connectivity.md](docs/history/notes-connectivity.md)) —
  shift of memory from accumulation to integration (semantic search, revision, compatibility
  gate); **narrative as notes**
  ([narrative-as-notes.md](docs/history/narrative-as-notes.md)) — unification: self-model
  observations move into notes with the `@self` tag; **summary as snapshot**
  ([summary-as-snapshot.md](docs/history/summary-as-snapshot.md)) — anti-bloat for the
  self-model description (genre boundary, size gate, per-section injection budgets,
  echo diet); **multilinguality** — axis A, agent language
  ([i18n.md](docs/history/i18n.md)) + external locales
  ([i18n-external-locales.md](docs/history/i18n-external-locales.md)), axis B, interface
  language ([i18n-ui.md](docs/history/i18n-ui.md)), CLI text
  ([i18n-cli.md](docs/history/i18n-cli.md)).

## Key architectural decisions
- **Engine = a local OpenAI-compatible HTTP server** (managed `llama-server` or
  any external one), the application is an HTTP client (`OpenAiClient`). Embedding (rlib)
  is deliberately NOT used (it drags in candle/CUDA).
- **The agentic loop is client-side** (in the orchestrator). Tools return a result +
  effects; the orchestrator (sole owner of `Chat`) applies them — no locks.
- **Sampling** (`SamplingConfig`, spec §8): standard OpenAI fields (temperature,
  top_k, top_p, frequency/presence_penalty, max_tokens, thinking, reasoning_effort,
  reasoning_budget) **plus llama.cpp extensions** that `llama-server` accepts
  in the request body: dynatemp_range/dynatemp_exponent (dynamic temperature), min_p,
  top_n_sigma, typical_p, adaptive_target/adaptive_decay (adaptive-p, experimental),
  repeat_penalty/repeat_last_n, dry_* (multiplier/base/allowed_length/penalty_last_n/
  sequence_breakers), xtc_* (probability/threshold), mirostat/mirostat_tau/mirostat_eta,
  seed, samplers (sampler order). Every field is `Option`, sent only when
  set (`skip_serializing_if`) — an unset one doesn't reach the JSON; a field
  unsupported by the server is ignored by it (a strict third-party OpenAI server
  might reject it, but only if the user sets the extension themselves). List fields
  (`dry_sequence_breakers`, `samplers`) are JSON arrays, not sent empty. UI — the
  "Sampling" section (Assistant/Impersonation subsections), fields via `SamplingParam`
  (`screens/settings.rs`; lists are edited as text: samplers via `;`, DRY breakers
  via `,` with `\n`/`\t`/`\r` escapes); also editable with the `set_sampling` tool
  (merges all fields).
- **EOS**: stopped by token-id on the server; the `stop` field is NOT sent (anti-self-cutoff).
- **"Thoughts" (CoT)**: `delta.reasoning_content` (`llama-server --reasoning-format`);
  fallback — parsing `<think>` out of content.
- **Storage**: JSON (config/profiles/chats, atomic write + .bak) + SQLite
  (notes/RAG, sqlite-vec, isolation by `profile_id` via a partition key).
- **Soft delete** everywhere (`is_hidden`, profile→chats cascade).
- **Unidirectional UI↔orchestrator flow**: `AppCommand` commands flow up, `AppEvent`
  events flow down; `generation_id` drops stale chunks; state machine
  `Idle/Generating/Cancelling`.

## Structure (FSD, dependencies strictly downward)
`app → screens → widgets → features → entities → shared`. A binary crate.
- `src/app/` — orchestrator, events, TUI loop, tokio↔UI bridge.
- `src/entities/` — domain types (`chat`, `message`, `profile`, `note`, `rag`, `sampling`).
- `src/shared/api/` — the engine behind the `EngineBackend` trait (OpenAI client `OpenAiClient`, `llama-server` launcher, thoughts parser, mock).
- `src/shared/storage/` — `json` + `db` (SQLite) + `Storage` facade.
- `src/shared/` — `config`, `paths` (portable, next to the binary), `logging`, `instance`, `error`.
- `src/{screens,widgets,features}/` — still stubs (filled in starting M3).

## Conventions
- Task workflow (design doc → branch → development → docs → PR) —
  **[AGENTS.md](AGENTS.md)**; below are code conventions.
- Rust edition 2024. **Before every commit**: `cargo fmt`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test` — all must be green.
- Errors: `anyhow` in application layers, `thiserror` in library modules under `shared`.
- Logs — file only (`logs/`, stdout is taken by the TUI). No `println!`.
- Tests are written next to the code as it's built (`#[cfg(test)]`); real server/model — `#[ignore]`.
- Comments and docs — in English; references to spec sections.
- End commits with a trailer naming the **actual model** that wrote the code
  (AGENTS.md §5), e.g.: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

## Commands
```
cargo build
cargo test                         # unit tests (no server needed)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run                          # TUI (needs a REAL terminal — see below)
```
Running against a real server (smoke test, llama.cpp):
```
llama-server -m gemma-4-it.gguf   --host 0.0.0.0 --port 8000 -ngl 99 -c 16384 --jinja        # chat
llama-server -m bge-m3-Q8_0.gguf  --host 0.0.0.0 --port 8001 -ngl 99 -c 16384 --embeddings   # embedder (memory/RAG)
$env:MINDFORK_ENGINE_URL="http://127.0.0.1:8000/v1"   # PowerShell; URL includes /v1
$env:MINDFORK_EMBED_URL="http://127.0.0.1:8001/v1"    # needed by the gate/graph/memory consolidation
cargo run
# All live smokes (silently skipped without the needed env var):
cargo test -- --ignored --nocapture --test-threads=1
# Memory/self-model/notes only (12 e2e):  cargo test e2e_live -- --ignored --nocapture --test-threads=1
```
Env for selecting the backend: `MINDFORK_ENGINE_URL` (external, any OpenAI server) OR
`MINDFORK_LLAMA_BIN` (+ `MINDFORK_MODEL` GGUF, `MINDFORK_NGL`, `MINDFORK_CTX`,
`MINDFORK_PORT`) for a managed `llama-server`.

## Status (as of 2026-08-03, version 0.9.4)
The entire **M0–M9** plan is done, plus extensive post-M9 work (on `main`). **1833 unit
tests green, 76 `#[ignore]` smokes** (the largest count — log below; the most
recent change makes **tool calls collapsible, like "thoughts"**: `Ctrl+O` folds a
call's arguments and result away while keeping the header that says what ran,
both kinds of block are **collapsed by default**, an expanded card is laid out
as name → arguments one per line → blank row → result (the header is a title: it
truncates, and could never show a structured argument at all), and the
collapsed/expanded choice is remembered **per chat** (`Chat.feed_view`, the `draft` playbook — no
migration) instead of being one global flag; before that — **`fetch_url` back the page**: extraction keeps section
headings and code blocks (prose-only extraction turned documentation into text
whose every "here is an example:" led nowhere), and a page over the attachment
budget lands as a **chat attachment** instead of being cut silently at 12 000
characters mid-word — with two neighbours of the same defect class fixed
alongside, `web_search` reporting "no results" while a provider served a captcha
behind HTTP 200, and `python_exec` not saying that each call gets a fresh
sandbox; before that — **`backup`/`restore` narrate their work**: every phase
announces itself before it runs and the packing/unpacking loops count their
entries, so a restore that takes twenty seconds no longer looks like a hung
program — least of all right after the echo-less password prompt; and keys
pressed while it worked are discarded instead of being replayed by the shell on
exit); before that — a small input-box refinement: **`Home`/`End` became a ladder
of stops** — a wrapped line's on-screen row first, then the whole logical line,
with `Home` additionally stopping at the line's first non-whitespace character;
the step is chosen by the cursor's position, not by counting presses, so nothing
has to reset it; before that —
the change that completes the **MCP server editor** track
([plan](docs/history/mcp-server-editor.md)) with **secrets for the `env` map and
JSON import**: a server's token is typed into a masked row and stored
machine-bound (ADR 0008, `mcp-<server>-<VAR>`) instead of demanding an OS
variable, with the named variable kept as the fallback — so ADR 0007 R8's real
invariant, *no plaintext secret on disk*, is unchanged; another client's
`mcpServers` config is imported **by path**, parsed by the orchestrator because
such a file carries literal tokens that must not travel through `screens`. The
trap the design caught before it shipped: a secret changes no *setting*, so
`is_current` had to start comparing the **resolved environment** or the debounce
would leave the server running with the old value — and comparing the resolved
env rather than the key blob keeps an unrelated OpenAI key from respawning `npx`;
before that — stage 1 of the same track, **MCP servers are configured in the
settings window**: a "Plugins" section with the master switch, a server editor
(`Ctrl+N`/`Ctrl+D` over `config.mcp.servers`) and the live statuses; the
orchestrator needed nothing for the edits themselves — the screen already ships
the whole config and `handle_update_config` diffs `config.mcp` into the restart
debounce — while `Enter` on a status row gained a second meaning, **reconnect**,
which is the only way back for a server past its restart budget since
`is_current` stopped re-applying an identical config; before that —
**spellcheck no longer underlines URLs and email addresses** (an
address is skipped whole in segmentation, so both the underlining and the
`Ctrl+G` popup ignore it; a bare domain must be lowercase ASCII, which keeps a
run-on sentence like `end.Next` a typo rather than a domain); before that — **a video's words can now land as a chat attachment**
([plan](docs/history/youtube-transcript.md)): `youtube_watch(transcript: true)`
also brings back the spoken words with timestamps, in the *same* provider call as
the description; past the per-file attachment budget they become an attachment
(spec §9.7) — already paged and searchable — instead of filling the context. The
stage's real work was the contract: `ChatEffect` gained a generic
`AddAttachment`, and because effects only reach `Chat` when the turn ends while
`ToolContext.attachments` is a turn snapshot, the loop mirrors the effect into
that snapshot, or "attached as X, read it with `attachment_read`" would be an
instruction the turn itself could not carry out; before that —
**the assistant can watch a YouTube video**
([research](docs/research/youtube-integration.md)): `youtube_watch` says what a
video shows *and* says, with timestamps. Measurement left one option — every free
caption route is now behind YouTube's PoToken gate (a **signed** `timedtext` URL
returns 200 with an empty body), `captions.download` needs the owner's OAuth, and
only Gemini takes video at all — so the tool calls Gemini **out of band**, the
ADR 0009 shape, and therefore works on a local model too; billed per second of
footage, it refuses past a ceiling and explains how to ask for a segment, and with
no key degrades to free metadata rather than vanishing; before that —
**password-protected backups** ([plan](docs/history/backup-password.md)):
`--password` on `backup`/`restore` or a password set once in settings → "Data",
stored machine-bound exactly like a cloud API key (ADR 0008); the archive is
standard AES-256 so 7-Zip still opens it, restore takes an encrypted *and* an
unencrypted copy with nothing to switch, and a wrong password is refused before
anything is replaced; before that — **a settings hint always fits its panel** (the bottom panel had
room for three lines, so the longer hints — API key, MCP servers, speculative
decoding — were cut mid-sentence; it is now as tall as the section's longest hint
needs and stays that height across the section, so the list doesn't shift);
before that — **real syntax grammars for 22 languages are vendored**
([plan](docs/history/vendored-syntaxes.md)): syntect ships a 75-syntax snapshot
of Sublime's defaults, so `zig`/`toml`/`dockerfile`/`powershell`/`swift`/… all
fell into the unhighlighted path; the grammars now live in `syntaxes/`, pinned
to upstream commits with their licences, and `build.rs` compiles them into one
dump — 0.55 ms to load, the same as before; before that — **Zig code blocks are
highlighted** (the same symptom, closed first with an approximation, now
superseded by the real grammar); before that — **an unhighlighted code block is drawn as a rectangle** (its
reverse-video background used to follow the ragged right edge of the text; rows
are now padded to the block's own width plus a blank column on the right —
content-sized like a table, not stretched across the panel); before that — **no server restart when nothing
effectively changed** (the
restart is decided against what each server is actually running, not against the
previous edit — so an edit and its `Ctrl+Z` no longer reload a multi-GB GGUF for
nothing); before that — **undo on the settings screen**
([plan](docs/history/settings-undo.md)): `Ctrl+Z` takes a setting edit back and
lands the cursor on the field it reverted — closing the consequence left over by
the change before it, which closed the *cause*; the whole thing needed no new
command or event, since `SaveConfig` already carries the entire working config;
before that — the **settings-screen focus model**
([plan](docs/history/settings-navigation.md)): `Enter` is now the only way into the field
pane and `Esc` the only way out (a second one closes the screen), the arrows only
ever change a value, and `Tab` no longer drops the focus — closing the trap where
the intended "go back" keystroke silently changed the server mode; before that —
**database compaction on backup/restore** (`VACUUM`: the archive
carries a compacted copy of `data.db` instead of the live file, sidecars folded
in; a restore compacts what it unpacked — including archives from older
versions), verified against the real 17 MB dev database; before that — the most
recent track — **full-text search over chat content**, now **complete**
([research](docs/research/chat-content-search.md),
[stage 2 plan](docs/history/chat-search-stage2.md)) — `Ctrl+F` in the chat list
switches its search from the title to message content, and `Ctrl+G` opens a
message-level results screen (hits grouped by chat, a snippet with the match
highlighted, `Enter` jumping the feed onto that exact message). Backed by a
**separate, disposable `cache.db`** (SQLite FTS5, trigram) kept in step in the
background: because the orchestrator is already the sole writer of chats, in-app
changes hook into `flush_saves` and only external ones need the startup pass,
which is a stat-only walk (~0.3 ms on the real corpus). Deleting the file is a
supported repair — it is derived data, so a version mismatch means "rebuild", not
a migration (ADR 0006 machinery not needed), and backup's allowlist excludes it
with no code change. Along the way the feed stopped being yanked to the bottom by
content arriving on its own — **done, stages 1–2; in-feed `/` search stays a
roadmap item, now cheap on stage 2a's jump infrastructure**;
before that — **the remote live e2e gate**, now **closed**
([docs/history/remote-e2e-hf.md](docs/history/remote-e2e-hf.md)) — the mandatory
live gate (AGENTS.md §3) stopped requiring one particular machine on one LAN
address: `tools/e2e_hf.py` rents a real `llama-server` (HF Inference Endpoints'
llama.cpp engine) plus **two** embedding models, runs the `#[ignore]` suite
against them and deletes them — verifying the deletion, since a leaked endpoint
is the one outcome that costs money. Triggered from CI by `workflow_dispatch`,
with an hourly sweeper as the backstop — **done, stages 0–3, two consecutive
green CI runs (66 passed / 0 failed)**;
before that — **periodic server health monitoring**
([docs/server-health-monitoring.md](docs/server-health-monitoring.md)) — the probe
stopped being one-shot: 60 s while healthy / 5 s while down, three consecutive
failures to go red and one success to come back, a dead managed child reported from
its exit signal in ~150 ms and relaunched under a crash-loop budget. The half users
feel is **recovery**: a server started after the app now becomes usable on its own,
where before generation stayed blocked until a restart — **done, live run GO**;
before that — a **readiness probe for the embedding server** (its status came from the
configuration alone, so a configured-but-unreachable server reported itself ready —
now all three servers are monitored alike) — **done, live run GO**;
before that —
**per-model input prefixes for embeddings**
([docs/research/embedding-input-prefixes.md](docs/research/embedding-input-prefixes.md)) —
the last groundwork item of the one below it: `Embedder::embed` gained an input
**role** (`Query`/`Passage`, no `Default` — a silent role is the failure the type
prevents), and a `PrefixedEmbedder` decorator sitting **inside** the model-change
guard marks each text per the model's convention (`none` by default / `e5` /
`e5-instruct`), so the canary and the calibration probes go through it and
switching the convention self-detects as the change of vector space it is.
Measured rather than assumed: on 40 documents the prefixes change **no ranking**
on e5 (the gain is a 25× wider *minimum* margin), and the **wrong** convention
costs bge-m3 a rank — hence the default off and no auto-selection, only a hint —
**done, live run GO**; before that —
the **embedding-model change** track
([docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md)):
stored vectors are only comparable to a query from the same model, and dimensionality
is **not** identity — `bge-m3` and `multilingual-e5-large-instruct` are both 1024-d and
pass every guard while living in different vector spaces; identity is established
**behaviourally** via a canary vector (stage 1, "detection and honesty"), and on a
change **nothing is deleted**: a monotonic **embedding generation** is bumped, so every
older vector reads as foreign — memory re-embeds itself on the next semantic path, while
the knowledge base — the user's data — is marked stale and `rag_search` refuses until
the new DB-global **`/reindex`** job re-embeds every stored vector **in place**, from
the text the DB already holds (batched, cancellable, resumable — stage 2, "re-embed in
place"); and because cosine distributions differ sharply per model (e5's usable range
is **2.6× narrower** than bge-m3's), the memory gates are no longer absolute constants
but positions in a reference scale, **calibrated automatically per model** on the same
once-per-model path and degrading to the identity when nothing is calibrated (stage 3,
"per-model thresholds") — **complete, stages 1–3, live runs GO**;
before that — **chat file attachments** (`/file attach|remove|list`: the file's text is
injected into the request's `system` on every turn — chat-scoped, no embedder, delivered
in full; a file over the budget switches to "by reference" instead of being refused;
plan [docs/file-attachments.md](docs/file-attachments.md)) — **complete, stages 1–3**
(stage 2 — `attachment_read`: a by-reference file is read page by page, the model
walks `1..M` and knows it read everything; stage 3 — `attachment_search` over a
chat-scoped semantic index: finding the right place by meaning in a big file, with
graceful degradation when no embedder is configured), **live runs GO**; before that — the **`mindfork.io` site URL in project metadata** (the registered domain now
also serves as the project homepage in the Windows installer, the Linux packages,
`Cargo.toml`, the README, `install.md` and the release-notes footer; every actionable
link stays on GitHub while the site is not up) — **done**, released as **0.9.4**;
before that — **beautifulsoup4 in the Python sandbox** (`sandbox setup` also installs
BeautifulSoup + soupsieve/typing_extensions — HTML parsing next to `requests`) —
**done**; before that — **custom role names** (the user/assistant names are editable in profile
settings and replace the localized `YOU`/`ASSISTANT` headers in the feed and the
`User:`/`Assistant:` labels in the `F5` export) — **done**; before that — **impersonation profiles** (the user
personas for `Ctrl+U` became a list of
their own with a name and system message each, referenced from the assistant profile;
plus a fix: a profile created with `Ctrl+N` in settings is now selectable right away) —
**done**; before that — **universal layout-independent hotkeys** (stage 1: on Windows the physical
key is resolved through the keyboard layout itself — every installed layout works,
not just Russian; research
[docs/research/layout-independent-hotkeys.md](docs/research/layout-independent-hotkeys.md),
decision points R1–R5 accepted 2026-07-24) — **done**; before that — the
**English source-language migration** (all docs/comments/internal
strings RU -> EN, the convention flipped and guarded in CI by
`tools/cyrillic_scan.py`; user-facing text stays localizable, `ru` remains a
supported locale) — **done**; before that — **chat message speech (TTS)** (the `/tts` command, OpenAI/
Gemini/external engines, a Markdown speech extractor, the `rodio` player; research
[docs/research/tts.md](docs/research/tts.md), decision 2026-07-23 — the primary engine
is OpenAI `gpt-4o-mini-tts`/`onyx`) — **done** (no local sidecar — future work);
before that — **API keys in settings** (entry in the settings window + machine-bound
encryption in the config; research
[docs/research/api-key-storage.md](docs/research/api-key-storage.md), decision points R1–R6
accepted 2026-07-20, result — **ADR 0008**) — **done** (stage 1 "core" + stage 2 "UI");
before that — **branding** (logo/wordmark: assets, `.exe` icon, Linux menu
entry, logo in help; plan [docs/branding.md](docs/branding.md)) — **done**;
even earlier — **the plugin system** — **done** (research
`docs/research/plugin-system.md`, decision points R1–R8 accepted 2026-07-17, result —
**ADR 0007**): **stage 1 "generic import"** (`feat/generic-import` — the neutral
`mindfork-import` format + `docs/import-format.md` + the `import` CLI,
`import-lamellama` removed — knowledge of the non-public LameLLaMA moves to a private
external converter); **stage 2 "MCP client probe" — GO** (`spike/mcp-client`:
a mini `shared/mcp.rs` client + a live smoke — Gemma 4 31B called a tool of the
real `server-filesystem` on the very first round, 14 tools ≈ 7 KiB of schemas,
protocol 2025-11-25); **stage 3 "MCP host"** (`feat/mcp-host`, parts 3a+3b:
client in the binary + `McpManager` + `McpTool` wrapper + `config.mcp` + gates/cancellation
+ TOFU catalog pinning + statuses/full descriptions in settings + i18n + spec §9.6;
live e2e through the orchestrator — GO); before that —
**Mermaid diagram rendering in the feed** (`feat/mermaid-render`, plan
`docs/research/mermaid-ascii-rendering.md`) — **done**: probe (NO-GO on
`mermaid-text` 0.56.0 — panic/garbled labels on Cyrillic) → **our upstream fix**
for multibyte handling (leboiko/markdown-reader#29 + PR #30 → release 0.56.1, "the shipped
implementation is substantially yours") → re-check with the probe (GO, 0 panics) →
implementation: flowchart/sequence whitelist, hard fallback to the source (a golden test
"worst case = previous behavior"), the `interface.render_mermaid` toggle
default-on, ASCII in compat mode; our feature request #32 (`max_width` as a hard
budget) was closed upstream in release 0.57.0 (`RenderOptions::max_width_strict` →
`Error::TooWide`) — we upgraded to 0.57.0, but do NOT use strict mode (our post-check
via `display_width` is more accurate); before that — **installers**
(Windows Inno Setup + Linux nfpm; plan `docs/history/installers.md`) —
**done** (stages 1–3; code signing deferred): a linear stage stack — stage 1
"in-code prerequisites" (`feat/installed-mode-prereqs`), stage 2 "Linux packages"
(`feat/linux-packages`), stage 3 "Windows installer" (`feat/windows-installer`);
even earlier — **release engineering** (versions/CHANGELOG/CI/data schema migrations) —
done; **i18n CLI** (all CLI text/`main.rs`/features/engine tail in bundles, own
parser instead of `clap`) — **done**, stages 1–3: branches `feat/cli-i18n-bootstrap` +
`feat/cli-i18n-features` + `feat/i18n-engine-tail`).
**The i18n track is completely done:** axis A (agent language, Tiers 1–2, all 35
tools), **Tier 3** (external `data/locales/*.json` layered over the built-in ones + new
languages without rebuilding — `Lang::Ext`, the `init(dir)` registry), axis B (interface
language, the entire UI chrome: status bar, feed, settings with all fields/groups/tool
labels, chat list, popups/help, "self-model", orchestrator errors and **displayed
supervisor reasons**, export; the "Interface language" field, independent of the agent
language). **i18n CLI is done** (stages 1–3): all CLI text (`main.rs`/help/parser instead
of `clap`), CLI features (`backup`/`sandbox_setup`/`migration`), and the engine tail
(`managed.rs` probe → status chip; `shared/sandbox.rs` → `python_exec` result/warmup) —
all in bundles.
The historical `#[ignore]` roundup was run against the live pair
**Gemma 4 31B (q4) + bge-m3** (`llama-server`, external, `--jinja`) — 25/25 green
(~370s): base Gemma smokes (streaming, EOS anti-self-cutoff, tool calling, "thoughts",
sampling extensions, control tools) + end-to-end self-model/notes/narrative tests
(duplicate gate, graph, auto-reflection/consolidation, semantic traits, cross-organ
links, RAG citation, narrative-as-notes). The multi-provider path (OpenAI/Gemini/Claude)
is covered by unit tests + separate `MINDFORK_ANTHROPIC_KEY` smokes.
Chat cycle, profiles with isolation, tools with a client-side agentic loop, sub-agent,
web search and Python behind switches, a settings screen with all sections and
a managed-server restart on model change, import from LameLLaMA (.NET), the help
overlay, a release profile, **themes (auto/dark/light)**, **legacy-terminal
compatibility mode** (conhost Windows 10: safe glyphs instead of emoji), **live
spellcheck driven by settings**, **Mermaid diagram rendering in the feed** (flowchart/sequence
as text graphics, a hard fallback to the source), **self/interlocutor "self-model"** and
**notes connectivity** (narrative as notes). The full post-M9 log is below.

> **Inference engine:** `xinfer` turned out raw for Gemma 4 (incoherent output,
> builds poorly on Windows). The working backend is **llama.cpp `llama-server`**
> (external, OpenAI protocol, `--jinja`). See [docs/install.md](docs/install.md) §3.
- **M0** — FSD scaffold, TUI loop with terminal restoration, single-instance, logging.
- **M1** — `shared/api` (xinfer client with streaming/cancellation, supervisor, thoughts
  parser, mock), orchestrator (state machine + generation_id), tokio↔TUI bridge, a minimal
  chat UI. History is in memory for now.
- **M2** — `entities` (all domain types), `shared/config`, `shared/storage`
  (atomic JSON + SQLite/sqlite-vec, per-profile isolation, soft delete + cascade).
  Storage is NOT yet wired to the orchestrator/UI.

### M3 (chat UI) — done so far
- **`[R]` UI crates closed** → [ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md):
  `tui-textarea`/`ratatui-markdown` lock in ratatui 0.29 → we take `tui-markdown` +
  `tui-scrollview`, and input is a **custom widget** (this also resolves the spellcheck
  rendering `[R]` — we draw the underlines ourselves).
- **`Storage` wired to the orchestrator**: multi-chat, bootstrap (default profile+chat),
  load/save with debounce (800ms), new `AppCommand`/`AppEvent`
  (`NewChat/SwitchChat/RenameChat/CloneChat/DeleteChat`, `ChatList/ChatActivated`).
  `ChatSummary` lives in `entities/chat.rs` (needed by both `app` and `widgets`).
- **`shared/markdown.rs`**: rendering via `tui-markdown` + a unicode approximation of LaTeX.
  **Replaced post-M9** with an own `pulldown-cmark`-based renderer (tables +
  delimiter-scoped LaTeX + theme) → [ADR 0003](docs/decisions/0003-own-markdown-renderer.md).
- **`features/chat_search_sort.rs`, `features/rename_chat.rs`**: pure logic.
- **`widgets/chat_list.rs`**: the `Ctrl+L` overlay (search, 2 sort orders via `Tab`,
  `F2` rename, `Ctrl+N` new, `Ctrl+D` clone, `Del` delete). Integrated into `runtime.rs`.
- **`widgets/input_box.rs`**: an own multiline input widget (`Vec<Vec<char>>`, cursor by
  character, scrolling; `Shift+Enter` for a line break, `Enter` to send). Replaced the
  placeholder single-line field.
- **`widgets/message_feed.rs`**: the feed with markdown rendering, a collapsible
  "thoughts" block (`Ctrl+T`), scrolling via `PageUp/PageDown` **and the mouse wheel**
  (`scroll_up`/`scroll_down`, the `Ctrl+W` capture toggle — see post-M9 below) with
  "tail-following".
- **Word wrap (`shared/wrap.rs`)**: lines are word-wrapped **before** `Paragraph`
  (a long word breaks by character; width in columns via `unicode-width` — Cyrillic
  = 1, emoji/CJK = 2). In `message_feed` this preserves row-based scrolling/"tail"; in
  `input_box` it gives an exact cursor mapping `(line, column)→(visual row, column)` and
  field-height growth under wrapping (`visual_line_count`). `Paragraph::wrap` is
  deliberately not used (it would break the scroll math and cursor position).
- **`widgets/status_bar.rs`**: a pure status widget (`ServerStatus` moved into `shared`).
- **`screens/chat.rs`**: `ChatScreen` — all UI state, mutators projecting events,
  `handle_key → ChatIntent`, `render`. `runtime.rs` is now a thin loop: it applies
  `AppEvent` via mutators, translates `ChatIntent → AppCommand`. **FSD is respected**:
  `screens`/`widgets` don't import `app` (the screen emits `ChatIntent`, not `AppCommand`).

- **`features/spellcheck/`**: `spellbook` (Hunspell) + an own word segmenter +
  a personal dictionary. `SpellChecker`: `check`/`misspellings`/`suggest`/
  `add_to_personal`. A word is correct if accepted by at least one dictionary.
  Dictionaries load **in the background** from `dictionaries/` (missing directory → the
  check is off, no crash). UI: underlining errors (`UNDERLINED`/red) with a 300ms
  debounce; a suggestions popup on `Ctrl+G` (word replacement / "add to dictionary" →
  `personal_dictionary.txt`).

### M4 (profiles + isolation) — done so far
- **`features/profiles.rs`**: pure profile operations (create/edit/
  name validation `sanitize_name`, `ProfileEdit`/`apply_edit`). UI section — at M8.
- **`Chat::from_profile` copies** `system_message`/`character_names` (the chat has its
  own copy — profile edits don't change them), but **does NOT copy** `default_sampling`.
- **Three-level sampling resolution** ([`sampling::resolve`], spec §8.3):
  `Chat.sampling_override → Profile.default_sampling → global` resolved **at request
  time** (not snapshotted at creation); a snapshot of what was applied goes into
  `Message.metadata`. `sampling_override` stays `None` until an explicit `set_sampling`
  (M5).
- **Contract**: `AppCommand::NewChat{profile_id}`, `CreateProfile`/`DeleteProfile`;
  `AppEvent::ProfileList`. `ProfileSummary` — in `entities/profile.rs`.
- **Orchestrator**: owns profiles, emits `ProfileList`, creates a chat from the selected
  profile (+ a greeting), `DeleteProfile → chats` cascade (`hide_profile_cascade`),
  a "can't delete the last profile" guard.
- **`widgets/profile_list.rs`**: the profile-picker overlay. `Ctrl+N` opens it when
  there's >1 profile, otherwise creates one right away. Wired in `screens/chat.rs` +
  `runtime.rs`.
- **Isolation** of notes/RAG by `profile_id` is confirmed by tests (db + `Storage`
  facade). The real `ToolContext` is an M5 deliverable (here `profile_id` is already
  available from the chat).

### M5 (tools + agentic loop) — done so far
- **`[R]` Embeddings closed** → [ADR 0002](docs/decisions/0002-embeddings-dedicated-server.md):
  a **dedicated** embedding server (the `Embedder` trait is split off from `EngineBackend`);
  `XinferClient: Embedder`; env `MINDFORK_EMBED_URL`/`_BIN`/`_MODEL`/`_PORT`;
  when not configured — `UnavailableEmbedder` (RAG returns an error, doesn't crash).
- **Engine: tool calls** — `ChatChunk::ToolCall(ToolCallDelta)` + `ToolCallAccumulator`,
  `ChatRequest.tools`/`tool_choice=auto`, serializing `assistant.tool_calls` into
  history and parsing `delta.tool_calls` (wire/client).
- **`features/tools/`**: the `Tool` trait, `ToolContext` (a snapshot), `ToolOutcome`+`ChatEffect`,
  `ToolRegistry` (`schemas_for` = profile ∩ registry, `invoke`). `standard_registry`/
  `default_tool_ids`. Tools: introspection (`get/set_sampling` with merge,
  `get/set_system_message`, `get_last_user_message_time`), notes (`note_save`/
  `note_recall`), RAG (`rag_add`/`rag_search`: chunking+embedding+kNN). All with
  `profile_id` isolation.
- **Orchestrator: client-side agentic loop** (spec §6.3): stream → `ToolCalls` →
  execution → a new request, up to `max_tool_rounds`; effects are applied by the
  orchestrator (owner of `Chat`) starting the next turn (§6.6). `Storage` → `Arc<Storage>`.
- **UI**: `AppEvent::ToolCall` → tool blocks (🔧 name/arguments/result) in the feed;
  tool messages aren't duplicated (shown as blocks inside the assistant's reply).
- Still pending: manual `#[ignore]` verification of the full agentic cycle against a
  real model (needs xinfer with tool calling), and the quality of the dedicated
  embedding model.

### M6 (`call_subagent`) — done so far
- **`features/tools/subagent.rs`**: `call_subagent` (spec §9.3.2) — an independent
  single-turn request via `ctx.engine`: `system` = as given, a single `user`
  = `message`, **no history, no tools** (`tools: []` → bans nesting/
  recursion). Token limit (≤1024) and a timeout (60s, cancellable via `CancellationToken`).
- Registered in `standard_registry`/`default_tool_ids`; in the UI — a regular
  tool block (name/arguments/result). Subject to `max_tool_rounds` (one round).

### M7 (web + Python) — done so far
- **`[R]` DDG endpoint closed**: `lite.duckduckgo.com/lite` (POST `q=`) — plain
  stable HTML. `reqwest` now with `rustls`+`form`; `scraper` added.
- **`features/tools/web.rs`**: `web_search` — **multi-provider with fallback**
  (`PROVIDERS`), each with its own infrastructure and markup: DDG lite
  (`a.result-link`/`td.result-snippet`) → DDG html (`a.result__a`/`a.result__snippet`)
  → **Mojeek** (`GET ?q=`, `a.title`/`p.s`) → **Ecosia** (`GET ?q=`, stable
  `data-test-id`; title and link are DIFFERENT tags, the parser aligns href/title/
  snippet by index). The first one with a non-empty result set wins; the real
  URL is decoded from `uddg=`. **Anti-bot throttling is detected** (`is_throttled`: DDG `HTTP 202`/
  the `anomaly` marker, Mojeek/others `403`/`429`) — previously a 202 passed as "success"
  (`error_for_status` lets 2xx through) and parsed to empty → the model saw "no
  results" for a valid query, and a 403 surfaced as a fatal "provider
  unavailable" error. Now on throttling the next provider is tried right away (throttling
  is sticky per-IP, retries only make it worse; providers throttle independently → almost
  always someone answers); if **all** are unavailable — an explicit error (not "no
  results"), so the model retries later. Request timeout 15s.
- **Content extraction + reranking (post-M9, done)**: after the results page
  arrives, results are fetched in parallel (`enrich_with_content` →
  `futures::join_all`, browser-like `Accept`/`Accept-Language` headers —
  otherwise some sites serve a block page); readable text is extracted from the HTML
  (`extract_readable` via `scraper`: paragraphs/lists `<p>`/`<li>` from `<article>`/`<main>`,
  otherwise from the whole document; **everything inside `nav`/`header`/`footer`/`aside` is
  dropped** — `in_boilerplate` walks ancestors, otherwise mega-menus leaked into
  content on sites without semantic markup; fragments < 40 chars are also filtered out;
  script/style outside `<p>` → dropped themselves; truncation to `MAX_CONTENT_CHARS=1500`).
  Fetch failures/non-2xx/empty extraction are logged to `logs/` (`tracing::debug`, for
  diagnostics). Results are then **reordered via embeddings**
  (`rerank_by_embeddings` through `ctx.embedder`, ADR 0002): the query + each result's
  `title`+content (≤`RERANK_EMBED_CHARS=800`) are embedded in one request,
  sorted by decreasing cosine similarity
  (`rerank_order`, stable — ties keep the provider order). All of this is
  **"best effort"**: one page's fetch failure just leaves its `content`
  empty (bot-hostile sites like Bloomberg return 403 — that's expected); if the embedder
  isn't configured/unavailable (RAG is off) or returned a mismatched vector count →
  reranking is skipped, provider order remains (graceful degradation, like RAG's).
  Output includes a "Content:" block with the extracted text.
- **`fetch_content` gate**: the `fetch_content` call argument (`false` → the fast path
  "titles/snippets only", no fetching/reranking) overrides the default from
  `config.tools.web_fetch_content` (default `true`). The default is threaded through
  `ToolConfig.web_fetch_content` → `WebSearch::new(bool)`; a "Web: fetch pages" toggle
  is in the "Tools" section of the settings screen (with a hint).
- **`features/tools/python.rs`**: `python_exec` — a separate process (`python -c`),
  10s timeout (kill_on_drop), output truncation. Interpreter path from config.
- **Global switches** (`config.tools`: `web_enabled=true`, `python_enabled=
  false`, `python_path`). The effective set = profile ∩ switches
  (`effective_tool_ids`); the agentic loop **also gates the call itself** (a disabled tool
  is rejected, not executed). Python is off by default on Windows.
- Real network/Python runs are `#[ignore]` (need network/interpreter).

### M8 (settings screen) — done so far
- **`AppConfig` extended** (`shared/config.rs`): `EmbedSettings` (the dedicated
  embedding server, ADR 0002), `ToolSettings.subagent_max_tokens/_timeout_secs`,
  `InterfaceSettings` (`Theme` auto/dark/light, `spellcheck_enabled`,
  `selected_dictionaries`). All through `#[serde(default)]` — old `settings.json`
  files load without migration.
- **`ToolConfig`** (`features/tools`): the registry is built from config
  (`standard_registry(&ToolConfig)`); `CallSubagent::new(max_tokens, timeout)` —
  limits aren't hardcoded; rebuilt on `config.tools` edits.
- **The orchestrator owns the whole `AppConfig`** and the servers via
  **`ServerSupervisor`** (`app/supervisor.rs`, real `XinferSupervisor` + mock):
  `apply_chat`/`apply_embed` bring up servers from config; `ServerHandle` lives in
  the orchestrator (`kill_on_drop`). `main.rs` no longer resolves the backend — it just
  loads the config and seeds it via env vars (`apply_env_overrides`, dev workflow).
- **Settings commands/event**: `AppEvent::Settings { config, profiles }` (a full
  snapshot); `AppCommand::UpdateConfig`/`UpdateProfile` — edits are persisted
  (`save_config`/`upsert_profile`), **switching `xinfer` restarts the server**, switching
  `tools` rebuilds the registry, switching `embed` re-raises the embedding server.
  An internal `ServerStatus` channel (background supervisor probe → `AppEvent`).
- **`screens/settings.rs`** (`SettingsScreen` + `SettingsIntent`, entry `Ctrl+P`):
  a left section menu (Model/Inference/Sampling/Profiles/Tools/Interface) +
  a right field list. `Tab` for section, `↑↓` for fields, `Space` toggle, `←→` enum,
  `Enter` a text/number editor. Edits apply **immediately on commit**
  (`SettingsIntent → AppCommand`). Profiles: select via `←→`, edit fields, tool
  toggles, `Ctrl+N`/`Ctrl+D` to create/delete. **FSD is strict**: `screens` doesn't
  import `app`.
- **`runtime`**: the settings screen sits over the chat; events keep applying to the chat
  (generation isn't interrupted); a `Settings` re-emit refreshes the working copy (create/
  delete of profiles is visible right away).
- Theme/keyboard layout in "Interface" are, for now, just **fields** (values are saved);
  actually applying the theme (`shared/theme.rs`) and custom layouts — planned for **M9**.
  Spellcheck on/off and dictionary selection are also editable, but actually
  applying them to dictionary loading — M9.

### M9 (polish) — done so far
- **LameLLaMA (.NET) importer** (`features/migration.rs`, spec §12.2): CLI
  `mindfork import-lamellama <dir>` (originally the `--import-lamellama` flag; later
  moved to a clap subcommand, see post-M9 backup/restore). `Settings.json` (`Configurations` → profiles
  with a deterministic UUIDv5 → idempotent) + `Conversations/*.json` → chats
  (keeps the original Id; `*.deleted` are skipped). Sampling with unsupported fields
  dropped, spellcheck/theme → `config.interface`. A UTF-8 BOM is stripped. **Verified on
  real data: 6 profiles, 226 chats.**
- **Crate-wide `#![allow(dead_code)]` removed**: dead code was deleted (fields
  `State`/`GenResult`, `shared/error.rs`, unused `is_empty`) or marked
  with a targeted `#[allow(dead_code)]`/`#[cfg(test)]` and a comment.
- **Chat file backup** (atomic write + `.bak`) finalized with a test (§12.3).
- **UX**: a keyboard-shortcut help overlay (`F1`/`?`), a status-bar hint.
- **Release**: `[profile.release]` (LTO/strip, `panic=unwind`); `docs/install.md`
  (Win/Linux build, portable data, xinfer managed/external + env, dictionaries,
  import, launch). `cargo build --release` builds.
- **Themes** (`shared/theme.rs`, branch `m9-themes`): a semantic `Palette`
  (roles user/assistant/tool/success/warning/error/accent) driven by
  `config.interface.theme` (auto = named ANSI; dark = bright; light = RGB-
  darkened). Threaded into widgets with real colors (`message_feed`,
  `status_bar`, `input_box`, `chat_list`); modifiers (dim/bold/reversed) are
  theme-independent. `ChatScreen` refreshes the palette from the `Settings` event.
- **Spellcheck obeys settings**: `dict::load(enabled, selected)` loads
  only the selected dictionaries (or nothing when disabled). `app/runtime.rs` owns the
  loading — `SpellLoader` reloads dictionaries in the background on changes to
  `interface.spellcheck_enabled`/`selected_dictionaries` (generation-guarded), so the
  toggle and dictionary selection apply live.
- **Layout-independent Ctrl shortcuts** (`shared/keys.rs`): a character is normalized
  to its "physical" Latin key (a Russian JCUKEN lookup table), so `Ctrl+L/N/G/T/C/P`
  work even on a Cyrillic layout (where crossterm reports `Ctrl+д` etc.).
  Applied in `screens/chat.rs`, `screens/settings.rs`, `widgets/chat_list.rs`.
  **The settings screen moved from `Ctrl+,` to `Ctrl+P`** (`Ctrl+,` on Windows
  is intercepted by Windows Terminal).

### M9 — Gemma check (done)
- **Gemma 4 E4B-it verified** against a live `llama-server` (llama.cpp). Added/
  extended `#[ignore]` smokes in [client.rs](src/shared/api/openai/client.rs):
  anti-self-cutoff for both families (`<|im_end|>` + `<end_of_turn>`), tool calling
  (`finish_reason=tool_calls` + parsing `delta.tool_calls`), "thoughts"
  (`reasoning_content` → `Thoughts`). All green. Nuance: Gemma reasoning "thinks"
  before calling a tool — smokes use a generous `max_tokens` (512).
- **`xinfer` doesn't work for Gemma 4** (too raw); the working path is external `llama-server`.

### Post-M9: full-screen chat list window + auto-title (done)
- **The chat list window (`Ctrl+L`) is now full-screen** (`widgets/chat_list.rs`):
  `render` draws a block spanning the whole `area` (previously a centered 60×70%
  popup); `centered_rect` removed.
- **Auto-title for a chat (`Ctrl+R` in the chat-list window)**: the model reads the
  conversation (or its start+end, if long) and comes up with a short title. Pure logic —
  `features/rename_chat.rs`: `build_conversation_digest` (tags roles, skips
  system/tool/empty ones, truncates the middle to a budget
  `TITLE_CONTEXT_BUDGET`), `TITLE_SYSTEM_MESSAGE`, `clean_generated_title` (strips
  quotes + `sanitize_title`). The request runs as a **background task** (`spawn_title`):
  an independent single-turn request with no history/tools, a 30s timeout; the result
  goes into an internal `title_tx` channel → `Orchestrator::handle_title_result`. Gated by
  server readiness (`ready_backend`); an empty chat → a clear error. Contract:
  `ChatListAction::AutoRename` → `ChatIntent::AutoRenameChat` →
  `AppCommand::AutoRenameChat`.
- **"Thoughts" for the title are suppressed through three layers + a guaranteed fallback.**
  For models with thinking "baked into" the GGUF (Gemma `peg-gemma4`), the server
  ignores the `thinking`/`reasoning_effort` fields, and the model spends **all**
  of `max_tokens` on reasoning → the resulting `content` is empty ("The model didn't
  return a title"; in the llama-server logs, `thinking = 1`, cut off by the token
  budget on reasoning). Fix: (1) a new `SamplingConfig.reasoning_budget=0` field (for
  llama.cpp built-in formats); (2) `wire.rs`, when `reasoning_budget=0`, also signals
  through `chat_template_kwargs.enable_thinking=false` (models' Jinja templates); (3) a
  generous `TITLE_MAX_TOKENS=2048`/60s timeout — if turning it off didn't work, the
  model still has time to **finish** "thinking" and produce a title in `content`; (4)
  `salvage_title_source` — if `content` still ends up empty, the title is taken from the
  last substantive line of `reasoning_content` (`Thoughts`), so the user no longer sees
  an error. For regular generation `reasoning_budget=None` — the fields aren't sent,
  behavior doesn't change.
- **A dedicated error area in the chat-list window** (`AppEvent::ChatListError`):
  list-operation errors (auto-title/delete/clone) go not into the chat feed (where they
  used to sit behind the full-screen overlay until a restart), but into a dedicated
  overlay status line (`ChatListState.error`, colored `palette.error`), which **fades on
  the first keypress**. `ChatScreen::set_overlay_error` routes into the overlay if it's
  open, otherwise (a late auto-title reply after the overlay closed) — as a note in the
  feed. Previously these errors went through the generic `AppEvent::Error`.
- **`AppEvent::ChatRenamed { id, title }`**: updates the active chat's feed header
  without a rebuild (previously manual renaming also didn't update the title until
  reactivation). Emitted from `handle_rename` and `handle_title_result`;
  `ChatScreen::rename_chat` applies it. The list/overlay update as before via
  `ChatList`. The shortcut is layout-independent (`shared/keys`, physical R = `Ctrl+к`).

### Post-M9: edit/regenerate the last reply (done)
- **Regenerate (`Ctrl+R`)**: truncates the chat history up to and including the last
  user message (the previous assistant reply + tool messages are dropped)
  and reruns generation with the same request. The shared send/regenerate path is
  factored into `Orchestrator::start_generation`; the feed is rebuilt via a re-emit of
  `ChatActivated`. Command `AppCommand::RegenerateLast`, intent
  `ChatIntent::RegenerateLast`.
- **Delete the last exchange (`Ctrl+E`)**: removes the last assistant reply
  together with the user message that triggered it; the user's text is restored into
  the input box (`AppEvent::RestoreInput` → `ChatScreen::restore_input`). If the
  input box isn't empty, the text is prepended to the existing content (nothing is lost).
  Command `AppCommand::DeleteLastExchange`, intent `ChatIntent::DeleteLastExchange`.
- Both operations are **gated by `Idle` state** (ignored during generation — both at the
  screen level and in the orchestrator) and are a no-op if the chat has no user
  message (e.g. a chat that only has a greeting). Shortcuts are layout-independent
  (`shared/keys`), added to the help overlay (`F1`/`?`).
- **Server-readiness gate**: the orchestrator holds `server_status` (updated from an
  immediate `ChatSetup.status` and a background probe). Send/regenerate only start when
  `Ready` — otherwise the request would go to a still-loading managed server and return
  `503` ("engine returned an error status"). The readiness check in regenerate runs
  **before** truncating the history (otherwise the old reply would be wiped out and the
  new one wouldn't arrive); if the send is rejected, the text is restored to the input
  box (`RestoreInput`), not lost. Not-ready is shown with a clear message ("Server is
  still connecting…"). `MockSupervisor` returns `Ready` synchronously — tests don't
  depend on the status race.
- **Readiness probe accounts for model loading** (`OpenAiClient::probe`): a managed
  `llama-server` binds the HTTP port right away, but for ~seconds (a large GGUF) responds
  `503 Loading model` to inference. The old probe hit `/usage` and treated any response
  as "ready" — `Ready` status was set before loading finished, and the very first request
  (send/regenerate) failed with `503` through the readiness gate. Not reproducible on
  external (the server is already loaded). Fix: the probe hits `/health` (at root, outside
  `/v1`) and treats `503` as "still loading" (not ready), `200` as ready, `404`
  (server without `/health`) as "alive, not loading" (ready). Now `server_status`
  becomes `Ready` only after the model is actually loaded, and the readiness gate
  correctly holds back send/regenerate with a clear message.
- The client no longer **swallows the error body**: status + JSON reason
  (`{"error":{"message":…}}`) are logged to file and included in the error text (truncated
  to 500 chars) — instead of the useless "engine returned an error status". It was exactly
  this body (`503 Loading model`) that pointed to the root cause above.

### Post-M9: mouse-wheel feed scrolling (done)
- **The mouse wheel scrolls the feed** on par with `PageUp/PageDown`. `ratatui::init()`
  doesn't enable mouse capture, so crossterm wasn't delivering wheel events — that's the
  crux of the fix. `runtime.rs` handles `Event::Mouse`, `ChatScreen::handle_mouse` maps
  `ScrollUp/ScrollDown` → `MessageFeed::scroll_up/scroll_down` (`WHEEL_SCROLL=3`
  lines); a no-op when an overlay/popup/help is open.
- **Mouse capture is a toggle, `Ctrl+W`** (**off** by default). Reason: the wheel and
  native text selection share the same terminal mouse-reporting mechanism —
  "wheel only, leave selection alone" can't be split out technically. Off → native
  mouse selection works; on → the wheel scrolls the feed, selection stays
  available with `Shift` held (Windows Terminal and most terminals support that).
  The toggle: `ChatIntent::SetMouseCapture(bool)` → `runtime::dispatch` sends
  `Enable/DisableMouseCapture` (a purely terminal side effect; the screen doesn't know
  about the terminal, per FSD). Capture is **always** released on exit and in the
  panic hook (the terminal doesn't stay in mouse mode after a panic). `Ctrl+M` is
  unusable for the toggle — the terminal reports it as `Enter`; `Ctrl+W` was chosen
  instead (W=wheel), layout-independent (`shared/keys`).
- The current mode is shown in the **status bar** (`mouse: scroll/select (Ctrl+W)`);
  added to the help overlay (`F1`/`?`). In "scroll" mode **the word "scroll"**
  is highlighted with the `accent` color (like markdown headings in the feed) —
  `Palette::hint_highlight_value` colors the value after the colon (the label with `:`
  stays dim; split by `:`, not by space — correct for
  multi-word values under localization); in "select" mode the whole description is
  dim.

### Post-M9: own markdown renderer (tables + LaTeX + theme) (done)
- **`shared/markdown.rs` was rewritten** from `tui-markdown` to an own walker over
  `pulldown-cmark` 0.13 events → [ADR 0003](docs/decisions/0003-own-markdown-renderer.md).
  Reason: `tui-markdown` didn't support tables and math and ignored the theme.
  Signature: `render(input, width, palette) -> Text<'static>` (`message_feed`
  passes the panel width and palette). The `tui-markdown` dependency is removed;
  direct `pulldown-cmark`, `syntect`, `ansi-to-tui` are added.
- **Theme**: colors (headings/links/list markers) come from `Palette` (previously the
  feed ignored dark/light). Code-block highlighting — `syntect` + `ansi-to-tui`,
  **the syntect theme is built from `Palette`** (`build_code_theme`): scopes → roles
  (keyword→accent, string→success, number→warning, function→user, type→assistant),
  text/comments — gray based on background lightness (the `Palette.dark` flag: Auto/Dark→dark,
  Light→light); named ANSI colors are converted to RGB (Campbell). Themes are cached by
  palette (`Box::leak` — there are only so many palettes, `HighlightLines<'static>`). It used
  to be hardcoded to `base16-ocean.dark`, disconnected from the theme (closes an ADR 0003
  gap).
- **Delimiter-scoped LaTeX** (modeled on the .NET `LaTeXConverter`): `normalize_delimiters`
  converts `\(…\)`→`$…$`, `\[…\]`→`$$…$$` (skipping code spans/blocks), a parser with
  `ENABLE_MATH` yields `InlineMath`/`DisplayMath`, and only their content goes through
  `latex_to_unicode` — the parser strips the dollar signs (no "$→$"), no false positives
  in prose/code. **Behavior change:** "bare" commands outside `$…$` (`\alpha`, `x^2`)
  are no longer converted. The converter is extended: `\frac{a}{b}`→`a/b`, `\sqrt{x}`→
  `√(x)`, `\pmod{n}`→`(mod n)`, text/font wrappers and accents
  (`\text/\mathrm/\mathbb/\vec/\hat/\overline/…`) → their content, operator-name
  functions (`\log/\sin/\cos/\lim/\max/\gcd/…`) → the word without `\` (the main
  gap fixed — `\log` in "O(n \log n)"), size modifiers for delimiters
  (`\left/\right/\big/\Big/…`) are stripped, spacing commands, `\{ \}` protection, dropping
  grouping braces, dozens of symbols.
- **Tables** (`ENABLE_TABLES`): cells accumulate in `TableBuilder`; on close —
  a box-drawing layout fit to the panel width. Column widths are "water-fill"
  (`fit_columns`): narrow ones keep their natural width, the rest split the remainder
  evenly with a readable minimum floor (`MIN_COL`/`MAX_MIN`). Cell content is word-wrapped
  (reuses `shared::wrap`), row height = the max rows among
  cells, alignment from markup, header in bold. If the minimums don't fit —
  natural width + horizontal clipping with "…". A table is guaranteed to be ≤ the width
  → a second wrap pass in `message_feed` is safe. Horizontal scroll instead of clipping — future work.

### Post-M9: repaint-on-change (cursor-blink fix) (done)
- **The `app/runtime.rs` loop only paints on change** (a `dirty` flag), not every tick.
  Previously `terminal.draw` was called unconditionally every iteration (~20/s,
  `TICK=50ms` period): whenever the input box was focused, every frame called
  `frame.set_cursor_position`, and `ratatui` after `draw` always sends "show +
  move cursor" even with an empty buffer diff. Windows Terminal resets the cursor's
  blink phase on every move → the cursor blinked more often and unevenly (CPU stayed
  ~0% since the diff was empty). Now `dirty` is raised on: an applied orchestrator
  event, a terminal event (input/mouse/**resize** — previously hidden by the
  unconditional repaint, now handled explicitly), a dictionary reload,
  a spellcheck-highlight recompute. There are no timer-driven animations in rendering, so
  idle ticks don't need to repaint.
- **The debounced spellcheck recheck was moved into the loop.** `maybe_recheck_spelling`
  (`screens/chat.rs`) — a deferred, timer-based action (300ms debounce): previously it
  was only invoked by `render`, and `render` ran every tick. With repaint-on-change,
  once the last keypress's events are gone → highlighting never appeared (visible
  only if dictionaries exist). Fix: the loop body still runs every tick anyway
  (the `poll` timeout), so it's the loop that calls `maybe_recheck_spelling` (now `pub`,
  returns `bool` — whether it recomputed), raising `dirty` **only** when the highlighting
  actually changed. So the loop, not render ticks, ensures the debounce wakeup, and there
  are no extra frames (with cursor repositioning) while idle.

### Post-M9: managed — preflight model-file check (done)
- **Symptom**: in managed mode, with a missing/inaccessible GGUF, the app would hang for
  a long time (up to the `MANAGED_READY_TIMEOUT=600s` timeout) in "server:
  connecting…" status. Cause: `ServerHandle::launch` (`shared/api/server.rs`) does a
  `spawn` of the child `llama-server`, and `spawn` **succeeds even without a model
  file** (it only fails if the binary itself is missing). `llama-server` then dies while
  loading and exits, but the background `wait_until_ready` probe doesn't notice and
  keeps polling the port pointlessly until the deadline.
- **Fix**: a preflight check in `ServerHandle::launch` — if `model_path` is set
  but `Path::is_file()` is false, immediately `bail!("model file not found or
  inaccessible: …")` **before** `spawn`. It fits neatly into the existing architecture:
  the supervisor (`app/supervisor.rs`) already maps a `launch` error → `ServerStatus::Disconnected(msg)`,
  and the status bar already shows it — no extra wiring needed. Covered by tests
  `launch_missing_model_file_errors_before_spawn` (no tokio runtime, checked before
  spawn) and `managed_with_missing_model_is_disconnected` (a real path → `Disconnected`).
- **Closed** (see below, "Post-M9: monitoring early `Child` exit in the probe"):
  a process that dies *while* loading (a corrupt GGUF, OOM) is now detected
  immediately by the probe, not by timeout.

### Post-M9: copy chat conversation to clipboard (done)
- **`F5` copies the whole chat conversation to the system clipboard** — both in the
  chat-list window (`Ctrl+L`, copies the selected chat), and in the main chat window
  (copies the active chat). `F5` (not `Ctrl+C`) — to avoid confusion with "quit" via
  `Ctrl+C` at the screen level; a function key doesn't depend on the layout (F1 — help, F2 —
  rename are already taken). In the main window `F5 → ChatIntent::CopyChat(active)`
  (the overlay intercepts `F5` earlier, so there's no conflict); a confirmation/error
  when the overlay is closed goes as a note into the feed (`set_overlay_notice/_error` → `push_*`).
- **Where the text is built**: the overlay only sees `ChatSummary` (title+count,
  no text), so the orchestrator — sole owner of `Chat` — assembles the conversation.
  A pure formatter `features/chat_export.rs::format_conversation(title, &messages)`:
  labels roles (`User`/`Assistant`), preserves multiline text,
  skips system/tool/empty messages, uses the chat's name as the title;
  `None` (no substantive messages) → an error "Nothing to copy".
- **Writing to the clipboard is a UI-layer side effect** (like the mouse toggle): the
  orchestrator emits `AppEvent::CopyToClipboard(text)`, `app/runtime.rs` writes via `arboard` (the
  `arboard` crate, `default-features = false` — text only, no image data). The client
  is created lazily and reused; on headless Linux without X11/Wayland the constructor
  may fail — then we show an error instead of panicking. Contract:
  `ChatListAction::Copy → ChatIntent::CopyChat → AppCommand::CopyChat`.
- **Confirmation/error** go into the same overlay status area as auto-title
  errors (`ChatListState.notice` — green `✓` for success, red `⚠` for
  error, mutually exclusive, fading on the first keypress). The overlay is **not**
  closed on copy. `ChatScreen::set_overlay_notice` routes into the overlay if it's open,
  otherwise — a note in the feed. Added to the help overlay (`F1`/`?`).

### Post-M9: `--no-mmap` flag + field hints in settings (done)
- **A `--no-mmap` setting for the managed server**: a new `EngineSettings.no_mmap`
  field (`shared/config.rs`, `#[serde(default)]` → old `settings.json` loads without
  migration) → `ManagedConfig.no_mmap` (`shared/api/server.rs`); `build_args` adds
  `--no-mmap` only when it's on. Threaded through the supervisor (`app/supervisor.rs`,
  `managed_config`); a toggle in the "Model/server" section of the settings screen. Loads
  model weights entirely into RAM instead of mmap — helps on network/slow disks, needs
  more memory (off by default).
- **Field description hints in settings**: `screens/settings.rs::field_description(id)`
  returns a human-readable text for the field; `render_fields` shows it as a line
  at the bottom of the section (a top divider line, `dim`, word-wrapped) when a field is
  focused — the space is reserved **only when the field has a description** (other sections
  as before). Currently described: `-ngl` (GPU layers), `--jinja`, `--no-mmap`; extended by
  adding one branch in `field_description`.

### Post-M9: loading/removing files in RAG via `/rag add|remove` commands (done)
- **Commands in the input box**: `/rag add <path>` indexes a file or directory into the
  active profile's knowledge base; `/rag add <path> -r` (or `--recursive`) — recurses;
  `/rag remove <path>` removes a file or directory (and everything under
  it) from the base. **The word `delete` is deliberately not supported** (users worried it
  would delete the file itself on disk) — only `remove`. Only
  `*.txt`/`*.md` are currently supported. Parsing is pure
  logic in `features/rag_command.rs::parse` (case-insensitive, a path with spaces and
  quotes, the flag in any position; a shared `extract_path`). The screen (`screens/chat.rs`),
  on `Enter`, first tries to parse a command: recognized → `ChatIntent::RagAdd { path,
  recursive }` / `ChatIntent::RagDelete { path }` (doesn't go out as a message, works even
  during generation — the operations are background/fast); a syntax error → a note-hint
  in the feed; otherwise a normal send.
- **Canonical source key + idempotency**: `source` in the DB is a canonical path
  (`rag_ingest::canonical_source` = `fs::canonicalize` without the `\\?\` verbatim
  prefix), the same whether added as a file, added as a folder, or deleted (robust
  to case/separator/relativity). **A repeated `/rag add` of the same file replaces**
  its previous chunks instead of duplicating them: `index_file` calls
  `Db::rag_delete_by_source` (an exact `source` match) before inserting.
- **Deletion** — `Db::rag_delete_under(profile_id, path)`: removes documents at an exact
  path and everything under it (a directory); the `norm_path` comparison is robust to
  `/`↔`\`, a trailing slash, and case (Windows); **doesn't require the file to exist on
  disk** (records of already-deleted files can be cleaned up). Removes from both
  `rag_documents` and `vec0` (`rag_vectors`) by rowid, strictly within the profile
  (isolation). The orchestrator (`handle_rag_delete`) does this **on the spot** (DB only,
  no embedding): if the path exists on disk — the canonical key, otherwise — the entered
  string (normalizing the DB); result — `RagProgress::Removed { chunks }` (0 → "nothing found").
- **Scanning** — `features/rag_ingest.rs::scan` (std::fs, testable on tempdir):
  a file path → itself when the extension is supported; a directory → all supported
  files (recursively with the flag), the result is sorted; nested-directory errors
  are skipped, an inaccessible root is an error. `read_text` strips a UTF-8 BOM.
- **Background indexing** — `app/orchestrator/rag.rs::spawn_rag_ingest` (a separate
  tokio task, doesn't block the orchestrator): scan → an embedder precheck (a fast
  bail-out if RAG isn't configured) → reads/chunks per file (reusing
  `features::tools::rag::chunk_text`, now `pub(crate)`)/embeds/writes with
  **isolation by `profile_id`** (the profile comes from the active chat). Cancellable
  (`rag_cancel: CancellationToken` on the orchestrator): a new indexing run cancels
  the previous one, `Quit` cancels the current one. `Storage` is thread-safe (an internal
  mutex), embedding is async.
- **Progress + spinner**: the `RagProgress` type (`Started/Indexing/Finished/Failed`)
  is defined in `features/rag_ingest.rs` — used by both `app` (emits
  `AppEvent::RagProgress`) and `screens` (renders it), without breaking FSD. The task
  sends progress via `evt_tx` directly (no internal channel — `Chat` state isn't
  touched). `ChatScreen::set_rag_progress` runs a banner line between the feed and the input
  (`files found: N` → `indexing file.md from dir (i/total)`) with a spinner
  (`⠋⠙⠹…`); completion/error clear the banner and leave a summary as a note in the feed.
  The `app/runtime.rs` loop repaints every tick while `screen.is_rag_active()` (the
  spinner animation) — outside of indexing, idle ticks don't repaint (the `dirty` flag,
  see the cursor-blink fix). Contract: `AppCommand::RagAdd → handle_rag_add`. The command
  was added to the help overlay (`F1`/`?`).
- **Command highlighting + no spellcheck**: if the input is recognized as a command
  (`rag_command::parse(...).is_some()`), the whole input box is colored `warning`
  (yellow), and spellcheck doesn't apply to it (commands and file paths aren't words).
  `InputBox::render` accepts a `command` flag; `ChatScreen::input_is_command` computes
  it, and `maybe_recheck_spelling` clears underlines and skips checking for commands.
- **Future work**: other formats (pdf/docx/html), readable-text extraction, `/rag list`
  (showing sources), per-chunk progress — for later.

### Post-M9: smart RAG chunking (overlap + markdown) + stitching on retrieval (done)
- **The chunker was rewritten** (`features/tools/rag.rs::chunk_text`): instead of
  "paragraphs + hard 800-char windows" (word-breaking, no overlap, spawning tiny
  chunks from a single line) — a packer following RAG best practices. Text is segmented
  into atomic units (`segment_units`: a whole paragraph if it fits the target; otherwise
  sentences via `split_sentences`; too-long ones get word windows via `break_long`, and
  a single gigantic word — by character), then `pack_units` packs units into chunks
  up to `CHUNK_TARGET_CHARS=800`, starting each next one with **the tail of the previous**
  (overlap ≤ `CHUNK_OVERLAP_CHARS=150`). Small neighboring paragraphs are grouped into
  one chunk; boundaries fall on words/sentences (never mid-word); `CHUNK_MAX_CHARS=
  1200` is the ceiling for an indivisible run. Lengths are counted in **characters** (`clen`),
  not bytes (Cyrillic). Both `rag_add` and background indexing use it.
- **Semantic markdown chunking** (`chunk_markdown`, for `.md` files): splits on
  ATX headings (`split_sections`, `is_atx_heading`), **protects fenced code
  blocks** (``` and `~~~` — a `#` inside them isn't a heading), prepends each section
  chunk with its heading as a **semantic anchor** (improves retrieval). Inside a section
  it's the same `pack_units` with overlap. A document with no headings → falls back to
  regular `chunk_text`. Dispatch by extension lives in `orchestrator::rag::index_file` (md →
  `chunk_markdown`, otherwise `chunk_text`); the `rag_add` tool (plain text, no format) —
  always `chunk_text`.
- **Stitching on retrieval** (`stitch_hits`, called from `RagSearch::invoke`): since
  overlap is baked in at chunking time, adjacent chunks from the same source have a
  **verbatim-matching** tail/head. `stitch_hits` groups hits by source and
  iteratively merges any two pieces with real overlap (`merge_overlap` →
  `overlap_len` finds the longest suffix of `a` that equals a prefix of `b`, ≥
  `MIN_STITCH_OVERLAP=24` chars, to avoid catching coincidence) into one contiguous
  fragment **with no duplication** — saves context and doesn't confuse the model with
  a repeat. A repeated markdown heading at the start of the second chunk is accounted for
  (stripped before matching, not duplicated). Fragments are ordered by best (minimum)
  distance. **No DB schema/entity changes** (deliberately no `seq` column added):
  adjacency is determined from the overlap text itself — the minimal edit that gives
  exactly the effect this project needs.
- **Future work**: ranking/dedup across sources, configurable chunk/overlap sizes.

### Post-M9: managed embedding server — raised the physical batch (fixes "chunks: 0") (done)
- **Symptom**: indexing a large file (e.g. `spec.md`) finished with "chunks:
  0, with errors: 1"; in the logs — `RAG: failed to index file … error=
  embeddings request returned an error status`.
- **Cause**: embedding models are **non-causal** — the whole input must fit into ONE
  physical batch (`n_ubatch`). By default `n_ubatch=512`, and llama-server
  equates `n_batch` to it (in the logs: *"setting n_batch = n_ubatch = 512"*).
  So **any chunk longer than ~512 tokens got the whole request rejected** ("input is too
  large to process. increase the physical batch size"). For Cyrillic/code, 512 tokens
  is only a few hundred characters, so `chunk_text`/`chunk_markdown` chunks (~800–1200
  chars) routinely didn't fit. The entire file's batch went as one request → one big error.
- **Fix**: the managed embedding server now launches with `-ub <ctx> -b <ctx>`
  (`server.rs::build_args` when `embeddings=true`, size = `context_size`, the embedder's
  `DEFAULT_CONTEXT_SIZE`=8192) — the physical and logical batch are raised to the
  context size, so an input chunk fits entirely in one ubatch. These flags aren't added
  for the chat server. Memory on embedding models (bge-m3 ~1.3 GB) grows negligibly.
  **Requires a managed-server restart** (happens on startup/when embedder settings change).
- **Diagnostics**: `OpenAiClient::embed` no longer **swallows the error body** (like
  `chat_stream` before it) — the status + JSON reason are logged and included in the
  error text (truncated to 500 chars). It was exactly this (and the llama-server log)
  that pointed to the root cause.
- **External embedder** (`MINDFORK_EMBED_URL`): we don't control the batch flags — if a
  third-party server is running with a small `n_ubatch`, a large chunk will still be
  rejected; now it's visible from the error text (rather than "chunks: 0"). Chunks are
  bounded by `CHUNK_MAX_CHARS` (≤ ~1200 tokens worst case), so they never exceed the
  embedder's context (8192).

### Post-M9: intuitive `Esc`/`Ctrl+C` for the chat list and quitting (done)
- **`Esc` toggles "chat list ↔ current chat"**: in the main view (when generation isn't
  running) `Esc` opens the full-screen chat-list overlay; `Esc` inside the overlay
  closes it again. During generation, `Esc` first **cancels it** (as before).
  Implementation: the `Esc` branch in `screens/chat.rs::handle_key`, instead of `Quit`, now
  sets `self.overlay = Some(ChatListState::new(...))`; closing goes through the regular
  `ChatListAction::Close`.
- **`Ctrl+C` — quits the app both from the chat and from the list overlay**: the chat
  already had `ChatIntent::Quit`; the overlay got `ChatListAction::Quit` (a `'c'`
  branch in `chat_list.rs::on_key_search`, layout-independent via `keys::physical_char`),
  `chat.rs::handle_overlay_key` maps it into `ChatIntent::Quit`.
- **`Ctrl+L` removed** (its role — opening the list — was taken over by `Esc`). Updated
  the help overlay (`HELP_KEYS`), the status-bar hint, the overlay's help line, and the
  docs (README/spec/install). Tests were rewritten for `Esc`-opens-overlay.

### Post-M9: fast multiline clipboard paste (done)
- **Symptom**: a large clipboard paste lagged in Windows Terminal, and a line break
  inside it was treated as `Enter` (= send the message). **Cause**: the terminal sent
  the paste **character by character** as regular keypresses, so the `run_loop`
  loop did a `terminal.draw` per character (slow), and `\n`/`\r` arrived as
  `KeyCode::Enter` → send.
- **A key nuance — bracketed paste does NOT work on Windows**: `Event::Paste` in
  crossterm 0.29 is emitted **only by the unix parser** (`src/event/sys/unix/parse.rs`);
  on Windows input is read via the Console API (`ReadConsoleInput`), and bracketed
  paste mode (`ESC[?2004h`) is useless — paste events arrive as regular `KeyEvent`s
  (interleaved with `KeyEventKind::Release`, at that). So a bare `EnableBracketedPaste`
  isn't enough — batching is needed in the loop.
- **Fix — batching events in `run_loop`** (`app/runtime.rs`): in one tick we drain
  **all** available events (`event::poll(Duration::ZERO)` in a loop), then
  `process_input_batch` coalesces consecutive "text" keypresses into a paste. This
  fixes both problems: one repaint per batch (not per character), and `Enter`
  **inside a run** → a line break, not a send.
  - `collect_press` drops "release"/repeat key events (the app ignores them anyway,
    and they would break up the character run on Windows).
  - `paste_char`: a text key without Ctrl/Alt → a character (`Enter→'\r'`, `Tab→'\t'`,
    `Char(c)→c`); everything else (arrows, shortcuts) breaks the run.
  - `chunk_batch` (pure, testable): a run of text keys of length **≥2** →
    `Chunk::Paste(String)`; a run of 1 (regular typing / a single `Enter`) — a regular
    event, so `Enter`-sends aren't broken. A human physically can't type 2+
    keys within one zero-timeout drain — so ≥2 reliably means a paste.
  - On unix `EnableBracketedPaste` is kept (under `#[cfg(unix)]`) — there a clean
    `Event::Paste` arrives, which the same `process_input_batch` handles as a `Chunk`.
- **Collecting the paste "tail" across console-chunk boundaries** (`PASTE_BURST`/`PASTE_GAP`):
  a large paste (thousands of characters) arrives in the Windows console buffer in
  **several chunks**, and the loop drains them over different iterations. At a chunk
  boundary the character run broke → if a chunk happened to end exactly on a lone
  `Enter`, it slipped through as a send (a bug: ~6900 characters went in as a paste, then
  one `Enter` fired as a send). Fix: if a single zero-timeout drain gathered a burst
  (`batch.len() ≥ PASTE_BURST=2`), we chase the paste's tail via
  `while event::poll(PASTE_GAP=20ms)` — while events keep arriving less than 20ms
  apart, we treat them as one paste (real human typing has pauses >>20ms). This way the
  whole paste is collected into ONE batch with no internal boundaries, and no more
  lone `Enter`s appear on the seams.
- **`InputBox::insert_str(&str)`** (`widgets/input_box.rs`): insertion at the cursor
  position in one pass (the line tail is cut off, the text is split on `\n` into
  logical lines, the tail is glued to the last one, the cursor lands at the end of what
  was inserted). `normalize_paste`: `\r\n`/`\r` → `\n` (so CRLF from the clipboard — whether
  it's `Enter`+`\n` or `\r\n` — collapses into one line break), `\t` → 4 spaces. No
  per-character loop → a large paste doesn't lag.
- **Routing**: a paste (coalesced or unix `Paste`) → on the settings screen goes to the
  active field editor (`SettingsScreen::handle_paste` → `editor.input.insert_str`,
  useful for the model path), otherwise — `ChatScreen::handle_paste` → the chat's input
  box. In the chat it respects modality (help/suggestions popup/overlays with single-line
  fields → no-op) and **never sends** a message, even with line breaks inside;
  `mark_input_changed` kicks off the spellcheck debounce. Added to the help overlay
  (`F1`/`?`, `Ctrl+V`).

### Post-M9: persisting the input-box draft in the chat file (done)
- **Unsaved input-box text is stored on the chat and restored on
  switching**: a new `Chat.draft` field (`entities/chat.rs`, `#[serde(default)]` →
  old chat files load without migration). Empty on a new chat; cleared on send.
  Switching to/creating a chat loads its draft into the input box (a new one →
  empty).
- **Contract**: an `AppCommand::SetDraft(String)` command (UI → orchestrator) and a
  `draft` field in the `AppEvent::ChatActivated` event. The orchestrator is the sole
  writer of `Chat`: `handle_set_draft` writes the active chat's draft and marks it dirty
  (`mark_dirty`, disk write with an 800ms debounce), **without touching `modified_at`**
  (editing a draft shouldn't bump the chat up the list); `handle_send` clears `chat.draft`
  along with appending the user message.
- **UI**: `ChatScreen::mark_input_changed` (called on any input edit — typing,
  pasting, restoring, suggestions) now also raises `draft_dirty`. The `app/runtime.rs`
  loop, every tick, picks up the changed draft (`take_dirty_draft`) and sends
  `SetDraft` (no repaint — the view doesn't change). `activate_chat` loads `draft` into
  the input box via `set_text`, but **deliberately doesn't set `draft_dirty`** (otherwise
  it would immediately send the same text right back via the same `SetDraft`) — it
  triggers a spellcheck recheck directly instead. See spec §11.7.

### Post-M9: persisting deleted exchanges in the chat file (`Ctrl+E`/`Ctrl+R`) (done)
- **Deleting an exchange (`Ctrl+E`) and regenerating (`Ctrl+R`) are irreversible in the
  UI, but what's deleted is saved to disk** for **manual** recovery by editing JSON in
  rare cases. A new `DeletedExchange { deleted_at, messages, draft }` type and a
  `Chat.deleted: Vec<DeletedExchange>` field (`entities/chat.rs`, `#[serde(default,
  skip_serializing_if = "Vec::is_empty")]` → old files load without migration, an
  empty collection doesn't clutter the JSON). This is **not** a message but a
  container: the deleted messages + the input-box draft at the time of deletion + the
  date.
- **What goes in**: `Ctrl+E` (`handle_delete_last`) — the user message and the
  assistant's reply (`split_off(idx)`) + `chat.draft` **before** the user's text is
  restored into the input box; `Ctrl+R` (`handle_regenerate`) — the assistant's reply
  and the round's tool messages (`split_off(idx+1)`) + `chat.draft`. A pure
  `Chat::record_deleted(messages, draft)` method sets `deleted_at = Utc::now()`, ignores
  an empty set, and inserts the entry at the **front** of the collection (recent
  deletions are faster to find), without touching `modified_at` (the truncation
  operations themselves update that). There's no UI restore. See spec §11.7.

### Post-M9: `↑/↓` navigation by visual row of a wrapped line (done)
- **The `↑/↓` arrows in the input box now move by visual row**, not by logical
  lines: if a long line wraps across several rows, `↑/↓` move the
  cursor between its parts (previously they'd jump across a whole logical line to the
  neighboring one). The column is preserved where possible. **Root cause of the bug**:
  `move_up`/`move_down` (`widgets/input_box.rs`) worked in terms of `self.row` (a
  logical line), while wrapping into visual rows (`wrap::wrap_ranges`) is only computed
  at render time against the width `view_w` — `on_key` has no width available.
- **Fix**: the widget caches the width of the last render (`InputBox.last_width`,
  set in `render`); `move_up`/`move_down` build visual rows
  (`visual_rows`) at that width, take the cursor's visual position (`cursor_visual`)
  and move it to the neighboring row via `col_for_visual` (the nearest column by
  width in columns, stepping one character back on a soft wrap — otherwise a
  position `== end` would land at the start of the next row, `is_soft`). Before the
  first render (`last_width == 0`) — a fallback to the logical transition (`move_up_logical`/
  `move_down_logical`). `↑` on the top visual row / `↓` on the bottom one — a no-op.
  Rendering in the loop happens before key handling, so the width is always current.
- **`Home`/`End` — by visual row**: `Home` → the start of the current visual row,
  `End` → its end (`move_home`/`move_end`). On a soft wrap, `End` lands on the
  last position of the row (via the same `col_for_visual`/`is_soft`), not sliding into
  the start of the next one. Before the first render — the whole logical line (fallback).
- **Goal column**: a run of `↑/↓` keeps the original column (`InputBox.goal_col`,
  in columns) when passing through short rows — like in large editors.
  Remembered on the first vertical move, reset by **any** other cursor
  move/edit (`move_left`/`move_right`/`move_home`/`move_end`, `insert_char`/
  `insert_newline`/`insert_str`/`backspace`/`delete`, `replace_range`/`set_text`/
  `clear` — reset inside those methods themselves, to also cover paths that bypass
  `on_key`: pasting, suggestions, draft loading). See spec §11.5.

### Post-M9: single-line settings fields + system-message popup (done)
- **Symptom**: editable fields on the settings screen looked single-line, but
  underneath them lived a multiline `InputBox` with word wrapping: a long value
  (a GGUF path, a URL) would wrap onto an **invisible** row (a height-3 popup → only 1
  line visible), and `↑/↓` moved by visual row and `Home/End` — by visual row, not
  across the whole value (see the post-M9 visual-row navigation section above — that's
  exactly what "broke" single-line behavior here).
- **`InputBox` single-line mode** (`widgets/input_box.rs`): a `single_line` flag +
  `set_single_line()` (off by default — chat input stays as before, multiline). In
  this mode the value **doesn't wrap**, it scrolls **horizontally** (`hscroll`,
  a separate `render_single_line` render path + a `col_at_width` helper): the cursor is
  always visible, a long value "slides" left. `↑/↓` — a no-op; `Home/End` — to the
  start/end **of the whole value**; a line break (`insert_newline`) is forbidden; line
  breaks in `set_text`/`insert_str` (a clipboard paste) collapse into a space (the
  "one logical line" invariant). `clear`/`set_text` reset `hscroll`.
- **The settings screen** (`screens/settings.rs`): the field editor opens in
  single-line mode for all text fields; **the exception is the profile's system
  message** (`FieldId::PSystem`) — multiline by nature. For it the editor is
  multiline (`set_single_line(false)`), inside a **large popup** (~80% width, ~60%
  height — `multiline_popup_height`) with long-line wrapping; `Shift+Enter`
  inserts a line break, `Enter` commits, `Esc` cancels (as in chat input).
  `Editor.multiline` drives the key/render branching. See spec §11.6.

### Post-M9: impersonation — writing a message on the user's behalf (`Ctrl+U`) (done)
- **Overview** (spec §11.8): on `Ctrl+U` the model writes the next message **on behalf
  of the user** into the input box. The request is built as follows: the chat's system
  message (the assistant's persona) is replaced with the **impersonation** one from the
  profile (`Profile.impersonation_system_message`; empty → `DEFAULT_IMPERSONATION_SYSTEM_
  MESSAGE`), the user↔assistant roles in history are swapped (`swap_role_message`;
  system/tool/empty messages are dropped — there are no tools in this mode), sampling is
  `AppConfig.impersonation_sampling`. Pure `build_impersonation_request`/
  `swap_role_message` functions in `app/orchestrator/impersonation.rs`.
- **Reasoning is forcibly disabled** (important): `build_impersonation_request`
  forces `thinking=false`, `reasoning_effort=None`, and critically `reasoning_budget=0`
  on top of the user's `impersonation_sampling`. Impersonation discards "thoughts"
  (`Thoughts` are ignored in `spawn_impersonation`), so it doesn't need reasoning.
  For models with thinking "baked into" the template (Gemma `peg-gemma4`, Qwen), the
  server ignores the `thinking`/`reasoning_effort` fields — only `reasoning_budget=0`
  (+ `chat_template_kwargs.enable_thinking=false` in `wire.rs`) actually suppresses
  "thoughts". Without this the model would spend its whole token budget on
  `reasoning_content`, and the reply's `content` would come back empty → the preview
  stayed empty (the same bug class as auto-title, see `title.rs`).
- **Three server modes** (`ImpersonationMode` in `shared/config.rs`):
  `shared` (default) — the same chat server as the assistant's, but with the
  impersonation sampling (no separate server is raised); `managed` — a separate child
  `llama-server`; `external` — a separate remote server. Settings —
  `AppConfig.impersonation_engine: ImpersonationEngineSettings` (fields like
  `EngineSettings`, but a three-way mode). Supervisor: `ServerSupervisor::
  apply_impersonation` (the real one raises managed/external + probes; not
  called for `shared` — the orchestrator uses the assistant's `backend`). The
  orchestrator holds `imp_backend`/`imp_handle`/`imp_status` and re-raises it on a
  change to `impersonation_engine` (`apply_impersonation_settings`).
- **Flow** (a background task, like auto-title): `AppCommand::Impersonate { seed }`
  → `handle_impersonate` (gated by `State::Idle` + no active impersonation + server
  readiness via `impersonation_backend_if_ready`) → `spawn_impersonation` streams
  `AppEvent::ImpersonationChunk`, and on end/timeout/cancel sends `(id, reason)` into
  an internal `imp_done` channel → `handle_imp_done` emits `ImpersonationFinished`.
  `AppCommand::CancelImpersonation` (`Esc`) cancels the token; so does `Quit`.
  `IMPERSONATION_TIMEOUT=600s` (a safety net against a stuck task; generous — on a slow
  local `llama-server` just prompt processing can take ~a minute, CPU generation is
  ~5 tok/s). "Thoughts"/tool calls in the input aren't shown.
- **Timeout ≠ user cancellation** (important, fixes "the reply cuts off and
  disappears"): a cancel via `Esc` arrives as `Ok(Ok(Cancelled))` (the stream inside
  `run` itself catches `cancel.cancelled()` and returns `Finished(Cancelled)`) and
  discards the text; a timeout is the `Err(_)` branch: we abort the server task
  (`cancel.cancel()`), but **keep** the accumulated text, returning it as `Length`. The
  old unconditional `if cancel.is_cancelled() { Cancelled }` collapsed both cases into
  discarding — which is why a timeout-truncated reply used to vanish.
- **UI** (`screens/chat.rs` + `widgets/impersonation_preview.rs`): while it's being
  written, the input box is **hidden**, replaced with a non-editable **streaming
  preview** with a spinner (`ImpersonationState`: text starts with whatever seed text
  was already typed). The accumulated text is inserted into the input box (`set_text`)
  on any completion **except an explicit cancel** (`Cancelled` — `Esc`): then the field
  keeps just the seed. A reply truncated by timeout (`Length`) or interrupted by a
  stream error (`Error`) **isn't lost** — partial text is more useful than an empty
  field. During impersonation only `Esc` (cancel) and `Ctrl+C` (quit) are handled. The
  `app/runtime.rs` loop repaints every tick while `is_impersonating()` (spinner
  animation). `Ctrl+U` is layout-independent (`shared/keys`), added to the help
  overlay (`F1`/`?`).
- **Continuing something already started**: if the input box isn't empty, the text
  goes in as `seed` — `build_impersonation_request` adds a request to the system
  message to continue what's started (output only the continuation), and the preview
  shows the seed + what's generated (final result = seed + continuation).
- **Settings** (`screens/settings.rs`): the "Model/server", "Sampling", and
  "Profiles" sections got a **subsection selector** for "Assistant"/"Impersonation"
  (`Subsection`, fields `model_sub`/`sampling_sub`/`profile_sub`; a new `FieldId::*Sub`).
  The Model impersonation subsection — the same fields + a three-way mode
  (`IxMode`, `cycle_imp_mode`); Sampling — the same fields over
  `impersonation_sampling`; Profile — **only the system message**
  (`PImpSystem`, a multiline editor), no tools. `ProfileEdit.
  impersonation_system_message` carries the edit.

### Post-M9: inline tool blocks in the feed + call-text wrapping (done)
- **Tool blocks are drawn at the call site, not in a "header"** (`widgets/message_feed.rs`):
  `FeedToolCall` got a `text_offset` field (a byte offset into `FeedMessage::
  text` — how much of the reply text had been generated BEFORE the call). `build_lines`
  for the assistant (`push_assistant_body`) splits the text by calls' `text_offset` into
  markdown fragments and inserts the tool block between them: `text-before-the-call → 🔧 block →
  text-after`. Previously `push_tools` ran BEFORE the body — all calls hung above the reply.
- **Call arguments and results are wrapped, not truncated**: `truncate`/
  `.take(6)` removed; `push_tool` via a new `push_wrapped` (gutter prefixes `  🔧 `/`  │ `,
  wrapping to visual width via `shared::wrap`, continuation alignment) lays out long
  `arguments`/`result` across several rows in full. The feed already does a second
  `wrap::wrap_line` pass, but lines are already ≤ the width → a no-op.
- **Agentic-loop round stitching** (`FeedMessage::from_messages`): on chat reload,
  consecutive round assistant messages (with the in-between tool
  messages in history dropped) merge into one "Assistant:" block with inline calls;
  each round's `text_offset` is shifted by the accumulated length.
  `screens/chat.rs::push_tool_call` sets `text_offset = last.text.len()`;
  `activate_chat` builds the feed via `from_messages`.
- **The live stream matches a reload byte-for-byte**: round texts in live mode used
  to be concatenated with no separator, while `from_messages` inserts `\n\n`
  (thoughts — `\n`). Flags `pending_text_sep`/`pending_thoughts_sep` on `ChatScreen`
  are set when a tool is called and consumed by the very first text/thoughts chunk of
  the next round, inserting the same separator (if the accumulated text is non-empty).
  Reset in `begin_generation`. Covered by a test
  `live_stream_with_tool_matches_reload` (live == `from_messages`).

### Post-M9: sampling "for variety" — dynatemp / adaptive-p / DRY breakers / sampler order (done)
- **Four new `SamplingConfig` fields** (llama.cpp request-body extensions, for
  livelier and less predictable replies): **dynamic temperature**
  `dynatemp_range`/`dynatemp_exponent` (temperature adapts to the distribution's entropy
  at each token), **adaptive-p** `adaptive_target`/`adaptive_decay`
  (a new sampler, llama.cpp PR #17927), **DRY breakers** `dry_sequence_breakers`
  (`Option<Vec<String>>`), and a configurable **sampler order** `samplers`
  (`Option<Vec<String>>`). Every field is `Option`, sent only when set.
- **List fields are JSON arrays, never sent empty** (`wire.rs::build_chat_request`,
  the `non_empty` helper): an empty `samplers` would be interpreted by the server as
  "disable all samplers", and an empty `dry_sequence_breakers` is **flatly rejected by
  llama-server** (`server-schema.cpp:238` throws `"must be a non-empty array of strings"`)
  — the filter prevents this.
- **Keys were verified against the llama.cpp source** (`tools/server/server-schema.cpp`,
  where body parsing moved to in recent versions from `server.cpp`): `dynatemp_range`/`_exponent`
  (:113), `adaptive_target` (:160, a flat key, `≤1.0`, negative=off),
  `adaptive_decay` (:164, hard range `0.0–0.99`), `dry_sequence_breakers` (:234),
  `samplers` (:474, an array of names or a string). Valid sampler names
  (`sampling.cpp`): `penalties`, `dry`, `top_k`, `top_p`, `top_n_sigma`, `typ_p`,
  `min_p`, `xtc`, `temperature`, `infill` (mirostat is a separate mode, not in the list).
- **UI** (`screens/settings.rs`, "Sampling" section): number fields as usual;
  `samplers` is edited as text via `;`, `dry_sequence_breakers` — via `,` with
  `\n`/`\t`/`\r` escapes (a single-line editor can't take them literally;
  `parse_breakers`/`join_breakers`/`decode_escapes`/`encode_escapes`). Also editable
  via the `set_sampling` tool (a schema + merging all the fields).
- **Tests**: wire serialization of the new fields + omitting empty arrays
  (`list_fields_sent_as_arrays_and_empty_omitted`), round-trip of lists and escapes
  (`samplers_list_round_trip`, `dry_breakers_decode_and_encode_escapes`), merging in
  `set_sampling`. **A live `#[ignore]` smoke** `accepts_creative_sampling_extensions`
  (`client.rs`) — all four fields in one request; checks the **combined** stream of
  `text`+`thoughts` (Gemma, a reasoning model, puts the answer in `reasoning_content` —
  the reasoning-budget trap). Verified on a live `llama-server` (Gemma 4 12B).

### Post-M9: clearing the input box with undo (`Ctrl+K`) (done)
- **`Ctrl+K` deletes all text in the input box**; pressing it again **restores what
  was deleted**, provided nothing was typed after the deletion (a toggle). The logic
  lives in the `InputBox` widget itself (`widgets/input_box.rs::clear_or_restore`): a
  `cleared: Option<String>` buffer holds the deleted text; a non-empty field →
  remember and clear it; an empty field with a live buffer → restore it (cursor at
  the end). **Any text input** (`insert_char`/`insert_newline`/`insert_str`/`replace_range`/
  `set_text`, and also an explicit `clear()` on send) invalidates the buffer to
  `None`, so it can only be restored right after the deletion; cursor movement and
  no-op backspace/delete on an empty field don't touch the buffer (restore survives
  them).
- **Wiring**: `screens/chat.rs::handle_key` — a `'k'` branch in the Ctrl match
  (layout-independent via `shared/keys`, physical K = `Ctrl+л`) calls
  `input.clear_or_restore()` + `mark_input_changed()` (draft persistence +
  a spellcheck recheck). No orchestrator command needed (purely local
  input state). Added to the help overlay (`F1`/`?`) and the keybinding table in spec
  §11.5.

### Post-M9: the generation state machine split out of the orchestrator (`GenState`) (done)
- **The generation state was moved** out of `app/orchestrator.rs` into a separate
  `app/gen_state.rs` module — a `GenState` type (`Idle`/`Generating{id,cancel}`/
  `Cancelling{id}`). Implementation choice — **a plain enum with transition methods**,
  not `statig`/`rust-fsm`: states carry data (`Uuid` + `CancellationToken`), which
  those frameworks don't fit well (rust-fsm is for state-less FSMs; statig would be an
  HSM overkill for 3 states), and the transitions are entangled with side effects
  (spawning tasks, cancelling the token, emitting events, writing to `Chat`), which
  need to stay with the orchestrator — the owner of `Chat`. No new dependencies added.
- **Transitions are encapsulated in methods**, correct by construction: `begin(id,cancel)`
  (`Idle→Generating`, a no-op if busy), `request_cancel()` (`Generating→Cancelling`,
  returns the token — the orchestrator actually triggers the cancellation), `finish(id)`
  (`→Idle` only on matching `id`, anti-staleness against `Stop→Send`), `active_cancel()`
  (a token for `Quit` with no transition), `current_id()`/`is_idle()`. The type is
  **pure** (no I/O) → unit-testable without a tokio runtime (5 tests in `gen_state.rs`).
- **The orchestrator got thinner**: the transition invariant is no longer smeared
  across `match` sites (`handle_send`/`handle_regenerate`/`handle_delete_last`/`handle_impersonate`
  gate through `is_idle()`; `Cancel`/`handle_switch` — `request_cancel()`; `Quit` —
  `active_cancel()`; `start_generation` — `begin()`; `handle_done` — `finish()`).
  The `state` field was renamed to `gen_state` (`gen` is a reserved word in edition
  2024). Impersonation (`imp_gen`) and RAG (`rag_cancel`) were **deliberately** not
  folded in — those are concurrent sub-states (RAG runs in parallel with generation).

### Post-M9: the orchestrator god object split by feature (done)
- **`app/orchestrator.rs` (≈1.8k lines of code + ≈1.5k of tests) was split into
  the `app/orchestrator/` directory** — a purely mechanical refactor (Phases 1+2),
  with no change to `Orchestrator`'s structure/fields and no new channels: the
  **"sole owner of `Chat`" invariant is preserved** (see architecture.md §1, §10).
  Only `run`/`OrchestratorDeps` still stick out of the module (`main.rs` untouched).
- **The skeleton — `mod.rs`** (≈450 lines): `struct Orchestrator`, the `run()` loop with
  `select!`, the `handle_command` dispatcher, `bootstrap`, and shared helpers (emitters
  `emit_chat_list/profile_list/settings`, `chat_mut`, `effective_sampling`,
  `activate`, `mark_dirty`, `flush_saves`, `backend_if_ready`, `new_chat_value`).
  Command handlers — separate `impl Orchestrator` blocks in per-feature files:
  `generation.rs` (send/regenerate/delete_last/done + the agentic-loop task),
  `chats.rs`, `profiles.rs`, `settings.rs` (+apply_*), `title.rs`, `impersonation.rs`,
  `rag.rs`; domain↔engine mapping — `request.rs`. All tests (411 of them) — in `tests.rs`.
- **Visibility**: methods called from the dispatcher/another file, and internal-channel
  types (`GenResult`, `TitleResult`), are marked `pub(super)`; the shared
  helpers stayed private in `mod.rs` (child modules see the parent's private items
  under Rust's visibility rule). Child modules import exactly their own
  dependencies (no `use super::*` in non-test code → no "dangling" imports under
  `-D warnings`). Background tasks (`spawn_generation/_title/_impersonation/
  _rag_ingest`) are private free functions in their own feature files.
- **Phase 3** (extracting cohesive sub-structures) — **done as a separate PR**, see below.

### Post-M9: Phase 3 — extracted the EngineManager / SaveQueue sub-structures (done)
- **`Orchestrator` was shrunk from 27 to 16 fields** by extracting two cohesive
  units (no behavior change, no new channels; the "sole owner of `Chat`" invariant
  intact):
  - **`EngineManager`** (`app/orchestrator/engines.rs`) — the lifecycle of the
    inference/embedding servers: owns `backend`/`imp_backend`/`embedder`, handles to
    managed processes (`*_handle`, `kill_on_drop`), readiness statuses
    (`server_status`/`imp_status`), and probe channels (`status_tx`/`imp_status_tx`) +
    `supervisor`. Exposes `apply_chat` (→ returns the immediate status, the caller
    emits it), `apply_embed`, `apply_impersonation`, `set_chat_status`/
    `set_imp_status` (from the probe loop), `backend_if_ready`,
    `impersonation_backend_if_ready(mode)`, `embedder()`. A counterpart to the
    `ServerSupervisor` trait. Server logic (~120 lines) moved out of the orchestrator;
    `settings.rs` got thinner (81 → 57 lines) — `apply_*_settings` now just delegate.
  - **`SaveQueue`** (`app/orchestrator/save_queue.rs`) — a debounce queue for
    deferred saving: `dirty: HashSet<Uuid>` + `deadline`. Methods `mark`/`forget`/
    `deadline`/`take` (+ `is_dirty` under `#[cfg(test)]`). `SAVE_DEBOUNCE` moved
    here. The actual write stayed with the orchestrator (`flush_saves` calls `saves.take()`).
- **Visibility**: the types are `pub(super)`; fields that tests/the loop set
  (`engines.backend`, `engines.server_status`, `engines.embedder`) are `pub(super)`,
  the rest private. Delegating helpers `Orchestrator::mark_dirty`/`flush_saves`
  are kept (minimal changes at call sites); the orchestrator's direct `backend_if_ready`
  was removed — calls go through `self.engines.backend_if_ready()`.
- **Not included**: impersonation's *generation* state (`imp_cancel`/`imp_gen`/
  `imp_done_tx`) and `rag_cancel` — those are concurrent turn sub-states, not
  servers; left on `Orchestrator` (like `gen_state`). `cargo fmt`/`clippy`/`test`
  green (411 passed, 7 ignored).

### Post-M9: a token counter in the status bar during generation (done)
- **A combined counter** (`tokens: 1290`): the whole conversation's (prompt) tokens
  **plus** the current/last reply's tokens, as one number. Visible **right at
  the start** (`~1234`, reply still 0), not "counting up from 1"; during
  generation the number grows live (bright, accent), afterward — dim (the turn's
  final result) until the next generation.
- **The conversation is an `~` estimate, then an exact number**. The server only
  sends the exact `prompt_tokens` at the end of a turn (with `usage`), so until
  then a client-side **estimate** is shown
  (`shared/tokens.rs::estimate_prompt`, a "UTF-8 bytes / 4" heuristic: Latin
  ≈4 chars/token, Cyrillic ≈2 — close to Gemma/Qwen BPE) marked with a `~`.
  `start_generation` emits the estimate right after `GenerationStarted`
  (`TokenUsage{completion:0, context:Some(est), context_exact:false}`); when
  `usage` arrives it's replaced with the exact `prompt_tokens` (`context_exact:true`, the
  `~` is removed).
- **The reply has two sources**: (1) a live approximation from the count of streamed
  deltas (`Text`/`Thoughts`) — with llama-server one delta ≈ one token, works with any
  server; (2) the exact `completion_tokens` from `usage`. The request asks for usage via
  a new `stream_options.include_usage=true` field (`wire.rs`, only while streaming);
  `ChatCompletionChunk.usage` → `Usage{prompt_tokens,completion_tokens}` → the client
  emits `ChatChunk::Usage(TokenUsage)` (a new enum variant) **before** parsing
  `choices` (a usage chunk has empty `choices`) and before `Finished`.
- **Flow**: `stream_round` (`orchestrator/generation.rs`) sends
  `AppEvent::TokenUsage{generation_id, completion, context:None, ..}` on every delta with
  an accumulated count `base_tokens + streamed` (doesn't touch the conversation —
  `context:None` keeps the estimate); on `ChatChunk::Usage` — the exact `completion`+`context`.
  The reply counter **accumulates across agentic-loop rounds** (`total_tokens` +=
  `RoundOutput.tokens`, where `tokens` = usage if present, otherwise the delta count).
  `runtime.rs` → `ChatScreen::set_token_usage` (gated by `generation_id`; the context is
  updated only when `Some`); fields `gen_tokens`/`gen_context`/`gen_context_exact` are
  reset in `begin_generation`. `status_bar::render` draws `tokens: [~]<conversation+reply>`
  as a single number (`~` while the conversation figure is an estimate); hidden when
  `tokens==0 && context==None`.
- An external/strict OpenAI server will either support `stream_options` (exact count)
  or ignore it (leaving the conversation estimate and the live reply approximation) —
  graceful degradation.

### Post-M9: chat list moved to a separate screen (done)
- **The chat list was an overlay inside `ChatScreen`** (`overlay: Option<ChatListState>`),
  now — a **standalone screen** `screens/chat_list.rs` (`ChatListScreen`),
  offloading the chat screen. There was no hard coupling: the widget `widgets/chat_list.rs`
  (`ChatListState`) already held its own snapshot, rendered fullscreen, and answered
  `ChatListAction`. The screen is a **thin wrapper** over the widget (FSD: `screens →
  widgets`): it holds render context (active chat for the `●` marker, palette) and
  translates `ChatListAction` → a new `ChatListIntent` (parallel to `ChatIntent`/
  `SettingsIntent`). The widget and its tests are untouched.
- **Three screens via an enum** (by choice): `app/runtime.rs` holds a base `ChatScreen`
  + `enum ActiveScreen { Chat | ChatList(Box<…>) | Settings(Box<…>) }` (variants
  boxed — screens are large, `clippy::large_enum_variant`). Replaced the former pair
  «`screen` + `Option<SettingsScreen>`». An open list/settings screen gets input and
  renders **instead of** the chat (as settings did before); `Esc`/`Close` → `ActiveScreen::Chat`.
- **Contract**: on `Esc` (not while generating) the chat now returns `ChatIntent::OpenChatList`
  (instead of locally opening the overlay); `runtime::dispatch` builds a `ChatListScreen`
  from chat snapshots (`chat_summaries`/`active_chat`/`palette` — new getters).
  `dispatch_chat_list` translates `ChatListIntent` into `AppCommand`/screen management:
  `Switch`/`Clone`/`NewChat` close the list, `Copy`/`Delete`/`Rename`/
  `AutoRename` leave it open (as the overlay used to). `NewChat` closes the list and
  calls `ChatScreen::request_new_chat()` (profile selection lives on the chat screen — the overlay
  no longer duplicates when there's >1 profile).
- **Removed from `ChatIntent`**: now-dead `SwitchChat/CloneChat/DeleteChat/
  RenameChat/AutoRenameChat` (only the removed `handle_overlay_key` built them; now
  their role belongs to `ChatListIntent`). What remains is `NewChat` (`Ctrl+N`) and `CopyChat` (`F5` in chat).
- **Event routing** (`app/runtime.rs::apply_event` now takes `&mut
  ActiveScreen`): `ChatList` is applied to the chat **always** (an up-to-date snapshot for
  the next open/`Ctrl+N`) and additionally to the open list (live update);
  `ChatListError`/the `CopyToClipboard` result go into the list's status area if
  open, otherwise as a note in the feed (`ChatScreen::push_note`/`push_error` are now pub);
  `ChatActivated` updates the active-chat marker in the open list (`set_active`);
  `Settings`, while the list is open, updates its palette (`set_palette`). Loop gates
  (spellcheck recheck, RAG/impersonation spinners, mouse wheel) are switched from
  `settings.is_none()` to `active.is_chat()` — while the list/settings are open, these
  chat-related things are paused (as was already the case for settings; background tasks
  for RAG/generation continue, their animation/banner just aren't drawn on top).
- `set_overlay_error`/`set_overlay_notice` removed from `ChatScreen` (routing moved into
  runtime). 421 tests green, clippy clean.

### Post-M9: RAG — `/rag list`, configurable chunking, `/rag rebuild` (done)
- **Configurable chunk/overlap sizes** (`config.rag: RagSettings` —
  `chunk_target_chars`/`chunk_overlap_chars`/`chunk_max_chars`, `#[serde(default)]` →
  old `settings.json` files read without migration; defaults = the former constants 800/150/1200).
  The previously hardcoded `CHUNK_*` are removed; instead a `ChunkParams` type
  (`features/tools/rag.rs`, `pub`) which `chunk_text`/`chunk_markdown`/
  `segment_units` take as a parameter. `ChunkParams::from_settings` sanitizes
  input (zero target → default; overlap < target; max ≥ target). Threaded into
  `ToolContext.chunk_params` (for the `rag_add` tool, built from `config.rag` in
  `orchestrator/generation.rs`) and into background RAG tasks. UI — three text fields in
  the "Tools" section of the settings screen (with description tooltips).
- **Storage of source raw text** (`rag_sources(profile_id, source, content,
  created_at)`, PK on `(profile_id, source)`; `CREATE TABLE IF NOT EXISTS` — without
  migration). File indexing (`/rag add`) **replaces** the source
  (`rag_source_upsert`), the `rag_add` tool (accumulates chunks) — **appends**
  (`rag_source_append`). `/rag remove` also clears `rag_sources` (same path predicate).
  Needed for `/rag rebuild` without touching files on disk.
- **`/rag list`** — the active profile's knowledge-base sources (chunk count + date of
  the earliest chunk): `Db::rag_list_sources` (`GROUP BY source`,
  `entities::rag::RagSourceInfo`); orchestrator `handle_rag_list` (in place, no task) →
  `RagProgress::Listed { sources }` → a note in the feed (`format_rag_sources`).
- **`/rag rebuild`** — reindexing via a background task (`spawn_rag_rebuild`): gathers
  sources from the DB, resolves text (stored → else reads the file by path for
  legacy data; unrecoverable ones count as an error), re-chunks/re-embeds with
  current parameters. **Changing the embedding model's dimensionality**: the vector dimension in
  sqlite-vec is one for the whole DB, so on `new_dim != current_dim` the task checks
  `Db::rag_other_profiles_have_docs` — if other profiles use the DB, it refuses with
  a clear message (not overwriting someone else's data); otherwise `Db::rag_reset_vectors`
  (drop the vectors table + reset `meta.rag_dim`) and reindex at the new dimensionality. The shared
  logic for file indexing and reindexing was factored into `index_source`
  (`orchestrator/rag.rs`). Markdown is detected by the source's `*.md` extension.
- **Contract**: `RagCommand::List/Rebuild` (`features/rag_command.rs`) →
  `ChatIntent::RagList/RagRebuild` → `AppCommand::RagList/RagRebuild` → orchestrator
  `handle_rag_list/handle_rag_rebuild`. `reset_rag_cancel`/`active_profile_id`/
  `fail_rag` were moved into shared helpers in `orchestrator/rag.rs`. Commands added to
  the help overlay (`F1`/`?`). **433 tests green**, clippy/fmt clean.

### Post-M9: monitoring early `Child` exit during the readiness probe (done)
- **Symptom**: on a corrupt GGUF / out of memory, the managed `llama-server` binds the port
  but **dies during model loading** (the preflight file check
  `launch_missing_model_file_errors_before_spawn` only catches a missing file, not
  "valid path but the process crashed"). The background probe (`wait_until_ready`) only held
  `OpenAiClient` and kept polling a dead port **until the timeout**
  `MANAGED_READY_TIMEOUT=600s`, leaving the UI at "server: connecting…".
- **Root-cause constraint**: `Child` was owned by `ServerHandle` (for `kill_on_drop`), but
  detecting exit is only possible via `child.wait()` — that needs `&mut Child` + ownership, which
  conflicts with the orchestrator holding the handle for kill-on-drop.
- **Fix** (`shared/api/server.rs`): `Child` moves into a **monitor task**
  (`spawn_monitor`), which `select!`s between `child.wait()` (process died → sets
  `exited: CancellationToken`) and `kill: CancellationToken` (set in `Drop for
  ServerHandle` → `start_kill` + `wait`). `ServerHandle` holds both tokens and hands
  `exited()` to the probe. `wait_until_ready(client, timeout, exited: Option<Cancellation
  Token>)`, after a failed probe, checks `exited.is_cancelled()` → an early `bail!`
  with a clear message ("…exited before becoming ready (corrupt GGUF or out of memory?)"),
  and during the pause between probes it `select!`s with the sleep, to wake instantly on process
  death. `kill_on_drop(true)` is kept as a fallback (the monitor owns `Child`).
- **Plumbing**: `app/supervisor.rs::spawn_probe` takes `Option<CancellationToken>`;
  managed modes (chat + impersonation) pass `Some(handle.exited())`, external —
  `None` (no process). Error → `ServerStatus::Disconnected` mapping already existed
  (the status bar shows the reason). **435 tests green** (+2:
  `wait_until_ready_bails_on_early_exit`, `monitor_cancels_exited_when_child_dies` —
  a real short-lived process, cross-platform), clippy/fmt clean.

### Post-M9: new tools — files, fetch_url, calculator, date/time (done)
- **Four new tools** (`features/tools/`), all following the existing `Tool`/
  `ToolContext`/`ToolOutcome` contract (no `Chat` mutation, a tool error → result
  text, not a panic):
  - **Files** (`fs.rs`): `fs_read`/`fs_write`/`fs_list` — read/write (with `append`)/
    listing of local files via `tokio::fs`. Under a **new global switch
    `tools.fs_enabled`** (off **by default**, like Python: the tool can
    read/overwrite any file). Optional **sandbox `tools.fs_root`**:
    if set, all paths are canonicalized and must lie inside the root (`FsRoot::
    resolve`: for existing paths — `canonicalize`, for a new file — `canonicalize`
    the parent + the name; `starts_with(root)` catches escape via
    `..`/an absolute path). Limits: reading ≤50k chars, listing ≤500 entries. UTF-8-lossy (there's nothing
    to read a binary with).
  - **`fetch_url`** (`fetch.rs`): page fetch via its own `reqwest` client
    (browser headers) → readable-text extraction (**reuses
    `web::extract_readable`** — made `pub(crate)`: `extract_readable`,
    `truncate_chars`, `USER_AGENT`, `ACCEPT_HTML`, `ACCEPT_LANGUAGE`) → summarization
    through `ctx.engine` as an independent single-turn request (like `call_subagent`: no
    history/tools, a token limit of 768, a 90s timeout). The `focus` argument
    focuses the summary; `summarize=false` → the extracted text without the model;
    a summarization failure → graceful degradation (returns the extracted text). Gated by
    **`web_enabled`** (network access, like `web_search`). Requires `http(s)://`.
  - **`calculate`** (`calc.rs`): its own **recursive-descent evaluator**
    (no new crates) — `eval(&str) -> Result<f64>` (pure, testable):
    a lexer + parser `expr/term/factor/unary/atom`, precedence, right-associative
    `^`, unary minus, `%`, parentheses, exponential notation, constants
    (`pi`/`e`/`tau`) and functions (`sqrt`/`cbrt`/`abs`/`exp`/`ln`/`log`(+base)/
    `log2`/trigonometry/`atan2`/hyperbolic/`floor`/`ceil`/`round`/`min`/`max`/
    `pow`). No I/O, **not gated**.
  - **`current_time`** (`datetime.rs`): local time + UTC (`chrono::Local`/`Utc`,
    default features already include `clock`); a `format` argument — a `strftime`
    string (an invalid spec is caught via `write!`+`DelayedFormat`, no panic).
    No I/O, **not gated**.
- **Reconciling tools for existing profiles** (`features/profiles.rs::
  reconcile_tools`, a new `Profile.known_tools` field, `#[serde(default)]`): profiles
  store a snapshot of `enabled_tools`, so tools added to the application
  **didn't show up** for already-saved profiles (the model only saw the old set and
  kept reaching for `python_exec` for everything — the symptom that surfaced this).
  `reconcile_tools` enables the profile's tools from `default_tool_ids` that it doesn't
  "know" yet (not in `known_tools`), and records them as known; tools **disabled by the user
  are not re-enabled** (they're in `known_tools`). For a profile without `known_tools` (created
  before the registry existed), the base of "known" is the current `enabled_tools`, so only
  truly new tools get added. Called during `bootstrap` (for each
  loaded profile, and on change — `upsert_profile`) and in `profiles::create`
  (a new profile gets the full current set right away). Idempotent.
- **Plumbing**: IDs added to `default_tool_ids` (meaning they automatically appear in
  the profile toggle catalog `screens/settings.rs::tool_catalog`); `effective_tool_ids`
  got a **4th parameter `fs_enabled`** and gates `web_search`+`fetch_url` via
  `web_enabled`, `fs_*` via `fs_enabled` (call sites updated in
  `orchestrator/generation.rs`); `ToolConfig` got `fs_root`, threaded into
  `standard_registry` → `Fs*::new` (from `build_registry`, `orchestrator/mod.rs`).
  `ToolSettings` extended with `fs_enabled`/`fs_root` (`#[serde(default)]` → without migration).
- **Settings UI** ("Tools" section): a "File access" toggle (`FieldId::TFs`)
  and "Files: sandbox directory" text field (`FieldId::TFsRoot`) with description tooltips
  (`field_description`). The safe `calculate`/`current_time` have no switches.
- **Tests**: pure (`calc::eval` — precedence/exponent/functions/errors; `datetime`
  rendering with injected time; `fs` sandbox/roundtrip/append/listing on a tempdir;
  `fetch` refusal on non-http and building a single-turn request with `focus` via a
  capturing backend). Network/real ones — `#[ignore]`. **472 tests green** (+37),
  clippy/fmt clean.

### Post-M9: TUI redesign (palette, role rails, pills, "keycaps") (done)
- **Goal**: dramatically refresh the interface per a finished mockup
  (`docs/redesign/TUI Redesign.dc.html`). Style: calm **rounded** borders,
  colored message **role rails**, status "pills", a quiet hotkey line with
  "keycap"-styled labels. Translating the mockup into ratatui — not pixel-perfect, but
  per its own "RATATUI IMPLEMENTATION" note (Block + rounded borders, `▌` rails,
  `Style::fg`, reversed keycap spans — all in 256 colors).
- **`shared/theme.rs` — the palette extended** with structural roles on top of the previous
  ones (`user/assistant/tool/success/warning/error/accent`): `*_soft` (light variants
  of role headers), `text`, `muted`, `border`/`border_focus`, `keycap_fg`/
  `keycap_bg`. Exact shades of the **dark** theme — taken from the mockup (oklch → sRGB, computed
  during development). **Auto** (default) — named ANSI (adapts to the
  terminal) + neutral grays for structure; **Light** — darkened. Helpers:
  `panel(title, focused)` (a rounded `Block` with a title), `keycap(label)`,
  `hint(key, desc)`, `pill(label, color)`, `border_style(focused)`, `muted_style`.
  (Unused `warning_style`/`error_style` removed.) The palette is still `Hash`
  (the syntect-theme cache key in `markdown.rs`).
- **`widgets/message_feed.rs`**: every message gets a **colored gutter rail**
  `▌` by role (content is built at width `width − 2`, wrapped, the rail is prepended
  to each visual row → `prepend_rail`); role headers in caps with an icon
  (`✦ ASSISTANT` / `❯ YOU`, `role_header`); "thoughts" collapse into a **pill**
  (`▸ thoughts · N para. · [Ctrl+T]`); a tool card — `⚒  name(args)` (two spaces
  after the emoji — it's 2 columns wide) + the result on the `└` gutter. Feed title: on the left
  `◆ <chat>`, on the right meta `model · Nk ctx` (`ChatScreen::model_meta` from the settings
  snapshot).
- **`widgets/input_box.rs`**: a rounded border + a focus color, a `❯` prompt
  column on the left (width `PROMPT_W`; the inner input area is shifted right, the test helper
  `render_at` accounts for this).
- **`widgets/status_bar.rs`**: server status shown as a **pill** (`● server: …` colored by
  status), separators `│`, hotkeys via `keycap`+`hint`. Substrings `ready`/
  `generating`/`tokens:`/`mouse:` kept (tests).
- **`screens/settings.rs`**: the main panel and hints — via `panel`/`keycap`;
  the section menu with a rail on the active section; field values colored by type
  (`render_field_line` takes the palette: a toggle — green/muted, a choice —
  blue, a dash — border color).
- **`widgets/chat_list.rs`**: a fullscreen rounded panel "▤ Chats" (+count on the
  right); **a search line inside a border** with a "`/` key" on the right; list rows — a dot
  (green for the active one), title and count, **right-aligned**
  (truncation with `…`, `truncate_to_width`), selection — a soft backdrop + a green rail on
  the selected row; a **hotkey status line at the bottom** laid out as a **neat grid**
  (`status_lines`: max columns to fit the width, columns line up vertically;
  `Del` — a red "key"). **Rename (F2)** — a separate input field with a
  **real cursor** (`render_rename` + `set_cursor_position`, horizontal
  scroll; `Mode::Rename` holds a `buffer: Vec<char>` + `cursor`, supports
  `←/→`/`Home`/`End`/`Backspace`/`Delete`/mid-string insert), the search line is hidden while
  editing. `profile_list.rs`/`impersonation_preview.rs`, the help and
  spellcheck overlays — also on rounded panels.
- **Width-2 emoji glyphs** (`⚒`/`⚙`/`⌨`) `unicode-width` counts as 1 → the following
  space got overwritten; after them we add **two** spaces (otherwise they merge with the text).
- The default theme stayed **Auto** (the redesign's structure is visible everywhere; the mockup's exact
  shades — the **dark** theme in settings). Unit tests updated to match the new
  look (finding tool blocks by name, rail-aware empty lines, `↑/↓` navigation accounting for the
  prompt column). Gate green.

### Post-M9: status bar — pill top-left, hotkey grid right (done)
- **Symptom**: on wrap, hotkeys went into a separate grid **under** the status line, and
  "`Ctrl+W  mouse: selection`" landed right under "`● server: ready`" — the two left
  columns merged and visually competed.
- **Fix** (`widgets/status_bar.rs`, `lines`/`grid_layout`/`right_grid`): the status
  pill (server + generation + token counter) stays on the left on the **top** line,
  and hotkeys are laid out as a **neat grid, right-aligned**, and **share
  the top line** with the pill. When everything fits — one line: pill on the left, hotkeys
  on the right (with a visible gap between them). When it doesn't fit — hotkeys wrap DOWN
  as a grid **whose columns line up vertically** (as in the chat list window), and the
  partial (wrapped) row is right-aligned — its keys land **exactly under
  the columns of the row above** (e.g. `Ctrl+C exit` exactly under `Ctrl+P settings`, padded
  out to the column width), so the left/middle part of the window is empty and doesn't draw attention.
- **Layout** (`grid_layout`): cells are filled row-by-row, left-to-right/top-to-
  bottom, but the incomplete bottom row takes the **rightmost** columns (`empty_lead`
  empty columns at the start); column widths — the max over the cells actually placed in
  that column (including the wrapped one), the total block width + leading indent =
  the full width → the block hugs the right. **Choosing the number of columns**: the max number of
  columns (→ the fewest rows) at which the pill can share the top line with the
  block (`state_w + GAP + block_width(cols) ≤ width`). A narrow fallback (even one column doesn't fit next to
  the pill): the pill on its own top line, the grid right-aligned below it.
  `height()`/`render()` and their call sites in `screens/chat.rs` unchanged (same signature).
  The old inline path and the left-side grid (`palette.hotkey_grid`) in the status bar are no longer
  used (the palette's grid helper remains for the chat-list overlay). Tests: a single
  line with edges flush; wrapping with the pill on the top line and `Ctrl+C` exactly under the
  `Ctrl+P` column (rendered in `TestBackend`, comparing positions in characters).

### Post-M9: role-rail color independent of line style (done)
- **Symptom**: a message's vertical role rail (`▌`) on the left looked a different
  color (dimmed) next to divider lines `───` and horizontal table
  borders.
- **Cause**: `markdown` sets `.add_modifier(Modifier::DIM)` on these lines at the
  **line level** (`line.style`, see `shared/markdown.rs` `rule`/`border_line`), and
  `message_feed::prepend_rail` copied `line.style` onto the whole output line, including
  the rail span. The rail span only overrode `fg` (not modifiers), so the
  line-level `DIM` leaked onto it too.
- **Fix** (`widgets/message_feed.rs::prepend_rail`): the line-level style is now
  **folded into the content spans** (`line.style.patch(span.style)`), while `out.style`
  is reset to the default. Content looks identical to before (same resulting
  span styles), but the rail is now its own span with its single `fg` and
  no longer inherits any line-level modifiers (a general fix — not only for
  the current `DIM`). Regression test `rail_is_not_dimmed_next_to_table_borders`.

### Post-M9: dim background under popups (done)
- **Symptom**: popups over the screen only cleared their own area (`Clear`), and the background
  behind them stayed at full brightness and visually blended with the popup, hurting
  readability.
- **Fix**: a helper `shared/ui.rs::dim_background(frame)` applies `Modifier::DIM`
  to **all** cells of the screen buffer; called **before** `Clear`+rendering the popup.
  `Clear` then resets the popup's cells to the default (undimmed) style — so
  only the background gets dimmed, while the popup itself stays bright. The `DIM` effect is
  terminal-dependent (Windows Terminal supports it). Lives in `shared` (FSD: `screens → shared`) so it can
  be reused across screens.
- **Where applied**: the help overlay (`F1`/`?`) and the spellcheck-suggestion popup (`Ctrl+G`)
  in `screens/chat.rs`; the large multi-line editor for a profile's system message/greeting
  in `screens/settings.rs` (only `editor.multiline` — compact single-line
  field-edit strips are edited in place and don't dim the background). Fullscreen
  overlays (profile picker, chat list, settings) don't dim the background — they already
  occupy the whole screen.

### Post-M9: manual chat rename in a single-line `InputBox` (done)
- **The rename field (`F2` in the chat list) switched from a hand-rolled
  character-by-character buffer to a single-line `InputBox`** (`set_single_line`, like
  editable settings fields). This gave it, "for free": **spellcheck**
  (underlining errors), **word-wise navigation/deletion** (`Ctrl+←/→`,
  `Ctrl+Backspace/Delete`), `Ctrl+Home/End`, **clear/restore `Ctrl+K`**, clipboard
  paste, and horizontal scrolling of a long name. Previously the `Vec<char>` buffer
  only supported character-by-character movement/editing.
- **`widgets/chat_list.rs`**: `Mode::Rename` now holds a `Box<InputBox>` (+ a
  `spell_dirty` flag) instead of `Vec<char>`/`cursor`; `on_key_rename` handles only
  `Esc`/`Enter`/`Ctrl+K` (layout-independent, `shared/keys`), everything else is handled by
  `InputBox` itself. The manual `render_rename` function was removed — the field renders via
  `input.render(...)` (its own rounded border + `❯` + a real cursor). New
  methods `handle_paste` and `recheck_rename_spelling(&SpellChecker) -> bool`. `render`
  became `&mut self` (`InputBox::render` requires `&mut`).
- **The spellchecker isn't duplicated**: the owner is the chat screen (`ChatScreen::spellchecker()
  -> Option<&SpellChecker>`); the loop in `app/runtime.rs`, when the list is open, lends
  it to the list screen (`ChatListScreen::recheck_spelling`) — the `screen`/`active`
  fields are separate, so an immutable borrow of the checker coexists with a mutable borrow of the list.
  No debounce (a name is short — recomputing on the `spell_dirty` flag is cheap). Clipboard
  paste for the list is routed into the rename field
  (`process_input_batch`). **A suggestion popup (`Ctrl+G`) for rename was NOT
  added** — only underlining (by agreement).

### Post-M9: conversation control tools (followup / rewrite) (done)
- **Two optional tools** (`features/tools/control.rs`, spec §9.3.3),
  letting the model control the structure of its own response:
  - **`send_followup_message`** ("write another message") — the assistant finishes
    the current message, calls the tool, and writes a **second reply** as a separate
    bubble right after the first.
  - **`rewrite_current_message`** ("rewrite my current message") — if partway through it realizes
    it answered incorrectly: the text accumulated in the current round is **discarded**, and
    the next round is written from scratch. The discarded content (assistant + tool) goes into
    `Chat.deleted` (like `Ctrl+E`/`Ctrl+R`) for manual recovery.
- **This is control flow, not an ordinary tool**: the client-side
  agentic loop itself recognizes it (`app/orchestrator/generation.rs`), not `Tool::invoke` (the
  `Tool` implementations only exist for the schema/registration/gating; `invoke` returns the same
  "permission" text that the loop synthesizes). Tool-calling protocol alternation
  isn't broken (`assistant(tool_call) → tool → assistant`), **no synthetic user turn is
  introduced** (by agreement — simpler and more reliable).
- **Optional, off by default**: not in `default_tool_ids` (→ `reconcile_tools`
  doesn't enable them for existing/new profiles), but present in a new catalog
  `all_tool_ids` — `screens/settings.rs::tool_catalog` builds profile toggles from
  it. The loop recognizes a control call **only if it's actually enabled** in
  the profile (`allowed_has`), otherwise — a plain refusal. `effective_tool_ids` doesn't
  touch them (they pass through `_ => true`).
- **A separate bubble** — a `Message.new_bubble` flag (`#[serde(default,
  skip_serializing_if)]` → without migration): `from_messages` (`widgets/message_feed.rs`)
  doesn't merge a flagged message into the previous assistant block. Internal
  tool blocks for control tools in the feed are **hidden** (`from_message` filters
  by `control::is_control_tool`).
- **Live stream ↔ reload**: new events `AppEvent::AssistantContinue`
  (followup: finish the bubble, start a new one) and `AssistantRewrite` (discard
  the accumulated text of the current bubble) — `screens/chat.rs::continue_assistant`/
  `rewrite_assistant`. In the generation loop, `pending_new_bubble` sets `new_bubble` on
  the next domain assistant message; a rewrite round doesn't consume it and puts the
  discarded content into `GenResult.deleted` → `handle_done` calls `chat.record_deleted`.
- Every call = one round (`max_tool_rounds`, a backstop against endless extra messages/
  rewrites). Tests: pure (`control` names/text; `from_messages` two bubbles
  + hiding the control block + regular merging still works), live (`chat.rs`: followup
  two bubbles == reload, rewrite discards the partial), integration for the orchestrator
  (followup → 4 messages with `new_bubble`; rewrite → the discarded content in `deleted`).
  **511 tests green**, clippy/fmt clean.
- **Verified on live Gemma 4 12B and Qwen 3.6 27B** (`llama-server`, `--jinja`): three
  `#[ignore]` smokes green on both — the client-level one
  (`client.rs::control_tools_are_callable`: the model actually calls
  `send_followup_message`) and two end-to-end orchestrator ones through a real
  `OpenAiClient` (`tests.rs::followup_tool_e2e_live` → history `user →
  assistant(followup) → tool → assistant(new_bubble=true)`; `rewrite_tool_e2e_live`
  → the discarded round in `Chat.deleted`, the final history clean). Run:
  `MINDFORK_ENGINE_URL=…/v1 cargo test … -- --ignored --nocapture --test-threads=1`.
- **Observation**: both models readily call `send_followup_message`; a call to
  `rewrite_current_message` is more sensitive to the prompt (Qwen, given a soft "little
  scenario" phrasing, would simply write an incorrect answer without calling the tool — required
  explicitly marking the call as a mandatory step). This is about **model behavior**
  on the counter-natural request "answer incorrectly, then rewrite," not about the mechanism: once
  a model actually calls the tool, discarding/archiving work the same. In real
  use the tool triggers naturally (the model itself realizes it made a
  mistake), and its description is neutral.

### Post-M9: emoji paste from clipboard (cluster width + clipboard recovery) (done)
- **Two independent bugs with emoji in the input box**, both only inside the application (in a bare
  terminal emoji paste fine and the cursor is correct — the terminal manages the caret itself):
  - **(B) Cursor "drifts" on BMP-emoji clusters** (`❤️` = `❤` U+2764 +
    a selector U+FE0F; `👍🏽` = an emoji + a skin-tone modifier). Cause: width was computed
    character-by-character via `unicode-width`, which gives `❤`=1, `U+FE0F`=0 (total 1), while
    the terminal draws the cluster at width **2** → the cursor landed in the middle of the emoji, text
    after it — one column to the left. `👍🏽` was measured as 4 instead of 2.
  - **(A) Supplementary-plane emoji (`😊` U+1F60A, `🥹`) don't paste at all.**
    Cause — a **crossterm 0.29 limitation on Windows**: clipboard paste arrives as
    ordinary console key events, and such emoji are encoded as a UTF-16 surrogate pair;
    console key-down/key-up records break the pair assembly in crossterm
    (`event/sys/windows/parse.rs`: the surrogate is handled without accounting for `key_down`), and
    the character is lost **before** our layer. BMP characters (letters, `❤`, `U+FE0F`) pass through.
- **Fix (B1) — width accounting for the emoji cluster** (`shared/wrap.rs`): a new
  `width_at(chars, i)` — a causal (only looking at the previous character) width for character
  `i`: a `U+FE0F` selector brings the preceding "text" character up to 2 columns
  (`2 − prev_width`); a skin-tone modifier (`U+1F3FB..U+1F3FF`) gives 0 (the base
  is already 2). `display_width` now sums `width_at`, and incremental consumers
  (`wrap_ranges`; `col_at_width`/`col_for_visual`/single-line rendering in `input_box`;
  truncation with "…" in `chat_list`/`markdown`) were switched from `char_width` to `width_at` —
  otherwise wrapping/cursor would diverge from `display_width` at a cluster boundary. `char_width`
  is kept as the context-independent base. Improves both the feed (`message_feed`) and tables.
- **Fix (B2) — cursor/deletion by grapheme cluster** (`shared/wrap.rs`
  `prev_boundary`/`next_boundary` via `unicode-segmentation`, UAX #29; `input_box`
  `move_left`/`move_right`/`backspace`/`delete`): the cursor and deletion moved by Unicode
  scalars, and `❤️`/`👍🏽` are **two** scalars (base + variant selector/modifier). Symptoms:
  `←`/`Backspace` through `👍🏽` required two presses (after the first, `👍` remained);
  through `❤️` the cursor first jumped to the middle; `Delete` at the start of a line left
  an "orphaned" variant selector/modifier → phantom glyphs and broken spellcheck
  underlines. Now movement/deletion take a whole cluster: `prev_boundary`/
  `next_boundary` give the boundary left/right of a position. A lone scalar emoji (`😊`)
  — a normal ±1 boundary (one press, as before).
- **Fix (A) — reconciling paste against the clipboard** (`app/runtime.rs`, Windows only):
  `Chunk::Paste` (reconstructed from key events) is checked against the clipboard via a pure
  `paste_projection_matches`: from the clipboard content, code points > U+FFFF are dropped (exactly the ones
  the console loses), both sides are normalized on `\r\n`/`\r`/`\t` — if they match, it's the
  same paste and the **full** clipboard text (with emoji) is used; otherwise (the clipboard is stale/it's not the
  same paste/unavailable) — the reconstruction (without emoji, but with no risk of pasting someone else's data).
  Safe in any outcome: if emoji are already present in the reconstruction — the projection won't
  match and the reconstruction stays; an empty reconstruction never matches. `read_clipboard_
  text` lazily creates an `arboard::Clipboard` (the same slot as `F5` copy).
  `reconcile_paste` on non-Windows is the identity function (there a correct `Event::Paste` arrives).
- **Residual limitation**: a paste consisting **entirely** of supplementary-plane emoji
  (with no BMP character at all) leaves no key events at all → neither `Chunk::Paste`
  nor another reconciliation trigger fires → it won't paste (nothing to latch onto). Emoji **inside text**
  (the common case, "hi 😊") are recovered. A full fix would require
  patching the console-reading layer (patching/forking crossterm or a custom `InputRecord` reader).

### Post-M9: emoji picker popup (`Ctrl+B`) (done)
- **`Ctrl+B` opens a popup grid of popular emoji** in the chat window; the chosen one
  is inserted into the input box **at the cursor**. The widget `widgets/emoji_picker.rs`
  (`EmojiPickerState` + `EmojiPickerAction`) is FSD-self-contained (`screens →
  widgets`): it only holds the selection index, and responds to key presses with an action
  (`None`/`Cancel`/`Pick`). The `EMOJIS` list is fixed (44 emoji, 4 rows of
  `COLS=11`); the grid width was chosen so that the popup's bottom hint fits
  in full. Navigation `←↑↓→` over the grid (clamped), `Enter` — insert, `Esc` —
  close. The selected cell uses a dark `palette.keycap_bg` background (like the selected
  chat-list row; a reversed style gave a light background that washed out the colored glyph).
- **Insertion** via `InputBox::insert_str` — safe for multi-scalar emoji
  (`❤️`, `👍🏽`; cursor/deletion by grapheme clusters is already correct, see above).
- **Remembers the last choice**: `ChatScreen.emoji_last` holds the index; opening goes
  through `EmojiPickerState::with_selected(idx)` (clamped), closing (both `Enter` and
  `Esc`) saves the current selection back. While the popup is open the input box loses
  focus, `handle_paste`/`handle_mouse` are no-ops (like the spellcheck popup); it's drawn
  on top with a dimmed background (`dim_background`). `Ctrl+B` is layout-independent
  (`shared/keys`, physical B = `Ctrl+и`), added to the help overlay (`F1`/`?`), the
  README/spec §11.5 keybinding tables. Pure widget tests (navigation/clamping/`with_selected`/
  render-without-panic) and screen plumbing (insertion at the cursor, remembering the choice,
  cancellation).

### Post-M9: multi-provider inference — Phase 0 (foundation) (done)
- **Cloud providers through the same `EngineBackend` trait** ([ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md)):
  Phase 0 unlocks **OpenAI** (`platform.openai.com`) and **Gemini** (via an
  OpenAI-compatible endpoint). Claude (a separate `/v1/messages` protocol) — Phase 2.
  Layers above the engine (orchestrator, agentic loop, tools, UI) are **untouched**.
- **A flat mode taxonomy** (`shared/config.rs`): `ServerMode` extended with
  `OpenAi`/`Gemini` (alongside `Managed`/`External`), `ImpersonationMode` — with them too
  (plus `Shared`). A new `CloudProvider { OpenAi, Gemini }` with `base_url()`;
  `ServerMode::cloud_provider()`/`ImpersonationMode::cloud_provider()` → `Option`.
  `EngineSettings`/`ImpersonationEngineSettings`/`EmbedSettings` got
  `model_name`/`api_key_env` (all `#[serde(default)]` → old `settings.json` without
  migration).
- **The model is a property of the backend, not the request** (minimal change): `ChatRequest`
  **doesn't carry** a model — it's injected into the body (`model`) by `OpenAiClient`. The client got
  `api_key`/`model`/`dialect` + builders `with_api_key`/`with_model`/`with_dialect`,
  sends `Authorization: Bearer` (method `auth`), and for embeddings sets `model` in
  `EmbeddingRequest`. `new(url)` = local/external (no key, lenient dialect).
- **Request-body dialect** (`shared/api/wire.rs`, `WireDialect { LlamaCpp, OpenAi,
  Gemini }`): `build_chat_request(req, stream, model, dialect)`. Strict cloud
  dialects (`OpenAi`/`Gemini`, `is_strict`) **strip** llama.cpp extensions and
  reasoning signals (`top_k`/`min_p`/`dynatemp_*`/`dry_*`/`mirostat*`/`samplers`/
  `thinking`/`reasoning_*`/`chat_template_kwargs`) — otherwise the cloud would return `400`
  even for the default `thinking:true` (`restrict_to_strict`). What remains: `temperature`/
  `top_p`/`frequency_penalty`/`presence_penalty`/`seed` + `tools`/`stream*`. **The token
  limit field differs:** OpenAI requires `max_completion_tokens` (newer models
  reject `max_tokens` with `400` — a real error hit during testing), Gemini-compat uses
  the classic `max_tokens`; `LlamaCpp` sends everything as before. Provider→dialect —
  `dialect_for` in the supervisor. `WireDialect` exported.
- **Supervisor** (`app/supervisor.rs`): `apply_chat`/`apply_impersonation`/`apply_embed`
  got cloud branches (`cloud_chat_setup`/`cloud_embed_setup`). The key is resolved from
  an **env variable, by name** (`resolve_api_key`; the secret isn't written to disk, ADR 0004);
  the base URL comes from the provider (override via `url`); dialect `OpenAi`. The cloud doesn't "load
  a model" → the status is immediately `Ready` (no probe). No model/key → `Disconnected` with
  a clear message (chat) / `UnavailableEmbedder` (RAG).
- **Settings UI — field visibility by mode** (`screens/settings.rs`): `model_fields`/
  the embed part of `tool_fields` show **only mode-relevant** fields (managed →
  llama-server parameters; external → URL+model (opt.); openai/gemini → model+API key
  (env)+base URL (opt.)) — this de-clutters the screen. The `ServerMode` cycle (`cycle_mode`,
  now directional) walks all 4 variants; `cycle_imp_mode` — 5. New
  `FieldId`s (`X/Ix/E` × `ModelName`/`ApiKeyEnv`) with `apply_text` and tooltips
  (`field_description`: modes, API-key-env, model name). The API key in the UI is the
  env-variable **name**, not a secret.
- **Marking unsupported sampling in the cloud** (`screens/settings.rs`): in the "Sampling"
  section, when the cloud provider is the subsection, parameters outside the strict
  dialect (llama.cpp extensions + `thinking`/`reasoning_effort`) are marked via the value's
  **color** — a set value is colored `palette.warning` (amber/brown —
  attention, but not alarm), visible without focus (`render_field_line(..., inactive)`).
  Only **set** values are flagged (`set`): unset ones (`—`) have nothing to flag.
  When focused, a detailed tooltip below reads `CLOUD_UNSUPPORTED_NOTE`
  ("…the value will be kept for local models"). **The value itself is untouched** (stored in the
  shared `default_sampling`/`impersonation_sampling`, will work with a local model).
  The supported subset is `cloud_supported_param`
  (temperature/top_p/frequency_penalty/presence_penalty/seed/max_tokens). The subsection's
  provider — `sampling_is_cloud` (for impersonation in `shared` mode, the assistant's engine is
  used). Fields aren't hidden (values survive switching back to a local model).
  Evolution: a focus-only tooltip → not noticeable → a text note → replaced on request
  with value coloring (more visible, no repeated text).
- **Tests**: config (cloud modes serialize/map to a provider, roundtrip,
  old JSON → defaults); wire (the OpenAi dialect strips extensions and sends `model`;
  LlamaCpp omits `model=None`); supervisor (`resolve_api_key` env/missing; cloud
  without model/key → `Disconnected`; with model+key → `Ready`+backend; cloud-embed
  without config → unavailable); settings (cloud mode shows model/api-key and
  hides llama-server fields). **543 tests green**, clippy/fmt clean.
- **Not included in Phase 0** (per the ADR 0004 plan): splitting `shared/api` into
  submodules (`contract/openai/anthropic/managed`) — deferred to Phase 2, when
  `AnthropicClient` shows up (until then the `anthropic/` submodule would be empty, and splitting for one
  OpenAI family alone would be premature). Live smokes against real OpenAI/Gemini — by
  key, outside CI.

### Post-M9: multi-provider inference — Phase 2 (Anthropic / Claude) (done)
- **Claude through its own Messages API protocol** ([ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md)),
  as a new `EngineBackend` trait implementation — layers above the engine untouched.
- **New module `shared/api/anthropic/`** (`client.rs` + `wire.rs`): `AnthropicClient`
  hits `/v1/messages` (headers `x-api-key` + `anthropic-version`), parses
  event-based SSE (`message_start`→usage.input_tokens; `content_block_start` tool_use→
  `ToolCall`; `content_block_delta`: `text_delta`→`Text`, `thinking_delta`→`Thoughts`,
  `input_json_delta`→`ToolCall` args; `message_delta`→`Usage`+`Finished` by
  `stop_reason`). `wire::build_request` translates the domain `ChatRequest`: system →
  a **top-level** `system`; only user/assistant roles; tool results → `tool_result`
  blocks **inside a user turn** (there's no `tool` role); tool calls → `tool_use`
  blocks; adjacent messages of the same role **are merged** (Anthropic requires
  strict alternation); `max_tokens` is required (default 4096); a tool's schema is
  `input_schema`. **Sampling: only `max_tokens`** — the newest Claude 4.x models
  (opus-4-8/haiku-4-5) have "locked in" sampling and reject `temperature`/`top_p`/
  `top_k` as deprecated (HTTP 400 during a live test run), so they aren't sent at all (the UI
  marks them as unsupported for Claude via `cloud_supported_param`). The error body
  isn't swallowed (as with `OpenAiClient`).
  Anthropic has no embeddings — `Embedder` isn't implemented (RAG uses a separate one, ADR 0002).
- **Config**: `ServerMode`/`ImpersonationMode` += `Claude`; `CloudProvider` += `Claude`
  (`base_url`=`https://api.anthropic.com`, the client itself appends `/v1/messages`).
- **Supervisor**: `cloud_chat_setup` branches by the provider's protocol — OpenAI/Gemini
  → `OpenAiClient` (+dialect), Claude → `AnthropicClient`. Embeddings in Claude mode →
  `UnavailableEmbedder` (Anthropic doesn't do embeddings) with a clear log message.
- **Settings**: `Claude` in the mode cycles (5 values for the engine, 6 for
  impersonation), the same cloud fields (model/API-key-env/base URL). **Sampling
  marking is provider-dependent**: `cloud_supported_param(provider, p)` — for Claude
  **`top_k` is supported** (unlike OpenAI/Gemini), while penalties/seed aren't;
  `sampling_cloud_provider` returns the subsection's provider (shared impersonation inherits the
  assistant's).
- **Claude only appeared in the selector now** (with a real client) — in Phase 0 it was
  deliberately absent, so as not to ship a nonfunctional entry.
- **Tests**: wire translation (system top-level, tool_use/tool_result+merging, input_schema,
  the max_tokens default, parsing all SSE events), stop_reason mapping, supervisor (Claude
  chat → Ready+backend; Claude embed → unavailable), settings (Claude marks penalties/
  seed, but not top_k). **555 tests green** (+a live `#[ignore]` smoke
  `MINDFORK_ANTHROPIC_KEY`), clippy/fmt clean.
- **Splitting `shared/api` into submodules (ADR 0004 §2) — done** (as a separate step
  after Anthropic, for symmetry): `backend.rs`→`contract.rs`, `client.rs`+`wire.rs`→
  `openai/` (a private `wire`, re-exporting `OpenAiClient`/`WireDialect`), `server.rs`→
  `managed.rs`, `anthropic/` already existed. Moves via `git mv` (history preserved); the public
  surface is the same (re-exported from `mod.rs`); external `shared::api::backend::*` →
  `::contract::*`. Final structure: `contract` (traits/types), `openai`/`anthropic`/
  `managed` (implementations/launch), `thoughts`, `mock`. **555 tests**, clippy/fmt clean.

### Post-M9: nested engine config by mode/provider (done)
- **The engine config became nested** (`shared/config.rs`): previously `EngineSettings`/
  `ImpersonationEngineSettings`/`EmbedSettings` shared a **flat** field set
  (`model_name`/`api_key_env`/`url`/`binary`/…), common to all modes — switching
  the provider overwrote someone else's values. Now every mode/provider has its **own
  subsection**: reusable `ManagedSettings` (llama-server: binary/model/ngl/ctx/
  jinja/reasoning_format/no_mmap/host/port), `ExternalSettings` (url + opt. model_name),
  `CloudSettings` (model_name/api_key_env/url-override) — one instance per
  `openai`/`gemini`/`claude`. Embeddings have their own `ManagedEmbedSettings` (fewer
  fields — host/jinja/no_mmap/ctx are fixed by the supervisor). You can keep
  managed + OpenAI + Gemini + Claude all configured at once and switch quickly between them.
- **Mode-aware accessors** `EngineSettings::cloud()`/`cloud_mut()` (and for impersonation/
  embed) return the **active** cloud substructure per `mode` (via
  `cloud_provider()`); shared helpers `cloud_ref`/`cloud_mut` in config.rs. This preserved the
  existing settings `FieldId`s — reading/writing routes by the current mode
  (only one group is visible): `XUrl`/`XModelName` write to `external.*` in external mode and
  `cloud_mut().*` in the cloud; managed fields → `managed.*`.
- **No migration** (user's decision): an old flat `settings.json` is read via
  `#[serde(default)]` — unrecognized flat engine fields are silently ignored, subsections
  come out default. The managed binary/model/key need to be entered again once. Chats/profiles
  aren't affected. `schema_version` stayed at 1.
- **Settings UI** (`screens/settings.rs`): `model_fields`/the embed part of `tool_fields`
  build fields from the substructures (helpers `managed_rows`/`cloud_rows`); mode-driven
  visibility (as before). **Cloud-unsupported sampling parameters are now
  hidden** (filtered by `SAMPLING_PARAMS` via `cloud_supported_param`), rather than colored
  yellow — the warning path was removed (`render_field_line(inactive)`, `CLOUD_UNSUPPORTED_NOTE`,
  `sampling_unsupported_in_cloud`); hidden-parameter values are preserved (they'll work
  with a local model). Impersonation in `shared` mode inherits the assistant's provider filter.
- **Chat-header cosmetics** (`screens/chat.rs::model_meta`): now driven by `engine.mode` —
  managed shows `name.gguf · Nk ctx`, external — `external.model_name`, cloud — the
  active cloud model's name (no ctx). Previously in cloud modes the local
  GGUF's name and its context were shown stuck there.
- Other consumers updated: `supervisor.rs` (building clients/`ManagedConfig` from
  substructures; a shared `managed_config(&ManagedSettings)`, helpers
  `external_chat_setup`/`managed_chat_setup`), `main.rs::apply_env_overrides`
  (`config.engine.managed.*`/`external.url`). **556 tests green**, clippy/fmt clean.

### Post-M9: get_sampling/set_sampling limited to fields available in the mode (done)
- **The `get_sampling`/`set_sampling` tools now show/change only the sampling
  fields that the current mode's engine actually accepts** — previously the `set_sampling`
  schema always carried the full set (including llama.cpp extensions), and in the cloud
  the model would try to change `top_k`/`min_p`/… which the provider rejects (`400`). Now
  the model sees exactly what's applicable.
- **A single source of truth** — `entities/sampling.rs::supported_sampling_fields(provider:
  Option<CloudProvider>) -> &[&str]` (mirrors the wire dialect's `restrict_to_strict` /
  `anthropic::wire`): `None` (local/external llama.cpp) — the whole
  `SETTABLE_SAMPLING_FIELDS` set; OpenAI/Gemini — `temperature`/`top_p`/`frequency_penalty`/
  `presence_penalty`/`seed`/`max_tokens`; Claude — only `max_tokens`. The settings
  UI now feeds off it too: `screens/settings.rs::cloud_supported_param` delegates there
  (via a new `SamplingParam::field_name()`), removing the duplication of "what's supported."
- **Tools are mode-aware via `provider`** (`features/tools/introspection.rs`):
  `GetSampling::new(provider)`/`SetSampling::new(provider)` store the chat engine's provider.
  `SetSampling::parameters` builds a JSON schema only from the available fields (`field_schemas`
  — name→schema, covers the whole `SETTABLE_SAMPLING_FIELDS`); `invoke` **drops**
  unavailable keys from the arguments and tells the model about it (rather than silently applying);
  `GetSampling::invoke` filters output by the same set; **the `set_sampling` result
  text is also filtered** (`filter_to_supported`) — otherwise the feed and the model
  would get the full config with dozens of `null` fields (confusing for both the model and
  the user); tool descriptions carry the list of available fields (`scope_note`).
  The provider is taken from `config.engine.mode.
  cloud_provider()` in `build_registry` (a `ToolConfig.sampling_provider` field); the registry
  **is also rebuilt on an engine mode change** (`orchestrator/settings.rs`), not only on a
  `config.tools` change.
- **If the mode has no supported parameters at all — the tools are unavailable to the model**:
  `effective_tool_ids` got a 5th argument `sampling_provider` and filters out
  `get_sampling`/`set_sampling` when `supported_sampling_fields` is empty (a safety
  net — every current provider has at least `max_tokens`). Constants
  `GET_SAMPLING_ID`/`SET_SAMPLING_ID` (re-exported from `introspection`).
- **Tests**: `supported_sampling_fields` (mirrors the dialect/subsets); tools
  (the cloud hides `top_k` in `get_sampling`'s output; Claude drops `temperature`/`top_k`
  from `set_sampling` and notifies about it; the `set_sampling` schema per mode; `field_schemas` covers
  the whole set); `effective_tool_ids` keeps the tools when parameters exist; the settings UI filter
  (the previous `cloud_hides_unsupported_sampling_params` — now green on the delegation);
  the `set_sampling` result in the cloud doesn't carry a full config dump.
  **567 tests green**, clippy/fmt clean.

### Post-M9: FlashAttention + speculative decoding in managed mode (done)
- **The managed `llama-server` got FlashAttention (`--flash-attn`) and speculative
  decoding (`--spec-*`)** — the latter lets MTP models be attached (e.g.
  `mtp-gemma-4-12B-it.gguf`) via `--spec-type draft-mtp`. Only for local managed
  (the assistant's chat server and the impersonation server); external/cloud are unaffected,
  and it's inapplicable to the embedding server (it doesn't generate tokens).
- **Config** (`shared/config.rs`): two enums — `FlashAttn { Auto, On, Off }`
  (`Auto` = the flag isn't passed, llama.cpp's default) and `SpecType` (`None`/`draft-simple`/
  `draft-eagle3`/`draft-mtp`/`ngram-simple`/`ngram-map-k`/`ngram-map-k4v`/`ngram-mod`/
  `ngram-cache`; serde kebab-case matches the CLI value; `as_arg()`/`label()`/
  `cycle(dir)`/`needs_draft_model()`). `ManagedSettings` extended with `flash_attn`,
  `spec_type` and draft fields `draft_model` (`-md`), `draft_gpu_layers` (`-ngld`),
  `draft_n_max` (`--spec-draft-n-max`), `draft_n_min` (`--spec-draft-n-min`). All via
  `#[serde(default)]` — old `settings.json` without migration.
- **Argument construction** (`shared/api/managed.rs`): `ManagedConfig` carries primitives
  (`flash_attn: Option<String>`, `spec_type: Option<String>`, `draft_*`) — like
  `reasoning_format`; the supervisor converts the enums to strings (`managed_config`). `build_args`
  adds `--flash-attn`/`--spec-type`/`-md`/`-ngld`/`--spec-draft-n-max`/`-n-min`
  **only when set** (unset → llama.cpp's default). The preflight file check
  extended to the draft model (`-md`): a nonexistent path → a clear error before `spawn`
  (otherwise `llama-server` silently fails while loading it, and the probe waits until the timeout).
- **Settings UI** (`screens/settings.rs`, "Model/server" section, both
  Assistant/Impersonation subsections): FlashAttention and `--spec-type` — Choice fields (←/→ cycle);
  draft fields (`-md`/`-ngld`/n-max/n-min) shown **only for draft-* types**
  (`needs_draft_model`), so as not to clutter the section for ngram/none. All fields have
  description tooltips (`field_description`). `managed_rows` was refactored from a long
  list of FieldId arguments into a `ManagedFieldIds` struct (+ two const instances
  `ASSISTANT_MANAGED_IDS`/`IMP_MANAGED_IDS`). Optional numeric fields are parsed via
  `parse_opt_num` (empty → `None`/clear, non-numeric → keep the previous).
- **Tests**: config (as_arg/serde/cycle/needs_draft_model, roundtrip of the managed section with
  spec fields); managed (the flash-attn/spec flags in `build_args`, defaulting without them,
  the preflight error on a missing `-md`); settings (flash/spec visibility in managed,
  draft fields revealing when cycling to draft-mtp, cycling flash-attn while persisting).
  **579 tests green**, clippy/fmt clean. A live run against a real
  `mtp-gemma-4-12B-it.gguf` — a manual check (needs a GPU/model).

### Post-M9: SelfModel MVP — agent "self-model" (probe, done)
- **A trimmed-down Phase 1 from the idea bank** ([docs/self-model.md](docs/history/self-model.md)) per
  the plan [docs/self-model-mvp.md](docs/history/self-model-mvp.md): a per-profile agent "self-
  model" — free-form text about itself + goals + a mental model of the interlocutor. The probe's goal
  is to check **behavioral** value (does a local model recall facts about
  the user and its own goals across chats), rather than build a full meta-cognitive
  machinery. Deliberately **left out**: `beliefs` with numeric "strengths", contradiction
  detect/resolve, a narrative, versioned history, timer-based auto-reflection, all
  "deep self-awareness" (Phase 4 of the source document).
- **Key departure from the source document**: it modeled the self-model via
  `ChatEffect` (as `Chat` state). In the actual architecture, per-profile data
  (notes/RAG) is written by tools **directly** into SQLite (`Storage` —
  thread-safe `Arc`+mutex), and `ChatEffect` only exists for `Chat`
  mutations (solely owned by the orchestrator). So **there are no new `ChatEffect`
  variants** — self-model tools write through `ctx.storage`, like `note_save`; the invariant
  "sole owner of `Chat`" is untouched.
- **Entity** `entities/self_model.rs`: `SelfModel { profile_id, version, summary,
  goals: Vec<Goal>, user_model: UserModel }`; `Goal { id, description, status:
  Active/Completed/Abandoned }`; `UserModel { perceived_traits, current_interests,
  relationship_dynamic }`. Methods `new`/`is_empty` (only counts **active**
  goals — a model made only of finished goals isn't considered informative)/`add_goal`/
  `set_goal_status`/`render_for_prompt(max_chars)` (a compact block "[Your self-
  model] About yourself:… / Active goals:… / About the interlocutor:…", truncated by characters for
  the sake of an 8k context). serde `#[serde(default)]`.
- **Storage** `shared/storage/db.rs`: table `self_models(profile_id PK, data JSON,
  version, updated_at)` (`CREATE TABLE IF NOT EXISTS` → without migration); methods
  `self_model_get`/`self_model_upsert` (INSERT OR REPLACE, the version is managed by the store;
  `version` is stored as `i64` — rusqlite doesn't support `u64`). Isolation by PK `profile_id`.
- **Tools** `features/tools/self_model.rs` (4, modeled on `notes.rs`):
  `get_self_model` (read), `reflect` (returns the current model + a rubric for
  self-reflection, without writing — an "entry point"), `update_self_model` (summary + goals:
  add/complete/abandon by id; goal management folded into one tool),
  `update_user_model` (lists replace the previous ones). Mutators read the fresh state from the DB (not
  the `ctx.self_model` snapshot) to see in-turn edits; errors are text, not a
  panic.
- **Optional, off by default** (like the control tools): in `all_tool_ids`, but
  **not** in `default_tool_ids` (`reconcile_tools` doesn't enable them for existing/new
  profiles; profile toggles are built from `all_tool_ids`). DB-only → in
  `effective_tool_ids` they pass through `_ => true` with no global gates.
- **Integration** `app/orchestrator/generation.rs::start_generation`: a snapshot
  `self_model_get(profile_id)` is put into `ToolContext.self_model` and **compactly**
  injected into `request.system` by a pure function `inject_self_model(system, model,
  enabled)` — gated on the profile having enabled `get_self_model` (opt-in). `ToolContext.self_model`
  is currently `#[allow(dead_code)]` (tools read from the DB; the snapshot is kept for the
  round-snapshot's completeness, like `chat_id`). `handle_done` is untouched (no effects).
- **Tests**: entity (goal merging, render/truncation, `is_empty` by active
  goals); db (round trip + per-profile isolation + version growth); tools (get/update/reflect,
  persistence, no-op with no arguments); mod (in the catalog, not in the defaults; passes through
  `effective_tool_ids`); orchestrator (pure `inject_self_model`: gate/empty/
  non-empty). **593 tests green**, clippy/fmt clean.
- **Probe verdict (done)**: a live `self_model_e2e_live` on Gemma 4 12B Q8_0 —
  in session 1 the model called `update_user_model`+`update_self_model` (data hit the DB), in
  session 2 (a new chat of the same profile) it precisely recalled the user/goals via
  injection into `system` + `get_self_model`. Go/no-go criterion — **go**, moved on to Tier 2.

### Post-M9: SelfModel Tier 2-A — narrative insights + contradictions in prose (done)
- **A "self over time" narrative** — the field `SelfModel.narrative: Vec<NarrativeSegment>`
  (`{id, text, created_at}`, `#[serde(default)]`): short insights/observations,
  append-only with a ceiling `MAX_NARRATIVE=50` (keep the freshest, `add_insight`
  trims old ones). **Contradictions are woven in here as prose** — without a separate
  type/`severity`/detect-resolve machinery (a deliberate Tier 2 decision).
- **`add_insight` tool** (`features/tools/self_model.rs`): records an observation
  into the narrative (including noticed contradictions as prose). Optional (in `all_tool_ids`,
  not in `default_tool_ids`), writes directly through `ctx.storage` (like the other
  self-model mutators, without `ChatEffect`). The `reflect` rubric got a question about
  contradictions and a mention of `add_insight`.
- **Render/injection**: `render_for_prompt` adds a "Recent observations:" block from
  the freshest `NARRATIVE_IN_PROMPT=3` insights (newest first; conserving context);
  `is_empty` accounts for the narrative (a model built only of insights is already informative).
- **Tests**: entity (append/ceiling/rendering the freshest); tool (`add_insight` persists +
  empty text → an error); catalog (`add_insight` in `all_tool_ids`, not in the defaults).
  **595 unit tests green**, clippy/fmt clean. Live smoke `self_model_insight_e2e_live`
  (`#[ignore]`) on Gemma 4 12B: the model calls `add_insight`, and prose about a
  contradiction ("volatility in response-length requirements") lands in the DB narrative.
### Post-M9: SelfModel Tier 2-B — self-model viewer screen (`F3`) (done)
- **A read-only viewer screen** of the active profile's self-model
  (`screens/self_model.rs`, `SelfModelScreen`): description, goals (with status
  ●/✓/✗), the interlocutor model, the narrative (newest on top). Opened from chat via
  **`F3`**, closed via `Esc`, `Ctrl+C` — quit; scrolling `↑↓`/`PgUp`/`PgDn`/`Home`.
  A fullscreen rounded panel + a hotkey line (modeled on `chat_list`).
- **Data is owned by the orchestrator** (`Storage`), so the screen **doesn't** open right
  away: `F3` → `ChatIntent::OpenSelfModel` → `runtime` sends `AppCommand::RequestSelfModel` →
  `handle_request_self_model` loads `self_model_get(active profile)` and emits
  `AppEvent::SelfModelView(Box<Option<SelfModel>>)` → `runtime::apply_event` creates
  `ActiveScreen::SelfModel`. A new `ActiveScreen` variant (the 4th screen); FSD is honored
  (`screens` doesn't know about `app`). The screen's palette is updated from `Settings` (theme).
- **Editing the model — for now only through the model itself** (self-model tools); manual
  editing via the UI — Tier 3 groundwork (data in SQLite, not JSON).
- **Tests**: screen (Esc/Ctrl+C intents layout-independent, scroll clamping,
  rendering an empty/populated model without a panic); runtime (`OpenSelfModel` sends
  `RequestSelfModel` and does NOT open the screen right away; `SelfModelView` opens the screen).
  **601 unit tests green**, clippy/fmt clean. Not tested against a live model
  (an interactive TUI — needs a real terminal). Added to the help overlay (`F1`/`?`).

### Post-M9: SelfModel Tier 3-A — narrative/injection parameters in settings (done)
- **Narrative size and prompt-injection volume moved from constants into config**
  (`config.self_model: SelfModelSettings` — `max_narrative`/`narrative_in_prompt`/
  `prompt_cap`; `#[serde(default)]` → old `settings.json` without migration; defaults =
  the previous 50/3/1200). The formerly hardcoded `MAX_NARRATIVE`/`NARRATIVE_IN_PROMPT`/
  `DEFAULT_PROMPT_CAP` are removed.
- **`SelfModelParams` type** (`entities/self_model.rs`, an analog of RAG's `ChunkParams`):
  `from_settings` sanitizes (max≥1; the prompt gets no more than what's stored; cap≥100).
  Entity methods are parameterized: `add_insight(text, max_narrative)`,
  `render_for_prompt(prompt_cap, narrative_in_prompt)`. Threaded into `ToolContext.
  self_model_params` (built from `config.self_model` in `start_generation`);
  the tools (`add_insight`/`render_or_empty`) and `inject_self_model` use it.
- **UI**: three numeric fields in the "Tools" section of the settings screen (next to the RAG
  chunking ones) with description tooltips (`field_description`); `FieldId::Sm*`.
- **Tests**: config (defaults in `partial_json_fills_defaults`); entity (custom
  parameters cap storage/injection; sanitization of inconsistent settings).
  **603 unit tests green**, clippy/fmt clean.

### Post-M9: SelfModel Tier 3-B — auto-reflection (background) (done)
- **Auto-reflection** (`app/orchestrator/reflection.rs`): every N assistant replies
  in a chat, a background task asks the model to review the recent conversation and
  **itself** update the "self-model". Enabled via `config.self_model.auto_reflect_every` (0 —
  off, by default). This was a deferred item from the original plan ("reflection after
  every N messages").
- **A mini agentic loop, not a single-turn request** (unlike auto-naming): reflection
  is given self-model tools (`get/update_self_model`/`update_user_model`/
  `add_insight`, intersected with the profile's set), and the loop **executes** their calls
  (the tools write directly into `Storage`). Up to `REFLECT_MAX_ROUNDS=6` rounds,
  a 120s timeout. The chat is **not mutated**, nothing is streamed to the UI — reflection
  is silent. The system message asks it to change only what actually changed and not write a reply
  to the user, only call tools.
- **Trigger** in `handle_done` (after a successful reply): `maybe_auto_reflect`
  counts replies (`reflect_counts: HashMap<chat_id,u32>`), resets and
  fires at the threshold. Gates: the feature is enabled, the profile enabled `get_self_model` (as with
  the injection), reflection isn't already running (`reflect_cancel`, one at a time), the server is `Ready`,
  there's enough of a conversation (a digest exists). A pure `due(count, every)` — testable.
- **Plumbing**: fields `reflect_cancel`/`reflect_counts`/`reflect_done_tx` on
  `Orchestrator`; an internal channel `reflect_done` (background → the loop clears the flag
  `handle_reflect_done`); cancellation on `Quit`. The digest — `rename_chat::
  build_conversation_digest` (as for auto-naming).
- **UI**: a "Self-model: auto-reflection (every N)" field in the "Tools" section
  of settings (`FieldId::SmAutoReflect`) with a tooltip.
- **Tests**: `due` (threshold/disabled); **604 unit tests green**, clippy/fmt clean.
  Live smoke `auto_reflect_e2e_live` (`#[ignore]`, polls the DB — reflection without
  a UI event) on Gemma 4 12B: with `auto_reflect_every=1`, after the first reply the model
  **on its own** (without being explicitly asked) called `update_user_model` → traits/
  interests of the interlocutor showed up in the DB.

### Post-M9: SelfModel Tier 3-C — manual model editing in the UI (`F3`) (done)
- **The `F3` screen became editable** (was read-only): editing the self-description, goals
  (add/rename/cycle status `Space`/delete `Del`), the interlocutor model
  (traits/interests as a comma-separated list, relationship dynamic), deleting
  narrative insights (`Del`), full clearing (`Ctrl+K` twice — with confirmation).
  Navigation `↑↓`/`Home`/`End`, `Enter` — edit (a text editor popup, self-
  description multi-line), `Esc` — close, `Ctrl+C` — quit.
- **Edit type** `SelfModelEdit` (`entities/self_model.rs`) + a pure `SelfModel::
  apply_edit(edit) -> bool` (whether it changed) and `cycle_goal_status`. UI↔
  orchestrator contract; the trait/interest lists are **replaced in full**.
- **Flow** (an edit doesn't mutate `Chat`, it goes through the `Storage` owner):
  `SelfModelIntent::Edit` → `AppCommand::UpdateSelfModel` → `handle_update_self_model`
  (load/create the profile's model → `apply_edit` → on change `self_model_upsert`
  → **re-emit** `AppEvent::SelfModelView`). `runtime::apply_event` updates the **already
  open** screen in place (`set_model`, keeping the selection), rather than recreating it.
  Clipboard paste is routed into the active field editor.
- **The editor** reuses `widgets::input_box::InputBox` (single-line for
  fields/goals/lists, multi-line for the self-description) — the same pattern as in
  the settings screen and chat rename.
- **Tests**: entity (`apply_edit` covers every operation + no-op on a repeat/nonexistent id);
  screen (Enter→edit summary, `Space`/`Del` on a goal, adding a goal + empty no-op,
  `Ctrl+K` confirm/cancel, list parsing, rendering an empty/full model without a panic);
  orchestrator (`update_self_model_persists_and_reemits` — an edit is saved and
  re-emitted). **610 unit tests green**, clippy/fmt clean. Manual editing
  via the UI is no longer Tier 3 groundwork — done.

### Post-M9: configurable `Ctrl+E`/`Ctrl+R` confirmation (done)
- **The UI-irreversible `Ctrl+R` (regenerate) and `Ctrl+E` (delete an exchange) can
  be protected with confirmation** — a new setting `interface.confirm_destructive_keys`
  (`#[serde(default)]` → old `settings.json` without migration; off
  **by default**, i.e. the previous instant behavior). A "Confirm Ctrl+R /
  Ctrl+E" toggle in the "Interface" section of settings (`FieldId::IConfirmKeys`, with a tooltip).
- **UX — a single modal popup** (by agreement: one shared toggle for both
  operations, a Yes/No popup): when the setting is on, the key press opens a
  "Confirmation" popup (`Enter` — yes, `Esc` — no; other keys are ignored, the popup
  stays open) instead of acting immediately. State — `ConfirmAction`
  (`Regenerate`/`DeleteExchange`) in the `ChatScreen.confirm` field; the intent
  (`RegenerateLast`/`DeleteLastExchange`) is only emitted on `Enter` and only if
  generation isn't in progress. Helpers `trigger_destructive` (open the popup or hand off the
  intent right away) and `handle_confirm_key`; rendering — `render_confirm` (a centered
  popup + `dim_background`, like help/spellcheck). The flag is picked up from the settings
  snapshot in `set_settings` (like the theme palette). See spec §11.7.
- **Tests** (`screens/chat.rs`): the popup opens and `Enter` confirms
  (`RegenerateLast`); `Esc` cancels; other keys neither close the popup nor
  get typed into the input box; with the setting off, the intent is emitted immediately.
  **614 tests green**, clippy/fmt clean.

### Post-M9: CoT (extended thinking) for Claude mode (done)
- **Claude (Anthropic) can now do "thoughts" (CoT)** — previously `anthropic/wire::build_request`
  only sent `max_tokens`, so reasoning was never requested. The receiving side was already
  ready (the client maps `thinking_delta`→`ChatChunk::Thoughts`, the feed renders the "thoughts"
  block). Two phases: **A** — CoT in chat without tools; **B** — CoT with tool use
  (critical — tools are central to the application). See CLAUDE.md (the engine section).
- **Phase A — enabling thinking** (`anthropic/wire.rs`): the request carries
  `thinking:{type:"adaptive", display:"summarized"}` when `sampling.thinking==Some(true)`
  (+ `output_config.effort` from `reasoning_effort`, except `none`). **`budget_tokens`/
  `reasoning_budget` are not sent** — Claude 4.x models reject them (`400`); only adaptive,
  depth is set via `effort`. `display:"summarized"` is needed so the "thoughts" text arrives
  non-empty (default `omitted`). `supported_sampling_fields(Claude)` extended with
  `thinking`/`reasoning_effort` (mirroring the dialect) → the settings screen and `get/set_sampling`
  now show/edit them for Claude; `top_k`/penalties remain hidden.
- **Phase B — thinking-block signature with tool use** (the main protocol nuance): Anthropic
  requires an assistant turn with `tool_use` to carry **its own thinking block with a `signature`**
  within the same turn — otherwise the next request in the round gets `400`. Implementation:
  - Parse `signature_delta` (`anthropic/wire::AntDelta::SignatureDelta`) → a new chunk
    `ChatChunk::ThoughtsSignature(String)` (the client emits it; other backends don't).
  - `RoundOutput.thoughts_signature` accumulates the round's signature; the agentic loop
    (`orchestrator/generation.rs`), when a signature is present, attaches a thinking block to
    `ApiMessage::assistant_tool_calls(...).with_thinking(...)` at the point it's pushed into the request
    history. The type `ApiMessage.thinking: Option<ThinkingBlock>` (text+signature) is generic,
    used only by the Anthropic wire; `anthropic/wire::build_messages` puts
    `AntBlock::Thinking` **first** in the assistant turn.
  - **The signature lives only in the memory of one `spawn_generation` call** — it is NOT persisted in
    the domain `Message`/JSON: Anthropic only requires thinking on the most recent
    assistant turn, and between turns (reload/new request/regeneration) old
    thinking blocks are auto-discarded by the server. `message_to_api` (history) doesn't carry thinking —
    which is correct for Anthropic. Unfinished tool turns from history aren't rebuilt
    (regeneration truncates up to the last user message), so persistence isn't needed.
- **A generic contract, backends don't break**: the new `ChatChunk` variant and the
  `ApiMessage.thinking` field are ignored by llama.cpp/OpenAI; the llama.cpp thinking path is unchanged
  (`reasoning_budget`/`<think>`). Exhaustive `ChatChunk` matches were updated across all
  consumers (title/impersonation/reflection/subagent/fetch/openai-client/generation).
- **Tests**: wire (thinking off by default; adaptive+summarized+effort when enabled;
  `budget_tokens` isn't sent; the thinking block comes before tool_use; no signature → no block;
  parsing `signature_delta`); `supported_sampling_fields`/the `set_sampling` schema updated.
  **622 tests green**, clippy/fmt clean. Live `#[ignore]` smokes
  (`anthropic/client.rs`, `MINDFORK_ANTHROPIC_KEY`) **verified against the real Anthropic
  API**: `extended_thinking_streams_thoughts_and_signature` (Phase A — "thoughts"
  and a signature arrive) and `thinking_with_tool_use_round_trips_signature` (Phase B — the second round with
  the signature resent goes through without `400`). A Phase B smoke nuance: guaranteeing a
  thinking block requires a prompt with an explicit reasoning step + `reasoning_effort: High` —
  on a trivial request, adaptive thinking skips the reasoning step (no signature, which is
  correct in itself — no block is needed without "thoughts").
- **Known limitation**: `redacted_thinking` blocks (a rare protective classifier
  response) aren't handled yet — if such a block arrives before tool use, it
  won't be resent (a possible `400`). Very rare in normal use; groundwork noted.

### Post-M9: SelfModel — partial display of a long item on the `F3` screen (done)
- **Symptom**: on the "Self-model" screen (`F3`), a list item with a multi-line value
  (a long description/insight) that didn't fully fit in the remaining height was **not
  shown at all** — an empty spot in its place, creating the illusion of the list ending.
- **Cause**: the list was rendered with the `List` widget, which **entirely skips**
  a multi-line item that doesn't fit height-wise in the remaining area (its
  `get_items_bounds` stops at `height + item.height() > max_height`). The chat list
  window doesn't suffer from this — its items are single-line.
- **Fix** (`screens/self_model.rs`): the `List` widget was replaced with a **manual row-by-row
  rendering** of visual rows. Each logical line is expanded into visual
  rows (`wrap::wrap_line`), rendered one `Paragraph` per row of the area;
  a trailing item that doesn't fit height-wise gets **clipped at the bottom edge** (its
  top is visible), rather than skipped. A persistent `scroll` field was added (the first visible row)
  + a pure `adjust_scroll(scroll, sel_start, sel_height, view_h)`: keeps the selected
  row visible, and if it itself is taller than the window, pins to its **top**. The selected
  row's highlight stays the same — a backdrop across the whole row width (the base `Paragraph` colors the
  whole area) + a `▌` marker on each of its visual rows. Navigation/selection remain by
  logical lines (key behavior unchanged).
- **Tests**: `adjust_scroll_keeps_selection_visible_and_pins_top_of_tall_item`
  (the scroll logic) and `render_long_trailing_item_in_short_area_does_not_panic`
  (a long insight in a tight window). **625 tests green**, clippy/fmt clean.

### Post-M9: configurable conversation-copy composition (`F5`) (done)
- **Copying a chat's conversation to the clipboard (`F5`) became configurable**: a new
  section `AppConfig.copy: CopySettings` (`copy_thoughts`/`copy_tool_calls`/`copy_tool_results`,
  `#[serde(default)]` → old `settings.json` without migration; all flags off
  **by default** — the previous "message text only" behavior). By choice,
  "thoughts" (CoT, before the text), tool-call parameters (name + arguments)
  and their results are added to the assistant block.
- **`features/chat_export.rs::format_conversation` takes `&CopySettings`**:
  the optional blocks are drawn from `Message.tool_calls` (name/`arguments`/`result`) —
  separate tool messages are still skipped (avoiding duplication). Tool block:
  a `[Tool: name]` header (printed when either of the two flags is on) +
  `Arguments: {json}` / `Result: …`. "Thoughts" — a `[Thoughts]` block. An assistant
  message with no text but with tool calls is included if the corresponding option
  is enabled (otherwise skipped, as before). Pure logic — the orchestrator
  (`chats.rs::handle_copy_chat`) passes `&self.config.copy`.
- **UI**: three toggles in the "Interface" section of settings (`FieldId::ICopyThoughts`/
  `ICopyToolCalls`/`ICopyToolResults`) with description tooltips (`field_description`).
- **Tests**: chat_export (by default text only; "thoughts" before the text; parameters/
  results under a shared header; a message made only of tool calls); config (defaults
  off). **631 tests green**, clippy/fmt clean.

### Post-M9: notes connectivity — MVP probe (Tier 1, done)
- **A shift of memory from accumulation to integration** per the plan
  [docs/notes-connectivity.md](docs/history/notes-connectivity.md). Motive — from a live
  conversation with the model (the "self-aware AI" profile): "inertia lives **not in
  accumulating notes, but in their connectivity**"; "identity — the mode in which I relate to notes
  (what I accept, what I reject, what I rewrite)." Previously notes could only pile up
  (a flat list, substring search, append-only). The probe checks the **behavioral**
  payoff (will the model start rewriting a duplicate instead of writing an almost-copy, will
  semantic search find relevant content that substring search missed) — go/no-go before Tier 2
  (an explicit graph), as with the SelfModel probe.
- **Three MVP pieces** (Tier 1; graph/supersede/merge/consolidation — deferred):
  1. **Embeddings on notes + a semantic `note_recall`**. A side table
     `note_vectors(note_id PK, profile_id, embedding TEXT)` (`CREATE TABLE IF NOT
     EXISTS` → without migration; the vector — a JSON f32 array, **deliberately NOT vec0**: notes
     number in the tens–hundreds, cosine similarity is computed brute-force in Rust — `db::cosine` +
     `note_search_semantic`, isolated by `WHERE n.profile_id`). `note_recall` with a query
     goes the semantic route; **graceful degradation** (like RAG reranking) — the embedder is
     unavailable (`UnavailableEmbedder`)/notes have no vectors → fall back to substring/tag
     matching (`semantic_recall(...) -> Option`, `None` → the previous `note_list`). Tags are a filter
     on top of the ranking.
  2. **`note_revise(id, content)`** — the core of integration: rewrite a note in place
     (`db::note_update`, isolated via `WHERE … AND profile_id`, `updated_at` bumps →
     it bubbles up in the list) + re-embedding (best-effort). A foreign/nonexistent id →
     a text error, not a panic. **In `default_tool_ids`** (safe, DB-only, central)
     → `reconcile_tools` will enable it for existing profiles too.
  3. **Compatibility gate in `note_save`** — after insertion it embeds (best-effort) →
     `note_vector_upsert` → semantically close existing notes get appended to the
     result ("Similar notes … rewrite via note_revise instead of a new entry"),
     so the model **at save time** sees a duplicate/conflict and decides: keep / rewrite
     / don't proliferate. This is the "ability to say no."
- **Backfilling "old" notes** (`ensure_note_vectors`, `db::notes_missing_vectors`):
  notes without a vector (created before the feature, imported, saved while the embedder was
  unavailable at the time) weren't seen by semantic search/the gate — a finding from a live test (the model
  called `note_recall` and didn't see notes it created earlier). The helper transparently
  (at the start of `semantic_recall` and before the `note_save` gate) embeds missing notes in batches
  (best-effort). Effectively a one-time cost per profile — afterward the list is empty and the call is nearly
  free (a single SELECT).
- **Architecture**: tools write **directly** through `ctx.storage` (like `note_save`),
  **without new `ChatEffect` variants** (notes aren't `Chat` state); the invariant "sole
  owner of `Chat`" is untouched. The orchestrator wasn't changed.
- **Tests**: db (`note_update` only affects its own profile; `note_search_semantic` ranks and
  isolates; upsert replaces the vector); tools (the gate surfaces a similar note on save;
  semantic recall finds a non-substring match; revision rewrites in
  place; a bad/nonexistent id; backfilling "old" notes for both recall and the
  `note_save` gate); mod (`note_revise` in the defaults). Semantic tests use `MockEmbedder`
  (a bag-of-characters + L2). **642 tests green**, clippy/fmt clean. A live run/evaluation of the
  probe on a local model — a manual step (a go/no-go criterion in the plan).
- **The probe was verified against a live model** (the model surfaced similar notes) → go,
  moved on to Tier 2 (below).

### Post-M9: notes connectivity — Tier 2 (graph + revision history, done)
- Continuation of Tier 1 ([docs/notes-connectivity.md](docs/history/notes-connectivity.md)):
  note connectivity as an **explicit structure** + integration via supersession/merging.
- **A link graph**: a table `note_links(profile_id, from_id, to_id, relation,
  created_at)` (a PK against duplicates, indexes on from/to, isolated by `profile_id`,
  `CREATE TABLE IF NOT EXISTS` → without migration). Tools `note_link(from_id, to_id,
  relation)` — types `supports`/`contradicts`/`refines`/`relates` (idempotent
  `INSERT OR IGNORE`, checking `note_is_active` at both ends, no self-links) and
  `note_neighbors(id, relation?)` — neighbors in both directions (`UNION ALL` from/to) with
  direction (→/←), excluding superseded ones. `note_recall` mixes in a "Related
  notes" block — **spreading activation** (neighbors of the top-3 hits, excluding those already
  shown/superseded, up to `RELATED_IN_RECALL=5`), so recall surfaces the whole cluster.
- **Revision history with a "scar"**: a table `note_superseded(note_id PK,
  profile_id, superseded_by, superseded_at)`. `note_supersede(old_id, content)` creates
  a new version (`create_note` = insert + embed) and marks the old one superseded;
  `note_merge(ids[], content)` folds ≥2 active notes into one, the originals are superseded.
  Superseded ones are **hidden** from `note_list`/`note_search_semantic`/`notes_missing_vectors`/
  `note_neighbors` (an anti-join against `note_superseded`), but kept for the change trail and
  manual recovery. A simple in-place edit is provided by `note_revise` (Tier 1).
- **Unchanged architecture**: all five tools are DB-only, in `default_tool_ids`
  (`reconcile_tools` will enable them for existing profiles), write directly via
  `ctx.storage`, no `ChatEffect`; the orchestrator untouched.
- **Tests**: db (supersede hides from the list/semantics/`is_active`; links and neighbors in
  both directions + filter by type + idempotency + a superseded neighbor disappearing);
  tools (link→neighbors + an unknown type/self-link; supersede hides the old one, shows the
  new one; merge folds and requires ≥2; recall surfaces a related but non-similar note).
- **Fixes from a live-model stress test** (the model exercised the graph's edge cases):
  - **`note_link` — an honest answer about a duplicate.** Previously repeating the same
    link answered "Link created" both times (there's no duplicate in the DB — there's a PK + `INSERT OR IGNORE`, but
    the message was misleading). Now `note_link_insert` returns "was it created"
    (row count), and the tool answers "The link already existed" when nothing was inserted.
  - **`note_revise` — a graph-integrity warning.** An in-place edit of a node with
    incoming edges could make them wrong (created a `contradicts` link when the note
    denied X; after the revision it asserts X — the edge now lies). `note_revise` now, given
    existing links (`note_link_count`), adds a warning and points to
    `note_supersede` (which preserves the superseded version the links refer to). **Not a
    ban** — the judgment call remains with the model (a cheap edit of unlinked notes doesn't suffer).
  **649 tests green**, clippy/fmt clean.
- Tier 2 merged (PR #86), the live-model stress test passed cleanly → Tier 3 (below).

### Post-M9: notes connectivity — Tier 3 (consolidation / "sleep", done)
- Completion of the track ([docs/notes-connectivity.md](docs/history/notes-connectivity.md)):
  integration happening **outside a single chat** (the project's original goal).
- **`consolidate_notes`** — a read-only entry point (like SelfModel's `reflect`): a
  knowledge-base overview — similar pairs (possible duplicates, pairwise cosine ≥ 0.85),
  `contradicts` links, notes with no links — + a rubric. The overview logic —
  `notes::build_consolidation_overview` (a pure DB read: `db::notes_with_vectors` +
  `db::note_links_all`). It changes nothing itself; the model then calls merge/supersede/
  revise/link.
- **Carrying links over on merge**: `note_merge` moves the edges of the source notes onto
  the merged one (`db::note_links_retarget` — deduped by PK, self-loops dropped), so the graph
  doesn't get orphaned.
- **Auto-"sleep"** (`app/orchestrator/consolidation.rs`, modeled on `reflection.rs`):
  every N replies (`config.notes.auto_consolidate_every`, opt-in, 0=off), a background
  task = a **mini agentic loop** with note tools; it's fed the overview, the model
  consolidates on its own (its calls execute against `Storage`). Gates: the feature is enabled, the
  profile enabled `note_merge`, active notes ≥ 2, the server is `Ready`, one at a time; the
  chat/feed aren't touched. Plumbing mirrors reflection: fields `consolidate_cancel`/`_counts`/`_done_tx`,
  an internal channel, cancellation on `Quit`, invocation in `handle_done`. A "Notes:
  auto-consolidation (every N)" field in settings.
- **Tests**: db (`notes_with_vectors` only active ones; `note_links_retarget` moves/
  dedups/drops self-loops); tools (`consolidate_notes` shows duplicates/
  orphans; `note_merge` carries links to the new note); consolidation (`due`).
  **654 tests green**, clippy/fmt clean. A live evaluation of auto-"sleep" — a manual step.
- **Out of scope (groundwork)**: vec0 as the note count grows; **linking memory organs**
  (notes / self-model narrative / RAG are unconnected) — an observation from the live test, a separate
  large track.

### Post-M9: data storage mode + backup/restore (done)
- **Data storage mode via a `location.json` marker** next to the binary
  (`shared/paths.rs`, type `DataLocation`: `portable`/`system`/`path`). By default
  (no marker/empty/`portable`) — data goes into a **`data/` subdirectory next to the binary**
  (`PORTABLE_DATA_SUBDIR`; the subdirectory separates data from build tooling files/caches —
  in dev, `target/debug/data/`, so it doesn't mix with build artifacts). `system` → a standard
  OS folder (the `directories` crate: Windows `%APPDATA%\mindfork-rs\data`, Linux
  `~/.local/share/mindfork-rs`); `path` → an arbitrary directory (no `data/` subfolder —
  the user specified an exact location). `Paths::discover()` reads the marker, resolves the root,
  and **creates it for every mode** (including the portable `data/`); the marker itself
  **always** sits next to the binary (outside `data/`) — it's about the installation, not
  user data. **A corrupted marker JSON is a startup error** (a typo in the
  path shouldn't silently drop you onto an empty dataset). Logs/instance-guard/storage follow
  the root automatically (`discover` → `logging::init`); `build.rs` copies dictionaries into
  `<profile>/data/dictionaries/`. A new accessor `paths.backups_dir()`. **Changing
  the layout**: previous portable data sat right in the binary's directory — after switching,
  it needs to be moved into `data/` once (auto-migration is deliberately not done).
- **Backup/restore** (`features/backup.rs`, a `clap`-based CLI):
  `mindfork backup [-o FILE] [-c 0..9]` and `mindfork restore <archive>` (no TUI,
  they end the process, like `import-lamellama`; they take the single-instance lock — protecting `data.db`
  from a race with a running application). `--import-lamellama` was switched to the same
  `import-lamellama` clap subcommand.
- **Zip contents** (paths relative to the root): `settings.json`, `profiles.json`,
  `data.db` (+ its sidecar `-wal`/`-shm`, if present), `personal_dictionary.txt`,
  the `chats/` and `dictionaries/` directories recursively (pulling in their own `*.bak`), all top-level
  `*.bak` files (`settings.bak`/`profiles.bak` — this project's `.bak` is
  `with_extension`, i.e. `settings.bak`, not `settings.json.bak`), and the
  "sandbox" directory `tools.fs_root` — **only if** its canonicalized path is inside
  the canonicalized root (otherwise skipped). **Excluded**: `backups/`, `logs/`,
  `location.json`. Entries in the archive are deduped by name. Compression level `0..=9` (0 → store,
  else deflate; clap validates the range, default 9). The default name —
  `backups/mindfork-backup-<date>.zip` (chrono Local).
- **Restore is transactional** (`restore_backup` → `RestoreOutcome`): (1)
  **archive validation before any destructive action** (opening the zip + checking
  `enclosed_name` for each entry — anti-zip-slip); an invalid/corrupted archive → `Err` without
  any cleanup. (2) If the root has data (`settings.json`|`profiles.json`|`data.db`|
  a non-empty `chats/`) → an **automatic pre-restore copy** of the prior data into `backups/`. (3)
  Clearing the whitelisted set (the same composition; **preserving**
  `backups/`/`logs/`/`location.json`; the cleanup is by whitelist, not "wipe everything" — stray
  files in the root are untouched) → unpacking the given archive into the root. (4)
  **If the unpacking fails partway through** and a pre-restore copy was made → an **automatic
  rollback**: another cleanup + unpacking the pre-restore copy (`RolledBack`); if the
  rollback itself fails → `Failed`, with the path to the pre-restore copy for manual recovery. `main.rs` reports
  every outcome to the console (stdout isn't occupied by the TUI). An `Err` from restore
  (before any destruction) and `Failed`/`RolledBack` give a nonzero exit
  code.
- **Dependencies**: `zip` (`default-features=false`, only `deflate` — no bzip2/zstd
  C dependencies), `clap` (derive), `directories`.
- **Tests** (`backup.rs`, on a tempdir): archive contents (expected files included, logs/
  backups/marker excluded); `fs_root` is only included when under the root; round-trip
  restore (replacing data + a pre-restore copy + removal of a stale chat); refusal on a
  corrupted archive without touching the data; restore into an empty root without a pre-restore copy;
  store-level (0) produces a valid archive; **rollback on an unpack failure** (a crafted archive with
  a file entry conflicting with a same-named directory surviving the cleanup →
  `RolledBack`). `paths.rs`: default-portable, a whitespace marker, round-trip of the three
  modes, path trimming, an error for an empty path/corrupted JSON, the system path contains the
  application name. **669 tests green** (+18 `#[ignore]`), clippy/fmt clean.

### Post-M9: scrolling with VS16 emoji without flicker (done)
- **A full feed redraw when scrolling with "drifting" VS16 emoji (`🕸️`/`🗂️`)
  no longer flickers.** Previously `app/runtime.rs` called `terminal.clear()`, which sends
  a screen-clear escape `ESC[2J` — the screen blanks for a moment (flicker). The point of the redraw is to
  wipe "hanging" artifacts: conhost/Command Prompt draw a VS16 cluster wider
  than ratatui's model, content drifts, and the per-cell diff doesn't reach the drifted character.
  Every cell needs to be explicitly rewritten, including spaces in empty spots.
- **A dead end (important)**: "lightening" `clear()` down to a plain back-buffer reset
  (`swap_buffers`) is NOT ALLOWED — then the diff compares "empty → frame" and **skips
  space cells** (they equal the empty back buffer): empty spots don't get
  redrawn, the old content/artifact stays visible (visually, a "mush" of
  overlaid text). The diff only emits cells that DIFFER.
- **Fix**: make the back buffer differ from **any** real cell. We fill
  the current buffer with a sentinel character `"\0"` (`current_buffer_mut().content`; such
  a character never occurs in real content) and move it into the back buffer via
  `swap_buffers()` — **without** flushing to the screen (flush isn't called). Then the next `draw`
  diffs "`\0` → the real frame": every cell differs (spaces too) → ratatui
  rewrites the whole screen cell-by-cell, **without** `ESC[2J` (no flicker) and with spaces in
  empty spots. The internal swap inside `draw` restores the invariant "back buffer =
  screen"; the `"\0"` itself never reaches the screen. The trigger is unchanged (`take_feed_scrolled`:
  VS16 is present and a scroll happened). The intra-line drift caused by a wide emoji is inherent to the
  terminal (the same as with the old `clear()`), not a regression. Real conhost behavior
  isn't caught by unit tests (TestBackend) — verified on a live terminal.

### Post-M9: compact status chips for all servers in the status bar (done)
- **The status bar showed one server** (chat) as a single pill `● server: ready`,
  whereas there are now several servers: chat, **embeddings**, **impersonation**. Now —
  a cluster of compact **chips** (glyph + label), one per active server.
- **Data model**: a new snapshot `ServerStatuses { chat, embed, impersonation }`
  (`shared/server.rs`); the event `AppEvent::ServerStatus` now carries it instead of a single
  `ServerStatus`. The orchestrator emits the snapshot on **any** status change:
  the chat background probe (`status_rx`), the impersonation probe (`imp_status_rx` — previously its
  status never reached the UI at all), and settings changes (`apply_chat/embed/impersonation_
  settings` via the `emit_server_status` helper). `EngineManager` got
  `embed_status` + a `statuses()` method; `apply_chat` no longer returns a status
  (it's read via `statuses()`).
- **The embeddings status wasn't tracked before** (`apply_embed` is lazy, no probe):
  `EmbedSetup` got a `status` field — `Ready` for a real embedder,
  `NotConfigured` for `UnavailableEmbedder`. There's no embeddings probe → the chip is binary
  (configured/hidden); a real probe is groundwork.
- **Render** (`widgets/status_bar.rs`): a glyph encodes the status — `●` ready (success),
  `◐` connecting (warning), `✕` no connection (error) / not configured (warning). **Chat
  is always shown** and, on disconnect, carries the reason (`✕ chat: no connection: <why>`) — it
  blocks generation; **embeddings/impersonation** — chips `emb`/`imp` shown **only
  when configured** (`NotConfigured`, including impersonation in `shared` mode → the chip hidden),
  without the reason text (compact). Glyphs are 1 column wide (no emoji) — the layout of the
  status line/hotkey grid doesn't "shift". The `Palette::pill` helper was removed (its role now
  belongs to the local `chat_chip`/`secondary_chip`/`chip` with different glyphs).
- **Tests**: chips are visible only for configured servers (impersonation in `shared` mode/
  embeddings off → hidden); glyph color follows status; earlier layout/token/
  mouse tests were ported to `ServerStatuses` (the grid-wrap width was tuned for the shorter
  chip). **671 tests green**, clippy/fmt clean.

### Post-M9: SelfModel — repairs from live-test findings (Opus 4.8 API) (done)
- **Following manual testing** of the "self-model" on Opus 4.8 (Anthropic API),
  identified and fixed issues that left effectively only the narrative
  (`add_insight`) working. No new tools, no DB schema change (the model is a JSON
  blob `self_models.data`, the new config flag is via `#[serde(default)]`) —
  **no migration needed**. Fixes touch rendering, tool arguments, descriptions, and
  one toggle.
- **Ellipsis in `get_self_model` (truncation).** Cause: read tools called the same
  **compact truncated** render that goes into the system prompt
  (`render_for_prompt(prompt_cap=1200, narrative_in_prompt=3)`) — the model saw "…"
  and complained. Fix: new `SelfModel::render_full()` (no truncation, full
  narrative, goals with id) — for `get_self_model`/`reflect`/echo after edits;
  `render_for_prompt` remains only for passive injection.
- **Goals never closed (write-once).** Mechanical blocker: `render_for_prompt`
  printed a goal as `- {description}` **without an id**, and `get_self_model`
  called exactly that → the model **never saw a goal's id**, while
  `complete_goals`/`abandon_goals` require one. Fix: `render_full` shows goals as
  `- #a1b2c3 (active) …` (short id = first 6 hex chars of the UUID) + recently
  closed ones (lifecycle visible); the resolver `SelfModel::match_goal` accepts a
  short `#id` **or** a full UUID (by unambiguous prefix, case-insensitive;
  ambiguity/miss — a clear report, not a panic). The `reflect`/auto-reflection
  rubrics were rewritten around "manage goals by #id".
- **`update_user_model` overwrote (the "mood swing" bug).** Cause: lists were
  replaced wholesale (`perceived_traits = […]`) — good mood → "kindest", bad mood →
  "merciless", each edit destroyed the accumulated data. Fix (notes philosophy
  "integration over accumulation"): **merge semantics** — `add_traits`/
  `remove_traits`, `add_interests`/`remove_interests` (case/Unicode-insensitive
  dedup) instead of replacement; the description was reframed as a *stable,
  integrated* model of the interlocutor (not a mood snapshot), transient
  observations → `add_insight`. The manual edit via `F3` (`SelfModelEdit::SetTraits`)
  remains a replacement — there a human is in the loop.
- **`update_self_model.summary`** — remains a replacement (natural for a coherent
  self-description), but the description/rubric now ask to **integrate** the
  previous with the new rather than rewriting from scratch; the current
  description is always visible (injection + `render_full`).
- **Predictability of tools independent of persona.** (1) A persona-neutral
  **"self-model maintenance protocol"** (`generation.rs::SELF_MODEL_MAINTENANCE_PROTOCOL`)
  is mixed into the system prompt on top of the profile persona: when to record
  changes, "transient — into observations", **"accuracy over agreeableness"** (a
  direct counter-measure against flattery from a kind persona). Toggle
  `config.self_model.maintenance_protocol` (`#[serde(default)]`, **on by default**),
  gated on the profile having enabled `get_self_model`; mixed in even for an empty
  model (bootstrapping the first record). (2) Background **auto-reflection**
  (`reflection.rs`, `auto_reflect_every`) remains **opt-in** (0 by default —
  background Opus API calls cost tokens), but its system message was rewritten
  around the new semantics/goals-by-id — enabling it gives you a deterministic
  writing path independent of the model's spontaneity.
- **UI**: "Self-model: maintenance protocol" toggle in the settings screen's
  "Tools" section (`FieldId::SmProtocol`, with a hint).
- **Tests**: entity (`render_full` shows goal ids and is not truncated;
  `match_goal` prefix/full/ambiguous; merge add/remove on user_model); tools
  (merge doesn't overwrite; closing a goal by short `#id`; goal miss — a report);
  orchestrator (injection: protocol on with an empty model, both with a non-empty
  one, off-behavior). **677 tests green** (+6), clippy/fmt clean. Live evaluation on
  Opus 4.8 — the next manual step.

### Post-M9: SelfModel Tier 2 — revision scar + narrative consolidation (done)
- **From the model's (Opus 4.8) feedback on the previous fix**: switching from
  "overwrite" to "accumulate" (add_/remove_) cured the *overwriting*, but pure
  accumulation has the mirror disease — **bloat and drift** (traits growing to
  forty, duplicates/staleness drowning the signal, and `remove_traits` erasing
  without a trace — "the biography doesn't remember it changed"). The identity
  fork of the feature was resolved: **the self-model is the current working
  snapshot of conclusions, while the scar-biography lives in the narrative** (we
  don't structure traits or clone the notes graph — the narrative already plays
  the role of "self-directed notes"). The DB schema was untouched (JSON blob +
  new entity methods), no migration needed.
- **Trait-revision scar (problem A)**: `update_user_model` gained an optional
  `note` — what changed and why; when present it goes into the narrative
  (`add_insight`), so a change of opinion about the interlocutor leaves a
  **trace** rather than vanishing without one. If `remove_traits`/
  `remove_interests` are non-empty and `note` wasn't passed — the result
  **reminds** the model to leave an explanation (the same warning pattern as
  `note_revise`). Traits remain a flat `Vec<String>` (no migration).
- **Consolidation against bloat (problem B)**: new tool
  `consolidate_narrative(remove[], add?)` — removes observations by `#id`
  (resolved the same way as goals: short hex prefix or full UUID,
  ambiguity/miss → a report) and optionally adds one **summarizing** entry in
  their place. This is integration (the durable is raised into
  summary/traits, the raw is folded/cleared), not silent loss from a FIFO cap —
  a mirror of `note_merge`/`consolidate_notes` in the self-model idiom,
  **without a graph**. Insights now show up in `render_full` with `#id`; the
  entity gained `match_insight`/`remove_insights` (a shared `resolve_handle`
  resolver with `match_goal`).
- **Optional, DB-only**: `consolidate_narrative` is in `all_tool_ids` (not the
  defaults), passes through `effective_tool_ids` via `_ => true`. The `reflect`
  rubric and the auto-reflection system message (`REFLECT_TOOL_IDS`) gained a
  consolidation item ("has the narrative bloated — raise the durable into
  summary/traits, clear out the raw").
- **Groundwork (not done)**: background auto-consolidation of the self-model on a
  timer (like `notes.auto_consolidate_every`) — consolidation is currently manual/
  via auto-reflection; and **unifying the memory organs** (narrative ≈ a second
  instance of notes) — still a large separate track (see notes-connectivity
  Tier 3).
- **Tests**: entity (`match_insight`/`remove_insights`; insights with `#id` in
  `render_full`); tools (trait revision with `note` → a scar in the narrative;
  removal without `note` → a reminder; `consolidate_narrative` clears duplicates +
  summarizes + reports a miss). **681 tests green** (+4), clippy/fmt clean. Live
  check on Opus 4.8 — a manual step.

### Post-M9: self-model refinements — stage 1 (atomic write, race fix) (done)
- Refinement plan — [docs/refinements.md](docs/history/refinements.md) (6 stages +
  the "narrative as notes" track). Branch `feat/self-model-refinements`.
- **Defect**: every self-model writer (turn tools, auto-reflection, manual `F3`
  edits) did read-modify-write as **three** calls (`self_model_get` → edit →
  `self_model_upsert`). `Db`'s mutex serializes individual calls but not the
  pair: auto-reflection (a background task running concurrently with the user)
  would read the model → the user would save an `F3` edit → reflection would
  write its own version on top, losing the edit.
- **Fix** (`shared/storage/db.rs`): `Db::self_model_update(profile_id, |m| -> bool)`
  — SELECT + `mutate` + upsert under **one** mutex acquisition. The public
  `self_model_get`/`self_model_upsert` delegate to private `*_conn` helpers
  (`std::sync::Mutex` is non-reentrant → the `mutate` closure **cannot** call
  `Db` methods — it can only mutate the `SelfModel` value; documented in a doc
  comment as a deadlock warning). `self_model_upsert` was kept (a symmetric
  primitive + tests, `#[allow(dead_code)]`).
- **Writers** converted: 4 tools (`add_insight`/`update_self_model`/
  `update_user_model`/`consolidate_narrative`; side data — unresolved handles,
  deletion flags, counters — are collected via `&mut` capture in the closure) and
  the F3 handler (`orchestrator/mod.rs`). Readers (`get_self_model`/`reflect`) use
  `self_model_get`.
- **Tests**: `self_model_update_is_atomic_under_concurrency` (2 threads × 50
  writes → exactly 100 insights, version=100 — under non-atomicity some would be
  lost), no-op doesn't write. **685 tests green** (+4), clippy/fmt clean.

### Post-M9: self-model refinements — stage 2 (time, narrative/goal caps) (done)
- Stage 2 of the [refinements.md](docs/history/refinements.md) plan: "me over
  time" gains time, and "silent loss" (narrative FIFO, unbounded growth of
  closed goals) gains visibility and integration.
- **Age labels** (`entities/self_model.rs`): a pure `age_label(at, now)` — day
  granularity (today/yesterday/N days/weeks/months/years). **Day granularity
  chosen deliberately**: the text is stable within a day, so the self-model
  injection into the system prompt doesn't change turn to turn (the local
  model's prefix cache suffers no more than once a day beyond actual edits).
  `render_full(now)` and `render_for_prompt(cap, n, now)` show the age of goals
  (closed ones — from the new `Goal.closed_at` field, `#[serde(default)]` → no
  migration) and observations. The `F3` screen adds a date to goals (local time
  zone, like insights).
- **Eviction made visible**: `add_insight(text, max) -> Vec<NarrativeSegment>`
  returns what was evicted past the cap; the `add_insight` tool reports
  "narrative N/M" and **what left** (a last chance to raise the durable). A new
  `narrative_fill_hint(max)` (≥80% full) makes the static maintenance protocol
  **data-aware** — a "time for consolidate_narrative" note is mixed into the
  system prompt (`inject_self_model`) and into the `reflect` rubric.
- **Closed-goal cap**: `fold_closed_goals(keep, max_narrative)` folds the oldest
  closed goals beyond `keep` into a narrative scar "[goal archive] …" and removes
  them from the structure (the same "integrate, don't lose" philosophy). Called
  in `update_self_model` and the F3 handler. New config
  `SelfModelSettings.max_closed_goals` (default **10**, `#[serde(default)]`) →
  `SelfModelParams` (sanitized to ≥1).
- **Prefix-cache trade-off locked in** (user decision 2026-07-03): the self-model
  injection stays in `system`; losing prefix cache on every update is the
  accepted price for the capability. Moving the block to the end of history is
  **not** being done. See architecture.md §9.
- **Tests**: entity (`age_label` buckets + a future timestamp; `closed_at` gets
  set/cleared; `add_insight` returns what was evicted; `narrative_fill_hint`
  80% threshold; `fold_closed_goals` archives the oldest beyond keep); tools
  (`add_insight` reports eviction; `update_self_model` folds closed goals);
  config (`max_closed_goals` default). **692 tests green** (+7), clippy/fmt
  clean.

### Post-M9: self-model refinements — stage 3 (reflection cadence by watermark) (done)
- Stage 3 of the [refinements.md](docs/history/refinements.md) plan:
  auto-reflection stops re-reading the same early material and no longer loses
  the cycle on a skip.
- **Three bugs**: (1) the reflection digest was built from the **entire** chat
  history → every cycle re-read what had already been reflected on → duplicate
  insights, later cleaned up by consolidation; (2) the cadence counter was reset
  **before** the "already running"/"server not ready" gates — a skipped run lost
  the whole cycle (with `every=10` the next attempt was 10 responses away);
  (3) in-memory counters (`reflect_counts`/`consolidate_counts`) were lost on
  restart, even though the data is per-profile.
- **Watermark in `Chat`** (`entities/chat.rs`): `reflected_upto: Option<usize>`
  (a watershed index — how many leading messages have been covered) +
  `reflected_at` (`#[serde(default, skip_serializing_if=Option::is_none)]` → old
  chat files read without migration, empty ones don't clutter the JSON; lives
  with the chat → survives restarts).
- **Window-based cadence** (`orchestrator/reflection.rs`): a pure
  `reflect_window(messages, reflected_upto) -> (wm, count)` — clamps the
  watermark to the length (resilient to `Ctrl+R`/`Ctrl+E` truncation) + counts
  non-empty assistant responses in the window `messages[wm..]`. `due(count,
  every)` as before. The digest is `build_conversation_digest(&messages[wm..])`
  (the signature already took a slice). The `reflect_counts` field was
  **removed** from the orchestrator (cadence is now computed from data).
- **Watermark shifts only on spawn**: after **all** gates (profile enabled the
  self-model, the window accumulated `every`, the digest is non-empty,
  reflection isn't already running, server is `Ready`) — `reflected_upto =
  messages.len()`, `reflected_at = now`, `mark_dirty` (debounced save).
  `modified_at` is untouched (reflection shouldn't bump the chat up in the
  list). A skip on any gate leaves the watermark alone → the cycle isn't lost,
  self-heals.
- **Notes consolidation** (`consolidation.rs`): still on an in-memory counter
  (its digest is a notes overview, not the conversation), but **the counter
  reset moved to after all gates** — closing the same cycle-loss bug.
- **Tests**: pure (`reflect_window` counts from the watermark + clamp after
  truncation); serde (an old chat JSON without watermark fields → defaults,
  empty ones aren't serialized); integration (`maybe_auto_reflect` advances the
  watermark when the engine is ready and does **not** advance it when the
  server isn't ready). **697 tests green** (+5), clippy/fmt clean.

### Post-M9: self-model refinements — stage 4 (interlocutor model) (done)
- Stage 4 of the [refinements.md](docs/history/refinements.md) plan: wiring
  already-existing interlocutor data into mechanisms that weren't using it.
- **`user_model` → impersonation** (4a): impersonation (`Ctrl+U`) writes a
  reply **on behalf of** the interlocutor, yet `user_model` — literally a model
  of that interlocutor — wasn't seen by `build_impersonation_request`. New
  `UserModel::render_for_impersonation(cap)` is mixed into the impersonation
  system prompt (`build_impersonation_request(..., user_hint)`); gated on the
  same opt-in as passive injection (profile enabled `get_self_model`).
- **Behavioral signals in the reflection digest** (4b): `Ctrl+R`
  (regeneration = "the answer wasn't good enough"), `Ctrl+E` (deleting an
  exchange), and rewrite rounds are already archived into `Chat.deleted` — the
  strongest implicit evidence about the interlocutor, which reflection wasn't
  seeing. New `DeletedCause` (`DeleteExchange`/`Regenerate`/`Rewrite`) + field
  `DeletedExchange.cause` (`#[serde(default, skip_serializing_if)]` → no
  migration; `record_deleted` gained a parameter, 3 call sites updated). A pure
  `behavior_markers(chat, since)` (`since` = the former `reflected_at` from
  stage 3) counts deletions within the window and appends a "Behavioral signals
  about the interlocutor:…" block to the digest (regeneration/deletion —
  about the interlocutor, rewrite — about the agent's own behavior; entries
  without `cause` aren't counted). `REFLECT_SYSTEM_MESSAGE` clarifies that the
  markers are evidence (an observation, not a judgment).
- **Scar on replacing relationship dynamic** (4c): `relationship_dynamic` — the
  most significant field of the interlocutor model — was replaced wholesale
  without a trace (the `note` reminder only fired on `remove_traits`/
  `remove_interests`). Now replacing a **non-empty** dynamic without `note`
  also produces a reminder scar (`replaced_dynamic`); initial population — no
  reminder. The tool description was updated.
- **Tests**: entity (`render_for_impersonation` Some/None + all fields);
  reflection (`behavior_markers` — count by cause, `since` filter, old entries
  without `cause` aren't counted, own behavior gets a separate phrase);
  impersonation (`build_impersonation_request` mixes in `user_hint`); tools
  (replacing a non-empty dynamic without `note` → reminder, initial population
  — none, with `note` → scar in the narrative). **701 tests green** (+4),
  clippy/fmt clean.

### Post-M9: self-model refinements — stage 5 (background-task observability) (done)
- Stage 5 of the [refinements.md](docs/history/refinements.md) plan:
  reflection/consolidation are silent background tasks whose failures are easy
  to miss; an open `F3` after a background edit showed a stale snapshot; a dead
  field in the contract.
- **Task outcome + failure streak** (5.1): the internal done channels for
  reflection/consolidation now carry `Result<(), String>` instead of `()`;
  errors are logged with `warn` (including `profile_id`). The orchestrator
  counts consecutive failures (`reflect_failures`/`consolidate_failures`); at
  the threshold `BACKGROUND_FAILURE_ALERT=3` it emits `AppEvent::Error`
  **once**, then stays quiet until the first success (reset) — observability
  without spam.
- **Status-bar indicator** (5.2): new event `AppEvent::BackgroundTask{kind:
  BackgroundKind, active}` (emitted on spawn/finish). `ChatScreen` holds flags
  (`set_reflecting`/`set_consolidating` — the `BackgroundKind` mapping is done
  by runtime, so `screens` doesn't depend on the `app` contract, FSD);
  `status_bar` draws a quiet muted chip `✻ reflection`/`✻ notes sleep` (a
  1-column-wide glyph — the hotkey grid layout doesn't "shift"). New parameter
  `background: Option<&str>` on `render`/`height`/`lines`.
- **Freshness of an open `F3`** (5.3): new event `AppEvent::SelfModelChanged`
  (no snapshot). Emitted after **successful** reflection (`handle_reflect_done(Ok)`)
  and in `handle_done` if the turn included SelfModel-tool calls (detected via
  a new `self_model::is_self_model_tool` + `ALL_IDS`). `runtime::apply_event`:
  if the `F3` screen is open → sends `AppCommand::RequestSelfModel` (re-fetch a
  fresh snapshot); if closed — ignored (doesn't open the screen, unlike
  `SelfModelView`). Consolidation doesn't send `SelfModelChanged` (it changes
  notes, not the self-model).
- **Contract cleanup** (5.4): removed the dead field `ToolContext.self_model`
  (`#[allow(dead_code)]`; tools read from the DB, reflection was passing
  `None`) — the contract no longer makes a false promise that "a snapshot is
  available". Removed from 8 construction sites (generation, reflection,
  consolidation, testkit + 4 tool test contexts). The snapshot for prompt
  injection now lives as a local variable in `start_generation`, not a context
  field.
- **Tests**: runtime (`SelfModelChanged` with `F3` open sends
  `RequestSelfModel`, with it closed — doesn't); orchestrator (3 consecutive
  failures → one error, success resets + sends `SelfModelChanged`;
  `handle_done` with a self-model call → `SelfModelChanged`); tools
  (`is_self_model_tool` recognizes the group); status_bar tests updated for the
  new parameter. **705 tests green** (+4), clippy/fmt clean.

### Post-M9: self-model refinements — stage 6 (loop dedup + single-source policy) (done)
- Final stage of the [refinements.md](docs/history/refinements.md) plan: a
  mechanical refactor, no behavior change.
- **Shared silent runner** (6.1, `app/orchestrator/tool_loop.rs`): the body of
  the mini agentic loop (stream → call accumulator → execute allowed tools →
  round, tolerant of Thoughts/ThoughtsSignature/Usage) was duplicated
  **verbatim** in `reflection.rs` and `consolidation.rs` (differing only in
  limits and the log label). Now there's one — `spawn_silent_loop(SilentLoop {
  backend, registry, ctx, request, allowed, cancel, max_rounds, timeout, label,
  profile_id, done_tx })` + a private `run_rounds`. Shared spawn tail (timeout
  + `warn` log + sending the outcome to the done channel). The `due` cadence
  predicate also moved here (was in both modules). Both sites got thinner: they
  build a `SilentLoop` and call the runner; their `ReflectSpawn`/
  `ConsolidateSpawn`/`spawn_reflection`/`spawn_consolidation`/`due` were
  removed. **The main generation loop was deliberately not merged in** —
  streaming to the UI, control-flow tools, Anthropic thinking signatures,
  usage, effects; its complexity doesn't pay for a shared sink (noted in the
  module doc).
- **Single-source maintenance policy** (6.2): the rule wording was duplicated in
  `SELF_MODEL_MAINTENANCE_PROTOCOL` (`generation.rs`) and
  `REFLECT_SYSTEM_MESSAGE` (`reflection.rs`) and had already drifted slightly.
  Introduced a canonical constant `self_model::POLICY_CORE` (integrate
  summary; manage goals by #id; merge user_model; transient → add_insight;
  accuracy over agreeableness; consolidate the narrative). Both texts are
  assembled from it: `self_model::maintenance_protocol()` = `POLICY_CORE`
  framed as "you manage this yourself"; `reflection::reflect_system_message()`
  = preamble + `POLICY_CORE` + an explanation of behavioral signals (built at
  runtime — `format!` doesn't work for `const`). The interactive `reflect`
  rubric was **deliberately** left as-is — a different genre (questions, not
  an imperative), covering the same topics.
- **Field-grouping `BackgroundLoop`** (plan 6.1) — **not done**: reflection and
  consolidation differ (consolidation has a `consolidate_counts` counter,
  reflection has the watermark), the payoff is cosmetic, and the risk of
  smearing the invariant across call sites doesn't pay off. The `*_cancel`/
  `*_failures`/`*_done_tx` fields were left on `Orchestrator`.
- **Tests**: `due` — one set (in `tool_loop`); `maintenance_protocol_wraps_policy_core`
  and `reflect_system_message_composes_from_policy_core` (composition from
  `POLICY_CORE` + its own framing). Loop behavior is checked by the prior
  integration tests (`auto_reflect_advances_watermark_on_spawn` and others —
  via the shared runner). **706 tests green**, clippy/fmt clean. **Self-model
  refinements (stages 1–6, refinements.md) — complete.**

### Post-M9: self-model narrative as notes — Tier 1 (done)
- **Unifying the memory organs** per the [docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)
  plan: the SelfModel narrative (`Vec<NarrativeSegment>` in the JSON blob, FIFO
  cap, no semantics/dedup/graph) moves into **regular notes** with the reserved
  tag **`@self`**, getting embeddings, semantic search, duplicate gates,
  scarred replacement, and auto-"sleep" for free. This closes a long-standing
  item of groundwork, "linking the memory organs" (notes-connectivity "out of
  scope", architecture §9.9). A probe, as with SelfModel/notes: a minimal
  implementation for a go/no-go on a live model.
- **Forks confirmed by the user**: the `@self` tag (a leading `@` doesn't occur
  in natural tags; a collision is rare and harmless); self-notes are **hidden**
  from the user-facing `note_recall` (memory about oneself ≠ memory about the
  interlocutor — mixing the output is risky; full mixing with an `[about self]`
  marker is deferred to Tier 2).
- **Unification at the storage level, not at retrieval** (`features/tools/notes.rs`):
  self-notes share tables/embeddings/graph/consolidation with regular ones but
  are excluded from user-facing `note_recall` by a tag filter (`is_self_note`)
  (the substring path `list_user_notes` — reads without a limit, drops self
  notes, then truncates; the semantic `semantic_recall` — filter + extra
  candidate margin; spreading activation also skips self notes), from the
  `note_save` gate, and from the consolidation overview
  (`build_consolidation_overview` + the ≥2-notes gate for auto-"sleep" only
  count user notes). DB methods were untouched.
- **Recording observations → @self notes**: `add_insight` = `create_note(@self)`
  + a **gate** (the core of the hypothesis: `self_note_similar` — semantically
  close observations with a hint to rewrite via `note_revise`/`note_supersede`
  instead of a near-duplicate); the trait-revision scar
  `update_user_model.note` → an @self note; `fold_closed_goals` now **returns**
  scars (`Vec<String>`), and `update_self_model`/the F3 handler write them as
  @self notes.
- **Reading observations → from notes by recency**: `render_for_prompt`/
  `render_full` gained a `recent: &[NarrativeSegment]` parameter (prepared by
  the caller — `self_notes_recent`; the main ripple — `render_*` stopped being
  pure with respect to the narrative — a deliberate cost). Observations in
  `render_full` — with the **full** id (they're rewritten by
  note_revise/supersede), goals — the old `#id`. `inject_self_model` (the
  orchestrator) and `get_self_model`/`reflect` (tools) read fresh self-notes;
  `is_empty` no longer counts the narrative.
- **Consolidating observations**: `consolidate_narrative` was **removed**
  (superseded by the stronger note tools: scarred replacement); in
  `REFLECT_TOOL_IDS` it's replaced by `note_revise`/`note_supersede`/
  `note_merge` (`note_recall` isn't given — it hides self-notes; the model
  takes full ids from `get_self_model`). `note_supersede`/`note_merge` now
  **inherit tags** from the source(s) (including `@self` — a self-note doesn't
  "fall out" into user-facing output on replacement/merge; `Db::note_get`).
  `POLICY_CORE` and the `reflect` rubric were rewritten around the notes idiom;
  the entity methods `add_insight`/`remove_insights`/`narrative_fill_hint`/
  `match_insight` were removed (the `narrative` field is kept for backfill and
  for reconstructing the `F3` snapshot).
- **Backfill** (`migrate_self_narrative`): a one-time idempotent migration of
  the blob narrative → @self notes (an **atomic drain** of the narrative under
  the mutex → no duplicates even on repeat; `created_at` is preserved; the
  vector is embedded lazily). Called best-effort in `start_generation` before
  reading observations (under the same opt-in gate, `get_self_model`).
- **F3**: the `SelfModelView` snapshot **reconstructs** `narrative` from
  self-notes (for display only — the snapshot isn't persisted, writing goes
  against the real model, which is empty on `narrative`); the screen itself
  didn't change. `SelfModelEdit::DeleteInsight` → deletes a self-note
  (`Db::note_delete` became profile-scoped + removes its vector); `Clear` →
  wipes @self notes and the blob.
- **Invariants intact**: notes are DB-only (no `ChatEffect`), isolated by
  `profile_id`, no schema migrations (`@self` is a plain tag; the `narrative`
  field is `#[serde(default)]`).
- **Tests**: notes (recall/gate/overview hide self notes; supersede/merge
  inherit tags; backfill migrates and is idempotent; note_delete is isolated +
  removes the vector); self_model (add_insight writes @self + the gate shows a
  similar one; get_self_model assembles from notes; trait scar → self-note;
  goal folding → self-notes); entity (`render_*` over `recent`; `is_empty`
  without the narrative; `fold_closed_goals` returns scars); orchestrator
  (injection reads observations; F3 DeleteInsight/Clear act on notes).
  **711 tests green**, clippy/fmt clean. **Probe assessment: GO** — a run of
  `self_model_gate_e2e_live` on Gemma 4 31B + bge-m3: the `add_insight` gate
  showed a similar observation, the model integrated it (`note_merge`/
  `note_revise`), 3/3 runs a near-duplicate was resolved. Graceful degradation
  verified (embed without `--embeddings` → the gate goes empty, nothing panics).
  → Tier 2 (below).

### Post-M9: narrative as notes — Tier 2 (structure: relevance + graph) (done)
- Continuation of Tier 1 ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)):
  observation-notes gain **structure**. Forks confirmed by the user: scope
  **A+B** (C — semantic gates for traits — deferred); relevance-based injection
  — in the **system prompt** (prefix cache is invalidated every turn, an
  accepted cost, §9.3).
- **A. Relevance-based injection** (`orchestrator/generation.rs`): observations
  in the system prompt are mixed in not only by recency but by **relevance to
  the latest message** — an old but topically relevant observation surfaces
  when the topic returns. `notes::self_notes_relevant` (backfills vectors →
  embeds the query → searches among @self by cosine similarity, top-K);
  `blend_self_notes` (K relevant + a guaranteed freshest one for continuity,
  dedup, cap); assembled into `injection_recent`. **The injection moved from
  the sync `start_generation` into the async task `spawn_generation`** —
  relevance requires an async embedding of the latest message, and the
  command handler is synchronous; `GenSpawn` carries `self_model`/params/flags/
  `last_user`, the token estimate is emitted after injection.
  `ensure_note_vectors` was generalized to `(storage, embedder, profile)`.
  Graceful degradation to recency (no embedder/reply).
- **B. Graph over observations**: the mechanics (`note_link`/`note_neighbors`)
  already worked on self-notes (they are notes, the tag isn't filtered) —
  Tier 2 **uses and surfaces** it. (B1) `REFLECT_TOOL_IDS` += `note_link`/
  `note_neighbors`, the `reflect` message/rubric nudge linking related
  observations (`contradicts`/`refines`/`relates`) by full id from
  `get_self_model`. (B2) `notes::self_related_block` — a "Related observations"
  block: graph edges touching the shown observations (structure — "what
  relates to what" that a flat list can't give), **self↔self only**; a
  neighbor outside the shown set is brought in with its text (spreading
  activation); edge dedup. `self_model::render_self_read` (= `render_full` +
  the block) is used in `get_self_model`/`reflect`. The passive injection does
  **not** show the graph (prompt compactness). The self-consolidation overview
  is deferred.
- **Not included**: C (semantic gates for `user_model` traits —
  near-duplicate traits), the self-consolidation overview, cross-organ links
  (self↔user↔RAG, Tier 3), full output mixing (`[about self]` in general
  recall, Tier 3) — per the scope decision.
- **Tests**: `self_notes_relevant` (ranking + @self filter + empty query);
  `blend_self_notes` (relevant first, freshest guaranteed, dedup, cap);
  `injection_recent_surfaces_relevant_over_fresh` (an old relevant observation
  surfaces above a fresh one — deterministic on `MockEmbedder` + a temp
  storage); `get_self_model` shows "Related observations" + the link type;
  `REFLECT_TOOL_IDS` contains the graph tools; `reflect_system_message` nudges
  `note_link`. Live smoke `self_model_graph_e2e_live` (`#[ignore]`,
  `spawn_orch_live`): the model links contradicting observations. **716 tests
  green**, 20 `#[ignore]`, clippy/fmt clean.
- **Graph smoke — GO** (`self_model_graph_e2e_live` on Gemma 4 31B + bge-m3):
  the model on its own recorded two contradicting observations
  (`add_insight`), saw them with full ids via `get_self_model`, and linked them
  with a `contradicts` edge (`note_link`) — the edge appeared in the
  observation graph. Relevance/graph are put to use on a live model.

### Post-M9: narrative as notes — Tier 2, step C (related-traits gate) (done)
- **Semantic gate for related `user_model` traits** — the deferred step C of
  Tier 2 ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)), a
  **mirror of the `add_insight` gate**, but over the flat interlocutor-trait
  list: on `add_traits`, for each actually-added trait the closest one among
  the **prior** traits (existing before this edit) is searched, above the
  cosine-similarity threshold `TRAIT_SIMILARITY=0.72`; if a close one is found,
  the tool **shows it** and asks the model to decide: a **duplicate** (merge
  via `remove_traits`) or a **contradiction** (record as an `add_insight`
  observation). **A soft gate** (not a block — the decision is left to the
  model, both traits are kept).
- **The 0.72 threshold was calibrated from a live test, not guessed.** The
  initial 0.85 (by analogy with the notes consolidation overview) on a live
  run **missed genuine paraphrases**: bge-m3 compresses short traits into a
  narrow band, "values brevity" ↔ "appreciates concise answers" = **0.77**
  (< 0.85 → the gate stayed silent, even though it's a duplicate). Calibration
  via our own client: paraphrases 0.73–0.83, unrelated 0.51–0.69 → threshold
  0.72. **Key observation:** bge-m3 groups traits by **dimension/topic**, not
  by direction of meaning, so **antonyms** also fall in the band ("values
  brevity" ↔ "values long explanations" = 0.71). This isn't a bug but a
  **reframing of intent**: the gate's wording was changed from "near-duplicate"
  to "related trait — duplicate or contradiction?" (consistent with the
  "integration over accumulation" philosophy + contradictions living in the
  narrative). A curl+awk measurement gave corrupted values (0.97 on
  everything, 1.000 on antonyms) — the correct values come from our own
  `OpenAiClient` (dim=1024, real bge-m3).
- **On-the-fly trait embedding** (`self_model::near_duplicate_traits`): traits
  have no stored vectors (`Vec<String>`, unlike notes with `note_vectors`), so
  the added traits + the prior ones are embedded in **one request** and
  compared via `cosine` (made `pub(crate)` in `notes.rs` for reuse). **Graceful
  degradation**: embedder unavailable / mismatched vector count → empty (like
  `add_insight`/`note_save`). The gate only fires when traits were actually
  added (an empty set skips the embedding call).
- **Snapshot of prior traits — inside the atomic edit** (the
  `self_model_update` closure, captured via `&mut existing_before_traits`);
  the embedding itself happens **afterward** (async/storage outside the
  closure, like the revision scars). The actually-added ones are computed
  outside the closure (requested ∖ prior, case-insensitive, deduplicated
  within the batch).
- **Traits stay `Vec<String>`** (no schema migration); the `update_user_model`
  description now mentions the gate. **Invariants intact**: DB-only, isolated
  by `profile_id`, no `ChatEffect`.
- **Tests**: `add_trait_gate_surfaces_near_duplicate` (a related trait raises
  the gate + a `remove_traits` hint), `add_trait_gate_silent_for_dissimilar`
  (unrelated stays silent, both traits are kept). On `MockEmbedder` ("aaaa
  bbbb" ↔ "aaab" cosine ≈ 0.89 > the threshold). Live `#[ignore]` smoke
  `trait_gate_e2e_live` (`spawn_orch_live`, a real embedder). **718 tests
  green**, 21 `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (Gemma 4 31B + bge-m3): the model added a similar trait → the
  gate showed a related one ("values brevity" ≈ "appreciates concise
  answers"), the model replied "this is a duplicate" and merged them into one
  trait via `remove_traits`. The gate fires, duplicate recognition/integration
  work.

### Post-M9: linking the memory organs — Tier 3, Path 1 (cross-organ links) (done)
- **The original long-range connectivity goal**
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)): link the
  memory organs (self-notes ↔ user notes ↔ RAG). The fork (confirmed by the
  user) — **Path 1: cross-organ edges** (Paths 2, "output mixing via an
  `[about self]` marker", and 3, "RAG ↔ notes", deferred).
- **Key point: the graph mechanics were already cross-organ** — `note_link`/
  `note_neighbors` take any id **without a tag filter**, so linking a self-note
  to a user note was already possible; it just never surfaced anywhere.
  Tier 3 (1) **surfaces** such edges in the output and (2) gives the model
  **addressability** of user notes. The organs remain SEPARATE at
  storage/retrieval level — only an edge **deliberately created** by the model
  surfaces (not "contamination" of the output, unlike Path 2).
- **Surfacing cross-edges** (`features/tools/notes.rs`): `related_block` (used
  by user-facing `note_recall`) no longer skips neighbor "about self"
  observations — it shows them marked **`[about self]`**;
  `self_related_block` (reading the self-model) shows neighbor user notes
  marked **`[note]`**. Regular search/spreading still doesn't pull in
  self-notes (Tier 1's hiding is intact) — the filter was lifted **only** for
  neighbors reached via an explicit edge.
- **Addressability** (`format_notes`): `note_recall` now prints note **ids** —
  otherwise the model couldn't reference a user note in `note_link`. This also
  closes a long-standing gap: `note_link`'s description promised "id from
  note_recall", but the id wasn't printed (notes from recall weren't
  addressable even for a regular graph). New constant `NOTE_RECALL_ID`.
- **Nudge** (`orchestrator/reflection.rs`, `self_model::Reflect`):
  `note_recall` was added to `REFLECT_TOOL_IDS` (gives reflection the ids of
  user notes; it still hides self-notes); the auto-reflection system message
  and the interactive `reflect` rubric suggest linking an "about self"
  observation with an "about the interlocutor" fact (the observation's id from
  `get_self_model`, the note's id from `note_recall`).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  schema migrations (the `note_links` graph already existed). **Tests**:
  `recall_shows_note_ids`; `recall_surfaces_cross_organ_self_neighbor_marked`
  (`[about self]` + the primary output without self); `self_related_block_surfaces_cross_organ_user_note_marked`
  (`[note]`); `reflect_tools_include_graph` (+`note_recall`);
  `reflect_message_nudges_cross_organ_linking`. **722 tests green**, 22
  `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (`cross_organ_link_e2e_live`, Gemma 4 31B + bge-m3): the model
  recorded a fact "about the interlocutor" (`note_save`) and an observation
  "about itself" (`add_insight`), then via `get_self_model` + `note_recall`
  (took the note's id) called `note_link` → creating a **cross-organ edge**
  `contradicts` ("the observation about verbosity ↔ the note about preferring
  brevity"). The model uses cross-links meaningfully; the go criterion — **go**.
- **Deferred**: Path 2 (`[about self]` in general recall, behind a toggle),
  Path 3 (RAG ↔ notes), the self-consolidation overview, vec0 as note counts
  grow.

### Post-M9: linking the memory organs — Tier 3, Path 2 (output mixing) (done)
- **Full output mixing behind a toggle**
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md), Path 2,
  confirmed by the user): `config.notes.recall_includes_self`
  (`#[serde(default)]`, **off** by default) enables showing self-notes
  (`@self`) in the general `note_recall` marked **`[about self]`**. Off =
  Tier 1 behavior (self hidden: memory about oneself ≠ memory about the
  interlocutor) — the reversal of hiding is deliberately kept **behind a
  toggle** so it can be validated safely.
- **Wiring**: `NotesSettings.recall_includes_self` →
  `ToolContext.recall_includes_self` (threaded through all construction sites
  — generation/reflection/consolidation/testkit + 4 tool test contexts). The
  recall paths (`list_user_notes` substring + `semantic_recall`), when the
  toggle is on, no longer drop self-notes; `format_notes` marks them
  `[about self]` and **hides the internal `@self` tag** from the tag display
  (the marker replaces it). Toggle in the settings screen's "Tools" section
  (`FieldId::NotesRecallIncludesSelf`, with a hint).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  migrations; behavior unchanged by default. **Tests**:
  `recall_includes_self_notes_when_enabled` (self shows up in both recall
  branches with the marker, without `@self`); default off
  (`partial_json_fills_defaults`). **723 tests green**, 23 `#[ignore]`,
  clippy/fmt clean.
- **Smoke — GO** (`recall_includes_self_e2e_live`, Gemma 4 31B + bge-m3): with
  the toggle on, `note_recall` returned both the user note and the "about
  self" observation marked `[about self]`, and the model's reply **cleanly
  separated the organs** ("— About you: values brevity; — About myself: tends
  toward verbosity") — **no contamination**, the marker works as intended (go
  on mixing safety).
- **Deferred**: Path 3 (RAG ↔ notes), the self-consolidation overview, vec0 as
  notes grow.

### Post-M9: linking the memory organs — Tier 3, Path 3 (RAG ↔ notes) (done)
- **The third memory organ (the RAG knowledge base) is linked to notes/observations**
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md), Path 3): a
  note can **cite a RAG source**, and search works **across both organs**.
  Completes the "narrative as notes" track (Tiers 1–3).
- **Key decision: cite the source's NAME, not the chunk id.** Chunk ids are
  **unstable** — `/rag rebuild` drops and reindexes documents with new uuids,
  and a link to a chunk-uuid would break; the source's name (path/label) is
  stable (stored in `rag_sources`). So the link targets `source`.
- **Schema**: table `note_rag_links(profile_id, note_id, source, created_at,
  PK(profile_id, note_id, source))` (`CREATE TABLE IF NOT EXISTS` → no
  migration, + indexes on note_id/source). DB methods: `rag_source_exists`
  (validation — you can only cite an existing source, in `rag_documents` OR
  `rag_sources`), `note_cite_source_insert` (idempotent, `INSERT OR IGNORE`),
  `note_cited_sources` (forward: a note's sources), `notes_citing_source`
  (reverse: a source's active notes, hiding superseded ones via an anti-join
  on `note_superseded`). `note_delete` cleans up `note_rag_links`. All isolated
  by `profile_id`.
- **Tool** `note_cite_source(note_id, source)` (`features/tools/notes.rs`):
  validates the note is active (`note_is_active`) + the source exists;
  understandable text refusals (not a panic), a "created / already existed"
  message. Added to `default_tool_ids` (like `note_link` — `reconcile_tools`
  enables it for existing profiles), registered. DB-only → passes through
  `effective_tool_ids` via `_ => true`.
- **Bidirectional output** ("search across both organs"): `note_recall` and
  `get_self_model` (`render_self_read`) show a "Source citations" block
  (note→source, `notes::cited_sources_block`); `rag_search` shows a "Notes
  citing these sources" block (source→notes, `notes_citing_source` over the
  sources of the matched passages, deduped by id, self-observations marked
  `[about self]`).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  migrations. **Tests**: db (`note_rag_links_bidirectional_and_isolated` —
  forward/reverse + isolation + cleanup on delete; `notes_citing_source_hides_superseded`);
  tool (`note_cite_source_links_and_recall_shows_it` — source/note validation,
  idempotency, showing up in recall); rag_search
  (`search_surfaces_notes_citing_matched_source`). **727 tests green**, 24
  `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (`note_cite_source_e2e_live`, Gemma 4 31B + bge-m3): the model
  added a document (`rag_add`), then within one turn `rag_search` →
  `note_save` → `note_cite_source`, linking the conclusion "The capital of
  France is Paris" to the "facts" source. In the DB: 1 note cites "facts". The
  full chain (search → note → citation) worked.
- **Deferred**: vec0 as note counts grow; a nudge for `note_cite_source` in
  reflection (currently — in regular turns, where `rag_search` is available).

### Post-M9: self-consolidation overview in reflection (Tier 3, narrative as notes) (done)
- **The "self-consolidation overview" deferred in Tier 2 is now enabled** after
  confirming the value of linking observations (Tier 3, GO): auto-reflection
  and the `reflect` tool now get **concrete data** for the self-memory "sleep",
  not just a rubric. A mirror of `build_consolidation_overview` (an overview of
  user notes for `consolidate_notes`), but over `@self` observations.
- **`build_self_consolidation_overview(storage, profile_id) -> Option<String>`**
  (`features/tools/notes.rs`): a pure DB read (vectors already in the DB) over
  **only** `@self` observations (mirroring the exclusion of self from the
  user-notes overview — the observation "sleep" doesn't touch memory about the
  interlocutor). Three sections: **similar pairs** (possible observation
  duplicates, pairwise cosine ≥ `CONSOLIDATE_SIMILARITY` 0.85, descending by
  similarity), **`contradicts` links** among observations (both ends `@self`),
  **observations without links** (candidates to link). `None` if there are
  fewer than 2 observations (nothing to consolidate). Isolated by
  `profile_id`.
- **Wiring**: `Reflect::invoke` (`features/tools/self_model.rs`) mixes the
  overview in between "Current self-model" and the rubric (empty when < 2
  observations); auto-reflection (`app/orchestrator/reflection.rs`) appends
  the overview to the digest after the borrowed block (`let mut digest`);
  `reflect_system_message` nudges using the "Observations overview for
  consolidation" block (merge similar pairs via `note_merge`/`note_supersede`,
  check `contradicts`, link unlinked ones via `note_link`).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  schema migrations (`@self` is a plain tag; vectors are already in
  `note_vectors`; the graph is in `note_links`).
  **Tests**: notes (`self_consolidation_overview_covers_self_only` — covers
  only observations, excludes user notes, `None` when < 2, shows a similar
  pair and `contradicts`); self_model
  (`reflect_includes_self_consolidation_overview` — with ≥2 observations
  reflect mixes in the overview; the `reflect_returns_current_and_rubric` test
  updated — with < 2 there's no overview); reflection (an assert in
  `reflect_message_nudges_cross_organ_linking`). **729 tests green**, 25
  `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (`self_consolidation_overview_e2e_live`, Gemma 4 + bge-m3):
  the model recorded two similar observations, called `reflect` — its result
  carried the "Observations overview for consolidation" block with the
  similar pair (a real embedder, similarity **0.89 ≥ 0.85**), after which the
  model merged the duplicates via `note_merge` into one observation. The full
  chain (observations → reflect with the overview → merge) worked.
- **Deferred** (groundwork): vec0 as the number of observations/notes grows;
  a background auto-consolidation of the self-model on a timer (currently
  the overview flows through auto-reflection/the interactive `reflect`).

### Post-M9: remembering the last-open chat (done)
- **The app now restores the last-open chat on the next launch.** Previously
  `bootstrap` always activated the most recently modified chat (`chats.first()`
  after sorting by `modified_at`) — switching to an old, unedited chat was
  "forgotten" on restart. Now the active chat is remembered in settings.
- **New field `AppConfig.last_active_chat: Option<Uuid>`**
  (`shared/config.rs`, `#[serde(default)]` via the container's
  `#[serde(default)]` → old `settings.json` files without migration; **not
  editable on the settings screen** — an orchestrator property). Written by
  `Orchestrator::remember_active_chat` (called from `activate`) — **only on an
  actual switch** of the active chat (`activate` is also called to rebuild the
  feed of the same chat during regeneration/exchange deletion — no write
  there); an atomic write of `settings.json`, an error is not escalated
  (memory is a convenience). `bootstrap` activates `last_active_chat` if it's
  still visible, otherwise — the previous fallback (most recent).
- **Settings round-trip protection**: `handle_update_config` replaces the
  entire `self.config` with the UI snapshot, which may carry a stale
  `last_active_chat` (e.g. `None` from startup) — after the replacement the
  actual value is restored (`old.last_active_chat`), so editing settings
  doesn't erase the memory of the chat.
- **Tests**: config (default `None`); orchestrator
  (`remembers_and_restores_last_opened_chat` — a two-phase test: switching to
  the first chat → `settings.json` has `last_active_chat` = the first one; a
  second launch on the same data restores exactly that one, even though the
  second chat was modified later). **734 tests green**, 25 `#[ignore]`,
  clippy/fmt clean.

### Post-M9: scrollbars (feed, input, settings, help, chat list) (done)
- **Shared helper `shared/ui.rs::render_scrollbar`**: a vertical scrollbar
  (ratatui `Scrollbar`) in the right-hand column of the given area; **a no-op
  when the content fits** (`total ≤ viewport`) — it stays quiet on short
  content. Drawn **over the panel's right border line**
  (`area.inner(Margin::new(0, 1))` — corners intact), doesn't take width away
  from the content → line wrap/scroll math is unchanged. The track uses the
  border color (the `focused` parameter says which color the border under the
  bar was drawn in: normal/focused), the thumb `█` uses the `text` color
  (readable on both). **A ratatui subtlety**: `ScrollbarState::content_length`
  is the number of **scroll positions** (`total − viewport + 1`), not rows;
  with `content_length = total` the thumb wouldn't reach the bottom at full
  scroll (see the comment in the helper).
- **Chat feed** (`widgets/message_feed.rs`): the bar sits on the right border
  when content overflows, position — `self.scroll` (clamping unchanged).
- **Input box** (`widgets/input_box.rs`, multiline mode): the bar appears when
  `vrows > visible_rows` (the field has hit its height cap and is scrolling);
  single-line mode is unaffected (it scrolls horizontally there).
- **Settings screen** (`screens/settings.rs::render_fields`): the bar sits over
  the screen's right border (column `list_area.right()` — the border line:
  `fields_area` reaches right up to the inner panel), below the section title
  line; position — the actual `ListState::offset()` after the list is
  rendered.
- **Help popup** (`F1`/`?`, `screens/chat.rs`): the key list now **scrolls** on
  a short terminal — `↑↓`/`PgUp`/`PgDn` scroll it (`ChatScreen.help_scroll`,
  clamped in `render_help` — the popup's height is only known there); any
  other key closes it (as before). `List` was replaced with a `Paragraph`
  using `scroll`; the bar sits on the popup's right border; a caption at the
  bottom when content overflows — "↑↓ scroll · Esc — close".
- **Chat list** (`widgets/chat_list.rs`): the bar sits over the right border of
  the "▤ Chats" panel on the list rows (the search line and hotkey grid are
  unaffected); position — the list's offset after rendering.
- **Tests**: the helper (the bar only appears on overflow; the thumb sits at
  the top initially and at the bottom on a full scroll; a zero-size area is a
  no-op); one test per feed/input/settings/chat list (the "█" thumb in the
  border column appears only on overflow); help (arrows scroll and don't
  close, reopening resets the scroll, an "overscroll" is clamped on render,
  the thumb appears on a short terminal). **744 tests green**, clippy/fmt
  clean.

### Post-M9: compatibility mode for old terminals (done)
- **Toggle `interface.terminal_compat`** (settings "Interface" section,
  `FieldId::ICompat`, **off** by default; `#[serde(default)]` → old
  `settings.json` files without migration): older emulators (Windows 10
  conhost, etc.) render emoji and rare Unicode characters as "tofu" boxes and
  ignore `DIM` — the mode switches the UI over to a safe glyph set. See
  spec §11.6.
- **`GlyphSet`** (`shared/theme.rs`): all decorative UI glyphs are gathered
  into one struct with two statics — `UNICODE_GLYPHS` (the previous redesign
  look) and `COMPAT_GLYPHS`. **The compat set targets WGL4** (Windows fonts'
  base repertoire: Consolas/Lucida Console) plus ASCII: `✦→*`, `❯→>` (role
  headers, the input prompt), `⚒→#` (the tool card; the prefix/continuation
  count-width matches in both sets), `▸/▾→►/▼` (the "thoughts" pill,
  "Sections"), `◆→♦` (titles), `▤→≡` (chat list), `⚙/⌨→#` (settings/help
  panels), `✓/✗/⚠→√/×/!` (operation statuses, `F3` goals), `◐/✕→○/×` (server
  chips; **the ready glyph `●` is not replaced** — it's already in WGL4),
  `⟳→»` (generation), `✻→*` (background task), `⌕/▏→?/│` (the search bar),
  `➕→+` (the spellcheck popup), the Braille spinner → ASCII `|/-\` (the RAG
  banner, impersonation), rounded borders (`BorderType::Rounded`, arc segments
  `╭╮╰╯`) → straight ones. WGL4-safe glyphs (`▌` rails, `│`/`└` gutters,
  markdown table box-drawing, the `█` scrollbar, `…`, arrows, `‹›`, `·`, `☺`)
  are deliberately left alone. Emoji in message **content** (and the `Ctrl+B`
  popup) aren't replaced — that's data, not chrome.
- **Wired through the palette** (minimal ripple): `Palette` gained a
  `compat: bool` field (+ builder `with_compat`, method `glyphs() ->
  &'static GlyphSet`) — the palette is already threaded through every render
  function, like the `dark` flag. `panel()` picks the border type from the
  set. Widgets/screens read glyphs from `palette.glyphs()`; spinner
  duplication (consts in `chat.rs`/`impersonation_preview.rs`) is gone —
  frames now live in `GlyphSet.spinner`. Rebuilding the palette with the flag
  — `ChatScreen::set_settings`, `SettingsScreen::palette()` (a helper),
  `runtime::apply_event` (for an open chat list/`F3`) — applied on the fly
  from the `Settings` event.
- **`dim_background(frame, palette)`** (`shared/ui.rs`): conhost doesn't
  support SGR `DIM`, so in compat mode the background under popups is dimmed
  **by color** — the fg of every cell → `palette.muted` + `BOLD` removed (in
  a 16-color mapping it would give a "bright" variant and undo the dimming);
  the normal mode keeps the previous `DIM`.
- **Tests**: theme (sets switch via the flag; the compat set has no glyphs
  needing replacement, the spinner is ASCII; tool-prefix count-widths match,
  the input prompt is 2 columns); ui (compat dimming colors the fg instead of
  DIM); message_feed (compat feed without emoji: `* ASSISTANT`/`> YOU`/
  `# note_save`/`► thoughts`); status_bar (compat chips `○/×`, `» generating`,
  `* reflection`, `●` stays); settings (the toggle in the "Interface" section,
  saving + the working-copy palette, the field description); config (default
  off, roundtrip). **751 tests green** (+7), clippy/fmt clean.
- **Groundwork**: auto-detecting an old terminal on first launch (a heuristic
  over `WT_SESSION`/`TERM_PROGRAM` on Windows) — currently manual toggling
  only.

### Post-M9: message metadata — mode/model + sampling limited to available fields (done)
- **The `Message.metadata` snapshot now carries the engine mode and model
  name, and sampling is pared down to the fields available in that mode.**
  Previously `finalize_message` (`app/orchestrator/generation.rs`) wrote
  `MessageMetadata { sampling: the full effective_sampling, model: None }` —
  the mode wasn't recorded, the model was always `None`, and the "what was
  applied" snapshot included llama.cpp extensions that a strict cloud dialect
  (OpenAI/Gemini/Claude) wouldn't even have accepted.
- **`MessageMetadata`** (`entities/message.rs`) gained a `mode: ServerMode`
  field (`#[serde(default)]` → old messages are read as `Managed`; `model:
  Option<String>` already existed). `sampling` is now filtered.
- **`SamplingConfig::retain_supported(provider)`** (`entities/sampling.rs`): a
  copy of the config with fields cleared to `None` when they're unavailable
  in the provider's mode — a **mirror of the wire dialect**, via the existing
  `supported_sampling_fields` (the same source of truth used by the settings
  UI and `get/set_sampling`; a serialization round-trip that retains keys,
  like `filter_to_supported` in introspection). Local mode (`None`) keeps the
  full configurable set; cloud gets strict subsets.
- **`EngineSettings::active_model_name()`** (`shared/config.rs`): the active
  model's name by mode (managed — the GGUF's base name without the path/
  `.gguf`; external/cloud — `model_name`). `screens/chat.rs::model_meta` was
  refactored onto it (removing duplicate model-name resolution — the feed
  caption and the metadata snapshot now source the name from one place).
- **Wiring**: `GenSpawn` gained `engine_mode`/`model_name` (a snapshot from
  `config.engine` at the start of the turn), `finalize_message` accepts them
  and builds the metadata with `retain_supported(mode.cloud_provider())`.
- **Tests**: entity (`retain_supported` drops `top_k`/`thinking` for OpenAI,
  keeps them for local; Claude keeps `thinking`, drops `temperature`); config
  (`active_model_name` per mode, an empty name = None); orchestrator
  (`assistant_metadata_records_mode_model_and_filtered_sampling` — cloud mode
  → metadata carries `mode=openai`, `model`, and `top_k` in sampling is
  cleared). **755 tests green** (+3), clippy/fmt clean.

### Post-M9: self-model summary as a snapshot, not a chronicle (anti-bloat) (done)
- **Problem** (user observation + a live profile): the self-model's `summary`
  was ballooning into a dense essay (~4.5–5k chars) with duplicates. Cause:
  every organ **except `summary`** has anti-bloat mechanics: observations have
  duplicate gates/replacement/graph, goals have a closed cap, traits have a
  0.72 semantic gate and merge, but `summary` had no target, no gate, no
  consolidation, no size feedback, and `POLICY_CORE` routed event-like
  conclusions there (a "durable/transient" axis instead of "state/event").
  Plan — four stage PRs, design doc
  [docs/history/summary-as-snapshot.md](docs/history/summary-as-snapshot.md).
- **Stage 1 — a genre boundary (text only, 0 logic):** `POLICY_CORE` (the
  single source for the maintenance protocol and auto-reflection) was
  rewritten around the axis "state → `summary`, event/conclusion →
  `add_insight` **even if durable**" (an observation isn't lost: it surfaces
  by relevance, gets linked, gets consolidated); `summary` becomes a compact
  snapshot of "who you are, what you value, how you work", and editing it
  should "integrate **and SHORTEN**" + a step "read it in full via
  `get_self_model` before editing (it's truncated in the prompt)". The
  `update_self_model`/`add_insight` descriptions and the `reflect` rubric were
  brought to the same boundary.
- **Stage 2 — a soft gate on `summary` size** (a gate, not a cap — data isn't
  truncated): new config `self_model.summary_target_chars` (default 1000,
  `#[serde(default)]`, sanitized to a floor of 200 in `SelfModelParams`); a
  pure `SelfModel::summary_fill_hint(target)` (`None` within target, otherwise
  text with the current size — an analogue of the former
  `narrative_fill_hint`); **three display points** — reading
  (`render_self_read` → seen by `get_self_model`/`reflect`/auto-reflection), a
  data-aware note after `maintenance_protocol()` in `inject_self_model`, and a
  "Description: N chars (target ≤ M)" line in `update_self_model`'s echo when
  the summary is edited; a "Self-model: description target (chars)" field in
  the settings "Tools" section (`FieldId::SmSummaryTarget`).
- **Stage 3 — per-section injection budgets** (only the rendering changes,
  the data doesn't): `render_for_prompt` truncates the "About you" section
  to **half** the limit (`max_chars/2`), guaranteeing the rest of the budget
  for goals/interlocutor/observations — a bloated summary no longer crowds
  them out of the prompt (before, a single final truncation was eating
  everything past the description); `truncate_chars_word` — truncation at a
  word boundary (falling back to the last space, one long word →
  char-by-char); a final truncation of the whole block is a safety net
  (char-exact); **`render_full` (full read) is NOT truncated** — a lesson
  from before.
- **Stage 4 — a leaner edit echo:** `update_self_model`/`update_user_model`
  now return **deltas** instead of the full `render_full` (full reads remain
  the job of `get_self_model`) — saves tokens and removes the "anchoring" on
  the essay genre. `update_self_model`: a size line + added goals with `#id`
  + goals closed as done/no-longer-relevant by `#id` + the count folded into
  observations (deltas are collected in the atomic edit via `&mut` capture;
  along the way the `changed` tracking was corrected — an empty `add_goal` no
  longer flags a change). `update_user_model`: compact final lists for the
  interlocutor + a scar confirmation (the `note` text); the trait gate and the
  `note` reminder are unchanged. Public `entities::self_model::short_id(&Uuid)`
  — a `#id` handle for the echo.
- **Invariants intact**: SelfModel mutations are DB-only (no `ChatEffect`),
  isolated by `profile_id`, no migrations (`#[serde(default)]`); injection
  stays in `system` (the prefix-cache trade-off accepted 2026-07-03).
- **Tests**: entity (`summary_fill_hint`, sanitization, the per-section budget
  doesn't crowd out other sections, word-boundary truncation, delta ids);
  tools (echo deltas, the reading hint, the size line, the POLICY_CORE/rubric
  genre markers); orchestrator (the data-aware note in `inject_self_model`);
  config (default). **770 unit tests green** (+11), clippy/fmt clean. **Live
  run — GO** (Gemma 4 31B + bge-m3): `summary_gate_e2e_live` — the model saw
  "grown to: 1600 ≤ 1000", shrank `summary` 1600 → 57 chars; no regression
  found — the `trait_gate`/`auto_reflect`/`self_model_e2e`/`self_model_gate`
  smokes are green (the new echo, trait gate, `note` scar, reflection,
  `add_insight` gate all work).
- **Groundwork**: timed auto-consolidation of the self-model (stage 2's gate
  will give it the "summary is bloated" signal); semantic comparison of
  summary paragraphs with @self observations in the self-consolidation
  overview; aging of `current_interests` — see the design doc's "Out of
  scope".

### Post-M9: settings-screen redesign — stage 1 (IA + field groups) (done)
- **The settings screen's information architecture fell behind the growth in
  field count** (~120): flat sections with no groups, "Tools" turned into a
  dumping ground (gates + the embedding server + RAG chunking + 6 self-model
  fields + notes), "Inference" was a single field, and labels used
  prefix-pseudo-groups ("Embeddings:", "RAG:", "Self-model:"). Stage 1 of the
  agreed plan: `screens/settings.rs` only (the `SettingsIntent` contract and
  `AppConfig` untouched, no migrations). Branch `feat/settings-redesign`.
- **Section regrouping**: `Inference` (1 field) was removed —
  `max_tool_rounds` moved into `Tools` (an "Agentic loop" group with the
  subagent limits); a new **`Memory`** section was added — RAG chunking (a
  "Knowledge base" group), notes (auto-consolidation, @self recall), and the
  self-model (6 fields) moved there from "Tools". Result: 6 sections — Model
  · Sampling · Tools · Memory · Profiles · Interface. The embedding **server**
  stayed in "Tools" (an "Embeddings (server)" group) — moving it to a "Model"
  subsection was deferred to stage 2 (needs a tab strip with a 3rd
  subsection).
- **Group headers** — a new `FieldRow.group: &'static str` field (the
  `grouped("Group", vec![...])` helper stamps a batch). Headers **are not
  part of `fields()`** (field-to-field navigation, no "dead" steps on a
  header) — they're injected at a group boundary **at render time**
  (`render_fields`): the selected field's position among the rendered
  elements is computed (`select` = field_idx + the number of headers before
  it), the scrollbar counts against the full element count. A header is
  `header_line` (name muted+bold, a continuation line `─` in the border
  color; `─` needs no compat replacement in WGL4). The managed `llama-server`
  fields were laid out into Server/Model/Performance/Speculative decoding
  groups; sampling — `SamplingParam::group()` + a reordered
  `SAMPLING_PARAMS` (Basics/Dynamic temp/Diversity/Penalties/DRY/
  Mirostat/Order/Reasoning).
- **Value alignment — per group, not per section** (a `HashMap<group,
  max_label_w>` in the renderer, floor `MIN_LABEL_COL=20`): one long name
  (e.g. "Self observations in note_recall") no longer pushes away the value
  column of every group in the section. **Superseded** — a single per-section
  column with a cap, see "Post-M9: settings — a shared value column per
  section" below.
- **Shorter labels**: the prefix moves into the group header ("Embeddings:
  binary" → "llama-server binary" under "Embeddings (server)"; "RAG: chunk
  size" → "Chunk size (chars)" under "Knowledge base"; "Self-model: …" → "…"
  under "Self-model"; "tool: X" → "X" under "Tools").
- **A fixed bottom panel** (height 4, always reserved when there are fields —
  the list doesn't "jump"): the full value of a selected **long** text field
  (paths/URLs/system message; a >32-column threshold so short numbers/hosts
  aren't duplicated) + a description hint. Values in the list are
  **truncated with "…"** (`truncate_to_width`, a local analogue of
  chat_list) to the group column's width.
- **A section field counter** in the left menu (on the right, muted) — a
  quick sense of scope. Descriptions were added for fields that became
  noticeable (the agentic loop, the web/python master gates).
- **Tests**: navigation in tests moved from a fragile `Tab`/`Down` counter to
  stable helpers `goto_section(Section)`/`goto_field(FieldId)` (they survive
  section/group reordering); new tests — the composition of "Memory"
  (RAG/notes/self-model moved, `max_tool_rounds` in "Tools"), group markup,
  long-value truncation with "…". **773 unit tests green** (+3), clippy/fmt
  clean. No live run needed (a pure UI refactor, no engine).
- **Next (agreed plan)**: stage 2 — subsection tabs (a tab strip, embedding →
  "Model"), menu, a contextual footer; stage 3 — profiles: grouped tools with
  descriptions and honest gates (`⊘` for globally-disabled ones); stage 4 —
  `/` search; stage 5 — a Choice popup, validation without closing the
  editor, `Del`-reset, "modified vs. default" `•` markers; stage 6 — server
  statuses on-screen + an optional restart debounce.

### Post-M9: settings-screen redesign — stage 2 (subsection tabs + contextual footer) (done)
- Continuation of stage 1 (`feat/settings-redesign`, `screens/settings.rs`
  only; the `SettingsIntent`/`AppConfig` contract untouched, no migrations).
- **A subsection tab strip** replaces the pseudo-field "Subsection ‹Assistant›":
  the subsection selector (`ModelSub`/`SamplingSub`/`ProfileSub`) **remains
  field 0 in `fields()`** (navigation/the `←→` cycle unchanged — minimal
  risk), but **is no longer drawn as a list row**: `render_fields` injects it
  as a pinned line under the title (`tab_strip_line`: `Assistant │
  Impersonation │ …`, the active tab highlighted — a `keycap_bg` fill when
  the strip is focused, an accent color otherwise; `←→` shown on the right
  when focused). The section title was pulled out of the `List` block into a
  separate header (`head_area` = title + an optional tab strip); the
  scrollbar now spans the full height of `list_area`. Detecting the
  subsection field — `is_subsection`; it's excluded both from group alignment
  and from the list rows.
- **A third "Embeddings" tab under "Model"** — the embedding **server** moved
  out of "Tools" (mirroring the status bar's three chips: chat/imp/embed). A
  new type `ModelTab { Assistant, Impersonation, Embeddings }` (on
  `model_sub`; `cycle(dir)` — a 3-position cycle that respects direction),
  `Subsection` (2 variants) stayed for sampling/profiles. Tab labels — const
  `MODEL_TABS`/`SUB_TABS` (index = discriminant, `as usize`). The `EMode`/
  `EBinary`/… fields and their handlers weren't touched — only their UI
  location changed. "Tools" now holds only gates/parameters (Agentic
  loop/Web/Python/Files).
- **Profiles reordered**: `ProfileSub` (the tab strip) — field 0, then the
  section-level `PSelect` (profile picker) and `PName`, then the
  subsection-level Persona/Tools.
- **Contextual footer** (`render`): base hotkeys + section-specific ones — in
  "Profiles" `Ctrl+N new`/`Ctrl+D delete` are added.
- **Tests**: updated for `ModelTab` and the profile reordering (navigation via
  the stable `goto_section`/`goto_field`); new ones — the third "Embeddings"
  tab (fields in "Model", absent from "Tools"), tab-strip rendering (tabs
  visible, "Subsection" isn't a list row). **775 unit tests green** (+2),
  clippy/fmt clean. Pure UI refactor.

### Post-M9: settings-screen redesign — stage 3 (profiles: grouped tools + gates) (done)
- Continuation (`feat/settings-redesign`). The wall of ~32 flat "tool: X"
  toggles in a profile was replaced with a **grouped list with descriptions
  and honest gates**.
- **Catalog metadata moved into `features/tools/meta.rs`** (FSD: `screens`
  pulls it from there instead of hardcoding it): `tool_group(id)` (8 groups,
  ordered by `TOOL_GROUPS`), `tool_description(id)` (a short 2–4-word
  description), `tool_gate(id) -> Option<ToolGate>` (`Web`/`Python`/`Fs` —
  mirroring `effective_tool_ids`). Covered by a test: "every tool in
  `all_tool_ids` has a group and a description".
- **Profile toggles by group**: `profile_fields` lays out tools by
  `tool_group` (a stable sort by `TOOL_GROUPS` **without changing indices** —
  `PTool(idx)` remains the position in `tool_catalog()`, the source of truth
  for `toggle_profile_tool`; only display order is grouped). Group headers —
  the prior stage-1 mechanism.
- **Inline descriptions** (`FieldRow.hint: Option<&'static str>`): a short
  description to the right of `[x]` (`get_sampling  [x]  show sampling`).
  `render_field_line` draws the hint into the remaining width (truncated with
  "…"). The field is generic — other sections don't set it.
- **An honest gate** (`FieldRow.warn`): a tool enabled in the profile but
  disabled by the **global** switch (`web/python/fs_enabled`) is drawn in a
  **warning color** with a "disabled globally: <switch>" hint, and the bottom
  panel spells it out — "enable it in the 'Tools' section". This closes the
  "[x], but actually unavailable" trap
  (`SettingsScreen::gate_disabled` + `gate_hint`).
- **An "on/total" counter in the group header** (generic): groups with ≥2
  toggles carry `N/M` (`header_line` gained a `count` parameter) — used both
  in "Interface" (e.g. "Copy conversation (F5) 2/3") and in a profile
  ("Memory and knowledge 11/11").
- **Tests**: meta (group/description/gate for every tool); profile (toggles
  grouped + contiguous + described; a globally-disabled one is flagged with
  the gate, an enabled one isn't); `header_line` with and without a count.
  **780 unit tests green** (+5), clippy/fmt clean. Pure UI refactor (no
  engine); an "opt."-marker for optional tool groups (control/self_model) —
  left as groundwork.

### Post-M9: settings-screen redesign — stage 4 (field search `/`) (done)
- Continuation (`feat/settings-redesign`). With ~140 fields across 6 sections
  and 3 subsections, added a **global search `/`** — jump to a field without
  paging through sections.
- **Search index** (`build_search_index`): enumerates fields of **all**
  sections and **all** subsections (not just the active one). Field builders
  were parameterized by subsection — `model_fields_for(ModelTab)` /
  `sampling_fields_for(Subsection)` / `profile_fields_for(Subsection)` (the
  public `*_fields()` call them for the active one); `ModelTab::ALL`/
  `Subsection::ALL`+`from_index` for enumeration. Each hit carries jump
  coordinates (`section_idx`, the subsection discriminant, `field_idx`), a
  breadcrumb "Section · Subsection › Group › Field", and a lowercase haystack
  (label+group+description+hint+subsection). Mode-dependent fields
  (managed/cloud) are indexed for the current mode. `collect_hits` skips the
  subsection selector.
- **Overlay** (`SearchState { input, all, results, selected }`, a `search:
  Option<...>` field): `/` opens it (inside an editor `/` is a plain
  character), typing filters via an **AND over word-substrings**
  (`search_filter`), `↑/↓` select, `Enter` — `jump_to_selected` (sets the
  section/subsection/`field_idx`/field focus, closes the overlay), `Esc` —
  cancel, `Ctrl+K` — clear the query. Rendering (`render_search`): a dimmed
  background, the popup = the query line (`InputBox`) + a list of
  breadcrumbs with a value (muted) + a scrollbar; the title carries a
  "found/total" count. Clipboard paste is routed to the search line. `/
  search` was added to the contextual footer.
- **Tests**: `/` opens and filters; `Enter` jumps to a field (**including a
  subsection switch** — searching for a field on the "Embeddings" tab
  switches `model_sub`); `Esc` doesn't move navigation; the index covers
  fields of inactive subsections (>100 fields). **784 unit tests green**
  (+4), clippy/fmt clean. Pure UI refactor.

### Post-M9: settings-screen redesign — stage 5 (Choice popup, validation, `Del` reset, `•` marker) (done)
- Continuation (`feat/settings-redesign`). Four field-editing improvements.
- **A Choice-field picker popup** (`ChoiceState`, a `choice` field): `Enter`
  on a Choice field opens a list of all options with the current one marked
  (`↑↓`/`Enter`/`Esc`; `←/→` still does a quick cycle). Critical for
  `--spec-type` (9 options), the modes (5–6), themes, and **profile
  selection** (`PSelect`). `choice_menu(id)` returns (options, index) —
  reusing `FlashAttn::ALL`/`SpecType::ALL` (made `pub`), `SERVER_MODES`/
  `IMP_MODES`/`THEMES` + `sampling_choice_menu` (thinking/reasoning);
  `apply_choice` applies the pick via the **existing `cycle_field`** (steps
  from the current value to the target — no new setters).
- **Validation without closing** (`Editor.error`): an invalid numeric field
  committed with `Enter` no longer closes the editor — the title turns red
  with a `⚠`/`!` glyph + a message, an edit clears the error, `Esc` cancels.
  Classification via `field_num_kind`/`SamplingParam::num_kind` (Int/Float) +
  `field_validation_error` (a soft i64/f64 check; empty is valid; the exact
  type/range check is left to `apply_text`).
- **`Del` — reset a field to its default** (`reset_field`): compares the
  value with the field from a **default config** (`default_fields` — a
  temporary `SettingsScreen` over `AppConfig::default()` with the same
  navigation); if different — applies the default via
  `toggle_field`/`apply_choice`/`apply_text` (already default → a no-op,
  no redundant save). Profile fields (`is_profile_field`) are untouched
  (they have no config default).
- **A `•` marker** (accent-colored, 2 columns to the left, fields visually
  indented under the group header): `render_field_line` gained a `modified`
  parameter — `render_fields` compares the value against `default_fields`
  (built once per render). Profile fields aren't marked (the default config
  carries the same profiles → they're equal). `•`, `‹›`, `⚠`/`!` come from
  WGL4/GlyphSet.
- Footer: `Del reset` (when a field is focused). `FlashAttn`/`SpecType`
  gained `pub const ALL` (enumerating variants).
- **Tests**: the popup opens/applies a choice/`Esc` cancels; an invalid number
  keeps the editor + the error, an edit commits; `field_validation_error`
  classifies int/float/text; `Del` resets a modified field and is a no-op on
  a default one; the `•` marker appears on a modified field and is absent on
  the default (a render check). **791 unit tests green** (+7), clippy/fmt
  clean. Pure UI refactor.

### Post-M9: settings-screen redesign — stage 6 (server-status chips on-screen) (done)
- Completion of the track (`feat/settings-redesign`). The only stage with
  wiring outside `screens` (delivering the status snapshot, like for
  `Settings`/the palette — FSD intact).
- **A server-status chip in the "Model/server" section header**, on the
  right, contextual to the active subsection: Assistant → `chat`,
  Impersonation → `impersonation`, Embeddings → `embed` (`● ready` /
  `◐ connecting…` / `✕ not configured` / no connection with a reason;
  glyphs/colors mirror `widgets::status_bar`, the glyph is 1 column wide —
  compat-safe). Edit the engine → see `connecting… → ready` right there
  without leaving to the chat (a restart is the `Connecting` status coming
  from the orchestrator).
- **Wiring**: `SettingsScreen.statuses: ServerStatuses` + `set_server_statuses`;
  a new `ChatScreen::server_statuses()` getter. Runtime: on `OpenSettings` it
  sets the initial snapshot from the chat; `apply_event(ServerStatus)`, with
  the settings screen open, mirrors the status into it (live updates). A
  `render_field_line`-independent chip is right-aligned on the title line
  (`server_status_chip`/`span_width`).
- **Tests**: the chip shows the active subsection's server (chat→embeddings
  on tab switch), and doesn't render in other sections (a render check).
  **792 unit tests green** (+1), clippy/fmt clean.
- **Engine-restart debounce — deliberately deferred** (marked "optional" in
  the plan): the only genuinely risky change (in the orchestrator — the most
  concurrency-critical component, with the "sole owner of `Chat`" invariant;
  it changes the long-standing "applies on commit" semantics). The chips
  already give the restart observability the stage was aimed at; churn only
  happens when quickly editing several engine fields in a row, and it's now
  visible/tolerable. To be done as a separate, focused PR.
  **Closed** — see "Post-M9: engine restart debounce" below.

### Settings-screen redesign (stages 1–6) — summary
The `feat/settings-redesign` track is complete (6 commits). The settings
screen went from a flat list of ~120 fields to: **field groups** with
headers and per-section alignment + a fixed value/description panel
(stage 1); a **subsection tab strip** and moving the embedding server into
"Model" + a contextual footer (stage 2); **grouped tool toggles** with
descriptions and honest `⊘` gates (stage 3); `/` **search** across all
sections with a jump to a field (stage 4); a **Choice picker popup**,
validation without closing the editor, `Del` reset, and a "modified" `•`
marker (stage 5); **server-status chips** on-screen (stage 6). The
`SettingsIntent`/`AppConfig` contract was unchanged, no migrations; +a new
`features/tools/meta.rs`. Result: **792 unit tests**, clippy/fmt clean.
Groundwork: an "opt."-marker for optional tool groups (the engine-restart
debounce was done as a separate PR, see below).

### Post-M9: engine restart debounce on settings edits (done)
- **Problem**: the settings screen applies an edit **on every field commit**
  (`SettingsIntent::SaveConfig` → `AppCommand::UpdateConfig` →
  `handle_update_config` → an immediate `apply_chat_settings()`), so a series
  of edits like "binary → model → -ngl" produced three heavy managed
  `llama-server` restarts back to back. Deferred groundwork from stage 6 of
  the settings redesign.
- **Solution**: **the server restart** is debounced, not saving the config —
  the config is written to disk and re-emitted to the UI right away (the
  settings screen lives off the re-emit), only the expensive `apply_*`
  (re)launch is delayed. New module `app/orchestrator/restart_queue.rs` —
  `RestartQueue { chat, embed, impersonation: bool, deadline }` (mirroring
  `SaveQueue`): `mark_chat/embed/impersonation()` set a flag and extend the
  deadline (`RESTART_DEBOUNCE = 1.2s` from the **latest** edit), `take()`
  returns the flags and resets. In the `run()` loop — a second branch
  `sleep_until_opt(restart_deadline) => flush_restarts()`; `flush_restarts`
  (in `settings.rs`) calls `engines.apply_*` only for the flagged servers +
  **one** `emit_server_status()`. It reads the final `self.config` (already
  replaced during the edit) → a series of edits = one restart with the final
  values. The "sole owner of `Chat`" invariant is untouched, no new channels.
- **Stayed immediate**: the initial server bring-up (before the loop, in
  `mod.rs` — the queue doesn't apply there), persisting + `emit_settings()`,
  and rebuilding the tool registry (switching `tools`/the provider is cheap,
  and the get/set_sampling schema needs to be current from the next turn).
  On `Quit`, deferred restarts are **deliberately not flushed** (servers are
  torn down via Drop/kill_on_drop — there's no point bringing up a process
  right before dropping it). Within the debounce window the old server stays
  alive (`Ready`), then the chip shows `Connecting → Ready` — exactly the
  observability added by stage 6.
- **Tests**: unit `RestartQueue` (coalescing flags + a `take` reset; the
  deadline is extended by each mark — under paused virtual time); an
  integration test `model_change_restarts_chat_server_debounced` (formerly
  `model_change_restarts_chat_server`): two engine edits in a row → `Settings`
  is re-emitted immediately, `chat_call_count()` is still 1 (the restart is
  deferred), after the deadline — exactly 2 (one restart for the whole
  series; the flush marker is `AppEvent::ServerStatus`). Deterministic timing
  — `#[tokio::test(start_paused = true)]`: `tokio` with the **`test-util`**
  feature was added to dev-dependencies (test builds only; virtual time is
  advanced to the deadline once all tasks are idle — no races).
  `MockSupervisor.chat_call_count()` already existed (from M8's DoD).
  **794 unit tests green** (+2), clippy/fmt clean. Docs: architecture.md
  §3/§6, spec §11.6.

### Post-M9: settings — a shared value column per section (done)
- **Symptom**: per-group value alignment (stage 1 of the settings redesign)
  produced a "sawtooth" — each group had its own value-column stop (in
  "Memory" three groups meant three different columns), and one long label
  would push the values of its group far from short neighbors ("Theme
  ‹auto›" was pushed past "Old-terminal compatibility").
- **Fix** (`screens/settings.rs`): the value column is now computed **across
  the whole section** — `section_label_col` (the max label width among
  visible fields; floor `MIN_LABEL_COL=20`, **cap `LABEL_CAP=28`**; the
  subsection selector is excluded — it's a tab strip). Values and inline
  hints for every group now line up on one vertical. A label longer than the
  cap does **not** push the column — its value sits locally right after the
  label (a safety net; there are no such labels in the current set),
  `value_w` is computed from the real end of the label in that case
  (so the "…" truncation doesn't lie). `FieldRow.group` is now used only for
  the group header and the toggle counter.
- **Outlier labels shortened** by moving context into the group header (the
  stage-1 technique): "Copy with X" → "With X" (the verb is already in the
  header "Copy conversation (F5)"), "Old-terminal compatibility" → "Old-terminal
  mode", "Self observations in note_recall" → ""About self" in note_recall".
  Words needed for `/` search were preserved in the field descriptions
  (`field_description` — the search haystack includes the description).
- **Tests**: `section_label_col` (floor/cap/selector exclusion);
  `all_labels_fit_alignment_cap` — a **future-proofing gate**: labels of every
  section and subsection (including the speculative-decoding draft fields,
  visible only when `spec_type=draft-*`) must fit within `LABEL_CAP`,
  otherwise the test requires shortening the label or moving the meaning into
  the group header; `value_column_is_shared_across_groups` (a render check:
  the values of "Tools"'s three groups line up in one column). **797 tests
  green** (+3), clippy/fmt clean. Docs: spec §11.6.

### God-object refactor — stage 1: `screens/settings.rs` → `screens/settings/` (done)
- **Track plan** — [docs/history/refactoring-god-objects.md](docs/history/refactoring-god-objects.md)
  (7 stages + optional): splitting up several monolithic files that had grown
  into god objects (settings/chat/orchestrator-tests/notes/db/markdown/
  runtime). Method — the same playbook used for the orchestrator split
  (Phases 1–3): a **purely mechanical move** of a file into a directory
  module, no change to types/fields/channels/behavior.
- **Stage 1**: `screens/settings.rs` (4966 lines, one `impl SettingsScreen`
  ~2040 lines + ~1100 lines of free functions) was split into **8 files** of
  the `screens/settings/` directory (byte-exact slices by line ranges — zero
  transcription risk): `mod.rs` (650: `SettingsIntent`, the
  section/subsection enums `Section`/`Subsection`/`ModelTab`, the form types
  `FieldId`/`FieldKind`/`FieldRow`/`Editor`/`Focus`/`SearchHit`/`SearchState`/
  `ChoiceState`, `SamplingParam` + `SAMPLING_PARAMS`, `struct SettingsScreen`,
  submodule declarations), `catalog.rs` (565: the constructor + field
  builders for all sections/subsections + gates `gate_disabled`/
  `sampling_cloud_provider`), `apply.rs` (675: `handle_key` + dispatchers +
  the field editor + toggles/cycles + `apply_text`/`save_*`), `choice.rs`
  (150: the Choice popup + `reset_field`/`default_fields`), `search.rs`
  (155: the `/` search overlay), `render.rs` (530: all rendering),
  `helpers.rs` (1130: free functions — `row`/`grouped`/`sampling_row`/
  `managed_rows`/`cloud_rows`, `field_description`/`gate_hint`,
  `render_field_line`/`header_line`/`tab_strip_line`, `cycle_*` cycles/label
  functions, `parse_*` parsers/`decode/encode_escapes`), `tests.rs` (1175).
  The former `impl SettingsScreen` was split into 4 impl blocks (≤ ~660
  lines each).
- **Visibility rules (a playbook for UI screens):** (1) submodules pull
  `mod.rs`'s items in via `use super::*;` — the glob also picks up the
  parent's private `use` imports (ratatui/uuid/crate::…), so submodules
  don't duplicate outside imports, and none go unused in `mod.rs` (all are
  used transitively → zero warnings). (2) `helpers.rs` free functions are
  marked `pub(super)` (otherwise siblings can't see them); consumers use
  `use super::helpers::*;`. (3) private `impl SettingsScreen` methods called
  from another file are marked `pub(super)` (method privacy in Rust follows
  the defining module).
- **Deviations from the plan**: `descriptions.rs`/`editor.rs` weren't split
  out (`field_description`/`gate_hint` → `helpers.rs`; the field editor →
  `apply.rs`, tightly coupled with key handling). `helpers.rs` was left as
  one module of free functions — further splitting was deferred (independent
  pure functions, not a tangled impl block). The module's public path was
  unchanged (`crate::screens::settings::{SettingsScreen, SettingsIntent}`),
  outside `use` sites (`app/runtime.rs`) weren't touched. **805 tests green**
  (0 failed, 26 `#[ignore]`; the count is unchanged — a pure move), clippy
  `-D warnings`/fmt clean. Docs: architecture.md §3.
### God-object refactor — stage 2: `screens/chat.rs` → `screens/chat/` (done)
- **Stage 2** of the god-object split plan
  (docs/history/refactoring-god-objects.md — living on the stage-1 branch;
  this stage — branch `refactor/chat-module-split` off `main`).
  `screens/chat.rs` (2616 lines, one `impl ChatScreen` with ~54 methods over
  ~1100 lines + render helpers) was split into the `screens/chat/` directory
  — a purely mechanical move (item-level slices: methods cut on the closing
  4-space `}`, free functions on column-0 closures; types/fields/the
  `ChatIntent` contract/behavior unchanged).
- **Layout**: `mod.rs` (409: `ChatIntent`, popup types `ConfirmAction`/
  `SuggestPopup`/`ImpersonationState`/`RagBanner`, `struct ChatScreen`,
  snapshot accessors — settings/list/status/palette setters/getters),
  `input.rs` (367: `handle_key`/`handle_paste`/`handle_mouse` + draft/spell/
  commands + `trigger_destructive`/`handle_profile_overlay_key`), `popups.rs`
  (281: the spellcheck/confirmation/emoji/help popups +
  `render_help`/`render_suggest`/`render_confirm`/`HELP_KEYS`), `feed.rs`
  (241: projecting `AppEvent` into the feed + `feed_msg_has_vs16`),
  `render.rs` (190: `render`/`model_meta` + `centered_rect`/
  `visual_line_count`), `rag.rs` (94: the banner + `format_rag_sources`),
  `impersonation.rs` (51), `tests.rs` (1034). The former `impl ChatScreen`
  was split into 7 impl blocks (≤ ~360 lines each).
- **Visibility rules** (the same playbook as stage 1): submodules pull
  `mod.rs` via `use super::*;`; private methods called across files, and free
  functions, are marked `pub(super)`. Cross-module free functions get
  targeted `use`s: `input.rs` → `super::feed::feed_msg_has_vs16`, `popups.rs`
  → `super::render::centered_rect`, `render.rs` →
  `super::popups::{render_help, render_suggest, render_confirm}`, `tests.rs`
  → `super::popups::HELP_KEYS`.
- The module's public path was unchanged (`crate::screens::chat::{ChatScreen,
  ChatIntent}`), outside `use` sites (`app/runtime.rs`) weren't touched.
  **805 tests green** (0 failed, 26 `#[ignore]`; the count is unchanged — a
  pure move), clippy `-D warnings`/fmt clean. Docs: architecture.md §3.
### God-object refactor — stage 3: `orchestrator/tests.rs` → `orchestrator/tests/` (done)
- **Stage 3** of the god-object split plan
  (docs/history/refactoring-god-objects.md — living on the stage-1 branch
  `refactor/settings-module-split`; this stage — branch
  `refactor/orchestrator-tests-split` off `main`, the plan doc is
  synchronized on merge). The test monolith `app/orchestrator/tests.rs`
  (3506 lines, 74 tests for every feature in one file) was split into the
  `app/orchestrator/tests/` directory — a purely mechanical move (item-level
  slices, no test behavior/name changes).
- **Layout**: `mod.rs` (315: **all fixtures** —
  `spawn_orch`/`spawn_orch_cfg`/`wait_for`/`bare_orch*`/`enable_all_tools`/
  `orch_*`/live helpers + submodule declarations) and one file per feature,
  mirroring the source modules: `generation.rs` (755), `live.rs` (959, all
  `#[ignore]` e2e smokes), `chats.rs` (323), `self_model.rs` (317), `rag.rs`
  (267), `impersonation.rs` (169), `settings.rs` (158), `profiles.rs` (113),
  `title.rs` (107), `reflection.rs` (64), `request.rs` (17). The largest file
  dropped from 3506 to 959.
- **Key rules** (a playbook for test modules): (1) **every** non-`#[test]`
  helper → `mod.rs`, so any submodule can see them via `use super::*;`
  (eliminates cross-file fixture visibility issues). (2) A name collision:
  the test submodule `tests::generation` shadows the source module
  `orchestrator::generation` — in `self_model.rs`, references to self-model
  injection (`inject_self_model`/`blend_self_notes`/`injection_recent`/
  `GenResult`) were rewritten from `super::generation::` to
  `super::super::generation::` (up to `orchestrator`). No other submodule
  has this collision.
- The `tests` module's path was unchanged (`mod tests;` in
  `orchestrator/mod.rs`). **805 tests green** (0 failed, 26 `#[ignore]`; the
  count is unchanged — a pure move), clippy `-D warnings`/fmt clean. Docs:
  architecture.md §3.

### God-object refactor — stage 4: `features/tools/notes.rs` → `features/tools/notes/` (done)
- **Stage 4** of the god-object split plan
  (docs/history/refactoring-god-objects.md — on the stage-1 branch; this
  stage — branch `refactor/notes-module-split` off `main`).
  `features/tools/notes.rs` (2242 lines: 9 tools + the self-notes subsystem
  + consolidation overviews) was split into the `features/tools/notes/`
  directory — a purely mechanical move (item-level slices, no behavior
  change).
- **Layout**: `mod.rs` (123: the `NOTE_*_ID` id constants, thresholds,
  `SELF_NOTE_TAG`, shared helpers `is_self_note`/`parse_id`/`parse_tags`/
  `clip`/`cosine`, re-exports), `recall.rs` (234: `NoteRecall` +
  `list_user_notes`/`semantic_recall`/`related_block`/
  `cited_sources_block`/`format_notes`), `edit.rs` (222: `NoteRevise`/
  `NoteSupersede`/`NoteMerge`), `overview.rs` (206: `ConsolidateNotes` +
  `build_consolidation_overview`/`build_self_consolidation_overview`),
  `save.rs` (161: `NoteSave` + `create_note`/`ensure_note_vectors`/
  `self_note_similar`), `self_notes.rs` (149: `self_notes_recent`/
  `self_notes_relevant`/`self_related_block`/`migrate_self_narrative`),
  `graph.rs` (115: `NoteLink`/`NoteNeighbors`), `cite.rs` (66:
  `NoteCiteSource`), `tests.rs` (1005).
- **Key point — preserving the external surface** (broad: the orchestrator/
  `self_model`/`rag`/`tools::mod`/`meta` call ~30 `notes::X` items): mod.rs
  re-exports everything via `pub(crate) use self::{cite::*, edit::*, …}::*;`,
  so outside `use crate::features::tools::notes::{NoteSave, create_note,
  self_notes_recent, …}` sites weren't touched. Private cross-submodule
  helpers (`related_block`/`list_user_notes`/`semantic_recall`/
  `format_notes`/`ensure_note_vectors`) were widened to `pub(crate)`;
  submodules pull everything in via `use super::*;` (a glob through the
  parent's re-export). Constants and small shared helpers stayed in `mod.rs`
  (visible to submodules as the parent's private items).
- **A parsing subtlety** (handled): a `///` line doc comment ending with `;`
  (prose like "…for storage/search;") was falsely treated as an item
  boundary and split the doc comment — the parser gained a guard: "a `;`
  isn't treated as a boundary on a line starting with `//`".
- The module's public path was unchanged. **805 tests green** (0 failed, 26
  `#[ignore]`; the count is unchanged — a pure move), clippy
  `-D warnings`/fmt clean. Docs: architecture.md §3.

### God-object refactor — stage 5: `shared/storage/db.rs` → `shared/storage/db/` (done)
- **Stage 5** of the god-object breakup plan (docs/history/refactoring-god-objects.md — on
  stage 1's branch; this stage — branch `refactor/db-module-split` off `main`).
  `shared/storage/db.rs` (1665 lines, one `impl Db` with ~41 methods, 4 data domains)
  split into the `shared/storage/db/` directory by domain — a purely mechanical move
  (item-level slices, DB behavior/schema unchanged).
- **Layout**: `mod.rs` (222: `struct Db`, `open`/`open_in_memory`/`from_conn`,
  `register_sqlite_vec`, **`migrate()` — the whole schema in one block**, `ensure_vec_table`/
  `vec_dim`, shared helpers `row_to_note`/`parse_uuid`/`parse_dt`/`cosine`, declarations),
  `rag.rs` (321: documents/search/sources/dimensionality + delete-by-path +
  `delete_matching`/`delete_sources_matching`/`norm_path`), `notes.rs` (242: insert/
  list/edit/delete + embeddings/semantics), `graph.rs` (218: link graph +
  supersession + source citation), `self_model.rs` (102: get/upsert/atomic
  update), `tests.rs` (591). The former `impl Db` split into 5 impl blocks (≤ ~320 lines each).
- **Key point**: `Db` methods are inherent methods (`pub`, called as
  `db.method()` through the `Storage` facade), so splitting `impl Db` across files
  **requires no re-exports** (the method resolves by type regardless of file). Private
  `struct Db` fields (`conn`) are visible to submodules (children see the parent's
  private items); shared free helpers in `mod.rs` (private) are picked up by
  submodules via `use super::*;` (a glob pulls in the parent's private items).
  It compiled on the first try — no visibility fixes, no explicit imports.
- The `db` module path is unchanged (`shared::storage::db::Db`). **805 tests green**
  (0 failed, 26 `#[ignore]`; the count didn't change — a pure move), clippy
  `-D warnings`/fmt clean. Docs: architecture.md §3.
### God-object refactor — stage 6: `shared/markdown.rs` → `shared/markdown/` (done)
- **Stage 6** of the god-object breakup plan (docs/history/refactoring-god-objects.md — on
  stage 1's branch; this stage — branch `refactor/markdown-module-split` off `main`).
  `shared/markdown.rs` (1995 lines, three subsystems: walker `Writer`, tables, LaTeX
  converter + code highlighting) split into the `shared/markdown/` directory — a purely
  mechanical move (item-level slices with awareness of dependencies, behavior unchanged).
- **Layout**: `mod.rs` (146: `render`/`render_with` — **the entire external surface** +
  palette-derived styles `heading_style`/`code_style`/… + wiring), `latex.rs` (578: LaTeX→
  unicode — delimiter normalization + command converter, **self-contained**),
  `writer.rs` (418: `Writer` — pulldown-cmark event walker → lines + `heading_number`),
  `table.rs` (312: `TableBuilder` + `render_table` + column layout), `code.rs` (176:
  syntect highlighting — syntax + theme from the palette), `tests.rs` (397).
- **Key point — `Writer` is the hub** (calls styles from `mod.rs`, highlighting from
  `code`, tables from `table`, LaTeX from `latex`; `mod.rs::render_with` builds the
  `Writer`). Wiring: subsystem items used across files are marked `pub(super)`
  (including `TableBuilder` fields — built/mutated by `Writer` and read by
  `render_table` — and the `Writer.lines`/`soft_break_as_newline` fields, which
  `render_with` accesses); `mod.rs` collects them with a private glob
  `use self::{code::*, latex::*, table::*, writer::*};`, and submodules pick up
  everything via `use super::*;` (styles from `mod.rs` — as the parent's private items).
- **Two clippy lessons** (not caught by `cargo build`, only `-D warnings`): (1) a
  re-export of internal wiring must be a **private** `use …::*` (not `pub(crate) use`) —
  otherwise "glob import doesn't reexport anything with visibility pub(crate)", since
  the items are `pub(super)`, not `pub`; (2) `latex.rs` is self-contained — its
  `use super::*` turned out unused and was removed.
- The external surface (`markdown::render`/`render_with`, used only by `message_feed`)
  is untouched. **805 tests green** (0 failed, 26 `#[ignore]`; the count didn't change —
  a pure move), clippy `-D warnings`/fmt clean. Docs: architecture.md §3.
### God-object refactor — stage 7: `app/runtime.rs` → `app/runtime/` (done)
- **Stage 7** (final) of the god-object breakup plan
  (docs/history/refactoring-god-objects.md — on stage 1's branch; this stage — branch
  `refactor/runtime-module-split` off `main`). `app/runtime.rs` (1099 lines: the TUI
  loop + paste batching + clipboard + dispatch) split into the `app/runtime/`
  directory — a purely mechanical move (item-level slices, behavior unchanged).
- **Layout**: `mod.rs` (309: `run`/`run_loop` — the loop + dirty repaint,
  `ActiveScreen`, `SpellLoader`, `TICK` + wiring), `dispatch.rs` (334: `apply_event`
  applies `AppEvent` to the screen + `dispatch`/`dispatch_chat_list`/`dispatch_settings`/
  `dispatch_self_model` translate Intent→`AppCommand`), `input.rs` (218: input batching
  and clipboard-paste handling on Windows — `Chunk`/`chunk_batch`/`process_input_batch`/
  `collect_press`/`paste_char`/`reconcile_paste`/`paste_projection_matches`),
  `clipboard.rs` (30: `read_clipboard_text`/`write_clipboard` via arboard), `tests.rs`
  (238). The external surface — only `run` (main.rs) — remains `pub` in `mod.rs`.
- **Wiring** (as in markdown): cross-file items are marked `pub(super)`, `mod.rs`
  collects them with a private glob `use self::{clipboard::*, dispatch::*, input::*};`
  (the `run_loop` hub calls them by their short name); submodules pick up
  `ActiveScreen`/types via `use super::*;`. **cfg gating is preserved** (`#[cfg(windows)]` on
  `read_clipboard_text`/`paste_projection_matches`, `#[cfg(windows)]`/`#[cfg(not(windows))]` on `reconcile_paste`).
- **Linux-build nuance**: `clipboard.rs` only uses `arboard::` (full path) and
  the prelude — its `use super::*` turned out unused (on Linux `read_clipboard_text`
  is absent under cfg, leaving only `write_clipboard`) and was removed to avoid an
  unused-import hit under `-D warnings` on non-Windows targets.
- The `runtime` module path is unchanged. **805 tests green** (0 failed, 26 `#[ignore]`;
  the count didn't change — a pure move), clippy `-D warnings`/fmt clean. Docs:
  architecture.md §3. **God-object breakup (stages 1–7) complete**: settings/chat/
  orchestrator-tests/notes/db/markdown/runtime; the largest source file dropped from 4966 to
  ≤1175 lines, all impl blocks ≤ ~660.
### Post-M9: settings-section field counter tied to the selected mode (done)
- **Reversal of a previous decision** (PR #121, "stable counter = union across modes/
  subsections"): at the user's request, the field counter for a settings section in
  the left menu is now **tied to the currently selected mode** of each server's
  engine/provider (managed/external/openai/gemini/claude), rather than a fixed union
  across all modes. Motive — **consistency with search**: the sum of section counters
  should match the number of fields in the search overlay (`/`).
- **Implementation — single source of truth**: `section_counts`/`section_field_count`
  (`screens/settings/render.rs`) are derived from the same index as search
  (`build_search_index`) — it enumerates fields of all subsections (tab strips) for
  their **current** modes, skipping subsection selectors (`collect_hits`). `section_counts`
  groups hits by `section_idx` in a single pass → the sum is identically equal to
  `build_search_index().len()`. Both functions became `&self` (no longer need to swap
  the config to iterate over modes); `section_field_count` is now `#[cfg(test)]` (the
  renderer only calls `section_counts`). The counter updates live on mode change (read
  from `self.config` on every render).
- **Tests**: `section_counts_sum_matches_search_index` (the sum of section counters ==
  the number of search fields across different modes); `section_count_tracks_selected_mode`
  (cloud shows fewer fields in the "Model" section than managed — the counter reflects
  this; matches the section's field count in the search index). The previous
  `section_field_count_is_tab_and_mode_independent` was replaced (it asserted the
  opposite). **808 tests green**, clippy/fmt clean.
### Post-M9: co-locating db/markdown tests with their submodules (done)
- **Tail of the god-object breakup** (docs/history/refactoring-god-objects.md §2.5): when
  breaking up stages 5 (db) and 6 (markdown), the domain tests were collected into a
  single `tests.rs`, even though the plan called for distributing them into `mod tests`
  in their own subfiles (following the "tests next to code" convention). Brought in
  line with the plan — a **pure mechanical move** (byte-exact slices of test bodies,
  names unchanged; behavior/types/schema untouched). Screens
  (`settings`/`chat` — tests "via `handle_key`/`render`") and `orchestrator/tests/`
  (already split by feature) were deliberately left as-is; `notes/tests.rs` was left
  untouched (many cross-tool integration tests save→recall→cite with no single
  "home"); `runtime/tests.rs` is small (238 lines) — not worth splitting.
- **db** (`shared/storage/db/tests.rs`, 591 → removed): 27 tests distributed into `mod tests`
  in the subfiles — `notes.rs` (9), `graph.rs` (5: supersede/links/merge/cite), `self_model.rs`
  (3), `rag.rs` (10). Each `mod tests` — `use super::*` + a local `fn db() ->
  Db { Db::open_in_memory().unwrap() }` (3 lines, self-contained; `Db`'s inherent methods
  resolve by type regardless of file).
- **markdown** (`shared/markdown/tests.rs`, 397 → removed): ~33 tests in `mod tests`
  in the subfiles — `code.rs` (resolve_syntax + highlighting; +`use crate::shared::config::Theme`
  to shadow syntect's `Theme`), `latex.rs` (latex_to_unicode/normalize + math-via-
  render), `table.rs` (table layout), `writer.rs` (base render/lists/quotes).
  Shared render test helpers (`rendered_text`/`rendered_text_w`/`max_line_width`/
  `fg_colors` + the `TABLE_MD`/`CODE_MD` constants) were moved into `#[cfg(test)] pub(super) mod
  testkit` in `mod.rs` (the §2.5 playbook); submodules pick them up via `use super::super::testkit::*`.
- **Visibility**: `mod tests { use super::* }` inside a subfile sees `mod.rs` items
  transitively (the subfile itself does `use super::*`; the descendant sees the
  parent's private imports — the same trick used in the orchestrator split);
  testkit helpers are `pub(super)` (public within the markdown module → visible to
  all its test descendants). Indentation is normalized by
  `cargo fmt`. **808 tests green** (count unchanged — a pure move), 26 `#[ignore]`,
  clippy `-D warnings`/fmt clean. Docs: architecture.md §3.

### Post-M9: tool-catalog metadata — a single source of truth in the `Tool` trait (done)
- **Metadata for profile toggles (semantic group, short label, global
  gate, "enabled by default") moved from centralized match tables in
  `features/tools/meta.rs` into the `Tool` trait itself** — each tool declares them
  right next to its own code (single source of truth). Previously this information
  lived in four `match id { … }` blocks (`tool_group`/`tool_description`/`tool_gate` +
  the `default_tool_ids`/`all_tool_ids` list), which could easily drift from the
  actual set of tools.
- **Trait contract** (`features/tools/mod.rs`): `fn group(&self) -> meta::ToolGroup`
  and `fn ui_label(&self) -> &'static str` — **mandatory** (no default), so a
  new tool **cannot** be added without declaring a group and a label (compile-time
  instead of the previous runtime test with fallbacks `_ => "Other"`/`_ => ""`);
  `fn gate(&self) -> Option<meta::ToolGate>` and `fn enabled_by_default(&self) -> bool`
  — with sensible defaults (`None`/`true`) for the typical case (optional control-/
  self-model tools declare `false`).
- **`ToolGroup` — an enum** (instead of `TOOL_GROUPS: [&str; 8]`): the variant order = the
  display order of groups (`derive(Ord)` → stable toggle sorting without string
  matching); `title()` gives the header, `ToolGroup::ALL`/`group_titles()` enumerate
  them. `meta.rs` now carries only **types** (`ToolGroup`/`ToolGate`/`ToolInfo`), not values.
- **Catalog from the registry**: `ToolRegistry::infos()` takes a `Vec<ToolInfo>` snapshot
  (id + group + label + gate + default) from the live tools; a static `CATALOG: LazyLock<Vec<ToolInfo>>`
  = `standard_registry(&ToolConfig::default()).infos()` (metadata doesn't depend on
  `ToolConfig` → built once, so `effective_tool_ids`, called on every agentic-loop
  round, doesn't rebuild the registry). `default_tool_ids`/`all_tool_ids`/`tool_catalog`
  are derived from `CATALOG` by filter/projection; `effective_tool_ids` takes `ToolGate` from
  the metadata (the dynamic sampling gate by provider remains a separate branch).
- **Settings UI** (`screens/settings/`): `SettingsScreen::tool_catalog()` returns a
  `Vec<ToolInfo>` instead of `Vec<String>`; profile-toggle grouping sorts by
  `ToolGroup` (Ord), the label/gate come from `ToolInfo` (not from `meta::tool_*`). The
  `PTool(idx)` index is still — a position in the stable `CATALOG` (toggle correctness
  is preserved: display and toggling read the same static).
- **Behavior change (cosmetic):** `default_tool_ids`/`all_tool_ids` are now in
  **alphabetical** order (the registry is a `BTreeMap`), rather than the previous manual
  grouped order. This only affects the order tools are presented to the model in new/
  reconciled profiles (`migration.rs`/`reconcile_tools`) and the order of `PTool`
  indices — it doesn't affect correctness (`tool_choice=auto`; the UI resorts by group
  anyway). `all_tool_ids`/`group_titles` are now only used by tests
  (`#[allow(dead_code)]`, kept as a public API).
- **Tests**: `meta` (every tool in the catalog has a group and a non-empty label;
  gates `web_search`/`fetch_url`→Web, `python_exec`→Python, `fs_write`→Fs, `note_save`→
  None — now via `tool_catalog()`); `mod` (the registry contains the whole catalog; the
  fake `Echo` declares `group`/`ui_label`); settings tests moved to `ToolInfo`/
  `tool_catalog()` (the toggle is picked up by the index of an actually enabled tool,
  since the catalog is ordered by id). **808 tests green** (a pure refactor, no engine),
  clippy `-D warnings`/fmt clean. Docs: architecture.md §8.

### Post-M9: SOLID refactor — stage 1: `ToolContext` (dependency bundles + constructor) (done)
- **First stage of the targeted SOLID-improvements track**
  ([docs/history/refactoring-solid.md](docs/history/refactoring-solid.md), branch
  `refactor/tool-context-bundles`): eliminated shotgun surgery when adding a
  `ToolContext` field — previously an 11-line literal was repeated in **8 places** (3
  production + 5 test), a new field meant editing all of them. A purely structural
  refactor (behavior unchanged).
- **Three building blocks + a constructor** (`features/tools/mod.rs`, **the flat
  public `ToolContext` fields are preserved** → tool code such as `ctx.storage`/
  `ctx.chunk_params`/… is untouched): `ToolDeps` (shared `Arc`s: storage/engine/
  embedder), `ToolParams` (a snapshot of parameters from the config; `from_config(&AppConfig)` —
  the **single** place that does the mapping) and `TurnInfo` (a snapshot of the turn:
  identity + `Chat` fields); `ToolContext::new(deps, params, turn)` unpacks them into the
  previous fields.
- **Orchestrator**: a helper `tool_deps(&self, backend) -> ToolDeps` (next to the shared
  helpers in `mod.rs`); the three production call sites switched to `new` — reflection/
  consolidation via `ToolParams::from_config(&self.config)`, generation too
  (`self_model_params` is computed separately there — it's still passed into `GenSpawn`
  for injection). **Borrow nuance**: in generation, `chat_mut` holds `&mut self`, so
  `TurnInfo` (the last access to `chat`) is built into a local before `new`, after
  which the `chat` borrow ends and `self.config`/`tool_deps` can be read.
- **testkit**: `ctx_with_storage` via `new`; added `ctx_with_backends`
  (custom engine/embedder — web/subagent/fetch delegate their local
  `ctx_with_engine` to it) and `ctx_with_deps` (a shared bundle for the rag isolation
  test, where two contexts share one storage). All literals in tool tests were removed.
- **Ripple check**: adding a field to `ToolContext` now requires editing **only**
  `ToolContext::new` (verified with a trial field). **808 tests green** (count
  unchanged — a refactor), 26 `#[ignore]`, clippy `-D warnings`/fmt clean. Docs:
  architecture.md §8.

### Post-M9: SOLID refactor — stage 2: background tasks (slot registry + a single done channel) (done)
- **Stage 2** of the SOLID-improvements track
  ([docs/history/refactoring-solid.md §4](docs/history/refactoring-solid.md), branch
  `refactor/bg-task-slots`): the family of "silent" background tasks (self-model
  auto-reflection + notes auto-consolidation — a UI-less mini agentic loop via the
  shared runner `tool_loop::spawn_silent_loop`) was maintained by copy-pasting its
  lifecycle — a triplet of fields + a channel + a `select!` arm + a handler per task.
  Prepares the ground for family member #3 (self-model auto-consolidation on a timer,
  roadmap §9.9): adding it will no longer touch `run()`/`Quit`. Purely structural,
  behavior unchanged (error/event texts byte-for-byte).
- **Slot registry** (`app/orchestrator/background.rs`, a new module): `BgSlot { cancel:
  Option<CancellationToken>, failures: u32 }` (a failure streak outlives a single run →
  belongs to the slot, not the task); the key is the existing `BackgroundKind` (given
  `Hash`). Methods on `impl Orchestrator`: `bg_running(kind)` (the "one at a time" gate), `begin_bg(kind, cancel)`
  (sets the "running" flag + the status-bar indicator), `handle_bg_done(kind, result)` (a
  **shared** outcome handler: clearing the indicator, a failure streak → a single error
  at the `BACKGROUND_FAILURE_ALERT` threshold, on **reflection**'s success — `SelfModelChanged`,
  none for consolidation), `cancel_all_bg()` (for `Quit`), `#[cfg(test)] bg_failures(kind)`.
  Error texts are assembled from `kind_label(kind)` ("Auto-reflection"/"Auto-consolidation")
  **byte-for-byte** with the previous ones — tests check exactly those.
- **Orchestrator fields 6 → 2**: `reflect_cancel`/`reflect_done_tx`/`reflect_failures` +
  `consolidate_cancel`/`consolidate_done_tx`/`consolidate_failures` → `bg: HashMap<
  BackgroundKind, BgSlot>` + `bg_done_tx: UnboundedSender<(BackgroundKind, Result<(),
  String>)>`. `consolidate_counts` (the per-chat consolidation cadence) **kept** — it's
  cadence data, not task lifecycle. In `run()`: two channels/two `select!` arms
  → one `bg_done` + one arm; `Quit` — enumerate the tokens → `cancel_all_bg()`.
- **`SilentLoop`** (`tool_loop.rs`) gained a `kind: BackgroundKind` field; `done_tx` now
  sends `(kind, outcome)` instead of a bare outcome. The spawn tails of
  `maybe_auto_reflect`/`maybe_auto_consolidate` were switched to `begin_bg` (sets the
  cancel token + the indicator), the "already running" gates — to
  `bg_running`; `handle_reflect_done`/`handle_consolidate_done` removed.
- **Family boundaries** (untouched): impersonation (its own done channel `(Uuid,
  FinishReason)`, streaming to the UI), RAG indexing (no done channel, progress via
  `RagProgress`), auto-title (`title_tx`, a result carrying the chat id). `gen_state`/
  `rag_cancel`/`imp_cancel` in `Quit` are unchanged.
- **DoD**: the `reflect_*`/`consolidate_cancel|_done_tx|_failures` fields removed; a single bg
  arm in `run()`; `BACKGROUND_FAILURE_ALERT` — the sole consumer of `handle_bg_done`.
  Tests moved onto the new API without renames (`bg_running`/`handle_bg_done`/
  `bg_failures`). **808 tests green** (count unchanged — a refactor), 26 `#[ignore]`,
  clippy `-D warnings`/fmt clean. Docs: architecture.md §3 (module map), §11
  (concurrency).

### Post-M9: SOLID refactor — stage 4: status-bar view-model + canonical runtime helpers (done)
- **Stage 4 (small, targeted)** of the SOLID-improvements track
  ([docs/history/refactoring-solid.md §6](docs/history/refactoring-solid.md), branch
  `refactor/status-bar-runtime`): removed the status bar's 10-argument signatures and
  scattered named screen enumerations in runtime. Purely structural (behavior
  unchanged). Did 4a/4b/4d; 4c (grouping `ChatScreen` fields) — **not done**
  (per the plan, only incidentally while touching `chat/`; not worth a standalone PR).
- **4a — status-bar view-model** (`widgets/status_bar.rs`): `render`/`height` carried
  10 arguments each (`#[allow(too_many_arguments)]`). Introduced `StatusModel<'a>` (a
  snapshot: statuses/generating/tokens/context/context_exact/mouse_scroll/background); `render`
  → 4 parameters, `height` → 3, both `allow`s removed. `ChatScreen` builds the snapshot via one
  private helper, `status_model()` (`chat/render.rs`) — a new indicator = a field +
  one place to fill it, no signature churn. **Borrow nuance**: the helper borrows all
  of `&self`, and between `height` and `render` there's `&mut self.feed_view` — so the
  snapshot is built as a temporary at each of the two call sites (a short-lived borrow
  that doesn't overlap the `&mut`), rather than held in a local. Tests were switched to
  a `StatusModel` literal (via the test helper
  `model()`; the `ready()` snapshot is bound to a local — otherwise the temporary would
  outlive the borrow).
- **4b — canonical screen enumerations** (`app/runtime/`): the palette broadcast (theme/
  compat-mode change) and paste routing were consolidated into methods on
  `ActiveScreen` itself (`set_palette`/`handle_paste`, next to the enum in `mod.rs`) —
  the `Settings` arm in `apply_event` keeps its own `refresh` (broader than the
  palette), other screens are handled by `other.set_palette(...)`. Intent
  taken off the active screen is now dispatched through a single
  `enum AnyIntent { Chat|List|Settings|SelfModel }` +
  `dispatch_any` (single ownership instead of 4 parallel `Option`s and 4 nearly
  identical `if` blocks — the previous shape was a workaround for `active`/`screen`
  borrow conflicts).
- **4d — clipboard out of `apply_event`**: the write + confirmation/error routing were
  moved into `deliver_clipboard(screen, active, clipboard, text)` (`dispatch.rs`) —
  `apply_event` no longer knows about `arboard` (the `CopyToClipboard` arm is a single call).
- **Deliberately left as-is**: the per-event `match active` in `apply_event`
  (ServerStatus/ChatList/ChatActivated/…) — this is event logic with different
  semantics per screen, not a "screen enumeration"; enum dispatch is idiomatic here
  (plan §6: "the `match` over screens doesn't go away — that's not the goal").
- **DoD**: status-bar signatures ≤4 with no `allow`; no named screen enumerations in
  `dispatch.rs`/`input.rs` outside of `ActiveScreen`/`dispatch_any`. **808 tests
  green** (count unchanged — a refactor), 26 `#[ignore]`, clippy `-D warnings`/fmt
  clean. Docs: architecture.md §9.

### Post-M9: SOLID refactor — stage 3, step 3.1: settings-field description in `FieldRow` (done)
- **Step 3.1** of stage 3 (settings-field descriptors,
  [docs/history/refactoring-solid.md §5](docs/history/refactoring-solid.md), branch
  `refactor/settings-field-descriptors`): the aspects of a single settings field were
  smeared across five match sites; 3.1 co-locates the **description** at the row-
  building site (SRP groundwork). Purely structural, behavior unchanged.
- **`FieldRow`** gained `description: Option<&'static str>` + a builder `describe(d)`
  (`row(...).describe("…")`). The 190-line `field_description(id)` match was **removed**;
  the texts moved: section-level ones (Tools/Memory/Interface) — as inline literals in
  `catalog.rs`; texts shared across several construction sites (engine mode, cloud
  model name, API-key env-var name, subsection selector) — as `const DESC_*` in `helpers.rs`;
  `-ngl`/`--jinja` (texts **differ** between assistant and impersonation) — as new fields
  `ngl_desc`/`jinja_desc` in `ManagedFieldIds`; other managed fields (no-mmap/flash-attn/
  spec-*, shared by both engines) — inline in `managed_rows`; sampling — `sampling_row`
  sets `p.description()` (the `SamplingParam::description` source is untouched).
- **Consumers** (the bottom panel in `render.rs`, the search trap `collect_hits` in
  `helpers.rs`) now read `row.description` instead of calling `field_description(f.id)`.
- **Nuance** (a consequence of co-location): a description now exists only for
  **visible** rows (speculative-decoding draft fields — only when
  `spec_type=draft-*`). Behavior parity was preserved: `XModelName`/`IxModelName`/`EModelName` in
  external mode also get a description (the old match matched by id regardless of
  mode). The `field_description(id)` tests were switched to a helper `field_desc(&screen, id)`
  (builds the section/subsection fields and looks up the row; the draft test enables
  `spec_type=draft-mtp`) — test names unchanged.
- **DoD of the step**: label + group + a field's description live in one place;
  `field_description` removed. **808 tests green** (count unchanged — a refactor), 26 `#[ignore]`, clippy
  `-D warnings`/fmt clean. Steps 3.2 (value access via `field_spec`) and 3.3
  (Choice options) — next per the plan.

### Post-M9: SOLID refactor — stage 3, steps 3.2/3.3: a field-value access table (`field_spec`) (done)
- **Steps 3.2 (core) + 3.3** of stage 3
  ([docs/history/refactoring-solid.md §5](docs/history/refactoring-solid.md), branch
  `refactor/settings-field-descriptors`): access to a config field's value in
  settings was smeared across **four** `FieldId` match sites (`toggle_field`/`cycle_field`/
  config branches of `apply_text`/`field_num_kind`) + Choice options (`choice_menu`).
  Consolidated into **a single table**. Purely structural, behavior unchanged — a safety
  net of ~134 settings tests.
- **New module `screens/settings/spec.rs`**: `enum Access { Toggle(fn(&mut AppConfig))
  | Text(fn(&mut AppConfig,&str)) | Choice { cycle: fn(&mut AppConfig,i32), options:
  fn(&AppConfig)->(Vec<String>,usize) } }` + `FieldSpec { access, num: Option<NumKind> }`
  + a **single** `field_spec(id) -> Option<FieldSpec>` covering all config fields.
  fn pointers (not closures) — `'static`, no capturing; **mode-based routing**
  (external → `external.*`, cloud → `cloud_mut()`) lives **inside** the setter (which
  has access to the whole `AppConfig`). Per-field parsing semantics (opt/`if let Ok`/
  `parse_opt_num`/the host special case/the dictionary list) live in that field's setter,
  byte-for-byte with the previous arms.
- **Consumers reduced to the table**: `toggle_field` (`Access::Toggle` + `save_config`;
  `PTool` — the previous path), `cycle_field` (`Access::Choice.cycle`; subsections/`S`/`IS`/
  `PSelect` — the previous path), `apply_text` (`Access::Text.set`; `S`/`IS`/profile
  fields — the previous path), `field_num_kind` (`field_spec.num`; `S`/`IS` — `p.num_kind()`),
  `choice_menu` (`Access::Choice.options` — this is **3.3**; `S`/`IS`/`PSelect` —
  the previous path).
- **Scope boundaries** (outside the table, the previous path in apply.rs): sampling
  parameters `S(p)`/`IS(p)` (their own `SamplingParam` descriptor), profile fields (over
  `profiles[idx]`, not `AppConfig`), subsection selectors and `PSelect` (navigation).
  The remaining `match id`s in apply/choice/helpers only serve these out-of-scope
  fields.
- **Step (c) (reset_field/the `•` marker via `get`-comparison) deliberately skipped**:
  `reset_field` and the marker already work generically through `default_fields()`
  (comparing `fields()` values against the default, **no per-field arm**) — there's no
  collapse target there; adding `get` to `Access` for this alone would be extra
  indirection. Catalog builders (label/value/description from 3.1) are untouched.
- **DoD of the stage**: `field_description` (3.1) + the config arms in `toggle_field`/`cycle_field`/
  `apply_text` + `field_num_kind`'s config branch removed; for config values, `FieldId`
  is now handled by **one** structural match (`field_spec`) + catalog builders — instead of
  the previous six. **808 tests green** (count unchanged — a refactor), 26 `#[ignore]`,
  clippy `-D warnings`/fmt clean. Docs: architecture.md §3. **The targeted
  SOLID-improvements track (stages 1–4) is complete.**

### Post-M9: improved tool-call view (code highlighting, console, compact header) (done)
- **Tool cards in the feed used to show raw JSON**: the header `⚒ name({"code":"...\n..."})`
  (multi-line Python code collapsed into a JSON string with `\n` escapes), the result —
  as flat `muted` text. Now arguments/results render meaningfully, with
  code highlighting and colored console output for `python_exec`.
- **A clean presenter** `features/tools/present.rs` (no ratatui, testable):
  `present(name, arguments, result) → ToolPresentation` with a `header_suffix` + `ToolBlock`
  blocks (`Code{lang,text}`/`Console{stdout,stderr,exit}`/`Markdown`/`Plain`).
  Knowledge about tools lives in the tools layer; the `message_feed` widget stays generic and
  just renders the blocks. `arguments` is parsed from a JSON string; on failure — graceful
  degradation to the previous inline view. Rules: `python_exec` → code as a `python`
  block + a console; `fs_write`/`fs_read` → content highlighted by the extension of
  `path`; a large text field (multiline/>100 chars) → a separate block, short scalars → the header
  (`name(value)` / `name(k=v, …)`, truncated to 100 chars); prose tools
  (`web_search`/`fetch_url`/`rag_search`/`note_recall`) → a markdown result. Tool
  names are string literals (a stable wire protocol).
- **A new helper `shared::markdown::highlight_code(code, lang, palette)`** — syntect
  highlighting of a code block **without the enclosing ` ``` `** and without wrapping, using the
  same theme-consistent palette (`build_code_theme`) as fenced blocks; an unrecognized
  language (`resolve_syntax` missed) → lines colored `text` (not reversed — more readable
  for non-strictly-code arguments); a trailing empty line is stripped.
- **Card rendering** (`message_feed::push_tool`/`push_block`/`push_console`/
  `push_gutter_lines`): `Code` — via `highlight_code` on a `│ ` gutter; `Console` —
  stdout colored `text`, **stderr — `error`**, the exit code — `warning`, sections on
  `└ `/`│ `; `Markdown` — via `markdown::render`; `Plain` — as before. The `│`/`└`
  gutters are WGL4-safe (compatibility mode); highlighting degrades to plain
  text. `parse_console` parses `python::format_output`'s output by label
  lines (`stdout:`/`stderr:`/`exit code:`), tolerant of empty lines inside
  sections; a non-our-format output ("(empty output, success)", launch errors) → `Plain`.
- **Fix for the rail indent after a card**: when a tool call is the last element of a
  message (the result = the end of a turn, typical for `python_exec`), the following
  indent used to get only the **railless** inter-message separator, and the colored `▌`
  rail was cut off at the result; for tools followed by more assistant text, the
  indent was railed. Now every card is followed by a railed empty line
  (`ensure_blank_line` after `push_tool`); `build_lines` no longer adds the railless
  separator if the body already ends with an empty line (avoiding a double gap).
  The indent after a card is now the same everywhere (as the last element / followed
  by text / between two calls).
- **Invariants**: FSD (`widgets → features → shared`), `FeedToolCall` unchanged,
  live streaming and reload go through the same presenter. **Tests**: the presenter (17 —
  python/fs/generic/console/truncation/invalid JSON), `highlight_code` (no fences +
  RGB; fallback with no language), card rendering (RGB code highlighting, stderr colored as
  error), the rail indent (`tool_last_in_message_keeps_railed_trailing_blank`). **827 tests
  green** (+19), 26 `#[ignore]`, clippy/fmt clean. Docs: spec §11.3–11.4,
  architecture.md §8.

### Post-M9: OpenAI no longer accepts `temperature`/`top_p` (done)
- **`openai` mode stopped sending and showing `temperature`/`top_p`**: only the
  **GPT 5.4** family accepted them (soon to be retired); **GPT 5.5/5.6** no longer
  have these parameters (a request with them → `400`). Previously both fields were
  part of the strict OpenAI/Gemini shared subset (multi-provider Phase 0).
- **Split the OpenAI and Gemini subsets** (`entities/sampling.rs::
  supported_sampling_fields` — the single source of truth for settings UI, `get_sampling`/
  `set_sampling`, and the `Message.metadata` snapshot): OpenAI = `frequency_penalty`/
  `presence_penalty`/`seed`/`max_tokens`; **Gemini** (the OpenAI-compatible endpoint) —
  the same **plus** `temperature`/`top_p` (the compat layer accepts them, as before);
  Claude — unchanged.
- **Mirrored on the wire** (`shared/api/openai/wire.rs`): the shared `restrict_to_strict`
  (llama.cpp extensions + reasoning) is untouched — `temperature`/`top_p` are stripped in
  the existing `dialect == WireDialect::OpenAi` branch alongside swapping
  `max_tokens → max_completion_tokens`, so the Gemini dialect keeps them.
- **Consequences with no code changes** (all derived from the single source): the
  "Sampling" section of the settings screen in `openai` mode no longer shows both
  fields (`cloud_supported_param`); the `set_sampling` schema no longer offers them, and `get_sampling`
  doesn't output them; `retain_supported` zeroes them out in the message metadata snapshot.
  Values remain saved in `default_sampling`/`impersonation_sampling` and will work on
  a local model/Gemini (like other hidden parameters).
- **Tests**: sampling (the OpenAI/Gemini subsets diverged; `retain_supported`
  drops `temperature` for OpenAI and keeps it for Gemini); wire (the OpenAI dialect strips
  `temperature`/`top_p`; Gemini sends them); introspection (`get_sampling` hides them for
  OpenAI, shows them for Gemini); settings (the "Sampling" section's field filter by
  mode); orchestrator (message metadata in `openai` mode without `temperature`). **827
  tests green** (count unchanged), clippy/fmt clean. Docs: architecture.md §9.

### Post-M9: OpenAI → Responses API ("thoughts", reasoning_effort, verbosity) (done)
- **`openai` mode moved from Chat Completions to the Responses API** (`POST /v1/responses`)
  — research and the decision are in [docs/research/openai-responses-client.md](docs/research/openai-responses-client.md),
  ADR 0004. Motive: reasoning summaries ("thoughts") and `reasoning.effort` for OpenAI
  live **only** in Responses; Chat Completions is legacy for reasoning models. Responses is
  a separate protocol of the same vendor → **a new `EngineBackend` implementation**
  alongside `AnthropicClient` (layers above the engine untouched). Chose **variant A**
  (a transport swap, not a second mode/toggle) — the config doesn't grow, `CloudProvider::OpenAi`
  remains the key for `supported_sampling_fields`. Gemini and External stay on Chat
  Completions (`OpenAiClient`); embeddings too (`/v1/embeddings`, absent in Responses).
- **New module `shared/api/openai/responses/`** (`ResponsesClient` + `wire`): hits
  `{base}/responses` (Bearer key), `wire::build_request` translates `ChatRequest` →
  Responses: system → top-level `instructions`; history → an `input` array of **items**
  (`{type:message}` with a string `content`, `reasoning`, `function_call`,
  `function_call_output` — no `tool` role); `max_tokens`→`max_output_tokens`; `store:false`;
  the function tool is flat (`{type,name,description,parameters,strict:false}` — our schemas aren't
  strict). SSE — event-based (the tag is the `type` field inside `data`, as with Anthropic):
  `response.output_text.delta`→`Text`, `response.reasoning_summary_text.delta`→`Thoughts`,
  `response.output_item.added`(function_call)+`response.function_call_arguments.delta`→
  `ToolCall`, `response.output_item.done`(reasoning with `encrypted_content`)→
  `ThoughtsSignature`, `response.completed`/`incomplete`→`Usage`+`Finished`. The finish
  reason is derived by the client (`saw_tool_call`→`ToolCalls`; `incomplete`→`Length`) —
  Responses doesn't send `finish_reason`. The error body isn't swallowed (like the other
  clients).
- **Reasoning + tool-use round-trip** (stateless `store:false`): with `include:
  ["reasoning.encrypted_content"]`, the reasoning item (`id`+`encrypted_content`) must be
  **resent before its `function_call`** — otherwise a quality regression (OpenAI
  measured ~3% on SWE-bench). This is **the same mechanism** as Anthropic's thinking
  signature (Phase B): `ChatChunk::ThoughtsSignature(String)` → `ThoughtsSignature(ThinkingRef{id,
  signature})` (only OpenAI carries `id`, Anthropic uses `None`), `ThinkingBlock` gained `id`;
  `RoundOutput.thinking_ref` accumulates the id+signature; `build_input` places the reasoning item
  before the `function_call`. Places that ignore the variant (`subagent`/`fetch`/`title`/
  `impersonation`/`tool_loop`) still match `ThoughtsSignature(_)` — untouched.
- **Sampling**: `ReasoningEffort` extended with `Minimal`/`XHigh` (gpt-5.x; the Anthropic
  wire maps them to `low`/`high`); a new field `SamplingConfig.verbosity: Option<Verbosity>`
  (`text.verbosity`, Responses-specific); `reasoning_budget==Some(0)` (impersonation/
  auto-title) → `reasoning.effort:"none"` with no summary (analogous to muting "thoughts").
  `supported_sampling_fields(OpenAi)` = `max_tokens`+`thinking`+`reasoning_effort`+
  `verbosity` (no `temperature`/`top_p`/`seed`/penalties — Responses lacks them). The UI's
  "Sampling" section shows reasoning/verbosity for OpenAI, `set_sampling`/`get_sampling`
  and the `Message.metadata` snapshot draw from the same source. A new `SamplingParam::Verbosity`
  (a Choice field, like Reasoning).
- **`WireDialect::OpenAi` removed** (its only consumer — cloud OpenAI — is now on
  Responses): along with the `max_completion_tokens` field and the OpenAI branch of `restrict_to_strict`.
  `LlamaCpp`+`Gemini` remain (Gemini is strict and keeps temperature/top_p).
- **The "proxy with a key" pattern is closed** (`ExternalSettings.api_key_env`, `#[serde(default)]` → no
  migration): the previous pattern "`openai` mode + a url override as an OpenAI-compatible proxy
  with a key" wouldn't have worked once `openai` moved to Responses — now there's
  External with an optional Bearer key from an env var for this (External previously had no
  key field at all — a standalone gap). Wired into the supervisor (chat/imp/embed external) and the UI
  (an "API key (env, optional)" field in the External group; the setter in `spec.rs` routes
  by mode, like url/model).
- **Tests**: responses-wire (system→instructions, store:false, reasoning/summary/effort/
  verbosity, budget=0→effort:none, tools strict:false, tool-call/result→items,
  the reasoning item before a function_call, thinking with no id → no reasoning item, parsing
  every SSE event); sampling (the OpenAi subset, `retain_supported`); settings (OpenAi
  shows reasoning/verbosity, Gemini/Claude don't; verbosity only for OpenAi);
  introspection (the `set_sampling` schema with minimal/xhigh + verbosity). **835 unit tests
  green** (+8), clippy `-D warnings`/fmt clean. Live `#[ignore]` smokes
  (`responses/client.rs`, `MINDFORK_OPENAI_KEY`): a simple generation, a `Thoughts`
  stream with thinking, a reasoning-item tool-use round trip — run against real OpenAI
  outside CI.
- **Live run (gpt-5.5)**: generation/tool-calling work; but **reasoning summaries
  ("thoughts") didn't arrive**. The cause is server-side, not the client: OpenAI only
  returns `reasoning.summary` **to verified organizations**
  (`platform.openai.com/settings/organization/general`) — for an unverified one the
  summary comes back empty (or a `400` on the very presence of `reasoning.summary`).
  Raw CoT is never returned — only the summary. Two fixes as a result: (1) `summary:"auto"` → **`"detailed"`**
  (some models return an empty summary at `auto` but text at `detailed`; gpt-5.x
  supports it); (2) parsing added for the `response.reasoning_text.delta` event in addition to
  `response.reasoning_summary_text.delta` — both → `ChatChunk::Thoughts` (robustness against
  models that stream reasoning under a different event name). Summaries will appear once
  the organization is verified.
- **Reasoning tokens in the status bar (done)**: `TokenUsage` extended with the
  `reasoning_tokens` field (OpenAI Responses `output_tokens_details.reasoning_tokens`;
  OpenAI-compat/llama.cpp `completion_tokens_details.reasoning_tokens`; Anthropic doesn't
  separate them → `0`, "thoughts" are already in `completion_tokens`). Accumulates across
  agentic-loop rounds (`stream_round` takes `base_reasoning`, `RoundOutput.reasoning_tokens`);
  the `AppEvent::TokenUsage.reasoning: Option<u32>` event (`None` — leave it be, it's only
  known from `usage`); `StatusModel.reasoning` → a "(reasoning N)" annotation next to the
  counter when `>0`. Lets you see the model's "thinking" activity even when the summary
  text is gated by organization verification. **836 tests** (+1: `token_counter_shows_reasoning_when_present`).
- **Groundwork** (docs/research/openai-responses-client.md §5): `cached_tokens` in the
  counter, `prompt_cache_key`, OpenAI's built-in server-side tools (`web_search`/
  `code_interpreter` — conflict with the client-side agentic loop), native Gemini via
  its own protocol (next track).

### Post-M9: native Gemini client (generateContent) — Phase A (done)
- **`gemini` mode moved from OpenAI-compatible Chat Completions to native
  `generateContent`/`streamGenerateContent`** — research and the plan are in
  [docs/research/gemini-native-client.md](docs/research/gemini-native-client.md), ADR 0004.
  Motive: the compat path (`OpenAiClient`+`WireDialect::Gemini`) stripped reasoning under
  the strict dialect — there was no "thoughts" or depth control at all (even `top_k`,
  which Gemini accepts natively, was cut). The native API is **a new `EngineBackend`
  implementation** alongside `AnthropicClient`/`ResponsesClient` (layers above the
  engine untouched). **Phase A** — the core (thoughts + reasoning, no signatures); **Phase B**
  (Gemini 3 thought signatures at tool-use) — the next step.
- **New module `shared/api/gemini/`** (`GeminiClient` + `wire`): hits
  `…/v1beta/models/{model}:streamGenerateContent?alt=sse` (the `x-goog-api-key` header),
  `wire::build_request` translates `ChatRequest` → Gemini: system → top-level
  `systemInstruction:{parts:[{text}]}`; history → `contents:[{role:"user"|"model",
  parts}]` (no `system`/`tool` roles — system is top-level, a tool result → a
  `functionResponse:{name,response:{result}}` part with `role:"user"`; adjacent items of the
  same role are merged); a call → a `{functionCall:{name,args}}` part (`args` is an **object**, not a
  string; Gemini has no `call_id` → the client synthesizes a stable id `"{name}-{index}"`,
  matching the functionResponse by it); `max_tokens`→`generationConfig.maxOutputTokens`;
  reasoning → `thinkingConfig`. SSE parts: `text`→`Text`, `text`+`thought:true`→`Thoughts`,
  `functionCall`→`ToolCall` (args as a whole chunk), `usageMetadata`→`Usage`
  (`thoughtsTokenCount`→`reasoning_tokens`), `finishReason`→`Finished`
  (`MAX_TOKENS`→`Length`; the presence of calls→`ToolCalls` even at `STOP`). The error body
  isn't swallowed (like the other clients). The client has no embeddings — RAG in Gemini mode gets
  them via the OpenAI-compatible `…/v1beta/openai/embeddings` (`OpenAiClient`), as with Anthropic.
- **thinkingConfig + generation inference** (added already in Phase A): `includeThoughts:true`
  when `thinking==Some(true)`; the depth — Gemini 3.x via `thinkingLevel`
  (`minimal/low/medium/high`), Gemini 2.5 via `thinkingBudget` (tokens; mirroring the compat
  table: low→1024/medium→8192/high→24576). The generation is inferred crudely from the model name
  (`gemini-3*`). `reasoning_budget==Some(0)` (impersonation/auto-title) mutes it:
  `thinkingBudget:0` (2.5 — off) / `thinkingLevel:"minimal"` (3.x — can't be fully
  turned off, like OpenAI's `effort:none` isn't available everywhere). Gemini has no `verbosity`.
  Tool schemas are sanitized to an OpenAPI subset (dropping `$schema`/
  `additionalProperties`; Phase C will refine this against live schemas).
- **Sampling**: `supported_sampling_fields(Gemini)` = `temperature`/`top_p`/**`top_k`**/
  `max_tokens`/`seed`/`frequency_penalty`/`presence_penalty` + **reasoning**
  (`thinking`/`reasoning_effort`), no `verbosity`. Differences from the previous compat
  path: **+`top_k`** (accepted natively) **+reasoning**. The single source of truth → the UI's
  "Sampling" section, `get/set_sampling`, and the `Message.metadata` snapshot pick this up automatically.
- **`WireDialect` removed entirely** (orphaned after Gemini's move — it was the only
  remaining strict consumer; `OpenAi` had already moved to Responses): along with
  `is_strict`/`restrict_to_strict` and the `dialect` parameter of `build_chat_request`/
  `OpenAiClient` (`openai/wire.rs` shrank by ~160 lines). `OpenAiClient` remains for
  external/proxy and embeddings, sending sampling as-is (llama.cpp ignores unknown fields).
- **Supervisor**: `cloud_chat_setup`'s `Gemini` branch → `GeminiClient`; a new
  `CloudProvider::chat_base_url()` gives the native `…/v1beta` for chat, while `base_url()`
  (`…/v1beta/openai`) remains for embeddings. Impersonation in Gemini mode automatically
  gets the native client (the same `cloud_chat_setup`).
- **Tests**: wire (system top-level; generationConfig; thinkingBudget for 2.5 /
  thinkingLevel for 3.x / force_off; schema sanitization; functionCall/functionResponse +
  merging; coercion of non-object args; parsing SSE parts — text/thought/call/usage);
  client (finishReason mapping, stripping the `models/` prefix). **848 unit tests green**
  (+13), 3 `#[ignore]` smokes (`MINDFORK_GEMINI_KEY`: generation, a thought stream, one
  tool round), clippy `-D warnings`/fmt clean. A live run against a real key is outside CI.

### Post-M9: native Gemini client — Phase B (thought signatures at tool-use) (done)
- **Gemini 3 thought signatures (`thoughtSignature`) for the tool-use round trip** — a
  unique difference from Anthropic/OpenAI: at Gemini a signature is tied to a
  **specific part** (`functionCall`), not to a whole turn, and it's **mandatory for
  Gemini 3** (otherwise a `400`: "missing thought_signature" on a historical call).
  Hence the existing `ThinkingRef`/`ThinkingBlock` (one per turn) **isn't reused** —
  the signature rides on the call itself. See docs/research/gemini-native-client.md §2.3.
- **Contract**: `ApiToolCall.thought_signature: Option<String>` and
  `ToolCallDelta.thought_signature` (`ApiToolCall` gained `Default`); `ToolCallAccumulator::
  push` accumulates the signature into the right call by `index` (for parallel calls
  Gemini attaches the signature only to the first — the accumulator survives this, the
  rest get `None`). Other backends leave the field unset.
- **Domain (persistence)**: `ToolCallRecord.thought_signature: Option<String>` (`#[serde(default,
  skip_serializing_if=Option::is_none)]` → old chats need no migration; empty doesn't
  clutter JSON). Persistence is **mandatory**: `message_to_api`/`record_to_api` rebuild the
  history on every generation, and without the signature on a historical `functionCall`
  Gemini 3 returns `400`. The signature is opaque/encrypted — safe to store.
- **Wiring**: the Gemini client puts `part.thought_signature` into `ToolCallDelta`; the wire
  `build_contents` resends it as a `functionCall` neighbor (`thoughtSignature`) when
  present; `generation.rs` persists `call.thought_signature` into `ToolCallRecord`;
  `record_to_api` carries it back on replay. Within a single generation the signature travels via
  `out.calls` (the accumulator → `assistant_tool_calls`), between generations — via persistence.
  Round-level `thinking_ref`/`.with_thinking` (Anthropic/OpenAI) is **untouched** — Gemini doesn't
  use it.
- **Decision point "persist vs. current-turn-only signature"** (§7-1): whether Gemini 3
  requires the signature on **all** historical `functionCall`s or just the most recent
  turn — pending a live key. By default we design **with persistence** (safer); if a live
  run shows an in-memory signature (like Anthropic) is enough, persistence can be dropped.
- **Known limitation**: signatures on **text parts** (a pure-reasoning turn with no call)
  aren't persisted — our assistant-text replay doesn't carry them; Gemini 3's hard
  requirement only concerns `functionCall` parts.
- **Tests**: wire (emitting `thoughtSignature` as a `functionCall` neighbor when present; no
  key → no signature); contract (carrying the signature through the accumulator); message (serde:
  `None` default for an old record, round trip, `skip` when empty). **851 unit tests green**
  (+3), + an `#[ignore]` smoke `tool_use_round_trips_signature` (Gemini 3: round 2
  resending the signature with no `400`). clippy `-D warnings`/fmt clean.
- **Live run — GO** (Gemini 3.1 Pro Preview, `MINDFORK_GEMINI_MODEL=gemini-3.1-pro-preview`):
  all 4 gemini smokes green (generation, a "thoughts" stream via `includeThoughts`, one
  tool round, a signature round trip) — `tool_use_round_trips_signature`'s signature
  arrived (`thought_signature present: true`) and the resend went through **without
  a `400`**. The signature mechanism and both phases are confirmed on a live model; decision
  point §7-1 closed by design choice (always resend → the failure mode is unreachable),
  persistence kept.

### Post-M9: native Gemini — Phase C (partial: force-off for 2.5 Pro + surfacing blocks) (done)
- Two targeted correctness fixes following Phases A+B (docs/research/gemini-native-client.md
  §8 Phase C). Full tool-schema sanitization and an explicit UI choice of `thinkingLevel`/
  `thinkingBudget` — deferred (a scan of the tool schemas found them clean: the only
  "exotic" bit is `enum`, which Gemini accepts; no `$ref`/`oneOf`/`nullable`/… present).
- **Force-off clamp on Gemini 2.5 Pro** (`wire::thinking_config`): 2.5 Pro **can't
  disable thoughts** (`thinkingBudget` has a minimum of 128) — the previous `reasoning_budget==0`
  (impersonation/auto-title) sent `thinkingBudget:0` → `400`. Now for 2.5 Pro
  (`is_gemini_25_pro` = the name contains `gemini-2.5-pro`) force-off sends `128` (+
  `includeThoughts:false`); Flash/Flash-Lite still get `0` (there `0` does disable it).
  A narrow bug (only 2.5 Pro + muting), but a concrete one.
- **Surfacing blocks** (`client.rs`): previously a `finishReason` like `SAFETY`/`RECITATION`
  and a blocked prompt reduced to a silent, empty `Stop` — the user saw a silently
  empty turn. Now: (1) `promptFeedback.blockReason` (the request rejected by the filter
  before generation, a new `PromptFeedback` type in wire) and (2) blocking `finishReason`
  values (`is_block_reason`: SAFETY/RECITATION/BLOCKLIST/PROHIBITED_CONTENT/SPII/IMAGE_SAFETY/MALFORMED_FUNCTION_CALL/
  OTHER, except when a tool call is present) → emit a `ChatChunk::Text` note
  ("⚠ Gemini did not produce a response (reason: …)") + a `warn` log, so an empty turn is
  explainable. `Finished(Stop)` (not Error) — a normal turn completion with an explanation
  in the feed.
- **Tests**: wire (force-off for 2.5 Pro → 128; parsing `promptFeedback.blockReason`); client
  (`is_block_reason`/`block_note`). **853 unit tests green** (+2), clippy `-D warnings`/
  fmt clean. **Groundwork (not Phase C)**: thought signatures on text parts (not
  persisted — Gemini 3's hard requirement is only for functionCall); checking the full
  tool registry against Gemini (on a live run — only fix real `400`s).

### Post-M9: markdown-render refinements (LaTeX + writer + feed cache) (done)
- Six focused stages per the plan
  [docs/history/markdown-refinements.md](docs/history/markdown-refinements.md) (branch
  `feat/markdown-refinements`, one commit per stage): event-walker defects, gaps in the
  unicode approximation of LaTeX on real LLM output, false positives of the
  math extension, and no render cache in the feed. Only `shared/markdown/`
  (writer/latex/code/mod) + `widgets/message_feed.rs`; the external surface
  (`render`/`render_with`/`highlight_code`) unchanged. See [ADR 0003](docs/decisions/0003-own-markdown-renderer.md),
  spec §11.4.
- **Stage 1 — writer defects** (`writer.rs`): `$$…$$` no longer double-skips before a
  formula (the first line is placed into the already-open empty paragraph line via
  `push_span`); DisplayMath inside a table cell stays **inside the cell** (lines joined with "; "),
  no longer leaks above the table; an autolink `<url>`/email doesn't duplicate the URL (`LinkType` —
  the link is no longer remembered); a syntect/ansi highlighting error → a flat line, not
  lost content (a shared `highlight_line_or_plain`); `<br>` → a line break (inside a
  cell — a space); a code block's info string (` ```rust,no_run `) resolves the syntax by the first
  token; `---` stretches to the panel width; images print alt text + the URL (a separate
  `image` field).
- **Stage 2 — degrading unknowns gracefully + symbol tables** (`latex.rs`): unrecognized
  brace commands **keep their braces** (`LBRACE`/`RBRACE` sentinels) — `\binom{n}{k}`
  stays readable, `\boxed{x}` stays as-is instead of collapsing into `\binomnk`; fraction
  variants `\dfrac`/`\tfrac`/`\cfrac` → aliases of `\frac`; `\binom{n}{k}→C(n, k)`; `\sqrt[3]{x}→∛(x)`,
  `\sqrt[n]{x}→ⁿ√(x)`; `\left.`/`\right.` — swallow the "invisible" delimiter dot;
  `\overset`/`\underset`/`\stackrel` → the base (second) argument; +text wrappers
  (`texttt`/`emph`/`mbox`/`overbrace`/…); ~50 new symbols (⟨⟩ ⌊⌋ ≅ ⊢ ∖ ↪ variant-
  Greek letters ⋃ †), `\backslash→\`; `normalize_delimiters` — `~~~` fences and an
  unclosed short `` ` `` (per CommonMark it's literal — formulas are normalized
  afterward; an unclosed `` ``` `` — verbatim to the end); a recursion-depth ceiling
  (stack protection).
- **Stage 3 — letter subscripts/superscripts, `\mathbb`, a bracket fallback** (`latex.rs`):
  letter super-/subscripts (`x_i→xᵢ`, `a_n→aₙ`, `x^T→xᵀ`, `\sum_{i=1}^{n}→∑ᵢ₌₁ⁿ` — the
  full available set of Unicode modifiers); an unmappable group keeps its
  grouping (`x^{q+}→x^(q+)`, previously lost the braces); `\mathbb{R}→ℝ`, `\mathcal{L}→ℒ`,
  `\mathfrak{C}→ℭ` (BMP only — supplementary-plane isn't used, terminals render it unevenly).
- **Stage 4 — environments and formula mode** (`latex.rs` + `writer.rs`) — the most
  valuable one for quality (`\begin{aligned}…\end{aligned}` — the main source of "mush"):
  `MathMode` (Inline/Display) + two wrappers `latex_to_unicode`/`_display` (DisplayMath calls
  display); `strip_environments` drops `\begin{…}`/`\end{…}` (+ array/tabular colspecs),
  `\\` → a line break (Display) / "; " (Inline), `&` (alignment) is removed,
  `\label{…}`/`\hline`/`\notag`/… is removed, `\&` → a literal `&`; real source
  line breaks (without `\\`) don't break the formula; `collapse_spaces` (dropping `&`/`\hline`
  doesn't leave double spaces); inline output has no `\n` (a ratatui span). **Behavior
  change**: `\\` is now a line separator, not a literal `\`.
- **Stage 5 — a price heuristic** (`writer.rs`): `$5-$10`/`$5/$7` (pulldown returns
  `InlineMath("5-"/"5/")`) are no longer swallowed by the math extension — content made
  only of digits/signs and ending in a separator (`-`/`–`/`/`) is printed as the literal
  `$…$`; legitimate `$3.14$`/`$2+2$`/`$x^2$`/`$n$` are unaffected.
- **Stage 6 — a feed render cache** (`message_feed.rs`) — the biggest systemic CPU win,
  orthogonal to the renderer: `build_lines` is called on every dirty frame
  (streaming — up to ~20/s, every scroll step) and used to re-run markdown+syntect over
  **the whole** history. A `CachedBlock{fingerprint,lines}` cache keyed by message position; the
  `CacheKey{width,palette,show_thoughts}` key (a change → resets the whole cache);
  the fingerprint (an std hash of all `FeedMessage` fields) is compared per block —
  a streaming/changed one is recomputed, the rest come from the cache; truncating the history
  (`Ctrl+E`/regen) drops the tail; `build_message_block` — a pure function computing one
  message's contribution (the rail + a trailing separator). Golden equivalence between a
  warm cache and a fresh render, checked by tests.
- Three existing tests were deliberately changed (plan §9): `\\` semantics
  (`escaped_backslash_and_brace`), `x^{ab}→xᵃᵇ`, `\mathbb{R}→ℝ`. **888 unit tests
  green** (+~35), 33 `#[ignore]`, clippy `-D warnings`/fmt clean. No live run
  needed (a pure render module with no engine; the feed is an interactive TUI, covered
  by golden tests).

### Post-M9: InputBox refinements (clusters/spellcheck/navigation) (done)
- Six targeted fixes to the `widgets/input_box.rs` widget (+ the shared `shared/wrap.rs`) from
  an audit — correctness with no user-behavior change; branch
  `feat/input-box-refinements`. See spec §11.5–11.6, ADR 0001.
- **(1) Cursor snaps to a cluster boundary at `↑/↓`/`End`** (`col_for_visual`): the
  target visual column could land **between an emoji base and its variation
  selector** (`❤️` = ❤ + U+FE0F, widths 1+1) — the cursor sat in the middle of the
  cluster, and the next `insert`/`backspace` would split it (an orphaned selector — the
  same class of bug already fixed for `←/→`). Now `col_for_visual` snaps down to a
  cluster boundary (a new `wrap::snap_boundary` — the largest boundary `≤ col`); for ASCII the
  snap is a no-op (boundaries are everywhere).
- **(2) A hard break of a long word doesn't cut a cluster** (`wrap::wrap_ranges`): when
  wrapping a word longer than the width, the break point could cut `❤️` or a **flag**
  (a pair of regional indicators) across rows (the second scalar "drifted" to the start
  of the next row). A new `hard_break` shifts the break to a cluster boundary while
  guaranteeing progress (≥1 cluster, otherwise `wrap_ranges` would loop). **Fast path**:
  the snap only kicks in when the character at the break is a VS16 or a regional
  indicator (otherwise `return i` with no boundary scanning) — wrapping long ASCII/Cyrillic
  "words" stays O(n), not O(n²). A shared module → improves `message_feed` for free too.
- **(3) A hard invariant for single-line mode** (`set_single_line`): the contract
  "call before `set_text`" was only documented; enabling it on already-multiline
  content had `render_single_line` take `lines[0]`, and `col` could point past its
  length → **a panic on the slice**. Now the setter, at `on=true`, collapses the lines
  into one via a space and clamps the cursor (all current callers are disciplined —
  this is a defensive path).
- **(4) Aligning `hscroll` to a character boundary** (`render_single_line`): with a wide
  glyph (CJK/emoji) on the left, the slice could start "in the middle" of a character, and
  `hscroll` (in columns) wouldn't match the real width of the hidden prefix — **the cursor
  was drawn a column to the right** of its actual position. After `col_at_width` we round
  `hscroll` to the `display_width` of that prefix (guaranteeing `≤ cursor_vw` → the cursor
  is visible).
- **(5) Syncing spellcheck underlines with edits** (`edit_misspelled`): error ranges
  (`misspelled`, row/character coordinates) used to be updated only externally with a
  ~300ms debounce, so structural edits (a line break, joining lines, deleting a word)
  drew stale ranges on the new text (half of a neighboring word underlined, ranges from
  "someone else's" line after a join). Now the mutators sync
  `misspelled` in place: `edit_misspelled(row, at, removed, inserted)` — ranges
  to the left aren't touched, ranges to the right shift by the delta, ranges that
  intersect the edit are reset (underlines stay put between recheck passes). Joining/
  wrapping lines resets the affected rows and syncs the `Vec`'s length; `set_text`/`insert_str`
  (paste) reset everything — the recheck rebuilds it. This also closed a latent bug:
  `set_text` previously **didn't** reset `misspelled` (stale underlines from the prior chat).
- **(6) `on_key` distinguishes an edit from a move** (`KeyOutcome { Edited, Moved, Ignored }`
  instead of `bool`): the chat screen marked the input "dirty" (spellcheck debounce +
  a disk `SetDraft`) on **any** handled key, including arrows/Home/End — pure
  navigation needlessly woke the recheck and sent a draft command. Now `mark_input_changed`
  is only called on `.edited()` (chat/input, chat_list rename); redrawing on
  cursor movement didn't suffer (the `runtime` loop marks `dirty` on any terminal
  event regardless of intent). A companion method `handled()` — `#[allow(dead_code)]`
  (needed by tests, paired with `edited()`). Three call sites that ignored the result
  (settings apply/search, self_model) are untouched.
- **Tests**: wrap (`snap_boundary`; a hard break keeps ❤️/a flag together); input_box
  (the `col_for_visual`/`↑` snap past the middle of a cluster; `set_single_line` on
  multiline content — joining without a panic; `hscroll` at a character boundary with CJK;
  `edit_misspelled` — right/left shift, reset on an edit inside the word, sync on
  wrapping/joining, cleared on `set_text`/paste; `on_key` returns Edited/Moved/Ignored + the helpers).
  **903 unit tests green** (+15), 33 `#[ignore]`, clippy `-D warnings`/fmt clean.
  A pure widget refinement (no engine).
- **Groundwork** (from the same audit, not included): text selection (Shift+arrows/Ctrl+A +
  copy), undo/redo (generalizing the `Ctrl+K` buffer), mouse click in the field (cursor
  position), `Shift+Enter` on a "bare" unix terminal (kitty keyboard protocol),
  a view-model for the 6-arg `render`, deduplicating `Ctrl+K` across five screens. The widths of ZWJ
  families and flags remain a deliberate boundary (cursor/navigation by clusters are
  correct — only the width diverges; terminals render them differently).

### Post-M9: InputBox — a wrap cache + streaming cluster-boundary search (item 7) (done)
- Eliminating repeated O(n) recomputes and extra allocations per frame/keystroke (item 7
  of the same audit). Behavior unchanged — a micro-optimization; branch
  `feat/input-box-refinements`.
- **(7a) A visual-row cache** (`widgets/input_box.rs`): `visual_rows` (wrapping,
  O(n) over characters) was built 2–3 times per frame — the field height via `content_rows`
  (the screen), `render` itself, `↑/↓`/`Home`/`End` navigation. Now a cache `rows_cache`
  `(width, revision, rows)`: a `revision` counter is bumped by the `touch()` helper in every
  mutator of `lines` (navigation doesn't call it), `rows_cached(width)` only recomputes the
  wrap on a width/content change. Wrapping is now computed **once** per frame (was 2), and holding
  an arrow key computes it **zero** times (the cache survives navigation). The three
  consumers that go on to mutate `self` (render, `move_*`) take a `.to_vec()`
  copy (a cheap memcpy of the result vs. an O(n) wrap); `content_rows` became
  `&mut self` and returns `.len()` from the cache (the caller in `chat/render.rs` switched from
  `match &self.impersonation` to `if let` — the `else` branch needs `&mut self.input`,
  the fields don't overlap). Type aliases `VisualRow`/`RowCache` (otherwise clippy's
  `type_complexity` fires on the nested tuple).
- **(7b) Streaming cluster-boundary search** (`shared/wrap.rs`): `prev_boundary`/
  `next_boundary`/`snap_boundary` used to build **the entire** boundary `Vec` via
  `cluster_boundaries` just to take one neighbor (called on every `←/→`/Backspace).
  Rewritten as a streaming pass over graphemes with early exit (the first boundary `≥/> col` or
  advancing until `≤ col`) — one fewer heap allocation and no tail scan; the semantics unchanged
  (existing tests `snap_boundary_lands_on_cluster_start`/
  `grapheme_boundaries_group_emoji_clusters` stay green). `cluster_boundaries` removed.
- **(7c) A cheap guard for `input_is_command`** (`screens/chat/input.rs`):
  checked every frame and allocated the entire `text()` + parsed it. A command always
  starts with `/` (the first non-whitespace character), so first — a new
  `InputBox::first_non_whitespace()` (a streaming pass over characters, no allocation), and only when
  it returns `Some('/')` is `text()` built and `rag_command::parse` called. For regular input
  (letters/Cyrillic) the full text is no longer built.
- **Tests**: cache invalidation across **all** mutators (`row_cache_invalidates_on_every_
  mutator` — compares `rows_cached` against a fresh `visual_rows`, catches a forgotten
  `touch()`); the invariant "navigation doesn't bump the revision, an edit does";
  `first_non_whitespace`. **906 unit tests green** (+3), 33 `#[ignore]`, clippy
  `-D warnings`/fmt clean. A pure widget optimization (no engine).

### Post-M9: InputBox — text selection (track "selection/undo/mouse", stage A of items 8–10) (done)
- The first of four stages in the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](docs/history/input-selection-undo-mouse.md), stage A);
  branch `feat/input-selection`. **Widget only** `widgets/input_box.rs` (+ tests) —
  selection is now available to **all five** `InputBox` consumers (chat, chat
  rename, settings fields, the self-model editor, search) via the shared `on_key`. Copying/
  cutting to the clipboard is stage B (a consumer side effect); here the widget only
  tracks selection and exposes `selected_text()`.
- **The selection model** — an `anchor: Option<(usize,usize)>` field (the anchor; the cursor —
  the existing `row/col`; the selection `[anchor, cursor]`, normalized in
  `selection_span` lexicographically). Methods: `has_selection`/`selected_text` (pub,
  for stage B — `#[allow(dead_code)]` for now), `set_anchor_if_none`/`clear_selection`/
  `select_all`, `delete_selection() -> bool` (joins multi-line content + syncs
  `misspelled`, item 5), `row_selection` (a row-local range for highlighting).
- **Keys** (`on_key`): `Shift`+navigation grows the selection (sets the anchor before
  moving), plain navigation clears it; `Ctrl+Shift`+`←/→` — by word; `Ctrl+A`
  (layout-independent via `physical_char`) — select all. Navigation was factored into a
  table `navigation(code, ctrl) -> Option<fn(&mut InputBox)>`, so the anchor logic
  applies uniformly to every direction with no duplicated branches.
- **Replacing/deleting a selection — inside the mutators themselves** (not `on_key`): `insert_char`/
  `insert_newline`/`insert_str` begin with `delete_selection()`, `backspace`/`delete`/
  `delete_word_*` begin with `if delete_selection() { return }`. So **paste**,
  `Shift+Enter`, and the emoji popup (`Ctrl+B`) also replace a selection, not just
  typing via `on_key`. `replace_range` (spellcheck suggestion) doesn't touch the
  selection (it targets a specific range).
- **Clearing the selection on an edit — centralized in `touch()`**: any edit to `lines`
  bumps the revision (the item-7 cache) **and** clears `anchor` (after an edit the anchor
  would point to stale coordinates). Navigation doesn't call `touch()` — it manages
  `anchor` itself. `delete_selection` clears `anchor` before `touch` (a double reset is harmless).
- **Invariants preserved**: extending the selection is `KeyOutcome::Moved` (doesn't wake
  the spellcheck debounce/`SetDraft`) and **doesn't bump the revision** (the item-7 wrap
  cache survives selection — the geometry doesn't change); replacing a selection is `Edited`.
  Highlighting — `styled_line` was rewritten (a per-character composition: spellcheck
  underline + **the selection background** `palette.keycap_bg` + command coloring); the
  background works in compat mode too (a color, not a glyph) — theme.rs untouched (`keycap_bg`
  is reused).
- **Tests**: `Shift`+arrow grows/plain clears; `Ctrl+A`/`Ctrl+Shift+→`;
  replacing the selection by typing; `Backspace`/`Delete` delete the whole selection; a multi-line
  delete joins + syncs `misspelled`; `Shift` navigation is `Moved`+revision
  unchanged, an edit is `Edited`+revision grows; the render carries the `keycap_bg`
  background (TestBackend); paste replaces the selection. **915 unit tests green** (+9), 33 `#[ignore]`, clippy
  `-D warnings`/fmt clean. No live run needed (a TUI widget, covered by
  TestBackend); spec §11.5/the help overlay/the status bar will be updated in stage B (copy +
  moving Quit from `Ctrl+C`→`Ctrl+Q`/`F10` — that's when the feature becomes user-complete).

### Post-M9: InputBox — copy/cut + moving Quit (track "selection/undo/mouse", stage B of items 8–10) (done)
- The second stage of the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](docs/history/input-selection-undo-mouse.md), stage B);
  branch `feat/input-clipboard`. Builds on stage A (selection). Decision points resolved
  by the user: `Ctrl+C` copies / `Ctrl+X` cuts; **Quit moves from
  `Ctrl+C` to `Ctrl+Q` + `F10`** (`Ctrl+C` freed up); `Ctrl+K` is replaced by shared
  undo — groundwork for stage C.
- **Moving Quit — across all four screens** (a coordinated edit, step B.0):
  `Ctrl+C → *Intent::Quit` lived in the chat screen (the main match plus the
  impersonation and confirmation-popup branches), the chat list (`widgets/chat_list`), settings
  (`settings/apply`), and the self-model screen (`self_model`). Everywhere it was replaced with `Ctrl+Q` (physical
  `q` via `physical_char`, layout-independent) **plus** `F10` (a second option — in case a
  terminal/DE intercepts `Ctrl+Q`). Hints updated: the status bar's `Ctrl+Q quit`, the help
  overlay and the list/settings help lines — `Ctrl+Q`/`F10`.
- **`Ctrl+Q` compatibility** (verified by reasoning): historically XON (resume) for
  flow-control XON/XOFF, but the application runs in **raw mode** (crossterm's `enable_raw_mode`
  clears `IXON`), so the terminal driver doesn't intercept it — it reaches the
  application (Linux/Windows). `F10` guards against rare interceptions (a multiplexer/DE);
  `F10` opens the emulator's menu in some Linux DEs, but that's dismissible — the keys
  cover for each other.
- **Copy/cut** (`screens/chat/input.rs`): `Ctrl+C` with a selection →
  `ChatIntent::CopyToClipboard(selected)` + clearing the selection (`clear_selection`); with no
  selection — a no-op (not quit). `Ctrl+X` → the same intent + `delete_selection()` +
  `mark_input_changed()`. The widget exposed `has_selection`/`selected_text`/`clear_selection`
  (made `pub`).
- **Side-effect wiring** (`app/runtime`): a new intent
  `ChatIntent::CopyToClipboard(String)` (modeled on `SetMouseCapture` — executed by
  `runtime`, not the orchestrator: the text is already at the UI). Intercepted **in
  `process_input_batch`** (which already has the `arboard` slot; `dispatch` lacks it and `screen`
  is immutable there) → `write_clipboard`; success is silent, a failure (headless Linux with no X11) —
  a `push_error` note in the feed. In `dispatch` — a defensive arm (never reached).
- **Coverage — chat-first** (Fork 6): selection + delete/replace work in all five
  consumers (stage A), but clipboard `Ctrl+C`/`Ctrl+X` are wired only in the chat's
  input box for now. In the list/settings/self-model screens `Ctrl+C` is freed up (a no-op) —
  wiring copy there is groundwork.
- **Docs**: spec §11.5 (explaining selection/copy/moving Quit) and §11.7
  (the key table: `Shift`+navigation/`Ctrl+A`/`Ctrl+C`/`Ctrl+X`; Quit `Ctrl+Q`/`F10`),
  README (the key table) updated; `Ctrl+K` unchanged for now (undo — stage C).
- **Tests**: chat (`Ctrl+C` copies the selection → `CopyToClipboard`, clears the selection,
  text unchanged; `Ctrl+X` cuts → the text shrinks; `Ctrl+C` with no selection — a no-op;
  `Ctrl+Q`/`F10` quit; impersonation/the confirmation popup — quit via `Ctrl+Q`/`F10`);
  chat list/settings/self-model (`Ctrl+Q`/`F10` → Quit, `Ctrl+C` is no longer quit);
  layout (`Ctrl+й` physical Q → quit). The former `ctrl_c_quits*` tests were renamed/
  rewritten. **916 unit tests green** (+1 net; 6 rewritten), 33 `#[ignore]`,
  clippy `-D warnings`/fmt clean. A live run is the track's final manual step.

### Post-M9: InputBox — undo/redo (track "selection/undo/mouse", stage C of items 8–10) (done)
- The third stage of the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](docs/history/input-selection-undo-mouse.md), stage C);
  branch `feat/input-undo`. **Widget only** `widgets/input_box.rs` + consumers
  (replacing `Ctrl+K`). A shared undo model across **all five** `InputBox` consumers.
- **Model — a stack of snapshots** `(lines, cursor)` with coalescing (not operational
  records — simpler and provably correct; the field is short). Fields `undo`/`redo: Vec<Snapshot>`,
  `last_edit_kind: Option<EditKind>` (`Insert`/`Delete`/`Structural`), a ceiling
  `UNDO_CAP=200`. `record_undo(kind)` at the start of every mutator writes a snapshot **before**
  the edit; coalescing: consecutive edits of the same class (except `Structural`) merge
  into a single unit; **a typing run breaks on whitespace** (word-granular — `insert_char` on
  whitespace clears `last_edit_kind`); navigation/selection (the `on_key` nav branch,
  `select_all`) break coalescing → the edit after them is a new unit; any edit
  clears `redo`.
- **`undo()`/`redo()`** (`Ctrl+Z`/`Ctrl+Y` in `on_key`, layout-independent):
  restore a snapshot (a shared `restore` — lines+cursor, clearing selection/highlighting,
  invalidating the item-7 cache), pushing the current state onto the opposite stack. They only
  return `Edited` when something actually changed (otherwise `Moved` — a no-op on an empty stack).
- **Consumer boundaries**: `set_text`/`clear` (a programmatic replacement — loading
  someone else's draft, sending, `restore_input`) **clear the history** (`Ctrl+Z` shouldn't
  resurrect someone else's context). `clear_undoable` (`Ctrl+K`) — writes a snapshot and
  clears the content, **without** touching the history → `Ctrl+Z` brings it back.
- **`Ctrl+K` → a special case of undo** (a user decision): the previous toggle semantics
  (`cleared`/`clear_or_restore`) were **removed**; `Ctrl+K` clears, `Ctrl+Z` restores.
  The consumers (chat/rename/settings/search/self-model) were switched from
  `clear_or_restore()` to `clear_undoable()`; `Ctrl+Z`/`Ctrl+Y` reach `on_key`
  through their `_` branch.
- **Replacing/deleting a selection — a single undo unit**: `delete_selection` was split into a
  public one (`Ctrl+X` cut — writes a snapshot + deletes) and a private `remove_selection`
  (no snapshot) — mutators (`insert_char`/`backspace`/…) write **their own** snapshot and
  call `remove_selection`, so "type/delete over a selection" undoes as a whole. A no-op
  at a boundary (`Backspace` at the start / `Delete` at the end with no selection) — no unit recorded.
- **Invariants preserved**: undo/redo go through `touch()` (invalidating the item-7 cache +
  clearing the selection); with tool delegation (`delete_word_left`→`backspace`), the double
  `record_undo(Delete)` **coalesces** — no extra snapshot.
- **Docs**: spec §11.5/§11.7, README, the help overlay (`Ctrl+Z`/`Ctrl+Y`, `Ctrl+K`).
- **Tests**: a typing run = a single unit + redo; a word-granular break on whitespace; navigation
  breaks coalescing; `insert_str` — its own unit; an edit clears redo; `set_text`
  clears the history; `Ctrl+K`→`Ctrl+Z`; a no-op undo/redo → `Moved`; `UNDO_CAP` evicts;
  undoing a selection replacement. The previous 6 `cleared`/`clear_or_restore` tests were replaced;
  consumer tests (chat_list rename, settings editor) — via `Ctrl+K`→`Ctrl+Z`.
  **920 unit tests green** (+4 net), 33 `#[ignore]`, clippy `-D warnings`/fmt
  clean. A live run — the track's final manual step (stage D — mouse — remains).

### Post-M9: InputBox — mouse in the field (track "selection/undo/mouse", stage D of items 8–10) (done)
- The final stage of the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](docs/history/input-selection-undo-mouse.md), stage D);
  branch `feat/input-mouse` (off `feat/input-undo`). Builds on stage A (dragging grows the
  selection). Left-clicking in the **chat's** input box places the cursor, dragging —
  selects. **Only with mouse capture (`Ctrl+W`)** (otherwise crossterm gets no mouse
  events — native terminal selection takes over); there's no separate toggle, it's the
  same capture used for the wheel.
- **Widget** `widgets/input_box.rs`: a new field `last_area: Option<Rect>` (the inner
  text area of the last render, after the border and the `❯` column) — set in `render`
  before the single/multiline branch (the same `inner` feeds `render_single_line`). A private
  `place_cursor_at(mx,my) -> bool` — **the inverse of wrap layout** (screen
  coordinates → a text position): multiline `vrow = (my−area.y)+scroll`, `vcol =
  mx−area.x`, takes `rows_cached(last_width)` (the item-7 cache!), inside a row the
  logical column comes from `col_for_visual` (which already handles "past the end → the
  end of the row" + a soft-wrap rollback + **snapping to a cluster boundary**, item 1);
  a click below the last row → the end of the text; single-line —
  `col_at_width(hscroll+vcol)`+snap. Public
  `mouse_press` (places the cursor + `anchor = cursor` → starts an empty selection) and
  `mouse_drag` (moves the cursor, `anchor` stays → the selection grows).
- **Invariants preserved**: a click/drag only moves the cursor/selection → `place_cursor_at`
  **doesn't** call `touch()` (the item-7 wrap-cache revision doesn't grow — the geometry doesn't change)
  and **doesn't** wake `mark_input_changed` (no `SetDraft`/spellcheck debounce); it resets
  `last_edit_kind` (a click breaks undo coalescing, like navigation). Redraw is provided by the
  `runtime` loop (`dirty` on any terminal event).
- **Screen** `screens/chat/input.rs::handle_mouse`: alongside the previous `ScrollUp/ScrollDown`,
  added `Down(MouseButton::Left)` → `input.mouse_press` and `Drag(Left)` →
  `input.mouse_drag`, **gated on `impersonation.is_none()`** (during impersonation the
  field is hidden, `last_area` is stale). The previous overlay/help/popup guard is preserved;
  a click outside the input area (into the feed) is a no-op (feed selection — "deferred beyond M3").
  `MouseButton` was added to the `chat/mod.rs` import.
- **Coverage — chat-first** (like stage B): mouse-in-field was wired only for the main
  chat input box (`runtime` only forwards `Event::Mouse` when `active.is_chat()`); other
  `InputBox` consumers (rename/settings/self-model) don't get mouse forwarding — groundwork.
- **Groundwork** (not in stage D): double/triple click (word/line — crossterm doesn't give it
  directly, would need its own time-based detector), autoscroll on dragging to an edge, mouse-in-field
  in non-chat consumers, feed selection.
- **Docs**: spec §11.3 (click/drag — new consumers of `Ctrl+W` capture) and §11.5, README,
  the help overlay (`F1`/`?`), the design plan (all A–D done).
- **Tests**: the widget (`place_cursor_at` — mapping a click to a position, past the end → the end of the
  row, below → the end of the text, snapping to the ❤️ cluster, outside the area/before a render → a no-op;
  `mouse_press`+`mouse_drag` build a selection, a click with no drag is empty, a drag through
  soft wrap follows logical coordinates); the screen (`handle_mouse`: `Down`+`Drag` in
  the field build a selection via `last_area_for_test`; a click into the feed doesn't move the cursor).
  **931 unit tests green** (+11), 33 `#[ignore]`, clippy `-D warnings`/fmt clean.
  A live run (click/drag on a real terminal) — the final manual step for the whole
  track (A–D). **The "selection/undo/mouse" track (stages A–D) is complete.**

### Post-M9: line breaks on unix terminals — the kitty protocol + Alt+Enter (audit item 11) (done)
- Closed **audit item 11** of InputBox (groundwork from docs/history/input-selection-undo-mouse.md §9):
  on a "bare" unix terminal, the legacy encoding sends the **same** CR for both
  `Shift+Enter` and `Enter`, so line breaks in the input box weren't available there at all
  (Windows unaffected — the Console API reports modifiers). A runtime-layer issue, not the widget.
- **Enabling the kitty keyboard protocol** (`app/runtime/mod.rs`, `#[cfg(unix)]` next to
  bracketed paste): if the terminal supports it (`crossterm::terminal::
  supports_keyboard_enhancement()`), we push `PushKeyboardEnhancementFlags(
  DISAMBIGUATE_ESCAPE_CODES)` — the terminal starts reporting modifiers for special keys,
  and `Shift+Enter` becomes **distinguishable** from `Enter` (and `Shift`+arrows — from bare
  arrows, which as a bonus enables keyboard-driven selection from stage A). We pop it
  (`PopKeyboardEnhancementFlags`) on exit and in the panic hook (harmless on an empty stack).
  **The `disambiguate` level was chosen deliberately** — it does NOT affect regular typing or
  a lone `Shift`+character (text arrives as-is): `?` (Shift+/) still comes in as a plain
  `Char('?')`+`NONE`, emoji input, and layout-independent parsing of Ctrl shortcuts
  (`shared::keys::physical_char`, Cyrillic) don't regress (unlike
  `REPORT_ALL_KEYS`/`REPORT_EVENT_TYPES`, which would introduce release events and escaping for
  every key). On Windows the block under `#[cfg(unix)]` doesn't compile (imports are also
  cfg-gated — no unused warning).
- **`Alt+Enter` — a fallback line break** for terminals **without** the protocol: `Alt+Enter`
  arrives as `Enter`+`ALT` via the ESC meta prefix and is recognized even on legacy
  terminals (unlike the indistinguishable `Shift+Enter`). Adopted in **all three**
  multiline fields: chat (`screens/chat/input.rs`), a profile's system message/greeting
  (`screens/settings/apply.rs`, only `editor.multiline`), the self-model editor
  (`screens/self_model.rs`). The match was widened from `(Enter, SHIFT)` to
  `(Enter, m) if m.intersects(SHIFT | ALT)` — a bare `Enter` still sends/
  commits. On Windows `Alt+Enter` is often intercepted by the emulator (full screen) — but
  `Shift+Enter` still works there, so the overlap is harmless.
- **Docs**: spec §11.5 (the protocol + `Alt+Enter`) and §11.7 (the key table), README,
  the help overlay (`F1`/`?`: "Shift+Enter / Alt+Enter — line break"),
  docs/history/input-selection-undo-mouse.md §9 (groundwork closed).
- **Tests**: chat (`shift_and_alt_enter_insert_newline_not_send` — both insert a break,
  a bare Enter still sends the whole multiline input), settings
  (`alt_enter_also_inserts_newline_in_multiline_editor`), self-model
  (`alt_enter_inserts_newline_in_editor`). The wrap cache/selection unaffected.
  **934 unit tests green** (+3), 33 `#[ignore]`, clippy `-D warnings`/fmt clean.
  A live run of the kitty protocol on a real unix terminal (not reproducible on
  Windows) — the final manual step.

### Post-M9: InputBox — API hygiene (audit items 12–15) (done)
- The final part of the InputBox audit: four "API hygiene" items — a clean refactor/
  documentation with no user-behavior change. Branch `feat/input-box-hygiene`.
- **(12) `RenderOpts` instead of six positional arguments + a configurable placeholder**
  (`widgets/input_box.rs`, a precedent — `StatusModel`, SOLID stage 4a): `render(frame,
  area, opts: RenderOpts, palette)` — `RenderOpts { title, focused, command, placeholder }`
  (the palette is separate, like `StatusModel`). The placeholder for an empty, unfocused field
  used to be **hardcoded** ("type a message…") right into the generic widget — semantically
  foreign for settings/rename/search fields (they were only saved by always being
  `focused`, so the placeholder never rendered). A constructor `RenderOpts::focused(title)` —
  the typical case of modal fields (no placeholder/command highlighting); the chat screen passes
  the full literal with computed `focused`/`command` and its own placeholder. All five
  callers (chat/rename/settings editor/search/self-model) updated.
- **(13) `Ctrl+K` no longer duplicated across five screens**: clearing the field (`clear_undoable`,
  returned by `Ctrl+Z`) is already handled by `InputBox` itself in `on_key` (added in stage C
  undo). Removed **five** duplicate branches in consumers (chat `input.rs`, chat_list's
  `on_key_rename`, settings `apply.rs`/`search.rs`, the self-model editor) — `Ctrl+K`
  now falls through to their shared `_` path in `on_key`, where the needed side effects already exist
  (`mark_input_changed`/`spell_dirty` via `.edited()`; `editor.error = None`;
  `search_filter()`). Byte-for-byte behavior; a now-unused `let
  ctrl` in `on_key_rename` was also removed. Only one `Ctrl+K` branch remains — clearing the
  entire self-model with confirmation (`self_model.rs`'s non-editor path) — a different
  semantics, not clearing a field.
- **(14) The two definitions of "word" documented as a known divergence** (`word_left_col`):
  word navigation/deletion (`Ctrl+←/→`, `Ctrl+Backspace/Delete`) go by the
  whitespace/non-whitespace class (punctuation is part of the word), while spellcheck
  segments by its own rules (`features/spellcheck/segment.rs`, punctuation is a separate
  class). A deliberate simplification (as in large editors); no need to reconcile.
- **(15) The ZWJ/flag-width boundary documented** (a module doc in `shared/wrap.rs`):
  `width_at` handles VS16 and the skin-tone modifier, but ZWJ families (`👨‍👩‍👧`) and flags
  are measured by scalars — there's no "correct" answer (terminals render them
  differently). Cursor/navigation go by grapheme clusters (UAX #29) — only the width diverges.
- **Tests**: `placeholder_is_configurable_on_unfocused_empty_field` (the placeholder is drawn
  from `RenderOpts`, not hardcoded); the previous consumer tests for `Ctrl+K`→`Ctrl+Z` (chat_list
  rename, settings editor) stay green through the delegated path. **935 unit tests green**
  (+1), 33 `#[ignore]`, clippy `-D warnings`/fmt clean. A pure hygiene pass (no engine).
  **The InputBox audit (items 1–15) is complete.**

### Post-M9: synchronized output (DEC 2026) — fixing the "jumping cursor" (done)
- **Symptom**: while a response was streaming, the cursor would jump between the input
  box and the token counter in the status bar; while RAG indexing ran in the
  background — between the input box and the banner spinner (at the frame rate,
  ~20/s). Dirty repaint (the earlier cursor-blink fix) doesn't cure this: during
  streaming/animation frames legitimately come often.
- **Cause** (traced through ratatui 0.30.1 / crossterm 0.29 source): the terminal's
  hardware cursor = the write position. ratatui writes the frame diff with the cursor
  **visible** (`apply_buffer_with_cursor`: first `flush()`es the diff, only then
  `show_cursor` +
  `set_cursor_position`), and for `CrosstermBackend` both cursor methods are
  `execute!` (an immediate flush), i.e. the tail of the frame **by construction**
  goes to the terminal as separate writes; a large diff is also chopped up by stdout's
  small buffer (`LineWriter`, ~1 KiB). Windows Terminal renders asynchronously (ConPTY) and
  would show an intermediate state: the cursor at the last cell written by the
  diff. The diff is written top-to-bottom by row, and the status bar is at the bottom of the screen →
  during generation the "last cell" is the token counter; in an RAG animation frame
  only the spinner changes. The observation matches the mechanics exactly.
- **Fix** (`app/runtime/mod.rs::run_loop`): every frame is now wrapped in
  synchronized output — `execute!(BeginSynchronizedUpdate)` (CSI `?2026h`)
  before `terminal.draw`, `execute!(EndSynchronizedUpdate)` (CSI `?2026l`) after.
  The terminal buffers everything in between and applies the frame **atomically** —
  intermediate states (the cursor on the counter/spinner) are physically never shown. A draw
  error is now propagated **after** the mode is lifted (the match arms no longer carry
  `?` — a shared `drawn?` after the ESU), so the terminal doesn't stay in buffering mode;
  `?2026l` is also emitted in the panic hook and on exit (a panic inside `draw` can
  happen between h/l — otherwise the frame would remain frozen until the terminal's timeout;
  DECRST of an unset mode is a
  no-op). Bonus: the full-feed repaint during VS16 scrolling (the `\0` sentinel) also
  became atomic on terminals that support this.
- **Compatibility**: Windows Terminal ≥ 1.18 (2023), kitty/alacritty/wezterm/foot/
  iTerm2/Ghostty/konsole/tmux 3.4+ — support it; conhost (compat mode)
  ignores the unknown private mode — graceful degradation (the jump remains, as
  before, no worse). On Windows the commands are declared ANSI-safe, crossterm's winapi
  fallback — a no-op. **Residual behavior**: ratatui unconditionally sends `show`+`MoveTo` every
  frame, and WT resets the blink phase on every move → during active streaming the
  cursor in the input box looks "solid" (doesn't blink). This is the previous
  behavior minus the jumps; unfixable without bypassing ratatui's draw cycle.
- **No tests, deliberately**: BSU/ESU go straight to real stdout, bypassing
  `TestBackend`; the effect itself is terminal-emulator behavior. Verification — a live run on WT
  (a long streaming response + `/rag add` of a large folder) and a look at conhost
  ("no worse than before"). **935 unit tests green** (count unchanged), 33
  `#[ignore]`, clippy `-D warnings`/fmt clean. Docs: spec §4.4.1, architecture §4.

### Post-M9: horizontal row separators for Markdown tables (done)
- **Tables in the feed gained optional horizontal separators between the
  body rows** (`├───┼───┤`, a "grid" look — easier to track a row across a
  wide table). Setting `interface.table_row_separators`
  (a container `#[serde(default)]` → old `settings.json`s need no migration),
  **off by default** (a compact look — a separator only below the
  header); enabling it gives a "grid" look. A toggle "Table row separators" —
  in the "Appearance" group of the "Interface" section (`FieldId::ITableSeparators`,
  with a description hint; in `field_spec` — a plain Toggle).
- **Wired via `markdown::RenderOpts`** (a precedent — `RenderOpts` on
  `InputBox`): the renderer gained a behavior-flag struct
  `{ soft_break_as_newline, table_row_separators }` — `render_with(input, width,
  palette, opts)` takes it instead of the previous positional soft-break
  bool; a 3-arg `render` facade (default flags) remains for the module's tests
  (`#[allow(dead_code)]` with a comment — the feed's production paths pass the flags
  explicitly). `Writer` carries the flag as a field (like `soft_break_as_newline`);
  `render_table` gained a `row_separators` parameter and inserts a
  `border_line(Mid)` **between** body rows (still `└─┴─┘` at the
  bottom after the last one; clipping a narrow table also clips the separators, the "≤ panel
  width" invariant holds).
- **The feed**: a field `MessageFeed.table_row_separators` (default `false` — mirroring
  the config default) + a setter `set_table_row_separators`; the flag joined the render
  **cache key** (`CacheKey`) — toggling the setting resets the cache and
  redraws the tables. Wired through all three markdown paths of the feed: assistant
  response fragments, user messages (alongside `soft_break_as_newline`),
  markdown blocks inside tool cards. `ChatScreen::set_settings` passes the setting from
  the snapshot (like the palette/confirm flag) — applies on the fly, no restart needed.
- **Tests**: table (a single `├…┤` under the header by default; with the flag —
  separators between rows and not after the last one; a single-row table
  looks the same as without the flag; the width invariant with the flag at 24–80 columns, including clipping);
  message_feed (the setting works + a value change invalidates the cache); settings
  (the toggle is in the "Interface" section, toggling it saves the config, a description
  is present); config (default on, round-trip with it off). Docs: spec §11.4.
  **947 unit tests green** (+6), 33 `#[ignore]`, clippy `-D warnings`/fmt
  clean.

### Post-M9: Python sandbox on Wasmer/WASIX — Phase 1 (sidecar scaffolding) (done)
- **`python_exec` gained two modes** ([docs/research/python-wasmer-sandbox.md](docs/research/python-wasmer-sandbox.md)):
  **Wasmer** (the default) — an isolated WASIX sandbox via a **`wasmer` sidecar**
  (a binary next to the app, not embedded in the exe — the Phase 0 §9.7 decision: embedding V8 in
  a dll would pull LLVM/libclang+a static V8 into our build, whereas killing a sidecar
  process is clean); **Local** — the previous system interpreter. Branch `spike/python-wasmer-sandbox`. User
  decisions: `python_enabled=false` (a master gate, as before), network in the sandbox
  `=true` with a toggle.
- **`shared/sandbox.rs`** — a `SandboxRunner` contract behind a trait (`availability`/`run`;
  `MockSandbox` for tests, the `EngineBackend` pattern) + a real `WasmerSandbox`:
  locates the binary (env `MINDFORK_SANDBOX_WASMER` → `data/sandbox/wasmer[.exe]`), a
  CPython source (env → `data/sandbox/python.webc` → the registry package `python/python`), launches via
  `tokio::process` (`wasmer run --v8 [--net] --volume HOST:GUEST --env … <python> --
  /w/job.py`), captures stdout/stderr, a timeout+`kill_on_drop` (python runs INSIDE
  the wasmer process — in-process V8, killing wasmer itself stops the code). Pure,
  testable `build_wrapper`/`build_args`. **The code wrapper carries a `setsockopt` shim**
  (a Phase 0 finding: WASIX doesn't implement `TCP_NODELAY` → `EINVAL`, while http.client/requests
  always sets it; the shim mutes it → requests/urllib work). The task script lives in a
  temp directory (`JobDir` with auto-cleanup via `Drop`, no `tempfile` runtime
  dependency). `site-packages/` is mounted at `/sp` (PYTHONPATH) when present.
- **`features/tools/python.rs`** — an enum dispatcher over `PythonMode`: the id `python_exec`
  **stays the same** (a stable wire protocol), `description()` varies by mode+network
  (in Wasmer it lists the packages — the model uses it more readily). A shared `format_output_parts`
  serves both paths → the feed's presenter (`present::parse_console`) untouched. A graceful
  "sandbox unavailable" when the binary is missing (the `UnavailableEmbedder` pattern).
- **Config** (`shared/config.rs`): `PythonMode{Wasmer(default)/Local}` (serde lowercase,
  `ALL`/`label`/`cycle` like `FlashAttn`); `ToolSettings += python_mode/
  python_net_enabled(default true)/python_wasm_timeout_secs(default 30)` (all `#[serde(default)]`
  → old `settings.json`s need no migration). `python_enabled` stays `false`,
  `python_path` — Local. `ToolConfig`/`build_registry` pass through the mode/network/timeout +
  `sandbox_dir` (from `Paths::sandbox_dir` = `data/sandbox/`; `JsonStore::sandbox_dir`
  delegates — no edits needed at every `OrchestratorDeps` call site).
- **Settings UI** (the "Tools" section → the "Python" group): a Choice mode field (`TPythonMode`) +
  the `python_enabled` toggle + **mode-driven visibility** (the interpreter path — only
  Local; network+timeout — only Wasmer), via field_spec/catalog (a precedent from managed/
  cloud); description hints. New `FieldId`s: `TPythonMode`/`TPythonNet`/
  `TPythonWasmTimeout`.
- **Tests**: sandbox (`build_wrapper` carries the shim; `build_args` ordering/net/mounting
  HOST:GUEST; `wasmer_in_dir`; availability Ready/Missing; the python fallback to a package);
  python (the Wasmer/Local dispatcher via `MockSandbox`; timeout/unavailability; the output
  format; `description` by mode); config (defaults + `PythonMode` serde/cycle);
  settings (mode-driven visibility of the Python group + cycling the mode). **963 unit tests
  green** (+16), clippy `-D warnings`/fmt clean. A live `#[ignore]` smoke,
  `runs_real_python_in_sandbox`, run against real `wasmer 7.2.0` (Windows):
  `print('hello sandbox')` executed inside the sandbox.
- **Next — Phase 2**: `mindfork sandbox setup` (download `wasmer` + `python.webc` +
  wheels from a lock list: numpy from the wasix index, the requests stack from PyPI), a
  cache of the compiled module, a first-run banner; `#[ignore]` smokes for numpy/requests/
  Cyrillic/timeout. **Phase 3** — resource limits, a "one task" gate, ADR 0005.

### Post-M9: Python sandbox on Wasmer/WASIX — Phase 2 (asset provisioning) (done)
- **`mindfork sandbox setup`** — a clap subcommand (`Sandbox{Setup{--force}}`, like
  `backup`/`restore`): installs the Python sandbox into `data/sandbox/` **out of the box**
  (user's decision — auto-download). Its own tokio runtime (network async outside
  the TUI) + a single-instance guard; progress printed to stdout. Idempotent: existing
  assets are skipped, `--force` re-downloads.
- **`features/sandbox_setup.rs`** — a pure core + a thin network layer. Provisioning via
  a **lock list with exact URL+sha256** (`ARCHIVES`/`WHEELS` — consts in the repo, resilient
  to "latest"): (1) the **`wasmer`** binary — a platform tar.gz from GitHub (by
  `std::env::consts::OS/ARCH`), a streamed download with incremental sha256 (the large
  archive isn't buffered), unpacked with `flate2` (pure Rust)+`tar` into `data/sandbox/wasmer-dist/`;
  (2) **`python.webc`** — via `wasmer` itself (`wasmer package download python/python -o … --wasmer-dir`)
  (home/cache under the sandbox, not `~/.wasmer`); (3) **wheels** — numpy from
  `pythonindex.wasix.org` (a native wasix wheel), the requests stack (requests/urllib3/
  certifi/idna/charset_normalizer) from PyPI (`py3-none-any`) → verify sha256 → unpack
  the zip into `site-packages/` **without pip/host Python** (protection against zip-slip via
  `enclosed_name`, as in `backup`). Pure, testable `archive_for`/`verify_sha256`/`hex_lower`/
  `unpack_wheel`/`extract_targz`.
- **Compilation cache** (`shared/sandbox.rs`): `WasmerSandbox::run` sets the env var
  `WASMER_CACHE_DIR=<dir>/cache` — the first run compiles python.wasm (seconds),
  afterward a warm start from the cache, self-contained.
- **A shared binary resolver** `shared::sandbox::locate_wasmer(dir)` (a direct
  `<dir>/wasmer[.exe]` for a manual install → `wasmer-dist/bin/wasmer[.exe]` after
  setup) — a single source of truth about the layout for the runtime (`WasmerSandbox`) and
  provisioning (`sandbox_setup`).
- **Dependencies**: `flate2` (already a transitive dep; miniz_oxide, no C), `tar`, `sha2` —
  all pure Rust.
- **Deferred to Phase 3** (by scope): a first-run banner (compilation progress
  tool→UI — needs an event pipe, the setup command prints progress to stdout); resource
  limits; a "single task" gate; revisiting the `python_enabled` default (setup is now one
  command).
- **Tests**: sandbox_setup (archive selection by platform; `verify_sha256`/`hex_lower`;
  `unpack_wheel` on a crafted zip; `extract_targz` round-trip on a synthetic tar.gz;
  the lock list covers numpy+the requests stack, all URLs https + sha256 64-hex);
  `locate_wasmer` (direct + wasmer-dist, direct takes priority). **969 unit tests green**
  (+6), **39 `#[ignore]`** (+5 live smokes), clippy `-D warnings`/fmt clean.
- **Run against a real rig** (Windows 11, wasmer 7.2.0): `mindfork sandbox setup`
  downloaded and unpacked everything (wasmer 206MB → `wasmer-dist/bin/wasmer.exe` 90MB, python.webc
  44MB, 6 wheels, all sha256 matched). **8 live smokes green** (`MINDFORK_SANDBOX_DIR`
  pointed at the provisioned dir): numpy 2.3.2 (matmul/sum via dynamic linking of native
  `.so`), requests HTTPS 200 (with network access), requests blocked with no network access, Cyrillic,
  timeout kill (`while True: pass` → "exceeded the time limit"), basic sandbox/local.

### Post-M9: Python sandbox on Wasmer/WASIX — Phase 3 (hardening/polish) (done)
- **Locked in by a decision**: [ADR 0005](docs/decisions/0005-python-sandbox-wasmer.md)
  (the `wasmer`/WASIX sidecar behind `shared/sandbox.rs`; the security/resource posture;
  defaults). Track outcome (Phases 0–3): the Python sandbox works out of the box.
- **A "one task at a time" gate** (`shared/sandbox.rs`): `WasmerSandbox` gained
  an `Arc<Semaphore>` (1 permit); `run` does a `try_acquire` **before** resolve/spawn
  — a concurrent call is rejected immediately ("the sandbox is busy with another task").
  Defense in depth against process/thread leaks and predictable load (in the normal
  agentic loop calls are already sequential). The permit is held for the duration of `run`,
  released on completion/timeout.
- **Warming the compilation cache at install time** (`features/sandbox_setup.rs::warmup`,
  best-effort at the end of `setup`): a single `import numpy` run through a real
  `WasmerSandbox` compiles `python.wasm` (+ numpy's native `.so`) into
  `<dir>/cache`, so **the first real tool call is warm** — no
  multi-second compilation in front of the user. This **replaces** the planned
  "first-run banner" (no tool→UI progress pipe needed): there's no cold start
  on the first call anymore. A warmup failure doesn't fail the install.
- **No RAM limit — deliberately not done**: the `wasmer` CLI has no memory/fuel/
  metering flag (only `--stack-size` and proposal toggles), and OS-level job objects/
  `setrlimit` are unsafe and platform-specific for a small payoff. The sandbox's resource
  posture: **timeout** (CPU) + **wasm32** (~4 GB address space) +
  the **single-task gate**; a hard RAM cap is groundwork (recorded in ADR 0005/spec §13.2).
- **`python_enabled` stays `false`** (a deliberate opt-in): the sandbox removes
  the original cause, but "enabled without assets" is worse than "disabled"; enabling is a step after
  `sandbox setup`. Recorded in ADR 0005.
- **Docs**: ADR 0005; spec §13.2 rewritten for the two modes + the resource posture, §9.3
  (the `python_exec` entry); install.md §4.1 (setup warms the cache); the ADR list in
  CLAUDE.md/architecture.md.
- **Tests**: the gate (`gate_rejects_second_concurrent_task` — holding the permit →
  rejection before spawn; `gate_permit_released_after_run` — the permit is returned).
  **971 unit tests green** (+2), **39 `#[ignore]`**, clippy `-D warnings`/fmt clean.
  Live run: `sandbox setup` warmed the cache (`import numpy`), smokes green.

### Post-M9: Python sandbox — OS-level memory limit (Windows Job Object) (done)
- **The "hard RAM cap" groundwork item from Phase 3 implemented** for Windows (on request). A new
  setting `tools.python_wasm_memory_mb: Option<u64>` (default **None** —
  opt-in): when set, the `wasmer` process is placed into a **Job Object** with
  `JOB_OBJECT_LIMIT_PROCESS_MEMORY`; exceeding it kills the process — **protects the host
  from OOM** on a runaway script.
- **Spike investigation** (scratchpad, a real wasmer): the Job Object limit works as a
  hard backstop; the failure **isn't graceful** (V8 "Fatal out of memory" on stderr, or a
  Python `MemoryError`), but the host is protected. **Baseline ~768 MB** — V8+CPython need
  that much to start (below it, the sandbox won't start), so a meaningful limit is ≥~1024.
  **The job handle is closed right after `AssignProcessToJobObject`** (the limit holds
  while the process is a job member) → the raw HANDLE isn't held across `await`, the future
  stays `Send`. **Not done on Unix**: `RLIMIT_AS` is unreliable with V8 (it reserves
  a large virtual address space, a low limit breaks startup) — there it's timeout + wasm32.
- **Implementation**: `WasmerSandbox::with_memory_limit(Option<u64>)` + `apply_memory_limit`
  (`#[cfg(windows)]` winapi via the `windows-sys` target dep; `#[cfg(not(windows))]` —
  a no-op with a debug log), called right after spawn. Threaded through `ToolSettings` →
  `ToolConfig` → `build_registry`. A UI field "Memory limit (MB, 0=none)" in the Python group
  (Wasmer mode) with a hint (Windows only, minimum ~1024).
- **Tests**: config (default None); UI (the field shown in Wasmer, hidden in Local). Live
  `#[ignore]`+`#[cfg(windows)]` smokes on the provisioned sandbox: `memory_cap_
  stops_runaway` (a 1 GB limit + a 3 GB alloc → failure, host intact) and `memory_cap_allows_
  normal_work` (a 2 GB limit doesn't get in the way) — **both green live**. **971 unit tests
  green**, **41 `#[ignore]`**, clippy `-D warnings`/fmt clean. ADR 0005 / spec §13.2
  updated.

### Post-M9: Python sandbox — pandas in the starter set (done)
- **pandas added to the `sandbox setup` lock list** (groundwork item "more packages";
  rounds out the planned numpy/pandas/requests starter set and makes the tool
  description accurate — it used to promise pandas, which wasn't there). `pandas 2.3.2`
  (a native wasix wheel) + pure PyPI deps (python-dateutil/six/pytz/
  tzdata) added to `WHEELS` (URL+sha256). **Warmup** switched to `import pandas` (pulls in
  numpy too — the heaviest compile path). **Verified live**: pandas imports
  under WASIX (DataFrame + aggregates; the first import takes ~11s to compile the `.so`, then
  it's warm). Smoke `pandas_in_sandbox` (`#[ignore]`) green; the `lockfile_wheels_*` test
  extended. **971 unit tests**, **42 `#[ignore]`**, clippy/fmt clean. The setup
  download grew ~10 MB (pandas ~8.7 MB + pure dep wheels).
- **Deliberately NOT added**: matplotlib (headless output is stuck inside the sandbox —
  useless), scipy (not in the wasix index yet), polars/PyTorch (announced, not yet
  in the index). Minor groundwork items (validating the minimum memory limit in the UI, a run on
  Linux/macOS) — out of the current scope.

### Post-M9: agent-scaffold multilingualism — Tier 1 (profile language) (done)
- **The first tier of the i18n track** (design plan [docs/history/i18n.md](docs/history/i18n.md), branch
  `feat/i18n-core`): the language of the agent's **scaffold** (background-task prompts, the self-model
  scaffold, tool results — text that the *model* reads; axis A) became
  a **profile property** (`Profile.language: Lang`). Motive: a profile with a non-Russian
  system prompt used to get a prompt salad (an English persona + a Russian
  "maintenance protocol" + Russian descriptions of 30+ tools). Axis B (the **interface**
  language, text for humans) is a separate future track, the bundle mechanism is shared. Decision points
  confirmed by the user (see design plan §4): axis A only; built-in ru+en bundles;
  tool descriptions localized per profile (Tier 2); tests — from the bundle + a run across
  all locales.
- **`shared/i18n.rs`** — its own micro-solution with no crates (precedent `calc.rs`/ADR 0003:
  `rust-i18n` — a process-global locale, we need per-profile; `fluent` — overkill):
  `Lang { Ru, En }` (serde lowercase, Default `Ru`, `ALL`), `Locale { t(key), tf(key,
  args) }` with a fallback chain "language → reference (ru) → the key itself" (a prompt doesn't panic on a typo),
  `locale(lang) -> &'static Locale` (a LazyLock over `include_str!` bundles). Bundles —
  `locales/ru.json`+`en.json`, a value is a string **or an array of strings** (joined with one
  space: long `\`-joined prompts are broken into word-wrapped fragments, readable and
  byte-for-byte identical). The ru bundle is extracted from the current strings **verbatim** (ru-profile
  behavior doesn't change).
- **Profile language: chosen at creation, locked once there's data** (user's decision):
  `Profile.language: Lang` (`#[serde(default)]` → old profiles = `Ru`, their data is
  Russian). There's no global agent language (independent of the UI language). **Lock**: switching
  is forbidden once the profile has data (visible chats / a non-empty self-model
  / notes — `Orchestrator::profile_has_data`; RAG documents don't count). A double
  gate: (1) the "Scaffold language" field in the profile's Persona subsection renders locked
  (`AppEvent::Settings.language_locked: Vec<Uuid>`, computed by the orchestrator — the screen has no
  DB access); (2) an authoritative check in `handle_update_profile` (a switch with data present
  is rejected). **Bootstrap nuance**: the default profile is created together with the default
  chat → immediately locked to `Ru`; for another language, create a **new** profile and set the
  language before the first chat (the main flow for non-Russian agents). Profile language governs
  **only the scaffold** — the app doesn't force the instruction "respond in language X" (that's
  the system message's job; a profile can be for language learning).
- **Resolution and wiring**: `Orchestrator::profile_locale(profile_id) -> &'static Locale`;
  `TurnInfo.lang` → `ToolContext.loc` (a single point — `ToolContext::new`, SOLID
  §3). **The hot core translated**: auto-titling (`title_system_message(loc)` + the digest's
  roles `build_conversation_digest(msgs, loc)`), impersonation (the default system message
  + the continuation postscript), consolidation/reflection (system messages +
  `behavior_markers(chat, since, loc)`), `POLICY_CORE`→`policy_core(loc)` and
  `maintenance_protocol(loc)` (a single source for the rules), `inject_self_model(..., loc)`,
  the self-model entity's scaffold (`render_for_prompt`/`render_full`/`age_label`/
  `fold_closed_goals`/`summary_fill_hint` — `&Locale` as a parameter; `entities → shared`
  per FSD), new-entity defaults (`default_profile(lang)`, the "new chat" title from
  the bundle). Self-model rendering was already localized in Tier 1 (via `ctx.loc`), though
  tool descriptions/schemas are Tier 2.
- **Tests — from the bundle + across all locales** (user's decision): i18n gates (key-set
  and placeholder parity; `en_bundle_has_no_cyrillic` — a direct proxy for the go criterion);
  per-locale structural tests (`render_localized_for_all_langs`, `age_label_/digest_roles_/
  reflect_system_message_/maintenance_protocol_localized_for_all_langs` — render under
  every `Lang::ALL`, checking headers from that language's bundle + no
  unsubstituted `{…}`); the existing Russian assertions moved to the reference
  `ru()` locale (pin the ru bundle, catch translation corruption). **982 unit tests green**
  (+11), **43 `#[ignore]`** (+1 live `i18n_en_profile_title_e2e_live`: an en profile →
  an English auto-title with no Cyrillic), clippy `-D warnings`/fmt clean.
- **Known limitations (Tier 2)**: tool descriptions/schemas/results are still
  Russian (an en profile = an en scaffold + ru tools, a documented temporary
  state); the en reflection prompt refers to English block names, which themselves
  (`notes/overview.rs`) stay Russian until Tier 2 translates them (a temporary block-name
  mismatch); `fetch.rs` (the summarization prompt) — Tier 2.
- **Live run — GO** (Gemma 4 31B q4 + bge-m3, `llama-server`): `i18n_en_profile_
  title_e2e_live` — an en profile produced the English auto-title "Moon Drifting From Earth"
  (no Cyrillic); the ru regression stayed green — `self_model_gate_e2e_live` (the
  `add_insight` gate surfaced a similar observation in Russian, the model resolved it via `note_merge`) and
  `auto_reflect_e2e_live` (reflection recorded `user_model`). The ru bundle confirmed
  byte-for-byte against a live model.
- **Next**: Tier 2 — `Tool::description(loc)`/`parameters(loc)`/`schemas_for(…, loc)`
  + translation by group (notes+self_model → rag+web+fetch → introspection+fs+python+…);
  Tier 3 — external `data/locales/*.json`; axis B (UI) — a separate track.

### Post-M9: default language for new profiles in `defaults.json` (done)
- **`location.json` renamed to `defaults.json`** (`shared/paths.rs`) and extended with a field
  `default_language` — the **agent-scaffold** language (axis A) in which the first
  (bootstrap) profile and new profiles (`CreateProfile`) are created. Motive (user request):
  always hardcoding a new profile to `Ru` was bad practice; the language should be set by the
  installer. Once an installer exists, it will fill in `default_language` from the user's choice, and
  the first profile will be created in the right language (eliminates the "bootstrap profile
  always Russian" issue from Tier 1).
- **Format** (`Defaults` = `DataLocation` + `default_language`): `#[serde(flatten)]` on
  the storage mode → flat JSON (`{"mode":"path","path":"…","default_language":"en"}`),
  **byte-compatible** with the old `location.json` (`{"mode":…}` still reads, language = default).
  **Backward compatibility**: no `defaults.json` → falls back to the old `location.json`
  (storage mode only); neither present → portable + `ru`. A corrupt JSON — a startup error
  (as before). `Defaults::read(exe_dir)` — the single read point; the old
  `DataLocation::read` removed (tests moved to `Defaults::read`).
- **Wiring**: `Paths` carries `default_language` (`Paths::default_language()`); `main.rs`
  threads it into `OrchestratorDeps.default_language` → `Orchestrator.default_language`
  → `bootstrap` (`default_profile(self.default_language)`) and `handle_create_profile`
  (a new profile's `.language = default_language`, editable in settings while there's
  no data yet). `backup.rs` excludes both `defaults.json` and the legacy `location.json` (install
  files, not user data — they're already outside the include whitelist).
- **Tests** (`paths.rs`): `Defaults::read` — mode+language from `defaults.json`; absence →
  portable+`ru`; **a fallback** to the legacy `location.json`; `defaults.json` takes priority over
  legacy; corrupt → an error; `#[serde(flatten)]` round-trip of the modes; `with_root`
  default `ru`; the backup test excludes `defaults.json`; OrchestratorDeps test fixtures carry
  `default_language`. **988 unit tests green** (+6), clippy `-D warnings`/fmt clean.
  A pure install-config refactor — no live run required.

### Post-M9: i18n Tier 2 — tool localization infrastructure (done)
- **The `Tool` trait gained the scaffold language** (branch `feat/i18n-tools`, docs/history/i18n.md Tier 2):
  `description(&self, loc)`/`parameters(&self, loc)`/`schema(&self, loc)`;
  `ToolRegistry::schemas_for(enabled, loc)`; call sites (generation/reflection/
  consolidation) pass the profile's language (`i18n::locale(profile_lang)`). All 35 impls
  added the parameter — for now as `_loc` (still returning Russian text regardless of the language);
  translation by group is the next step. **A mechanical refactor, no behavior change**
  (an en profile still gets Russian tool descriptions/results — Tier 2
  will translate them by group). **988 unit tests green**, clippy `-D warnings`/fmt clean.
- **Next step (not started at the infra point)**: translation by group (2a → 2b → 2c).

### Post-M9: i18n Tier 2 — group 2a: translate the notes + self_model tools (done)
- **14 tools in group 2a translated** (branch `feat/i18n-tools-notes`, docs/history/i18n.md
  Tier 2): notes — `note_save`/`recall`/`revise`/`supersede`/`merge`/`link`/`neighbors`/
  `consolidate_notes`/`cite_source`; self_model — `get_self_model`/`reflect`/
  `update_self_model`/`update_user_model`/`add_insight`. Descriptions, JSON schemas (field
  descriptions), result text, **gates** (`note_save`/`add_insight`/related traits),
  **blocks** ("Related notes"/"Observation links"/"Source citations"), **consolidation
  overviews** (user-facing + `@self`), the `reflect`/`consolidate_notes` rubrics, and
  all validation errors moved into bundle keys (`locales/ru.json` **byte-for-byte** +
  `locales/en.json`). Key convention `tool.<id>.desc`/`.param.<field>`/`.result.<what>`/
  `.gate.<what>`, shared ones — `notes.block.*`/`notes.mark.*`/`notes.overview.*`/
  `notes.err.*`/`selfmodel.result.nothing`.
- **`loc` wiring**: `_loc`→`loc` across all impls; the helpers `format_notes`/
  `build_consolidation_overview`/`build_self_consolidation_overview`/`parse_id` gained
  a `&Locale` parameter (they had no `ctx`), the rest read `ctx.loc`. Overview calls in the
  orchestrator (`consolidation.rs`/`reflection.rs`) pass the profile's `locale(lang)`.
  Cross-references (block headers in `overview.rs` ↔ mentions in the
  reflect/consolidation prompts) stay on one profile language; the known light mismatch of
  block names (Tier 1) is preserved as-is in both bundles.
- **ru byte-for-byte → tests unchanged**: the testkit uses `Lang::Ru`, so the
  existing ru assertions keep passing (results are identical to the previous strings).
  Added per-locale tests (§3.5): `note_tool_descriptions_are_localized`/
  `self_model_tool_descriptions_are_localized` (catch a forgotten `_loc`: en≠ru + no
  Cyrillic), `note_recall_result_localized_for_all_langs`/
  `add_insight_result_localized_for_all_langs`. Direct overview calls in `notes/tests.rs`
  moved to the `ru()` locale. The key/placeholder parity + `en_bundle_has_no_cyrillic` gates
  cover completeness. **992 unit tests green** (+4), **44 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt clean.
- **Live run — GO** (Gemma 4 31B q4 + bge-m3, `llama-server`): a new smoke
  `self_model_gate_en_e2e_live` (an en mirror of `self_model_gate_e2e_live`) — on an en profile
  a turn recorded an observation (`add_insight`), a near-duplicate raised the gate
  **in English** ("Similar observations (possible duplicate …)"), the model integrated it via
  `note_supersede` (the result "Note superseded: … → new …" — also English),
  no Cyrillic in the tool results. The ru regression stayed green
  (`self_model_gate_e2e_live` — the gates "Similar observations …" and "Note superseded: …"
  byte-for-byte Russian). Tier 2's go/no-go criterion — **go**.
- **Remaining (next PRs)**: 2b (rag + web + fetch), 2c (introspection + fs + python +
  utils + control + subagent). See [docs/history/i18n.md](docs/history/i18n.md).

### Post-M9: i18n Tier 2 — group 2b: translate the rag + web + fetch tools (done)
- **4 tools in group 2b translated** (branch `feat/i18n-tools-rag-web`, docs/history/i18n.md
  Tier 2): `rag_add`/`rag_search` (`rag.rs`), `web_search` (`web.rs`), `fetch_url`
  (`fetch.rs`). Descriptions, JSON schemas (field descriptions), result text, blocks
  ("Notes citing these sources", "Search results (N):", "Content:"),
  the **`fetch_url` summarization prompt** (system + the task, with/without `focus`), and all errors
  reaching the model moved into bundle keys (`locales/ru.json` **byte-for-byte** +
  `locales/en.json`). The `[about self]` marker in `rag_search` — the shared key `notes.mark.self`
  (from 2a).
- **`loc` wiring**: `_loc`→`loc` across all impls; the methods `web::WebSearch::fetch` and
  `fetch::FetchUrl::fetch_text` gained a `&Locale` parameter — their `anyhow` contexts
  (provider network errors / page-fetch errors) surface to the model in the result on
  full failure, so they're localized too. `summarize_text`/`invoke` take `ctx.loc`.
  The substitution order in `tf` for the summarization task template was chosen so `{text}`
  (the page content) is substituted last — content with literal `{url}`/`{f}` isn't
  re-substituted.
- **Not translated (deliberately)**: tool `ui_label`s (axis B — settings UI chrome) and
  `web.rs` tracing logs (file logs, docs/history/i18n.md §2.3) — remain Russian.
- **Tests**: ru byte-for-byte → the existing ru assertions unchanged (testkit = `Lang::Ru`).
  Added per-locale: `rag_tool_descriptions_are_localized`,
  `web_search_description_is_localized`,
  `fetch_url_description_and_summary_system_localized` (en≠ru + no Cyrillic),
  `rag_add_result_localized_for_all_langs`. The key/placeholder parity +
  `en_bundle_has_no_cyrillic` gates cover completeness. **996 unit tests green** (+4),
  **45 `#[ignore]`** (+1), clippy `-D warnings`/fmt clean.
- **Live run — GO** (Gemma 4 31B q4 + bge-m3, `llama-server`): the smoke `rag_en_e2e_live`
  (an en profile + RAG tools) — the model added a fact (`rag_add` → "Chunks added: 1.")
  and found it (`rag_search` → "Fragments found: 1 …") — **RAG tool results in
  English**, no Cyrillic. The ru regression covered by unit tests
  (`add_then_search_returns_relevant_chunk` and others — byte-for-byte Russian results).
- **Remaining (next PR)**: 2c — introspection + fs + python + calc + datetime +
  subagent + control.

### Post-M9: i18n Tier 2 — group 2c: translate the remaining tools (done)
- **Tier 2's final group** (branch `feat/i18n-tools-rest`): translated
  `get/set_sampling`, `get/set_system_message`, `get_last_user_message_time`
  (`introspection.rs`); `fs_read`/`fs_write`/`fs_list` (`fs.rs`); `python_exec`
  (`python.rs`); `calculate` (`calc.rs`); `current_time` (`datetime.rs`);
  `call_subagent` (`subagent.rs`); `send_followup_message`/`rewrite_current_message`
  (`control.rs`). Descriptions, schemas, results, all validation/network/file-system
  errors, the evaluator's output, and control-tool permission text moved into bundle keys
  (ru byte-for-byte + en). Plus three agentic-loop strings (`generation.rs`: a tool
  disabled / skipped during a rewrite / a tool error — `loop.*`).
- **`loc` wired deep**: where tools had no `ctx` in their helpers, `loc` was
  threaded as a parameter: `fs::FsRoot::resolve`/`arg_path`/`truncate_chars`,
  `python::run_local`/`run_wasmer`/`format_output_parts`/`truncate`,
  `datetime::render`, `introspection::scope_note`/`humanize`,
  **the entire recursive-descent evaluator `calc`** (`eval`/`tokenize`/`Parser`/
  `apply_function`/`constant`), `control::control_permission_text` (became `-> String`,
  also called by the loop in `generation.rs`).
- **The `present.rs::parse_console` snag solved**: python's exit-code label moved to
  the key `python.console.exit` (the localized "exit code:" label); the output
  format is localized (`format_output_parts` takes `loc`), and the parser (`exit_labels()`)
  recognizes the label **across all built-in locales**. The `stdout:`/`stderr:` labels are universal —
  not translated. Tool `ui_label`s (axis B) and the
  `∞`/`-∞` symbols (language-neutral) deliberately stayed Russian.
- **Tests**: ru byte-for-byte → the previous assertions unchanged (shadow wrappers `eval`/
  `format_number` in calc and the `ru()` locale in datetime/python give zero call-site
  churn). Added a **strong shared gate** `all_tool_descriptions_localized_to_en`
  (`mod.rs`): EVERY registry tool's description in en, no Cyrillic, and ≠ ru —
  catches a forgotten `_loc` in any group; `python_console_parses_localized_exit_label`
  (`present.rs`: the parser recognizes the en label). **998 unit tests green** (+2),
  **46 `#[ignore]`** (+1), clippy `-D warnings`/fmt clean.
- **Live run — GO** (Gemma 4 31B q4, `llama-server`): the smoke `utils_en_e2e_live`
  (an en profile) — the model called `current_time`, the result "Local time: 2026-07-13 …\n
  UTC: …" — **an English label, no Cyrillic**. The ru regression covered by unit tests
  (tool labels/errors byte-for-byte Russian).
- **Tier 2 complete**: all 35 tools localized (2a notes+self_model, 2b rag+
  web+fetch, 2c the rest). Remaining in the i18n track: Tier 3 (external `data/locales/
  *.json`) and **axis B** (UI chrome for humans) — separate future tracks.

### Post-M9: interface multilingualism — axis B (UI language) (done)
- **Localizing UI chrome** (text for the **human**) per the plan
  [docs/history/i18n-ui.md](docs/history/i18n-ui.md), branch `feat/i18n-ui`. Continues
  axis A (agent language, docs/history/i18n.md): reuses `shared/i18n` (`ui.*` keys), but
  introduces a **new global setting** `config.interface.language: Lang`,
  **independent** of the agent language (`Profile.language`): a Russian UI + English-speaking
  agents is a legitimate combination. Decision points (design plan §2, confirmed 2026-07-13): feed
  roles → `ui.*` (not `CharacterNames`); pluralization → number-neutral wording
  (a plain `tf`, no grammar); the UI default from `defaults.json` on a fresh install;
  scope — one PR.
- **Wiring mirrors `Palette`** (docs/architecture.md §9): a `&'static Locale` is stored
  as a field on the screens (`ChatScreen`/`ChatListScreen`/`SelfModelScreen`), updated in
  `set_settings`/`set_loc` from `config.interface.language`; the runtime broadcasts to open
  overlay screens as one message (`ActiveScreen::set_theme(palette, loc)`); stateless
  widgets (`status_bar`/`message_feed`/`chat_list`/`emoji_picker`/`profile_list`/
  `impersonation_preview`/popups) take `loc` as a render parameter. The settings screen
  computes `self.loc()` from its own config snapshot. The orchestrator resolves the UI
  locale (`Orchestrator::ui_locale()`) for error/notice text and `chat_export` (`F5`).
  `main.rs`: a fresh install (no `settings.json`) → the UI language from `defaults.json`
  (`paths.default_language()`); an old `settings.json` without the field → `Ru` (serde default).
- **We reuse `i18n::Lang`, no new `UiLanguage`**: the UI language is "which
  bundle to read `ui.*` from"; the axes are distinguished by field naming (`interface.language` vs
  `Profile.language`), not by type. `Locale` gained `lang()` (the feed's cache key) and
  `has_key()` (a gate test).
- **Localized** (~365 live strings, `ui.*` keys in `locales/{ru,en}.json`): the status bar
  (server chips/token count/hotkeys/mouse), the feed (role headers "YOU"/"ASSISTANT"→`ui.feed.*`,
  the "thoughts" pill, the placeholder), the input box/RAG banners/chat notices, popups
  (help `HELP_KEYS`, spelling, confirmation, emoji, profile selection, the impersonation
  preview), the chat list (title/search/hotkeys/sorting), the self-model screen
  (`F3`: section labels/goals/hotkeys), **the settings screen** (~209 keys: sections/groups/
  field labels/hint descriptions/Choice labels/footer/search/chips — delegated
  to a subagent), orchestrator errors (`ui.err.*`, ~38 keys), conversation export
  (`ui.export.*`). **A new field "Interface language"** (Choice) in the "Interface" section of
  settings — applies live (settings re-render in the new language immediately).
- **ru — byte-for-byte**: the `ru.json` key values are verbatim equal to the previous
  literals, so ~hundreds of tests asserting Russian substrings passed **with no assertion changes**
  (the default locale in tests = `Ru`) — only render/constructor calls needed updates for
  the new signatures (the `loc` argument) + one `InterfaceSettings.language` initializer.
- **Tests** (`TestBackend`, no live model required): the gate
  `all_ui_keys_referenced_in_code_exist_in_bundle` (scans the source for `ui.*`
  literals and checks they're in the bundle — directly catches the bug class "key not in bundle →
  a slug instead of text"), per-locale render (`role_headers_localized_for_all_langs` — en
  "YOU"/"ASSISTANT"; the status bar's `localized_for_all_langs`), the settings language field
  (`interface_language_field_cycles_and_relocalizes`), the default `interface.language=Ru`.
  The existing i18n gates (key/placeholder parity, `en_bundle_has_no_cyrillic`) now
  cover all `ui.*` keys too (~378 per bundle). **1002 unit tests green** (+~5),
  46 `#[ignore]`, clippy `-D warnings`/fmt clean.
- **Groundwork item closed** (same PR, see below): (1) tool labels in profile toggles and
  (2) supervisor "reasons" localized. All that remains: axis A's Tier 3 (external
  `data/locales`) and technical engine probe errors (`shared/api/managed.rs` — a provider
  layer, not UI).

### Post-M9: axis B — closing the groundwork item (tool labels + supervisor reasons) (done)
- **Tool labels in the settings profile toggles** (`ToolInfo.ui_label` inline hint +
  `ToolGroup::title()` group headers — lived in `features/tools`, returned Russian `&'static
  str`) localized by **resolving in the settings layer**, without changing the text in
  `features/tools`. A new `Locale::get(&'static self, key) -> Option<&'static str>` —
  a value for an existing key as `&'static` (for **dynamic** keys `ui.tool.
  label.{id}`; `t()` doesn't fit — it ties the lifetime to the key argument; `get` with `&'static
  self` takes the string from the static map). `ToolGroup::i18n_key()` (meta.rs) gives
  a stable ASCII key for the group; `title()` remains a Russian fallback. In catalog.rs:
  `r.group = loc.get(info.group.i18n_key()).unwrap_or(info.group.title())`, hint =
  `loc.get(&format!("ui.tool.label.{}", info.id)).unwrap_or(info.label)`. Bundle: 32
  `ui.tool.label.*` keys (every catalog tool) + 8 `ui.tool.group.*` (ru
  byte-for-byte → the settings tests on Russian hints unchanged; en — translated). **The toggle
  shows the tool's English `id`**, the hint/group are in the UI language. Gate test
  `ui_label_and_group_keys_exist_in_all_bundles` (meta.rs): for every tool in
  `tool_catalog()` and every `ToolGroup::ALL`, a key exists in both languages — catches a forgotten
  **dynamic** key (the generic code scanner can't see `format!`-built keys).
- **Displayed supervisor reasons** (`ServerStatus::Disconnected(reason)` → the chip
  "chat: no connection: {why}") localized. Displayed reasons only occur in
  `cloud_chat_setup` (no model / API key errors); `external`/`managed` give
  `NotConfigured`/`Connecting`, and engine probe errors (`shared/api/managed.rs`) are
  technical, a provider-layer concern, **deliberately left as is**. `resolve_api_key` moved
  to a structured error `ApiKeyError { NoName, Missing(var) }` (no locale — a low-level
  helper; other call sites via `.ok()` unaffected); `cloud_chat_setup(..., loc)`
  formats it from the bundle. UI locale threaded through: the trait `ServerSupervisor::apply_chat`/
  `apply_impersonation` gained `loc: &'static Locale` (as the last param; `apply_embed` not
  touched — its reasons go into logs/unavailable), both impls (`LlamaSupervisor`/`MockSupervisor`)
  + `EngineManager::apply_chat`/`apply_impersonation` + the orchestrator (`self.ui_locale()`
  in `apply_*_settings`/`flush_restarts`). Bundle: 3 `ui.err.server.*` keys (no_model/
  no_api_key_env/env_missing; ru byte-for-byte → the supervisor tests unchanged).
- **Axis B fully localized** (except for technical engine probe errors).
  **1003 unit tests green** (+1 tool gate test), 46 `#[ignore]`, clippy
  `-D warnings`/fmt clean. No live run required (pure UI, TestBackend).

### Post-M9: i18n Tier 3 — external locales + new languages without a rebuild (done)
- **Completing the i18n track** (design plan [docs/history/i18n-external-locales.md](docs/history/i18n-external-locales.md),
  branch `feat/i18n-external-locales`): at startup the app scans
  `data/locales/*.json` and layers them **on top of** the built-in ru/en bundles. A file
  `<code>.json` with a matching code **overrides** the built-in keys (a partial override —
  tuning prompts/UI without a rebuild; closes the deferred review of en wording via a
  file edit), a file with a new code adds a **new language** without a rebuild. The scope
  decision point (docs/history/i18n.md §4.3) was taken in full per the user's decision (2026-07-13).
- **`Lang` → an enum + `Ext(&'static str)`** (`shared/i18n.rs`): the `Ru`/`En`
  variants **kept** (zero churn across ~80 `Lang::Ru`/`Lang::En` call sites), added
  `Ext(code)` for external languages. The type stays `Copy` (`&'static str`), `Eq`/`Hash` are
  by the code's content. An external language's code is **interned**
  (`Mutex<HashSet<&'static str>>` + `Box::leak`, finitely many languages — a precedent from the
  syntect theme cache). Hand-written `Serialize`/`Deserialize` as a code string (`"ru"`/`"en"`/`"de"`;
  existing data round-trips byte-for-byte); `#[derive(Default)]`+`#[default]` on `Ru`.
  `code()`/`from_code()` — code↔variant (interning doesn't depend on the registry →
  a profile with an unknown code deserializes even before `init`).
- **The locale registry**: `BUILTIN` (`LazyLock`, tests/`init`'s base) + `REGISTRY`
  (`OnceLock`, set by `init(dir)`). `init` builds owned maps of the built-ins → `overlay_external`
  merges the external ones in → each map is leaked once via `Box::leak` into a `&'static Locale`. Without `init`
  (tests) — `registry()` returns `BUILTIN` → pure built-in behavior, ~1000 tests
  unaffected. `locale(lang)` — a registry lookup with a fallback to the reference (`ru`);
  `locale_exact` (no fallback) — for `Lang::label` (otherwise an external language without
  `ui.lang.name` would show ru's name). `json_to_map` — lenient (`Err` instead of panicking);
  `overlay_external` validates the name (`[a-z]+`), a broken/unreadable file → `warn`+skip
  (the built-in stays intact — graceful degradation, the difference between external and built-in), merges
  overrides by key / a new language (an `info` log). The key `ui.lang.name` added to both built-in bundles
  (a language's self-name in the selector; the built-in ru/en have a hard fallback).
- **`Lang::ALL` (built-in) vs `Lang::all()` (registry)**: `ALL` kept for completeness
  gate tests and per-locale structural tests (they check the **built-in** bundles; external ones are
  user content, not gated); `all()` (registry-aware,
  deterministic order `Ru`,`En`,`Ext`-by-code) — for the UI language selectors (axis A
  `PLanguage`, axis B `ILanguage`: `spec`/`apply`/`choice`/`cycle_lang`) and
  `present.rs::exit_labels` (recognizing the `python.console.exit` label across all languages,
  including ones added externally).
- **Wiring**: `Paths::locales_dir()` = `root/locales` + the directory is created in
  `discover` (for discoverability); `i18n::init(&paths.locales_dir())` in `main.rs` right
  after logging, **before** `Storage::open`; `locales/` in the `backup.rs` include whitelist
  (user overrides go into the backup); `docs/install.md §2.3` on editing/adding a
  language.
- **Tests**: 12 new units in `i18n.rs` — `json_to_map` (string/array/error),
  `overlay_external` (overriding a built-in / a new language / skipping a broken one and a bad name / no
  directory), `sort_langs`, `code`/`from_code` round-trip, serde, the `label` fallback to the code,
  a full compose over a **local** map (overlay→leak→`all_from`/`get`/`t`) +
  `Paths::locales_dir`. The global `REGISTRY` is **not touched** in tests (pure functions
  over a passed-in map — otherwise one test would swap the bundles for the whole binary).
  **1015 unit tests green** (+12), 46 `#[ignore]`, clippy `-D warnings`/fmt clean.
- **Live run** (not against a model — a file mechanism): `data/locales/de.json`
  (3 keys) at real-binary startup → the log `INFO … external locale: new
  language added … lang="de" keys=3` — the `init`→registry integration confirmed on a real
  path. **The i18n track (axis A Tiers 1–3 + axis B) is complete.**

### Post-M9: hardening i18n (single-pass tf, gates, observability, export) (done)
- **Refining the multilingualism mechanism** by review (branch `feat/i18n-hardening`): correctness
  fixes + a wider safety net + external-locale groundwork. Only
  `shared/i18n.rs` (+ `main.rs` CLI, docs); the built-in ru/en behavior is byte-for-byte (the existing
  assertions unchanged). No live run required (a file mechanism/pure functions,
  covered by units).
- **Single-pass `tf` substitution** (`substitute`): an argument's value is substituted
  verbatim and **not re-scanned** — a placeholder inside the value isn't expanded.
  Eliminates the cascading re-substitution class, previously worked around by hand
  (in `fetch.rs` the argument order was chosen so `{text}` — page content that could
  contain `{…}` — went last; the same trap with `{core}` = `policy_core` in
  `selfmodel.maintenance_wrapper`). A `debug_assert` catches code↔bundle drift (a passed-in
  argument that's not in the template → a renamed/forgotten placeholder); compiled away in
  release. A run of the full suite confirmed: no call site sends extra
  arguments.
- **`Lang::label` also falls back on an empty value** (`.filter(!is_empty)`): an external
  override `"ui.lang.name": ""` no longer produces an empty label in the selector.
- **`en_bundle_has_no_cyrillic` now covers `ё`/`Ё`** (U+0451/U+0401, outside the
  the basic Cyrillic letter ranges).
- **The "code key exists in the bundle" gate now covers the WHOLE of axis A, not just `ui.*`**
  (`all_bundle_key_references_in_code_exist`): a generic scanner for dotted literals +
  a filter over prefixes actually present in the bundle (`tool.`/`selfmodel.`/`notes.`/
  `prompt.`/…). Before, a typo like `loc.t("tool.…")` would silently ride out as
  **a slug in the model's prompt** — the worst failure mode. Two filename
  collisions (`defaults.json`, `python.webc`) — in the whitelist. **A reverse gate**
  `bundle_keys_are_not_dead`: every bundle key is referenced by a literal in the code (a dead
  key = a typo/leftover), except the dynamic family `ui.tool.label.*`
  (assembled via `format!`, covered by `meta.rs`).
- **Observability of `t()`'s silent degradation**: exhausting the fallback chain (no key
  anywhere → a slug reaches the output) is logged as `warn` **once per key** (dedup via
  `Mutex<HashSet<(Lang,String)>>`; the happy path doesn't touch the lock). The only signal
  of this defect for external locales, which have no gates.
- **Substantive validation of external files** (`validate_external_map`, mirroring the
  gate tests' parity check, at runtime): a `warn` on (a) a key absent from the reference
  (a typo → a dead override); (b) a mismatch in the `{placeholder}` set of an
  overridden key vs. the reference (breaks `tf`). Not a rejection — advice (still merges).
- **External language codes per BCP-47** (`is_valid_lang_code`): lowercase letters/digits/hyphen,
  starting with a letter, no double hyphens/trailing hyphen — `de`, `pt-br`, `zh-tw`, `sr-latn`
  (previously just `[a-z]+`).
- **The `Ext` fallback chain via the `_fallback` meta-key** (`Locale.fallback`, extracted
  in the single `Locale::from_map` constructor): a language translated from English sets
  `"_fallback":"en"` — gaps get filled from en, not ru. Chain: own bundle →
  `_fallback` → reference `ru` → the key itself (direct map access, no recursion through `t`).
- **CLI `mindfork locales export <code> -o <file>`**: dumps a bundle template without rebuilding/
  needing the project sources. `ru`/`en` → **a verbatim source dump** (arrays/formatting
  preserved), otherwise → the reference's full key set with that language's values (JSON): for
  a new code — entirely ru as a translation starter. `-o` is required, an existing file
  **isn't overwritten** (a guard). `export_bundle` in `i18n.rs`, the subcommand `Locales`
  in `main.rs` (the registry is already up via `init` before the match).
- **Tests** (`i18n.rs`, +11): single-pass/no-cascade/`unused`-args in
  `substitute`+`tf`; `is_valid_lang_code` (BCP-47/garbage); `from_map` extracts
  `_fallback`; the `_fallback`→en chain takes priority over ru; `label` filters empty; overlay
  accepts `pt-br`, validation isn't fatal; `export` of a built-in = the source, of an unknown
  one = the ru template; two generic key gates. **1028 unit tests green** (+13 net),
  46 `#[ignore]`, clippy `-D warnings`/fmt clean. Docs: install.md §2.3 (export,
  `_fallback`, number-neutrality), architecture.md §3 unchanged (same module).

### Post-M9: i18n CLI — stage 1 (bootstrap: our own parser + all of main.rs's text in bundles) (done)
- **The user's directive (2026-07-14):** move **all** text shown to the user into the translation
  bundles, **including** `println!`/`eprintln!`/`bail!` in
  `main.rs`. Continuing the i18n track (axis A Tiers 1–3 + axis B complete). Design
  plan [docs/history/i18n-cli.md](docs/history/i18n-cli.md), 3 stages; this is **stage 1** (`feat/cli-i18n-bootstrap`).
  Decision points confirmed by the user — per the recommendations, with one amendment: on **total
  uncertainty** about the language (neither `settings.json` nor `defaults.json`, or a corrupt
  `defaults.json`), print in **English** (not the reference ru).
- **Our own micro CLI parser `features/cli.rs`** instead of `clap` (precedent: markdown/i18n/
  calc/InputBox): `clap` doesn't let us localize parse errors, and its derive attributes don't
  accept a locale parameter (would need a process-global locale — the very thing
  we rejected `rust-i18n` for). The surface is small (5 subcommands, a few options). `parse(args,
  &Locale) -> Result<CliCommand, String>` (Err — print-ready localized
  text), `render_help(topic, &Locale)`. Parity with the old clap: `--opt value`/
  `--opt=value`/`-o value`, `-h`/`--help` (globally and after a subcommand), `-V`/
  `--version`, `-c 0..=9` validation, required positionals/options. Subcommand/
  flag names (`backup`, `--output`) **aren't translated** (protocol). **`clap` removed from
  the dependencies.**
- **`main() -> ExitCode` + a "peek" phase before parsing arguments.** `Paths::discover`
  split into `resolve()` (root/language, **without creating directories** — `--help`/`--version`
  don't touch the disk) and `ensure_dirs()` (created only on paths that hold data). CLI language —
  `cli_lang(settings→defaults→En)`: `settings.json`.`interface.language`, else
  `defaults.json`.`default_language` **if the file is present** (`Defaults::marker_present`
  distinguishes "the installer set it" from "a fresh binary"), else **English**. `i18n::init`
  scans external locales **before** parsing (so `--help` respects overrides) and now
  **returns** warnings (the log subscriber isn't up yet) — logged in
  `real_main` after `logging::init`; the early-exit branches (`--help`) discard them.
  `main` prints errors itself (`{localized prefix}: {err:#}`, a single-line
  chain) — the English `Error:`/`Caused by:` from std/anyhow are gone.
- **~90 `cli.*` keys** (ru+en): help (`cli.help.*`), parse errors (`cli.parse.*`),
  command messages (`cli.backup.*`/`cli.restore.*`/`cli.locales.*`/`cli.import.*`),
  single-instance/guard (`cli.instance.*`/`cli.guard.*`), anyhow contexts (`cli.ctx.*`),
  version. ru — fresh text (new keys, no existing assertions). **A structured error,
  formatted at the boundary** (precedent `ApiKeyError`): `instance::InstanceError` —
  its Display is no longer user-facing text, `main`/`acquire_cli_guard` build it
  from the bundle by variant. `logging::init(paths, loc)` localized. `Paths::resolve`'s contexts
  (before the language is known → print in en) moved to **English** — a documented
  boundary (docs/history/i18n-cli.md §7).
- **The safety net widened automatically:** the `cli` prefix landed in
  `bundle_prefixes()`, so the existing gates (`all_bundle_key_references_in_code_exist`,
  `bundle_keys_are_not_dead`, key/placeholder parity, `en_bundle_has_no_cyrillic`) and
  `export_bundle` covered `cli.*` with no test changes.
- **Tests:** the parser (`features/cli.rs`, 13 — all subcommands/option forms/errors/`-c` out of
  range/`--help` after a subcommand; `render_help` with no unsubstituted `{…}` across all
  `Lang::ALL`; a help-column-alignment regression); `main.rs` (2 — `cli_lang`
  settings→defaults→en; `cli_error_line` localized single-line). **1043 unit tests
  green** (+15), 46 `#[ignore]`, clippy `-D warnings`/fmt clean. **No live run
  required** (engine/memory unaffected — pure CLI/i18n). Manual check against the real
  binary: `--help`/`--version`/a parse error in ru (`settings.json`) and in en (no
  configuration → total uncertainty) — text is localized, alignment even,
  exit code 2 on parse errors. Docs: architecture.md §3/§12, docs/history/i18n-cli.md.
- **Next:** stage 2 (`feat/cli-i18n-features`) — thread `loc` through `backup`/
  `sandbox_setup`/`migration` (internal `bail!`/`context` + progress); stage 3
  (`feat/i18n-engine-tail`) — `managed.rs` (probe → status chip) + `shared/sandbox.rs`.

### Post-M9: i18n CLI — stage 2 (localizing backup / sandbox_setup / migration) (done)
- **Stage 2** of the plan [docs/history/i18n-cli.md](docs/history/i18n-cli.md) (`feat/cli-i18n-features`,
  stacked on stage 1): the internal `bail!`/`context`/progress of three CLI features moved into
  the locale bundles. Mechanics — the Tier 2c playbook (threading `loc: &Locale` as a parameter through
  the function tree, ru byte-for-byte with the previous strings → existing tests unchanged,
  only the call sites gained the `ru()` locale). No live run required (engine/memory
  unaffected); manual binary check — a localized restore error in ru and en.
- **`features/backup.rs`** (17 `backup.*` keys): `create_backup`/`restore_backup`
  gained `loc`, threaded through **all** helpers with anyhow contexts (`gather_entries`/
  `collect_dir`/`write_zip`/`validate_archive`/`extract_archive`/`clear_user_data`/
  `remove_file_if_exists`/`remove_dir_if_exists`). Contexts for archive creation/reading
  a directory/packing/unpacking/deletion + `backup.err.unsafe_entry` (zip-slip) — from
  the bundle.
- **`features/sandbox_setup.rs`** (37 `sandbox.setup.*` keys): `setup(dir, opts,
  loc, progress)` — **all progress strings** (Downloading/Unpacking/Warming/Done) and
  error contexts (`ensure_wasmer`/`ensure_python_webc`/`ensure_wheels`/`warmup`/
  `download_to_file`/`download_bytes`/`verify_sha256`/`extract_targz`/`unpack_wheel`/
  `http_client`) localized. The callback signature `FnMut(&str)` unchanged — the strings
  come through `loc.tf`.
- **`features/migration.rs`** (4 `migration.*` keys): `parse_settings`/`import_dir`
  gained `loc`; the previous **English** contexts (`parsing LameLLaMA Settings.json`
  etc.) now come from the bundle. The receiving profile's name when importing without
  configurations (`«Imported»`) is localized to the CLI language (the language at data-
  creation time; the UUIDv5 id is name-deterministic — idempotency within one
  language is intact).
- **All three `loc` parameters** threaded into `main.rs` too (stage 1's call sites already had
  `loc`). The feature tests gained `fn ru() -> &'static Locale` and `ru()` at call sites.
- **Tests**: +3 per-locale regression tests against a forgotten `loc` (`backup`:
  `corrupt_archive_error_is_localized` — the context "corrupted" + en with no
  Cyrillic; `sandbox_setup`: `sha_mismatch_error_is_localized`; `migration`:
  `parse_settings_error_is_localized`). The existing i18n gates (key/placeholder
  parity, `en_bundle_has_no_cyrillic`, key-exists-in-code, no-dead-keys) covered all 58
  new keys automatically. **1046 unit tests green** (+3), 46 `#[ignore]`,
  clippy `-D warnings`/fmt clean.
- **Next:** stage 3 (`feat/i18n-engine-tail`) — `managed.rs` (probe errors → status chip)
  + `shared/sandbox.rs` (the `python_exec` result — profile language; warmup — UI language).

### Post-M9: i18n CLI — stage 3 (the engine tail: managed.rs + shared/sandbox.rs) (done)
- **The final stage** of the plan [docs/history/i18n-cli.md](docs/history/i18n-cli.md)
  (`feat/i18n-engine-tail`, stacked on stage 2): localized the errors displayed by the
  engine/sandbox layer. **The i18n CLI track is complete** (stages 1–3). Mechanics — the
  Tier 2c playbook (threading `loc`, ru byte-for-byte → existing substring assertions
  intact). No live run required (a pure localization of error text; successful
  execution paths unchanged).
- **`shared/api/managed.rs`** (5 `ui.err.managed.*` keys, **axis B** — status chip):
  preflight checks (`model_not_found`/`draft_not_found`), the `spawn` context, and
  probe errors in `wait_until_ready` (`early_exit`/`timeout`) surface in the supervisor as
  `ServerStatus::Disconnected(msg)` → the server chip. `ServerHandle::launch(cfg, loc)` and
  `wait_until_ready(..., loc)` gained `loc: &'static Locale`; the supervisor threaded the
  UI locale (`managed_chat_setup`/`external_chat_setup`/`spawn_probe` — from
  `apply_chat`/`apply_impersonation`, which already had `loc` from axis B). `ManagedConfig`
  **not** touched (its `Debug` derive doesn't play well with `Locale`) — `loc` goes in as a parameter.
  The embedding server (`apply_embed`, no `loc`, its error only reaches the log) passes the
  **reference** locale (`Lang::default()`) — the text stays log-only.
- **`shared/sandbox.rs`** (7 `sandbox.err.*` keys, **dual audience**):
  `SandboxRunner::availability(loc)` and `run(..., loc)` gained `loc`. The unavailability
  reason (`not_installed`) and launch errors (`busy`/`not_found`/`job_dir`/
  `write_script`/`spawn`/`wait`) used to be embedded by `python_exec` as a raw `{why}`/
  `{e}` inside a localized wrapper — for an en profile the wrapper was English while the
  nested text stayed Russian. Now `python_exec` passes `ctx.loc` (**axis A**, the profile's
  language), and provisioning's warmup (`sandbox_setup::warmup`, stage 2) passes the UI language
  (**axis B**). `MockSandbox` ignores `loc`.
- **Boundaries** (docs/history/i18n-cli.md §7, i18n.md §2.3 updated): **HTTP client wrappers**
  (`shared/api/{openai,anthropic,gemini}`) — not localized (the content is the server's error
  body / an OS string, untranslatable; localizing the wrapper doesn't pay for threading the locale
  through every client). Pre-language contexts in `paths.rs`/`resolve` — English (stage 1).
- **Tests**: +2 per-locale regression tests against a forgotten `loc` (`managed.rs`:
  `launch_error_is_localized` — en "model file not found" with no Cyrillic; `sandbox.rs`:
  `busy_error_is_localized` — en "busy" with no Cyrillic). The existing substring assertions
  (the localized "model file" error is asserted in the supervisor, "busy"/`"wasmer"` in
  sandbox) intact (ru byte-for-byte). The i18n gates covered the 12 new keys automatically.
  Plus **en-profile `python_exec` smokes** (a testkit helper `ctx_with_storage_lang`
  for an en ctx): `en_sandbox_missing_is_localized` (not ignored — sandbox unavailability
  in English with no Russian leaking, no real `wasmer` needed) + two `#[ignore]`
  (`en_sandbox_output_and_exit_label_localized`, `en_sandbox_timeout_localized`).
  **1049 unit tests green** (+3), **48 `#[ignore]`** (+2), clippy `-D warnings`/fmt clean.
- **En-profile live run — GO** (the provisioned sandbox `data/sandbox/`,
  a real `wasmer` 7.2.0 + python.webc): with an en ctx, real execution
  (`print`+`sys.exit(3)` → output + the **English `exit code:` label**; `while True: pass`
  → the English "exceeded the time limit") — **no Cyrillic**; sandbox unavailability
  is explained in English (the wrapper + the nested reason from `sandbox.rs`). The ru regression
  stayed green (`numpy_in_sandbox`/`cyrillic_print_in_sandbox`/`timeout_kills_sandbox` —
  real numpy execution and Russian labels intact). Stage 3's signature change
  for `run`/`availability` didn't break the ru path.

### Post-M9: release engineering — stage 1 (CI pipeline + toolchain pin + license) (done)
- **The first stage of the "release engineering" track** (design plan
  [docs/history/release-engineering.md](docs/history/release-engineering.md), decision points confirmed by the
  user 2026-07-15; branch `feat/ci-pipeline`): versioning, changelog,
  CI, and data-schema versioning/migrations. Stage 1 closes out **CI** — before this,
  the `fmt`/`clippy -D warnings`/`test` gates relied on nothing but agent discipline, and
  **the Linux build wasn't checked at all** (development happens on Windows).
- **`.github/workflows/ci.yml`**: a `lint` job (ubuntu — `cargo fmt --check` +
  `cargo clippy --all-targets -- -D warnings`) and a `test` job (a matrix of
  `ubuntu-latest` + `windows-latest` — `cargo test`). `#[ignore]` smokes
  (engine/network/live model) are silently skipped without the env vars — CI needs no network or a live
  server. `Swatinem/rust-cache` caching, `concurrency` with
  `cancel-in-progress` by ref, triggers `pull_request` + `push:main`.
- **`rust-toolchain.toml`** — pinned to `1.96.0` + `rustfmt`/`clippy` (F4): the
  `-D warnings` gate would suddenly break on every new stable's new lints;
  toolchain upgrades are a deliberate separate PR.
- **`LICENSE`** (MIT, the file was added — the license was declared in `Cargo.toml` but the file
  didn't exist); CI + License badges added to the README header.
- **A platform-portability scan of the tests** (subagent, a very thorough scan of
  `src/`): **0 tests are able to fail Linux CI** — the code was written with Linux in
  mind (sorting for determinism, stripping the verbatim `\\?\` prefix, paired
  `#[cfg(windows)]`/`#[cfg(not(windows))]` variants, CRLF tolerance; spawns
  in tests gated by `cfg!(windows)`, `.exe` suffixes cross-platform via
  constants). The residual risk — a latent Linux-only clippy `-D warnings` hit
  (only checkable by an actual CI run; the key `cfg(not(windows))` branches
  were checked manually — gated correctly). No live engine run required
  (CI infrastructure, engine/memory/tools unaffected).
- Local gates on Windows are green: **1049 unit tests passed, 48 `#[ignore]`**,
  clippy `-D warnings`/fmt clean. Docs: [docs/history/release-engineering.md](docs/history/release-engineering.md).
- **Next:** stage 2 (`feat/versioning-changelog` — bump to `0.9.0`, `CHANGELOG.md`,
  version in help/logs), stages 3–4 (JSON/SQLite migrations, ADR 0006), stage 5
  (release pipeline), optional stage 6 (`cargo-deny`).

### Post-M9: release engineering — stage 2 (version 0.9.0 + CHANGELOG + showing the version) (done)
- **Stage 2** of the "release engineering" track
  ([docs/history/release-engineering.md](docs/history/release-engineering.md), branch
  `feat/versioning-changelog`): app versioning and the changelog.
- **Bump `0.1.0` → `0.9.0`** (`Cargo.toml` + `Cargo.lock`): a signal of "almost 1.0".
  `1.0.0` — once the track is proven in production (CI + migrations + a release
  pipeline), at which point "data survives upgrades" becomes a contractual promise (F1).
- **`CHANGELOG.md`** (Keep a Changelog 1.1, Russian): rubrics Added/Changed/
  Fixed/Removed/**Data** (storage formats and migrations get their own
  rubric)/Security; there's always an `[Unreleased]` section; comparison links to GitHub.
  Initial fill-in — the `[0.9.0]` section = a condensed retrospective of features by
  track (with a link to the CLAUDE.md journal for detailed history; the whole journal
  isn't carried over).
- **Showing the version**: `env!("CARGO_PKG_VERSION")` (compile-time, no
  parameter threading) in the **`F1` help overlay's title** (`⌨ Hotkeys · mindfork-rs
  v0.9.0`; the popup width isn't narrower than the title) and in the **startup log** (`tracing::info!`
  `version = …` next to `root = …`). `--version` already printed `CARGO_PKG_VERSION`.
- **Changelog discipline** codified in the process: a row in **AGENTS.md
  §4** (a user-visible effect / a data-format change → an entry in
  `[Unreleased]`) + a checklist item in the **PR template**.
- Pure infrastructure/UI — **no live engine run required**. Gates green;
  no tests exist for the help title's content (verified). Docs:
  [docs/history/release-engineering.md](docs/history/release-engineering.md).
- **Next:** stage 3 (`feat/json-schema-migrations` — a JSON version/migration framework,
  a downgrade guard, a pre-migration backup, ADR 0006), stage 4 (SQLite `user_version`),
  stage 5 (the `release.yml` release pipeline).

### Post-M9: release engineering — stage 3 (schema versions + JSON migrations) (done)
- **Stage 3** of the "release engineering" track
  ([docs/history/release-engineering.md](docs/history/release-engineering.md) §3.4, F7–F12; branch
  `feat/json-schema-migrations`): versioning the schemas of saved data and a migration
  framework so a binary upgrade never loses data. **ADR 0006**.
- **Per-artifact versions** (F7): `SETTINGS_SCHEMA`/`PROFILES_SCHEMA`/`CHAT_SCHEMA`/
  `DB_SCHEMA` (all = 1) — artifacts change at different rates, one global number
  would force "migrating" untouched files. `SETTINGS_SCHEMA` is pinned to
  `config::SCHEMA_VERSION` by a test (won't drift apart).
- **The framework — `shared/storage/schema.rs`** (pure, Value-level): types `Step`/
  `JsonArtifact`/`Assessment`, version detection is **structural** (no version field → 1,
  so existing files aren't rewritten), `assess`/`apply_steps`. The registry —
  `settings/profiles/chat_artifact()` (all current=1, empty steps; the engine is "in production" with
  empty migrations). **Orchestration — `features/data_migration.rs`** (not in `shared`:
  the pre-migration backup is `features::backup`, and `shared` can't depend on
  `features`, FSD): reads files as `Value` → gates → backup → apply+control-parse+
  atomic write. `main.rs` calls `data_migration::run` before opening storage (TUI +
  the CLI `import`). A deviation from the design doc (there everything lived in `schema.rs` inside
  `Storage::open`) — for FSD and zero churn at `Storage::open`'s test sites; recorded
  in ADR 0006.
- **Downgrade guard** (F10): data from a newer app version (`detect > current`)
  → **the app refuses to start**, with a localized message (not "read it best-effort").
- **Hardened reads** (F11): a corrupt `settings.json`/`profiles.json` → the app refuses to start
  (previously — silent defaults that then overwrote the `.bak`); a corrupt `chats/<id>.json`
  → skipped with a `warn`, the file untouched (previously one corrupt file crashed the
  entire startup — hardcoded in `json.rs::load_chats`).
- **Pre-migration backup** (F9): a non-empty plan → one backup (reusing
  `backup::create_backup` → `backups/pre-migrate-<date>.zip`) before any write; a backup
  failure → the migration doesn't start. Dormant while every schema stays at 1.
- **Bump policy** codified (F12, `schema.rs` + AGENTS.md §4): additive — no
  bump (as before, `#[serde(default)]`); breaking — a bump + a step + a golden fixture +
  a CHANGELOG entry (the "Data" rubric).
- **Localization**: 5 `migrate.err.*` keys (ru+en); migration logs stay Russian (file
  logs, not localized, i18n §2.3). **Tests**: the framework against a synthetic artifact at current=2
  (assess/apply/downgrade/step-fail); the orchestrator against a tempdir (a no-op with no backup,
  downgrade→refusal, a corrupt settings→refusal, a corrupt chat→skip, a live migration+backup+write).
  **1060 unit tests green** (+11), 48 `#[ignore]`, clippy `-D warnings`/fmt clean.
  No live engine run required (storage infrastructure); **a manual run on a copy of
  real data (226+ chats) is a recommended pre-merge step**. Docs: ADR 0006,
  spec §5.2/§12, architecture §7.
- **Next:** stage 4 (`feat/db-schema-migrations` — `PRAGMA user_version` + steps in
  transactions, a shared pre-migrate moment with JSON), stage 5 (the release pipeline).

### Post-M9: release engineering — stage 4 (SQLite schema migrations) (done)
- **Stage 4** of the "release engineering" track
  ([docs/history/release-engineering.md](docs/history/release-engineering.md) §3.4; branch
  `feat/db-schema-migrations`): versioning and migrating the SQLite schema on top of
  stage 3's framework. **ADR 0006** (extended).
- **`PRAGMA user_version`** as the DB schema version (`DB_SCHEMA = 1`). `db/mod.rs::migrate`
  rewritten to be version-aware: (1) **additive DDL** (`CREATE … IF NOT EXISTS`, extracted into
  `baseline_ddl`) runs **every time** — this is the mechanism for adding tables/indexes to
  an existing DB without a bump (the additive policy from F12); (2) a fresh/existing DB
  (`user_version = 0`) gets stamped with baseline version **1** (not a data migration — no backup
  needed); (3) breaking **steps** (`DB_STEPS`, empty for now) are run by the runner
  `apply_db_steps` — **each in its own transaction along with the `user_version` update**,
  rolled back entirely on failure; (4) downgrade (`user_version > DB_SCHEMA`) — a protective `bail`.
- **A shared pre-migrate moment for JSON+SQLite** (no second backup): `data_migration::run`
  peeks at `db::peek_user_version` before opening storage (a downgrade guard with
  a localized message, the `data.db` file) and factors in `db::needs_step_migration` when
  deciding on a **shared** backup; the actual DB migration runs later in `Db::open`. **The SQLite backup
  API isn't needed**: at backup time the DB isn't open yet (quiescent), so zipping its files
  (`data.db`+`-wal`+`-shm`, already in `create_backup`) is consistent. A deviation from the design doc
  (which suggested `rusqlite::backup`) — justified by the quiescent state.
- **Existing DBs** (today at `user_version = 0`) get stamped with 1 on the first run
  (no backup/messages — data untouched). `DB_STEPS` is empty → no real DB migrations/
  backups yet (dormant, like JSON).
- **Tests**: baseline stamps a fresh DB with 1 + idempotency of a repeated `migrate`;
  downgrade → `bail`; `peek_user_version` (no file → 0); `needs_step_migration` (empty);
  `apply_db_steps` — a successful step commits (+a table, version 2) and a failing one **fully
  rolls back** (no table, version unchanged); `run` → refusal when the DB's `user_version`
  is from the future. **1067 unit tests green** (+7), 48 `#[ignore]`, clippy `-D warnings`/fmt
  clean. No live engine run required; **a manual run on a copy of a real `data.db`
  (stamping 0→1) is a recommended pre-merge step**. Docs: ADR 0006, spec §12.2,
  architecture §6/§7.
- **Next:** stage 5 (`feat/release-pipeline` — `release.yml` on a tag, archives+sha256,
  a schema-version manifest in the backup zip, AGENTS.md §6 "Release"), optional stage 6 (`cargo-deny`).

### Post-M9: release engineering — stage 5 (release pipeline + backup manifest) (done)
- **The final stage** of the "release engineering" track
  ([docs/history/release-engineering.md](docs/history/release-engineering.md) §3.5; branch
  `feat/release-pipeline`). The track is complete (stages 1–5; optional stage 6 — `cargo-deny`).
- **`.github/workflows/release.yml`** (trigger — a `v*` tag): the `build` job (a matrix of
  `windows-latest` + **`ubuntu-22.04`** — old glibc 2.35) builds `cargo build
  --release --locked` and uploads the binary as an artifact; the `release` job (ubuntu, `contents:
  write`) downloads both, **packs on Linux** (which has both `tar` and `zip` — avoiding
  Windows shell differences) into `mindfork-rs-vX.Y.Z-x86_64-{windows.zip,linux.tar.gz}`
  (the binary + README/CHANGELOG/LICENSE/install), computes `sha256sums.txt`, extracts
  the notes = the `[X.Y.Z]` section of the CHANGELOG (`awk`), `gh release create --verify-tag`.
- **A schema-version manifest in the backup zip** (`features/backup.rs`): `BackupManifest`
  (`app_version` + `SchemaVersions{settings,profiles,chat,db}` + `created_at`) is written as the
  entry `manifest.json` in every archive (`create_backup`); on unpacking it's **not
  extracted** into the root (it's metadata, not data — `extract_archive` skips it).
  `read_manifest` reads it (None — an old backup). `mindfork restore` warns
  (localized, `backup.warn.newer_manifest`) when `is_newer_than_current` — a copy from
  a newer app version (data intact; the startup downgrade guard still
  protects it).
- **AGENTS.md §6 "Release"**: a release checklist (a release PR bump+CHANGELOG → merge → the
  user tags → `release.yml` → an artifact smoke) + the condition for promoting to `1.0.0`.
- **Tests**: the manifest in the archive + `read_manifest` round-trip; `is_newer_than_current`;
  None for an archive with no manifest; restore doesn't extract the manifest into the root. **1071
  unit tests green** (+4), 48 `#[ignore]`, clippy `-D warnings`/fmt clean. Both workflow
  YAMLs valid. `release.yml` is only checkable by an actual tag (doesn't trigger on a PR)
  — **the first `v0.9.0` tag after merging** will be its live run + an artifact
  smoke. Docs: CHANGELOG, AGENTS §6, install.md.
- **Next (groundwork, out of the track):** the first `v0.9.0` tag; promotion to `1.0.0`; optional
  `cargo-deny` (stage 6); self-update/installers/musl — see release-engineering.md §5.

### Post-M9: release engineering — stage 6 (cargo-deny) + dictionaries in the release (done)
- **Optional stage 6** of the "release engineering" track (F6) + a release
  pipeline tweak; branch `feat/cargo-deny-release-dicts`.
- **`cargo-deny` dependency audit**: `deny.toml` (advisories/licenses/bans/sources)
  + `.github/workflows/audit.yml` (**weekly `cron` + `workflow_dispatch`, NOT on
  `pull_request`** → doesn't block merges, advisory mode). `EmbarkStudios/cargo-deny-action@v2`.
  Run locally (`cargo-deny 0.20.2`): licenses/bans/sources — ok (the license
  allowlist checked against the dependency graph); of the advisories, **`anyhow` (RUSTSEC-2026-0190, an unsound
  `Error::downcast_mut`) — a direct dependency, fixed by bumping `1.0.102 → 1.0.103`**
  (the floor in Cargo.toml raised); 4 unfixable transitive ones via `syntect`
  (yaml-rust/bincode unmaintained, quick-xml ×2 DoS — RUSTSEC-2024-0320/2025-0141/
  2026-0194/2026-0195) added to `ignore` with a rationale (syntect parses **its own
  built-in** syntaxes/themes, not user input → the risk doesn't materialize; revisit on a
  syntect upgrade). Principle: fix what we control, document what we ignore among the
  unfixable transitive ones. `cargo deny check` green.
- **Spellcheck dictionaries in the release**: `release.yml` puts `dictionaries/*.aff`+`*.dic`
  (`en_US`/`en_GB`/`ru_RU`, ~5 MB) into the archive as **`data/dictionaries/`** — a portable
  layout the app reads next to its binary → **spellcheck out of the box** in
  the release too (not just in dev via `build.rs`). Kicks in from the **next**
  tag onward (the published `v0.9.0` doesn't include the dictionaries — re-release if desired).
- **Clarified stale comments**: the dictionaries have **been in the repo for a while**
  (6 files checked in, no `.gitignore` entry) — comments fixed in `build.rs`, CLAUDE.md (the entry
  below), AGENTS.md §6 (archive contents). The old "not part of the repo (.gitignore)"
  comments were stale info.
- Pure infrastructure/docs + a patch-level dependency bump — **no live engine run
  required**. **1071 unit tests green** (anyhow 1.0.103 API-compatible), 48
  `#[ignore]`, clippy `-D warnings`/fmt/`cargo deny check` clean. The `audit.yml` YAML
  valid (doesn't trigger on a PR — runs via `workflow_dispatch`/`cron` after a merge).
- **The "release engineering" track is fully closed** (stages 1–6 + the v0.9.0 release +
  the plan in `docs/history/`).

### Post-M9: installers — research + stage 1 (code prerequisites) (done)
- **A new "installers" track** (the order: Windows msi/exe + Linux deb/rpm/
  pkg.tar.zst; at install time — choosing the interface language and the data
  location, see `defaults.json`; code signing left open). **Research** —
  [docs/history/installers.md](docs/history/installers.md) (three parallel web surveys
  based on primary sources, July 2026): Windows — **Inno Setup 6.7.x (exe)**, not MSI (every
  "special" requirement is a stock `CreateInputOptionPage`/`CreateInputDirPage`/
  `SaveStringsToUTF8File`/the official `Russian.isl`; MSI would be days-to-weeks of workarounds);
  Linux — **nfpm** (one YAML → all three formats), a layout of `/usr/lib/mindfork-rs/` +
  a symlink at `/usr/bin` (on Linux `current_exe` resolves a symlink to the real path — the app's
  path resolution works **with no code changes**); deb/rpm/pacman are non-interactive → on
  Linux the app determines the language from the OS locale. **Code signing**: without signing,
  SmartScreen reputation resets with every release, and even EV no longer gives instant reputation;
  options are — SignPath Foundation (free, public OSS), Certum Open Source
  (~€69/€29, for individuals), Azure Artifact Signing ($9.99/mo, restricted geography).
  **Decision points R1–R9 confirmed by the user 2026-07-16 per the recommendations; signing (R8)
  deferred** — the repo is private, there's no site/logo/icon (revisit when
  preparing for a public launch). Research branch — `docs/installers-research`.
- **Stage 1 "code prerequisites"** (branch `feat/installed-mode-prereqs`) — three additive
  changes preparing the app for an installed (not portable) look; no migrations
  (`#[serde(default)]`), engine/memory unaffected.
  - **P1 — a dictionary fallback next to the binary.** `dict::load` gained a parameter
    `bundled_dir: Option<&Path>`: dictionaries are looked up first in the data root (`dict_dir`),
    then in the portable layout `exe_dir/data/dictionaries` (`Paths::
    bundled_dictionaries_dir`, from a new field `Paths.exe_dir`). A pair already loaded
    from the root **isn't** reloaded from bundled (`loaded: HashSet` by base name —
    a user dictionary of the same name wins); in portable mode bundled ==
    dict_dir → the second pass is a no-op. Before, with `mode=system`/`path`, there were no
    dictionaries in the data root → spellcheck silently disabled; now bundled picks them up
    (where the installer/package puts them). Shared logic factored into `load_dir(dir, selected, loaded,
    dicts)`. Threaded through `SpellLoader`/`runtime::run`/`run_loop` → `main.rs`
    (`paths.bundled_dictionaries_dir()`).
  - **P2 — auto-detecting the language from the OS locale.** `Defaults.default_language:
    Lang` → `Option<Lang>` (`#[serde(default)]` → None when the field is absent; a deb/rpm
    package writes only `{"mode":"system"}`). `Paths::resolve` resolves
    `default_language.unwrap_or_else(i18n::detect_os_language)`; `detect_os_language`
    delegates to the pure `lang_for_locale(Option<&str>)` (the primary subtag `ru*`→Ru, else
    En; testable) over `sys_locale::get_locale()` (a new dependency `sys-locale`,
    cross-platform, pure Rust). Resolved **before** CLI parsing (the peek phase), so the
    language is always concrete → `cli_lang` simplified to `settings.unwrap_or(default_language)`
    (`defaults_present`/`Defaults::marker_present` removed, `resolve() -> Self` with no
    bool). Only changes fresh installs' behavior (previously — always Ru when the field is
    absent). Closes the roadmap groundwork item "Detect language from the OS locale".
  - **P3 — `defaults.json` tolerates a UTF-8 BOM** (`strip_bom` before the whitespace check
    and parsing — precedents `rag_ingest::read_text`, the LameLLaMA importer). An installer/
    editor could write the file with a BOM → the app would crash on startup.
- **Tests**: paths (an Option language with the field present/absent/a legacy fallback; BOM;
  `bundled_dictionaries_dir` None in `with_root`); dict (bundled supplies
  missing ones; a root dictionary wins over bundled with no duplicate); i18n
  (`lang_for_locale` — subtags/None); main (`cli_lang` — settings ∨ resolved).
  **1077 unit tests green** (+6), 48 `#[ignore]`, clippy `-D warnings`/fmt clean.
  **Live run**: engine/memory unaffected; against the real binary confirmed the
  peek phase (`--version`/`--help` localized), **BOM tolerance** (`defaults.json` with a
  BOM + `mode:system` → starts fine), and refusal on a corrupt `defaults.json` (exit code 1).
  A full TUI dictionary smoke is interactive (needs a terminal); the logic is covered
  by the unit test `bundled_dir_supplies_missing_dictionaries`.
- **Next:** stage 2 `feat/linux-packages` (nfpm → deb/rpm/archlinux + a CI install
  smoke), stage 3 `feat/windows-installer` (Inno Setup), optional stage 4 "Signing"
  (deferred). Playbook/layout/DoD — `docs/history/installers.md` §8.

### Post-M9: installers — stage 2 (Linux packages deb/rpm/pkg.tar.zst) (done)
- **Stage 2** of the "installers" track (`docs/history/installers.md` §4, §8; branch
  `feat/linux-packages`, **stacked on `feat/installed-mode-prereqs`** — the packages put
  dictionaries at `/usr/lib/mindfork-rs/data/dictionaries`, found via P1's fallback from
  stage 1). Packaging/CI only — the app's source code untouched (zero increase in
  unit tests, engine/memory unaffected).
- **`packaging/nfpm.yaml`** — one nfpm config → **three formats** (`--packager
  deb|rpm|archlinux`; nfpm handles all three, cargo Arch tooling doesn't cover it).
  Layout §4.2: the real binary at `/usr/lib/mindfork-rs/mindfork-rs` + `defaults.json`
  (`{"mode":"system"}`, `type: config|noreplace` → an edit survives an upgrade) + dictionaries
  at `/usr/lib/mindfork-rs/data/dictionaries/` + a **symlink** at `/usr/bin/mindfork-rs`
  (`type: symlink`) + docs at `/usr/share/doc/mindfork-rs/`. `defaults.json` can't
  go in `/usr/bin` (FHS); the symlink layout works **with no code changes** —
  `current_exe()` on Linux resolves `/proc/self/exe` to the real path, the app
  finds the neighboring `defaults.json`/dictionaries. Dependencies set by hand (nfpm doesn't compute them):
  deb — `libc6 (>= 2.35)`, rpm/arch — nothing (glibc is in the base; the stack is rustls +
  bundled SQLite + x11rb).
- **`packaging/linux/build-packages.sh`** — stages the binary into `dist/stage/` and runs
  nfpm three times with the conventional names (`mindfork-rs_X.Y.Z-1_amd64.deb`,
  `-X.Y.Z-1.x86_64.rpm`, `-X.Y.Z-1-x86_64.pkg.tar.zst`). One shared script for CI and local use.
- **CI**: (1) a new **`packaging.yml`** (on `pull_request`/`push:main`, touching
  `packaging/**`, + `workflow_dispatch`): the `build` job (cargo build --release → nfpm
  via the goreleaser apt repo → 3 packages as an artifact) + the `smoke` job (a matrix of
  containers `ubuntu:24.04`/`fedora:latest`/`archlinux:latest`: installs the package via its own
  package manager, checks the layout/symlink/`defaults.json`, runs `mindfork-rs
  --version` **as a regular user** — the peek phase creates no directories). This is
  the CI install smoke (validated on a PR — the only way to check nfpm/
  containers since development happens on Windows). (2) **`release.yml`** gained the job
  `linux-packages` (from the ready `bin-linux` via the same script) + `release` now
  `needs: [build, linux-packages]` and puts the packages into `dist/` (they end up in
  `sha256sums.txt` and the GitHub Release).
- **The idiomatic Arch path — groundwork**: `.pkg.tar.zst` on Releases for `pacman -U` exists; an AUR
  `mindfork-rs-bin` (PKGBUILD + .SRCINFO from Releases) — a separate small step after
  the first package release.
- **Checks**: `cargo fmt/clippy/test` unaffected (no code changes; **1077 unit tests**
  same as stage 1); the nfpm/both workflow YAML valid (structure/types `config|noreplace`/
  `symlink`/paths verified). **Live run**: nfpm and the container installs **can't be
  reproduced locally on Windows** — `packaging.yml` runs them on a PR (building packages +
  an install smoke across three distros); that's the stage's DoD verification.
- **Next:** stage 3 `feat/windows-installer` (Inno Setup), optional stage 4 "Signing"
  (deferred by the user).

### Post-M9: installers — stage 3 (Windows installer, Inno Setup) (done)
- **Stage 3** of the "installers" track (`docs/history/installers.md` §3.3, §8; branch
  `feat/windows-installer`, **linear stack on `feat/linux-packages`** — both stages
  edit `release.yml`, the linear `1→2→3` chain avoids a file conflict; builds
  on P1/P3 from stage 1: writes `defaults.json` with a UTF-8 BOM and puts dictionaries next to the binary).
  Packaging/CI only — the app's source code untouched.
- **`packaging/windows/mindfork.iss`** — Inno Setup 6.7.x, exe format (not MSI: every
  requirement is a stock Inno feature). Two **custom wizard pages**:
  "Application language" (radio buttons Russian/English, `CreateInputOptionPage(Exclusive)`) and
  "Data location" (radio buttons: the standard OS folder / portable / a custom folder
  via `CreateInputDirPage`). The choice is written to `{app}\defaults.json` at `ssPostInstall`
  (`{"mode":…,"default_language":…}`, `SaveStringsToUTF8File` — with a BOM, dropped by P3;
  Cyrillic paths in the JSON are escaped). **Not overwritten on upgrade**
  (`if not FileExists` + `ShouldSkipPage` hides both pages if `defaults.json`
  already exists). A bilingual UI: `[Languages]` en+`Russian.isl` (the official one), page
  text via `[CustomMessages]` with `ru.`/`en.` + `CustomMessage()`. Per-user, no UAC
  (`PrivilegesRequired=lowest` + `…OverridesAllowed=dialog`, `{autopf}`→
  `%LOCALAPPDATA%\Programs`); the portable option is hidden when installing per-machine
  (in Program Files you can't write data next to the exe). Dictionaries — at `{app}\data\
  dictionaries` (reserved by P1). The uninstaller cleans up only `defaults.json` +
  the installed files; user data (`%APPDATA%`/portable) is untouched.
  **The file is saved as UTF-8 with a BOM** — otherwise Inno on an en-US runner corrupts the
  Cyrillic.
- **CI**: (1) `packaging.yml` gained a `windows-installer` job (a Windows runner):
  installs Inno via choco, **compiles the `.iss` with a stub binary** — validates the
  `.iss` syntax and the Pascal `[Code]` on a PR (a real build isn't done here; locating
  `ISCC.exe` is resilient to Inno 6/7's version/path). (2) `release.yml` gained a job
  `windows-installer` (compiles the real `setup.exe` from the ready `bin-windows`);
  `release` now `needs: [build, linux-packages, windows-installer]` and puts
  the installer into `dist/` (→ `sha256sums.txt` + the GitHub Release).
- **Fix for the `{app}` crash in `ShouldSkipPage` (found via a live GUI run).** The first version
  checked for an upgrade via `ExpandConstant('{app}\defaults.json')` — but `{app}` isn't
  initialized yet at the wizard-page-display stage, and expanding it crashed **any
  interactive install** into a "Runtime error: An attempt was made to expand the
  "app" constant before it was initialized" dialog. A silent install **didn't** catch this
  (`ShouldSkipPage` isn't called during it; `CurStepChanged`/`ssPostInstall` uses
  `{app}` correctly by then) — so the bug only showed up on the GUI path. Fix: the path via
  `AddBackslash(WizardDirValue) + 'defaults.json'` (the directory field's current value,
  valid during the wizard). Lesson: **a silent install doesn't substitute for a live GUI
  run** for Inno `[Code]` that depends on `{app}`/pages.
- **Checks**: no Rust code → **1077 unit tests** as in stages 1–2, fmt clean;
  both workflow YAMLs valid. **Live run — GO** (a real **Inno Setup 6.7.3** on
  the dev machine): (1) the `.iss` **compiles** (`ISCC.exe` — the Pascal `[Code]`, all
  `[Files]`, `SourcePath` paths, Cyrillic) → `setup.exe`; (2) **the GUI wizard launches
  with no crash** — the `TWizardForm` window "Setup - mindfork-rs version 0.9.0", no Runtime
  error dialog (before the fix, that's the only thing that appeared); (3) **a silent install**
  (`/VERYSILENT /CURRENTUSER /LANG=en`) writes `defaults.json` via the wizard code — exactly
  `{"mode":"system","default_language":"en"}` (BOM `EF BB BF`, dropped by P3);
  (4) **the "custom folder" mode (`mode:path`)** verified by exercising the same `[Code]`
  (`DataPage`=custom + a Cyrillic path) → `{"mode":"path","path":"C:\\Users\\…\\
  <cyrillic-folder>\\sub","default_language":"ru"}` — **the `\`→`\\` escape** (`JsonEscape`),
  **Cyrillic** (UTF-8), `/LANG=ru`→`"ru"`; (5) **the installed binary reads** both
  `defaults.json` variants (`--version`→`0.9.0`, exit 0 → the JSON is valid, the `system`/`path`
  modes resolve — a full installer↔app round trip); (6) **an upgrade doesn't
  overwrite** `defaults.json` (a repeat install with `/LANG=ru` on top → the file stayed
  `…"default_language":"en"`); (7) **a clean uninstall** (files, the HKCU
  Uninstall registry entry, the shortcut). All test artifacts removed. (8) **A live GUI wizard + screenshots**
  (`Graphics.CopyFromScreen` over the `TWizardForm` rect): both custom pages render
  correctly in **both locales** — "Application language" (radio buttons
  Russian/English) and "Data location" (system/portable/custom
  folder), the title "Setup - mindfork-rs version 0.9.0". **Clicking GUI controls wasn't
  automated**: Inno's controls (custom VCL) aren't in the UI Automation tree, and Win32
  `SendMessage` blocks synchronously — navigation went via the Enter key (the default
  button), the `mode:path` choice was verified by exercising `[Code]` via a silent install.
- **The "installers" track — complete** (stages 1–3: research + code prerequisites +
  Linux packages + the Windows installer; the plan moved to `docs/history/installers.md`).
  Optional stage 4 "Signing" deferred by the user (private repo, no site/icon). Groundwork:
  a winget manifest (portable zip until signing), an AUR `mindfork-rs-bin`, an MSI for GPO/Intune
  if demand arises.

### Post-M9: rendering Mermaid diagrams in the feed (done)
- **` ```mermaid `-blocks in the feed render as text graphics** (crate
  `mermaid-text` ≥ 0.56.1; two net-new dependencies — `mermaid-text` + `ascii-dag`)
  instead of printing the source. Getting to the feature was a three-step move: a **probe**
  (2026-07-14, verdict NO-GO on 0.56.0 — a panic on Cyrillic sequence diagrams + silent corruption of
  flowchart labels, byte offsets treated as character ones;
  [docs/research/mermaid-ascii-rendering.md](docs/research/mermaid-ascii-rendering.md)) →
  **an upstream fix, ours** (a bug report
  [leboiko/markdown-reader#29](https://github.com/leboiko/markdown-reader/issues/29) +
  a ready-made [PR #30](https://github.com/leboiko/markdown-reader/pull/30) with three fixes and
  regression tests; the maintainer did a full audit off our "broader note", closed
  3 more spots of the same class, and shipped 0.56.1) → **a re-probe** (0 panics on
  a 31-case corpus, the corruption gone) → implementation per the validated plan §4.
- **Key rule — a hard fallback instead of clipping** (clipping with "…" is fine for tables, a
  cut-off diagram is unreadable): only the whitelist renders
  (**flowchart/sequence** per `detect`; pie/gantt/class/state are ugly as text) and only
  what fits the panel width entirely (**our post-check**: the crate's `max_width` is a soft
  hint, sequence diagrams ignore it; the feed's "line ≤ width" invariant is hard, otherwise
  a second wrap pass would break the panel borders). Any failure (garbage/a truncated stream/a type outside the
  whitelist/overflow) → the source, as a code block, **byte-for-byte identical to the disabled-
  toggle render** (the golden test `mermaid_fallback_matches_disabled_render` compares
  whole `Line`s, styles included). Worst case = the previous behavior.
- **Implementation** (modeled on `TableBuilder`): `Writer.mermaid: Option<(info, src)>`
  — `start_codeblock`, when the flag is on and `lang=="mermaid"`, buffers the block (the fence
  isn't printed), `text()` accumulates the source verbatim (resilient to it being split across
  events), `end_codeblock` decides: `mermaid::render_mermaid_block` (a new submodule
  `shared/markdown/mermaid.rs`) or `emit_fenced_source` (the fallback, matching the old
  unhighlighted look: reverse video + DIM fences with the full info string ` ```mermaid title=x `).
  The compat palette (`palette.compat`) → `render_ascii_with_width` (ASCII glyphs, for conhost).
  Streaming: an unfinished block doesn't parse → the source; once finished, the feed's cache
  recomputes the message and swaps in the diagram. **We deliberately don't catch upstream
  panics** (`catch_unwind`): the app's panic hook restores the terminal on any
  panic, including a caught one — "catch and continue" would leave the TUI broken;
  we rely on the 0.56.1 audit + the narrow whitelist (recorded in the module doc).
- **Wiring**: `RenderOpts.render_mermaid` (Default=false — tests/tool cards see no
  behavior change); the feed threads through **the whole `RenderOpts`** instead of the bare
  `table_row_separators: bool` via `build_message_block`/`push_body`/
  `push_assistant_body`/`push_markdown_fragment`/`push_tool`/`push_block` (a future
  flag won't touch signatures again; `soft_break_as_newline` gets added to `push_body` for
  the user just as before). `MessageFeed.render_mermaid` + `set_render_mermaid` +
  a field in `CacheKey` (toggling the setting invalidates the cache); `ChatScreen::set_settings`
  threads it from the snapshot. The config `interface.render_mermaid` (`#[serde(default)]`
  on the container → old `settings.json` files without migration), **on by default** —
  the fallback makes enabling it safe. A "Mermaid diagrams" toggle in the "Appearance"
  group of the "Interface" section (`FieldId::IMermaid`, described in both locale
  bundles — the i18n gates covered it automatically).
- **Tests**: the mermaid module (a sequence/Cyrillic flowchart with no corruption; fallbacks —
  a narrow width/outside the whitelist/garbage/a truncated stream; ASCII in the compat palette; the width budget at
  60/90/120); writer (a diagram instead of the source + a 4-scenario golden fallback +
  the flag off + Cyrillic); the feed (the toggle + cache invalidation); settings
  (the toggle persists the config + the description); config (default-on). **1090 unit tests
  green** (+13), 48 `#[ignore]`, clippy `-D warnings`/fmt clean; the new crates' licenses
  (MIT; MIT OR Apache-2.0) are in the `deny.toml` allowlist. **No live engine run
  required** (a pure render module without the engine/memory — a precedent from
  markdown-refinements); the crate's behavior on real LLM diagrams was verified by
  the probe (a 31-case corpus, including 17 real ones from architecture.md).
- **Groundwork**: extending the whitelist (state/class/er — once their text render becomes
  readable). `max_width` as a hard budget — **closed**: our upstream feature request #32
  was implemented in 0.57.0 (`RenderOptions::max_width_strict` → `Error::TooWide`);
  we upgraded to 0.57.0 but did NOT adopt the strict mode — our post-check via
  `display_width` is more precise (accounts for CJK/emoji, matches the wrap in `message_feed`),
  and `render_with_width` already tightens the gaps to the width.

### Post-M9: plugins — stage 1: generic import (the mindfork-import format) (done)
- **The first stage of the "plugin system" track** (research
  [docs/research/plugin-system.md](docs/research/plugin-system.md), decision points R1–R8
  accepted by the user 2026-07-17 per the recommendations; branch `feat/generic-import`,
  stacked on `docs/plugins-research` — general roadmap edits, a precedent from the linear
  installer stack). Importing from third-party apps moved from an importer for
  LameLLaMA hardcoded into the monolith to a **neutral documented exchange
  format**: an external (possibly private) converter emits a single JSON file — the app imports
  it with the command `mindfork import <file>`. The pattern "converter → a documented
  file → import" — precedents beancount/beangulp, KeePass, Netscape bookmarks
  (research §5); knowledge of the non-public LameLLaMA leaves the monolith.
- **The `mindfork-import` v1 format** ([docs/import-format.md](docs/import-format.md) —
  an external contract): `format`/`version` + `profiles[]` (key/name/language/
  system_message/greeting/character_names/sampling) + `chats[]` (key/profile_key/
  title/dates/messages with user|assistant|system roles) + an optional `settings`
  (sampling/interface). **Deliberately NOT the domain entities as-is** (R3): a dedicated,
  small format doesn't lock the internal schema (`deleted`, `reflected_upto`, …)
  into an external contract. Properties: **idempotency** — id = UUIDv5 of a stable
  `key` (separate namespaces for profiles/chats — the ASCII `mindfork import` + a tag;
  an optional explicit `id` for continuity with a prior import); **strict about
  structure** (a wrong `format`, a version newer than supported → a downgrade guard,
  a duplicate/empty `key`, a reference to a profile not in the file, an unknown role → a clear,
  localized error with coordinates) while **tolerant of extension** (unknown
  fields are ignored — evolution without a bump, mirroring `#[serde(default)]`); a BOM
  is dropped. Sampling parses straight into `SamplingConfig` (unknown knobs
  dropped); interface settings apply **partially** (only fields that are set —
  not overwriting the user's settings; a refinement over the previous wholesale approach).
  A chat's system message: an explicit field takes priority, else the first system message in the
  history (system messages aren't written to history — in mindfork it's separate from the messages).
- **`features/import.rs`** (replaced `features/migration.rs`, which was removed):
  pure `parse_import(json, loc)`/`import_file(path, loc)` → `ImportResult
  {profiles, chats, sampling, interface}`; `main.rs::run_import` as before
  (data_migration before opening storage, upserts, partial settings application).
  **CLI**: the subcommand `import <file>` replaces `import-lamellama <dir>` (R4 —
  remove it right away); running the old command gives **a hint about the
  replacement** (`cli.import.lamellama_removed`), not an "unknown command" error. i18n: 11 new
  `import.*` keys + `cli.*` edits (ru+en; the parity/no-dead/Cyrillic gates
  covered it automatically; a nuance — the test file isn't named `import.json`: the
  gate scanner picks up dotted literals with the bundle prefix `import.`).
- **Tests**: a golden fixture of the format (mirroring the spec's example, with intentional
  unknown fields), idempotency/separate namespaces, an explicit `id`,
  an explicit `system_message` taking priority, all validation errors (+ en localization with no
  Cyrillic), BOM/a missing file; CLI (import/the import-lamellama hint/
  help/column alignment). **1093 unit tests green** (+3: −8 migration,
  +10 import, +1 cli), 48 `#[ignore]`, clippy `-D warnings`/fmt clean.
- **No live engine run required** (a file operation without the engine/memory);
  instead — **a manual smoke against the real binary** in an isolated directory
  (a portable `data/` next to the exe): importing 1 profile + 2 chats → all fields land
  correctly (language/greeting/profile sampling, system from history, thoughts, partial
  settings: `temp=0.7`, `theme=dark`, spellcheck untouched); **a repeat import is
  idempotent** (same UUIDs, overwrite + `.bak`, no duplicates);
  `import-lamellama` — a localized hint; `--help` — even columns.
  Importing real LameLLaMA data (226 chats) — pending a private
  converter (outside this repo; the ready wire types from the old `migration.rs`
  are portable into it as-is).
- **Next**: the `spike/mcp-client` probe (stage 2 of the track, go/no-go on a live
  Gemma 4) → `feat/mcp-host` (stage 3). See the plan in research §7.

### Post-M9: plugins — stage 2: MCP client probe (GO) (done)
- **A probe for the "plugins" track** (stage 2 of the plan, branch `spike/mcp-client` —
  stacked on `feat/generic-import`): a go/no-go check for "a live local model
  correctly calls tools of a real MCP server via our agentic-loop machinery".
  Verdict — **GO** (results in the research doc
  [docs/research/plugin-system.md §9](docs/research/plugin-system.md)), stage 3
  (`feat/mcp-host`) unblocked.
- **Mini-client `shared/mcp.rs`** (tools-only, stdio, target revision 2025-11-25;
  in the probe the module is under `#[cfg(test)]` — not compiled into the binary, it
  enters the binary at stage 3): transport is decoupled from the process —
  `McpConnection::over` works over any `AsyncRead`/`AsyncWrite` (protocol unit
  tests on `tokio::io::duplex` without processes), `McpClient::spawn` adds the
  subprocess (`CREATE_NO_WINDOW`, continuous stderr drain to the log — otherwise a
  filled pipe would block the server, a shutdown ladder "close stdin → wait → kill",
  `kill_on_drop`). Protocol: `initialize` (we send 2025-11-25, we accept any counterpart
  version — the tools subset has been wire-stable since 2024-11-05) →
  `notifications/initialized`; `tools/list` with `nextCursor` pagination; `tools/call`
  (text blocks concatenated, non-text → a placeholder, `isError` surfaced); we reply to
  `ping` with an empty result and `-32601` to any other server request (silence would
  hang a well-behaved server); `notifications/cancelled` on request timeout; **garbage
  lines on stdout are skipped with a warn** (field-host pitfall #1), unknown
  notifications are ignored.
- **Protocol unit tests** (5, against a scripted fake server over duplex):
  handshake + pagination + a call; banner garbage before JSON doesn't break the
  connection; ping→an empty result and a sampling request→`-32601` (checking
  client-side responses from the server's point of view); a timeout sends
  `notifications/cancelled` with reason=timeout; a JSON-RPC error surfaces as text.
- **Live smoke — GO** (`gemma_reads_file_via_mcp_filesystem_server`,
  `#[ignore]`; Gemma 4 31B q4, an external `llama-server --jinja` on a nearby machine
  + **a real third-party** `npx @modelcontextprotocol/server-filesystem`): the
  `secure-filesystem-server 0.2.0` server confirmed protocol **2025-11-25**;
  **14 tools, schemas ≈ 7 KiB** — the 16k context stays intact; the model called
  `read_text_file` with correct JSON arguments **on the very first round**, the result
  went out in the second round, the final answer uses the file's content;
  2 rounds, ~10 s, a clean shutdown. The smoke's manual mini-agentic-loop mirrors the
  orchestrator's mechanics (stream → `ToolCallAccumulator` → execution →
  `assistant_tool_calls`+`tool` messages → the next round). The "npx is a
  `.cmd` shim" pitfall was confirmed: on Windows, spawn via `cmd /c npx …`.
- **Refinements for stage 3 based on the probe's findings**: accept any counterpart
  protocol version (the 2025-11-25 server replies with our own version; a strict
  rejection isn't needed); the server config must accept both `cmd /c npx …`
  commands and direct exe paths. **1098 unit tests green** (+5), **49 `#[ignore]`**
  (+1), clippy `-D warnings`/fmt clean.

### Post-M9: plugins — stage 3a: MCP host core (done)
- **Stage 3 of the "plugins" track** (docs/research/plugin-system.md §4.4–§4.7, §7;
  part 3a — the core; branch `feat/mcp-host`, stacked on `spike/mcp-client`): MCP
  servers are launched by the app from config, their tools become full-fledged
  `Tool`s in the registry, called by the standard agentic-loop. UI polish
  (statuses/descriptions in settings, TOFU catalog pinning), i18n chrome, and docs
  (spec/architecture/README/install/CHANGELOG/ADR) — stage 3b.
- **`shared/mcp.rs` graduated from `#[cfg(test)]` into the binary** and grew into a
  host: a monitor task owns the `Child` (`kill`/`exited` tokens, mirroring
  `managed.rs`; the client's `Drop` arms `kill`, a grace period for a self-initiated
  exit after closing stdin); **Job Object kill-on-close** (Windows, borrowed from
  `sandbox.rs`) — a process tree (`cmd /c npx` → `node`) doesn't outlive app
  exit/crash; spawn accepts an **env map** (already-resolved pairs); **`.bat`/`.cmd`
  are forbidden** as the server command (`forbidden_batch_command`, BatBadBut
  CVE-2024-24576 — `cmd /c npx …` and direct exes are allowed);
  `request_cancellable` — cancelling via the token sends the server
  `notifications/cancelled` (reason=cancelled, mirroring the timeout);
  `list_tools`/`call_tool` moved onto `McpConnection` (testable on a duplex without
  processes); `shutdown(self)` simplified to "drop + wait for exited".
- **Config `config.mcp`** (`McpSettings`, `#[serde(default)]` → no migration): a
  master switch `enabled=false` (like Python) + `servers[]` (an `id` slug /
  `command` / `args` / an `env` map of the **names** of source env vars — secrets
  never enter `settings.json`, per R8 / `enabled` / `tool_timeout_secs`=60 /
  `max_result_chars`=20k).
- **Wrapper `McpTool: Tool`** (`features/tools/mcp.rs`): id
  `mcp__<server>__<tool>` (a sanitizer `[A-Za-z0-9_-]`, ≤64 — truncation + a hex
  tail derived from the full name against collisions); the description/schema —
  **a server snapshot, not localized** (an i18n boundary, like engine probe
  errors); `invoke` → `tools/call` with a per-call timeout and `ctx.cancel`; the
  result **is clipped** (`max_result_chars`, a localized marker
  `tool.mcp.result_truncated`); `isError` with empty text → a clear marker.
  Group `ToolGroup::Plugins` (ALL=9) + gate `ToolGate::Mcp`;
  `enabled_by_default=false` (**double opt-in**, per R7 — `reconcile_tools`
  doesn't auto-enable it); labels are interned as `&'static str`
  (`ToolInfo.label`; a precedent — interning i18n language codes).
- **`effective_tool_ids` += `mcp_enabled`**: `mcp__*` tools are gated by the
  master switch **by prefix** — they're dynamic, absent from the static `CATALOG`
  (a `CATALOG` lookup is impossible). A 6th parameter, all call sites updated.
- **Cancellation in `ToolContext`** (a small refactor, useful for native tools too):
  `TurnInfo`/`ToolContext` now carry `cancel: CancellationToken` (a clone of the
  generation task's / background loop's token); the agentic-loop wraps
  `registry.invoke` in a **`select!` with the token** — Esc isn't blocked by a
  long-running tool (MCP/network); a cancelled call yields the result
  `loop.tool_cancelled`, after the round the loop ends the turn as `Cancelled`
  (accumulated output preserved). The MCP call itself notifies the server
  (`notifications/cancelled`) in that case; for other tools the select is a
  safety net.
- **`McpManager`** (`app/orchestrator/mcp.rs`, mirroring `EngineManager`): spawns
  enabled servers as **background tasks** (command handlers stay synchronous) →
  events `Ready{conn,tools}`/`Failed{reason}`/`Exited` into the loop's internal
  channel `run`; **an epoch guard** — late events from shut-down servers (a
  "died before cancel" race) are dropped; config validation before spawn (slug
  id, non-empty command, batch ban) → `Disconnected(reason)`; a missing source
  env variable — warn+skip. **A restart budget** (the VS Code LSP pattern): a
  crash after readiness → restart, but ≤3 per 5 min, beyond that —
  `Disconnected` until settings are edited; a startup failure (`Failed`) isn't
  restarted (likely a config error). `rebuild_registry()` — the **only** path
  for rebuilding the registry (standard set + live MCP wrappers; previous
  `build_registry` call sites migrated); `config.mcp` edits go through the
  `RestartQueue.mark_mcp` debounce (a 4th flag); `Quit` shuts servers down
  (dropping slots → cancel → the shutdown ladder).
- **A dynamic catalog in the UI**: `AppEvent::Settings` += `mcp_tools:
  Vec<ToolInfo>` (a snapshot from the manager; `ChatScreen.settings_snapshot`
  became a 4-tuple `SettingsSnapshot`); `SettingsScreen::tool_catalog()` — an
  instance method (the static catalog + an MCP tail, `PTool` indices for the
  static part are stable; `default_fields` copies the MCP snapshot into the tmp
  screen). MCP tool toggles — grouped in the profile under "Plugins (MCP)" with
  an **honest gate** ("disabled globally: MCP servers"); the master toggle "MCP
  servers" — in the "Tools" section (`FieldId::TMcpEnabled`). Server statuses on
  screen — stage 3b.
- **Tests**: client (cancel → `notifications/cancelled`; `call_tool`
  concatenation/non-text placeholders; a batch-command ban, incl. in `spawn`);
  config (a partial server entry → defaults, round-trip); McpTool (the id
  sanitizer — short/long with a hex tail; character-based clipping +
  localization; invoke over a duplex fake; cancellation via `ctx.cancel`);
  manager (slug validation; the restart budget under paused time; Ready builds
  the catalog + a foreign epoch is dropped; Exited clears tools and respects the
  budget; a disabled/invalid server); orchestrator (Ready → a wrapper in the
  registry + `mcp_tools` in the Settings snapshot; Exited removes it from the
  registry); settings (MCP toggles in the profile with a gate; the master
  toggle). **1120 unit tests green** (+22), **50 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt clean.
- **Live run — GO** (Gemma 4 31B q4, external `llama-server` + a real
  `npx @modelcontextprotocol/server-filesystem`): `mcp_filesystem_e2e_live` —
  **the full path through the orchestrator** (config with a server → `Ready` →
  profile toggles → a turn): 14 tools in the catalog, the model called
  `mcp__fs__read_text_file` and named the secret number from the file; the
  probe's client-level smoke (`gemma_reads_file_via_mcp_filesystem_server`) is
  green on the new API (protocol 2025-11-25, 2 rounds, ~5–8 s). A side lesson
  from the test: `wait_for` drains events — `ProfileList` (bootstrap) has to be
  pulled **before** waiting for the later Settings that carries the MCP catalog,
  otherwise the test hangs.
- **Deliberate boundaries of 3a** (→ 3b/future work): server statuses and
  viewing full tool descriptions in the UI, TOFU catalog pinning (a rug-pull
  detector), localization of status reasons (currently Russian, a technical
  layer), `notifications/tools/list_changed` (a catalog re-listing), non-text
  result blocks (placeholder), a per-server tool ceiling.

### Post-M9: plugins — stage 3b: MCP host (TOFU, UI, i18n, docs, ADR) (done)
- **Completion of the "plugins" track** (same branch `feat/mcp-host`, on top of
  3a): MCP host security and observability + a full docs package. The track
  (stages 1–3) is closed by **ADR 0007** "Plugins: MCP host + a neutral import
  exchange format"; behavior — spec **§9.6** (a new section).
- **TOFU catalog pinning** (a rug-pull detector / tool poisoning, per R7):
  `catalog_hash` (`features/tools/mcp.rs`) — sha256 over sorted (name+
  description+JSON schema) tools; determinism via sorting by name + ordered
  `serde_json` keys (BTreeMap). The pin — `McpServerConfig.pinned_catalog`
  (`#[serde(skip_serializing_if)]`; written by the **orchestrator**, not the UI;
  manually removing the field = resetting trust). Flow: first startup →
  auto-pin (`McpEventOutcome.pin` → `persist_mcp_pin` — a direct config write,
  NOT through `handle_update_config`, otherwise diffing `config.mcp` would loop
  restarts); a match → registration; **a mismatch** → the catalog is held in
  the slot (`PendingCatalog{conn,tools,hash}`), the tools are NOT handed to the
  model, status "catalog changed — please reconfirm". Confirmation: Enter on the
  server row → `SettingsIntent::ConfirmMcpCatalog` →
  `AppCommand::ConfirmMcpCatalog` → `McpManager::confirm` (registers + persists
  the new pin). A guard against a UI race: `handle_update_config` **inherits
  pins** by server id from the previous config when the UI snapshot doesn't
  carry them (a stale copy doesn't reset trust or trigger a false restart).
- **MCP host snapshot in the UI**: `McpSnapshot { tools: Vec<ToolInfo>, servers:
  Vec<McpServerSnapshot{id,status,tool_count,pending_catalog}> }`
  (`features/tools/mcp.rs` — FSD-clean for screens) replaced the bare
  `mcp_tools` in `AppEvent::Settings`; `handle_mcp_event` now emits settings on
  **any** event (live statuses), the registry only rebuilds when the catalog
  changes. `ToolInfo` gained `description: Option<String>` — the **full**
  description of an MCP tool (the server's own text; built-in tools have
  `None` — their descriptions live in the bundles).
- **Settings UI**: in the "Plugins (MCP)" group of the "Tools" section — server
  status rows (`ready · tools: N` / `connecting…` / a failure reason;
  read-only `FieldId::TMcpServer(idx)`); a server whose catalog has changed —
  warn + a hint "Enter: confirm". MCP tool toggles in the profile show the
  **full description** in the bottom panel on focus (a tool-poisoning antidote
  — descriptions go into the system prompt). For this, `FieldRow.description`
  was changed from `Option<&'static str>` to `Option<String>`
  (`describe(impl Into<String>)`; ~3 consumers adjusted) — dynamic server text
  isn't interned.
- **i18n of status reasons** (axis B): `McpManager::apply`/`handle_event`/the
  spawn task gained `loc` — config validation, the restart budget, "catalog
  changed", and `Failed` contexts — pulled from the bundles (`ui.err.mcp.*`,
  ru+en); server-row text — `ui.settings.mcp.*`. **Boundary**: the client's
  wire errors (`shared/mcp.rs`) — a technical layer (like HTTP client
  wrappers, i18n-cli §7), tool descriptions/schemas — server text (not
  localized).
- **Docs**: spec §9.6 (a full description of the subsystem), architecture §3 (a
  map: `shared/mcp.rs`, `orchestrator/mcp.rs`, `features/tools/mcp.rs`) + §8
  (the "Plugins (MCP)" group in the table, an implementation bullet), README (a
  feature block), install.md §4.2 (configuring servers with a jsonc example,
  `cmd /c npx`, TOFU), CHANGELOG (Added + Security), roadmap (MCP future work:
  HTTP transport, resources/prompts, list_changed, deferred schemas, a UI
  editor, …), ADR 0007 (+links from CLAUDE.md/architecture), status in the
  plugin-system.md research doc.
- **Tests**: manager (first startup auto-pins + returns the pin to persist; a
  changed catalog is held → confirm registers and returns the hash; a matching
  pin — no re-persisting; descriptions in `ToolInfo`); orchestrator (the pin
  persists to `settings.json`; a changed catalog doesn't reach the registry
  until `ConfirmMcpCatalog`; pin inheritance under a stale UI snapshot);
  settings (server rows + Enter confirmation only when pending; a full
  description in the toggle row). **1125 unit tests green** (+5 over 3a),
  **50 `#[ignore]`**, clippy `-D warnings`/fmt clean.
- **Live run — GO** (Gemma 4 31B q4 + a real `npx server-filesystem`): the e2e
  `mcp_filesystem_e2e_live` was rerun after 3b — TOFU auto-pin on the happy
  path is transparent (14 tools, `mcp__fs__read_text_file`, 7319 in the
  response, ~7 s); the TOFU negative path (a changed catalog) is covered by
  unit/integration tests (a live simulation of a server swap isn't needed —
  the mechanics are purely client-side).

### Post-M9: Mermaid — source until the closing fence (fix for streaming flicker) (done)
- **Symptom**: during a streamed response a ```mermaid block "flickered" — as
  chunks arrived, the feed alternately rendered a stub as a diagram and reverted
  to the source. **Cause**: CommonMark (pulldown-cmark) stretches an unclosed
  fence to the end of the document, so from the parser's events an in-progress
  streamed block is indistinguishable from a complete one, and a syntactically
  valid stub (`flowchart LR\n A --> B` with no tail) successfully rendered as a
  partial diagram. The claim in spec §11.4 that "an incomplete block doesn't
  parse → source" relied on the stub failing to parse — but it often does
  parse. Branch `fix/mermaid-stream-fallback`.
- **Fix** (`shared/markdown/`): fence closure is checked **against the
  source**. `render_with` switched to `Parser::into_offset_iter()`;
  `Writer::run(src, iter)` calls a new `fenced_block_is_closed(src, range)`
  before every `Start(CodeBlock)` (only when mermaid rendering is enabled) — a
  `Start(Tag)` range covers the whole element: a block followed by more text
  in the document is closed by construction; for a block running to EOF, the
  range's last line must be a closing fence (the same fence character
  `` ` ``/`~`, no shorter than the opener; blockquote prefix/indent trimmed).
  The result is a `Writer.codeblock_closed` field; the mermaid branch of
  `start_codeblock` buffers the block only when the fence is closed, otherwise
  — the regular source path (byte-for-byte identical to the toggle-off render,
  the prior golden invariant). The check is deliberately simple: a false
  "closed" on an edge case would only trigger an attempted render with the
  standard fallback.
- **No plumbing through the screen/events is needed**: the feed's cache already
  recomputes a streaming message by fingerprint — once the closing fence
  arrives, the block is recomputed and the source is swapped for the diagram;
  exactly **one "source → diagram" transition per block** (closure is a
  property of the block, not of the end of the message: a closed diagram
  renders even while the message's tail is still streaming). Bonus: a block
  whose model forgot the closing fence forever stays source (consistent with
  the hard-fallback philosophy).
- **Tests**: writer (`mermaid_unclosed_fence_streams_as_source` — golden: 6
  truncation shapes, including a valid stub, a tilde fence, and "closer shorter
  than the opener", byte-for-byte equal to the disabled render;
  `mermaid_closed_fence_renders_even_while_tail_streams`); feed
  (`streamed_mermaid_stays_source_until_fence_closes` — simulating streaming
  across two chunks through the cache). **1128 unit tests green** (+3), 50
  `#[ignore]`, clippy `-D warnings`/fmt clean. No live run needed (a pure
  render module without an engine — precedent mermaid-render/
  markdown-refinements); behavior covered by golden tests. Docs: spec §11.4,
  CHANGELOG (Fixed).

### Post-M9: self-model consolidation — stage A1 (background "sleep" on a timer) (done)
- **First stage of the "self-model consolidation" track** (design doc
  [docs/history/self-model-consolidation.md](docs/history/self-model-consolidation.md),
  branch `feat/self-model-auto-consolidate`): a periodic background "sleep"
  specifically for memory-about-self — mirroring `notes.auto_consolidate_every`.
  Previously self-model consolidation only happened via auto-reflection/the
  interactive `reflect`; A1 adds a **separate** periodic task that itself
  merges duplicate observations (`@self` notes), compresses a bloated
  `summary`, and links contradictions. The scaffolding was **specifically
  prepared** by SOLID stage 2 ("family task #3 doesn't touch `run()`/`Quit`")
  — adding it amounted to mirroring `consolidation.rs`/`reflection.rs`.
- **A mini agentic-loop, like reflection/note consolidation**
  (`app/orchestrator/self_consolidation.rs`, new): `maybe_auto_self_consolidate`
  is called in `handle_done`; gates — the feature is enabled
  (`self_model.auto_consolidate_every`, 0=off), the profile has enabled
  self-model tools (`get_self_model`), **there's something to consolidate**
  (`@self` observations ≥ 2 **or** `summary` exceeds `summary_target_chars`
  per `summary_fill_hint`), no "sleep" is already running, the server is
  `Ready`. Cadence tracked by the `self_consolidate_counts` counter (reset only
  on an actual spawn — a gate skip doesn't lose the cycle, same as its
  siblings). Tool set (intersected with the profile):
  `get/update_self_model`/`update_user_model` + `note_revise`/`supersede`/
  `merge`/`link`/`neighbors` over `@self`; **`note_recall` is withheld** (it
  hides `@self`; full ids come from `get_self_model`). Digest —
  `build_self_consolidation_overview` + a `summary_fill_hint` line. The system
  message `prompt.self_consolidate.system` is assembled from
  `self_model::policy_core` (a single source of rules, same as
  `reflect_system_message`). DB-only, the "sole owner of `Chat`" invariant
  intact.
- **Observability** (`background.rs`): `handle_bg_done` now emits
  `SelfModelChanged` on success for **both** reflection **and**
  `SelfConsolidation` (an open `F3` reloads the snapshot); a run of failures →
  `ui.err.bg_self_consolidation`. A new `BackgroundKind::SelfConsolidation`
  (events.rs) → `dispatch.rs` → `ChatScreen::set_self_consolidating` → a quiet
  status-bar chip. `background_hint` **was generalized** from a 2-flag match
  into a `·`-joined list of active-task labels (scales to N tasks; the key
  `ui.chat.bg.both` was dropped, `ui.chat.bg.self_consolidate` added).
- **Config/UI**: `SelfModelSettings.auto_consolidate_every` (`#[serde(default)]`,
  default 0 — no migration; mirrors `auto_reflect_every`); field
  `SmAutoConsolidate` in the "Memory" → "Self-model" section (catalog/mod/spec),
  i18n keys for the fields/descriptions.
- **A separate toggle, not a shared one** (user's decision): gates/data for the
  self-model and for notes are already kept apart, its own counter is more
  precise. **A2** (summary↔observation semantics) and **A3** (interest aging) —
  deliberately NOT in this PR (next stages).
- **Tests**: integration (`orchestrator/tests/self_consolidation.rs` — spawns
  at the threshold + resets the counter; the counter stays intact when the
  server isn't ready; a gate when the feature is disabled; a "nothing to
  consolidate" gate that preserves the counter; success →
  `SelfModelChanged`); unit (the system message is composed from `policy_core`
  + per-locale; the tool set excludes `note_recall`). **1136 unit tests
  green** (+8), 51 `#[ignore]`, clippy `-D warnings`/fmt/i18n gates clean.
  Live smoke `self_consolidation_e2e_live` (`#[ignore]`, mirroring
  `auto_reflect_e2e_live`) — a run against real Gemma 4 + bge-m3 was a manual
  step. **Live smoke green** (Gemma 4 31B q4 + bge-m3): with
  `auto_consolidate_every=1`, two similar observations were merged, a bloated
  summary compressed.

### Post-M9: self-model consolidation — stage A2 (summary↔observation semantics) (done)
- **Second stage of the track** (design doc
  [docs/history/self-model-consolidation.md](docs/history/self-model-consolidation.md)
  §A2, branch `feat/self-model-summary-semantics`): the self-consolidation
  overview gained a section "a paragraph of the self-description (`summary`)
  semantically overlaps observation X → extract/stitch". Observations (`@self`
  notes) hold vectors in the DB, but `summary` has none (free-form text) — so
  paragraphs are embedded **on the fly** in a single request (a direct mirror
  of the trait gate `self_model::near_duplicate_traits`).
- **An async layer over a synchronous handler** (a key nuance):
  `build_self_consolidation_overview` stays a **pure synchronous** DB read
  (the 3 previous sections, tests intact). A2 semantics — a separate **async**
  helper `notes::summary_observation_overlaps(storage, embedder, profile,
  loc)` (graceful degradation: no embedder / a vector-count mismatch →
  `None`). Wired into three places: (1) interactive `reflect`
  (`Reflect::invoke`, already async) — appends the section; (2)+(3)
  background reflection/`self_consolidation` — their handlers are
  **synchronous** and spawn a task, so the section is computed **inside the
  spawned task**: `SilentLoop` gained a new optional field
  `summary_semantics: Option<SummarySemantics{embedder,storage,profile_id,
  loc}>`, and `spawn_silent_loop` `await`s the helper before the loop and
  appends the result to the request's first user message. Note consolidation
  (`consolidation.rs`) passes `None` (its overview is about the interlocutor's
  notes, not about `summary`).
- **The threshold was calibrated on live bge-m3** (not guessed; the smoke
  `summary_obs_calibration_e2e_live` prints cosines for labeled pairs):
  paraphrase pairs "paragraph ↔ observation" scored **0.69–0.80**, unrelated
  pairs — **0.48–0.51**; a clean gap 0.51→0.69 → `SUMMARY_OBS_SIMILARITY =
  0.62` (inside the gap, with margin on both sides). Paragraphs are longer
  than short traits, so paraphrases score a bit lower than the trait gate
  (0.73–0.83, threshold 0.72). One match per paragraph (to avoid noise);
  fragments < 40 characters are dropped.
- **No config/migration** — the threshold is currently a code constant (no
  extra settings field added, as in the design doc); an i18n key for the
  section `notes.self_overview.summary_obs` (ru+en). DB-only, the "sole owner
  of `Chat`" invariant intact; FSD (the helper lives in `features`,
  `tool_loop` calls it).
- **Tests**: unit tests on `MockEmbedder` (threshold-robust: a match ≈1.0,
  unrelated ≈0.0) — the section surfaces a matched paragraph and doesn't
  surface an unrelated one; graceful degradation (no observations / an empty
  summary / a vector mismatch → `None`). Live `#[ignore]`:
  `summary_obs_calibration_e2e_live` (prints cosines for calibration) and
  `summary_obs_overlap_e2e_live` (the section surfaces on real bge-m3 —
  measured at 0.77). **1138 unit tests green** (+2), 53 `#[ignore]`, clippy
  `-D warnings`/fmt/i18n gates clean. **Live run green** (bge-m3): calibration
  + section surfacing confirmed; the 0.62 threshold catches all paraphrases,
  filters out unrelated pairs.
- **A3** (aging of `current_interests`) — the next/final stage of the track.

### Post-M9: self-model consolidation — stage A3-light (interest aging) (done)
- **Final stage of the track** (design doc
  [docs/history/self-model-consolidation.md](docs/history/self-model-consolidation.md)
  §A3, branch `feat/self-model-interests-aging`): the interlocutor's
  `current_interests` — a plain `Vec<String>` — was never washed out by
  anything. **The light path** was chosen (no schema/config change, in the
  project's spirit of "integration by the model itself"): a **nudge** in the
  canonical "maintenance protocol" `selfmodel.policy_core` — "for 'current'
  interests: remove via `remove_interests` those the interlocutor hasn't
  confirmed in a while." The tool `update_user_model` already supports
  `remove_interests` — code/schema untouched, only the bundle text was edited
  (ru+en).
- **One source → three consumers**: `policy_core` feeds, through composition,
  the turn injection (`maintenance_protocol`), auto-reflection, and self-model
  auto-consolidation — the nudge propagates to all three without duplication.
  **The heavy path** (`current_interests: Vec<Interest{text, updated_at}>` —
  real time-based aging) was deliberately **not** done: it would break the
  flat `Vec<String>` and ~a dozen call sites; left as future work in case the
  light path proves insufficient on a live model.
- **Tests**: `policy_core_nudges_interest_aging` (the nudge carries
  `remove_interests` + the aging idea). **1139 unit tests green** (+1), 53
  `#[ignore]`, clippy `-D warnings`/fmt/i18n gates clean. The effect is
  behavioral/soft (the composed prompts are already covered by live
  reflection/consolidation smokes) — a separate live run isn't needed. **The
  "self-model consolidation" track (A1–A3-light) is complete** (A3-heavy —
  future work).

### Post-M9: RAG — indexing HTML sources (stage B1a) (done)
- **First stage of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B1a,
  branch `feat/rag-html-sources`): `/rag add` indexes `.html`/`.htm` alongside
  `.txt`/`.md`. **No new dependencies** — readable text is extracted by the
  already-existing `web::extract_readable` (scraper: paragraphs from
  `<article>`/`<main>`, dropping nav/header/footer/aside/scripts;
  `pub(crate)`, covered by tests).
- **An FSD-clean layout** (a deliberate departure from the letter of the design
  doc, which proposed an `extract_text` dispatcher inside `rag_ingest`):
  `features/rag_ingest.rs` stays a **pure file module**
  (`SUPPORTED_EXTENSIONS` += `html`/`htm` + a helper `is_html`), while
  **extraction is dispatched in the `app` layer**
  (`orchestrator/rag.rs::read_source_text`) — otherwise `features →
  features/tools` would be a sideways import, forbidden by FSD; `app`,
  however, may call `features/tools/web`. `read_source_text` (raw via
  `read_text` → for HTML, `extract_readable(&raw, usize::MAX)`, since RAG
  chunks it whole — no truncation) replaced `read_text` at the two points that
  read files (`index_file` and the legacy file read during `/rag rebuild`).
- **`index_source` untouched**: an HTML source isn't markdown → it goes to
  `chunk_text` (extracted text has no heading structure). The single
  user-facing string about formats (`ui.err.rag_no_files`) was updated to
  `.txt/.md/.html`.
- **Tests**: `is_supported`/`is_html` on html/htm/HTML (case-insensitively);
  `read_source_text` extracts an article paragraph and drops
  nav/header/script, passes non-HTML through verbatim (tempdir). **1141 unit
  test green** (+2), clippy `-D warnings`/fmt/i18n gates clean. No live run
  needed (pure extraction without an engine). Track future work: **B1b**
  (pdf/docx — with new crates and a license review), **B2** (ranking/dedup +
  per-chunk progress).

### Post-M9: RAG — per-chunk indexing progress (stage B2a) (done)
- **Second stage of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B2a,
  branch `feat/rag-chunk-progress`): previously `RagProgress::Indexing` was
  **per file**, and `index_source` embedded all of a file's chunks **in a
  single batch** — on a large file the banner froze until embedding finished.
  Now embedding runs in **sub-batches** and progress moves as chunks become
  ready.
- **Mechanics**: `RagProgress::Indexing` gained fields
  `chunks_done`/`chunks_total` (0/0 — the file has just started, chunking is
  still ahead → the banner is unchanged from before). `index_source` now
  takes a callback `progress: impl FnMut(usize, usize)` and embeds/writes in
  a loop over sub-batches `chunks.chunks(EMBED_BATCH_CHUNKS=16)`, calling
  `progress(done, total)` after each; `index_file` forwards the callback.
  Both background tasks (`spawn_rag_ingest`, `spawn_rag_rebuild`) emit an
  initial `Indexing{…,0,0}`, then updated chunk counts from the callback. The
  banner (`screens/chat/rag.rs`), when `chunks_total>0`, appends the
  `ui.rag.chunks` suffix (" · chunks N/M").
- **A batching change** (an improvement): a file with ≤16 chunks — still a
  single request (identical result); a large file now sends **bounded-size**
  requests instead of one giant one (bounded memory/request + live progress).
  Chunk order and stored documents unchanged. Requests are **smaller** than
  the previous single batch, hence strictly safer against a real server.
- **Tests**: `index_source_reports_chunk_progress_in_subbatches` (a file with
  > 16 chunks: the first tick `(0,N)`, the last `(N,N)`, monotonicity, all N
  documents written and found by `rag_search`),
  `index_source_empty_content_single_zero_tick`, a banner test (the chunk
  suffix when `chunks_total>0`, none when 0). **1143 unit tests green** (+2),
  clippy `-D warnings`/fmt/i18n gates clean. Live smoke `rag_en_e2e_live` on
  real bge-m3 green (the embed path intact; sub-batching introduced no
  regressions). `index_source` grew to 8 arguments — a targeted
  `#[allow(clippy::too_many_arguments)]` with an explanation (cohesive
  arguments, carving out a bundle just for one parameter would be extra
  churn). Track future work: **B1b** (pdf/docx), **B2b** (ranking/dedup
  across sources).

### Post-M9: RAG — indexing PDF/DOCX (stage B1b) (done)
- **Continuation of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B1b,
  branch `feat/rag-pdf-docx`): `/rag add` indexes `.pdf` and `.docx`
  alongside `.txt`/`.md`/`.html`. Extracted plain text → `chunk_text` (no
  heading structure). Extraction is best-effort: a scanned PDF with no text
  layer yields nothing (0 chunks), a corrupt file is skipped with a `warn`
  (the existing ingest loop already handles file errors).
- **New module `features/doc_extract.rs`** (pure functions over bytes,
  external crates, no cross-layer imports — FSD): `extract_pdf(&[u8])` (crate
  `pdf-extract`), `extract_docx(&[u8])` (DOCX = a deflate ZIP; reads
  `word/document.xml` via the already-present `zip` + `quick-xml`, assembling
  text from `<w:t>` matched by **local** name, `</w:p>`→`\n`, `<w:tab/>`→`\t`,
  `<w:br/>`→`\n`). **A quick-xml 0.39 nuance**: entities (`&amp;`) arrive as a
  separate `GeneralRef` event (the name without `&;`) — we reconstruct and
  unescape via `quick_xml::escape::unescape`.
- **Dispatching lives in the `app` layer** (as in B1a):
  `orchestrator/rag.rs::read_source_text` became `anyhow::Result<String>` and
  branches html→`web::extract_readable`, pdf/docx→`doc_extract` (raw
  **bytes** via `fs::read`, not `read_text`), else→`read_text`. `rag_ingest`
  only gained the extensions (`SUPPORTED_EXTENSIONS` += pdf/docx) and helpers
  `is_pdf`/`is_docx`. `index_source` untouched (pdf/docx aren't markdown →
  `chunk_text`).
- **Dependencies**: `pdf-extract 0.12` (MIT, **pure Rust**, no C/`*-sys`;
  pulls in lopdf + font/CFF/CMap parsers + RustCrypto for encrypted PDFs —
  **~28 new transitive crates**, all with permissive licenses in the
  allowlist); `quick-xml 0.39.4` was promoted from transitive to direct
  (pinned to the version in Cargo.lock — no duplicate). `cargo deny` stays
  clean: one ignore was added for `RUSTSEC-2026-0192` (ttf-parser
  unmaintained — an advisory about being unmaintained, not a vulnerability;
  local user files, best-effort) + the rationale for the quick-xml DoS
  advisories was extended (now also covering direct use for DOCX). Allowlist
  licenses unchanged (everything already covered). A warn-level duplicate
  `thiserror 1.x/2.x` (pdf-extract pulls in 1.x) — not a blocker.
- **Tests**: doc_extract (DOCX: paragraphs via `\n`, Cyrillic, `&amp;`
  unescaping, tags don't leak, `<w:tab/>`/`<w:br/>`, corrupt/non-ZIP → `Err`;
  PDF: extraction from a **checked-in fixture**
  `tests/fixtures/hello.pdf` — a minimal valid 587-byte PDF with a correct
  xref, non-PDF → `Err`); rag_ingest (`is_supported`/`is_pdf`/`is_docx`);
  `read_source_text_routes_docx_and_pdf` (routing through extraction, txt
  passed through verbatim). **1151 unit test green** (+8), clippy
  `-D warnings`/fmt/i18n/**`cargo deny`** clean. No live run needed
  (extraction is offline-testable, the embed path unchanged — verified in
  B2a). Future work: **B2b** (ranking/dedup across sources).

### Post-M9: RAG — cross-source dedup of search results (stage B2b) (done)
- **Final stage of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B2b,
  branch `feat/rag-cross-source-dedup`): `rag_search` removes near-identical
  passages from **different** sources (the same content indexed from two
  files), so the model isn't fed a repeat.
- **Textual dedup, not embedding-based** (a fork, user's decision): the
  design doc proposed embedding-based dedup+reranking, but `rag_search` is a
  **hot path** (every search), and vec0 **already ranks** by relevance
  (distance). Re-embedding passages on every search would add latency, and
  reranking would largely duplicate vec0's sort. So — deterministic textual
  dedup: **no embedder, no threshold/calibration, no latency**.
- **Mechanics** (`features/tools/rag.rs::dedup_passages`, a pure function):
  after `stitch_hits` (stitching adjacent chunks within a source), passages
  go through dedup — in ranking order, a passage is kept only if its
  normalized text (Unicode lowercased + whitespace collapsed + trimmed) is
  **not wholly contained** in the text of an already-kept passage (a
  `k.contains(&norm)` check covers both equality and subset-inclusion).
  vec0's order is preserved (no re-sorting). Source-agnostic (the main case
  is a cross-source duplicate, but an intra-source repeat is noise too). **A
  lower-ranked superset** of a more relevant passage — both stay (no
  information lost: only a passage that's a subset of a **more relevant**
  already-kept one is dropped). Wired in as `dedup_passages(stitch_hits(hits))`
  in `RagSearch::invoke` before formatting; the format/empty-result path is
  unchanged.
- **Tests**: an identical duplicate from another source → one kept (the more
  relevant one); a subset of a more relevant one → dropped; distinct passages
  → all kept, order intact; dedup is case/whitespace-insensitive; a
  lower-ranked superset → both remain; empty input → empty output. **1157
  unit tests green** (+6), clippy `-D warnings`/fmt clean. No new
  dependencies/config/i18n. No live run needed (pure textual logic; the embed
  path untouched). **The "RAG: sources and retrieval" track (B1a html + B1b
  pdf/docx + B2a per-chunk progress + B2b dedup) is complete.**

### Release 0.9.1 (prepared)
- **A release PR** per the checklist in [AGENTS.md §6](AGENTS.md) (branch
  `chore/release-0.9.1`): bumped `Cargo.toml` `0.9.0 → 0.9.1` (+
  `Cargo.lock`), `CHANGELOG.md` — `[Unreleased]` → `[0.9.1] — 2026-07-18`, a
  fresh empty `[Unreleased]` opened, comparison links updated. The `v0.9.1`
  tag is applied by the user after the merge (the agent doesn't push
  tags/`main`).
- **Packages and the installer were already wired into the release** (the
  "installers" track, stages 2–3): `release.yml` contains the
  `linux-packages` (nfpm → deb/rpm/pkg.tar.zst) and `windows-installer` (Inno
  Setup → `setup.exe`) jobs, both listed in the publishing job's `needs`,
  artifacts are copied into `dist/` and end up in `sha256sums.txt` and the
  GitHub Release. No pipeline code changes were needed — only the endpoints
  were cross-checked: the `.iss` writes to `dist/*.exe` (matching the upload
  path) and accepts `/DAppVersion`; `build-packages.sh` +
  `packaging/nfpm.yaml` are in place. **v0.9.1 is the first tag that actually
  carries the packages/installer**: they landed in the pipeline only after
  `v0.9.0` (the published 0.9.0 doesn't include them, nor the spellcheck
  dictionaries).
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D
  warnings` / `cargo test` green — **1157 unit tests, 53 `#[ignore]`**. No
  live run needed (version + docs, app code untouched). Along the way: within
  `[0.9.1]` two duplicate "Fixed" sections were merged and the section order
  brought in line with what's declared in the CHANGELOG header; the test
  count and the date in the "## Status" header were refreshed (previously
  1128/50 from 2026-07-17).

### Post-M9: branding — logo and wordmark (stages 1–4) (done)
- **A new "branding" track** (design doc [docs/branding.md](docs/branding.md)):
  bring the `artwork/` directory to full-fledged branding — portable assets,
  a brand guide, a packaging icon, a TUI logo, a wordmark in the docs.
  Forks R1–R7 confirmed by the user on 2026-07-18: **R2** — the logo in the
  help overlay's header (`F1`), **R3** — leave the interface palette
  **untouched** (`accent` carries the meaning "activity"; the logo is drawn
  in the brand colors), **R4** — `.desktop` with `Terminal=true`; R5–R7 per
  the recommendation (no CLI banner, `artwork/` is the single source,
  AppStream — future work).
- **Stage 1 `feat/brand-assets`** — assets committed to git + a wordmark
  repair. **Key finding**: all six `mindfork-wordmark*.svg` carried
  `<text class="wm">` **with no `<style>`, `font-family`, or `font-size`**
  (the classes were never defined anywhere — the stylesheet block was lost
  during export), i.e. they rendered in a default serif font. Adding
  `font-family` wouldn't have helped: a viewer (GitHub, someone else's
  browser, a Linux viewer) doesn't have the needed monospace font. **The
  text was converted to outlines** (`<path>`) — the SVGs are self-contained.
  The typeface was reconstructed from the reference `wordmark-example.png`
  by **fitting metrics** (letter boundaries → font metrics, least squares):
  **JetBrains Mono ExtraBold** (SIL OFL 1.1), size 277.7 px, tracking
  −0.035 em. The reconstruction matches the reference **pixel-for-pixel** —
  word width 1249 px and the "icon → text" gap 213 px in both, individual
  letter widths diverging ≤3 px out of ~140 (anti-aliasing); a confirming
  detail — the descender of `o`/`d` (−10 font units) predicts their bottom
  edge exactly at the measured 348 px. The font was found locally (bundled
  with PyCharm) — no download needed; the `.ttf` isn't checked into the repo
  (needed only for regeneration). Added `artwork/build-wordmarks.py` (a
  generator; the palette, tracking, and lockup proportions are constants)
  and `artwork/README.md` (a brand guide: palette, glyph geometry, metrics,
  usage rules). The `viewBox` values were tightened to the actual content
  (`219.9×48` instead of `340×80` etc.) — the previous values assumed a size
  twice the reference and never rendered correctly; the vertical lockup was
  redrawn (word width = 2× the icon — no reference exists for it, and at the
  horizontal proportions the composition was bottom-heavy). The wordmark was
  placed in the README header (`<picture>` + `prefers-color-scheme`,
  falling back to the light variant; wrapped in an `<h1>` with `alt`,
  keeping the heading accessible).
- **Stage 2 `feat/windows-icon`** — the `.exe` icon via the **`winresource`**
  crate (a fork of the abandoned `winres`) from a ready-made `mindfork.ico`.
  **A double gate in `build.rs` is required**: the crate is declared under
  `[target.'cfg(windows)'.build-dependencies]`, and for **build**
  dependencies `cfg` is evaluated by the **host** — on a Linux host the
  crate is absent, and referencing it wouldn't compile; hence
  `#[cfg(windows)]` gating by host (is the crate present) **plus**
  `CARGO_CFG_TARGET_OS` by target (is the icon needed). Consequence:
  cross-compiling Linux → Windows won't embed the icon — a `cargo:warning`
  is printed there (the normal path is unaffected: the release workflow
  builds Windows on a windows runner). A failure to embed doesn't break the
  build (`cargo:warning`) — the app must still build on a machine without
  the Windows SDK. `SetupIconFile` and `WizardSmallImageFile` were added to
  `.iss` (`artwork/mindfork-wizard-small.png`, 138×140 — the size for the
  modern style, a white background under Inno's white header);
  `UninstallDisplayIcon` and the `[Icons]` shortcuts picked up the icon on
  their own.
- **Verified live**: `rc.exe` from the Windows SDK was found, the build
  produced no warnings; the icon was **extracted** from the built `.exe` and
  from `setup.exe` — both identical, carrying exactly the brand colors
  `#09090b`/`#5c6370`/`#c25a27`; the `.iss` compiled; the wizard was
  screenshotted (icon in the title bar + logo in the page header; the
  install was **not** actually run). **The worry "Inno 6 only accepts BMP"
  wasn't confirmed** — a test compile of both formats succeeded, PNG was
  chosen (35× lighter: 1.6 KB vs. 58 KB). `cargo deny check` —
  `advisories/bans/licenses/sources ok` (winresource pulls in 7 build-only
  crates: the toml stack + winnow). **1157 unit tests green** (count
  unchanged — the crate's own code untouched), 53 `#[ignore]`, clippy/fmt
  clean.
- **Stage 3 `feat/linux-desktop-entry`** — an application-menu entry and
  theme icons: `packaging/linux/mindfork-rs.desktop` (**`Terminal=true` is
  mandatory** — the app is a TUI, stdout is occupied by the interface;
  without it, launching from the menu would close the window instantly;
  localized `GenericName`/`Comment`/`Keywords` for ru) + `nfpm.yaml`:
  `.desktop` into `/usr/share/applications`, six raster icons into
  `hicolor/<N>x<N>/apps` and an SVG into `hicolor/scalable/apps` — all under
  the name `mindfork-rs` (must match the `Icon=` key). The `packaging.yml`
  smoke was extended with checks for the presence/content of `.desktop` and
  all icons; `desktop-file-utils` is installed on Ubuntu/Fedora so the stock
  `desktop-file-validate` **actually runs**, rather than being skipped by a
  `command -v` check. `Chat` is **deliberately absent** from the categories
  — per the spec it requires a primary `Network` category, otherwise the
  validator warns. No cache-refresh scriptlets were added: Debian (the
  `update-icon-caches` trigger) and Fedora (a file trigger on `hicolor`)
  handle it themselves. Checked as far as possible locally on Windows (the
  YAML is valid, every `src` path in `contents` exists, `.desktop` is UTF-8
  without a BOM, LF-only, a primary category is present); actual `nfpm`/
  installs in containers aren't reproducible on Windows — validated by
  `packaging.yml` on the PR.
- **Stage 4 `feat/tui-logo`** — a logo drawn in terminal cells. New
  `widgets/logo.rs`: a pair of vertical pixels is encoded as one cell (both
  halves the same color — `█`; different — `▀` with `fg`/`bg`; one — `▀`/`▄`
  **with no background**, so as not to drag in a solid backdrop). The size —
  bounded **by the ink** (x 3…13, y 2…14 → 10×12 pixels = **10 columns × 6
  rows**; the height is even, so half-blocks line up exactly), rather than
  the full 16×16 grid with empty margins. Colors — **fixed brand RGB, not
  from the palette** (R3): the logo isn't retinted by the theme. The glyphs
  `█`/`▀`/`▄` fall within WGL4 → conhost compatibility mode needs no
  separate substitution (like `█` for the scrollbar and `▌` for the rails).
  Placement — as the first content block of the help overlay (`F1`, whose
  title already carries the version → de facto an "About" screen, so no new
  key was needed), but **only when there's spare height**: the hotkey list
  is long (33 entries, a popup ~35 rows), so when there's no room the logo
  isn't drawn at all — the hotkeys aren't pushed around and no extra scroll
  appears (the same degradation as the scrollbar and Mermaid).
- **A "code ≡ asset" gate**: a test parses the `<rect>` elements from
  `artwork/mindfork-icon-transparent.svg` and cross-checks them against the
  `GLYPH` table in the code — the asset and the code won't silently drift
  apart (the technique mirrors i18n key-parity and `LABEL_CAP`).
  **Mutation-tested**: editing one constant fails the test with a clear
  message. **1164 unit tests green** (+7: 5 for the widget — including "only
  WGL4 glyphs" and "every row has a stem" — and 2 for showing/hiding in the
  help screen), 53 `#[ignore]`, clippy `-D warnings`/fmt clean. No live run
  needed (pure UI, `TestBackend`).
- **The "branding" track (stages 1–4) is complete.** Future work: a large
  `WizardImageFile` for the installer's "Finished" page, AppStream metainfo
  (R7), a CLI ASCII banner (R5 — decided against).

### Post-M9: branding — TUI wordmark, a lockup instead of a bare glyph (done)
- **A refinement of stage 4 based on user feedback** (branch
  `feat/tui-lockup`): the help overlay's header (`F1`) had a **centered glyph
  with no word and no top breathing room** — it read as a standalone image
  glued to the frame. Now there's a **horizontal lockup** (glyph + the word
  `mindfork`), left-aligned, with breathing room above and below.
  Assets/packaging untouched; changes only in `widgets/logo.rs` and
  `screens/chat/popups.rs`.
- **The wordmark is a custom pixel font, not the SVG.** Reusing
  `artwork/mindfork-wordmark*.svg` isn't possible: the text there **has been
  converted to outlines** (stage 1, §2 of branding.md) — there's nothing to
  rasterize them with in the terminal. So `logo.rs` gained a `#`/`.` matrix
  per letter (`WORDMARK`, 8 glyphs for the word), drawn with the same
  half-blocks as the icon. The lettering mirrors the brand (JetBrains Mono
  ExtraBold): lowercase, descenders on `d`/`f`/`k`, **2-pixel strokes** — the
  same weight as the icon's bars (its grid also uses 2-unit bars). The
  result — 52 columns × 4 rows; the lockup as a whole — **65×6**.
- **Proportions taken from the brand metrics, not eyeballed**
  (`artwork/README.md`): word height = `0.5227 × S`, where `S` is the
  **icon size** (16) → ≈ 8 pixels = 4 rows vs. 6; the baseline
  (`0.7418 × S`) → the word's bottom sits **one row above** the icon's
  bottom (`WORDMARK_TOP_ROW = 1`); the gap `0.3608 × S` is measured from the
  icon's **bounding box**, but we draw only the ink (x 3…13), so in the
  terminal it comes out to `0.3608 × 16 − 3` ≈ **3 columns**. Letter
  spacing — 1 column.
- **A fix from feedback on the first version** (too much air on the right;
  `i` twice as heavy as the other letters) — **one root cause**: the
  fraction `0.5227` was taken from the icon's **ink** height (12), not the
  icon size (16), so the word came out 6 pixels tall instead of 8. At 6
  pixels, 2-pixel strokes don't fit (an x-height of 4 leaves no gap inside
  `o`/`d`), and the font ended up effectively 1-pixel wide — thin next to
  the chunky glyph; `i` was the only letter with a proper 2-pixel stem,
  hence its "double width". Fixing the height to 8 unlocked the correct
  weight: the word became ExtraBold like the brand, and along the way grew
  from 48 to 65 columns — the air on the right shrank from ~26 to ~7 columns
  (against a popup width of ~76). Both complaints closed with one fix.
- **A second fix from feedback — `f`**: its hook (ascender height) and
  crossbar (x-height) sit right next to each other, with no separating row
  left in the budget — the 2-pixel hook merged with the crossbar into a
  solid block. The hook was made **1-pixel**: it lands in the top half of
  the cell (`▀`), the bottom half is empty → a gap shows, and the crossbar
  stays at x-height, on the same row as the top bars of `n`/`o`/`r` (an
  alternative — dropping the crossbar to the middle of the x-height — would
  have preserved the hook's weight but broken the overall baseline). `f`
  was widened 5 → 6 columns so the hook could be two columns wide (`▀▀`) and
  read clearly.
- **`mind`'s color comes from the palette, and this doesn't violate R3.**
  `fork` stays the fixed brand accent (`#c25a27`), while in the brand `mind`
  is "text on dark"/"text on light" (exactly why separate `-dark`/`-light`
  variants were set up), i.e. a color derived from the background. We take
  `palette.text` → a single lockup is correct in both themes, while the
  signature colors stay on-brand. The glyphs `█`/`▀`/`▄` are WGL4, so
  conhost compatibility mode still needs no separate substitution.
- **Left alignment** on the same margin as the hotkey list (two spaces),
  plus a blank line above (breathing room from the frame) and below. The
  mark reads as a block header and shares a vertical axis with the hotkeys.
  The block grew from `LOGO_ROWS + 1` = 7 to `LOCKUP_ROWS + 2` = 8 rows.
- **The show gate was extended to width**: previously only height was
  checked. The popup width under the lockup is **deliberately not
  stretched** (the popup is sized by the hotkey list) — when `width <
  LOCKUP_COLS + margin + padding`, the mark isn't drawn at all. The same
  hard degradation as the scrollbar and Mermaid. The order of computations
  in `render_help` was reordered: width first, then the decision on the
  mark (it depends on width), then the popup height.
- **The invariant "the word fits within the glyph's height" is a `const`
  assertion**, not a test: editing the font or `WORDMARK_TOP_ROW` in a way
  that would make `lockup_lines` silently clip the word's bottom rows
  **fails the build**. The "code ≡ SVG" gate for the icon (stage 4) stays
  intact.
- **Tests**: font integrity (the word is exactly `mindfork`, matrices are
  rectangular, no empty letters, no stray characters); word size and
  separate coloring of `mind`/`fork`; the lockup doesn't touch the icon (the
  first `LOGO_COLS` columns match byte-for-byte) and places the word
  **only** in its own 3 rows; `uses_only_wgl4_block_glyphs` extended to the
  lockup; the help-screen test rewritten from "there's orange somewhere" to
  positional — the stem sits exactly on the hotkey list's margin (i.e. the
  mark is left-aligned, not centered), "fork" is to the right of the icon.
  Test trap: `Span::content.len()` is **bytes**, and block glyphs are 3
  bytes wide (measure width via `chars().count()`); and `╭` appears in the
  buffer for the chat's own panels at column 0 — the popup's corner is
  taken as the rightmost one. **1167 unit tests green** (+3), 53
  `#[ignore]`, clippy `-D warnings`/fmt clean.
- **No live run needed** (pure UI without an engine/memory; covered by
  `TestBackend`). The composition was checked against a dump of the
  rendered popup in both locales (ru/en): the word reads correctly, the
  lockup fits within the popup width with margin (65+6 vs. ~78).

### Post-M9: emoji popup — a "hanging" selection ghost after closing (done)
- **Symptom** (branch `fix/emoji-picker-residue`): after closing the emoji
  popup (`Ctrl+B`), a ~1-column-wide colored rectangle (background
  `keycap_bg`) lingered at the previously selected cell, surviving
  subsequent redraws.
- **Cause found empirically** (a probe against real `Buffer::diff` in
  ratatui 0.30.1, not a guess): a wide emoji occupies **two** cells — its
  own (the glyph) and a **trailing** one that ratatui resets to the default
  (`" "`, the default style). When the popup closes, the frame "popup →
  empty" produces updates for cells `x` (the glyph) and `x+2` (the space to
  the right), but **not for `x+1`**: in both buffers it's the default
  space, so the diff considers it unchanged and doesn't send it to the
  terminal. conhost/Command Prompt itself doesn't clear a wide glyph's
  second half when the first half is overwritten → a piece of it stays on
  screen. It's visible only where the cell had a **background** — hence
  exactly the selected grid cell "glowed", not the whole popup.
- **Fix** — reused a mechanism already present in the project for a
  flicker-free full redraw (the buffer sentinel `"\0"` + `swap_buffers`,
  set up for "drifting" VS16 emoji when scrolling the feed): a probe
  confirmed that with the sentinel, diff emits **every** cell, including
  the trailing one. `ChatScreen` gained a flag `full_redraw` +
  `request_full_redraw()`; `handle_emoji_key` requests a redraw for **any**
  action in the popup — not just closing, but also shifting the selection
  via arrows too (the highlight likewise drifts off the previous cell,
  leaving the same artifact). Opening the popup requires no redraw (the
  glyph goes nowhere).
- **Generalizing the flag without losing the original source**: the
  `app/runtime` loop switched from `take_feed_scrolled()` to
  `take_full_redraw()`, which unconditionally pulls in **both** sources
  (not via `||` — `take_feed_scrolled` must reset the feed widget's
  internal state regardless of the second request); the previous VS16 path
  is additionally pinned by an assertion in an existing test.
- **Tests**: screen behavior (`emoji_picker_actions_request_full_redraw` —
  closing via `Enter`/`Esc` and shifting the selection request a redraw,
  the flag is consumed exactly once, opening — no request;
  **mutation-tested**: removing `request_full_redraw()` fails the test) +
  **a root-cause canary**
  (`wide_glyph_trailing_cell_is_not_repainted_by_plain_diff` in
  `widgets/emoji_picker`): pins that a plain diff skips the trailing cell
  while the sentinel repaints it; the test failing would mean upstream
  fixed this on its own and the workaround could be reconsidered. **1169
  unit tests green** (+2), 53 `#[ignore]`, clippy `-D warnings`/fmt clean.
  No live engine run **needed** (a pure UI fix, no engine/memory involved);
  visual verification on conhost is left to the user.
- **A related case closed at the user's request: the spellcheck suggestion
  popup (`Ctrl+G`).** First **verified by a probe**, rather than "just in
  case": the hypothesis was that there was no defect since `List` applies
  highlighting via `buf.set_style(row_area, …)` **after** rendering the
  content — seemingly covering the trailing cell too. The probe showed the
  opposite: the glyph `➕` = `mod=ITALIC | REVERSED`, while its **trailing
  cell** is `mod=NONE`, and it isn't repainted on close. So the class is
  the same as with emoji (the highlight doesn't reach the trailing cell),
  and a fix was needed. Added `request_full_redraw()` to
  `handle_suggest_key` — **more precise than for emoji**: only when an
  action actually changes something (`Esc`/`Enter`/`↑`/`↓`), other keys
  (`_ => return`) don't request a redraw. Test
  `suggest_popup_actions_request_full_redraw` (shifting the selection /
  both closing paths request it, a no-op key doesn't; **mutation-tested**).
  `➕` isn't VS16 (U+2795), so the row shift seen with `❤️` doesn't occur
  here.
- **A second iteration, based on a user report (same branch): VS16
  clusters removed from the popup list.** A full redraw uncovered an
  adjacent defect: after moving the selection through the grid, `🔥` would
  disappear and the frame "broke" in two spots (rows 3 and 4 — exactly
  where `✌️` and `❤️` sat). **Mechanics** (confirmed by a probe against
  real `Buffer::diff` + reading the backend, not by guesswork): for a VS16
  cluster, `ratatui` **deliberately** emits the glyph's trailing cell (a
  workaround for terminals that don't clear the second half), and
  `CrosstermBackend::draw` tracks `last_pos` **by cell number, without
  accounting for glyph width** (`x == p.x + 1` → no `MoveTo`) — the
  trailing cell prints one column to the right, and the rest of the row
  drifts: the following wide emoji gets its right half overwritten
  (conhost blanks the whole glyph → `🔥` disappeared), and the frame gets
  pushed outward. Without the sentinel the trailing cell wasn't emitted at
  all (the glyph didn't change: space → space), which is why the defect
  only surfaced after the first iteration. **Fix at the source**:
  `✌️`→`🤞`, `❤️`→`💖` (supplementary-plane characters, honest 2 columns, no
  trailing cells produced); the rest of the set untouched. Widths were
  checked programmatically (`unicode-width` 0.2 counts a VS16 cluster as 2
  — the grid model was consistent, the bug is specifically in terminal
  output).
- **Two gates for the invariant** (both **mutation-tested** by
  reintroducing `❤️` into the list): `emoji_list_is_width2_without_vs16` —
  a direct one (width 2, no U+FE0F, a message suggesting a replacement) and
  `sentinel_repaint_never_writes_into_second_half_of_wide_glyph` —
  behavioral and terminal-agnostic: during a full redraw, no diff update
  targets the second half of a wide glyph. **1174 unit tests green** (+7
  over `main`), 53 `#[ignore]`.
- **Third iteration: the same defect found and fixed in the feed — at the
  sentinel level.** Checking a deferred risk (at the user's request): the
  feed's long-standing scrolling path uses the same technique and is
  triggered **specifically when VS16 is present**. A probe against a real
  feed frame (40 lines with `🗂️`): during a **normal** scroll — **0**
  updates into the second half of a wide glyph, **via the sentinel — 6**,
  exactly matching the number of VS16 cells in the frame. So the row shift
  in the feed was being triggered by the sentinel itself.
- **A fix at the mechanism, not the content** (unlike the emoji list): the
  sentinel was factored out into a named `shared/ui.rs::prime_full_redraw`
  and redesigned — instead of the symbol `"\0"`, a cell gets **a space +
  the `HIDDEN` marker modifier**. The space matches the content of a wide
  glyph's trailing cell (which `ratatui` resets to default), so the
  condition for emitting the trailing cell
  (`prev.symbol() != next.symbol()`) no longer triggers — no shift; while
  the modifier difference preserves full redraw coverage. `HIDDEN` is
  unused elsewhere in the UI (the palette works with
  `DIM`/`BOLD`/`ITALIC`/`UNDERLINED`/`REVERSED`) — pinned by test
  `sentinel_modifier_is_unused_by_ui`. Measurement: **1194 out of 1200**
  cells repainted, the uncovered ones are exactly 6 trailing halves (they're
  already covered by the glyph itself — no need to write there).
- **The gate moved to the mechanism**:
  `prime_full_redraw_repaints_all_but_wide_glyph_tails` (`shared/ui.rs`) on
  a synthetic frame with VS16 checks both properties — (1) no update
  targets the second half of a wide glyph, (2) no gaps in the repaint (the
  only uncovered cells are trailing ones). **Mutation-tested**: reverting
  the sentinel to `"\0"` fails it with a precise message. Emoji popup tests
  switched over to the real helper instead of their own copy of the
  sentinel. **1177 unit tests green** (+10 over `main`), 53 `#[ignore]`.
- **Fourth iteration: the full-redraw trigger extended from scrolling to a
  change in feed content.** A detailed conhost run by the user (a chat
  about the mechanics of emoji — VS16, ZWJ families, surrogate pairs)
  showed: artifacts show up on **streaming output** and on **adding a
  note** (`F5` "Conversation copied…", with the feed scrolling down at the
  time), while **scrolling fixes them**. The cause is systemic: only
  scrolling was requesting a full redraw (`take_feed_scrolled`), while
  `scroll_to_bottom` didn't raise the flag at all — a content change had
  no trigger.
- **How "change" is defined (the outcome of the second attempt)**: the flag
  is set by the feed's **own mutators** (`ChatScreen::mark_feed_changed` in
  `activate_chat`/`push_*`/`continue_`/`rewrite_assistant`/`Ctrl+T`). The
  first attempt defined a change **based on render** (a miss in the feed's
  block cache) — it worked, but was one frame late: an artifact would
  still **flash** ("defects appear for a split second and vanish" per the
  user's report). A bad frame can't be hidden behind synchronized output —
  on conhost, where the problem lives, mode 2026 is ignored. So we
  determine it **before** the render: `take_full_redraw()` in the loop sees
  the flag already raised, and the bad frame never gets drawn. The
  render-time plumbing (`MessageFeed.content_changed`) and the "catch-up"
  frame in the loop were removed.
- **The risk flag is cached** (`ChatScreen.feed_has_risky`) and only
  accumulates: edits always touch the last block, so `mark_feed_changed`
  checks it, while `activate_chat` (a full feed replacement) recomputes
  from scratch — otherwise scanning the whole feed on every streamed chunk
  would be O(the entire chat's text). Over-estimating is safe (an extra
  redraw isn't visible, a missed one leaves an artifact).
- **The risk of this approach is closed by a gate**
  `every_feed_mutator_marks_content_change`: it runs **all** feed mutators
  (+`Ctrl+T`) and requires a redraw request — a forgotten call would
  reintroduce the flash. **Mutation-tested** (removing the call in
  `push_chunk` fails the test with the mutator's name in the message).
- **The detector was expanded** from VS16 to a risk group
  (`feed::is_risky_glyph`): VS16, **ZWJ** (`👨‍👩‍👧` — on the screenshots
  it's precisely these that produce the worst mess), skin-tone modifiers,
  supplementary-plane pictographs (`😀`), width-2 BMP emoji (`✅`/`⭐`).
  **CJK is deliberately excluded** — terminals render ideographs
  consistently, no point triggering a full redraw for them on every chunk.
  Cost: streaming text with emoji makes every frame a full redraw (plain
  text — unchanged from before).
- **Not closed: "the background inverts on half a glyph"** (the report's
  third symptom). The mechanics differ and **lie upstream**: `ratatui`
  resets a wide glyph's trailing cell to the **default style** (confirmed
  by a probe: the glyph cell has `bg=Red`, the trailing cell has
  `bg=Reset`) and omits it from the diff, while conhost never sets the
  attribute of the second half itself — half the glyph keeps the old
  background. A full redraw doesn't fix this (the cell simply isn't
  emitted). It'd need to be fixed either in `ratatui` (having the trailing
  cell inherit the glyph's style) or in `ratatui-crossterm` (tracking
  `last_pos` with glyph width in mind — then emitting the trailing cell
  would stop shifting the row and become safe). A candidate for an
  upstream PR; there's precedent (mermaid-text #29/#30).
- **Consequence**: replacing `❤️`/`✌️` → `💖`/`🤞` in the popup is now
  **not strictly required** (the shift is fixed by the sentinel), but stays
  as defense in depth — VS16 clusters in a fixed-width grid remain a
  fragile class, and the list gate pins this.

### Post-M9: ratatui-core/crossterm update 0.1.1 → 0.1.2 (done)
- **A dependency update along with a review of the workarounds** from the
  previous PR (#189, emoji artifacts on conhost); branch
  `chore/ratatui-0.1.2`. Forks confirmed by the user on 2026-07-20 (narrow
  the redraw down to closing; keep the VS16 emoji replacement).
- **The 0.1.2 release's scope is exactly one functional change** (checked
  against the crates' source, not the release notes): `ratatui-core/src/
  buffer/diff.rs`; `backend.rs` — doc changes only (mentions of Termina),
  while **`ratatui-crossterm` is byte-for-byte identical to 0.1.1**. Hence
  the narrow regression surface.
- **What exactly 0.1.2 fixes** (ratatui#2585, restored `invalidated` in
  diff): a wide glyph's trailing cell is now emitted **when**
  `previous_width > cell_width` **AND** "the style is visible on an empty
  cell" (`bg != Reset` or `REVERSED`/`UNDERLINED`/`SLOW_BLINK`/
  `RAPID_BLINK`/`CROSSED_OUT`). I.e. it closes exactly **our** observed
  defect — the selection-backdrop ghost when **closing** the popup.
- **What's NOT fixed** (verified by probes against `Buffer::diff`, not
  assumption):
  1. **Unstyled trailing cells** — in the emoji popup's grid, 43 of 44
     glyphs go without emitting a trailing cell (`styled emitted=1
     missed=0 | unstyled emitted=0 missed=43`).
  2. **Shifting the selection** — the glyph stays wide, the condition
     `previous_width > cell_width` doesn't hold, no trailing cell is sent
     (confirmed both on the emoji popup and on `➕ add to dictionary`:
     `ITALIC|REVERSED` → `tail_close=true tail_move=false`).
  3. **The backend's `last_pos` ignoring glyph width** (ratatui#2651, the
     row shift) — the source wasn't touched at all.
- **A key finding that changed the plan** (the task assumed the popups'
  redraw could be removed entirely): for **shifting the selection**, a
  full redraw provably **accomplishes nothing** — the sentinel must skip a
  wide glyph's trailing cell (otherwise the backend would print half of it
  without `MoveTo` and shift the row, per item 3), so it was a no-op there
  from the start; the backdrop is actually cleared by the glyph itself
  being reprinted via a plain diff. So the redraw request was **narrowed
  to closing** the popups (emoji and spelling), not removed and not kept
  for every action. It stays for item 1 (unstyled trailing cells), which
  upstream doesn't cover.
- **The canary was rewritten to match the new boundary** (the old
  `wide_glyph_trailing_cell_is_not_repainted_by_plain_diff` was failing —
  by design, since it was set up for "upstream fixed it itself"):
  `upstream_repaints_styled_tail_on_close_but_not_on_selection_move` pins
  **both** sides — (A) upstream sends a styled trailing cell on close
  (fails if upstream regresses), (B) it doesn't on a shift (fails once
  upstream also fixes that case → then the workaround should be removed),
  plus that the sentinel recovers the trailing cell on close and
  **doesn't** touch it on a shift.
- **Left unchanged** (with justification): `prime_full_redraw` and its
  "space + `HIDDEN`" design — the invariant "never write into the second
  half of a wide glyph" rests on the unfixed #2651 and became **more**
  important, not less; `mark_feed_changed` + `is_risky_glyph` — the feed's
  text is unstyled (`bg = Reset`), the 0.1.2 fix doesn't apply to it;
  replacing VS16 emoji (`💖`/`🤞`) and the
  `emoji_list_is_width2_without_vs16` gate — defense in depth, while #2651
  remains open.
- Popup tests were rewritten under the narrowed contract and
  **mutation-tested in both directions** (bringing back the redraw on
  shift → the shift assertion fails; removing it from close → the close
  assertion fails). **1177 unit tests green** (count unchanged), 53
  `#[ignore]`, clippy `-D warnings`/fmt clean. Docs: architecture.md §4
  (the boundary with upstream + issue numbers), CHANGELOG (`[Unreleased]`
  wording aligned to the actual mechanics — the mention of "moving across
  the grid" removed).
- **No live engine run needed** (rendering, no engine/memory involved). **A
  manual regression pass on conhost was run by the user**: popups
  (emoji/spelling), Mermaid, and the feed stream — clean, narrowing the
  redraw introduced no regressions. A separate defect of the same class
  was found (below).

### Post-M9: a full redraw on screen switch and in the input box with VS16 (done)
- **Symptom** (a manual conhost regression pass after the ratatui update):
  in a feed with `❤️`, switching to the chat list / `F3` and **back** would
  add an extra space after the emoji; it went away on scroll. Same branch
  `chore/ratatui-0.1.2`.
- **Cause — the VS16 branch of diff + the unfixed ratatui#2651**
  (reproduced by a probe against `Buffer::diff`, all three observations
  matching the report): for a VS16 cluster, ratatui emits the trailing
  cell **when its symbol has changed**. Within a single screen, feed edits
  don't touch the trailing cell (`same_screen=false` — which is why the
  bug wasn't visible during normal use), but on **returning from another
  screen** it held a foreign symbol (`switch=true`) → the trailing cell is
  sent to the terminal, the backend prints it with no `MoveTo` (position
  is tracked by cell number, without glyph width) → the rest of the row
  drifts right. Scrolling "fixed" it because it requests a full redraw,
  while the sentinel **doesn't** send the trailing cell (`sentinel=false`)
  — exactly the property for which it's built as "space + `HIDDEN`".
- **Fix — a full redraw on SCREEN SWITCH** (`app/runtime/mod.rs`),
  centralized: `prime_full_redraw` was hoisted above the `match`, out of
  the `ActiveScreen::Chat` arm; the condition is `requested || switched`,
  where `switched` = a change in `std::mem::discriminant(&active)` **as
  observed after rendering** (switching "there and back" between frames
  changes nothing visually). A one-place fix instead of 8+
  `*active = ActiveScreen::…` sites; covers any direction and any screen
  (VS16 can appear both in a chat title in the list, and in the `F3`
  narrative). The chat's flag is only pulled while the chat is active
  (otherwise it would be lost).
- **The same class was found and closed in the input box** (via probing,
  not reported by the user): an edit **to the left of** `❤️`, immediately
  followed by a non-space character, moves the glyph onto a foreign cell
  → the trailing cell gets emitted → the row shifts. `mark_input_changed`
  now requests a full redraw when the field contains a risk-group glyph;
  the check is streaming — a new `InputBox::any_char(pred)` (the caller
  supplies the predicate, the widget doesn't know about upper layers —
  FSD), avoiding building `text()` on every edit. For ordinary text — a
  no-op.
- **Tests**: `shared::ui::screen_switch_emits_vs16_tail_without_full_redraw`
  pins the mechanics (a screen switch sends the trailing cell; an edit
  within a screen — doesn't; the sentinel — doesn't) and explains why a
  screen switch must go through a full redraw;
  `input_with_risky_glyph_requests_full_redraw` — the field's behavior
  (mutation-tested). **1179 unit tests green** (+2), 53 `#[ignore]`,
  clippy `-D warnings`/fmt clean.
- **What remains upstream**: half the background of wide glyphs (the
  trailing cell carries the default style, and conhost never sets its
  attribute) — not fixable locally, the cell is marked `skip` and doesn't
  reach the diff. Same root as ratatui#2651; would be fixed either by
  having the trailing cell inherit the glyph's style, or by tracking
  `last_pos` with width in mind (then trailing-cell emission would stop
  shifting the row and become safe).

### Post-M9: spellcheck popup — selection style now matches the rest of the lists (done)
- **Symptom** (branch `fix/suggest-popup-selection-style`): the spellcheck
  suggestion popup (`Ctrl+G`) highlighted the selected item by **inverting
  the whole line** (`highlight_style(Style::new().reversed())`) and with no
  rail, while every other list in the app had long since settled on a
  different style: **a soft backdrop** `bg(palette.keycap_bg)` + **a green
  rail `▌`** (`palette.success`) on the selected line — the chat list
  (`widgets/chat_list.rs`, `item_line`), the settings screen's section
  menu and fields (`screens/settings/render.rs` + `render_field_line`),
  the "self-model" screen (`screens/self_model.rs`). The popup stood out
  from the rest.
- **Why this style, not reverse video** (the reasoning was already
  recorded for the settings section menu and carried over into the new
  code): `reversed()` swaps `fg↔bg` **per span independently** — the rail
  `▌` (a left half-block) ends up smeared over ~1.5 columns, and different
  spans of the line get different backgrounds. A uniform backdrop
  `keycap_bg` + a rail on top of it read cleanly.
- **The fix** (`screens/chat/popups.rs::render_suggest`): items are built
  via `.enumerate()`, the selected one gets a rail prefix `▌ ` in
  `success` color, the others get a matching-width indent (the text
  doesn't "jump" as the selection moves); `highlight_style` →
  `Style::new().bg(palette.keycap_bg)`. The selection index
  (`popup.selected.min(len-1)`) is computed once and reused for the
  `ListState`. `▌` is WGL4 — conhost compatibility mode still needs no
  substitution (like the feed's role rails and the `█` scrollbar).
- **A comment was tightened along the way** in `handle_suggest_key`: the
  full redraw fallback when closing the popup used to be justified by the
  selected line carrying `REVERSED`. The `keycap_bg` backdrop is, for
  ratatui 0.1.2, just as "visible on an empty cell" a style
  (`bg != Reset`, see architecture §4), so the mechanics are unchanged,
  but the wording now matches the fact.
- **Tests**: `suggest_popup_selection_matches_other_lists` (rendered in
  `TestBackend`: the rail is present and green, the backdrop is
  `keycap_bg`, no cell of the selected line is reversed).
  **Mutation-tested** — reverting to `.reversed()` fails it. **1180 unit
  tests green** (+1), 53 `#[ignore]`, clippy `-D warnings`/fmt clean. **No
  live run needed** (pure UI without engine or memory involved, covered
  by `TestBackend`).

### Post-M9: API keys in settings — stage 1 (core: machine-bound storage) (done)
- **A new track** (at the user's request): cloud keys (OpenAI/Gemini/Claude)
  should be entered **in the settings window**, not via env variables
  ("ordinary users don't really understand environment variables"), and
  stored in the config **securely** — encrypted with a **machine key**,
  while keeping the config portable: scenario A/B/A (entered on A → moved
  the config to B → re-entered there → back on A → the keys are still
  readable). Research doc
  [docs/research/api-key-storage.md](docs/research/api-key-storage.md);
  forks D1–D6 **adopted by the user, per the recommendations, on
  2026-07-20**. Branch `feat/api-key-store` (stacked on
  `docs/api-keys-research`). Stage 1 is **the core** (a UI input field —
  stage 2), so there's no user-visible effect yet.
- **Format — a list of entries, one per machine** (`AppConfig.api_keys:
  Vec<ApiKeyEntry>`; `#[serde(default)]` + `skip_serializing_if` →
  additive, **no migration or schema bump**, per the ADR 0006 F12 policy).
  An entry carries `label` (a PC name + date, human-facing only),
  `scheme`, `check`, and `keys: provider → ciphertext`. **Recognizing
  "our" entry is done by decrypting the `check` probe**, rather than by
  storing a machine id: a machine identifier doesn't need to appear in a
  portable config, and it's not needed for Windows paths either. Foreign
  entries are left untouched (they'll come alive on their own machines);
  an entry with an **unfamiliar scheme** is read, kept, and simply
  treated as foreign — the format is extensible (future work: an OS
  keychain as another `scheme`).
- **Two schemes** (`shared/secrets.rs`, a new module): **`dpapi`**
  (Windows) — the system's `CryptProtectData`/`CryptUnprotectData` (a
  *user*'s key managed by the OS; decryption by another user/on another
  machine is impossible; `pOptionalEntropy` is an app-level constant).
  **`machine-key-v1`** (Linux) — HKDF-SHA256 over `/etc/machine-id` (the
  `sd_id128_get_machine_app_specific` pattern — systemd explicitly warns
  against using the raw machine-id; a fallback to
  `/var/lib/dbus/machine-id`) + ChaCha20-Poly1305 (AEAD, a random nonce as
  a prefix), the username in `info` → per-user binding, like DPAPI. No
  machine-id → the scheme is unavailable, leaving the env fallback in
  place. The ciphertext is encoded as **hex** (a hand-rolled codec — a
  precedent is `sandbox_setup::hex_lower`; a base64 dependency isn't
  needed, and the length difference doesn't matter for a config).
- **The "stored → env" resolution** (fork D3): `resolve_api_key(stored,
  api_key_env)` (`app/supervisor.rs`) — a stored key takes priority
  (entered by an explicit action, the target user never sees env), env
  stays a fallback (CI, power users, systems without machine-id).
  Decryption lives in `EngineManager` (`stored_key(api_keys, provider)`),
  while the supervisor accepts an already-decrypted `stored_key:
  Option<&str>` — **the server-launch layer knows nothing about the
  secret-storage format**, and its tests don't need encryption (a
  deliberate deviation from the design doc, noted there). Keys are
  **provider-centric**: one OpenAI key serves chat + impersonation +
  embeddings (removing the previous "specify the env name three times");
  an external proxy stays env-only (D4 — an arbitrary provider URL can't
  be bound to it).
- **Command `AppCommand::SetApiKey { provider, key }`** →
  `handle_set_api_key` (`orchestrator/settings.rs`): encrypts, places it
  into this machine's entry, persists it (`save_config`, rolled back on
  failure), flags a deferred (re)startup for **only the** slots whose
  active provider's key changed (the same `RestartQueue` debounce used for
  engine settings edits). An empty key means removal; an emptied entry
  gets dropped. The plaintext lives only in the argument and in the HTTP
  client.
- **Round-trip protection**: `handle_update_config` restores `api_keys`
  from the previous config — the UI snapshot never carries them, otherwise
  editing any setting would wipe the keys (precedents —
  `last_active_chat`, MCP TOFU-pin inheritance).
- **i18n**: `ui.err.server.no_api_key_env` → `ui.err.server.no_api_key`
  ("enter a key in settings or set an environment variable"); a new
  `ui.err.api_key_save_failed` (ru+en; the i18n parity/no-dead gates
  covered it automatically).
- **Dependencies**: `chacha20poly1305` + `hkdf` (RustCrypto, pure Rust,
  licenses already in `deny.toml`'s allowlist); `windows-sys` gained the
  `Win32_Security_Cryptography` feature (the crate was already present —
  the sandbox's Job Object). No new C dependencies.
- **The threat model is documented honestly** (§3 of the doc, module doc):
  we protect the **file** — moving/copying/backing up/syncing the config
  (outside its own machine it's a useless ciphertext), and on Windows
  also from other users of the machine. It doesn't protect against code
  running under the same user — it would call the same DPAPI; this is
  fundamental for any scheme where "the app decrypts on its own" (that's
  how Chrome and Git Credential Manager work too). The previous env-based
  path was no safer.
- **Tests**: `secrets` (7 — hex round-trip and rejecting garbage; AEAD
  round-trip; **rejecting a foreign key/a different user/a corrupted
  one**; nonce randomness; `derive_key` determinism and domain
  separation; a full write-then-read cycle on **the live platform
  scheme** — on this machine that's real DPAPI; a foreign entry and an
  entry with an unfamiliar scheme are ignored and **not overwritten**);
  supervisor (2 — priority of a stored key over env and working
  **without** `api_key_env`; a cloud mode reaches `Ready` on a single
  stored key); orchestrator (3 — the key persists as **ciphertext**
  (asserted: "no plaintext appears in `settings.json`") and reads back
  correctly; editing settings doesn't wipe keys; an empty key deletes the
  entry). **1192 unit tests green** (+12), 53 `#[ignore]`, clippy
  `-D warnings`/fmt clean.
- **No live engine run needed** (client protocols untouched — the same
  key goes into the same header; engine/memory unaffected). The DPAPI
  path was actually verified: the full-cycle unit test was run on a live
  Windows machine. The Linux path: the pure scheme core
  (`derive_key`/`encrypt_with_key`/`decrypt_with_key`) is covered by
  tests and run; the platform's ikm source (`/etc/machine-id`) will be
  verified by the `ubuntu` CI job (a Windows host only builds its own
  target).
- **Next — stage 2 (`feat/api-key-ui`)**: a masked `InputBox` mode, an
  "API key" field with a "configured (this computer)" status in the cloud
  subsections, "configured" flags in `AppEvent::Settings`,
  `SettingsIntent::SetApiKey`, README/spec/install.md/CHANGELOG + an
  **ADR** summarizing the track.

### Post-M9: API keys in settings — stage 2 (UI: input field, masking, status) (done)
- Completion of the track (branch `feat/api-key-ui`, stacked on
  `feat/api-key-store`). Cloud keys are now entered **in the settings
  window** — env variables are no longer needed. The track's conclusion is
  captured in **ADR 0008** (refining ADR 0004: "no secrets on disk" → "no
  secrets on disk **in plaintext**").
- **A masked `InputBox` mode** (`set_mask`, modeled on `single_line`):
  characters render as `•`, `selected_text()` returns `None` (a secret
  can't leak via copying — deleting a selection still works, going
  through `remove_selection`); enabling it switches the field to
  single-line mode (otherwise the multiline render path would reveal the
  content). **The mask substitutes characters before width calculations**:
  `•` has width 1, so a wide glyph inside a secret (an emoji from the
  clipboard) doesn't offset the cursor relative to the visible text.
- **The "API key" field** in the cloud subsections
  Assistant/Impersonation/Embeddings (`FieldId::XApiKey`/`IxApiKey`/
  `EApiKey`, row `api_key_row`): the value is a **status** ("configured
  (this computer)" / "not set" / "unavailable on this system", when
  there's no machine-id), not a secret. `Enter` opens an **empty** masked
  editor (a stored key can't be shown — even the screen doesn't have it),
  `Del` deletes it. Committing doesn't go into the config's working copy,
  but becomes the intent `SettingsIntent::SetApiKey` →
  `AppCommand::SetApiKey` (a branch in `apply_text` ahead of the
  `field_spec` table; `reset_field` has its own branch, since comparing
  against the default doesn't apply — the value shown is a status). The
  field's provider is derived from **its own** engine's mode
  (`api_key_field_provider`) — the key is shared across all three slots.
- **Secrets never leave the backend for the UI at all** (a hardening
  beyond the initial design): `emit_settings` **strips** `config.api_keys`
  in the snapshot and adds flags `api_keys_present: Vec<CloudProvider>`;
  the screen keeps only these (`set_api_keys_present`). So the UI doesn't
  even carry ciphertexts around, and the stage-1 round-trip protection
  (`handle_update_config` restoring keys from the previous config)
  becomes not "insurance" but a required link.
- **i18n**: field/statuses/descriptions (`ui.settings.field.api_key`,
  `ui.settings.value.key_set|key_unset|key_unsupported`,
  `ui.settings.desc.api_key|api_key_unsupported`) in ru+en; the
  parity/no-dead gates covered it automatically.
- **Tests**: `input_box` (2 — masking: only `•` shows on screen, the
  secret isn't in the buffer, `selected_text` is empty, deletion works;
  width/cursor with a wide glyph); `settings` (4 — the status field
  appears only in cloud modes and reacts to the presence flag; the editor
  opens **empty and masked**, committing yields `SetApiKey`, and the
  secret **never lands in the screen's config**; `Del` deletes only when
  a key is present; each slot's field addresses its own engine's
  provider); orchestrator (1 — the snapshot carries flags, but
  `config.api_keys` is empty). Stage-1 tests were switched from
  `config.api_keys` to `api_keys_present`. **1199 unit tests green** (+7),
  53 `#[ignore]`, clippy `-D warnings`/fmt clean.
- **No live engine run needed** (pure UI + the key path: client protocols
  unchanged). The DPAPI cycle was already verified for real in stage 1;
  the Linux scheme is checked by CI. A full manual scenario (entering a
  key → a cloud chat → moving the config) is still left to the user — it
  needs a real terminal and a live key.
- **Future work** (ADR 0008): an OS keychain as an additional `scheme`;
  keys for the external proxy and MCP-server env maps via the same
  mechanism; UI management of entries from other machines ("forget this
  computer").

### Post-M9: chat message speech (TTS) — OpenAI/Gemini/external (done)
- **The track was restored from a reverted stage 1** (`git show 19ead9f` —
  the `pulldown-cmark` extractor, the `/tts` command, the `rodio` player,
  stop points, the settings tab, clients) after **a re-investigation via a
  live spike** ([docs/research/tts.md §13](docs/research/tts.md)): the
  previously implemented engines didn't deliver acceptable quality on real
  text, so the track was reverted (PRs #197–199), then candidates were
  checked on live hardware. **DECISION (2026-07-23): the primary engine is
  OpenAI TTS** (`gpt-4o-mini-tts`, default voice **`onyx`** — verified by
  the spike: ~1 stress error per paragraph, better than every local
  option; loanwords handled natively; no drift). The key product argument:
  **not a new vendor** — the OpenAI key is already stored (ADR 0008), no
  new sign-up needed (ElevenLabs/Azure would require one). **The stage's
  scope — OpenAI + Gemini + external** (three `TtsMode` modes); **no
  local sidecar**. Forks D1–D9 (§10) remain valid behaviorally; only the
  engine choice was revisited.
- **A local managed sidecar — future work, not "stage 2"**: the spike
  showed local engines are either NO-GO for Russian (Qwen3-TTS, Supertonic
  3 — wrong stresses, an accent/drift), or need their own Rust frontend
  (vosk-tts — GO with caveats: 22 kHz, a custom Russian frontend needed
  for a Python-free distribution). For an offline/non-OpenAI audience —
  the `external` mode (any OpenAI-compatible TTS server:
  Kokoro-FastAPI/speaches/LocalAI). The managed mode wasn't added to the
  selector (precedent: `Claude` before `AnthropicClient` — we don't ship a
  non-functional entry).
- **Config `TtsSettings`** (`shared/config.rs`, field `AppConfig.tts`), all
  `#[serde(default)]` — no migrations: `mode: TtsMode
  {OpenAi(default)/Gemini/External}`; `openai`/`gemini: TtsCloudSettings`
  (model/voice/instructions/api_key_env/url); `external:
  TtsExternalSettings` (url/model_name/voice/api_key_env); `speed:
  f32=1.0`; `speak_roles=false`; `stop_on_chat_switch=true`;
  `stop_on_generation_start=false`. Defaults: OpenAI
  `gpt-4o-mini-tts`/`onyx`, Gemini `gemini-2.5-flash-preview-tts`/`Kore`.
  The cloud key is **shared with chat** (ADR 0008), the env-variable
  fallback preserved. **`speed` is effectively ignored by
  `gpt-4o-mini-tts`** (a known defect) → speed is requested via words in
  `instructions`.
- **Clients** (`shared/tts/`): `openai.rs` — `POST {base}/audio/speech`,
  serving **both** the OpenAI cloud (format `pcm`, s16le 24 kHz, bypassing
  a decoder) **and** a third-party server (format `wav`, the most
  portable); the `instructions` field only for the cloud; unset fields
  aren't sent (`skip_serializing_if`). `gemini.rs` — native
  `generateContent` with `responseModalities:["AUDIO"]` (the same
  transport as `GeminiClient`), PCM from `inlineData`; **a mandatory
  read-aloud directive** (default `Read this text aloud verbatim`) to
  avoid the `400 Model tried to generate text…` error. Known Gemini
  issues: 503 overload and voice drift across chunks (hence an opt-in, not
  the default). Input limits: OpenAI 4096, Gemini/External 2000
  characters.
- **Speech extractor** (`shared/markdown/speak.rs`,
  `speakable_text(markdown, loc)`) — a second walker over `pulldown-cmark`
  events (ADR 0003), `Writer` untouched: code blocks, ` ```mermaid `,
  tables, display math ($$…$$) **are skipped with a voice marker** in the
  profile's language (axis A); inline code is read as text, inline math →
  `latex_to_unicode`, links → text (the URL is dropped). Thoughts (CoT)
  and tool blocks aren't spoken (they're absent from `Message.text`).
  Markers/role prefixes are in the **profile's language** (axis A, speech
  content, not UI chrome).
- **Player** (`shared/tts/playback.rs`, `rodio`): the `Player::append`
  queue gives a pipeline of "synthesizing N+1 while N plays" for free;
  headless/CI (`open_default_sink` → `Result`) — graceful degradation
  "audio unavailable", not a panic (the `UnavailableEmbedder` pattern,
  test `open_degrades_gracefully_without_audio_device`).
- **Orchestration** (`app/orchestrator/tts.rs`, mirroring `rag.rs`): the
  orchestrator (the owner of `Chat`) takes a conversation snapshot →
  selection by scope (`build_utterances`, a snapshot at command time —
  works while generation is running too, no generation gates) → chunking
  (`chunk_utterances`/`pack_sentences`: sentences from
  `rag::split_sentences` are packed into a chunk up to the provider's
  limit, a short message → **one request** — no "growing pauses" as seen
  in the spike; packing follows speech blocks) → a cancellable background
  task (`tts_cancel: CancellationToken`, cancelled by a new command/
  `Quit`). **Stop points**: per setting — switching chats
  (`stop_on_chat_switch`) and starting generation
  (`stop_on_generation_start`); **unconditionally** — deleting an
  exchange (`Ctrl+E`), regeneration (`Ctrl+R`), deleting a chat (the text
  being spoken no longer exists). Contract: `AppCommand::Tts(TtsScope)`/
  `TtsStop`, `AppEvent::TtsActive(bool)`.
- **UI**: a command `/tts`/`/tts N`/`/tts all`/`/tts stop` (parser
  `features/tts_command.rs`, case-insensitive, `TtsScope
  {Last/Recent(n)/All}`; an error carries **the argument itself**, the
  hint text is built by the UI locally), command highlighting in the
  input box, the help overlay, a quiet chip **"♪ speaking"**
  (`ui.status.speaking`, the note U+266A is WGL4 → unaffected in
  compatibility mode), a **"Speech"** tab in the "Model" settings
  section: an "Engine" group (mode-driven fields
  engine/voice/speed/instructions; the key via the shared provider-status
  field from ADR 0008) + a "Behavior" group (toggles
  `speak_roles`/`stop_on_chat_switch`/`stop_on_generation_start`).
- **Dependencies**: `rodio` (+`cpal`/`symphonia`, MPL-2.0 already in
  `deny.toml`'s allowlist) and `base64`. A new **system** dependency —
  ALSA on Linux: build-time `libasound2-dev` (apt steps in
  `ci.yml`/`release.yml`/`packaging.yml`), runtime `libasound2t64 |
  libasound2` (deb; a time_t64 trap — a virtual dependency would
  otherwise pull in an OSS shim with no symbols) / `alsa-lib` (rpm/arch,
  installed explicitly in the Arch smoke — `pacman -U` doesn't pull
  dependencies).
- **Tests**: the command parser, the speech extractor (a golden test on a
  representative LLM response + a per-locale gate), clients (wire shapes,
  Gemini parsing, sample rate from mimeType, blocking), PCM→f32
  conversion + graceful player degradation, message
  selection/prefixes/chunking, orchestration (failures with an
  unconfigured engine, stop points, `/tts stop` idempotency, a late-`done`
  race), settings. **1258 unit tests green**, clippy
  `-D warnings`/fmt/`cargo deny` clean (checked at restoration time). Live
  `#[ignore]` smokes (OpenAI/Gemini synthesis, a third-party server, tone
  playback, an e2e run through the orchestrator) — carried over from the
  verified stage 1 (they were passing before the revert). **The live
  smoke `openai_synthesizes_russian_speech_live`** was rerun in this
  session against the real OpenAI API — GO (300 KB of PCM). OpenAI
  `onyx`'s quality was confirmed by the spike (§13.9, direct API calls).
  An interactive `/tts` TUI run wasn't performed in this session (it
  needs a real terminal) — the command/orchestration are covered by unit
  tests.
- **Pause/resume `/tts pause`/`/tts resume`** (at the user's request
  after a live run — for long text): `rodio::Player::pause`/`play`. A key
  shift — the device is now opened by **the handler** (`handle_tts`,
  still lazily, "on command"), wrapped in `Arc<Playback>` (`Send+Sync`)
  and **shared with the background task**; the orchestrator holds a
  handle (`tts_playback: Option<Arc<Playback>>`) → pause/resume apply
  **instantly**, without waiting for a poll. The task's loops **were
  unchanged**: while paused, the queue isn't drained, so the pipeline
  itself holds back synthesis, and `done` isn't emitted
  (`is_drained=false`) until resumed. `stop_tts`/`handle_tts_done` clear
  the handle (dropping closes the device); `stop_tts` clears the pause
  too (otherwise a cancelled task would never drain). `Playback::open`
  moved into the handler: no audio → no task (previously the task would
  start and then fail). Contract: `TtsCommand::Pause/Resume` →
  `ChatIntent`/`AppCommand::TtsPause/Resume` →
  `handle_tts_pause/resume` (no-op with no speech active). i18n: key
  `ui.help.tts_pause`, `ui.tts.bad_arg` updated. Tests: the parser
  (pause/resume + case), a no-op with no device (audio unavailable in CI
  → the `tts_playback=None` path). **1255 unit tests**, clippy/fmt/i18n
  gates clean.
- **Distinct user/assistant voices for `/tts all`** (at the user's
  request after a live run): a new field `voice.user_voice` in
  `TtsCloudSettings`/`TtsExternalSettings` (`#[serde(default)]` → no
  migration). When a separate "User voice" is set for the active mode and
  it **differs** from the assistant's voice, **a second engine** is built
  (`engines_from_config → (assistant, Option<user>)`, a type alias
  `TtsEnginePair`; reuses model/key/base-URL resolution via
  `build_engine(voice_override)`). The pipeline now **carries the role**:
  `build_utterances`/`chunk_utterances` work with
  `Vec<(MessageRole, String)>`, and the task picks the engine by the
  chunk's role (`user_engine.unwrap_or(engine)` for user turns). Voice is
  a property of the engine at creation time (not of the request), hence
  two instances rather than a `synthesize` parameter (minimizing
  trait/client changes). `user_voice` is **independent of
  `speak_roles`** (voices differ acoustically, prefixes are verbal); when
  `None`, everything uses one voice (the previous behavior). A config
  helper `TtsSettings::active_voices() -> (assistant, user)` reads the
  active mode's fields, empty = `None`. UI: field "User voice"
  (`FieldId::TtsUserVoice`) in the "Speech" tab (both the external and
  cloud branches) + i18n `ui.settings.{field,desc}.tts_user_voice`.
  `engine_from_config` was removed (orphaned). Tests: `active_voices` (per
  mode/empty), `engines_from_config` (a second engine only when set and
  different), `chunk_inherits_role`, pipeline tests updated to tuples.
  **1258 unit tests**, clippy/fmt/i18n clean. Live multi-voice testing is
  left to the user (it needs a terminal + audio).
- **Future work** (roadmap): a local sidecar (vosk-tts + a custom Russian
  frontend, `mindfork tts setup` modeled on ADR 0005) + an ADR summarizing
  it; stitching chunks across speech boundaries (even smoother
  multi-paragraph speech), SSE streaming within a chunk, an audio cache,
  auto-picking a voice by message language, reading tables row by row,
  speaking a selected message from the feed, multi-speaker (Gemini), OS
  TTS.

### Post-M9: help/"About" dialog with tabs (F1) (done)
- **The help overlay (`F1`/`?`) reworked from one long scrollable list into a modal
  dialog KDE/Qt-style** (branch `feat/help-about-tabs`): a logo lockup in the header (the
  same `widgets/logo::lockup_lines` as before), a **tab strip** of four tabs, and
  scrollable content for the active tab with a scrollbar. Tabs (in shown order):
  **"About"** (name+description, author `Vladimir Shylov`, version, links — website
  `mindfork.io`, repository, crate `crates.io/crates/mindfork`), **"Hotkeys"**
  (the former `HELP_KEYS` list), **"License"** (MIT text), **"Components"** (third-party
  dependencies + their licenses). Opens on "Hotkeys" (`F1`/`?` — the familiar
  help key; prior behavior kept, "About" is the neighboring tab).
- **New module `shared/credits.rs`** (FSD `shared`, language-neutral data): constants
  `AUTHOR`/`SITE_URL`/`REPO_URL`/`CRATE_URL`, `LICENSE_TEXT` (`include_str!("../../LICENSE")`),
  `COMPONENTS: &[(&str,&str)]` (44 direct runtime dependencies + windows-sys, licenses
  cross-checked against `cargo metadata`; dev/build dependencies `tempfile`/`winresource` excluded).
  **Gate test** `components_cover_direct_dependencies` cross-checks `COMPONENTS` against
  `[dependencies]`+`[target.'cfg(windows)'.dependencies]` in `Cargo.toml` (a line-based parser,
  like the SVG parser in `widgets/logo.rs`), plus tests "sorted/licensed" and
  "LICENSE embedded". License/component data **does not go through locales** — it is read by
  `screens/chat/popups.rs` directly (the legal text/SPDX/URL are language-neutral).
- **State** (`screens/chat/mod.rs`): `show_help: bool` + `help_scroll: usize` replaced
  with `help: Option<HelpState>` + types `HelpTab {About,Hotkeys,License,Components}` (order
  = `ALL`) and `HelpState {tab, scroll}` (`next_tab`/`prev_tab` cycling with scroll
  reset). Navigation (`screens/chat/input.rs`): `Tab`/`←→` — tabs, `↑↓`/`PgUp`/`PgDn`/
  `Home` — scroll, `Esc`/repeat `F1`/`?` — close, `Ctrl+Q`/`F10` — quit (punch through the
  dialog, as in the confirmation popup). Other keys are **ignored** (previously "any
  key closes" — with tabs that would have interfered with navigation).
- **Render** (`screens/chat/popups.rs`, `render_help(frame, &mut HelpState, palette, loc)`):
  fixed dialog size (76×34 + border, clamped to the screen — the window doesn't "jump" between
  tabs), header = (optional lockup + spacer) + tab strip + separator line `─`, below it —
  scrollable content (`Paragraph.scroll`, `total` = number of logical lines) +
  a scrollbar on the right border **along the content area** (not the full height — the
  thumb doesn't intrude on the tabs). Lockup shown **only with room to spare**
  (`inner.height >= LOCKUP_ROWS+6` and width; hard degradation, docs/branding.md §5). **The
  license is word-wrapped**: paragraphs (separated by a blank line) are reassembled and
  wrapped via `wrap::wrap_line` to the dialog width (otherwise long MIT lines would be
  clipped on the right; assembly by `lines()` — CRLF-safe). A local tab strip (does not
  reuse `settings::helpers::tab_strip_line` — that one is `pub(super)` to the settings
  module).
- **i18n**: new keys `ui.help.tab.{about,hotkeys,license,components}`, `ui.about.{desc,
  author,version,site,repo,crate}`, `ui.components.intro`, `ui.help.footer.tabs`
  (ru+en); `ui.help.title` → "About"; removed the orphaned
  `ui.help.footer.{scroll,any}`. i18n gates (parity/no-dead/Cyrillic) covered this automatically.
- **Tests**: `credits` (gate cross-check against Cargo.toml + sorted/licensed + embedded
  LICENSE); chat screen (`f1_opens_help_and_esc_closes` — other keys don't close;
  `help_navigation_scrolls_and_switches_tabs` — scroll + Tab/← with reset; clamp+
  scrollbar on a short terminal; `help_tabs_render_distinct_content` — all 4 labels in
  the tab strip + distinctive content per tab; logo shown/hidden by height).
  **1262 unit tests green** (+4), clippy `-D warnings`/fmt/i18n clean. No live run
  needed (pure UI, `TestBackend`; composition verified by dumping the buffer of all tabs).
- **Polish per feedback** (same branch): (1) **separate "Commands" tab** — input-box
  commands (`/rag …`/`/tts …`) split out of `HELP_KEYS` into a new `HELP_COMMANDS`
  (shared `key_lines` render), tab `HelpTab::Commands`, key `ui.help.tab.commands`;
  tab order is now `About/Hotkeys/Commands/License/Components` (5). (2) **Component
  versions** — `COMPONENTS: &[(&str,&str,&str)]` `(name, version, license)`; new
  gate `component_versions_match_cargo_lock` (parses `Cargo.lock`, the version must be
  among the locked ones — robust to `thiserror` 1/2, `windows-sys` duplicates). (3) **Spacer
  above the logo** (an extra blank line in the header). (4) **Remembering the last tab** —
  new field `ChatScreen.help_last_tab` (like `emoji_last`): closing saves `help.tab`,
  `HelpState::open(tab)` restores it on the next open (`new()` removed;
  `DEFAULT_HELP_TAB = Hotkeys`). (5) **Spacing between "About" items**
  (a blank line before each). (6) **Brand name `mindfork`** in the popup title
  (`credits::APP_NAME` instead of `CARGO_PKG_NAME=mindfork-rs`) — and in the "About"
  tab body. (7) **Version in the terminal window title** (`app/runtime`: `SetTitle`
  is now `mindfork v<version>`, matching the popup; Windows). Tests: `help_remembers_
  last_tab`, the "Commands" tab in `help_tabs_render_distinct_content` (+ checking that
  there are NO commands on the hotkeys tab), component version in the dump. **1264 unit tests green**
  (+2), clippy `-D warnings`/fmt/i18n clean.
- **Groundwork**: clickable links (OSC 8 hyperlinks — terminal-dependent), generating the
  component list (versions/licenses) from `cargo metadata` in build.rs (currently a
  curated list + gates against Cargo.toml/Cargo.lock).

### Post-M9: English source-language migration (done)
- **The project's development language moved RU -> EN** (design doc
  [docs/history/english-source-migration.md](docs/history/english-source-migration.md),
  branch `chore/english-source`): all documentation, every comment and
  doc-comment in 176 `.rs` files, and the non-Rust tail (packaging, workflows,
  `Cargo.toml`, `build.rs`, artwork) are now English. **Not an i18n rollback** —
  user-facing text stays localizable through axes A/B, and `ru` remains a fully
  supported locale. Prep for going open source (roadmap direction, now closed).
- **Sequencing** (one commit per phase, gate green at each): Phase 0 tooling +
  glossary + validation samples -> Wave 1 living docs -> Wave 2 CLAUDE.md (7k-line
  journal, split/translate/reassemble) -> Wave 3 archive docs -> Wave 4 code
  comments (~11.4k lines, subagents batched by directory cluster) -> Phase 5a
  non-user-facing strings inline -> Phase 5b user-facing gaps into the bundles ->
  Phases 6-7 convention flip + acceptance. Large files were split at heading
  boundaries into disjoint chunks so parallel subagents could not conflict; the
  glossary held terminology consistent across chunks.
- **`tools/cyrillic_scan.py` — the worklist, the acceptance gate, and now a CI
  lint** (a step in the `lint` job of `ci.yml`, before the Rust setup — it needs
  neither the toolchain nor ALSA). It reports every non-allowlisted Cyrillic line
  and exits non-zero while any remain: **30,890 -> 0**. The allowlist is
  deliberate, not laziness — `locales/ru.json`, Hunspell dictionaries, in-test
  fixtures and `ru`-locale assertions, `.desktop` `[ru]` keys and `.iss` `ru.`
  installer messages, `keys.rs` JCUKEN char data, and `cyrillic-ok`-marked
  endonyms all stay Cyrillic by design. `docs/research/mermaid-ascii-rendering.md`
  is skipped wholesale: its repro inputs must be multibyte to demonstrate the
  byte-offset bug it documents. The scanner needed four corrections along the way,
  all false-positive sources rather than real work (multi-line test fixtures;
  `tests.rs` files whose `#[cfg(test)]` marker lives in the parent `mod.rs`; `//`
  inside string literals; its own Cyrillic ranges) — roughly 3.2k of the original
  count was never translation debt.
- **Phase 5b — the last 36 strings were i18n gaps, not translation debt**: they
  were user-facing text that had never been localized at all. Each got a key in
  both bundles (`ru` keeps the original verbatim), resolved on the correct axis —
  axis A (profile/agent locale via `ctx.loc`): `loop.*`,
  `selfmodel.render.impersonation.*`; axis B (interface locale via
  `ui_locale()`/`screen.loc()`): `ui.err.server.*`, `ui.rag.*`, `ui.chat.copied`,
  `ui.err.copy_failed`, `ui.settings.choice.python_*`. Reaching a locale changed a
  few signatures (`backend_if_ready`/`impersonation_backend_if_ready`,
  `rag_command::parse`/`extract_path`, `UserModel::render_for_impersonation`), and
  `PythonMode::label` moved from `shared/config.rs` to `screens/settings/helpers.rs`
  (mirroring `theme_label`) so the config layer no longer owns UI text.
- **Two behavior changes** (both in CHANGELOG): `CharacterNames::default()` now
  seeds **new** profiles with "You"/"Assistant"/"System" — editable seed data, not
  chrome, so existing profiles keep their stored values and no migration is
  needed; and the previously unlocalized strings above now follow the selected
  language instead of always showing Russian.
- **Pre-existing defects surfaced and fixed along the way**: `present.rs` detected
  a failed `fs_read` by matching a hardcoded Russian failure prefix, so an
  English-profile failure was mis-rendered as a highlighted code block (now
  resolved across all locales via `fs_read_failure_prefixes()`, mirroring
  `exit_labels()`); 32 `spec.md` anchors broken by heading translation; tool-call
  artifacts committed into two history docs; an unlocalized thoughts label and
  console exit-code line in `message_feed.rs`.
- **Convention flipped** (`CLAUDE.md` §Conventions, `AGENTS.md` §3 and §5,
  README): comments, docs **and commit messages** are English from here on, with
  the scanner guarding it in CI. The design doc and the RU->EN glossary were
  archived to `docs/history/` (the glossary kept as a terminology reference rather
  than deleted). **1267 unit tests green**, 58 `#[ignore]`, clippy
  `-D warnings`/fmt/i18n gates clean — the test count is unchanged by translation
  itself; the +3 over the previous entry come from the per-locale tests added in
  Phase 5b.
- **Live regression run — GO** (Gemma 4 31B q4_0 + bge-m3, external
  `llama-server`, `--jinja`): **44 of the 58 `#[ignore]` smokes executed, all
  green** — 20 orchestrator e2e (memory/self-model/notes/RAG/graph/cross-organ
  links/`en`-profile i18n), the 6 base engine smokes (streaming, EOS
  anti-self-cutoff, tool calling, "thoughts", sampling extensions, control
  tools), MCP (client-level + orchestrator e2e), the Python sandbox
  (numpy/pandas/requests/memory cap/timeout/`en` localization), web search,
  fetch, local Python, and tone playback. The remaining 14 skip without cloud
  credentials (Anthropic/Gemini/OpenAI Responses/TTS endpoints). Nothing in the
  engine, memory, or tool paths regressed — expected, since translation touched
  comments and string *content*, but worth confirming after Phase 5b changed
  tool descriptions and a few signatures.
- **Two run-the-smokes gotchas** (cost a false failure each, worth knowing): the
  `provisioned()`-gated Python sandbox smokes **silently no-op and still report
  `ok`** when `MINDFORK_SANDBOX_DIR` is unset (the helper returns `None` and the
  test just returns) — fixed, the helper now prints a skip line. Set the var,
  plus `MINDFORK_SANDBOX_WASMER`/`_PYTHON` for `runs_real_python_in_sandbox`,
  which resolves against the default dir rather than that env and **fails
  rather than skipping** when the sidecar is missing — kept that way on
  purpose (decision 2026-07-24): an absent sandbox that was meant to be
  installed should be loud. And `mcp_filesystem_e2e_live` can exceed its 120s readiness
  timeout on the **first** `npx @modelcontextprotocol/server-filesystem` run
  (cold npm cache, the package downloads); it passes in ~13s once warm.

### Post-M9: universal layout-independent hotkeys — stage 1, Windows (done)
- **Problem**: `Ctrl+<letter>` shortcuts were layout-independent for exactly
  **one** layout — `shared/keys.rs::physical_char` was a hardcoded table for the
  standard Russian JCUKEN. Under Greek/Hebrew/Georgian/Bulgarian/Armenian/Thai/
  Turkish/… the character passed through unchanged, the match against `'q'`/`'l'`
  failed, and every shortcut was dead. Research —
  [docs/research/layout-independent-hotkeys.md](docs/research/layout-independent-hotkeys.md),
  decision points R1–R5 accepted by the user per the recommendations 2026-07-24;
  branch `feat/universal-hotkeys-win`.
- **Key insight (from crossterm's own source)**: on Windows the character we
  receive is not raw input — for `Ctrl+<letter>` the console delivers a C0 code,
  and crossterm *computes* the character itself via
  `ToUnicodeEx(vk, active layout)` (`event/sys/windows/parse.rs::get_char_for_key`),
  discarding the VK/scan code. So the character can be **inverted with the mirror
  API**: `VkKeyScanExW(ch, hkl)` → VK → `MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC, hkl)`
  → **scan code** → one static "Set 1 scan code → QWERTY" table (~47 entries,
  a hardware standard, one table for all languages forever). That's the whole
  mechanism: **no per-language data**, covers every installed layout including
  ones that don't exist yet. The static tables approach was rejected as the
  mechanism (§4A of the research): one script ≠ one layout (Bulgarian BDS vs
  Phonetic, Armenian East/West, Arabic 101/102 — a char-keyed table can't know
  the user's national variant; the OS can).
- **Tier ladder** (`hotkey_char(&KeyEvent) -> Option<char>`, the new single entry
  point): (0) **ASCII short-circuits** — never remapped by position, so on
  AZERTY/QWERTZ/Dvorak a shortcut still belongs to the key *labeled* with that
  letter (previous behavior, and what users expect); (1) Windows, **active
  layout** (`GetForegroundWindow`→`GetWindowThreadProcessId`→`GetKeyboardLayout`,
  mirroring crossterm) — the *exact* inverse, authoritative, also handles
  non-standard geometries like Russian Typewriter; (2) the static **JCUKEN**
  table; (3) Windows, **installed layouts** (`GetKeyboardLayoutList`) for
  characters the table doesn't know; (4) pass-through.
- **Why the table sits BETWEEN the two Windows tiers** (deviation from the
  research sketch, which put all WinAPI first): under conhost the foreground
  window's layout can't be queried (crossterm documents this) → tier 1 returns
  `None`, and probing installed layouts becomes a **guess** — with both Russian
  and Serbian installed the same letter sits on different keys, so the guess
  could regress today's Russian-on-conhost users. Keeping the known-good table
  ahead of the guess makes the change **strictly non-regressive**.
- **Call-site sweep**: 9 sites in 6 files (`screens/chat/{input,popups}.rs`,
  `screens/settings/apply.rs`, `screens/self_model.rs`, `widgets/chat_list.rs`,
  `widgets/input_box.rs`) moved from `keys::physical_char(c)` (inside a manual
  `if let KeyCode::Char(c)`) to `keys::hotkey_char(&key)`. The `KeyEvent`-shaped
  signature is deliberate: the future unix tier (below) arrives as a **field on
  the event**, so that upgrade is a one-file change instead of a second sweep.
  `physical_char` became a `#[cfg(test)]` pure core (table + lowercase);
  `is_slash_key` unchanged (both `/` and `.` are ASCII → not remapped, R4 defer).
- **Dependencies**: `windows-sys` (already direct — Job Object, DPAPI) gained
  `Win32_UI_Input_KeyboardAndMouse` + `Win32_UI_WindowsAndMessaging`. No new
  crates. Code lives behind `#[cfg(windows)] mod win_layout` in `shared/keys.rs`
  (precedent — `shared/secrets.rs::dpapi`, `shared/sandbox.rs`).
- **Unix is a separate mechanism, not a gap in this one**: the kitty keyboard
  protocol already carries the answer — the **base layout key** ("the key
  corresponding to the physical key in the standard PC-101 layout") of the
  `REPORT_ALTERNATE_KEYS` (0b100) enhancement — but **crossterm 0.29 parses only
  the shifted alternate and drops it** ([crossterm#968](https://github.com/crossterm-rs/crossterm/issues/968),
  open, no PR), so pushing the flag today gains nothing. Plan: contribute
  upstream (precedent — mermaid-text #29/#30 → 0.56.1), then prefer
  `base_layout_code`; recorded in docs/roadmap.md. Note the irony found during
  research: on kitty-protocol terminals our existing `DISAMBIGUATE` push
  *replaces* the terminal's own legacy fallback with faithful layout reporting —
  the **VTE family** (GNOME Terminal & co., which hasn't shipped the protocol)
  falls back to the Latin group itself and works for every language with no
  effort on our side.
- **Tests**: pure (table lookups, `hotkey_char` reads character keys only, ASCII
  pass-through, unknown characters); the **scan-code table** gate (letter rows +
  digits/punctuation + non-character keys like Esc/Tab/Space → `None`); and two
  OS-path tests that are machine-independent by construction — Latin letters
  resolve through installed layouts to *distinct* ASCII keys (true on QWERTY,
  AZERTY, QWERTZ alike), and a **self-calibrating cross-check**: if the standard
  Russian layout is installed (probed via a sentinel), the OS lookup must agree
  with the static table on all 26 characters, otherwise the test announces a skip
  (CI has no Russian layout; a Typewriter-only machine would legitimately
  disagree). **1273 unit tests green** (+6), 58 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (this machine, real Windows layouts, temporary diagnostic
  test removed afterward): the full OS pipeline resolved `д`→`l`, `й`→`q`,
  `ф`→`a`, `я`→`z` through **both** the active and the installed-layout tiers,
  agreeing with the static table; the self-calibrating cross-check ran (not
  skipped) over all 26 characters. **Bonus found in the run**: the resolver also
  fixes characters the table never had *within Russian* — `ю`→`.`, `ж`→`;`,
  `э`→`'`, `б`→`,`, `х`→`[` sit on punctuation keys, outside the 26 letter
  positions, so those `Ctrl` combos were dead before. Greek/Hebrew/Turkish
  characters returned `None` (those layouts aren't installed here) and correctly
  fell through to pass-through. A full interactive TUI check under a switched
  layout needs a real terminal — left to the user.

### Post-M9: universal layout-independent hotkeys — stage 3, upstream crossterm (submitted)
- **Stage 3** of the track (research
  [docs/research/layout-independent-hotkeys.md](docs/research/layout-independent-hotkeys.md) §4 C,
  fork R2): the unix half of layout independence isn't ours to write — the kitty
  keyboard protocol already carries the **base layout key** ("the key
  corresponding to the physical key in the standard PC-101 key layout"), but
  crossterm 0.29 parses only the shifted alternate and drops it
  ([#968](https://github.com/crossterm-rs/crossterm/issues/968), open since Feb
  2025, **zero comments**, no competing PR). So the work was an upstream
  contribution — precedent: mermaid-text #29/#30 → 0.56.1.
- **Patch** (submitted as
  [crossterm#1074](https://github.com/crossterm-rs/crossterm/pull/1074),
  +137/−15): the `CSI u` parser reads **both** alternates positionally — which
  also fixes a latent bug, since any alternate may be empty
  (`CSI 1076::108;5u`) and the old `codepoints.next()`, reached only under
  SHIFT, would then hand out the wrong slot; the base layout key is exposed as
  `KeyEvent::base_layout_code: Option<KeyCode>` (mirroring the existing
  enhancement-gated `kind`/`state` fields) plus a `with_base_layout_code`
  builder.
- **The one design decision**: the field takes **no part in
  `PartialEq`/`Hash`**. It describes the same key press rather than identifying
  it, and including it would silently break the ubiquitous
  `event == KeyEvent::new(KeyCode::Char('c'), CONTROL)` comparison for every
  application the moment it enables `REPORT_ALTERNATE_KEYS` — making the feature
  unusable. crossterm's manual impls destructure `KeyEvent` exhaustively, so
  this is an explicit `base_layout_code: _`. (Derived `PartialOrd`/`Ord` do
  include it, inconsistent with `Eq` — but they already disagree via
  `normalize_case`; pre-existing wart left alone, offered to the maintainers.)
- **Verification without a local unix toolchain**: no WSL at first, so the patch
  was checked by `cargo check --target x86_64-unknown-linux-gnu` locally +
  a throwaway workflow in our own repo (GitHub blocks Actions on forks until
  enabled by hand) that clones the patched fork and runs its suite on ubuntu.
  A local "just un-cfg the unix parser on Windows" attempt was **abandoned** —
  it cascaded into un-gating `InternalEvent` variants and then broke exhaustive
  matches in the Windows code, i.e. each hack lowered the fidelity of what was
  being tested. After the user installed WSL (**Ubuntu 24.04**, chosen to match
  `ubuntu-latest` in both CIs), everything was re-run natively: **119 crossterm
  tests green**, fmt/clippy clean, and — the load-bearing detail — **all
  pre-existing tests passed unchanged**, which is what makes "equality is
  untouched" concrete rather than asserted.
- **End-to-end spike caught a design error** (§4 C.2): our tree with
  `[patch.crates-io] crossterm = { path = … }` + the tier-0 branch, run on
  Linux. The base layout key must **not** be honoured for an ASCII character —
  on AZERTY the key labelled `A` sits at the US `Q` position, so it arrives as
  `Char('a')` with base layout key `q`, and taking it unconditionally would turn
  `Ctrl+A` (select all) into `Ctrl+Q` (**quit**) for every AZERTY user. Tier 0
  therefore belongs **after** the ASCII short-circuit — the same rule that
  protects Latin layouts in the Windows tier. Spike results: Greek `λ`, Hebrew
  `ק`, Cyrillic `д` resolve through the protocol to `l`/`e`/`l`; AZERTY `a`/`q`
  stays `a`; without the enhancement the table still answers. Our full suite on
  Linux: **1271 passed** (the delta from 1273 on Windows is exactly the
  `#[cfg(windows)]` tests), clippy `-D warnings` clean. The spike is
  deliberately **not committed** (it cannot build without the patched
  crossterm) — the research doc is its durable form.
- **Our side stays put until an upstream release** (bump → push
  `REPORT_ALTERNATE_KEYS` → add the tier-0 branch; no call-site changes thanks
  to the `KeyEvent`-shaped `hotkey_char` from stage 1). Timing caveat recorded
  in the roadmap: crossterm merges PRs regularly, but **the last crates.io
  release was 0.29 in April 2025** — the release, not the review, is the long
  pole. No unit-test count change in this repo (docs only).

### Post-M9: custom user/assistant names in profile settings (done)
- **The role names shown to the user became editable** (branch `feat/custom-role-names`,
  user's request): two fields in the settings "Profiles" section (the "Persona"
  group) — "User name" / "Assistant name". A set name replaces the feed's role
  header (**uppercased** to match the header style: `GAIA` instead of `YOU`) and the
  label in the `F5` conversation export (`Gaia:` instead of `User:`). Both are
  **empty by default**: empty = "not set", and each surface falls back to the
  interface language's own label, so the chat follows axis B until the user
  overrides it. Whitespace-only counts as unset (the field isn't a way to blank the
  label out).
- **The type already existed and was dead** — `CharacterNames {user, assistant,
  system}` has been on `Profile` (and copied onto `Chat`) since M2, but nothing ever
  displayed it. The work was wiring it to the two surfaces, not adding a field.
- **Key decision — the profile is the source of truth, resolved at render time**
  (a deliberate exception to spec §10 "editing a profile doesn't affect existing
  chats"): the copy semantics exists so the assistant can change a chat's
  `system_message` per chat, but a *display* name the user just typed in settings has
  to show up in the chat they're looking at. So the feed and the export read
  `Profile.character_names`; `Chat.character_names` stays (import format + a possible
  future per-chat override) but is documented as not used for display.
- **Plumbing**: a new `AppEvent::CharacterNames` — the orchestrator (which owns both
  chats and profiles) resolves the active chat's profile and pushes the names; emitted
  from `activate()` (switch/create/bootstrap) and from `handle_update_profile` (so a
  rename lands on the open chat immediately). `ChatSummary` carries no `profile_id`,
  so the screen can't resolve this itself — hence a push, not a pull. The names ride
  the feed's **render-cache key** (`CacheKey`), otherwise a rename wouldn't repaint
  the already-cached header lines.
- **A one-time cleanup of legacy seeds** (`features/profiles::clear_seed_character_names`,
  called from the same `bootstrap` loop as `reconcile_tools`): `CharacterNames::default()`
  used to seed a Russian placeholder triple, migrated to `You`/`Assistant`/`System` by
  the english-source migration (`b83ae37`). Those were never displayed, so with the
  fields now live a Russian-interface user would suddenly get Latin `YOU`/`ASSISTANT`.
  Fields still holding a seed value are cleared (per-field, both sets); a name that
  came from an import/hand edit survives. Idempotent, additive — **no schema bump**
  (ADR 0006 F12).
- **Not included** (deliberate, out of the requested scope): `character_names.system`
  gets no field (system messages appear in neither surface); TTS role prefixes
  (`speak.role.*`, axis A — the *model's* language) keep their localized text.
- **Tests**: entity (empty default, trimming, blank = unset); export (custom labels,
  one-sided naming, blank fallback); feed (headers replaced and uppercased, cache
  invalidated on rename); settings (the fields commit into `character_names`, an
  empty value is a valid edit, grouped with Persona + described); orchestrator (names
  follow the profile and are re-sent after an edit; `F5` labels; bootstrap clears
  seeds but keeps a chosen name); runtime (the event reaches the feed even with
  another screen on top). **1294 unit tests green** (+13), 58 `#[ignore]`, clippy
  `-D warnings`/fmt/i18n gates/`cyrillic_scan` clean.
- **A live run isn't required** — no engine/memory/tool path is touched: this is UI
  rendering, a pure export formatter, and a profile-field edit, all covered by
  `TestBackend`/unit tests.

### Post-M9: impersonation profiles + a newly created profile is selectable (done)
- Two defects in the settings "Profiles" section, reported by the user; branch
  `feat/impersonation-profiles`. Forks confirmed by the user 2026-07-25 (per the
  recommendations): the impersonation persona is bound **through the assistant
  profile**, and the legacy field is **migrated automatically**.
- **(1) `Ctrl+N` created a profile you couldn't then select.**
  `handle_create_profile` emitted only `AppEvent::ProfileList` — which the **chat**
  screen consumes; the settings screen keeps its own copy of the list from the
  `Settings` snapshot, so a new profile stayed invisible there (not selectable, let
  alone editable) until a restart. `handle_create_profile`/`handle_delete_profile` now
  also `emit_settings()` — `SettingsScreen::refresh`'s doc comment ("after create/delete
  of a profile") had described this contract all along; only the emit was missing.
  Auto-selection: the orchestrator owns the list, so the screen can't know the new id —
  `Ctrl+N` raises a one-shot `pending_profile_select`, and `refresh` selects whichever
  profile isn't in the previous snapshot (comparison by id, not "the last one" — robust
  to ordering). One-shot by construction: the flag is cleared whatever the snapshot
  brings, so a later unrelated re-emit can't hijack the selection.
- **(2) The "Impersonation" subsection edited the assistant's profile.** It showed the
  assistant profile selector and name (both section-level, shared with the "Assistant"
  subsection) and, under them, a single system message — an asymmetry inherited from
  storing the impersonation prompt as `Profile.impersonation_system_message`. Now the
  two subsections edit **different lists**: "Assistant" — the AI-interlocutor profiles,
  "Impersonation" — the user personas, each with its own name and system message
  (`IpSelect`/`IpName`/`IpSystem`); `Ctrl+N`/`Ctrl+D` act on whichever list the active
  subsection shows. The assistant profile ties them together with a new
  "Impersonation profile" field (`PImpProfile` → `Profile.impersonation_profile_id`),
  so "who the assistant is" and "who I am in this conversation" travel together and
  several assistant profiles can share one persona.
- **Storage — `AppConfig.impersonation_profiles`, not `profiles.json`** (a deliberate
  departure from "profiles live in profiles.json"): impersonation is already configured
  globally in `settings.json` (`impersonation_engine`, `impersonation_sampling`), and
  `profiles.json` is a bare array with no room for a second list. The payoff is large —
  the settings screen edits the list through the ordinary config-save path, so
  **no new `AppCommand`/`AppEvent`/`SettingsIntent`, no storage artifact, no schema
  registration**, and creating a persona needs no orchestrator round-trip (hence no
  auto-select problem for that list). The cost is a cross-file reference: a dangling id
  is legal and reads as "not set" → the shared default text (so deleting a persona
  doesn't have to rewrite every referencing profile). Both fields are additive
  (`#[serde(default)]` + skip-if-empty) → **no schema bump** (ADR 0006 F12).
- **Migration** (`Orchestrator::migrate_impersonation_profiles`, called from
  `bootstrap`): every profile still carrying a non-empty legacy message and no
  reference gets a persona "«name» (impersonation)" created and linked. Idempotent (a
  profile with a reference is skipped) → safe across upgrade/downgrade cycles; the
  config is written **before** the profile links (a link persisted without its target
  would dangle), and on failure the next launch simply retries. The legacy field is
  **kept** on disk — nothing reads it for prompt building any more.
- **Resolution** was pulled out of `handle_impersonate` into a pure
  `Orchestrator::impersonation_system(profile, loc)` — reference → persona → non-empty
  message, with every miss (no reference / dangling id / blank message) falling back to
  `prompt.impersonation.default`. That made it unit-testable without a streaming
  backend.
- **Ripple**: `ProfileEdit.impersonation_system_message` → `impersonation_profile_id`
  (`Some(None)` unlinks); `FieldId::PImpSystem` removed, `PImpProfile`/`IpSelect`/
  `IpName`/`IpSystem` added (registered in `is_profile_field` — user data has no
  "config default", so no `•` marker and no `Del`-reset); `choice_menu` gained the
  persona lists (`PImpProfile`'s option 0 is "not set"); the import format is
  unaffected (v1 never carried impersonation). Along the way a latent bug was fixed:
  the "no profiles" branch of `profile_fields_for` used to return a lone row **without**
  the subsection tab strip, stranding the user in the section.
- **Tests**: settings (the impersonation subsection edits its own list and has no
  assistant selector/name/tools; create → edit name+message → delete; the assistant's
  reference cycles "not set" ↔ persona; a new profile is selected on the re-emit and the
  flag is one-shot); orchestrator (create/delete re-emit `Settings`; the legacy
  migration links and is idempotent across two launches; resolution covers reference/
  dangling/blank); config (additive default + round-trip); render (user data never gets
  the `•` "modified" marker — a false positive found by dumping the rendered section:
  a chosen persona was flagged only because the *default* config has no personas at
  all; `is_profile_field` now gates the marker as its doc comment always claimed).
  **1281 unit tests green** (+8), 58 `#[ignore]`, clippy `-D warnings`/fmt/i18n gates/`cyrillic_scan` clean.
  **A live run isn't required** — no engine/memory/tool path is touched: the
  impersonation *request* is unchanged, only where its system message is read from
  (covered by unit tests); the rest is settings UI.

### Post-M9: Python sandbox — beautifulsoup4 in the starter set (done)
- **beautifulsoup4 added to the `sandbox setup` lock list** (branch
  `feat/sandbox-beautifulsoup`, user's request; the same mechanical change as the
  earlier pandas addition, ADR 0005 §4 — no architectural decision, so no design
  doc). HTML parsing is the natural companion to the already-present `requests`:
  fetch a page → parse it, without the model having to hand-roll regex over markup.
- **Three pure-Python wheels from PyPI** (`py3-none-any`, exact URL + sha256, as
  everything else in the lock list): `beautifulsoup4 4.15.0` + its runtime
  dependencies `soupsieve 2.9.1` (CSS selectors — `soup.select`) and
  `typing_extensions 4.16.0`. Nothing native → no wasix index involved, and
  **`warmup` was deliberately not touched** (it exists to compile native `.so`
  files; pure Python has nothing to compile — unlike the pandas addition, which
  switched warmup to `import pandas`).
- **The `dir` field is the name in `site-packages`, not the distribution name** —
  verified by actually listing the wheels rather than assuming: `bs4`,
  `soupsieve`, and `typing_extensions.py` (a single module file, like the
  existing `six.py` entry). Getting this wrong would silently break idempotency
  (the directory check would never hit, so every `setup` run would re-download).
  The sha256 of all three was verified against a real download.
- **The tool description** (`tool.python_exec.desc.wasmer`, ru+en) now names
  beautifulsoup4 — this is what the model reads, so an unlisted package is a
  package it won't reach for; "scientific packages" was also reworded to
  "preinstalled packages" (bs4 isn't scientific).
- **Tests**: the `lockfile_wheels_*` gate extended with `bs4`/`soupsieve`/
  `typing_extensions.py`; a live `#[ignore]` smoke `beautifulsoup_in_sandbox`
  (parses HTML offline — no network needed — and calls `soup.select`, which
  exercises soupsieve, the dependency most likely to be missing). **1294 unit
  tests green** (count unchanged — the new test is `#[ignore]`), **59
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/i18n gates/`cyrillic_scan` clean.
- **Live run — GO** (real `wasmer` 7.2.0 + the provisioned dev sandbox):
  `sandbox setup` skipped everything already installed and downloaded/unpacked
  exactly the three new wheels (idempotency intact), warmup passed; the new smoke
  printed `bs4 4.15.0` / parsed text / one CSS-selector match. **No regression**:
  all 14 `python::tests` sandbox smokes green (numpy/pandas/requests with and
  without network/timeout/memory cap/en localization) — adding
  `typing_extensions` to a shared `site-packages` didn't shadow anything.
  (`runs_real_python_in_sandbox` needs `MINDFORK_SANDBOX_WASMER`/`_PYTHON` on top
  of `MINDFORK_SANDBOX_DIR` — a known, deliberate quirk, see the english-source
  migration entry.)
- **Setup download grew by ~190 KB.** Deliberately **not** added: `lxml` (needs a
  native wasix wheel — not in the index), `html5lib` (an extra parser on top of
  the stdlib `html.parser` bs4 already uses).

### Post-M9: the mindfork.io site URL in project metadata (done)
- **The domain `mindfork.io` was registered** (2026-07-26) for a future project
  site. It was already in the `F1` "About" dialog (`credits::SITE_URL`); this
  change carries it into the places that have a genuine **"project homepage"**
  slot, where the repository URL had been standing in. Branch
  `chore/release-0.9.4` (started as `chore/site-url`).
- **Where it went**: the Windows installer's `AppPublisherURL`
  (`packaging/windows/mindfork.iss` — surfaces in "Apps & features"); nfpm's
  `homepage` (→ deb `Homepage:` / rpm `URL:` / pacman `url`); the standard
  `homepage` field in `Cargo.toml` (was absent entirely); a README badge in the
  brand accent `#c25a27`; a header line in `docs/install.md` (that file ships
  inside the packages and the Windows install directory); and a footer on every
  GitHub Release page (`release.yml`).
- **Every actionable link deliberately stayed on GitHub** — the site does not
  exist yet, so a dead link must never be the only way to get help or a
  download. The installer's other two ARP links were **split** for exactly this
  reason: `AppSupportURL` → `/issues`, and a new `AppUpdatesURL` → `/releases`
  (both previously pointed at the bare repo root).
- **An audit of the repo URL found nothing else to convert**: all remaining
  occurrences are legitimately GitHub-specific (CHANGELOG `/compare/` links, the
  CI badge, the Releases download link, `Cargo.toml`'s `repository`,
  `credits::REPO_URL`, and — in a research doc — a *different* repo, the
  crossterm fork). The one homepage slot left is **outside the codebase**:
  GitHub's own repo "Website" field (`gh repo edit --homepage`), which is the
  user's to set.
- **The crate URL was deliberately NOT added anywhere user-facing.** The name
  `mindfork` is still free (verified against the registry API — 404), but the
  package is `mindfork-rs`, so claiming it is a packaging decision, not a URL
  edit: either a full rename (which renames the binary and ripples into the
  installer, nfpm layout, `.desktop` `Exec=`, the `/usr/bin` symlink, docs and
  CI artifact names) or `name = "mindfork"` + `[[bin]] name = "mindfork-rs"`.
  Until it is published, a crates.io version badge or a `cargo install mindfork`
  line would render **broken**; recorded as a roadmap item instead.
- **Release-notes footer** (`release.yml`): the body was just the CHANGELOG
  section; it now ends with the site + the install guide **pinned to the tag**
  (so instructions travel with the release they describe). Appended after the
  empty-notes fallback, so it is present on both paths; the repo URL is built
  from `GITHUB_SERVER_URL`/`GITHUB_REPOSITORY` rather than hardcoded. The step
  also `cat`s the composed notes into the job log.
- **Verification**: the `.iss` was **compiled** with real `ISCC.exe` 6.7.3 (an
  unknown directive is a compile error — that is the meaningful check for
  `AppUpdatesURL`); the release step was simulated against the real
  `CHANGELOG.md` on both the normal and the fallback path. **Gate green**: 1294
  unit tests, 59 `#[ignore]`, clippy/fmt/`cyrillic_scan` clean. No live engine
  run needed (metadata/docs only; the sole Rust change is a doc comment).
- **A false alarm worth recording**: the first simulation appeared to show the
  CHANGELOG extraction failing and falling back to a bare `Release vX.Y.Z.`
  — a would-be significant pre-existing bug. It was an artifact of the
  reproduction: a quoted heredoc collapsed `\\[` to `\[`, turning the awk regex
  into a character class. `od -c` against the real file showed the workflow has
  the correct double backslash, and re-running with bytes extracted verbatim
  from `release.yml` produced the section correctly. **The release notes were
  never broken.**

### Post-M9: cutting GitHub Actions minutes (done)
- **Trigger**: the `v0.9.4` release run was refused by GitHub with *"The job was
  not started because recent account payments have failed or your spending limit
  needs to be increased"* — the 2000 included minutes of the Free plan ran out.
  Diagnosis note: the run showed up as `cancelled`, but the real cause was one
  matrix job failing at **scheduling** (no steps, 1 s) with `fail-fast: true`
  cancelling the rest; the reason is only visible in the job's **annotation**,
  not in the run/job status. `gh run rerun` is futile while the block is in
  place (attempts 2–4 all failed identically).
- **Where the minutes went** (measured, not estimated — GitHub rounds each job
  up to a whole minute and bills `windows-latest` at **2x**): CI on a PR =
  ubuntu 105 s + windows 467 s + lints 69 s → **20 billable min**; CI on the
  push to `main` = the same thing again → **20 min**; Packaging on `main` →
  **9 min**. The Windows test job alone is 16 of the 20.
- **The redundancy**: GitHub runs `pull_request` against the **merge result**
  (`refs/pull/N/merge`), so the push-to-`main` run re-tests code that has
  already passed on that exact tree — visible as a pair in the run list on every
  single merge.
- **Fix** (two workflow edits, no Rust): (1) `ci.yml` — the `test` matrix is now
  an expression, both OSes on `pull_request` and **Linux only** on `push`
  (`github.event_name == 'push' && fromJSON(...) || fromJSON(...)`); (2)
  `packaging.yml` — the `push: [main]` trigger removed entirely (`workflow_dispatch`
  kept). Per merge: CI 40 → 24 min, Packaging 18 → 9 min, ≈ **43 % less**.
- **Why `main` keeps a Linux CI run** rather than dropping the trigger outright:
  the README CI badge tracks the default branch and would go stale without one,
  and a Linux run still catches a semantic conflict between two separately-green
  PRs. It costs 4 min against the 16 saved by dropping Windows there.
- **Not done** (offered as follow-up): skipping the `test` job for docs-only
  changes. It is the biggest remaining win for this repo (the CLAUDE.md journal
  makes many PRs almost entirely `.md`, and PR #212 burned 20 min on one), but
  job-level path filtering needs either a third-party action or a hand-rolled
  `git diff` gate, and a wrong verdict silently skips the test gate. The `lint`
  job must keep running regardless — `cyrillic_scan` guards the docs themselves.
- Branch protection could not be consulted (403: unavailable for private repos on
  Free), so there are no required status checks to strand.

### Post-M9: skipping the test job for docs-only pull requests (done)
- The follow-up left open above (stacked on `ci/reduce-actions-minutes`). This
  repo's PRs are frequently near-pure documentation — the CLAUDE.md journal alone
  makes that the norm — and each one paid the full **20 billable minutes**
  (PR #212, a docs/packaging change, is the example). A new `changes` job
  classifies the PR's files and the `test` job gains
  `if: needs.changes.outputs.docs_only != 'true'`. `lint` is deliberately
  **untouched and always runs**: `cyrillic_scan` is a guard on the docs
  themselves, so skipping it exactly when only docs change would remove the gate
  where it matters most.
- **The allowlist is the whole design, and it is narrow**: `docs/**` plus
  top-level `*.md`. It is an allowlist rather than a denylist, so anything new or
  unrecognized falls through to running the tests. Files that *look* like docs
  but are read by the build or a gate test — established by grepping
  `include_str!`/`include_bytes!`/`CARGO_MANIFEST_DIR`, not by assumption — and
  therefore must never be in it: **`LICENSE`** (`include_str!` in
  `shared/credits.rs`, asserted by a test), **`artwork/*.svg`** (parsed by the
  `widgets/logo.rs` "code ≡ asset" gate), **`locales/*.json`** (`include_str!` +
  the i18n parity gates), **`Cargo.toml`/`Cargo.lock`** (the credits gates), plus
  `tools/`, `packaging/`, `dictionaries/`, `tests/fixtures/` and `.github/`
  itself. Note `.github/pull_request_template.md` is a `.md` that correctly does
  **not** match (the pattern anchors a top-level name, no slash).
- **Fail-safe by construction**: `docs_only=true` requires a `pull_request`
  event **and** a successful API call **and** the returned file count matching
  the PR's `changed_files` **and** every path inside the allowlist. Any other
  outcome runs the tests. The count check exists because the files endpoint
  truncates on very large PRs, and a partial page could otherwise hide a source
  file and look docs-only. The gate is `!= 'true'`, not `== 'false'`, so an
  empty output still runs the tests.
- **Corrected right after merging**: that value guard was *not* sufficient on its
  own. A failed `needs` dependency skips the dependent job **regardless of
  `if`**, so an infrastructure failure in `changes` silently skipped the test
  gate — observed for real in run 30217314709, where the billing block stopped
  `changes` from starting and `Tests` came out "skipped". Fixed by adding
  `!cancelled()` to the condition (`always()` would be wrong — cancelling the
  workflow must still cancel the job). Lesson: in a `needs` + `if` gate, the
  value and the upstream job's *status* are two separate failure modes.
- **Verified by executing the script**, not by reading it: the `run:` block was
  extracted from the YAML and run against a stubbed `gh` for six scenarios —
  docs-only → skip; code present → run; **truncated listing** → run; empty list
  → run; API failure → run; non-PR event → run. The path pattern was separately
  checked against PR #212's real file list and against each trap above.
- **Cost**: the `changes` job bills 1 minute per PR run (GitHub's per-job
  minimum) and saves 20 on a docs-only PR. Kept as its own job rather than an
  output of `lint` on purpose — reusing `lint` would serialize `test` behind it
  and couple a lint failure to the test gate.
- Applies to `pull_request` only. On `push` the trimmed Linux-only run from the
  previous commit is already just 4 minutes, and the `before`-SHA edge cases
  (branch creation, force push) would add real risk for very little gain.

### Post-M9: chat file attachments — stage 1 (`/file attach`) (done)
- **A new track** (user request): attach text files to a chat via `/file attach`/
  `/file remove`, with the commands at the top of the help popup's command list.
  Research + plan — [docs/file-attachments.md](docs/file-attachments.md); forks
  **F1–F10 confirmed by the user 2026-07-27**. Branch `feat/file-attachments`
  (stacked on `docs/file-attachments`, the precedent being `feat/generic-import`
  over `docs/plugins-research`).
- **The central question was "inline vs RAG", and RAG loses on three counts** (§3
  of the plan): it **hard-depends on the embedding server** (ADR 0002 — often
  unconfigured, so the feature would simply not work), it is **profile-scoped**
  (a file attached in one chat would surface in every other chat of the profile),
  and — decisively — **retrieval ≠ guaranteed reading**: top-k fragments are the
  wrong model for "summarize this document"/"review this file", and nothing tells
  the model it missed something. Plus `/rag add` **already is** the RAG path, so
  a `/file attach` that indexed into the knowledge base would be a second name
  for an existing command. **The key observation for the user's question "how do
  we make the model read everything it needs": a model doesn't search a store it
  doesn't know exists** — any RAG-backed variant still needs a pointer in the
  prompt. Once a prompt-side block is required anyway, the honest design puts the
  *content* there when it fits.
- **Decision (F1d+F5b): the hybrid in full** — inline block **plus**
  `attachment_read` **plus** a chat-scoped semantic index, delivered as three
  PRs. This stage is the first: entity + commands + extraction + the block +
  modes/budgets + UI. **F11 (where attachment vectors live) — a separate
  chat-scoped index, agreed**: `rag_vectors` is a vec0 table partitioned by
  `profile_id` and applies `k` **inside** the partition, so a `WHERE chat_id`
  join would filter *after* kNN and silently return fewer than `k`; and
  `rag_documents` has no `chat_id` column (`CREATE TABLE IF NOT EXISTS` doesn't
  add columns → a guarded `ALTER` or the first real `DB_STEPS` bump). Reusing the
  profile base would also pollute `/rag list|rebuild|remove` and cross-source
  dedup with per-chat data.
- **Domain**: `Chat.attachments: Vec<Attachment>` (`#[serde(default,
  skip_serializing_if)]` → old chat files read without migration, no schema bump,
  ADR 0006 F12). The **extracted text is a snapshot** stored in the chat file
  (F3): the conversation stays coherent if the file later changes/disappears,
  `build_request` stays synchronous with no I/O, and the chat is self-contained
  for backup/export — the same reasoning behind RAG's `rag_sources`. Sizes are in
  **estimated tokens** (F6, `shared::tokens`) — characters mislead across scripts
  (Cyrillic ≈2 chars/token vs ≈4 for Latin).
- **Delivery**: `request::inject_attachments` (pure, testable) appends the block
  to `ChatRequest.system` (F2) — one code path, no per-provider wire risk
  (Anthropic top-level `system` / Gemini `systemInstruction` / OpenAI
  `instructions` are all already handled), precedent `inject_self_model`, and a
  position at the front of the prefix so the conversation after it stays
  prefix-cached (spec §6.6); re-prefilled only when the attachment set changes.
  The header is in the **profile** language (axis A) and marks the content as
  **DATA, not instructions** (prompt injection, spec §13); **section fences widen**
  (`fence_width`) so a file quoting `>>>` can't close its own section — covered by
  a test.
- **Two modes, and nothing is ever refused for size** (a better story than the
  original "refuse above the budget"): within `max_file_tokens` **and** the chat's
  remaining `max_total_tokens` → **inline** (full text); otherwise → **by
  reference** (metadata + head excerpt; exhaustive reading arrives in stage 2).
  Refusal is reserved for a missing file, a directory, >32 MB, undecodable
  content, or empty content.
- **Formats (F8): any valid UTF-8** plus html/pdf/docx via the extractors RAG
  already uses — RAG's extension allowlist is wrong here, since the most obvious
  attachment is a source file (`main.rs`, `config.toml`, a log). Extraction reuses
  `orchestrator/rag.rs::read_source_text` (promoted to `pub(super)`), which stays
  in `app` because it reaches into `features/tools/web` — a sideways import
  `features → features/tools` is forbidden by FSD (the documented precedent from
  the RAG html/pdf/docx work).
- **Reading runs in a background task** (`spawn_blocking` + an internal
  `attach_tx` channel, mirroring `title_tx`): a large PDF must not block the
  orchestrator's command loop. The orchestrator — the sole owner of `Chat` —
  decides the mode against the budget and inserts the attachment; re-attaching the
  same path **replaces** the previous snapshot (idempotent, like re-adding a RAG
  source).
- **UI**: `/file attach|remove|list` parsed by `features/file_command.rs` (a
  direct sibling of `rag_command.rs`, **`remove` and never `delete`** — the same
  wording decision RAG made); a feed note per outcome; a quiet status-bar chip
  `§ files: N (~tokens)` — attachments cost tokens on **every** turn, so the
  standing cost must be visible (the `§` glyph is WGL4 and one column wide, so it
  needs no compat replacement and doesn't shift the hotkey grid — the `♪`
  precedent; an emoji paperclip would). `/file` entries head `HELP_COMMANDS` as
  requested. Budgets — three fields in the settings "Memory" section
  ("Attachments" group).
- **Tests**: entity (token estimate, name/path matching, excerpt on a word
  boundary and never splitting a character, `format_bytes`, serde); parser
  (subcommands, quoted paths, `#N`/name/path resolution, the per-locale error
  gate); injection (no-op when empty, inline vs excerpt, **fence widening**,
  standalone block, per-locale); orchestrator integration through the real `run`
  loop with a capturing backend (the text reaches `system` and the conversation
  stays clean, persistence to the chat file, `/file remove` takes it back out of
  the request, over-budget → by reference + `#N` addressing, re-attach doesn't
  duplicate, a missing file reports an error); screen (the commands aren't sent as
  messages, an invalid one leaves a note, the chip counts only inline weight,
  `/file` first in the help popup). **1330 unit tests green** (+36), **60
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/i18n gates clean.
- **Live run — GO** (Gemma 4 31B q4_0 + bge-m3, external `llama-server`,
  `--jinja`): `file_attachment_e2e_live` — the **baseline** chat (nothing
  attached) answered "couldn't find any information regarding an internal build
  code", while the chat with the file attached answered exactly `ZARYA-7719`;
  the attachment came back `Inline`, 139 B / ~35 tokens. So the block reaches the
  model through the real wire path and is actually used. The mirror half — that
  `/file remove` takes the text back out of the request — is deterministic and
  covered by a unit test with a capturing backend, so it needs no model.
- **Regression — clean**: all **22** orchestrator live e2e smokes green (598 s) —
  memory/self-model/notes/RAG/graph/cross-organ links/control tools/i18n/TTS.
  Worth running in full here because `build_request` sits on **every** generation
  path and its signature changed. Client-level smokes (`OpenAiClient`) were not
  re-run — that layer is untouched.

### Post-M9: chat file attachments — stage 2 (`attachment_read`) (done)
- **Triggered by a live in-app run of stage 1** (user, GPT-5.6): a small PDF
  (131 KB, ~3.5k tokens, inline) worked perfectly — the model read the article
  and reviewed it. A 1.6 MB / ~418k-token TXT went **by reference** (correct —
  inlining would have destroyed the context), but the model **could not read
  it**: it had the excerpt and no reader, so it improvised — `fs_list`,
  `fs_read` (into the sandbox error), four `web_search` calls — and ended with
  "send a few pages or reload the file", which the user cannot do. Six wasted
  tool rounds and an impossible suggestion.
- **Two defects, not one.** The missing tool is stage 2 by design; but the
  by-reference block **not telling the model what is and isn't possible** was a
  stage-1 wording bug of mine. The entry now states the page range, names
  `attachment_read`, and says the file is unreachable by other means — pinned by
  a regression test (`by_reference_entry_tells_the_model_how_to_read_the_rest`).
- **`attachment_read(name, page)`** (`features/tools/attachment.rs`): returns one
  page of the stored snapshot with a `name — page N of M` header. **Pages, not
  character offsets** (fork F12): discrete and enumerable, so the model can walk
  `1..M` and *know* it read everything — the guarantee retrieval cannot give.
  Failure paths answer usefully instead of erroring: an unknown name **lists what
  is attached**, an out-of-range page **reports the real count** — so the retry
  can succeed. No gate, enabled by default: unlike `fs_read` this **narrows**
  access (only what the user explicitly attached, never the filesystem).
- **Plumbing**: `TurnInfo.attachments`/`ToolContext.attachments` as
  `Arc<[Attachment]>` — the turn snapshot pattern already used for
  `system_message`; `Arc` because `ToolContext` is `Clone` and texts can be
  hundreds of KB. Background loops (reflection/consolidation) pass an empty
  snapshot — they run outside a chat turn. `page_tokens` rides `ToolParams` from
  config, with a settings field next to the other attachment budgets.
- **Token accounting corrected** (also from the screenshot): the chip showed
  a cost of `~0` for a by-reference file. Technically it carried no inline text, but
  its excerpt **is** re-sent every turn, so "free" was a lie. New
  `Attachment::prompt_tokens` — inline: the whole file, by reference: the
  excerpt — and the chip/`/file list` report that. The **budget** still counts
  inline text only (that is what `max_total_tokens` governs); the two figures are
  deliberately different and documented as such.
- **A real pagination bug caught by its own test**: the cut landed *before* the
  separator, so a line break started the next page instead of ending the current
  one, shaving a word off every page (pages came out as `["line", " one\nlin",
  "e two\nlin", …]` — every page starting with the previous one's separator). Fixed to
  cut *after* the separator, with a quarter-budget floor so a boundary near the
  start doesn't waste the page. Pagination is lossless (`pages.concat() == text`)
  and never splits a character — both pinned by tests.
- **Tests**: entity (pagination is lossless / prefers line breaks / never splits
  a character; a short or empty text is one page and `page 1` always exists;
  by-reference cost is the excerpt, non-zero); tool (walking `1..M` reassembles
  the file byte for byte; `page` defaults to 1; unknown name lists attachments;
  out-of-range reports the count; per-locale description gate); injection (the
  by-reference entry names the tool and the range, an inline one doesn't);
  orchestrator (the turn snapshot actually carries the chat's attachments, and
  the tool is registered under its wire name). **1341 unit tests green** (+11),
  **61 `#[ignore]`** (+1), clippy `-D warnings`/fmt/i18n gates/`cyrillic_scan`
  clean.
- **Live run — GO** (Gemma 4 31B q4_0): `attachment_read_e2e_live` — a file
  forced by reference (1454 tokens, `prompt_tokens: 59` — the excerpt only), the
  answer planted on the **last** page. The model called `attachment_read` **five
  times**, walked the pages and answered `ZARYA-8823`. Exactly the behaviour the
  live stage-1 run lacked. **Regression — clean**: all **23** orchestrator live
  e2e smokes green (718 s), the turn snapshot/`ToolParams` changes touching every
  tool path.
- **Along the way**, `excerpt` and `paginate` were deduplicated onto a shared
  `cut_point` — they had grown two independent "back off to a character, then a
  word boundary" implementations that had already drifted (half- vs
  quarter-budget floor, and one kept the separator while the other dropped it).
  One visible consequence: an excerpt now **ends with** its separator, like a
  page (harmless — a newline follows it in the prompt). Both attachment live
  smokes were re-run after the refactor.

### Post-M9: chat file attachments — stage 3 (`attachment_search`) (done)
- **Completes the track** ([docs/file-attachments.md](docs/file-attachments.md)).
  Stage 2 made a big by-reference file **readable** (`attachment_read` walks
  pages `1..M`), but it did not make it **searchable**: on the user's real 1.6 MB
  file (~280 pages), finding a specific place by paging is hopeless — a dozen-plus
  rounds, and `max_tool_rounds` runs out first. Stage 3 adds a **chat-scoped
  semantic index** and `attachment_search`. The two are complementary, not
  redundant: search answers *where* to look, `attachment_read` guarantees
  *everything* can be read.
- **Fork F11(a) — a separate index, not a `chat_id` column on `rag_documents`**
  (confirmed by the user 2026-07-27). Three technical reasons, in order of weight:
  (1) `rag_vectors` is a vec0 table partitioned by `profile_id` and the `k`
  constraint applies **inside** the partition — a `WHERE chat_id` in the join
  would filter *after* kNN and silently return fewer than `k` hits; (2) the column
  doesn't exist and `CREATE TABLE IF NOT EXISTS` can't add one → a guarded `ALTER`
  or the first real `DB_STEPS` bump; (3) the user's profile knowledge base is
  **curated** — one chat's attachments would pollute `/rag list|rebuild|remove`
  and cross-source dedup. New `shared/storage/db/attachments.rs`:
  `attachment_documents` + a vec0 `attachment_vectors` partitioned by **`chat_id`**.
  Scoping by chat is **strictly narrower** than the `profile_id` isolation
  invariant (spec §10.3) — a chat belongs to exactly one profile, so it holds a
  fortiori (documented in the module doc). Purely **additive** DDL
  (`CREATE TABLE IF NOT EXISTS` in `baseline_ddl`) → **no `DB_STEPS` bump, no
  migration** (ADR 0006 F12); the table rides the existing backup as part of
  `data.db`.
- **A shared dimensionality, and the trap it opened.** Per F11 the vector size
  stays one per DB (`meta.rag_dim`): mixing vectors from two embedding models is
  meaningless anyway. `ensure_vec_table` was split into `ensure_dim` (registers
  the shared size, errors on a mismatch) + a per-table `CREATE VIRTUAL TABLE IF
  NOT EXISTS`. That exposed a latent read bug: `rag_search` and `delete_matching`
  used "is a dimension recorded?" as a proxy for "does `rag_vectors` exist?" —
  true before, false now (an attachment can register the size first, leaving RAG's
  table absent → a SQL error on a table that isn't there). Both switched to a real
  `table_exists` check, pinned by a regression test.
- **`rag_reset_vectors` → `reset_vectors`, and it now drops both.** A dimension
  change (`/rag rebuild` with a new model) must drop `attachment_vectors` too,
  **and** delete `attachment_documents`: their vectors are gone and sqlite reuses
  rowids, so surviving rows would join onto whatever lands on those rowids next —
  stale text at wrong distances. Keeping this in one method was deliberate:
  splitting it into two calls at the call site would make the pair forgettable,
  and forgetting it is silent corruption. It returns the number of chunks dropped
  so the rebuild task can `warn` about the loss instead of hiding it. The
  attachment index is **derived** data (the text snapshot lives in the chat file),
  so re-attaching rebuilds it — recorded as roadmap groundwork.
- **Indexing is a background task** (`spawn_attachment_index` in
  `orchestrator/attachments.rs`, the `spawn_rag_ingest` pattern): chunking via
  RAG's own `chunk_text`/`chunk_markdown` (`ChunkParams` from `config.rag` — no
  new settings), sub-batched embedding (`EMBED_BATCH_CHUNKS`, both made
  `pub(super)`), progress through the **same banner slot** the RAG banner uses
  (`RagBanner` — both are "an index is being built in the background", and they
  don't overlap in practice; the field's doc says so, and `is_rag_active` already
  gates the spinner). Only **by-reference** files are indexed (fork F13): an
  inline one is already in the prompt in full, so search would return duplicates
  of what the model can see.
- **Graceful degradation is the load-bearing property** (ADR 0002 pattern): with
  no embedder the index is skipped with a note (`IndexSkipped`), and the block,
  `attachment_read` and everything else keep working. The feature never *depends*
  on RAG being configured — which is the whole reason attachments exist as a
  separate mechanism (§3 of the plan).
- **The block only advertises what exists.** `inject_attachments` gained an
  `indexed: &[Uuid]` argument (one `attachment_indexed_ids` query per turn, and
  only when the chat has attachments): a by-reference entry names
  `attachment_search` **only** for a file that really has an index — promising
  search over an unindexed file is exactly the "sent down a dead end" failure
  stage 2 was created to fix. The tool likewise distinguishes "nothing indexed
  here" from "no hits", pointing at page reading in both cases.
- **No cancellation machinery, by design.** The obvious race — an indexing task
  finishing *after* its file was removed — is closed where it actually matters:
  `attachment_search` filters hits by the **turn's attachment snapshot**, so a
  removed file can never surface, and `attachment_prune(chat, keep)` (called on
  attach and on remove) collects the leftover rows. That replaced a
  `HashMap<Uuid, CancellationToken>` + lifecycle bookkeeping with one DB
  primitive. Along the way `ToolContext.chat_id` finally got a real consumer (its
  `#[allow(dead_code)]` is gone).
- **Tests**: db (chat isolation on kNN — chat B's identical vector must not leak
  into A; delete/prune scoped to one chat; re-index replaces; the shared dimension
  + `reset_vectors` clearing both; the `rag_search`-without-its-table regression);
  tool (finds by meaning and names the file; **hides files no longer attached**;
  reports "nothing indexed" and degrades when the embedder is gone — both pointing
  at `attachment_read`; empty query); injection (search offered only for an
  indexed file, page reading either way); orchestrator through the real `run` loop
  (a by-reference file is indexed and searchable, another chat sees nothing of it;
  an inline file is **not** indexed; `/file remove` drops its index). **1356 unit
  tests green** (+15), **62 `#[ignore]`** (+1), clippy `-D warnings`/fmt/i18n
  gates/`cyrillic_scan` clean.
- **Live run — GO on the first attempt** (`attachment_search_e2e_live`, Gemma 4
  31B q4_0 + real bge-m3): a 240-item document with the payload buried at item
  121, indexed into 29 fragments; the model made **one** `attachment_search` call
  and answered `ZARYA-4417` in ~9 s. That is exactly the stage's criterion —
  "finds the right place by meaning in one call instead of paging through".
- **The regression run turned up the stage's most interesting finding — in the
  stage-2 smoke.** `attachment_read_e2e_live` failed: with an embedder configured
  the by-reference file is now indexed too, and the model **stopped walking pages
  entirely** — one `attachment_search` call, correct answer (`ZARYA-8823`). Not a
  defect: it is the feature working, and the narrow "must call `attachment_read`"
  assertion had simply become wrong. Rewritten as two turns: turn 1 keeps the real
  stage-1 regression (the answer is found **and** the model stays inside the
  attachment tools — no `fs_read`/`web_search` improvising), turn 2 asks for a
  specific page, which search cannot answer, keeping the guaranteed path covered
  live. Both green (turn 2 quoted page 1's first line). Worth remembering as a
  pattern: a new capability can invalidate an older smoke's *assertion* while
  improving its *outcome*.
- **Regression — clean otherwise**: the remaining **23** orchestrator live e2e
  smokes green (571 s) — memory/self-model/notes/RAG/graph/cross-organ
  links/control tools/i18n. Worth the full set here: `build_request` gained an
  argument, `rag_search`/`delete_matching` changed their "is anything indexed?"
  guard, and `reset_vectors` was renamed and widened.
- **Follow-up after the merge — numbering the search fragments.** A live in-app
  run (GPT-5.6 over a real 1.6 MB collection, 1441 fragments indexed) showed the
  feature working end to end — including the epistemics we were after: the model
  answered *and* volunteered that "the file is 281 pages, so this is a choice
  among the candidates I found, not the result of reading the whole collection".
  But the **result format didn't survive real data**: a fragment is a whole chunk
  (~800 chars) and is routinely multi-line, while `- [name] ` marked only its
  first line — ten fragments ran together into one wall of text, boundaries lost
  both for the reader in the feed and for the model parsing the result. Now each
  fragment is numbered, its text starts on its own line, and a blank line
  separates them (`1. [name]\n<text>`). The number separates, it doesn't address —
  no tool takes a fragment index, and the comment says so. Deliberately **not**
  routed through `present.rs`'s markdown path: file fragments are arbitrary text,
  and markdown would turn a leading `#`/`- ` into headings and lists. No CHANGELOG
  entry — the feature itself is still in `[Unreleased]`, so this is polish on
  something nobody has seen released. **1357 unit tests** (+1), gates clean; a
  pure formatting change, no live run needed.

### Post-M9: rag_search — the same fragment separation, and a rendering defect it uncovered (done)
- **Asked for as "do the same for `rag_search`"** (numbering the fragments, after
  the same fix landed for `attachment_search`). Porting it blindly would have
  made things **worse**, so the format was checked against the real renderer
  first — three probes, and each overturned an assumption:
  1. `rag_search` results go through `present.rs`'s **markdown** path
     (`PROSE_RESULT_TOOLS`), unlike `attachment_search`, which is `Plain`. So in
     the feed markdown collapses a multi-line fragment into one item line anyway
     — the numbering alone would have changed `-` into `1.` and nothing else.
  2. Worse: putting the fragment's text on its own line lets a **block construct
     inside the fragment escape its list item**. A `## Heading` renders as a
     document heading in the middle of the tool result and splits the fragment in
     two — and `chunk_markdown` **deliberately prepends a section heading to every
     `*.md` chunk**, so this is the common case, not a corner one.
  3. And the probe showed the defect **already exists today**: a heading on any
     line after the first breaks out of the current `- [source] …` bullet just the
     same. A pre-existing bug, not one the change would have introduced.
- **So the fix is the one `attachment_search` already had**: `rag_search` leaves
  `PROSE_RESULT_TOOLS` and renders `Plain`, and its passages get the same
  numbering (`1. [source]
<text>`, blank line between). Both halves are needed —
  numbering without plain rendering is invisible, plain rendering without
  numbering leaves the boundaries unmarked. Fragments of the user's files are
  **data, rendered verbatim**; that markdown was ever applied to them was the
  actual mistake.
- **`web_search`/`fetch_url`/`note_recall` keep markdown**: their payload is prose
  (summaries, the user's own notes), not verbatim file content. The "linked notes"
  block inside `rag_search`'s result also stays a plain `-` list — notes carry
  real ids for addressing, so numbering them would add a second, fake handle.
- **Tests**: `rag_search` numbers passages and separates them; a `present.rs`
  regression test pinning both directions — the fragment tools render verbatim,
  the prose tools stay markdown. **1359 unit tests** (+2), gates clean. No live
  run needed: the change is to a result string's shape and to feed routing, both
  covered deterministically (and the underlying search behaviour was verified live
  in the attachment-index stage).

### Post-M9: embedding-model change detection (stage 1) (done)
- **Stage 1 of a new track** (research
  [docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md),
  forks R1–R7 accepted by the user as recommended — options "a" — 2026-07-27;
  branch `feat/embed-model-change-detection`): **detect** that the embedding
  model changed and **invalidate** the vectors it orphaned. No reindexing —
  that is stage 2. ADR 0002 deferred "switching the model requires reindexing"
  from the start; this converts the worst failure mode (silent) into a visible
  one.
- **The finding that drives everything: dimensionality is not identity.** It was
  the *only* signal the app had, and `bge-m3` and
  `multilingual-e5-large-instruct` are **both 1024-d** — so a swap between them
  passed `ensure_dim`, passed `/rag rebuild`'s `dim_changed` check, and passed
  every other guard, while turning retrieval into noise: the same text embedded
  by both scores a cosine of **0.37**, and on a 4-document probe corpus the
  retrieval margin collapsed 3x (0.449 → 0.149), with the *correct* hit after a
  swap (0.315) scoring below an *irrelevant* hit in the healthy run (0.283).
  Silent: no error, no warning, no mismatch.
- **Notes were the worst case, in both directions** — and this is the single
  most valuable fix here, since memory-about-self is the project's flagship
  track. `note_vectors` was **never refreshed by anything**
  (`notes_missing_vectors` returns only notes with *no* vector row, so
  `ensure_note_vectors` backfilled but never refreshed; `/rag rebuild` never
  touched the table). Same dimension → stale vectors silently mixed with fresh
  queries. Different dimension → `db::cosine` returns `0.0` on a length
  mismatch, so semantic recall scored **everything** at 0.0, sorted by a
  constant, and returned **arbitrary** notes as "semantically relevant" while
  every duplicate gate stopped firing. RAG in the same situation fails loudly on
  insert; notes failed silently.
- **Identity is established behaviourally** (`shared/embed_identity.rs`, pure):
  embed a fixed `CANARY_TEXT`, store the vector, compare next time
  (`EmbedFingerprint { canary, model_id }`, `matches()`/`display_id()`).
  Measured on the live pair: same model **1.000000** (both on a repeat call and
  inside a differently-sized batch), cross-model **0.368940** → a margin of
  **0.63**, so `CANARY_MATCH = 0.999` only has to sit above a single provider's
  numeric noise (a cloud provider is not bit-exact the way a local
  `llama-server` is). A canary catches what a config fingerprint cannot: the
  same GGUF path re-pointed at another file, a requantization, or a server
  restarted with different pooling/normalization flags. `model_id` (from the new
  `EmbedSettings::active_model_name()`) is **display metadata only, never the
  trigger** — a generic id or an unchanged name after a file swap makes it
  unreliable alone.
- **A decorator, checked lazily** (`app/orchestrator/embed_guard.rs`):
  `EmbedGuard` wraps `Embedder` and runs the check on the **first real embed
  call** (`tokio::sync::OnceCell::get_or_try_init`). Embeddings are deliberately
  lazy (ADR 0002 — `apply_embed` runs no probe), so there is no startup moment
  when a managed embedding server is known to be up; the first real use is the
  moment it has demonstrably answered. Wrapping also makes the check impossible
  to forget at a call site. A **failed** check is never cached (it retries) and
  never blocks the real call — the call below reports the real error itself.
  Installed in `apply_embed_settings`, which runs at bootstrap and on every
  embedding-settings change, so changing the model in settings re-arms it.
- **Each store gets the cheapest correct route, all of which already existed**:
  **notes** — `note_vectors` dropped; the note text is intact, so
  `notes_missing_vectors` lists them and the existing `ensure_note_vectors`
  backfill re-embeds them on the next semantic path (self-healing within one
  `note_recall`, and it costs the user nothing). **Chat attachments** — index
  dropped; derived data, so `attachment_search` degrades to its `not_indexed`
  answer pointing at `attachment_read` (the guaranteed path) and re-attaching
  rebuilds it. **RAG** — **never touched**: re-embedding it needs the full
  ingest pipeline (stage 2), and it is the user's own data. The affected
  profiles are recorded instead and `rag_search` **refuses** with a message
  naming `/rag rebuild`. **Refusing rather than warning** is the point: the
  vectors are in a different space, so results would be noise dressed up as
  answers.
- **Staleness is per profile, not global**, because `/rag rebuild` is
  per-profile. `/rag rebuild` lifts the mark **right after it deletes the old
  chunks**, not at the end — so it stays correct even if the rebuild is
  cancelled or some sources fail, since nothing old survives either way.
  `/rag remove` lifts it once the base is empty, closing the dead end "removed
  everything, re-added under the new model, still refused" (the mark would
  otherwise only be liftable by a rebuild, which needs sources to rebuild from).
- **Honesty over noise**: the fingerprint is recorded **after** invalidation, so
  an interrupted run redoes it and a healthy launch never re-invalidates; and a
  first run with nothing recorded is **silent** — with no prior fingerprint
  there is no evidence anything is stale, and claiming otherwise would cry wolf
  on every first launch. The user is notified only when something was actually
  invalidated, and only the knowledge base asks anything of them.
- **Storage — three keys in the existing `meta` table**: `embed_canary` (a JSON
  f32 array), `embed_model_id`, `rag_stale_profiles` (a JSON uuid array).
  Purely additive → **no schema bump, no migration** (ADR 0006 F12). New `Db`
  methods `embed_fingerprint`/`set_embed_fingerprint`, `profiles_with_rag_docs`,
  `rag_stale_profiles`/`set_rag_stale_profiles`/`clear_rag_stale_profile`/
  `rag_is_stale`, `note_vectors_clear_all`, `attachment_index_clear_all`; new
  private `meta_get`/`meta_set`/`meta_del` helpers now back `vec_dim`/
  `ensure_dim` too. `reset_vectors` clears all three new keys as well — it is
  the "start completely fresh" primitive, and with no vectors left there is
  nothing to be stale *relative to*, so a leftover fingerprint would report a
  change against data that no longer exists. Corrupt `meta` values (a
  hand-edited `data.db`, an empty canary) deliberately read as "nothing
  recorded" rather than bricking startup; the next launch repairs the record.
- **i18n**: `ui.embed.model_changed`/`ui.embed.rag_stale` (axis B — the notice
  is for the user) and `tool.rag_search.err.stale` (axis A — the refusal is read
  by the model), both bundles.
- **Tests**: fingerprint matching (scaling and numeric noise still match, a
  cross-model figure and any dimension change do not, the canary string is
  pinned against a careless edit — changing it would report a model change for
  every existing installation); the guard against a `SaltedEmbedder` fixture —
  two instances standing for two models at the **same** dimensionality, the case
  no dimension check can see (first run records silently; an unchanged model
  invalidates nothing; a swap drops note vectors while the notes survive; RAG is
  **marked, not deleted**; the check is cached per instance; an unavailable
  embedder records nothing); DB round-trips, corrupt-value degradation, the
  empty-list-removes-the-key invariant, `reset_vectors` forgetting the model;
  `rag_search` refusing on a stale base, naming the fix, and healing after the
  mark is cleared. **1387 unit tests green** (+28), **63 `#[ignore]`** (+1),
  clippy `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (`same_dimension_model_swap_detected_live`, needs
  `MINDFORK_EMBED_URL` + `MINDFORK_EMBED_URL_ALT`; real `llama-server` instances
  holding `bge-m3-Q8_0` on :8001 and `multilingual-e5-large-instruct-q8_0` on
  :8002): the same model twice stayed **silent** — the more important half, since
  a false positive would wipe the note vectors and nag on every launch, and real
  servers are not obliged to be bit-exact the way a mock is — and the
  same-dimension swap was detected, dropped the note vectors, marked the
  knowledge base stale, **left the base itself intact**, and notified the user.
  The smoke also asserts both models report the same dimensionality, so it stays
  meaningful only for the case no existing guard can catch.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (530 s)
  on Gemma 4 31B q4_0 (external `llama-server`, `--jinja`) + bge-m3 — memory/
  self-model/notes/graph/cross-organ links/RAG/attachments/control tools/i18n/
  MCP. Worth the full set here: **every** embedder call now goes through the
  guard, `rag_search` gained a pre-query check, and `vec_dim`/`ensure_dim` were
  rerouted through the new `meta` helpers — so the blast radius is the whole
  memory subsystem, not just the new code.
- **Deliberately not in this stage**: re-embedding in place and per-row
  fingerprints (stage 2, fork R4a) — re-embedding is a *different operation*
  from re-chunking (it needs only the chunk text, which all three stores already
  hold), so it belongs in one DB-global resumable job rather than in
  `/rag rebuild`; and per-model similarity thresholds (stage 3, fork R6a). Keep
  the second-order finding visible: `CONSOLIDATE_SIMILARITY = 0.85`,
  `TRAIT_SIMILARITY = 0.72` and `SUMMARY_OBS_SIMILARITY = 0.62` are calibrated
  on bge-m3, and on e5 an **unrelated** trait pair scores 0.751 (above the 0.72
  gate) while an antonym pair scores 0.887 (above 0.85) — so even a perfectly
  correct reindex would flip the gates from "silently never fire" to "fire on
  everything", trading a silent failure for a loud wrong one.

### Post-M9: embedding-model change — stage 2 (re-embedding in place) (done)
- **Stage 2 of the same track** (research
  [docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md)
  §8, sub-decisions **S1–S5 in §8.1**, recorded before implementation per
  AGENTS.md §1; same branch `feat/embed-model-change-detection`): **re-embed in
  place**, plus the mechanism that replaces stage 1's blunt deletion. Stage 1
  changed what stage 2 is *for* — notes and attachments already heal
  themselves, and `/rag rebuild` already repairs a knowledge base — so the
  target is what stays genuinely broken: a rebuild **loses** legacy rows whose
  stored text is absent and whose file is gone (it counts them as errors and
  drops them), it is **per profile** (a model change means switching into each
  one in turn), and attachment indexes come back only on **re-attach**.
- **Embedding generations (S1, S3) — the core.** A monotonic `meta.embed_gen`
  counter plus an `embed_gen INTEGER` column on the three **plain** tables a
  vector belongs to (`note_vectors`, which holds its vectors itself, plus
  `rag_documents`/`attachment_documents`). The two `vec0` virtual tables are
  deliberately untouched — a virtual table cannot take an `ALTER`, and each
  joins by `rowid` to one of those plain tables, which can. Identity
  itself already lives in the canary; a *row* only needs to say **which
  generation produced it**, so the marker is a small integer, not a vector.
  Writers (`note_vector_upsert`/`rag_insert`/`attachment_insert`) stamp
  themselves **under the lock they already hold** — an unstamped vector is
  therefore impossible to write, and **not one of the ~10 call sites changed**.
  Readers ignore foreign generations (`note_search_semantic`,
  `notes_with_vectors`, `attachment_indexed_ids`, `attachment_search`), while
  `notes_missing_vectors` **lists** foreign-generation notes, so the existing
  `ensure_note_vectors` backfill re-embeds them with no new code at all.
- **So stage 1 stopped deleting anything** (S3): the guard bumps the counter
  instead — **one increment retires the whole database**. `note_vectors_clear_all`
  and `attachment_index_clear_all` are gone. Keeping the rows is what makes
  re-embedding possible at all (it works from the text they already hold), what
  lets attachment indexes come back without re-attaching, and what makes
  switching **back** to the previous model cost exactly nothing — a generation
  the DB has already seen makes its vectors current again, with zero work.
  `reset_vectors` deliberately leaves the counter alone: it is monotonic, and
  reusing a number would make a surviving old row read as current.
- **Schema (S2) — a guarded `ALTER`, not the first `DB_STEPS` bump.**
  `ALTER TABLE … ADD COLUMN` in `baseline_ddl`, made idempotent by
  `PRAGMA table_info` (`column_exists`/`add_column_if_missing`) rather than by
  matching on the "duplicate column name" error string, which would also swallow
  a genuinely different failure. **No `DB_SCHEMA` bump, no step, no migration, no
  pre-migration backup**: a nullable column is additive and backward-compatible —
  every query names its columns explicitly, so an older binary ignores it — which
  is exactly the case ADR 0006 F12 says needs no bump, and `CREATE TABLE IF NOT
  EXISTS` simply cannot express it. A bump would also force a backup of `data.db`
  on every upgrade and exercise never-before-run machinery for a change that
  doesn't need it.
- **`NULL` must read as foreign, and that is the load-bearing detail**: every row
  in a real user's database predates the marker. A plain `embed_gen = ?`/`<> ?`
  evaluates to `NULL` — neither true nor false — and would silently skip exactly
  the rows most in need of the work, so every predicate folds through
  `IFNULL(embed_gen, NULL_EMBED_GEN)` against a `-1` sentinel that can never
  collide (generations start at 1). A corrupt counter reads as the first
  generation — the usual "garbage in `meta` degrades to the natural empty state"
  rule, and the safe direction: rows stamped higher then read as foreign and get
  re-embedded, rather than being served from a space nothing matches.
- **`/reindex` (S4)** — a new top-level chat command
  (`features/reindex_command.rs` + `app/orchestrator/reembed.rs`), **DB-global**.
  Not a `/rag` subcommand: it spans notes, chat attachments and **every**
  profile's knowledge base, so filing it under the knowledge-base family would
  misdescribe its scope; `/rag rebuild` keeps its own meaning (re-chunk one
  profile after a chunking-parameter change). The global scope is **not** a
  breach of the `profile_id` isolation invariant (spec §9.5): the invariant
  governs what one profile's *queries* may see, and the job serves no query — it
  rewrites a row's vector under the partition key the row already carries.
  Trailing arguments are **reported, not ignored** (unlike `/rag list` there's no
  subcommand to disambiguate a typo from, so silence would hide it).
- **Re-embedding is not re-chunking** (research §3) — the whole reason this is
  its own operation. It needs no source text and no chunker, so it repairs legacy
  rows whose file is gone, covers every profile in one run, brings attachment
  indexes back without re-attaching, and **keeps chunk ids stable** so nothing
  downstream is invalidated. One loop over the three stores: *for each row whose
  generation is not current, embed its stored text, replace the vector, stamp the
  generation.* Order within `Store::ALL` is cheapest-first (notes → attachments →
  knowledge base): notes restore memory almost immediately, and the base is both
  the largest and the one held back by a stale mark until the end anyway.
- **Resumable by construction**: stamping a row removes it from the queue
  (`ORDER BY rowid` + `LIMIT`, batches of `EMBED_BATCH_CHUNKS`=16), so an
  interrupted run leaves a consistent partial state and a rerun continues exactly
  where it stopped. Inside `set_vector` the step order is load-bearing:
  `ensure_table` **first** (a dimensionality mismatch must fail before anything is
  written, or the row would be stamped current while holding the old vector); then
  skip a row that is gone or belongs to another partition (the user may delete a
  source between the job reading a batch and writing it back — an ordinary race,
  and inserting anyway would orphan a vector on a rowid sqlite later reuses);
  then **vector, then stamp, never the reverse** (a crash between the two makes
  the row look foreign and it is simply redone). Two loop guards: an embedder
  failure or a mismatched vector count is **fatal** for the job (retrying would
  spin on the same batch forever), and a batch that wrote **nothing** breaks that
  store (the queue would otherwise return the same rows forever).
- **The stale marks stay, and keep doing the honesty job**: `rag_search` still
  refuses while a base is mixed. `/reindex` lifts them only when the queue is
  **genuinely empty** — derived from `count_rows_to_reembed`, not from "the loop
  ran" — so a cancelled or partly failed run correctly leaves search refused. The
  stage 1 notice and the `rag_search` refusal now name `/reindex`.
- **A dimensionality change needs no special case (S5)**: a `vec0` table is
  fixed-width, so the job drops both up front via a new `drop_vector_tables` and
  every row then reads as foreign and takes the same path. Deliberately distinct
  from `reset_vectors`: this one keeps the document rows, the fingerprint and the
  stale marks ("keep the texts, re-embed them"), while `reset_vectors` is the
  "start over" primitive that additionally deletes the attachment rows and forgets
  which model produced everything — losing that distinction would mean a
  dimensionality change silently discarded every chat's index instead of
  rebuilding it.
- **Plumbing**: `AppCommand::Reindex`/`ChatIntent::Reindex`; a new terminal event
  `RagProgress::Reembedded { rows, errors, cancelled }` whose cancelled wording
  says a rerun continues rather than reading like a failure; progress reuses the
  **RAG banner** and the single background-indexing slot (`reset_rag_cancel`), so
  `/reindex` and the `/rag` commands are one-at-a-time by construction. `/reindex`
  is in `HELP_COMMANDS` (`F1`), highlighted as a command in the input box and
  skipped by spellcheck. i18n: 5 `ui.reindex.*` + 2 `ui.rag.reembedded*` +
  `ui.embed.reindex_hint` + `ui.help.reindex` keys, both bundles.
- **Tests**: the counter (starts at 1, increments, survives reopening, corrupt
  reads low); **`NULL` reads as foreign everywhere** (queue readers list it, the
  lazy backfill lists it, and no semantic path serves it); the queue (carries
  partition/text, spans every profile and chat, stable batches with no repeats,
  superseded notes excluded, the count agrees with the list); `set_vector`
  (replaces without duplicating and keeps the rowid, skips a deleted/foreign row,
  refuses a dimension mismatch **without stamping**, works at a new width after
  the drop); `drop_vector_tables` keeps what `reset_vectors` would discard;
  **switching back needs no work**; the guarded `ALTER` is idempotent and keeps
  the previous run's stamps; the job (drains all stores and lifts the mark, "no
  work" reports zero plainly, a dead embedder leaves the queue **and** the mark
  untouched so a rerun redoes it, cancelled-before-start keeps search refused, a
  dimension change handled in the same loop); the parser (bare/whitespace/case,
  trailing args rejected, neighbours like `/rag rebuild` and `/reindexer` are not
  it, per-locale errors); the screen (intercepted on Enter, a malformed one leaves
  a note instead of going out to the model, recognized as a command) and the
  `Reembedded` note (clean/errors/cancelled). **1420 unit tests green** (+33),
  **64 `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (`reindex_restores_retrieval_after_a_model_swap_live`, needs
  `MINDFORK_EMBED_URL` + `MINDFORK_EMBED_URL_ALT`; real `llama-server` instances
  holding `bge-m3-Q8_0` on :8001 and `multilingual-e5-large-instruct-q8_0` on
  :8002, **both 1024-d** — the case no dimension check can see): a corpus and a
  note indexed under bge-m3, the swap detected and everything queued, `/reindex`
  re-embedded 4 rows, the queue drained and the stale mark lifted. The payoff is
  the assertion that matters — **retrieval actually recovered**: an e5 query ranks
  the correct chunk first again and the note is found by semantic recall.
  Reporting success was deliberately not enough for this test.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (501 s)
  on Gemma 4 31B q4_0 (external `llama-server`, `--jinja`) + bge-m3. The full set
  is the right scope here: four readers gained a generation predicate
  (`note_search_semantic`, `notes_with_vectors`, `attachment_indexed_ids`,
  `attachment_search`), **every** vector writer now stamps, and `baseline_ddl`
  gained an `ALTER` that runs on every open — the blast radius is the whole
  memory subsystem, not just the new code.
- **A process trap worth recording** (cost ~20 min of false debugging): a
  subagent building the crate in a *copy* of the tree poisoned the shared
  `target/`, so `cargo test` ran artifacts compiled from other sources — seven
  tests "failed", six with `Cargo.toml`/`Cargo.lock`/`artwork` **not found**
  (`CARGO_MANIFEST_DIR` baked in from the copy) and one asserting against a
  locale string it had never been compiled with. `cargo clean -p mindfork-rs`
  restored a clean 1420/0. When delegating, keep subagents out of a second build
  of the same crate — or treat a sudden cluster of path-not-found failures as a
  build-artifact symptom, not a code one.
- **Deliberately not in this stage**: per-model similarity thresholds (stage 3,
  fork R6a) — and stages 1–2 make it the *last* thing standing between the app and
  a supported model swap, since a switch is now both detectable and completable.
  `CONSOLIDATE_SIMILARITY = 0.85`, `TRAIT_SIMILARITY = 0.72` and
  `SUMMARY_OBS_SIMILARITY = 0.62` are calibrated on bge-m3; on e5 an *unrelated*
  trait pair scores 0.751 (above the 0.72 gate) and an antonym pair 0.887 (above
  0.85), so a perfectly correct reindex flips the gates from "silently never fire"
  to "fire on everything".

### Post-M9: embedding-model change — stage 3 (per-model similarity thresholds) (done)
- **The final stage of the track** (research
  [docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md)
  §8.2, sub-decisions **S6–S10** recorded before implementation per AGENTS.md §1;
  same branch `feat/embed-model-change-detection`). Stages 1–2 made a model swap
  **detectable** and **completable**; §6 is what still made it *wrong*. The three
  gates that decide when two pieces of memory mean the same thing —
  `CONSOLIDATE_SIMILARITY = 0.85`, `TRAIT_SIMILARITY = 0.72`,
  `SUMMARY_OBS_SIMILARITY = 0.62` — are absolute cosines derived from live runs
  against **bge-m3**, i.e. positions inside *that* model's distribution, not
  universal constants.
- **The measurement, on a fixed probe corpus against both live servers**
  (2026-07-27):

  | | bge-m3 | e5-large-instruct |
  |---|---|---|
  | paraphrase mean | **0.8176** (0.660–0.909) | **0.9456** (0.901–0.973) |
  | unrelated mean | **0.4128** (0.345–0.500) | **0.7897** (0.726–0.834) |
  | usable span | **0.4048** | **0.1559** |

  e5's usable range is **2.6× narrower** — the whole problem in one number: a
  constant tuned inside bge-m3's range lands somewhere else entirely inside e5's.
  Concretely, the trait gate would have fired on **8/8 unrelated** probe pairs.
  Without this stage a *correct* re-embedding would have traded a silent failure
  ("the gates never fire") for a loud wrong one ("the gates fire on everything").
- **Calibration is automatic, not a table (S7).** R6a's "threshold profiles keyed
  by fingerprint" only helps models someone has already measured — an arbitrary
  local GGUF would still be handed bge-m3's numbers, which is the same failure the
  stage exists to fix, just rarer. Instead the corpus is embedded **once**, on the
  very path that already runs exactly once per model (`embed_guard.rs::calibrate`,
  right where the canary fingerprint is recorded): one extra request of 32 short
  strings, and the two means are stored in `meta` beside the fingerprint
  (`embed_cal_unrelated`/`embed_cal_paraphrase` — keys, so **no schema bump, no
  migration**, ADR 0006 F12).
- **An affine map anchored on two measured points (S8)**:
  `t' = u + (t − u_ref)·(p − u)/(p_ref − u_ref)`, with `u_ref = 0.4128` and
  `p_ref = 0.8176` (`REFERENCE_UNRELATED`/`REFERENCE_PARAPHRASE` in
  `shared/embed_calibration.rs`). **bge-m3 maps to itself**, so nothing moves for
  the model the project is tuned on. The thresholds keep their present values and
  meaning (S6) — what changes is only that they are *read* in whatever range the
  active model actually has. The map equalizes **scale**; it cannot equalize
  semantics, and is not meant to.
- **Failure is always downhill (S9) — the property that makes this safe to ship.**
  Nothing calibrated → `SimilarityScale::identity()`, whose `map(t)` returns `t`
  **exactly** rather than through arithmetic that merely ought to cancel out. So
  every existing installation is bit-for-bit unaffected until the model actually
  changes. A calibration that cannot be measured, cannot be read back, or comes
  out degenerate (non-finite, outside the cosine range, or `paraphrase <=
  unrelated` — a zero span would collapse all three gates onto the unrelated mean,
  i.e. make everything a duplicate) falls back to the same identity, and mapped
  values are clamped to a sane cosine range as a backstop. A failed calibration
  can therefore only leave the gates exactly as they are today — never make them
  wilder. Calibration failure is logged, never fatal.
- **The probe corpus is a fixture, not prose (S10)** — `shared/embed_probes.json`
  (`include_str!`), 8 paraphrase pairs and 8 unrelated pairs, deliberately
  **bilingual** (so is the application) and deliberately phrased as the short
  trait/preference/observation statements the gates actually judge: calibrating on
  encyclopaedic prose would measure a different distribution than the one the
  thresholds operate in. It **must never be edited casually** — changing a probe
  silently invalidates `u_ref`/`p_ref` and therefore every mapped threshold, and
  would make already-calibrated installations disagree with freshly calibrated
  ones. The reasoning is in the file's own `_comment` header, and it earns a
  deliberate entry in `tools/cyrillic_scan.py`'s allowlist (measurement data, not
  prose to translate). A malformed fixture degrades to an empty corpus rather than
  panicking (this runs inside a TUI), with a test pinning that the shipped file
  parses.
- **Reading it back**: `Db::similarity_scale()` is **infallible** on purpose — a
  threshold is needed on paths that have no way to report a storage error, and
  every failure has the same right answer, the identity. Four gate sites read it
  (`notes/overview.rs` ×2 — the user-notes and `@self` consolidation overviews;
  `notes/overview.rs::summary_observation_overlaps`;
  `self_model.rs::near_duplicate_traits`), each mapping **once**, outside the
  nested loop it feeds. The two places that *show* the threshold to the model now
  show the **effective** one, formatted `{:.2}` (`format_threshold`): fixed
  precision earns two properties — an uncalibrated overview prints as it did before
  calibration existed (`0.85`, byte-identical), and because rounding is monotonic
  a listed pair (`s >= threshold`) can never *display* below the displayed
  threshold, so the model is never shown a number that contradicts the selection
  it is looking at.
- **Lifecycle**: `reset_vectors` clears the calibration ("start over" — it already
  discards the fingerprint, and the calibration describes the same model);
  `drop_vector_tables` deliberately **keeps** it, because by the time the re-embed
  job drops the tables the guard has already recorded and calibrated the *new*
  model, and clearing there would throw away a fresh correct calibration and leave
  the gates uncorrected for the very model being re-embedded into.
- **Validated on the corpus** — the mapped thresholds fire on the same pairs:
  consolidate **3/8 vs 3/8** paraphrase, trait **6/8 vs 7/8**, summary↔obs
  **8/8 vs 8/8**, and **0/8 unrelated on both models** — against **8/8 unrelated**
  with the raw constants on e5. The residual 6/8 vs 7/8 is real model difference,
  not calibration error.
- **Tests**: the fixture (shape and non-empty probes, a stable flattening order
  `measure` reads back); `measure` refusing a batch that cannot describe the
  corpus (wrong count, an empty vector, mixed widths — a scale guessed from a
  mismatched batch would be a *wrong* scale, worse than none); the identity
  passing thresholds through by **exact** equality; the reference calibration
  coming out as the identity; e5 reproducing the §8.2 numbers (0.85→0.958,
  0.72→0.908, 0.62→0.869) *and* the point of the exercise — e5's unrelated mean
  sits above the raw 0.72 gate and below the mapped one; a narrower range moving
  every gate up while keeping their order; every degeneracy falling back to the
  identity; the clamp; DB round-trip, corrupt values reading as "never
  calibrated", `reset_vectors` forgetting vs `drop_vector_tables` keeping; and the
  four gate sites plus the two display sites. The gate wiring was
  **mutation-tested**: reverting the four comparisons fails four tests
  one-to-one, and reverting only the two display sites fails exactly the two
  overview tests. **1440 unit tests green** (+20), **65 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (`similarity_scale_follows_the_model_live`, needs
  `MINDFORK_EMBED_URL` + `MINDFORK_EMBED_URL_ALT`; real `llama-server` instances
  holding `bge-m3-Q8_0` on :8001 and `multilingual-e5-large-instruct-q8_0` on
  :8002): bge-m3 calibrated to **0.41277 / 0.81764** — matching the reference
  constants to four decimals, i.e. **identity confirmed against a live server**,
  not just against its own arithmetic; e5 to **0.78968 / 0.94561**, mapping
  0.72 → **0.9080** and 0.85 → **0.9581**, exactly the designed values. The
  behavioural payoff is the assertion that matters: an unrelated pair ("values
  brevity" / "the train leaves from platform nine") scores **0.7479** on e5 —
  **above** the raw 0.72, so the raw constant would have called it a duplicate,
  and **below** the calibrated 0.9080, so the corrected gate rejects it.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (543 s)
  on Gemma 4 31B q4_0 (external `llama-server`, `--jinja`) + bge-m3. Two of them
  are the rewired gates themselves — `trait_gate_e2e_live` and
  `summary_obs_overlap_e2e_live` — so the identity guarantee is confirmed end to
  end through the orchestrator on a live model, not only in unit tests.
- **The embedding-model change track is complete (stages 1–3)**: a swap is
  detected behaviourally, nothing is deleted, `/reindex` re-embeds every stored
  vector in place, and the memory gates follow the model instead of one fixed
  calibration. Groundwork left in research §9: e5-style `query:`/`passage:` input
  prefixes (the `Embedder` contract has no notion of input role), vec0 for notes,
  and cross-model migration without re-embedding.

### Post-M9: per-model input prefixes for embeddings (done)
- **The last groundwork item of the embedding-model change track**
  ([docs/research/embedding-input-prefixes.md](docs/research/embedding-input-prefixes.md),
  forks **R1–R6 accepted by the user as recommended — options "a" — 2026-07-27**;
  branch `feat/embed-input-prefixes`). The e5 family expects each input marked
  with its role (`query:`/`passage:`); the `Embedder` contract had no notion of
  an input role — a query and a stored chunk went through the same call. Only
  relevant now that a model swap is actually supported (stages 1–3 of the
  previous track).
- **The measurement came first, and it reshaped the recommendation.** The
  roadmap justified this with one number on a small corpus (margin 0.155 →
  0.186). Re-measured on **40 documents / 14 queries** in the register the app
  actually indexes (notes, `@self` observations, knowledge-base chunks,
  bilingual, with deliberate near-neighbours): on e5 the prefixes changed **no
  ranking at all** — 12/14 top-1 under every convention, MRR moving by 0.001.
  The entire benefit is separation: mean margin +15%, and the **minimum** margin
  **25×** (0.0002 → 0.0056). A 0.0002 margin is an arbitrary tie-break, so that
  part is real robustness — but it is not a correctness fix, and the docs say so
  rather than overselling it. R6 was therefore offered as a genuine "don't
  implement" option; the user chose to implement with the default **off**, so
  existing installations are bit-identical until they opt in.
- **A convention is a 3-way per-family choice, not a boolean.**
  `multilingual-e5-large-instruct` — the model the earlier figure was measured on
  — does **not** use `query:`/`passage:`; the `-instruct` variants want
  `Instruct: <task>` + `Query: ` and a **bare** passage. So that number was
  measured with the wrong convention for that model and still improved. And the
  wrong convention is measurably **harmful**: on bge-m3 (which wants bare text)
  `e5-instruct` costs a rank (11/14 → 10/14) and 31% of the mean margin. Hence
  `EmbedConvention::None` is the default, and the convention is **never** applied
  automatically — when a model change is detected and the new name looks like an
  e5, the notice merely says which convention it suggests (R3a: a hint, never an
  action).
- **The role lives on the call (R1a)**: `Embedder::embed(texts, role)` with
  `EmbedRole { Query, Passage }` — deliberately **no `Default`**, since a
  silently defaulted role is exactly the failure the type exists to prevent. One
  method, 5 impls, and every batch in the codebase is homogeneous except
  web-search reranking, which now issues two requests (a path that already does N
  parallel page fetches).
- **The prefix is applied by a decorator, and its position is load-bearing
  (R2a)**: `EmbedGuard { PrefixedEmbedder { real embedder } }`, built in
  `apply_embed_settings`. The guard's canary and calibration probes therefore go
  **through** the prefixer, which is what makes both traps self-solving rather
  than merely documented:
  - **calibration** — prefixing bge-m3 moves the corpus's unrelated mean +0.097
    and narrows its span 16%, which would silently invalidate
    `REFERENCE_UNRELATED`/`REFERENCE_PARAPHRASE`. Measuring through the same path
    means bge-m3 stays on `none` (constants valid by construction) and e5
    measures its own means under its own convention. The live calibration smoke
    still reproduces 0.41277 / 0.81764 exactly;
  - **detection** — a prefixed canary scores 0.78–0.9965 against a bare one, all
    below the 0.999 detector, so turning prefixes on reads as the change of
    vector space it really is: the generation is bumped, memory re-embeds itself,
    `/reindex` is offered. Verified, not assumed.
- **Two refinements the measurement argued for.** The canary carries
  **`Passage`**: stored vectors are all passage-role, so the passage marker alone
  defines the space the database is in — a change to the *query* marker alters
  retrieval but leaves every stored vector valid and must **not** force a
  reindex, and tracking the passage role gets that granularity right for free.
  And since e5's passage margin to the threshold is only **0.0025**, the
  convention id joins `EmbedFingerprint` as an **exact second trigger** (R5a) —
  it can only *add* detections, never mask one, which is what separates it from
  the config-only fingerprint rejected as D2 in the previous track. A fingerprint
  written before this exists has no convention field and reads as the default, so
  no installation reports a spurious change on upgrade.
- **Role assignment is the specification, not bookkeeping (R4a).** Research §5 is
  a 20-site table, and its load-bearing finding is that **all four calibrated
  gate sites are symmetric passage↔passage** — `self_note_similar` (the
  `add_insight` gate), the `note_save` duplicate gate,
  `summary_observation_overlaps`, `near_duplicate_traits`. They *read* like
  queries but must be passages, which is also why the calibration corpus is
  passage-role. Mismatching one side costs −0.027 on e5 = **17% of its entire
  usable range**. `/reindex` must use the identical role to the original writers,
  or it would quietly re-create the mixed-space problem the previous track exists
  to kill.
- **Wiring**: `EmbedConvention` + `PrefixedEmbedder` in a new
  `shared/embed_prefix.rs`; `EmbedSettings.convention` (`#[serde(default)]` → **no
  schema bump, no migration**, ADR 0006 F12); `meta.embed_convention` beside the
  canary; an "Input prefixes" Choice field in the Embeddings tab (independent of
  the mode — the convention is a property of the *model*, not of where it runs);
  i18n for the field, its description and the hint, both bundles.
- **Tests**: a new `features/tools/embed_roles_tests.rs` — the executable form of
  the §5 table, using a `RoleRecorder` embedder, because a wrong role is
  otherwise **invisible** (it changes no return value and no other assertion);
  the decorator order (a canary embedded through the guard carries the passage
  marker, and so do the 32 calibration probes); a convention switch bumping the
  generation while **keeping** the note; the hint appearing only when it differs
  from what is set; `web.rs` issuing exactly `[Query, Passage]` with the query
  excluded from the page batch; the default convention passing text through
  **byte for byte**; `suggested_for` recognising the family and nothing else; the
  settings field and the config default. **1466 unit tests green** (+26), **66
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/i18n gates clean.
- **A real bug caught by the existing suite**: splitting `web.rs` into two
  requests left the old `results.len() + 1` length check and the `vecs[0]`
  indexing that assumed the query still rode in the same batch — reranking
  silently returned the provider order. `rerank_reorders_results_by_query` failed
  immediately, which is exactly what that test is for.
- **Live run — GO** (`conventions_behave_as_measured_live`, real bge-m3 :8001 +
  e5-large-instruct :8002): e5's own convention widened the relevant/irrelevant
  gap **0.1213 → 0.1789**, and the wrong convention narrowed bge-m3's **0.4406 →
  0.3317** — both directions confirmed on live models, matching the research
  spike. The smoke deliberately asserts on the **margin**, not on top-1: the
  research measured that prefixes change no ranking, so asserting a recovered
  rank would assert something that was never true.
- **Regression — clean**: the three two-server smokes of the previous track (swap
  detection, `/reindex` restoring retrieval, the calibration scale) and all **25**
  orchestrator e2e live smokes green (584 s) on Gemma 4 31B q4_0 + bge-m3. The
  full set is the right scope: **every** embedding call site changed signature,
  and the guard's canary/calibration path was rewired.
- **Groundwork** (research §9): tuning the `-instruct` task string; other
  families' conventions (BGE-v1.5's retrieval instruction, Nomic's
  `search_query:`/`search_document:`) — the design is a table, so a row is cheap;
  per-role calibration is explicitly **not** needed, since all four gate sites are
  passage↔passage.

### Post-M9: readiness probe for the embedding server (done)
- **Symptom** (user report, with a screenshot): the machine hosting the embedding
  server was off (`ping 192.168.1.20` — "Destination host unreachable"), yet the
  chip read **`● embeddings: ready`** (green). **Cause**: the status was derived from the
  configuration, never from the network — in `external` mode the entire "check"
  was a non-empty URL → `ServerStatus::Ready` ([supervisor.rs](src/app/supervisor.rs)),
  and `embed_status` was written exactly once, in `EngineManager::apply_embed`,
  with no channel to update it later. Deliberate at the time (ADR 0002 — RAG is
  lazy) and documented on `EmbedSetup`, with "a real probe is groundwork" recorded
  in the journal; this closes that item. Branch `fix/embed-server-probe`.
- **Fix — the embedding server is now probed exactly like the chat server**:
  `ServerSupervisor::apply_embed` gained `cancel`/`status_tx`/`loc` and returns an
  **immediate** `Connecting`, while a background `/health` probe (the existing
  shared `spawn_probe`) posts the real status. Managed passes `handle.exited()`, so
  a process that dies *while loading* (corrupt GGUF/OOM) is reported at once
  instead of after `MANAGED_READY_TIMEOUT`; external gets `EXTERNAL_READY_TIMEOUT`.
  `EngineManager` gained `embed_status_tx` + `embed_probe_cancel` +
  `set_embed_status` (a stale probe can't overwrite a newer server's status — the
  invariant the chat/impersonation servers already had), and `run` gained a fifth
  `select!` arm → `emit_server_status`.
- **The cloud deliberately keeps `Ready` with no probe** (mirroring
  `cloud_chat_setup`): there's nothing to load and no `/health`. Pinned by a test
  that asserts the channel stays *silent* — otherwise a future refactor could
  quietly start probing an endpoint that doesn't exist.
- **A launch failure is no longer disguised as "not configured"**: managed
  `ServerHandle::launch` failing (e.g. a missing model file) now yields
  `Disconnected(reason)` instead of `unavailable_embed()`. The settings-window chip
  shows the reason (the status-bar chip stays compact, by design). This also let
  the "`apply_embed` has no `loc`, pass the reference locale, the text is log-only"
  workaround go — the reason is now displayed, so it's localized properly (axis B).
- **Nothing gates on the new status** — RAG stays lazy and degrades through the
  error from the call itself; the status is informational. Two doc comments that
  had justified themselves with "there is no probe" were corrected rather than
  deleted: `EmbedGuard`'s reasoning survives intact but for a sharper reason — a
  probe says the *server* answers, never *which model* does, which is precisely
  what the canary exists to establish (and the cloud has no probe at all).
- **A testing trap worth recording**: the natural negative test (probe a dead port,
  expect `Disconnected`) ran **63 s**. Measured rather than guessed — a single
  `probe()` against a closed local port costs **~2.0 s** on Windows (SYN retry),
  and the probe retries until its timeout: 30 × 2 s. `start_paused` was already
  working (the sleeps were virtual); the connect was the whole cost. Fixed by
  making the stub *listen*: a throwaway `TcpListener` that accepts and either
  answers `200` or hangs up — failure is then immediate and the retry sleeps stay
  virtual. 63 s → **2 s**, and the test now covers the happy path end to end as
  well. Second trap inside it: writing the response without first draining the
  request makes the close an RST that discards the response — the healthy stub has
  to `read` before it writes.
- **Tests**: `Connecting` (not `Ready`) for a configured external server — the
  direct regression; the probe posting `Ready` against a live-ish stub and
  `Disconnected` against a silent one; a stale probe sending nothing; a managed
  launch failure surfacing as `Disconnected`; the cloud `Ready` **without** a probe.
  **1472 unit tests green** (+6), **67 `#[ignore]`** (+1), clippy `-D warnings`/fmt/
  `cyrillic_scan` clean.
- **Live run — GO** (bge-m3 on a real external `llama-server --embeddings`,
  `MINDFORK_EMBED_URL`): `embed_probe_reaches_ready_on_live_server` — the one
  question unit tests can't settle is whether a real `--embeddings` server serves
  the `/health` the probe relies on, since a probe that misjudges a *working*
  embedder would be worse than the bug it fixes. It does: `Connecting` →
  **`Ready`**. (The risk was bounded anyway — `probe()` treats `404` as alive, so a
  server without `/health` reads as ready either way — but it needed confirming,
  not assuming.)
- **Regression — clean**: all **25** orchestrator e2e live smokes green (507 s) on
  Gemma 4 31B q4_0 + bge-m3 (external `llama-server`, `--jinja`) — memory/
  self-model/notes/graph/cross-organ links/RAG/attachments/control tools/i18n/MCP.
  The full set is the right scope: `apply_embed` builds the embedder every memory
  path then uses, so "the status is now honest" had to be shown not to have cost
  anything downstream.

### Post-M9: periodic server health monitoring (done)
- **The other half of the previous entry** (user request; design doc
  [docs/server-health-monitoring.md](docs/server-health-monitoring.md), forks
  **F1–F6 confirmed 2026-07-28** — F2/F4 put to the user explicitly, the rest taken
  by recommendation). Branch `feat/server-health-monitoring`, stacked on
  `fix/embed-server-probe`.
- **The framing that shaped the design**: the obvious symptom is a stale chip, but
  the same one-shot probe had a sharper consequence nobody had named — **a server
  that isn't up when the app starts stays unusable until the user intervenes**. The
  initial probe fails → `Disconnected` → `backend_if_ready` refuses → nothing ever
  re-probes, so starting the app before `llama-server` (the ordinary order for a
  local setup) left generation blocked even after the server came up. So the
  feature is really **recovery**, and that's the half users feel; the honest chip is
  the by-product. This drove F2: the interesting question isn't "how fast do we
  notice a failure" (the failing request answers that immediately, with a better
  message) but "how fast do we notice a *fix*".
- **`spawn_probe` became a monitor** (`supervisor.rs`): phase 1 is the existing
  `wait_until_ready` (a managed GGUF loads for minutes), phase 2 keeps watching.
  Cadence `HEALTHY_POLL=60s` / `RECHECK_POLL=5s`, the fast one used while down
  **or** while a failure streak is pending — without that second condition,
  confirming a failure at N=3 would take ~3 minutes instead of ~15 s. Hysteresis
  (`FAILURES_TO_UNHEALTHY=3` down, one success up) lives in a small pure `Health`
  type, so the transition rules are testable without a server or a clock; only a
  **flip** is published, so a steady server never wakes the UI.
- **Managed relaunch** (F4a): a dead child leaves a port no amount of probing will
  revive, so `Orchestrator::relaunch_dead_managed_servers` re-`apply`s it under a
  `RestartBudget` (≤3 per 5 min, cleared on reaching `Ready` — the budget guards a
  crash *loop*, not a machine's lifetime outages). Deliberately a second small
  implementation rather than sharing `McpManager::allow_restart`: unifying them is a
  mechanical refactor and doesn't belong in a behavior change (AGENTS.md §2).
  A launch that fails **synchronously** (the missing-model preflight) publishes no
  status and so never reaches this path — retrying it would be pointless until the
  settings change, and that's also what keeps the relaunch from looping.
- **External/cloud are never relaunched** — we don't own the process; their monitor
  recovers them by itself. Cloud isn't even monitored (F1a): the only way to check
  it is a real API call, which costs money and quota to answer a question the next
  real request answers for free.
- **Live measurements decided three things, none of them guessed**: (1) `/health`
  answers **200 during active generation** on this llama.cpp build (5 probes while a
  600-token completion streamed) → the monitor needn't pause during generation,
  though hysteresis covers builds that answer `503` under load; (2) a **real managed
  child killed externally is reported in ~150 ms** from the exit signal; (3) the
  same process going away *without* the exit signal takes **76 s** (one healthy poll
  + the streak) — which is exactly the gap that justifies watching `exited`.
- **A test that failed for the right reason, worth recording**: the first version of
  the managed smoke killed the child by **dropping the handle** and asserted <10 s.
  It measured 76 s and reported a *probe* error rather than the exit message —
  because dropping the handle is **us** stopping the server deliberately, which the
  process monitor treats differently (and must: that's what a re-`apply` does, and
  it has to stay silent). Wrong premise, right code; the fix was to kill the process
  from outside (`taskkill` on the PID from `netstat`), which then reported in 148 ms.
- **Tests**: pure `Health` (a streak of N−1 does **not** flip — the assertion that
  fails if someone simplifies the counter away; a success mid-streak resets; a steady
  server publishes nothing; the cadence follows suspicion, not just state);
  `RestartBudget` (cap, window pruning, cleared on recovery, independent per server);
  the monitor end to end against a **toggleable** stub listener — `Ready` → the
  server goes away → `Disconnected` → it comes back → `Ready`, all with paused time;
  a steady server stays quiet over 10 virtual polls; the orchestrator relaunching a
  dead managed server until the budget stops it, and never relaunching an external
  one. **1484 unit tests green** (+12), **69 `#[ignore]`** (+2), clippy
  `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (Gemma 4 31B q4_0 + bge-m3, external `llama-server`; plus a
  local `llama-server` + bge-m3 for the managed case):
  `monitor_does_not_flap_against_a_live_server` — the realistic failure mode the stub
  can't speak to is a status that flaps against a *real* server on a *real* network;
  watched for a full healthy interval + margin, **not one spurious status**.
  `managed_child_death_is_noticed_at_once` — **148 ms** (see above).
  **Regression — clean**: all 25 orchestrator e2e live smokes green.

### Post-M9: the remote live e2e gate on HF Inference Endpoints (stages 0–3, done)
- **The mandatory live gate stopped depending on one machine.** AGENTS.md §3
  requires a live run for anything touching engine / memory / tools, and until
  now that meant `run_all_tests.bat` → `http://192.168.1.20:8000/v1`: not
  reproducible by anyone else, not runnable in CI, and a llama.cpp regression
  catchable only by hand. Research
  [docs/research/remote-e2e-gpu.md](docs/research/remote-e2e-gpu.md) (forks
  **R1–R8 accepted by the user as recommended, 2026-07-28**), plan
  [docs/history/remote-e2e-hf.md](docs/history/remote-e2e-hf.md). Branches
  `spike/hf-endpoint-probe` (stages 0–1) and `feat/e2e-hf-runner` (stage 2).
- **Why HF Inference Endpoints and not a rented pod** (R1a): the survey's real
  question was not price but *"can we guarantee the GPU is released when the run
  crashes"*. No rented-pod provider gives one — you build it, and every external
  watchdog is another machine that can also fail. A managed endpoint moves the
  guarantee into the platform: idle auto-scale-to-zero **is** the dead-man's
  switch, so the worst case is one wasted idle window rather than a GPU running
  until someone notices. RunPod is ~4× cheaper per run; at ten runs a month that
  gap buys away an entire class of problem for ~$7. HF also runs **its own
  llama.cpp engine** — a real `llama-server`, so the llama.cpp-specific paths
  (request-body extensions, `--jinja` tool calling, `/health` + `503 Loading
  model`, embedding batch behaviour) are genuinely exercised, which no
  vLLM-backed serverless option would do.
- **Stage 0 — the probe (`tools/hf_probe.py`), verdict GO.** Seven unknowns, all
  settled against real throwaway endpoints for ≈ $0.17, mostly by reading the
  API's own 422 bodies (the script uses raw REST rather than `huggingface_hub`
  precisely so a rejected payload *teaches* the schema). Two findings improved
  the design: **`LlamacppMode` has an `embeddings` value**, so bge-m3 is served
  by the llama.cpp engine itself — the *same GGUF and quantization the
  similarity gates were calibrated on* — instead of TEI as §3.1 of the research
  had reasoned; and **the container `url` is ours to supply**, so the build can
  be pinned to a tag instead of tracking `master`, retiring the reproducibility
  caveat. Also: `ctxSize` is an explicit field (asked 16384, got `n_ctx=16384`),
  not the indirect Max Tokens × Max Concurrent Requests story in the docs, and
  deploy took **21 s** for a 17.65 GB model, because HF serves the weights from
  its own storage.
- **The trap worth remembering:** `EndpointType` is `public | authenticated |
  private`, and the API **silently coerces** an unknown value instead of
  rejecting it. The older wording `protected` produced a `private`
  (PrivateLink-only) endpoint no CI runner could reach — with a 200 and a
  healthy-looking response. The client now refuses to continue when the echoed
  type differs from the requested one.
- **Stage 1 — the enabling change** (~10 lines, useful on its own):
  `shared/api::live_client(url_var, key_var)` replaced six hand-rolled
  `OpenAiClient::new(env)` sites, and `probe()` now sends the key too. An unset
  or empty key sends no header — byte-for-byte the previous behaviour against a
  local `llama-server` — so this only *adds* the ability to point the same
  smokes at any authenticated OpenAI-compatible server. The `probe()` half is
  load-bearing rather than cosmetic: without it an authenticated `/health`
  answers 401, and 401 is not 503, so the probe reported "ready" whatever the
  key was and the two supervisor smokes would have passed for the wrong reason.
- **Stage 2 — the runner.** `tools/e2e_hf.py`: create both endpoints → wait for
  `running` → **wait for `/health`** → run the suite → delete and verify. Three
  departures from the plan's sketch, each earning its keep:
  - **`tools/hf_api.py` — the client was extracted and shared** with the probe
    rather than copied. Two copies of the create payload would drift the moment
    the schema moved, and two copies of the cleanup would mean two places where
    a bug leaks a billing GPU.
  - **The tests are compiled before the GPU exists** (`cargo test --no-run`), so
    a cold runner does not spend minutes of billed L40S time linking, and a
    build error costs nothing at all.
  - **Cleanup sends every DELETE before verifying any of them.** A cancelled CI
    job gives the handler ~7.5 s before SIGKILL; the calls that stop the meter
    must not queue behind a confirmation round-trip for the previous endpoint.
- **`running` is not loaded** — encoded as `wait_healthy()`, and it is the same
  distinction `OpenAiClient::probe()` exists to draw: HF's `running` means the
  container is up while `llama-server` still answers `503 Loading model`.
  Gating on the endpoint state alone fails the suite's first request.
- **The failure drill found a real leak — not the one it was designed to find.**
  The drill script itself crashed on a cp1252 encode error while echoing the
  runner's output; that broke the runner's stdout pipe, and **both endpoints
  leaked**. Root cause: `cleanup()` emptied the name list *before* its first
  `print`, that `print` raised `BrokenPipeError`, and the `atexit` re-entry then
  found nothing to do. Two fixes, both about not depending on being able to
  talk: prints inside cleanup go through a `say()` that swallows I/O errors, and
  **a name leaves the list only once its endpoint is proven gone**, so a
  cleanup that dies partway is retryable instead of amnesiac. Verified against a
  stubbed HTTP layer (deletes with a dead stdout; a crash mid-cleanup leaves the
  rest retryable; DELETEs all precede the verifications).
- **The sweeper fails safe towards keeping.** `e2e-*` endpoints older than 90
  minutes are deleted hourly, but an endpoint whose `createdAt` cannot be parsed
  is **kept and reported loudly**: deleting one could kill a run still using it,
  destroying real work for a false red, whereas a leak is already money-bounded
  by scale-to-zero. The 90-minute threshold must stay above the live job's
  45-minute timeout, or the backstop becomes a saboteur.
- **A checked assumption that was wrong.** The sweeper's decision logic was
  exercised against a fabricated listing before spending anything, and that
  caught a genuine bug: the fractional-second truncation in the timestamp parser
  also ate the digits of the timezone offset. The live API emits exactly
  `"2026-07-28T17:01:14.686Z"`, so this was on the main path, not a corner —
  every endpoint would have read as "age unknown" and never been swept.
- **Workflows:** `e2e-live.yml` (`workflow_dispatch` only — R5a: a live run is a
  considered act, ~$1 and ~25 min, and the non-hermetic smokes would flake
  unattended) and `e2e-sweeper.yml` (hourly `cron`, active only once on the
  default branch). Inputs reach the shell through the environment, never
  interpolated into a `run:` script. The job warms the npm cache before the GPU
  exists, since a cold `npx` can outlast the MCP smoke's 120 s readiness
  timeout.
- **No Rust changed in stage 2** — **1486 unit tests green** (the +2 over the
  previous entry are stage 1's), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan` clean. **No CHANGELOG entry**: dev infrastructure with no
  user-visible effect (AGENTS.md §4).
- **Smoke — GO** (2026-07-28, `gemma-4-31B_q4_0-it.gguf` on **nvidia-l40s** x1 +
  `bge-m3-q8_0.gguf` on **nvidia-t4** x1, aws us-east-1, llama.cpp
  `server-cuda`, `authenticated`, ctx 16384): **69 passed, 0 failed** —
  `cargo test -- --ignored --nocapture --test-threads=1`, both endpoints ready
  in **41 s**, suite **929 s**, total **970 s**, ≈ **$0.62**. Of the 69, **41
  actually exercised the endpoints**; 28 skipped for want of local assets or
  other credentials (11 Python sandbox, 12 cloud keys, 4 `MINDFORK_EMBED_URL_ALT`
  — stage 3, 1 managed `MINDFORK_LLAMA_BIN`). Both endpoints deleted and
  verified gone; `list` empty. This is the first fully green remote run — stage
  0's was 67/2, and both failures were fixed on that branch beforehand.
- **Failure drill — GO**: the runner was signalled **58 s into a live suite**;
  the handler deleted and verified both endpoints, exited 130, and a *separate*
  process confirmed none remained. The only difference from a cancelled CI job
  is which signal arrives (SIGBREAK on Windows, SIGINT there) — the handler and
  cleanup path are the same, and SIGBREAK is registered precisely so the drill
  can be run on a dev box.
- **Stage 3 — the second embedder (done).** Four smokes (`embed_guard` ×2,
  `reembed`, `embed_prefix`) guard the embedding-model-change track and need a
  *second, different* model via `MINDFORK_EMBED_URL_ALT`; without it they skipped
  **while reporting ok**, which is the exact failure a gate exists to prevent. A
  third llama.cpp endpoint on a T4 now serves
  `Ralriki/multilingual-e5-large-instruct-GGUF` / `…-q8_0.gguf` — the same
  quantization as the local stand, since the calibration constants were measured
  against that file, and deliberately a model that is **also 1024-d**, because
  the whole point of `same_dimension_model_swap_detected_live` is that no
  dimensionality check can tell the two apart. **No Rust change was needed**:
  stage 1 had already routed those smokes through `live_client`, key variable
  (`MINDFORK_EMBED_KEY_ALT`) included. **On by default** (`--no-alt-embed` opts
  out) against the plan's "optional" — ~$0.13 of a ~$1 run is the wrong thing to
  optimise when the alternative is four memory-critical smokes silently not
  running.
- **Smoke — GO (stage 3)**, and the interesting part is *how* green: the rented
  endpoints **reproduce the LAN stand's measurements to 3–5 decimals**, which is
  the evidence that matters when swapping the infrastructure under
  calibration-sensitive tests. bge-m3 calibrated to **0.41317 / 0.81843**
  against the reference constants 0.4128 / 0.8176; e5 to **0.78967 / 0.94557**
  against the journal's earlier live figure of 0.78968 / 0.94561 — identical to
  four decimals — mapping 0.72 → 0.9080 and 0.85 → 0.9580 exactly as designed;
  the prefix margins came out `e5 0.1223 → 0.1791` and `bge 0.4417 → 0.3330`
  against 0.1213 → 0.1789 and 0.4406 → 0.3317 measured locally. **5 passed, 0
  failed** (the 4 model-change smokes + the embedding readiness probe), ready in
  108 s, suite 78 s, total 187 s, ≈ $0.15; all three endpoints deleted and
  verified.
- **Still deliberately not done**: a *scheduled* live run; the managed-server
  smoke, which needs a child process of our own and so cannot run remotely at
  all; and the cloud-provider smokes, which need their own keys.
- **Merged in on the way**: `refactor/live-test-log-english` (the live-smoke log
  translation and the print-macro lint that closes the gap by position rather
  than by discipline). Not scope creep — the branches collide by construction:
  both edit `orchestrator/tests/live.rs`, and the stricter `cyrillic_scan.py`
  flagged 47 lines here without the translation, so CI's lint would have gone
  red whichever side landed first.
### Post-M9: live-smoke diagnostic log in English (done)
- **The developer-facing log of the live smokes was half-Russian** — 31 lines of
  `eprintln!` labels across `orchestrator/tests/live.rs` (29) and `tests/mcp.rs` (2)
  ("session 1: tools=…", "self-notes (observations) in DB: …", "gate showed a similar
  observation: …" were all Russian). Now English, matching the convention flipped by
  the english-source migration — and the precedent set there for provisioning progress:
  "it is a developer-facing test log". Branch `refactor/live-test-log-english`.
- **Why it survived the migration**: `tools/cyrillic_scan.py` allowlists test files
  **wholesale** (`path.endswith("tests.rs") or "/tests/" in path`) — a deliberate
  allowance, since these files legitimately hold Cyrillic fixture data and `ru`-locale
  assertions, and the scanner cannot tell a label from a fixture. So the gate was never
  going to catch it; it only became visible once the live suite started running in CI.
- **The scope line is what matters here** — only label text inside print macros moved.
  Untouched: the prompts sent to the model, the `ru`-locale substrings the gate
  assertions match (the `r.contains(…)` checks in `self_model_gate_e2e_live`,
  `summary_gate_e2e_live`, `trait_gate_e2e_live`,
  `self_consolidation_overview_e2e_live` and `recall_includes_self_e2e_live`),
  note/document fixture bodies, the A2 calibration probe pairs, and the TTS
  config/prompt strings. Translating any of those would have quietly stopped the tests
  testing what they test.
- **The one line that named a Russian value in its label** — how many notes cite the
  RAG source — was **not** solved with an opt-out marker but by binding that (Russian)
  source name once to a `const SOURCE` and interpolating it (`notes citing
  {SOURCE:?}`). That deduplicates a literal that had been repeated three times in code
  positions (two DB lookups + the log, where a typo in one would have failed
  confusingly), keeps the log naming the actual source, and leaves the **label** pure
  English. Values are data; labels are prose — interpolation is what separates them,
  and it is now the documented escape hatch.
- **A mislabel fixed in the same log** (the A2 calibration smoke, spotted while
  translating): both loops printed `MATCH?`, but the second is the *non*-match loop —
  so the log was already wrong in English. Now `MATCH?`/`NON-MATCH?`, padded to a
  common width, since the point of that smoke is eyeballing the two groups' cosines to
  place a threshold between them, and misreading which group a row belongs to is
  exactly the error it invites.
- **The gap is now closed by the gate, not by discipline** (`tools/cyrillic_scan.py`):
  test files stay allowlisted wholesale — the scanner cannot tell a fixture from a
  label **by file** — but it now can **by position**. A new `print_fmt_spans` tracks
  the format string of `print!`/`println!`/`eprint!`/`eprintln!` across lines (the
  string routinely opens on the `eprintln!(` line and carries its text on
  `\`-continuation lines), and Cyrillic inside that span translates even in a test.
  Deliberately narrow: **only the first literal**, so a value argument like
  `eprintln!("gate: {}", r.contains("<ru string>"))` is untouched; and
  **`assert!`/`panic!` are excluded**, because their *condition* sits in the same macro
  call as the message and flagging them would hit precisely the ru-locale assertion
  data that must stay. `write!`/`writeln!` too — in tests they usually build an
  expected-value buffer.
- **Mutation-tested in both directions** (a throwaway probe file, since the rule is
  cross-line state that a single-line reading can't confirm): it flags a single-line
  Russian label, a label on a **continuation line**, and one whose format string opens
  on the line *after* the macro; it does not flag ru-locale assertion data, fixture
  prompts, a Cyrillic literal in a value argument, an interpolated value, or a line
  carrying the `cyrillic-ok` opt-out. Repo-wide the rule has a **clean baseline** — a
  survey before writing it found exactly one candidate line, the one now interpolated.
- **No live run needed** (AGENTS.md §3): these are log strings and a lint — no behavior
  change, no engine/memory/tool path touched, and the changed lines only execute under
  `--ignored`. `--all-targets` compiles the ignored tests, so the `const`/interpolation
  change is still compile-verified. **1484 unit tests green** (count unchanged —
  string-only), 69 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan` clean. No
  CHANGELOG entry (§4: purely internal tests/tooling). Convention recorded in
  **AGENTS.md §3**.

### Post-M9: what the first real CI run of the remote gate found (done)
- The gate's first `workflow_dispatch` on GitHub (run 30396557877) is the run
  that mattered: the pipeline worked end to end — build → three endpoints →
  health gating → suite → cleanup, ~24 min inside a 45-min timeout, **62
  passed** — and everything it got wrong was invisible from a developer machine.
  Branch `fix/e2e-gate-ci-findings`.
- **A false `CLEANUP FAILED`, and it is the serious one.** The DELETE returned
  200 and the verification GET **~400 ms** later still saw the endpoint: HF
  acknowledges the delete and the endpoint disappears a moment afterwards. Only
  the *last* endpoint hits it — cleanup sends every DELETE before verifying any
  (the ~7.5 s SIGKILL window), so the earlier ones get incidental delay for free.
  Two consequences, the second worse than the first: the loudest signal in the
  system cried wolf over an endpoint it had genuinely deleted, and **exit 3
  masked the four real test failures**. The fix rests on a distinction worth
  keeping: **the DELETE is the action, the GET is only the proof**, so only the
  proof may wait — `verify_gone` now polls for ~10 s, which is safe on every path
  precisely because the meter is already stopped. A real leak still exits 3
  (pinned by a test).
- **The alternate embedder scaled to zero mid-suite** → the smoke that woke it
  got a `503`. Systematic, not a flake: `embed_guard` uses it early and
  `embed_prefix` some twenty minutes later, past the 15-minute idle window. The
  local stage-3 run could not have caught this — `--filter embed` packed all four
  smokes into 78 s. The obvious fix (widen the window) was the wrong one:
  **that window *is* the leak ceiling**, the thing that bounds a crashed run and
  the reason this platform beat a rented pod. One knob was doing two jobs. A
  keep-alive daemon thread now issues a real inference request every 5 minutes
  while the suite runs, keeping `lastUsedAt` fresh and leaving the guarantee
  intact (user's decision, 2026-07-29); pings are best-effort and can never fail
  the run.
- **Two smokes cannot run on a GitHub runner at all**, and were failing for
  environment facts rather than regressions: `plays_generated_tone_live` needs a
  sound card (ALSA finds none on a headless runner), and
  `live_search_returns_results` needs an IP that search engines do not throttle —
  a datacenter one is throttled far harder than a home one. Both now **skip** on
  the condition, as the sandbox and cloud-key smokes already do; a gate that is
  permanently red for an environment fact stops being read. The web one matches
  the **bundle key** rather than the prose, so it survives the locale, and leans
  on a distinction the tool already drew between "throttled" and "broken".
- **Left alone**: `simple_generation` returned an empty response once. The same
  model and stack passed it forty minutes earlier locally, so it is a flake until
  it recurs — worth naming rather than silently hardening around.
- **Smoke — GO, in the environment that produced the failures** (run
  30399806549, a second dispatch on the fix branch — three of the four are
  invisible anywhere else): **66 passed, 0 failed**, suite 1120 s, endpoints
  ready in 257 s. Each fix left its own evidence: `keepalive: 3 round(s) of
  pings` with the alternate embedder never leaving `running`, so
  `conventions_behave_as_measured_live` passed; `skip: no audio device
  available`; `skip: every search provider is throttling this IP`; all three
  deletes verified `gone` and exit 0. `simple_generation` passed, which is what
  makes calling it a flake honest rather than convenient.
- Gates green: **1486 unit tests**, 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan` clean. No CHANGELOG entry — dev infrastructure and tests
  (AGENTS.md §4).

### Post-M9: broken documentation links, and a gate that stops them recurring (done)
- **28 relative links in the docs pointed at nothing**, and had for a while.
  Nobody noticed because a stale link is *valid Markdown*: `cargo`, `clippy` and
  `cyrillic_scan` all pass, and it only 404s when someone clicks it on GitHub.
  Branch `fix/markdown-links`.
- **The cause is procedural, not careless.** AGENTS.md §4 says a finished track's
  plan moves to `docs/history/` — the file gains a directory level, and every
  relative link *inside* it silently breaks (`../src/foo.rs` starts resolving to
  `docs/src/foo.rs`). Links **to** the plan are the obvious half and do get
  updated; links **inside** it are the half that gets missed.
  `refactoring-solid.md` alone carried 23, because it cites source files
  heavily. One more was a different cause: CLAUDE.md still pointed at
  `src/shared/api/client.rs`, which moved to `openai/client.rs` in the ADR 0004
  Phase 2 split.
- **`tools/link_check.py`** (stdlib, the `cyrillic_scan.py` pattern: tracked
  files, `--list`, non-zero exit) now runs in CI's `lint` job — before the
  toolchain setup, since it needs neither Rust nor ALSA. It is **deliberately
  narrow**, so a green run means something: relative links only (checking the
  network would be slow and flaky), the file part only (heading slugs are a
  rendering detail, and chasing them would fail on every heading edit), and
  **code is skipped**.
- **Skipping code is what removes the need for an exception list.** A first,
  naive scan reported 32 hits, three of which were `[t](u)`, `√[n](x)` and
  `[text](url)` — all inside backticks, i.e. *examples* of links rather than
  links. Blanking fenced blocks and inline spans drops them by construction; the
  real count was **28**, not the 29 I had estimated by eye. Worth the note: the
  tool corrected its author before it corrected the docs.
- **Mutation-tested in both directions**, because a link checker that passes is
  indistinguishable from one that does nothing: re-breaking a single link the way
  a history move does turns it red; a file containing *only* code-span links
  stays green; adding one real link to that same file turns it red again.
- **The gate is placed where the bug is.** The `lint` job runs on every PR
  including a docs-only one (which skips the `test` job), which is exactly when
  links break. AGENTS.md §1 and the §4 table now name the trap and the tool.
- **No live run needed** (AGENTS.md §3): documentation, one CI step and a
  standalone script — no engine, memory or tool path touched. **1486 unit tests
  green**, 69 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`
  clean. No CHANGELOG entry — internal docs and tooling (§4).

### Post-M9: full-text search over chat content — stage 1 (done)
- **The roadmap item "Search within chat content ... via SQLite FTS"**, stage 1
  of 2. Research
  [docs/research/chat-content-search.md](docs/research/chat-content-search.md);
  **forks F1–F6 decided by the user 2026-07-29** (all as recommended). The user's
  framing set the shape: chats stay in JSON, the index goes in a **separate
  database that can be deleted with no risk**, synced in the background, and able
  to hold other cheap-to-recompute data later. Branches
  `docs/chat-search-research` → `feat/chat-content-search`.
- **Everything load-bearing was measured on the real 171-chat dev corpus, not
  estimated** — and measuring is what decided the design twice and caught two
  defects (below). FTS5 turns out to be **already compiled into the bundled
  SQLite 3.53.2** (`ENABLE_FTS5`), so the whole feature needed **no new
  dependency**. Cyrillic folds correctly under `unicode61` *and* under `trigram`
  — the latter verified rather than assumed, since trigram's case folding was
  historically ASCII-only.
- **F1, the one fork the user really had to settle: `trigram`.** The existing chat
  filter is substring (`title.contains`), so users are trained on `естов` finding <!-- cyrillic-ok -->
  `тестовое` — and **FTS5 ships no Russian stemmer** (`porter` is English-only), <!-- cyrillic-ok -->
  so `unicode61` would not find `памяти` from `память`, a daily miss rather than <!-- cyrillic-ok -->
  an occasional one. Auto-appending `*` mitigates but does not close it
  (`память*` still misses `памяти` — they diverge at the last letter). Cost of <!-- cyrillic-ok -->
  trigram: 2× index, a 3-character floor, weak `bm25`. Hence **stage 1 filters
  rather than ranks** — content mode narrows the chat list and the user's existing
  Created/Modified sort still orders it, sidestepping weak ranking entirely.
- **`cache.db`, and why a second file removes machinery rather than adding it.**
  `data.db` holds irreplaceable content, hence ADR 0006 (steps in transactions,
  downgrade guards, pre-migration backups). An index needs **none of it**: a
  version mismatch, a corrupt file or an unreadable schema all have the same right
  answer — delete and rebuild. So `CacheDb::open` **self-heals instead of
  bailing**: a disposable index must never be able to block startup. Two things
  fell out for free: `features/backup.rs` uses an **allowlist**, so the new file
  is excluded from archives with **no code change** (and a restore correctly lands
  without an index and rebuilds), and "delete it" becomes a supported repair
  instead of data loss.
- **Schema — external-content FTS5, decided by probe.** A standalone FTS5 table
  can only carry `chat_id` as `UNINDEXED`, so re-indexing one chat means a full
  scan; external content (`content='messages'`) keeps metadata in a real table
  with a real index, and `snippet()` still works (verified — it reads through to
  the content table). Triggers make a full rebuild ~4× slower, which is the right
  trade: rebuilds are rare and background, deletes happen on every incremental
  write.
- **Message-level diff — the finding that mattered most.** A chat is saved every
  ~800 ms while a reply streams, and re-indexing a large chat wholesale costs
  ~385 ms *per save*. Diffing on `(message_id, text_hash)` reduces a streaming
  save to **one row**. The hash is FNV-1a inline — stable across Rust versions,
  unlike `DefaultHasher`, which would silently re-index everything on a toolchain
  bump.
- **Sync rests on an invariant the project already keeps.** The orchestrator is
  the sole writer of chats, so in-app changes have an exact hook (`flush_saves`)
  and need no polling; only *external* changes — import, restore, a hand edit, a
  deleted cache — need the startup pass, and that is a **stat-only walk**
  (~0.3 ms for the whole directory), so it runs unconditionally on every launch in
  `spawn_blocking`.
- **Two defects that only the real corpus exposed** (both invisible to the
  synthetic tests, which is the lesson):
  - **A lost update.** The startup pass reads a chat, the app saves and indexes
    that same chat, and the pass's older snapshot lands last — the chat silently
    absent from search until the next launch. That is "launch the app and start
    typing", the common case, and on a 3 s pass the window is wide. Fixed with a
    **compare-and-set on the bookkeeping row inside the transaction**
    (`index_chat_if_unchanged`; `index_chat` stays the unconditional primitive the
    live hook uses), plus reading the bookkeeping **before** the directory — the
    other order forgets a chat created between the two reads.
  - **Hidden chats re-parsed on every startup, forever.** They were *forgotten*,
    which drops the bookkeeping too, so the next pass found no record, parsed the
    file again and dropped it again. Soft delete is the only delete here (spec
    §12.3), so that set only grows: **43 of 171 chats** on the real corpus.
    Recording them with an **empty message set** (which both clears their rows and
    records the file state) took the warm pass from 27 ms with 43 re-parses to
    **0 ms with nothing re-parsed**. Pinned by a regression test that asserts on
    the *second* pass — a single-pass test cannot see it — and mutation-tested.
- **Two checks I specified turned out vacuous**, caught by an agent probing
  instead of assuming: scanning an external-content FTS table reads values back
  *through* the content table, so a `LEFT JOIN` can **never** see a stale index
  row (it returned 0 while `MATCH` still returned a deleted message — blind to
  exactly the user-visible bug); and the bare `integrity-check` only verifies the
  index against itself unless passed an explicit `1`. Both now pinned by a
  mutation test against a deliberately-broken trigger, and the deviation is
  recorded in executable form rather than prose.
- **Query escaping is the pitfall most likely to bite.** Raw input cannot reach
  `MATCH`: measured, `C++`, `cost-benefit`, `50%`, `AND` and `(` are all SQL
  errors on ordinary text — and `cost-benefit`/`a:b` are read as **column
  filters**, so the error names a column the user never typed. Every token is
  quoted with inner quotes doubled; tokens under trigram's 3-character floor are
  dropped rather than allowed to zero out the whole query, counted in
  **characters** (a 3-character Cyrillic token is 6 bytes — a byte floor would
  keep 2-character ones; mutation-tested). The rule lives in **one** place,
  `features/chat_search.rs`, called by the orchestrator: `shared/storage` may not
  depend on `features` (FSD), so `CacheDb::search_chats` takes an already-escaped
  query.
- **Contract**: `AppCommand::SearchChats(String)` →
  `AppEvent::ChatSearchResults { query, chat_ids: Option<Vec<Uuid>> }`, where
  `None` means "not a searchable query — do not filter" (deliberately an `Option`
  rather than "all ids", so the event never claims every chat matched);
  `ChatListAction/Intent::SearchContent`. The widget stays dumb — it sends the raw
  query and filters by whatever comes back, so the floor rule isn't duplicated.
  Stale results are still applied (keeping the last set avoids flashing the full
  list between keystrokes).
- **Measured after the fixes** (171 chats, 13.5 MB of JSON): cold pass 3.1 s
  indexing 1213 messages into 12.7 MB; warm pass **0 ms**; `естов` → 14 chats by <!-- cyrillic-ok -->
  infix, `C++` → 26 (an FTS5 syntax error unescaped), `rust память` → 23 (implicit <!-- cyrillic-ok -->
  AND across scripts), a nonsense query → 0, `ми` → below the floor. <!-- cyrillic-ok -->
  **1531 unit tests green** (+45), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3): no
  engine, memory or provider protocol is touched — but the reconciliation was
  nonetheless exercised against the **real corpus**, which is what found both
  defects above.
- **Stage 2** — done, see the next entry.

### Post-M9: chat content search — stage 2, jump and the results screen (done)
- Completes the track ([stage 2 plan](docs/history/chat-search-stage2.md), forks
  **S1–S6 decided by the user 2026-07-29**, all as recommended). Stage 1 answered
  *"which chats mention this?"*; stage 2 answers *"where exactly, and take me
  there."* Split along the risk line: **2a** the jump infrastructure, **2b** the
  screen that rides on it.
- **An investigation of the feed came first, and it paid for itself** — it closed
  off the obvious approach before any code was written:
  - **A jump can only be applied inside `render`.** The per-block render cache
    makes the row offset nearly free (`cache[..idx].lines.len()` summed), but it
    is only valid after a `build_lines` at the current width — width and palette
    are the cache key. So "compute the row in `activate_chat` and call a setter"
    is impossible; a jump is necessarily a **deferred request** consumed by the
    next render.
  - **`FeedMessage` carries no identity, and the projection is lossy.**
    `from_messages` merges consecutive assistant messages of agentic rounds into
    one bubble and drops `Tool`/`System` entirely, so N domain messages become
    M ≤ N feed items. Unrecoverable afterwards — hence `message_ids: Vec<Uuid>`
    recorded *at the merge site*. A single `Option<Uuid>` would have lied about
    merged bubbles.
  - **Highlighting a match inside the feed by source offset is not feasible.**
    The renderer receives `pulldown-cmark` byte ranges and discards them — and
    threading them through would not help, because `normalize_delimiters`
    rewrites the string **before** parsing, so the ranges do not address
    `Message.text` at all; downstream, LaTeX→unicode, mermaid substitution, table
    re-layout, syntect→ANSI, two wrapping passes and rail-prepending each destroy
    the correspondence independently. Stage 1's research flagged this as "worth a
    look"; it is now settled, and fork **S3** took "mark the message, don't
    highlight the match".
- **Three fields, not one — decided by measurement.** The first implementation had
  a single `focus` in `CacheKey`, cleared by manual scroll. Measured on the
  largest real chat: the first scroll after a jump cost **38 ms against 17 ms**,
  because clearing it wiped the block cache and re-ran markdown+syntect over the
  whole chat, scaling linearly with chat size. Splitting it fixed that (**38 → 18
  ms**, identical to any warm frame) — and exposed something worse hiding in the
  original design: since `scroll_to_bottom` would have been what cleared the
  marker, and `push_user_message` calls it, **every send would have re-rendered
  the entire chat**. Final shape: `pending_focus` (consumed by the next render),
  `anchor` (row re-derivation on rewrap, released by manual scroll), `marker`
  (the accent rail — the only one in `CacheKey`, surviving scrolling).
- **A behaviour change that fell out, worth its own CHANGELOG line**: the seven
  `scroll_to_bottom()` sites are now split by *who asked*. User-initiated ones
  (activating a chat, sending, starting a generation) still go to the bottom
  unconditionally; content arriving on its own (tool cards, notes, follow-ups)
  respects `follow`, so a reader who scrolled away is no longer yanked back. Without
  this a jump is worthless — the first tool card would undo it.
- **2b, the screen**: `Ctrl+G` from the chat list's content mode opens results
  **grouped by chat** (fork S2 — 163 hits for a common word is not a flat list you
  scroll), each hit a Rust-built snippet with the match highlighted. `Enter` jumps
  to that exact message; `Esc` returns with the search still live. Capped at 200
  with an honest "showing N of M" — the true total comes from a separate count,
  since the rows only equal the total below the cap. Also in content mode, `Enter`
  on a chat now opens it **at its first matching message** rather than at the end.
- **Snippets are built in Rust, not by SQLite `snippet()`** (fork S4): we already
  store the text, we need byte offsets to highlight the list, and under trigram
  the budget counts 3-grams — 64 "tokens" yields ~70 characters, against a
  documented ceiling this build silently exceeds. The measurement that settled a
  worry: a trigram hit containing no literal token would fall back to the head of
  the message, but on the real corpus that is **0% across every query tried**,
  including `памяти` at 163 hits — so no clever trigram-overlap positioning was <!-- cyrillic-ok -->
  needed.
- **Adding an `ActiveScreen` variant meant auditing nine match sites that compile
  silently.** Two were real: the `SelfModelView` arm would have **replaced the
  results with a self-model snapshot**, and the `Settings` broadcast would have
  left the new screen's theme and UI language frozen. Both are now exhaustive by
  variant, so the next screen is forced to decide.
- **A plan-vs-implementation divergence, caught in review and fixed in the code,
  not the spec.** Fork S2 as agreed said "chats ordered by your existing sort",
  but the screen hardcoded `modified_at` and ignored the list's `Tab` toggle —
  invisible because the default toggle position *is* `Modified`. The sort is now
  threaded through the contract, pinned by a test that asserts the *other*
  position, and mutation-tested. Rewriting the spec to match the code would have
  been the wrong direction.
- **1604 unit tests green** (+73, including the live-run fixes below), 69
  `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3):
  storage, pure logic and TUI rendering — no engine, memory or provider protocol.
  Snippet quality and the miss rate were nonetheless measured against the **real
  corpus**, as in stage 1.
- **The live run found two things, and one of them reversed a fork.** The user ran
  it, and both reports were fair:
  - **The match was highlighted in the results list but not in the feed** you
    landed in — visibly inconsistent the moment you arrive. This was fork
    **S3(a)**, which I had recommended and which they had accepted, so the honest
    answer was to say so *and* revisit: they had now seen it. Implemented as
    **S3(b), post-render span matching**. This is **not** a reversal of §1.4 —
    what that ruled out was mapping *source* byte offsets onto rendered markdown
    (the renderer discards `pulldown-cmark` ranges, and `normalize_delimiters`
    rewrites the string before parsing, so the ranges do not address
    `Message.text`). Matching the text that was actually *rendered* needs none of
    it, which is exactly why the reversal was cheap. The archived plan records the
    revision rather than pretending (a) was never chosen.
    Two consequences the plan had not anticipated, both **over**-highlighting
    where §4a had only foreseen under-highlighting: the role header would have
    matched a plain search for "assistant" in every assistant bubble (excluded),
    and thoughts/tool cards get highlighted although fork F3 indexes
    `message.text` only — so *highlighted* does not mean *this is what matched*.
    Kept, since the word is genuinely on screen, and documented.
  - **`Esc` from a chat opened out of the results threw the results away** and
    went to the chat list. A genuine gap, not a decision — we never designed the
    way back. Now a one-deep back-stack in `runtime` holding the live
    `SearchScreen` (not the query — re-running it would lose selection and
    scroll). Cleared in the **one funnel** every chat-opening route ends in
    (`ChatActivated`) rather than by enumerating routes, and on a *different*
    chat rather than any activation, because `activate()` also rebuilds the same
    chat's feed after a regeneration or `Ctrl+E`. The chat screen learns nothing
    about search: `ChatIntent::OpenChatList` already means "go back", and *where*
    back is, is app-layer knowledge (FSD).
- **A third fix, from disagreeing with an agent's judgement call**: it had left
  the status bar saying `Esc чаты` — reasoning that a runtime→screen flag was <!-- cyrillic-ok -->
  real contract surface for one word. Sound in general, but that word sits in
  exactly the flow the user had just called confusing, and `F1` enumerating both
  meanings does not help someone reading the bottom of the screen. It is
  **derived** from the back-stack once per frame rather than mirrored into state —
  the alternative would have meant writing the rule at four set/clear sites. The
  label was **measured**, which changed the obvious choice: `результаты` costs an <!-- cyrillic-ok -->
  extra row in the hotkey grid at 120 columns, right where everything otherwise
  fits on one line; `к поиску`/`to search` costs none at any width. <!-- cyrillic-ok -->
- **Still open**: in-feed `/` search with next/prev (fork S5 kept it out — a
  different, same-chat interaction), now cheaper still since both the jump *and*
  the highlight machinery (`match_ranges` + span re-splitting) exist — what it
  needs on top is widening the matcher past the focused message and next/prev;
  `cache.db` holding chat-list summaries to remove the 94 ms startup parse.

### Post-M9: in-feed text search (`Ctrl+F`) (done)
- Closes the roadmap's last search item ([plan](docs/history/in-feed-search.md),
  forks **F1–F6 decided by the user 2026-07-29**, all as recommended). Two
  stages: **3a** moved the highlight out of the block cache, **3b** built the
  mode. Done by hand rather than delegated — five consecutive agent runs died on
  transient API 529s.
- **The investigation invalidated the roadmap's own wording, before any code.**
  Both `docs/roadmap.md` and stage 2's fork S5 specified this as "`/`-search".
  `/` is **unimplementable** here: the chat's input box is always focused, and
  typing `/` into an empty box is exactly the gesture that starts a command
  (`/rag`, `/file`, `/tts`, `/reindex`). Gating on empty input does not rescue it
  — that *is* the command-entry gesture. The settings screen can use `/` only
  because it has a top-level state with no focused editor, which
  `keys::is_slash_key` documents as its precondition; the chat never has one. So
  `Ctrl+F`, and both documents were corrected rather than left standing.
- **Stage 3a — the query left `CacheKey`, and that was the bulk of the work.**
  Highlighting every match instead of one is a one-line change; the cost was that
  the query was part of the cache key, and a key mismatch clears every block. For
  an incremental field that meant re-running markdown + syntect + LaTeX + mermaid
  + table layout over the whole chat **on every keystroke** — up to 70 blocks and
  260 K characters. Measured after moving it out: a query change costs **17 ms**,
  a normal warm frame, against **39 ms** for a rebuild. One rebuild per jump
  remains and should — the marker changes the rail colour, which is baked in.
  - The header exclusion had to be rebuilt: it worked by slicing an index into the
    *unwrapped* body, meaningless post-cache. `CachedBlock` now records how many
    **output** rows the header took, counted while the block is built — normally
    one, but a long custom role name can wrap it, so counting output rows is the
    only stable answer. Mutation-tested.
  - **I predicted a bug that does not exist.** Excluding the rail span from the
    match text looked necessary; the mutation **survived**, because
    `highlight_line` derives its offsets from the same concatenation it matches
    over, so including the rail shifts both consistently and the output is
    byte-identical. The parameter was dropped — an untestable precaution is worse
    than none — and the reasoning lives at the call site.
  - Five stage-2 tests moved from asserting on `feed.cache[..]` to asserting on the
    lines handed to the renderer. Not a behaviour change: the cache is query-free
    by design now, and reaching into it was reading an implementation detail. One
    asserted the exact property this stage inverts and was rewritten.
- **Stage 3b — next/prev goes to the matched line, not the message.** The feed
  could only scroll to a block's first row, and stage 2's fork S6 had refused to
  *store* an intra-block offset because a rewrap changes a block's height. Here it
  is **derived every frame** instead, riding the same wrap-accumulating loop the
  jump uses, so S6's objection does not apply. Not a nicety: the largest real
  message is **38,782 characters**, so message-granular stepping would leave the
  viewport unmoved — the test asserts the scroll row moves, not the match index.
- **The counter is asserted equal to the number of highlighted occurrences**, so
  the two cannot drift. It exists because a common word matches **200–300 times in
  a single chat** (measured) — for calibration, the cross-chat screen caps at 200
  hits across *all* chats. Matching runs over what is drawn (fork F2), which is
  what makes that equality possible; the honest costs are that it also covers
  thoughts and tool cards, which the index does not, and misses text the renderer
  reshaped.
- **Three things that fail silently, all tested.** Fast typing arrives as one
  coalesced paste and would have landed in the message being written, so the field
  has its own paste target. `activate_chat` renumbers the feed under any open
  search (`Ctrl+E`, `Ctrl+R`, a rewrite round, a cross-chat jump all funnel
  through it), so the search closes there. And the field stands in for the input
  box rather than taking a layout row, because a fifth constraint would shrink the
  feed and rewrap the chat on open *and* close.
- **Two process lessons, both mine.** A `python` patch script that asserts on one
  pair mid-batch and dies **writes nothing**, silently losing the whole batch — it
  cost a lost early return in `handle_key`, found by a test rather than by eye. And
  **`cargo clippy … | tail` discards the exit code** (the pipeline reports `tail`'s),
  so a `&&` chain sails past a failing gate; that is how a commit landed with
  clippy red. Verified by exit code afterwards and the commit rewritten.
- **1615 unit tests green** (+11), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3): TUI
  rendering and pure matching, no engine, memory or provider protocol.
- **Still open**: highlighting a match the renderer transformed (needs
  `highlight_ranges` threaded through the renderer — stage 2's fork S3(c));
  `cache.db` holding chat-list summaries to remove the ~94 ms startup parse.

### Post-M9: raw HTML blocks render their text (and S3(c) measured, then rejected) (done)

- **Started as the deferred fork S3(c)** — highlight a match the renderer
  transformed (docs/history/in-feed-search.md, stage 2 §1.4) — and the
  measurement redirected it. Branch `docs/feed-highlight-transformed`.
- **Measured exhaustively rather than by guessing queries**: for every message in
  the dev corpus, every alphanumeric run of ≥3 characters in the *source* (what
  the index can match) checked against the *rendered* output (what the feed can
  highlight). Result: **88 of 186 805 words (0.047%)**, across **20 messages of
  1213**. The words settle it — `rightarrow` (the LaTeX command name, not the `→`
  actually on screen), `e0f2fe` (a hex colour in a mermaid `style` line), `graph
  lr`, `500px`, `td`. Nobody searches for those, and the prose around them
  highlights fine. So S3(c) — reworking `writer`/`latex`/`table`/`code` and the
  mermaid path to carry source ranges — was **rejected, not deferred**, and
  recorded as such in the plan (§5) and the roadmap. Along the way a hunch of mine
  was checked and killed: a soft wrap does **not** break a match, because
  `find_matches` matches each query token independently and a token never contains
  whitespace.
- **What the measurement found instead**: 68 of the 88 came from one message and a
  *different* defect. Block-level raw HTML was dropped by the renderer — prose
  included — so it was **invisible in the feed**, not merely unhighlightable.
  Verified directly rather than inferred: `<table><tr><td>visible prose
  here</td></tr></table>` renders to `""`, while inline `<strong>bold</strong>`
  renders fine, because pulldown-cmark delivers inline HTML's inner text as `Text`
  events and a block's as nothing at all. The old code comment ("text between tags
  arrives as `Text`") was right for inline and wrong for blocks.
- **The fix** (new `shared/markdown/html.rs`, the `latex.rs` precedent): the block
  is accumulated whole — a tag can straddle two `Html` chunks — and converted once
  at `TagEnd::HtmlBlock` by a small state machine. Tags stripped;
  `<script>`/`<style>` **content dropped** (otherwise "strip the tags" would dump
  CSS into the conversation — the one way the fix could be worse than the bug);
  entities decoded, unknown ones left visible; whitespace collapsed; block elements
  end the line and `<td>`/`<th>` separate words, so a row reads as one line;
  `<img>` prints alt + URL **exactly as `Writer::end_image` does** for a markdown
  image, so the same picture reads the same either way. Not the markup verbatim (a
  30-row table would become a wall of tags) and not a rebuilt table (a far larger
  feature that would still need this fallback). Not an HTML parser either — no tree
  is built, so malformed markup degrades into text rather than an error.
- **Not reusing `web::extract_readable`**, which does the same job for RAG and
  `fetch_url`: it lives in `features`, which `shared` may not depend on (FSD), and
  pulling `scraper` into the renderer to strip tags is heavy for the job.
- **A pre-existing behaviour deliberately preserved**: a lone `<br>` is an HTML
  *block*, so it now goes through the buffer — `is_break_only` keeps it producing
  the blank line it always did. The buffering arm sits **before** `is_br` on
  purpose: a `<br>` line inside a larger block belongs in the buffer, in order,
  not pushed out ahead of it.
- **A test of mine that was wrong, and was fixed rather than the code**: it
  asserted HTML-block lines fit the panel width. The writer does not wrap
  paragraphs — the feed does — so the assertion was inventing a promise. Replaced
  with the invariant that actually matters: no span carries a raw newline (the
  source is full of them, and one surviving would break the feed's row math).
- **Correcting my own estimate**: I expected this to close 77% of the highlight
  gap. Re-measured after the fix, it closes **26%** — 88 → **65** words. The 23
  recovered are prose (`milestones`, `methodically`, `matters`); what remains from
  that message is attribute names and values (`frameborder`, `background`,
  `500px`), which the converter drops **by design** — they are markup, and showing
  them would be the wrong fix.
- **Verified on the real message**, not only on fixtures: the corpus's one
  HTML-carrying message (1574 characters previously swallowed) now renders its
  table as prose rows, decodes `&amp;` into "What & Why", prints the image as
  "Clarity - With Specflow (clarity.svg)", and leaks no `<td`, `background-color`
  or `500px`.
- **1632 unit tests green** (+17), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **A live run is not required** (AGENTS.md
  §3): this is the markdown renderer — no engine, memory, tool or provider
  protocol is touched. Docs: spec §11.4 (raw HTML) and §11.3.1 (the highlight
  boundary, now with the measured number), architecture §3 (the module map, which
  was also missing `mermaid.rs`), CHANGELOG, roadmap, and the plan's §5.

### Post-M9: confirmation before dangerous tool calls (done)

- **Human-in-the-loop before a tool call that changes something outside the app**
  (roadmap §Tools, one of the five "most valuable next"). Design plan with forks
  F1–F8 — [docs/history/tool-confirmation.md](docs/history/tool-confirmation.md),
  accepted by the user as recommended 2026-07-30 with one addition: the feature
  must be **fully** switchable off. Behaviour — spec §9.8. Branch
  `feat/tool-confirmation`, one PR.
- **Reading the code first moved the work.** The gate itself is trivial:
  `generation.rs` has exactly one invocation site, already wrapped in a `select!`
  with the turn's cancellation token and already carrying three "do not run it"
  branches (disabled / control tool / discarded rewrite round) whose results are
  localized text. What had to be *designed* is that **the agentic loop is a
  background task that only emits events** — every existing internal channel
  (`title_tx`, `imp_done`, `bg_done`) runs task → orchestrator, and nothing yet
  sends anything back *into* one. That is fork F8 and the only architectural
  decision here: the orchestrator holds the in-flight turn's confirmation sender
  and routes `AppCommand::ConfirmTool` into it.
- **Two staleness guards, both of which would otherwise run a tool nobody looked
  at**: a reply whose `generation_id` is not the turn in flight (the user answers
  at the exact moment a turn is cancelled and the next begins — the same guard
  `AppEvent::TokenUsage` already uses), and, within a turn, a reply matched to its
  `call_id` (the model made several calls in one round and they were answered out
  of order). The wait is a `select!` against the cancellation token, so `Esc`
  works with the popup open, and a closed channel reads as a refusal rather than
  as approval.
- **What counts as dangerous is declared by the tool** (`Tool::danger()`, default
  `false`), like `group`/`ui_label`/`gate` — the single-source-of-truth decision
  the project already took for tool metadata. `python_exec`, `fs_write` and
  **every** MCP tool; not reads, not writes to our own storage (notes,
  self-model, RAG, attachments — visible in the UI, profile-scoped, reversible).
  The default is `false` precisely *because* the switch is opt-in: a wrong `false`
  costs a confirmation someone wanted, while a wrong `true` on `current_time`
  would train them to press `Enter` without reading.
- **MCP annotations are deliberately not parsed.** `destructiveHint`/
  `readOnlyHint` are server-supplied, i.e. untrusted: they could only ever
  *relax* a decision, which is the attack. Treating every MCP tool as dangerous
  is both simpler and safer, and the noise is bounded by the feature being
  opt-in and by "allow for this turn".
- **Declining does not cancel the turn**: the model is told (axis A) and the loop
  carries on, so it can explain itself or take another route — ending the turn
  would throw away the text already streamed. `Esc` declines; a second `Esc`,
  with the popup gone, cancels as it always did.
- **An FSD correction caught mid-implementation**: `ToolDecision` was first put
  next to `AppCommand`, but the popup that produces it lives in `screens`, which
  may not import `app`. Moved to `features/tools/confirm.rs` — the same reason
  `RagProgress` lives in `features`.
- **The popup shows the call through `present.rs`** — the same formatting the feed
  will show for it afterwards, so `python_exec` reads as code rather than as a
  JSON blob; long arguments cut with "…" (a decision prompt, not a viewer). It is
  checked **before** the generation gate in the key routing, because unlike every
  other popup it is open precisely while the turn runs.
- **Sub-agents were used at the user's request to save context**: one took the
  setting end to end (config field, settings row, `field_spec` entry, locale keys,
  tests), one took the chat-screen tests. The delicate halves — the channel, the
  gate, the orchestrator integration tests — were kept in the main session. One
  concurrency lesson: an agent editing `screens/chat/**` silently reverted a
  section-number fix applied there mid-flight, so edits to a file an agent owns
  have to be re-applied after it finishes.
- **A section-number collision worth noting**: `§9.7` was already chat file
  attachments, so this became **§9.8** — the references written during
  implementation had to be renumbered, and a `grep` by keyword rather than by file
  was needed to avoid renumbering the attachment ones.
- **Tests**: 6 orchestrator integration tests through the real `run` loop
  (approve, decline, allow-for-turn, a safe tool never asked about, the setting
  fully off, and both staleness guards — each bogus reply says *deny*, so
  honouring either shows up as an unwritten file), plus chat-screen tests for the
  popup and settings tests for the switch. `fs_write` is the tool under test
  rather than `python_exec`: dangerous by the same rule, and whether it ran is a
  fact on disk rather than a sandbox that has to be provisioned.
- **1647 unit tests green** (+14), **70 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **A defect the sub-agent found in its own test, by mutating rather than by it
  passing**: ratatui's `Buffer` `Debug` prints row content **without escaping
  quotes**, so an assertion containing `"` against `format!("{:?}", buffer)` can
  never match — a render test written that way passed even when the popup showed
  raw JSON. Rewritten to join rows by hand. Worth remembering for any future
  buffer assertion carrying a quote.
- **Live run — GO** (Gemma 4 31B q4_0 + bge-m3, external `llama-server`,
  `--jinja`): `tool_confirmation_e2e_live` passed first time — the real model
  reached for `fs_write` on its own, the confirmation arrived carrying the actual
  arguments (`{"content":"ZARYA-5150","path":"note.txt"}`), `Allow` let it
  through and the file was written. That is the half the mocked tests cannot
  cover: that a real model calls the tool at all, so the confirmation appears on
  the path users take rather than only when a fixture forces the call.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (577 s) —
  memory/self-model/notes/graph/cross-organ links/RAG/attachments/control
  tools/i18n/TTS — plus the MCP smoke (`mcp_filesystem_e2e_live`, 14 tools,
  `mcp__fs__read_text_file`). The full set is the right scope: the tool-invocation
  path sits on **every** turn, and MCP tools are now all marked dangerous, so the
  default-off path had to be shown to leave them untouched.

### Post-M9: database compaction on backup and restore (done)

- **`data.db` never shrinks on its own.** Deleted notes, `/rag remove`d chunks
  and an attachment index dropped with its chat all leave free pages that SQLite
  keeps in the file. So a backup was archiving the holes as well as the data, and
  a restore laid them back down. Now `mindfork backup` packs a **`VACUUM INTO`
  copy** of the database instead of the live file, and `mindfork restore`
  compacts what it unpacked — the second half matters because an archive made
  before this existed (or by another tool) is fragmented, and it is also what
  folds in a `-wal` an older archive may carry. Branch
  `feat/backup-db-compaction`. A simple task by AGENTS.md §1 (no cross-layer
  contract, no new dependency), so no design doc — but two things had to be
  measured rather than assumed, below.
- **The choice was `VACUUM INTO`, not an in-place `VACUUM` before copying.** The
  source is only read, so a backup cannot damage what it is backing up; the
  result is a single self-contained file, which is why the `-wal`/`-shm` entries
  are dropped from the archive when it succeeds (their content is folded in)
  rather than packed next to a copy they no longer describe. Restore is the one
  place an in-place `VACUUM` is right: the file is already ours, and rewriting it
  is the whole point.
- **Both paths are best effort, and that is the design, not a shortcut.** A
  `data.db` that cannot be read as a database is packed raw (sidecars included)
  and left alone on restore. A backup that *happens* for a corrupt database is
  worth more than a compact one, and the fallback is exactly the pre-change
  behaviour. Failures are logged (`tracing`), not surfaced — the CLI's own output
  is unchanged.
- **Measured hazard #1 — opening a database is not free.** The fallback test
  failed by finding that a stale `data.db-wal` had *disappeared* from the data
  root: SQLite deletes it next to a file it reads as **zero-page**, and that is a
  VFS-level delete which `SQLITE_OPEN_READ_ONLY` does **not** prevent. A separate
  probe showed a read-**write** open removes it for a valid database too (WAL
  recovery + checkpoint on close). A backup silently mutating the data root is
  not acceptable, so a non-database is now refused **by its header before being
  opened at all**, and a real one is opened read-only. Both guards are
  mutation-tested: dropping the header check fails the "empty" case, dropping
  read-only fails the "a real database" case.
- **Measured hazard #2 — `vec0` is addressed by rowid.** `rag_vectors` /
  `attachment_vectors` join their neighbours by `rowid`, so a renumbering would
  leave search returning the *wrong text* — silent, and invisible to any size
  assertion. `VACUUM` preserves `user_version` and explicit `INTEGER PRIMARY KEY`
  rowids, which is what makes this safe; `vacuum_into_preserves_the_vector_index`
  pins it by searching the compacted copy rather than by trusting the
  documentation.
- **Consequence worth knowing**: a genuinely hot WAL cannot be recovered
  read-only, so compaction is skipped and `data.db`+`-wal`+`-shm` are packed
  together — which is the consistent thing to do anyway. In practice the app
  never enables WAL mode, so this is a corner.
- **Tests**: `db/mod.rs::compact_tests` (the vector index survives; the schema
  version survives; a stale scratch destination is overwritten; in-place
  compaction reclaims pages and keeps the data; a non-database is refused; the
  source directory is untouched across placeholder/empty/real inputs) and
  `features/backup.rs` (a compacted database is packed **without** the sidecars
  and is still searchable, with no scratch file left behind; the raw file is
  packed byte-for-byte when it cannot be compacted; restore compacts a
  legacy-style raw archive; restore leaves an unreadable database alone).
  **1657 unit tests green** (+10), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **Verified against the real 17.2 MB dev `data.db`** — no live model needed
  (nothing touches the engine, and the memory *content* paths are unchanged), but
  a synthetic database cannot answer whether a real one with 1441 attachment
  chunks and real `vec0` indexes survives. The real CLI was run on an isolated
  copy: backup → the packed database has **freelist 3 → 0**, an **identical
  content hash** over every table, `user_version` preserved, and the
  `rag_documents`/`attachment_documents` rowid sets **matching** their `vec0`
  shadow tables; then a hand-built "legacy" archive (raw, uncompacted database)
  was restored into a fresh root and came back compacted with the same content
  hash. The size gain there is small (40 KB) precisely because that database is
  barely fragmented — the gain scales with how much has been deleted.

### Post-M9: settings-screen focus model (done)

- **Reported from real use**: users step from the section list into the parameters
  with `→`, then try to come back with `←` — and on a switch that changes its value
  instead; confused, they press `Esc`, leave the screen entirely, and come back
  trying to remember what they just changed. Plan with forks R1–R6 —
  [docs/history/settings-navigation.md](docs/history/settings-navigation.md) (**user's decision,
  2026-07-31**, all as recommended). Branch `feat/settings-focus-model`, stacked on
  `docs/settings-navigation`.
- **The damage is larger than "a setting changed", and reading the code is what
  showed it.** In "Model/server" `→` lands on field 0, which is the **subsection tab
  strip** — so `←` there switches Assistant → Impersonation and the whole field set
  changes under the user's hands. One `↓` further is `XMode`, and `←` cycles the
  server mode, which is emitted at once as `SaveConfig` and, after the debounce,
  **restarts the server**. So the "way back" was a silent, server-restarting edit
  that the UI offers no way to undo.
- **Root cause — a key collision that cannot be removed while `→` enters.** As long
  as `→` enters the pane, users build the model "`←` leaves it"; but `←` must cycle
  a `Choice` value, and the first field of most sections *is* a `Choice` (`XMode`,
  `ITheme`). The value binding is the one that has to stay (`←/→` is the convention,
  and the tab strip uses it too), so the entry binding is the one that goes.
- **The `Esc` ladder already half existed** — the field editor, the `Choice` popup
  and the `/` overlay all close *into* the pane rather than closing the screen, and
  the footer already read "Esc back", which was strictly speaking a lie. So R3
  completes an existing rule instead of introducing one.
- **Target model, one sentence**: *the arrows change, `Enter` goes in, `Esc` goes
  out, `Tab` switches section.* Four points in `apply.rs`: `Esc` became focus-aware
  (`Fields → Menu`, `Menu → Close`); the menu's `Enter | Right` became `Enter`
  (guarded on a non-empty field set — defensive, no section produces one);
  `Left`'s "fall through to the menu" tail is gone, so the `Left`/`Right` arms are
  symmetric; `move_section` no longer resets the focus (it still resets
  `field_idx` — field sets differ per section). Untouched: the initial focus and
  the `/`-search jump (`Focus::Fields` is exactly right there — "Enter on a result
  goes to that field" — and `Esc` now steps back out of it).
- **The footer became focus-contextual** (`render.rs`) — it is the only place the
  model is stated, so leaving it flat would have made the rules undiscoverable. The
  mechanism already existed (`Del` was already focus-conditional). Three new i18n
  keys in both bundles (`hint.enter_fields`/`hint.close`/`hint.to_sections`);
  `hint.back` **had to be deleted**, not just left unused — the
  `bundle_keys_are_not_dead` gate fails on a dead key.
- **Follow-up from a live run of the branch (user):** the section menu's `▸` marker
  was *shown or hidden* by focus, which flickered and shifted the title text
  sideways on every change, while the field pane's `◆` didn't track focus at all.
  Both now stay put and only their **colour** moves — `success` for the pane that
  holds the focus, `muted` for the other — through one shared
  `helpers::focus_marker_style`, since they are a pair encoding the same fact from
  opposite sides. Green already means "you are here" on this screen (the active
  section's rail, the selected row's rail), so this reuses a meaning rather than
  adding one. Pinned by `pane_markers_stay_put_and_swap_colour_with_focus`, which
  asserts **both** halves — the colours swap *and* the glyph positions are
  unchanged; mutation-tested against restoring either old behaviour (the
  show/hide title fails it with "marker not drawn at all", the fixed-colour `◆`
  with the muted assertion).
- **The test helpers encoded the old rules, and R4 breaks them silently.**
  `goto_section` was documented as "after the call, focus is in the menu (Tab
  resets it)", and `goto_field` builds on that by pressing `Enter`; with the focus
  preserved that `Enter` would open an editor instead of entering the pane — in
  many tests at once. The helper now returns to the sections itself instead of
  relying on `Tab`'s former side effect, and the one test that used `Left` as "back
  to the menu" moved to `Esc`.
- **Mutation-tested, and it corrected one of my own comments.** Each of the five
  changed lines was reverted in turn: `right_does_not_enter_the_field_pane`,
  `esc_steps_out_of_the_field_pane_then_closes`,
  `left_on_a_non_choice_field_is_a_no_op`,
  `tab_preserves_focus_and_resets_the_field_index` and
  `footer_hints_differ_by_focus` each failed for its own revert. The `←`-on-Choice
  test, which I had commented as pinning R2, turned out **not** to distinguish the
  versions — a `Choice` field returns early in both — so its doc comment was
  corrected to say what it actually is: a regression guard that symmetrizing the
  arms didn't break value cycling.
- **Tests**: `esc_closes` split into "closes from the sections" + the ladder test;
  five new behaviour tests (one per adopted fork) + `footer_hints_differ_by_focus`
  (`TestBackend`) + `up_on_the_first_field_stays_in_the_pane` (R5 — unchanged
  behaviour, pinned against a future "helpful" change) + the marker test below.
  **1665 unit tests green** (+8), 70 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`
  clean.
- **A live run isn't required** (AGENTS.md §3): key handling and rendering on one
  screen — no engine, memory, tool or provider path is touched. The established
  precedent for settings-screen work (the whole redesign, stages 1–6).
- **Deliberately out of scope** (fork R6, a follow-up): an accidental edit is still
  saved immediately and can't be undone from the UI. This change removes the main
  *source* of accidental edits, not the consequence — and the existing `•` marker
  means "differs from the **default**", not "I just touched this", so it doesn't
  answer "what did I change?". The cheap candidate is a second marker driven by a
  config snapshot taken when the screen opens; full `Ctrl+Z` would have to interact
  with the server-restart debounce and with profile edits, which travel as a
  different intent. See docs/history/settings-navigation.md §7.

### Post-M9: undoing an edit on the settings screen (done)

- **The follow-up deferred as fork R6** of the focus-model track
  ([settings-navigation.md §7](docs/history/settings-navigation.md)): that change
  removed the main *source* of accidental edits, this one removes the
  *consequence*. Plan with forks U1–U4 —
  [docs/history/settings-undo.md](docs/history/settings-undo.md) (**user's
  decision, 2026-07-31**, all as recommended). Branch `feat/settings-undo`.
- **The finding that made it cheap, and it came from reading the contract rather
  than the screen:** `SettingsIntent::SaveConfig` already carries the **whole**
  working `AppConfig`, and `SaveProfile` a full `ProfileEdit` snapshot of the
  profile's fields. So undo is *restore an older snapshot into the working copy
  and emit the same intent again* — **no new `AppCommand`, no new `AppEvent`, no
  orchestrator change at all**. The three config fields the orchestrator owns
  (`last_active_chat`, `api_keys`, MCP TOFU pins) are already restored by
  `handle_update_config` on every update, so replacing a whole older config
  cannot clobber them — that trap was closed before this feature existed.
- **Recording goes through one funnel, not nine call sites.** Config/profile
  mutations happen at ~9 places (`toggle_field`/`cycle_field`/`apply_text`/
  `reset_field`/`apply_choice`/`toggle_profile_tool`/`apply_profile_text`/persona
  create+delete); hooking each is shotgun surgery and easy to forget when a field
  type is added later. Instead `handle_key` was split into a thin wrapper over
  `handle_key_inner`: snapshot before, dispatch, keep the snapshot **only if a
  `SaveConfig`/`SaveProfile` intent came back**. The §2.1 exclusions
  (API key / assistant-profile create+delete / MCP catalog confirmation) then
  fall out of that `match` instead of needing guards of their own — the same
  "single funnel" property the codebase already uses for `mark_feed_changed` and
  `InputBox::touch`.
- **The snapshot is cheap where it matters**: `pre_edit_snapshot` returns `None`
  unless the key *could* commit an edit (`Enter`/`Space`/`Del`/`←`/`→`/`Ctrl+N`/
  `Ctrl+D`), so typing inside a text editor clones nothing per keystroke — only
  the committing `Enter` does. The transient snapshot holds both stores (the kind
  is only known from the intent afterwards); `record_edit` keeps the relevant
  half, so a *stored* step stays small.
- **Coalescing by field (U2)**: a run of edits to the same field is one step,
  keeping the *oldest* "before" value — cycling `managed → external → openai`
  undoes to `managed` in one press, mirroring `InputBox`'s own snapshot
  coalescing and producing fewer saves (and server restarts) on the way back.
  `None` for persona `Ctrl+N`/`Ctrl+D`, which therefore never coalesce —
  otherwise creating two personas would be undone by a single press.
- **The jump (U4) reuses the search index.** `build_search_index` already
  enumerates every field of every section/subsection *with its rendered value*,
  and `jump_to_selected` already moves section+subsection+field — so undo
  compares the index before and after the restore and lands on the first field
  present in **both** whose value differs. `SearchHit` gained an `id: FieldId`
  for this: positional comparison would be wrong exactly where it matters, since
  changing an engine mode changes *which* fields are visible. Fields that only
  appear or disappear are skipped — they are the consequence of the change, not
  the change — which leaves the mode field itself as the match. `jump_to_selected`
  was split so both callers share `jump_to`.
- **Key layering**: `Ctrl+Z`/`Ctrl+Y` sit **below** the editor/search/choice
  branches in `handle_key_inner`, so while any of those is open the keys stay
  that widget's text undo. Matched by the physical Latin key (`keys::hotkey_char`)
  — layout-independent. Footer gained one entry `Ctrl+Z/Y` in both focus states:
  the settings screen isn't in the `F1` overlay, so the footer is the only place
  these are discoverable.
- **Known consequence, recorded rather than fixed** (plan §5.1): an engine edit
  and its undo each mark a restart, so the debounce coalesces them into **one**
  restart that reloads the server with the values it already had. Correct, merely
  wasteful. Avoiding it means diffing against the *last applied* config rather
  than the previous one — an orchestrator change, deliberately not bundled here.
- **Tests**: 9 behaviour tests, one per fork plus the boundaries — config undo,
  profile undo, coalescing (and that two different fields stay two steps), redo +
  its invalidation by a fresh edit, empty-stack no-op, the API-key exclusion, and
  the jump across sections. **All five changed lines were mutation-tested**;
  worth noting that reverting the key layering also broke the **pre-existing**
  `ctrl_k_clears_and_ctrl_z_restores_multiline_editor`, so the editor's own undo
  is independently guarded. **1674 unit tests green** (+9), 70 `#[ignore]`,
  clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): key handling and screen state on
  one screen — no engine, memory, tool or provider path is touched.

### Post-M9: no server restart when nothing effectively changed (done)

- **Closes the consequence recorded in §5.1** of
  [settings-undo.md](docs/history/settings-undo.md): `handle_update_config` marked
  a restart by diffing the incoming config against the **previous edit**, so an
  engine edit plus its `Ctrl+Z` marked it twice and the debounce produced one
  restart that reloaded the server with the values it already had — on a managed
  server, unloading and reloading a multi-GB GGUF for nothing. Branch
  `fix/idle-server-restart`. No forks — the approach was already recorded in the
  plan, so no design doc (AGENTS.md §1: a simple task).
- **The fix splits two meanings that had been conflated.** The `RestartQueue`
  flag keeps saying "something was edited" (cheap, set at edit time); whether a
  restart is *worth doing* moved to `flush_restarts`, which compares the final
  config against what each server was **actually launched with**. A flag can't be
  un-set, so deciding at flush time is the only place the answer can be right.
- **The recorded identity is (settings, key blob)**, because `apply_*` resolves
  the key itself via `stored_key` — equal settings with a different blob still
  warrant a relaunch. The comparison uses the stored **ciphertext**, never the
  plaintext: no extra copy of a secret is kept, and re-encrypting the same key
  yields a different nonce, so it errs towards restarting — the safe direction.
- **Recorded by `apply_*` itself, not by the flush.** That's what keeps
  `relaunch_dead_managed_servers` correct by construction: it re-applies the same
  settings on purpose after a crash, and must never be skipped. Impersonation
  records in **both** arms of its `match`, including `shared` — that mode is a
  state the server can be *in*, so switching away and back must not read as
  "nothing to do".
- **MCP got the same guard** (`McpManager::is_current`): a re-apply there kills
  and respawns `npx` → node processes, the most expensive one on the screen, and
  it is reached from the same debounce.
- **A deliberate loss, recorded rather than papered over**: since a re-apply now
  needs a real difference, the undocumented trick of "wiggle a settings field to
  reload the server" no longer works — including as a way to pick up a changed
  env-var API key (`api_key_env`). It was never a designed feature and depended
  on the two edits landing on opposite sides of the debounce; the honest fix, if
  wanted, is an explicit "restart servers" action.
- **The test had to prove a restart *didn't* happen**, which can't rest on
  waiting for an absent event. `an_edit_and_its_undo_cost_no_restart` advances
  virtual time past the deadline (under `start_paused` tokio runs the
  orchestrator's timer first, so its flush has provably run), asserts the counter
  is unchanged, and then makes a **genuine** change and checks the counter
  against *its* status event — a deterministic marker that also proves the flush
  path still works. Mutation-tested: restoring the unconditional flush fails it.
  **1675 unit tests green** (+1), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): the change is *when* `apply_*` is
  called, not what it does — the launch path, its arguments and every provider
  protocol are untouched, and `MockSupervisor`'s call counter is exactly the
  observable in question.

### Post-M9: an unhighlighted code block is drawn as a rectangle (done)

- **Reported from a screenshot**: a ` ```text ` block's background followed the
  ragged right edge of every line, so the block read as a stack of bars of
  differing length rather than one panel. Asked for explicitly: the rectangle
  should be **sized to the text, not stretched across the panel** — the rule
  tables already follow. Branch `fix/code-block-rectangle` (a simple task by
  AGENTS.md §1: one module, no cross-layer contract, no new dependency — no
  design doc).
- **Which blocks this is about, and why it is not all of them.** A code block's
  background is `code_style()` = `REVERSED`, pushed onto the line-style stack —
  and **only on the unhighlighted path** (no language, or one syntect doesn't
  know, which is what ` ```text ` is). A **highlighted** block has no background
  at all: the pipeline `as_24_bit_terminal_escaped(.., false)` carries only
  foreground color (ADR 0003). So there is nothing to square off there, and
  padding it would be invisible weight — deliberately left alone, pinned by
  `highlighted_block_is_left_ragged` so the scope decision is a test rather than
  a comment. The visual asymmetry between the two kinds of block predates this
  and is a separate question.
- **The width is known only at the end**, so the block is squared off at
  `end_codeblock`: `Writer.code_start` records the index of the opening fence in
  `lines` (set on the unhighlighted branch only), and `code::pad_code_block`
  takes the rows from there, measures them and pads each with a raw space span.
  Raw on purpose — the background comes from the **line** style, so the padding
  picks it up on its own.
- **One blank column on the right** (`CODE_RIGHT_PAD`, added after the first
  live look at the result — flush text against the background's hard edge reads
  as abrupt). Deliberately asymmetric: a matching column on the left would shift
  the code out of alignment with the fence markers and the surrounding prose.
  Rows are wrapped to `width - CODE_RIGHT_PAD` rather than to `width`, so the
  column survives the one case where the cap would eat it — a block whose text
  fills the panel, i.e. exactly where the edge is tightest. The price is that
  such a line wraps one column earlier; the alternative (keep the column only
  when it happens to fit) would give wide blocks a different look from narrow
  ones, which is the raggedness this whole change is about.
- **Rows are wrapped by the renderer, not left to the feed.** A code line longer
  than the panel would otherwise be split later and its tail would stay ragged
  inside an otherwise rectangular block. Wrapping here makes the feed's re-wrap a
  no-op (every row ≤ `width`) — exactly the invariant tables already rely on. The
  block's width is then `min(widest row, panel)`: after a word-wrap the widest
  row is often narrower than the panel (a 180-column line at panel 20 wraps to
  17), and that is correct — the rectangle follows the content.
- **`trim_row_trailing_ws` is reused from the table code** for a reason that bit
  tables first: `wrap_ranges` "spills" a word-boundary space past the row's edge,
  which would inflate the measured width and defeat the padding. Trailing spaces
  are meaningless in a code block (leading ones — indentation — are not).
- **The fences' `DIM` moved from the line onto its span** (`fence_line`). It had
  to: the padding takes the *line* style, so a line-level `DIM` would make the
  rectangle's top and bottom edges a different shade from its body. Visually
  identical for the fence text itself (the line style is folded into the spans on
  render). Mutation-tested — restoring the line-level `DIM` fails
  `rectangle_padding_is_not_dimmed` and nothing else.
- **A blank line inside the block becomes a full row of the rectangle** rather
  than a gap in it — it falls out of the same padding, and is pinned separately
  because it is the case a reader notices first.
- **Tests**: the rectangle (all rows one width, that width the block's own and
  below the panel, and the rows really carry the background); a blank line filled;
  the blank right column (at a comfortable panel and at one exactly as wide as the
  block's longest line, plus "exactly one column, not a margin"); a long line
  wrapped into the rectangle at four panel widths; the padding not dimmed; the
  highlighted block left ragged. The existing "content not glued to the fence"
  test now compares trimmed text — its point is the *content*, not the trailing
  background. **Mutation-tested**: removing the `pad_code_block` call fails four
  of the new tests **and** the golden `mermaid_fallback_matches_disabled_render`
  (the mermaid fallback pads through `emit_fenced_source`, so the two paths would
  diverge); `CODE_RIGHT_PAD = 0` fails the column test alone — while
  `highlighted_block_is_left_ragged` stays green through both, as a scope pin
  should. **1681 unit tests green** (+6), 70 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): this is the markdown renderer —
  no engine, memory, tool or provider path is touched (the precedent set by
  markdown-refinements and the mermaid work). The result was nonetheless checked
  against the **actual block from the report** (a rendered dump: 11 rows, all 44
  columns wide, every one carrying `REVERSED`).

### Post-M9: Zig code blocks are highlighted (done)

- **Reported from a screenshot**: a ` ```zig ` block rendered as flat text on the
  reverse-video rectangle while the ` ```rust ` block above it was coloured.
  Branch `fix/zig-code-highlighting` (a simple task by AGENTS.md §1: one table
  entry, no cross-layer contract, no new dependency — no design doc).
- **Cause, confirmed by probing the bundle rather than by reading the alias
  table**: `SyntaxSet::load_defaults_newlines` carries **75** syntaxes — the
  Sublime Text defaults — and Zig is not among them. So `resolve_syntax("zig")`
  returns `None` and `start_codeblock` takes the *unhighlighted* branch, which is
  exactly the reverse-video rectangle the previous entry squared off. Nothing was
  broken by that change: an unrecognized language has always landed there.
- **The same probe answered the wider question the report implies**: of ~60
  labels models commonly emit, **36 do not resolve** — `toml`, `dockerfile`,
  `powershell`, `swift`, `scss`, `graphql`, `terraform`, `asm`, `julia`,
  `solidity` and the rest. Zig is one instance of a standing limit, not a
  regression, and the doc comment now states the bundle's size and names Zig
  among the gaps.
- **Which grammar to borrow was measured, not guessed** (the table already has
  the pattern — `typescript → js`, `kotlin → java`): a representative Zig snippet
  was highlighted through the C, C++, Go, Java, JS and Rust grammars and the
  spans compared. **Rust wins**: it colours `const`/`pub`/`fn`, the call name,
  the numeric type names (`u8`/`usize` — spelled as in Rust), numbers, strings
  with `\n` escapes, `//` comments and the operators, missing only
  `try`/`defer`/`var`. Go is the runner-up and the interesting one — it is alone
  in catching `var`/`defer`, but loses `pub`/`fn`/the types **and** paints
  `while` with the function colour, i.e. it is actively misleading where Rust is
  merely silent. C++ catches `try` and little else.
- **Tests**: `zig`/`Zig` added to `language_aliases_resolve_to_syntax`, plus
  `zig_block_is_highlighted` — a behavioural test asserting the *symptom*, that
  the block carries RGB foreground colours and **no** `REVERSED` line style, so
  it pins the path taken rather than the table lookup. **Mutation-tested**:
  removing the alias fails it with the rendered rectangle in the message.
  **1682 unit tests green** (+1), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): this is the markdown renderer —
  no engine, memory, tool or provider path is touched (the precedent set by
  markdown-refinements and the previous code-block entry).
- **Groundwork**: real grammars for the missing languages. syntect can load
  `.sublime-syntax` YAML at runtime (the `yaml-load` feature is on by default),
  so vendoring a few definitions and extending the set once in the `LazyLock`
  would give true Zig/TOML/Dockerfile highlighting — an asset to license and
  maintain, and a separate track from this one-line fix.

### Post-M9: vendored syntax grammars for 19 languages (done)

- **Asked for right after the Zig fix**, which had closed the symptom with an
  approximation and named the real fix as groundwork. Plan with forks F1–F5 —
  [docs/history/vendored-syntaxes.md](docs/history/vendored-syntaxes.md)
  (**user's decision, 2026-07-31**, all four questions as recommended: the
  curated set, vendored files with a manifest, a build-time dump, no user
  overlay yet). Branch `feat/vendored-syntaxes`, stacked on the Zig fix (they
  collide by construction — this deletes the alias that fix added).
- **Everything load-bearing was probed against real grammars before any
  code**, and two of the findings decided the design:
  - **syntect loads only `.sublime-syntax`** — there is no `.tmLanguage`
    syntax loader (`plist-load` is for *themes*), which disqualifies several
    obvious upstreams (`PowerShell/EditorSyntax`,
    `Microsoft/TypeScript-Sublime-Plugin`, `wmertens/sublime-nix`).
  - **`extends:` is unsupported**, so every grammar must be self-contained.
    That is what rules out cherry-picking from the current `sublimehq/Packages`
    — its `TypeScript`/`TSX` extend `JavaScript` and fail to load, as does
    `alexlouden`'s `HCL`. Measured, not assumed: each produced its own error.
- **Cost, measured in release** — the number that chose F3: assembling the set
  at runtime is **130 ms** (24 unlink + 105 build) on the first code block,
  while a prebuilt **uncompressed** dump loads in **0.55 ms** — indistinguishable
  from the 0.57 ms the app already paid for syntect's defaults. The compressed
  form is 4.1 ms for 45 KiB less, so uncompressed wins; it is the same trade
  syntect makes for its own assets.
- **`build.rs` assembles the dump, and a grammar that fails to load fails the
  build.** That is not a slogan — it fired on the first run: the **Swift**
  grammar was rejected over a subroutine call (`\g<1>`). The cause was my own
  optimisation: I had given the build-dependency `regex-fancy`, reasoning that
  a dump stores regex *source* so the engine cannot matter. It does — loading a
  grammar **compiles** its regexes, so the build is also what validates them,
  and fancy-regex rejects what oniguruma accepts. The build-dep now uses
  `regex-onig`, the runtime's engine; validating with a different one would
  reject working grammars and could accept broken ones.
- **The set: 19 grammars** (Zig, TypeScript, TOML, Dockerfile, PowerShell,
  Swift, Kotlin, SCSS, Sass, GraphQL, Terraform, Elixir, Solidity, Julia, Nix,
  Dart, Protobuf, CMake, nginx) in `syntaxes/`, each pinned in `SOURCES.md` to
  an upstream repository, commit and licence, with the licence text vendored
  next to it. `tools/fetch_syntaxes.py` re-fetches from those pins;
  `--check` reports drift and is **not** in CI (it needs network — the
  grammars are vendored precisely so the build does not).
  `sharkdp/bat`'s `.gitmodules` was the shortlist source (a curated,
  syntect-verified list), and three files come from bat's own converted copies
  where no self-contained upstream exists — with the **original** author's
  licence recorded, not bat's.
- **The alias table had to be pruned, and that is a rule, not a cleanup**:
  `resolve_syntax` tries the canonical token first, so while `zig → rs`
  remained, the vendored Zig grammar was **never reached** — measured, `zig`
  still highlighted as Rust after the grammar was added. `typescript → js` and
  `kotlin → java` went the same way. What is left maps only what no grammar
  answers (`docker`, `pwsh`, `hcl`, `proto3`, `jsx`, `tsx`).
- **Binary size: +181 KiB**, measured on the release binary (20 397 056 →
  20 582 400). Worth recording because I had claimed the opposite in a code
  comment — that dropping syntect's `default-syntaxes` would make the binary
  *smaller* — and the measurement refuted it: under LTO the linker already
  drops the unused dumps. The feature is still off (it is genuinely dead
  weight), but the comment now states the measured number.
- **Attribution**: the "Components" tab (`F1`) gained a grammars section, built
  by **parsing the manifest** (`include_str!`) rather than duplicating it into
  a static list — drift is then impossible by construction, which beats a gate
  test that merely detects it. The gate that remains checks what a parser
  cannot: that every vendored file has a manifest row, and every row a licence
  text on disk.
- **Tests**: `vendored_grammars_resolve_by_their_own_label` (27 labels, the
  point being that a vendored grammar needs no alias);
  `dump_carries_the_vendored_grammars` (the dump is the bundled 75 **plus**
  every file — a guard against the build silently degrading to defaults);
  `grammar_manifest_matches_the_vendored_files`; the Components tab scrolled to
  its last page. **All three gates were mutation-tested** — re-adding `zig → rs`
  fails the first, deleting a grammar file fails the other two.
  **1686 unit tests green** (+4), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): the markdown renderer and a
  build step — no engine, memory, tool or provider path is touched. What
  replaces it here is that the build itself validates every grammar with the
  runtime's own regex engine, and the resolution tests assert the exact grammar
  each label reaches.
- **A process lesson worth recording**: `git checkout <file>` to undo a
  scripted measurement mutation reverted **all** uncommitted work in those two
  files, not just the mutation — Cargo.toml's feature trim and code.rs's whole
  change had to be redone from context. Back up the file, or apply the mutation
  as a patch you can reverse.
- **Follow-up, same branch: Vue, Svelte, Nim, V, JSONC** (asked for right after
  the track landed). Three vendored — the set is now **22** — and the other two
  answered by measurement rather than by adding files:
  - **V could not be vendored, and that is a licensing fact, not a preference**:
    the only `.sublime-syntax` for V that exists
    (`elliotchance/vlang-sublime`) has **no licence file and no statement in its
    README**, i.e. all rights reserved. §5 of the plan says such a candidate is
    dropped, so the label maps to **Go** — measured the best of go/rust/c on real
    V code (it shares `:=`, `import`, `struct`, the primitive type names,
    single-quoted strings, `//`). Rust catches `pub`/`fn`/`mut` but reads `'` as
    a lifetime and **mangles V's default string form** — worse than three plain
    keywords. Recorded in `syntaxes/SOURCES.md` under "not vendored".
  - **JSONC needed no grammar at all**: the bundled JSON grammar already
    highlights `//` and `/* */` as comments (measured), so `jsonc`/`json5` → `json`
    is exact rather than approximate. Worth checking before adding a file.
  - **Svelte repeated the SCSS lesson** — `master` fails to load on
    sublime-syntax v2 features, bat's pinned commit works. Vue's grammar lives
    on the `new` branch under a filename with a space (`Vue Component`), which
    is why `fetch_syntaxes.py` now quotes the path.
- **The follow-up produced the gate the original stages lacked**:
  `every_syntax_can_highlight_without_panicking`. Vue and Svelte embed other
  languages by scope, and an unresolved reference is **invisible at load time** —
  it surfaces only while parsing, where a panic would kill the app (we
  deliberately do not `catch_unwind`, see the mermaid module). Highlighting a
  mixed markup/script/style snippet through **every** syntax in the set closes
  that gap; all 100 pass, and Vue/Svelte/Nim were additionally eyeballed —
  embedded JS and CSS inside `<script>`/`<style>` really do highlight.
  **1687 unit tests green** (+1), gates clean.

### Post-M9: a settings hint always fits its panel (done)

- **Reported from a screenshot**: the API-key hint ran past the bottom panel's
  last row and was cut mid-sentence — precisely on the half that says what to do
  ("on another computer the key has to be entered again"). Branch
  `fix/settings-hint-fits` (a simple task by AGENTS.md §1: one screen, no
  cross-layer contract, no new dependency — no design doc).
- **Cause**: the panel was a fixed `Length(4)` — a border plus three content
  rows — deliberately constant so the field list wouldn't jump between fields.
  The description was then handed to `Paragraph`'s `Wrap`, which wraps *after*
  layout, so nothing could know it needed a fourth row. **Measured** rather than
  eyeballed: the longest descriptions are ~310 characters (`mcp_enabled`,
  `sm_protocol`, `spec_type`, `mcp_server`, `embed_convention`, `api_key`), i.e.
  four rows at a typical pane width and more on a narrow terminal — a standing
  limit, not a corner case.
- **Sized per field set, not per field** — that is the whole design decision.
  A height following the *selected* field would fix the clipping and shift the
  list on every step down, trading one annoyance for a worse one; taking the
  **maximum hint** over the current field set keeps the panel constant exactly
  where the user is navigating (it can only change when the field set does — a
  section or subsection switch, which already replaces the list wholesale).
  Bounded on both sides: never below the three rows it has always had (short
  sections look unchanged), never above `HINT_MAX_ROWS = 12` and a third of the
  pane — an **MCP server's tool description is arbitrary server text**, so
  without a ceiling one field could push the list off the screen.
- **The full-value preview now gives way to the hint.** It used to be pushed
  first and could eat the whole panel; the height is reserved for the hint, so
  the preview takes only what the hint leaves. The right priority because the
  value is *also* in the list row above (truncated with `…`) while the hint
  exists nowhere else — and it means a long system message can't inflate the
  panel for a whole section.
- **Pre-wrapping is what makes the measurement possible**: two small helpers
  (`wrapped_rows` counts, `wrap_text` builds) over the existing `shared::wrap`,
  so the panel's content is wrapped **before** the vertical layout instead of by
  `Paragraph::wrap`, which is dropped. Both split on `\n` first — `wrap_line`
  treats a newline as an ordinary zero-width character, so a multiline system
  message would otherwise have run its lines together.
- **Tests**: the reported symptom end to end (the API-key field focused at three
  widths — the whole description present in the rendered pane, not a prefix of
  it), the preview priority (a 400-character value plus a described field: both
  the preview and the *complete* hint are on screen), and the height rule as a
  pure function (floor, growth to the longest, cap). **Mutation-tested**: pinning
  the height back to three rows fails the first, dropping the preview's
  `truncate` fails the second. A test-helper trap worth recording — the pane's
  text has to be extracted **between** the panel's borders, since a `│` at either
  end lands between the joined rows and breaks a match on wrapped text.
  **1690 unit tests green** (+3), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): layout and rendering on one
  screen — no engine, memory, tool or provider path is touched (the precedent set
  by the settings redesign and the focus-model work).

### Post-M9: password-protected backups (done)

- **Asked for directly**: a backup password, settable as a CLI argument *or* in
  the settings, stored machine-bound and encrypted the same way cloud API keys
  are (ADR 0008); restore must take both an archive encrypted with that password
  and an unencrypted one. Plan with forks F1–F8 —
  [docs/history/backup-password.md](docs/history/backup-password.md)
  (**user's decision, 2026-08-01**, all as recommended). Branches
  `docs/backup-password` → `feat/backup-password`.
- **Everything load-bearing was measured against a throwaway probe crate before
  any design was committed to**, and two of the results shaped the code directly:
  - **A password handed to an *unencrypted* archive is discarded by the zip
    layer** (`(Some(_), false) => password = None`). So the requirement's own
    wording — "restore either kind" — needs **no detection branch and no mode
    switch**; one code path does both, which is why the diff is small.
  - **The password is verified when an entry is *opened***, not after reading it
    (AES stores a 2-byte verifier in the entry header). That is what keeps the
    transactional restore intact: `validate_archive` already runs before anything
    destructive, so a wrong password becomes a clean refusal rather than a
    rollback. Had verification only happened at EOF, every entry would have had
    to be read up front.
  - Also measured: writing works at all (`with_aes_encryption`), the plaintext is
    absent from the archive bytes, entry **names are not** encrypted, the two
    failure modes are *distinct* errors (`UnsupportedArchive("Password
    required")` vs `InvalidPassword`), the manifest can stay unencrypted inside
    an encrypted archive, and the ciphertext **is authenticated** — 66 of 66
    single-byte corruptions in an entry's payload detected, **0** silently wrong.
    That last one corrected a first, sloppier probe of mine that flipped a byte
    at `len()/3` and reported no error: the byte had landed outside the entry.
- **F1 — WinZip AES-256 inside the zip, not our own container.** The stronger
  option (ChaCha20-Poly1305 + Argon2id, which would also hide the file names and
  give a KDF we control) was rejected for what a backup *is*: an artifact whose
  job is to be recoverable when the application is not available. Standard AES
  keeps it openable by 7-Zip/WinZip by hand; a private format makes the archive
  depend on this program continuing to exist and run.
- **The limits are written down rather than implied** (module doc, settings hint,
  install.md), in the house style of `shared/secrets.rs`: entry names and sizes
  stay visible (content-only encryption); the KDF is fixed by the format at
  PBKDF2-HMAC-SHA1/1000 and is weak against offline brute force of a short
  password — hence the hint asking for a passphrase; and **a machine-bound
  password plus a dead machine means unreadable archives**, which inverts the
  point of a backup, so the hint says to record it elsewhere. This last one is
  sharper than for an API key, where re-entering is merely an inconvenience.
- **F2 — the existing `AppConfig.api_keys` was reused** under a reserved entry
  key, so `put_key`/`stored_key`/`is_ours` work unchanged and one per-machine
  entry keeps holding everything that machine knows. Renaming the field to match
  its widened meaning is exactly what the additive-only rule (ADR 0006 F12)
  forbids, so the doc comment moved instead of the field. **The reserved name is
  `backup-password`, with a hyphen**: the first version used a dot, and the i18n
  gate correctly flagged it as a bundle key that isn't in the bundle — a real
  false positive, better removed at the source than taught to the gate as an
  exception.
- **F3/F4 — the copies the app makes on its own are covered too.** The
  pre-restore copy inherits the run's one effective password (argument, else the
  setting), and the pre-migration backup (ADR 0006) reads the stored password out
  of the raw `settings.json` `Value` — it runs before storage opens, so there is
  no typed `AppConfig` yet. The reasoning is the same in both places: a setting
  that says "my backups are encrypted" must not have an exception that quietly
  writes a plaintext copy of everything.
- **F5 — the manifest stays unencrypted** (it holds only version metadata), so
  `read_manifest` and the "this backup is from a newer version" warning keep
  working with no password, and the archive stays self-describing.
- **F6 — `restore` prompts for the password** when it is missing or wrong, up to
  three attempts, using `crossterm`'s raw mode (already a dependency — no
  `rpassword`). Deliberately **skipped when stdin is not a terminal**: prompting
  in a pipe or a CI job would hang forever instead of failing with a message.
  Restoring a foreign archive on a fresh machine is exactly the case where no
  stored password can apply, and the alternative is `--password` in the shell
  history.
- **UI — a new "Data" section.** None of Model/Sampling/Tools/Memory/Profiles/
  Interface is about the data root, and a security setting filed under an
  unrelated heading is a setting nobody finds; the section also gives the
  data-location and compaction groundwork items a home. The field itself is a
  mirror of the "API key" row (a *status*, never the value; an empty masked
  editor; `Del` clears), which is what the `is_api_key_field` → `is_secret_field`
  rename and the extracted `secret_row` are for.
- **Secrets still never reach the UI**: `emit_settings` sends a
  `backup_password_present: bool` beside `api_keys_present`, and the orchestrator
  restores `api_keys` on the way back — the round-trip protection that already
  existed is what makes the new flag safe. `handle_set_api_key`'s "encrypt →
  persist → roll back on failure" core was extracted as `store_secret` and shared.
- **Dependencies**: `zip` gained `aes-crypto` — exactly the three predicted new
  crates (`pbkdf2`, `sha1`, `constant_time_eq`, plus the `zeroize_derive` proc
  macro), all RustCrypto, still **no C dependency**. `cargo deny check` clean.
- **Tests**: the four-row read matrix; that an encrypted archive carries no
  readable data (asserted on the archive's **bytes** against a level-0 control —
  an API refusal would still pass if the content sat there in the clear); a
  round trip over both kinds of archive with the password held throughout (the
  requirement's own case); a bad password refused **without touching data or even
  writing a pre-restore copy**; the manifest readable without it; an empty
  password meaning plain; the pre-restore copy inheriting the password;
  corruption detected; the CLI parser (both spellings, on both commands, and the
  option before the positional); the help column that had to widen for
  `--password <PASSWORD>`; the settings field; the orchestrator persisting
  ciphertext and emitting the flag; and the precedence rule. **1702 unit tests
  green** (+12), 70 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check`/`cargo deny` clean.
- **Mutation-tested**: dropping the pre-flight password check, writing the
  pre-restore copy without the password, and making `write_zip` ignore the
  password each fail exactly the tests meant to catch them (1, 1 and 5
  respectively).
- **A live model run isn't required** (AGENTS.md §3) — no engine, memory, tool or
  provider path is touched. What stands in for it, as in the database-compaction
  work, is the **real CLI against a copy of the real data root** (311 chats,
  42 MB): a plain and an encrypted backup; **7-Zip reports `Method = AES-256
  Deflate` and `Encrypted = +`** on the data entries and *nothing* on the
  manifest, extracts `settings.json` with the password and refuses a wrong one —
  the interop claim behind F1, verified against a third-party tool rather than
  our own reader; restore refused (exit 1, data intact at 311 chats) with no
  password and with a wrong one; restore succeeded with the right one, and the
  **pre-restore copy came out encrypted**; the plain archive restored while a
  password was held; and with a password seeded into `settings.json` through the
  app's own encryption, `backup` and `restore` used it with **no argument at
  all** (314 encrypted entries).
- **A trap worth re-recording** (the journal already has it, and it bit again):
  `./mindfork-rs restore … | tail` reports **`tail`'s** exit code, so a refusal
  looked like `exit=0` until it was re-run without the pipe. Also, one probe of
  mine was a bad instrument rather than a finding: a stray file at the data root
  survives a restore **by design** (the cleanup is an allowlist), so proving a
  real replacement needs a stray file inside `chats/`.
- **Groundwork**: encrypting the archive's file names would need the outer
  container from F1(b); a stronger KDF is impossible without leaving the zip
  format; `MINDFORK_BACKUP_PASSWORD` (F7) was deliberately not added — trivial
  later, and a third source now would widen "where did this password come from"
  for no current need.

### Post-M9: the assistant can watch a YouTube video (`youtube_watch`) (done)

- **Asked for as "research the possibility of integrating with YouTube, so the
  assistant can get a description of what is talked about **or shown** in a
  video"** — and that "or shown" turned out to be the whole story. Research with
  forks R1–R9 — [docs/research/youtube-integration.md](docs/research/youtube-integration.md)
  (**user's decision 2026-08-01, all as recommended**); behaviour — spec §9.9.
  Branch `docs/youtube-research` (research + stage 1).
- **Everything load-bearing was measured live from this machine before any
  design**, and two results decided the shape:
  - **Every free caption path is closed.** The watch page still hands out a fully
    **signed** `timedtext` URL, and that URL returns **HTTP 200 with a zero-byte
    body** — tried bare, `&c=WEB`, `&fmt=srv3`, `&fmt=vtt`, `&fmt=json3`, with
    browser `User-Agent` + `Referer`. That is YouTube's PoToken gate, and it is
    **not** a datacenter-IP problem: this was a residential IP, the sympathetic
    case. InnerTube `player` gives `UNPLAYABLE` on WEB/MWEB and `400
    FAILED_PRECONDITION` on ANDROID/IOS; `get_transcript` 400s;
    `captions.download` requires the **video owner's** OAuth by design. The Rust
    crates on that surface (`yt-transcript-rs`, `ytranscript`, `ytt`) inherit it —
    a dependency moves where the failure prints, not whether it happens.
  - **Gemini ingests a YouTube URL directly**, through the same
    `v1beta:generateContent` the project already speaks: a 20 s clip = **2098
    prompt tokens in 3.4 s**; the full 213 s video = **22 050 in 8.0 s**; ~103
    tok/s at low detail (audio 32 exactly, video ~71). Verified across
    `2.5-flash`, `3.1-flash-lite`, `3.6-flash`; `watch?v=`/`youtu.be`/`shorts`
    accepted verbatim. OpenAI's Responses API and Anthropic take **no** video —
    checked against primary sources, after a blog claiming a "Claude video API"
    turned up in search and proved to be invented.
- **So the capability is Gemini-only, which is exactly why it must not go through
  the chat engine.** Putting a media part into the provider-agnostic
  `ChatRequest` (ADR 0004's boundary) would hand video only to users whose *chat*
  engine happens to be Gemini — i.e. deny it to the local-model users most likely
  to want it. Instead: a tool with its own client under `shared/video/`, called
  out of band, returning text into the conversation — the shape **TTS already
  uses** (ADR 0009). The key needs no plumbing at all: `stored_key` is
  provider-centric (ADR 0008), so the Gemini key entered for chat or embeddings
  is the same one.
- **Returns the answer, not the material** (the `fetch_url` shape): a 10-minute
  video is ~62k tokens *at Google* and a few hundred *in the conversation*, which
  is what makes it usable from a local 8k-context model. A raw transcript is
  deliberately out of scope — its honest home is a chat attachment (§9.7), which
  is already paged and searchable.
- **Cost is bounded before it is spent.** The tool refuses past
  `config.video.max_minutes` (default 30 ≈ 186k tokens) and names `start`/`end`
  in the refusal, so the model can retry with a segment instead of giving up. The
  gate measures the **segment**, not the video — with bounds given only that span
  is charged, so gating on full duration would refuse requests that cost little.
  When the length cannot be read at all, the request is clipped to the ceiling
  **and the answer says so**: silently describing only the beginning is the
  failure mode fork R4 rejected.
- **Degradation is the contract, not an afterthought.** Title/channel/length/the
  author's description come free from the watch page (oEmbed as fallback) and
  still work with no key; with no provider the tool does **not** disappear — it
  returns that metadata plus a plain statement of what is missing, so the model
  can explain itself to the user. A provider failure or timeout degrades the same
  way with the reason included.
- **Four things the design sketch got wrong, found while building:**
  - `is_youtube_url` is **not** `video_id().is_some()`. A bare 11-character id is
    accepted as input (models pass one as often as a URL), but must not let
    `fetch_url` route arbitrary 11-character text into the YouTube branch.
  - The watch page is parsed with a **string-aware balanced-brace scanner**, not
    the probe's regex: the blob contains `}` inside strings, and the non-greedy
    match survived only by luck.
  - **Thinking has to be muted explicitly.** `maxOutputTokens` *includes* thought
    tokens, so on a 3.x model the budget can go entirely to thinking and the
    answer comes back blank (observed on `gemini-3.6-flash`). The mute is per
    generation, so rather than write that heuristic a second time,
    `is_gemini_3`/`is_gemini_3_pro` in the engine wire became `pub(crate)`.
  - `VideoConfig`'s `Debug` is **hand-written to redact the key** — the type is
    reachable from `ToolConfig`, which derives `Debug`, so a plaintext key must
    not be one stray `{:?}` from a log file. Pinned by a test.
- **`fetch_url` stopped dead-ending on YouTube links** (fork R6): measured, the
  watch page has **zero** paragraphs and **zero** list items, so readability
  extracted nothing and the answer was "failed to extract readable text" — a dead
  end the model cannot reason its way out of. It now returns the same metadata
  block and points at `youtube_watch`.
- **Registry wiring**: `config.video` feeds the **tool registry** (the client is
  built there), so it rebuilds on a `config.video` edit *and* on a **Gemini key
  change** — otherwise the tool would keep reporting itself unconfigured until
  some unrelated settings edit happened to rebuild it.
- **Tests**: URL forms (every shape plus a bare id, and the links that must
  *not* parse); the brace scanner against `}` inside a string; watch-page and
  oEmbed parsing; duration formatting; the wire body (file part, bounds only when
  set, per-generation thinking mute, model-prefix normalization); a blank answer
  naming its `finishReason` and a filter block reported rather than silently
  empty; the tool's degraded paths (no provider, non-YouTube URL, reversed range,
  over-ceiling segment, provider failure) each asserting the provider was **not**
  called where it must not be; key redaction; localization. Settings: four rows
  in a "Video" group, going through the `field_spec` access table (the settings
  plumbing was delegated to a subagent with that constraint spelled out; it
  followed the precedents and reused two existing label keys rather than adding
  near-duplicates). **1732 unit tests green** (+30), **72 `#[ignore]`** (+2),
  clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **Live run — GO** (real Gemini `gemini-2.5-flash`, 2026-08-01):
  `watches_a_real_video_live` clips a video to its first 20 s — where a
  transcript would give almost nothing, the first sung line starting at ~0:18 —
  and got back a timestamped account of clothing, hair, three distinct settings
  and the cuts between them, under the live title and duration read from the
  watch page (7.2 s end to end). That is the assertion that matters: it is the
  half a transcript could not have produced.
  `live_youtube_link_returns_metadata_not_a_dead_end` confirms the `fetch_url`
  half against the real page (1.3 s).
- **Groundwork** (roadmap): `transcript: true` landing a long transcript as a
  chat attachment (R3c); cross-chat caching of an expensive watch (R8b) — within
  one chat a follow-up is already free; a transcript path needing no cloud key at
  all (R7) — the one story stage 1 does not serve; and default-model rot
  (`gemini-2.5-flash-lite` already 404s for new users).

### Post-M9: YouTube stage 2 — the words, as a chat attachment (done)

- **Stage 2 of the YouTube track**, fork **R3(c)**: plan with forks F1–F5 —
  [docs/history/youtube-transcript.md](docs/history/youtube-transcript.md)
  (**user's decision, 2026-08-01**, all as recommended); behaviour — spec §9.9.
  `youtube_watch(transcript: true)` brings back the spoken words as well as the
  description, and a large transcript lands as a **chat attachment** (spec §9.7)
  rather than as a tool result — attachments already have a budget, page-by-page
  reading and semantic search, while a tool result goes into the context whole
  and would destroy an 8k local model. Branch `feat/youtube-transcript`.
- **It is not a cheap path, and the parameter says so.** A transcript is the
  *same request* with a different prompt — the video is ingested either way, and
  on the 3.x models audio is not even billed apart from video (research §3.3a).
  So it is **one** provider call for both halves, split on a marker, and the
  `transcript` description states the cost; without that the model would reach
  for it by default. What genuinely changes is the **output** side, which is
  where truncation stops being cosmetic (below).
- **The stage's real work was the contract, and reading the code decided it.**
  Tools do not mutate `Chat`: they return `ChatEffect`, which had exactly two
  scalar variants, and the **orchestrator** applies effects in `handle_done` —
  i.e. when the whole **turn** is over. Meanwhile `ToolContext.attachments` is a
  snapshot taken at the turn's start. So the naive implementation has a hole with
  a name: the tool says "attached as *X*, 14 pages", the model does exactly what
  it was told — `attachment_read("X", 1)` — and gets "no such attachment". That
  is the **third instance of one defect class** here, both earlier ones found
  live: the by-reference attachment block that described the situation without
  saying what was possible, and stage 1's own unconfigured path (eight tool calls
  rediscovering measured dead ends). Both were closed by making the message close
  the door; here the door has to be genuinely open, because the tool can deliver.
- **Fix (fork F1a)**: a generic `ChatEffect::AddAttachment(Box<Attachment>)`
  carrying the **already-built** attachment, mirrored by the loop into its own
  turn snapshot at the end of the round (`generation::sync_attachments`, applying
  the same dedupe-by-source rule the orchestrator will) and persisted by the
  orchestrator through `insert_attachment` — the path `/file attach` takes, so
  the index, the feed note and the status chip all follow. The loop still never
  touches `Chat`; the snapshot is its own. Carrying the built object means what
  the model was told about and what gets stored are the same one, `id` included —
  that `id` is the key the background index is written under. Two facts fell out
  of the same reading: a **cancelled turn still delivers its effects**, so a
  transcript already paid for is not lost on `Esc`; and the attach path was
  already reusable, so stage 2 joined it instead of building a second one.
- **The threshold is an existing setting (F2), and it has an invariant.**
  `config.attachments.max_file_tokens` decides result-vs-attachment: below it the
  model reads the words at once with no second call, and attaching would put the
  same text in the pinned block *and* in the history. Above it — an attachment,
  which for **any** value of the setting is **by reference by construction**
  (inline requires `est <= max_file_tokens`, exactly the other side of the
  threshold). So this path never adds an inline attachment, never competes for
  the chat's inline budget, and is indexed for `attachment_search` with no
  special case (F3). The source key `youtube:<id>#transcript@<segment>` makes
  re-transcribing the same span replace it while two different spans coexist.
- **Truncation had to become part of the contract (F5).**
  `VideoUnderstanding::describe` returned `String`, and a `MAX_TOKENS` finish
  with partial text was returned silently. For a description that is cosmetic;
  for a transcript it is a correctness bug, because a complete-looking prefix
  would let the model believe it had read the video to the end — losing the very
  guarantee `attachment_read`'s page walk exists to give. It now returns
  `VideoAnswer { text, truncated }`, and the ceiling is reported in **both**
  places: the result (what the model reads now) and the file (what it reads
  later, when being a prefix is otherwise invisible).
- **The live run measured something the plan had flagged as unmeasured, and it
  came out "no".** The prompt asks for timestamps counted from the start of the
  video; asked for a 0:40–1:20 clip, Gemini numbered the transcript **from
  zero** — while the attachment header says they are absolute, so the file was
  making a false claim to anyone reading it weeks later (seek to 0:03 for a line
  that is really at 0:43). Fixed with arithmetic we control rather than an
  instruction we hope is followed. And the **second** live run justified the
  design of that fix: the same model, same request, emitted **absolute**
  timestamps on its own — a blind shift would have pushed them outside the
  segment entirely. So whether to shift is decided by the first timestamp, and
  the ambiguity is bounded by the offset itself (documented at the function).
- **Tests**: the marker split and its no-guessing fallback; the F2 threshold in
  both directions plus the by-reference invariant; the source key separating
  segments; truncation reported in result and file; the timestamp correction in
  both directions; and — the stage's real risk — **the loop letting the next
  round read what the previous one attached**, end to end through the real
  agentic loop with a fake attaching tool (round 1 attaches, round 2 reads it
  back), plus the persistence half through `handle_done`. **1749 unit tests
  green** (+14), **73 `#[ignore]`** (+1), clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **Mutation-tested**: dropping the
  mirroring, the persistence, or the truncation flag each fails its own test and
  nothing else.
- **Live run — GO** (real Gemini, `gemini-3.5-flash`): a 0:40–1:20 clip,
  `transcript: true` — the transcript came back as an attachment carrying the
  sung lines with timestamps inside the requested span (5.9 s), and the tool
  result named the attachment, its page count and the two tools that reach it.
  The assertion is deliberately the mirror of stage 1's, which asserts on
  something only *visible* on screen: this one asserts on something only *said*,
  so both halves of the question the track started from are covered live. Stage
  1's smokes (`watches_a_real_video_live`, the `fetch_url` one) re-run green.
- **Groundwork unchanged** (roadmap): cross-chat caching of an expensive watch
  (R8b), a transcript path needing no cloud key (R7), default-model rot. New:
  `fetch_url`/`python_exec` gaining `attach: true` — the effect makes it cheap,
  which is exactly why it should be a deliberate decision rather than a side
  effect of this stage.

### Post-M9: `youtube_watch` — a degraded answer has to close the door (done)

- **Found by the user in a live run**, the first real one after the merge: an
  `openai` chat (gpt-5.6) with **no Gemini key stored**, so `youtube_watch` took
  its unconfigured path and returned the metadata plus "video understanding is
  not configured". The model then spent **eight tool calls** rediscovering the
  dead ends this project had already measured — three `web_search`es (including
  one for "<video id> transcript"), scraping `captionTracks` off the watch page
  and hitting the signed `timedtext` URL (200, **0 bytes**, exactly as §3.1 of
  the research says), `pip install youtube-transcript-api` (**no pip** in the
  sandbox), looking for `yt-dlp`/`ffmpeg` (**absent**) — until the user cancelled
  the turn. Branch `fix/youtube-dead-end-wording`.
- **This is our defect, not the model's.** The stage-1 message said *what* was
  missing and how the **user** could fix it, but never said the video's content
  is unreachable by any other route — so an agentic model quite reasonably read
  it as a local limitation and went looking. **The same defect class**, and the
  same fix, as the by-reference attachment block: there too the entry described
  the situation without stating what was and wasn't possible, and there too the
  model improvised (`fs_list`, `fs_read`, four `web_search`es) and ended on an
  impossible suggestion.
- **Fix — the degraded answers now name the routes that don't work**
  (`not_configured`, `failed`, `timeout`, both bundles): captions come back empty
  without a token, neither `fetch_url` nor `python_exec` gets around that (no
  `yt-dlp`, no `ffmpeg`, no `pip` in the sandbox), and no transcript is in web
  search — followed by what to do instead (answer from what is there, say plainly
  that the video was not seen, and tell the user how to enable watching). The
  **tool description** says the same up front, so the choice is informed before
  the call rather than after it. `failed` additionally allows **one** retry, since
  a provider error can be transient — but not by another route.
- **The regression test asserts a property, not prose**: every degraded message,
  in every built-in locale, must name `python_exec` and `fetch_url` — tool ids,
  which are stable identifiers rather than wording. **Mutation-tested in both
  directions**: weakening the clause fails it with the whole message in the
  failure output. Nothing else changed — no code path, no config, no schema.
- **Worth recording separately**: the trace is also a clean field confirmation of
  §3.1 of the research, produced by a different agent on a different day from a
  different starting point. The `timedtext` fetch returned **200 with an empty
  body**, `pip`/`yt-dlp`/`ffmpeg` were absent, and web search had no transcript —
  every measured dead end, re-measured live.
- **Follow-up from the same live run: a Gemini key had nowhere to be
  entered.** The user's setup was chat on OpenAI with local embeddings — and the
  "Model" section offers a key row only for a slot whose **mode is that cloud**,
  so there was no field for a Gemini key anywhere in the UI, while `youtube_watch`
  needs one whatever the chat engine is. That is the gap that put the tool on its
  unconfigured path in the first place. Fixed by giving the "Video" group its own
  stored-key row (`FieldId::VideoApiKey`) — the same machine-bound storage as every
  other key (ADR 0008), addressing `CloudProvider::Gemini` **unconditionally**
  rather than deriving the provider from a mode, since this slot has no mode. All
  the behaviour came free from `is_secret_field` + `api_key_field_provider`: masked
  empty editor, commit as `SetApiKey`, `Del` deletes, and the key never reaches the
  screen's config. Tests pin the part that could regress silently — the row targets
  Gemini **with no engine set to Gemini** — plus the status/`Del` pair.
  **1735 unit tests green** (+2).
- **Default model moved to `gemini-3.5-flash`** (user's request after a working
  live run: 2.5 is old and will not stay around). Verified before switching, and
  the verification changed two documented figures. On the **same 20 s clip with an
  otherwise identical request**, `mediaResolution` turns out to be a **no-op on the
  3.x flash models** — 1822 prompt tokens at `LOW` *and* at `MEDIUM`, checked on
  3.1-flash-lite, 3.5-flash and 3.6-flash — while on 2.5 it behaves as documented
  (2062 → 5902). And 3.x reports **no `AUDIO` modality** at all, which looked like
  losing half the feature; it is a *reporting* difference, confirmed
  **behaviourally** rather than assumed: asked to quote the words in a segment,
  3.5 and 3.6 returned the sung lines with correct timestamps just as 2.5 did.
  Audio is folded into the video bucket. Net effect: the new default is **~12%
  cheaper** (91 vs 103 tok/s at low detail), so the 30-minute ceiling is ~164k
  tokens rather than ~186k. The resolution setting is **kept** — it is real on
  2.5-class models and the API documents it generally — but its hint now says
  where it does nothing instead of promising a 3× saving. Figures corrected in
  spec §9.9, install.md, both settings hints and the `MediaResolution` doc
  comment; the research doc gained §3.3a with the comparison table. **Live smokes
  re-run against the new default — GO.** A trap worth recording: the settings
  hints are stored as **arrays of strings** (the bundle allows either), so a
  single-line regex edit mangles them — caught by validating the JSON before
  writing, which is why the files were never damaged.
- **1735 unit tests green** (+3), 72 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run needed** — the change is the
  text of three localized strings and a tool description; the engine, memory and
  tool paths are untouched, and the behaviour it fixes is the model's reading of
  that text, which no automated smoke can assert. Whether the wording actually
  stops the flailing is the user's next live run to judge.

### Post-M9: spellcheck skips URLs and email addresses (done)

- **Asked for directly**: the input box's spellcheck shouldn't touch URLs — and,
  once that landed, email addresses too. It was underlining a pasted link word by
  word — `github`, `vshylov`, `mindfork`, `blob` — i.e. the loudest noise lands
  on exactly the text a user *pastes* rather than types, and none of it is
  correctable. Branch `fix/spellcheck-skip-urls` (a simple task by AGENTS.md §1:
  one module, no cross-layer contract, no new dependency — no design doc).
- **The fix goes in segmentation, and that's the whole reason it is small.**
  `segment::words` has exactly two callers, both in `SpellChecker`
  (`misspellings` for the underlines, `misspelled_word_at` for the `Ctrl+G`
  popup), so skipping a link there covers both with no second rule to keep in
  sync — and the `chat_list` rename field, which shares the checker, comes
  along for free. `words` skips a link span whole rather than filtering
  afterwards, so no word can even start inside one; spans are
  whitespace-delimited and words never contain whitespace, so a word is always
  entirely inside or entirely outside one, and the character offsets after a
  link still address the original line (pinned by a test — those offsets are
  what draws the underline).
- **Four shapes, deliberately a heuristic and not a parser**: an explicit
  scheme (matched *anywhere* in the token, so `(https://x)` counts without
  trimming games), a `www.` prefix, a bare domain — the last one because
  `github.com/foo` is what people actually paste — and an email address.
  Surrounding punctuation is trimmed, so a trailing sentence period, `«…»` or
  `<user@example.com>` doesn't hide the link.
- **The load-bearing detail is the restriction on bare domains: lowercase
  ASCII.** Without it, a missing space after a period reads as a domain —
  `end.Next`, and its far more common Cyrillic equivalent — and the check would
  switch itself off for a genuine typo, silently. Requiring lowercase ASCII
  labels rules out both (the Cyrillic case by script, the English one by the
  capital that follows a period), while `example.com` and `sub.example.co.uk`
  still match. A digits-only last label (`3.14`) is not a TLD, so version
  numbers are unaffected. Filenames like `main.rs` do read as domains — that
  is a bonus rather than a cost: a filename isn't prose either.
- **The `@` inverts that restriction, which is why emails got their own
  branch rather than being folded into the domain rule.** An address is
  unambiguous by shape — there is no run-on sentence containing an `@` — so the
  host may be written in any case (`Vladimir.Shylov@Outlook.COM`), where a bare
  domain may not; the two cases are carried by a `DomainCase` parameter rather
  than by two copies of the label check. The local part keeps its own case and
  allows the characters people actually use (`._%+-'`), an optional `mailto:` is
  stripped, and the split is at the **last** `@`. Conversely, a bare `@` is not
  enough: `@username` has no domain, so a mention stays checkable.
- **Scope**: file paths are *not* covered — widening further would be a
  different feature. Neither are internationalized (non-ASCII) domains; the
  lowercase-ASCII rule is what buys the run-on-sentence guard.
- **Tests**: the four shapes skipped and the prose around them still checked;
  punctuation around a link; the run-on-sentence guard in both scripts plus a
  dotted abbreviation and `3.14`; a mention and a domain-less `a@b` staying
  checkable; offsets surviving a skipped link; and, in `check.rs`, that a URL is
  neither underlined nor offered suggestions. **Mutation-tested**: forcing
  `is_link_token` to `false` fails exactly the four URL tests, and forcing
  `is_email` to `false` fails exactly the email one — while the two negative
  guards stay green in both runs, as scope pins should. **1756 unit tests
  green** (+7), 73 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check` clean.
- **A live run isn't required** (AGENTS.md §3): pure text segmentation inside
  one feature module — no engine, memory, tool or provider path is touched.
- Docs: spec §11.5 (the rule and its boundary; the same bullet also corrected a
  stale claim that segmentation uses `unicode-segmentation` — it has been our
  own scanner since M3).

### Post-M9: MCP servers in the settings window (done)

- **Closes the ADR 0007 groundwork item "server editor UI"** — until now
  `config.mcp.servers` was hand-edited in `settings.json` (decision point R6,
  deliberate at the time: the host was new and a file was the smaller surface).
  Design plan with forks F1–F8 —
  [docs/history/mcp-server-editor.md](docs/history/mcp-server-editor.md)
  (**confirmed by the user 2026-08-02, all as recommended; scope — stage 1**).
  Behaviour — spec §9.6. Branches `docs/mcp-server-editor` → `feat/mcp-server-editor`.
- **Reading the code first shrank the task and reshaped it.** The settings screen
  already owns a working `AppConfig` and emits `SettingsIntent::SaveConfig` carrying
  the **whole** thing; `handle_update_config` diffs `config.mcp` → `mark_mcp()` → the
  1.2 s debounce → `apply_mcp_settings()`. So editing the inventory from the screen
  needed **no new `AppCommand`, no new `AppEvent` and no orchestrator change at all**
  for the edits themselves — the impersonation-persona shape
  (`config.impersonation_profiles`, spec §11.8), which is the precedent this follows
  throughout. `Ctrl+Z` undo came free for field edits *and* for create/delete (it
  restores a whole older config snapshot), and `is_current` already suppresses the
  no-op restart from an edit plus its undo.
- **The hole that reading turned up, and the one genuinely new action.** A server
  that exhausts its restart budget (3 crashes / 5 min) sits `Disconnected` "until
  manual intervention (editing settings recreates the slot)" — but since `is_current`
  landed, toggling a field off and on inside the debounce window yields an
  *identical* final config and therefore **no re-apply at all**. There was no
  reliable way to retry a dead server short of restarting the app. So `Enter` on a
  status row now does what that row needs: confirm a changed catalog when one is
  pending (today's meaning), otherwise **reconnect** — an action, not a config edit,
  so it gets an intent/command pair mirroring `ConfirmMcpCatalog`. The restart budget
  is cleared by it: an explicit reconnect *is* the manual intervention it waits for.
- **Reconnect forced the event epoch to become per slot** (`McpSlot.epoch`), replacing
  the comparison against the manager's global one. Bumping the global epoch to restart
  one server would drop **another** server's in-flight `Ready` and strand it on
  "connecting…" forever; keeping the epoch would let the cancelled task's late
  `Exited` restart the fresh one. Per-slot is strictly more precise than the global
  check it replaces (an event for a server that was removed, or whose slot has moved
  on, is dropped exactly as before) and is a ~10-line diff.
- **Where it lives (F1b): a dedicated "Plugins" section**, not a group in "Tools".
  The "Data" precedent — a section exists when nothing else is about that thing:
  "Tools" is a list of *gates for built-in tools* (one toggle each), while this is an
  *inventory of external programs* with per-server settings, statuses and a lifecycle.
  It also gives the remaining MCP groundwork (HTTP transport, resources/prompts, tool
  caps) somewhere to land. Layout: master switch → a `Choice` server selector plus the
  selected server's fields → the status rows that used to sit in "Tools" (kept as the
  at-a-glance list, so status is not duplicated).
- **The two non-scalar fields.** `args: Vec<String>` is edited as a **shell-quoted
  command line** (F3a) — how a person types one, and unlike a separator it is total:
  `join_args` quotes any argument containing whitespace or either quote character, so
  the round trip holds for *any* value (pinned by a test over paths with spaces,
  embedded quotes, apostrophes and an empty argument). `env` is `VARIABLE=SOURCE`
  pairs (F4a): flat parsing is unambiguous **because** the value is the name of a
  source environment variable rather than a secret (ADR 0007 R8) — a property worth
  stating, since stage 2 changing that would change the shape.
- **A new server starts disabled** (F5a) with a generated free id and no command:
  nothing is spawned while the command is half-typed, and flipping "Enabled" becomes
  the deliberate "start it" moment a per-field debounce cannot express.
- **Validation before the commit** (F8a) for the three rules the screen can check
  itself: slug shape, **uniqueness** among servers, and the batch-command ban. The
  status row remains the backstop for everything else, but an invalid **id** creates
  no slot at all — just a log warn — so without this a typo'd id would make the
  server *vanish* from the status list. The server-id rule moved from
  `orchestrator/mcp.rs` into `shared/mcp.rs` next to `forbidden_batch_command`: the
  screen may not import `app` (FSD), and one rule with two copies is how they drift.
  The validation is checked **before** taking the editor's `&mut` borrow — uniqueness
  needs the rest of the config, which a validator holding that borrow cannot see.
- **The TOFU pin is deliberately untouched by an edit**: changing `command`/`args`
  keeps it, so pointing a server at a different program trips the catalog check —
  precisely when it should fire. A **rename** drops the pin (it is inherited by id in
  `handle_update_config`) and the server re-pins silently, which is acceptable since a
  rename changes every tool name anyway; documented rather than special-cased.
- **Not in this stage** (F4b/F6, a separate decision): machine-bound secret **values**
  for the `env` map and import of the ecosystem's `mcpServers` JSON. Storage needs no
  change for the first — `secrets::put_key`/`store_secret` already take an arbitrary
  key name, as the backup password proved — but it amends R8, and the two are coupled
  (an imported `env` carries literal secrets), so they stay stage 2. The honest limit
  today: a hosted server still needs its token in an OS environment variable.
- **Tests**: create → edit → delete round trip (including that a new server is off and
  gets a free id); `args`/`env` round-tripping through the field, plus hand-typed
  forms; the id refused for shape, emptiness and collision while its *own* id is not a
  collision; the batch ban; the selector cycling and its popup; `Del`/the `•` marker
  skipping user data; undo restoring a deleted server; and at the manager level,
  reconnect respawning one server **without stranding another's in-flight event** and
  clearing an exhausted budget. **All seven load-bearing behaviours were
  mutation-tested** — each mutation (a server created enabled, validation dropped,
  Enter doing nothing on a healthy row, the global epoch restored, the budget kept,
  the fields treated as config data) fails its own test and only that one. Plus the
  seam the whole editor rests on — a `config.mcp` edit reaching the host through the
  existing `UpdateConfig` → diff → debounce path, and *not* costing a restart when
  nothing effective changed — which was the one untested claim behind "the
  orchestrator needed no changes" (mutation-tested too). **1766 unit tests green**
  (+10), **74 `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check` clean.
- **Live run — GO** (`mcp_reconnect_live`, a real
  `npx @modelcontextprotocol/server-filesystem`): first bring-up **Ready, 14 tools**;
  reconnect → the tools leave with the connection, the server comes back **Ready with
  the same 14** in 6.1 s. That is the half unit tests cannot answer — they can prove
  the slot is reset and a task spawned with a fresh generation, but whether a real
  subprocess is actually torn down and a new one handshakes in its place is a property
  of the process handling. The smoke needs only `npx`, no model, so it runs without an
  engine. `mcp_filesystem_e2e_live` is green too (Gemma 4 31B q4_0 + bge-m3, external
  `llama-server`, `--jinja`): 14 tools in the catalog, the model called
  `mcp__fs__read_text_file` and used the result.
- **Regression — clean** (same stack): **26 of 27** orchestrator e2e live smokes green
  on the first pass (669 s) — memory/self-model/notes/graph/cross-organ links/RAG/
  attachments/control tools/i18n/MCP. The one failure was `followup_tool_e2e_live`,
  where the model simply answered with a bare one-line greeting and never
  called `send_followup_message`; it passed on re-run (`saw_continue=true`, a second bubble).
  A model-behaviour flake of the class the control-tools entry already records, not a
  regression: the diff touches **zero** files on the agentic-loop/control-tool path.
- **The first manual run in a terminal found two things** (plan §8), both fixed on
  the same branch.
  - **A platform-dependent config.** A spike measured *why* `cmd /c npx …` is
    needed on Windows rather than assuming: `cmd.exe` completes a bare name from
    `PATHEXT` and **Rust does not** (`Command::new("npx")` → `NotFound`,
    `Command::new("npx.cmd")` → spawns). `shared::mcp::resolve_command` now does
    that completion, so one config works on every platform. The same spike
    **overturned the premise of the `.bat`/`.cmd` ban** (ADR 0007 §2):
    CVE-2024-24576 is fixed in `std` as of Rust 1.77.2 — measured, `a"b`, `%CD%`
    and `a&whoami` are escaped and an embedded newline is *refused* with
    `InvalidInput` — so the ban added nothing over `std` while pushing users onto
    `cmd /c`, where arguments are re-parsed by `cmd.exe` **outside** that
    escaping. Removed, with the reasoning recorded in the ADR; the live smokes now
    use a bare `npx` on every platform, which makes them the demonstration.
  - **The live run then caught a bug in the new resolver**, which is what a live
    run is for: npm ships an extensionless `npx` (a Unix script) *next to*
    `npx.cmd`, and preferring the exact name spawned the script — `os error 193:
    not a valid Win32 application`. `cmd.exe` only ever completes a **bare** name;
    a name that already carries an extension is tried as written and then still
    completed (so `my.tool` reaches `my.tool.exe`). The rule's core became a pure
    function over `PATH`/`PATHEXT`, so a test builds the npm layout in a temp
    directory instead of mutating the process environment. The *wiring* (that
    `spawn` calls the resolver) is covered by the **live smoke only** — a unit test
    would need a global `PATH` mutation (racy under a parallel suite) or would sit
    through the 30 s handshake timeout; recorded rather than faked.
  - **"Ready" could mean invisible.** The reported symptom was a server showing
    `ready · tools: 1` while the assistant said it had no such tool — the double
    opt-in (R7) working as designed, but the row never said so. Same defect class
    as the by-reference attachment block and the `youtube_watch` unconfigured path:
    the UI states a situation without saying what is possible next. The row now
    reads `ready · tools: N · in profile: K` and points at "Profiles" when `K` is
    zero; the double opt-in itself is unchanged.
  - **1769 unit tests green** (+3), 74 `#[ignore]`; the resolver's bare-name rule,
    its extension handling and both halves of the status row were mutation-tested.
    **Live — GO** with a bare `npx` on Windows: `mcp_filesystem_e2e_live` (14
    tools, the model read the file) and `mcp_reconnect_live` (Ready → reconnect →
    Ready, same 14).
- **Acceptance criterion — met** (manual run by the user, 2026-08-02): a real
  server configured **entirely from the settings window** — `mcp-echo-server` with
  the platform-independent `npx` + `-y mcp-echo-server` (no `cmd /c`), the status
  row reading `ready · tools: 1 · in profile: 1`, and the model calling
  `mcp__mcp-echo-server__echo` and getting its result back. Every part of the track
  that only a human at a terminal could exercise is confirmed: authoring, the
  resolver, the profile-count row, and the tool reaching the model.
- **A process trap, hit for the second time in this repo** (the journal already
  records it from the vendored-syntaxes work): `git checkout -- <file>` used to revert
  a scripted mutation reverts **all** uncommitted work in that file. It cost the whole
  of `apply.rs`, restored by re-running the patch script. The fix is procedural —
  commit before mutating, which is what the second run did.

### Post-M9: MCP servers — secrets for the `env` map and JSON import (stage 2, done)

- **Completes the "MCP servers in the settings window" track** (plan
  [mcp-server-editor.md](docs/history/mcp-server-editor.md) §9, forks **S1–S8
  confirmed by the user 2026-08-02, all as recommended**). Stage 1 made a server
  *authorable* in the window; it still could not be *given a token* — a hosted
  server (GitHub, Slack) needed an OS environment variable and an app restart,
  which is the opposite of the track's goal. Behaviour — spec §9.6; **ADR 0007
  R8 amended** (as stage 1 amended R6) and **ADR 0008** gains its third reuse.
- **Reading the code first settled four things, and one of them was the whole
  risk of the stage.** The storage needed no change (`put_key`/`store_secret`
  take an arbitrary key name — proved by the backup password); the resolution
  pattern already existed and was exactly right (`resolve_api_key(stored,
  api_key_env)` — stored wins, env is the fallback); decryption belongs in
  `McpManager`, mirroring `EngineManager`, which itself calls `stored_key` and
  hands the spawn layer plaintext. And the trap: **`McpManager::is_current`
  compares `McpSettings` only**, so changing a secret leaves the config identical,
  the debounce skips the re-apply, and the server keeps running with the old value
  with nothing to notice it. The engines had already solved this
  (`chat_is_current(&settings, keys)`); `is_current` now takes the key blob too.
  Recorded in the plan §9.1 **before** implementation — it would otherwise have
  shipped silently.
- **The comparison is against the *resolved* environment, not the key blob** (a
  deliberate departure from the engines): killing and respawning `npx`→node is the
  most expensive re-apply on the screen, so changing an unrelated OpenAI key must
  not touch it. `applied: Option<(McpSettings, ResolvedEnvs)>`; the slot carries
  its resolved env because a respawn after a crash or a `reconnect` builds the task
  from the slot.
- **S1 — the `env` row keeps its meaning and becomes the declaration.** An empty
  source (`TOKEN=`) is legal and already parsed; below the row sits **one secret
  row per declared variable** (status + empty masked editor + `Del` deletes) —
  the "API key" row's exact behaviour, reused wholesale (`secret_row`,
  `is_secret_field`, the masked editor). **No new config field and no schema
  change**: the presence of a secret is a fact about `config.api_keys`, which
  `handle_update_config` already restores on the way back, so secrets survive
  every settings edit with no new code.
- **S2+S3 — one typed `SecretKey`, not a third variant.** `SecretKey { Provider |
  BackupPassword | McpEnv{server,var} }` + `storage_name()` replaced
  `SetApiKey`/`SetBackupPassword` with one `SetSecret`, and
  `api_keys_present`+`backup_password_present` with one `secrets_present`. Typed
  rather than a raw name because the side effects after storing differ per kind
  (a provider key re-raises the slots that use it and rebuilds the registry for
  Gemini; an MCP one marks `mcp`; a backup password needs nothing) — dispatching
  those by parsing a string is how they drift.
- **S4 — orphaned secrets are never collected automatically.** A rename or delete
  orphans them, but the `env` row commits on `Enter`, so a half-typed edit would
  destroy a value the user then has to re-enter — and `Ctrl+Z` restores the
  *config* while secrets are deliberately absent from that snapshot, making a
  garbage collector's mistakes unrecoverable. An orphan is inert ciphertext;
  cleanup belongs with ADR 0008's "forget this computer" groundwork.
- **S5–S7 — the import, and why the orchestrator parses it.** A `mcpServers` file
  carries **literal** secrets, so the row takes a **file path** rather than pasted
  JSON (a pasted blob would leave live tokens on screen and in the editor's undo
  buffer) and `features/mcp_import.rs` only *plans* — the orchestrator reads,
  stores each literal value as a machine-bound secret, and only then extends the
  config. Imported servers arrive **disabled**; ids are sanitized to our slug with
  a numeric suffix for collisions *within* the file; an id that **already exists is
  skipped** (a re-import is a no-op, never an overwrite — a bug caught by its own
  test, since the first version uniquified before checking and quietly imported a
  duplicate); non-stdio entries are reported rather than dropped.
- **Along the way**: `valid_env_name` (POSIX-shaped `[A-Za-z0-9_]`) moved next to
  `valid_server_id` in `shared/mcp.rs` — it is what keeps the flat `VAR=SOURCE`
  row parseable *and* `mcp-<server>-<VAR>` unambiguous (only the server id may
  contain `-`); and the "Command" field's hint stopped claiming `.bat`/`.cmd` are
  banned — stage 1 removed that ban and the hint still told users to write
  `cmd /c npx …`.
- **A design conflict found by a failing test**: `McpEnvSecret` belongs in
  `is_profile_field` (user data — no `•` "modified" marker, nothing to reset to),
  but that predicate is also the first gate in `reset_field`, which would have made
  `Del` a no-op on a secret row. The secret branch now runs **before** it: the two
  meanings of "has no default" and "Del has a real meaning" are separate.
- **Tests**: the storage name; a stored secret winning over the source and the
  source still serving as fallback; `is_current` false when **only** a secret
  changed; an unrelated provider key leaving the servers current; per-variable rows
  following the declaration, showing status, never putting the secret into the
  screen's config; `Del` deleting; the row dropping unrepresentable names; the
  import row committing a path; and end-to-end through the orchestrator — secrets
  stored as ciphertext with **no plaintext in `settings.json`**, servers disabled,
  ids sanitized, a re-import a no-op. **1787 unit tests green** (+13), **75
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **Mutation-tested, and it earned its keep**: nine mutations, eight caught by
  their own tests — the ninth **survived**, exposing that nothing asserted a stored
  MCP secret schedules the re-apply, without which the `is_current` fix above is
  never even consulted. `storing_an_env_secret_schedules_the_reapply` closes it
  (and fails under that mutation).
- **Live run — GO** (Gemma 4 31B q4_0, external `llama-server`, `--jinja`; real
  `npx`): the new smoke `stored_env_secret_reaches_the_child_process_live` spawns a
  minimal Node MCP server that reports what it sees in **its own environment** —
  it answered `ZARYA-7719`, the value stored only as a machine-bound secret. That
  is the one thing unit tests cannot show: that decryption, the spawn env and the
  child's `process.env` really line up. Regression: `mcp_filesystem_e2e_live` (14
  tools, the model called `mcp__fs__read_text_file` and used the result) and
  `mcp_reconnect_live` (Ready → reconnect → Ready, same 14) both green.
- **Follow-up from the manual acceptance run** (the user, 2026-08-02): the import
  worked — a real Cursor config imported cleanly — but the variable configuration
  "looks strange and inconvenient: why type `variable=value` pairs and then enter
  the value in a separate field as well". Fair, and the phrasing is the evidence:
  the value slot means the **name of a source variable**, and it *reads* as
  `variable=value`, so the row below asking for the value looks redundant.
  Checking `McpClient::spawn` settled how much of the mechanism was even
  load-bearing — it uses `Command::envs` with **no** `env_clear`, so the child
  already inherits the whole application environment, and `NAME=SOURCE` is only
  needed to take a value from a *differently named* variable. The row is now a
  **bare list of names** (`GITHUB_TOKEN, SLACK_TOKEN`), labelled "Variables";
  `NAME=SOURCE` is still parsed but no longer advertised. **No behaviour
  changed** — only what the field asks for. The claim the new hint makes is
  pinned live rather than asserted: the smoke now also sets a variable named
  **nowhere** in the server's config and the child reports it
  (`ZARYA-7719|INHERITED-4417`), so an `env_clear` appearing later would fail the
  test instead of quietly making the hint false.

- **A second follow-up from the same run**, and it was a defect rather than a
  preference: with a source named (`API_KEY=CLAUDE_API_KEY`) the screen still
  offered a value row, and S8's "stored wins" meant a secret could silently
  override the source the row named. Hiding the row alone would have made it
  worse — the override would remain, minus the only thing that could reveal it.
  The rule is now **one origin per variable, decided by the row**: a bare name
  takes the stored value (or is left to inheritance), `NAME=SOURCE` takes it from
  that OS variable and consults no stored value, and gets no value row. This
  narrows S8 instead of reversing it — the fallback existed for a shared config
  on a machine where the variable is set in the OS, and that case uses the
  variable's own name, which inheritance already covers. The row index stays the
  position in the whole map, so skipping a row never shifts `secret_field_key`'s
  addressing (pinned by the test). Two tests had their premise inverted and were
  rewritten rather than patched: `each_variable_has_exactly_one_origin` replaced
  `stored_secret_wins_over_the_env_source`.

- **A third follow-up, and the objection was about the logic rather than the
  screen** ("the fallback is good, but values are machine-bound, so you end up
  checking whether the variable exists *and* whether a key was entered on this
  particular machine"). Half of that had already gone with the change above —
  per variable the row decides which route applies, so the two are never both in
  play. What was left is an **asymmetry of visibility**: the stored route
  reported itself, a named source reported nothing, and a missing OS variable
  surfaced only as the server failing to work. A sourced variable now gets a
  **read-only status row** in place of the value field it deliberately lacks
  (`from CLAUDE_API_KEY: found / not found`, flagged when missing), and its hint
  states what was invisible before: the app sees the environment it was
  **started** with, so a variable set after launch needs a restart.
  Mutation-tested. Rejected: leaving it silent (the diagnosis stays indirect) and
  dropping the source form from the UI (it is the only way to rename a source).

- **The live check of that row found two defects, one of them older than this
  track.** Focusing a variable whose source is missing printed "the tool is
  enabled in the profile but disabled by a global switch — enable it in the
  «Tools» section", which is about neither that row nor that section. Root cause:
  `FieldRow.warn` meant **two** things — "colour this row" and "append the gate
  explanation" — so the sentence appeared wherever the flag was raised, including
  (since stage 1, unnoticed) on an MCP server whose catalog had changed. The
  explanation now travels in its own `warn_note`, set where the gate is actually
  detected; `warn` is back to meaning only "this row needs attention". And the
  text now **names the section that holds the switch**: the MCP master toggle
  moved to "Plugins" when the editor got its own section, so the hardcoded
  "Tools" had been wrong for it ever since — the user caught that too. Third,
  smaller: the new row's description showed a literal `{src}`, resolved with `t`
  where it needed `tf`. One test pins all three (mutation-tested).

- **Groundwork** (roadmap): the external proxy's key via the same mechanism; an
  explicit cleanup of orphaned MCP secrets, with "forget this computer"; HTTP
  transport, resources/prompts, `list_changed`, deferred schemas.

### Post-M9: `Home`/`End` as a ladder of stops (done)

- **Asked for directly**: `End` moved to the end of the row a wrapped line was
  broken into, and there was no way short of `Ctrl+End` — which leaves for the
  end of the *whole text* — to reach the end of the line itself; same for
  `Home`. Then, on the same branch, **`Home` was asked to stop at the first
  non-whitespace character first** ("smart home"). Branch
  `feat/input-home-end-toggle` (a simple task by AGENTS.md §1: one widget, no
  cross-layer contract, no new dependency — no design doc).
- **The steps are told apart by the cursor's position, not by counting
  presses.** `move_home`/`move_end` compute the ladder of stops and hand out
  **the next one after where the cursor already is** (the first stop if it is at
  none). Stateless, so nothing has to be reset by the many other things that
  move the cursor (typing, paste, undo, a mouse click, `activate_chat`) — a
  press counter would need clearing at every one of them, and forgetting one is
  a silent bug. It also does the useful thing when the cursor reached a stop
  **by typing** rather than by `Home`, which is the common case at the end of a
  line; that is the rule large editors use.
- **The `Home` ladder, and why the third stop is conditional**
  (`home_stops`): the row's first non-blank → the row's start → **the line's**
  first non-blank → the line's start, deduplicated by value. The line-level text
  stop is only included when it lies **before** the row's start, i.e. on the way
  left from a later row of a wrapped line: on the line's first row it is already
  the row's own text stop, and in the pathological case of indentation wider
  than the field it would make `Home` jump *forward*. With the guard every case
  came out sane, checked by probing the real stop lists rather than by reasoning
  alone — an unindented line collapses to the two steps it had before, an
  indented wrapped line gets three from its bottom row.
- **The other boundaries coincide where you would expect them to**, so no case
  needs a special branch: on a line's last visual row the row end *is* the line
  end, so `End`'s second step is simply a no-op there. `End` deliberately gets
  **no** trailing-whitespace stop (nobody indents the right margin), and
  `single_line` mode keeps `Home` at column 0 — a settings path that starts with
  an accidental space is easier to fix from there than from its text.
- **A whitespace-only row has nothing to skip to**, so `first_non_blank` yields
  the range's start and the stop collapses instead of throwing the cursor to the
  far end of the blanks.
- **The line-level steps stop at their own line** — they use
  `self.lines[self.row]`, never the visual-row table, so they cannot run past a
  real `\n` into a neighbouring line (pinned by its own test, since a wrapped
  line's row table spans the whole text).
- **A pre-existing invariant kept the change small**: `col_for_visual` already
  rolls back off a soft wrap (`is_soft`), so the first `End` never yields a
  position that renders at the start of the *next* row — which is what makes
  "already at a stop" a well-defined comparison.
- **Tests**: the full ladder in both directions (row → line → a further press
  staying put), the "stays on its own logical line" guard, the indented line
  (including `Home` pressed from *inside* the indentation), the indented wrapped
  line's three-step ladder, and the all-blank row; the existing
  `home_end_act_on_visual_row` is unchanged and still green — an unindented
  line's first press behaves exactly as before. **Mutation-tested**: dropping
  the row-text stop, the line-text stop, the blank-row fallback or either
  toggle fails exactly its own test(s) and nothing else. **1792 unit tests
  green** (+5), 75 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check` clean.
- **A live run isn't required** (AGENTS.md §3): cursor movement inside one
  widget — no engine, memory, tool or provider path is touched.

### Post-M9: `backup`/`restore` narrate their work, and give back the keyboard (done)

- **Reported from a real password-protected restore**: after typing the password
  and pressing `Enter` the program printed a newline and then **sat silent for
  ten seconds**, after which all three result lines appeared at once; and the
  `Enter`s pressed during that silence were replayed by `cmd` afterwards as
  three empty prompts. Branch `fix/restore-progress-and-typeahead` (a simple task
  by AGENTS.md §1: one feature module and the CLI, no cross-layer contract, no
  new dependency — no design doc).
- **Two defects with one symptom, and the second one made the first worse.**
  Every message was written *after* the work (`cli.restore.pre_saved`,
  `cli.restore.cleared`), so the whole restore ran mute; and a password prompt
  echoes nothing, so the one moment the user most needs a sign of life is the
  one where the program looked dead. Pressing `Enter` is then the natural thing
  to try — and those keystrokes sat in the console input buffer with nobody
  reading them, so on exit the shell inherited and replayed them.
- **Progress is a callback of already-localized lines**, the shape
  `sandbox_setup::setup` already uses: `features` has no TUI, so the CLI decides
  where the lines go (`println!`) and `data_migration` — which runs at startup,
  before the TUI — passes `|_| {}`. Each phase announces itself **before** it
  runs (checking → pre-restore copy → compacting → packing → clearing →
  unpacking → compacting), which is what makes it feedback rather than a log.
- **The entry loops count themselves out** (`EntryProgress`, `N of M`, at most
  once per 500 ms, counted from the loop's start so a small data root finishes
  in silence). Deliberately **not** a byte counter: the measured shape of a real
  root is ~300 small chat files plus one 26 MB `data.db`, so entry counting
  answers "is it moving?" for the bulk of the time, while an honest byte counter
  would have to reach inside the copy of a single file — recorded as groundwork
  rather than done.
- **The keystrokes are discarded on the way out**
  (`features/terminal_input.rs::discard_type_ahead`, the module renamed from
  `password_prompt.rs` — it is now terminal input for the CLI generally). Safe
  because there is no other consumer: the CLI has finished reading by then, and
  the keys were typed at us. Gated on stdin being a terminal, so a pipe or a CI
  job keeps its input; called on **both** commands and on every outcome
  (including a rollback), since the shell inherits the terminal either way.
- **One message survived the rewrite for a reason**: the path of the pre-restore
  copy is still printed at the end (`cli.restore.pre_saved`), because that is the
  path you undo a restore with and by then the progress lines have scrolled past.
  The phase label above it therefore names no path — it is announced before the
  copy exists. `cli.restore.cleared` became a progress line and its key was
  deleted (the i18n gate fails on a dead key).
- **Tests**: the phase order on a real restore (the contract is that each label
  precedes its step), a pre-restore copy **not** announced when the root is
  empty, the rollback announcing itself, `backup` announcing compaction before
  packing, and the ticker staying quiet under its interval / counting `N of M`
  with no unsubstituted placeholder in either language. **Mutation-tested**:
  dropping the clearing label, dropping the rollback label, or ignoring the
  throttle each fails exactly its own test. **1798 unit tests green** (+6), 75
  `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **No live model run is required** (AGENTS.md §3) — no engine, memory or tool
  path is touched. What stands in for it, as in the compaction and password work,
  is the **real CLI against a copy of the real data root** (321 chats, 26 MB
  database, 58 MB): `backup --password` narrated 331 entries and finished in
  15.7 s, `restore` narrated every phase of its 21.4 s and left the data intact
  (321 chats, the database restored), and a restore with no password still
  refuses before touching anything. The **type-ahead half cannot be tested from
  here** — it needs a real console, and `IsTerminal` is false in a piped
  harness — so it is left for the user's interactive check, the same boundary
  the MCP command resolver's wiring sits behind.

### Post-M9: page fidelity for `fetch_url` (+ two neighbouring traps) (done)

- **Specified by one real chat, not by a roadmap item.** The user pasted
  `https://docs.vlang.io/memory-management.html` and asked the assistant to read
  it and compare V's memory-management modes. One `fetch_url` call turned into 20
  messages — 3 fruitless `web_search`es and **6** `python_exec` rounds, including
  downloading the same 16 MB tarball of the V repository **twice**. Reading the
  transcript against the code found four causes, and they chain. Plan with forks
  F1–F2 — [docs/history/fetch-url-fidelity.md](docs/history/fetch-url-fidelity.md)
  (**user's decision 2026-08-03: all four, F1(a) `fetch_url` only, F2(b) attach**);
  behaviour — spec §9.3.1. Branch `feat/fetch-url-fidelity`.
- **P1, the root cause — extraction threw away the code.** `extract_readable`
  selects `p, li`, and the page was measured rather than assumed: **0 `<pre>`**
  (VitePress wraps code in a bare `<div class="language-v">`), section titles in
  `<h2>` — both dropped whole. The result the model received has visible holes
  ("…define a `free()` method on custom data types:" straight into the next
  paragraph). Documentation whose every "here is an example:" leads nowhere reads
  as **arriving damaged**, so the model went looking for the source elsewhere —
  and every later detour follows from that one inference.
- **`extract_rich` is `fetch_url`'s alone** (F1a): `web_search` budgets 1500
  characters per result page for **ranking**, where headings and code spend the
  budget without helping. Pinned by a test asserting the prose path is unchanged.
  Rich extraction keeps document order (one selector run), renders Markdown-ish
  (`## Heading`, fenced code with the language from a `language-*` class on the
  element or its inner `<code>`), preserves whitespace inside code (`collapse_ws`
  is right for prose and destroys code), exempts code and headings from
  `MIN_FRAGMENT_CHARS` (that floor exists to drop navigation chrome from
  snippets; a two-word heading is content), and **dedupes by ancestry** — a
  `div.language-*` wrapping a `<pre>` matches two selectors and must not emit its
  content twice.
- **P2 — the text was cut at 12 000 characters, silently, mid-word.** The
  recorded result is exactly 12 000 characters and ends on `…final out`. Nothing
  marked it partial and nothing could fetch the rest, so a long page was
  indistinguishable from a complete one. Now a page over the budget **is attached
  to the chat** (F2b): `ChatEffect::AddAttachment` already existed (built for
  `youtube_watch(transcript:)`), and an attachment is already paged
  (`attachment_read`) and searchable (`attachment_search`) — this is the
  mechanism's second consumer, not a new one.
- **The threshold is `attachments.max_file_tokens`, exactly as `youtube_watch`
  uses it**, and one consequence carries over for free: an attachment made this
  way is **always by reference**, because inline requires `est <=
  max_file_tokens` — the other side of the same comparison. The mode still goes
  through `decide_mode`, so it cannot drift from what `/file attach` decides.
  `summarize=true` still summarizes (from the head — the summarizer is a
  single-turn subagent) **and** attaches: a summary alone would repeat P2 in a
  politer form, since the model would have no way to reach what the summary
  skipped. A far higher hard ceiling stays (400 000 characters) and is now
  **announced** when reached, closing P2 at both ends.
- **P3 — `python_exec` gives a fresh sandbox per call and never said so.**
  `WasmerSandbox::run` creates a `JobDir` per run and drops it; only `/w` and
  `/sp` are mounted and the guest's `/tmp` dies with the process. The description
  said "no access to the machine's files" and nothing about state. Same defect
  class the journal already records twice (the by-reference attachment block,
  `youtube_watch`'s unconfigured path): the message describes a situation without
  saying what is possible next. Fixed in the description, with a test pinning
  that it names **`/tmp`** — the concrete path a model reaches for — in every
  built-in locale.
- **P4 — `web_search` said "no results" three times while it was blocked.**
  Measured live from this machine on the transcript's own queries: DDG lite 202 +
  `anomaly` (detected), Ecosia 403 (detected), **Mojeek HTTP 200 with
  "Verification required. Please complete the challenge"** — invisible to
  `is_throttled`, so `got_clean_page` was set and the honest "all providers are
  throttled" error was suppressed. The model was told the web knows nothing about
  V's memory management and, quite reasonably, went to `python_exec` +
  `requests`. The check (`is_challenge_page`) runs **only on a page that parsed to
  zero results**, so a genuine result set can never be mistaken for a challenge —
  strictly narrower than adding the markers to `is_throttled` itself.
- **Tests**: rich extraction against the transcript's real markup shape (heading,
  fenced code with language, whitespace preserved, nav dropped — plus the prose
  path asserted **unchanged**, which is what fork F1a means); no double emission
  of a wrapped block; language from an inner `<code>`; short code/headings
  surviving the floor; the attachment path (whole text, by reference, named by
  `<title>` with the URL as fallback, the result naming `attachment_read`/
  `attachment_search` and the page count); summarizing from the head while still
  attaching; the ceiling announced on both branches; the captcha classifier
  against the real interstitial text and against two results-page fixtures.
  **1811 unit tests green** (+13), **76 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check`/i18n gates clean.
  **Mutation-tested**: dropping ancestry dedup, neutering the challenge
  classifier, dropping the attachment effect, removing the `/tmp` sentence, and
  reverting `fetch_url` to prose extraction each fail exactly their own test.
  One gap recorded rather than faked — the provider loop's three-line branch has
  no unit test, since the providers are hardcoded URLs and a captcha cannot be
  summoned on demand; the classifier it calls is what the test pins.
- **Live run — GO** (network only, no model needed):
  `live_documentation_page_keeps_its_code` against the same page the transcript
  used — **18 907 characters** extracted (against the 12 000 truncated before),
  carrying `## Control`, a fenced ```v block and `fn (data &MyType) free()`, and
  landing as a **4-page by-reference attachment** whose result points at
  `attachment_read`. That is the pair of claims only a live fetch can settle,
  since both depend on the real markup. The other three network smokes
  (`live_fetch_without_summarize`, the YouTube dead-end one,
  `live_search_returns_results`) are green — the last one also confirming the
  provider fallback still answers from this IP.
- **The user's own live re-run confirmed all four in the app** (chat
  `28cdf212-…`, gpt-5.6): 15 messages instead of 20 — `fetch_url` → **all four**
  attachment pages read in a single round (the "walk `1..M` and know you read
  everything" guarantee, doing its job) → one `python_exec` call instead of six,
  with no `/tmp` write-then-lose and no second download. P4 fired for real: all
  three searches came back with the "every provider is throttling us" error
  instead of "no results", so the model got the truth rather than a false
  negative about the web.
- **And it exposed one more silent-wrong-answer path, fixed on the same branch.**
  The attachment was named **"V Documentation"** — the site-wide `<title>`;
  measured, `docs.vlang.io` gives every page that same title while `<h1>` names
  the page. Two pages of one site would have shared a name, and
  `attachment_read` resolves a name with `.find()` — **the first match**,
  silently reading a different file than the one asked for. Three guards, cheap
  and layered: the name now comes from **`<h1>` first** (better names, and it
  removes the collision at its source for the common docs layout); a name
  already taken by a *different* page gets the URL's last segment appended
  (deterministic, so re-fetching the same page keeps its name and replaces its
  own attachment rather than piling up copies); and `attachment_read` **reports
  an ambiguous name** with each candidate's source instead of guessing —
  addressing by source keeps working. The first two make a collision unlikely,
  the third makes it loud when it happens anyway, including for attachments that
  arrived some other way. Live on the same page: the attachment is now named
  **"Memory management"** rather than "V Documentation". **1814 unit tests
  green** (+3), 76 `#[ignore]`; the three new guards mutation-tested
  (preferring `<title>` over `<h1>`, dropping the disambiguation, dropping the
  ambiguity check each fail their own test).
- **Model-level regression — GO** (Gemma 4 31B q4_0 + bge-m3, external
  `llama-server`, `--jinja`), run once the user brought the LAN stack back up:
  **26 of 26** orchestrator e2e live smokes green in 556 s — memory/self-model/
  notes/graph/cross-organ links/RAG/attachments/control tools/i18n/MCP/tool
  confirmation. The right scope even though no orchestrator code changed: a tool
  now emits an effect on a path that previously never did, and the tool registry
  sits on every turn.

### Post-M9: collapsible tool calls, and the collapse state per chat (done)

- **Asked for directly**: tool calls should fold away the way "thoughts" already
  do, be **collapsed by default**, and the collapsed/expanded state should be
  remembered **per chat** — for both kinds of block. Plan with forks C1–C3 —
  [docs/feed-collapse.md](docs/feed-collapse.md) (**user's decision, 2026-08-03**,
  both questions as recommended). Behaviour — spec §11.3. Branch
  `feat/feed-collapse`.
- **Reading the code decided the shape and made it small.** Per-chat UI state
  already has a playbook — `Chat.draft` (spec §11.7): a field on `Chat`
  (`#[serde(default)]`, **no migration**), an `AppCommand`, a field in
  `ChatActivated`, written with the save debounce and **without touching
  `modified_at`**. Everything here follows it, so the per-chat half needed no new
  mechanism, only a second traveller on an existing road.
- **One place it deliberately departs from that playbook**: the toggle returns
  `ChatIntent::SetFeedView` **straight from `handle_key`** instead of raising a
  dirty flag the loop picks up. `draft_dirty`/`take_dirty_draft` exists because
  typing is continuous and must not send a command per keystroke; a `Ctrl+O` press
  is discrete, so the flag machinery would be ceremony.
- **`FeedView { thoughts, tools }`, a named type rather than two bools** (C1): it
  travels through a command, an event, a chat field and the feed's render-cache
  key — a bare `(bool, bool)` is exactly the pair that gets swapped by accident.
  `Copy + Eq + Hash`, both `false` = collapsed, and
  `skip_serializing_if = "FeedView::is_default"` so a chat nobody expanded
  anything in writes no new key at all.
- **`Ctrl+O` for tools** (C2, `Ctrl+T` being taken). The candidates were narrowed
  by what the terminal itself claims — `Ctrl+I` is Tab, `Ctrl+H` Backspace,
  `Ctrl+M`/`Ctrl+J` Enter — leaving `o`/`l`/`d` free in both the chat screen and
  `InputBox::on_key`. `Ctrl+L` carries "clear screen" from shells and `Ctrl+D`
  reads as EOF on unix, so `Ctrl+O` ("output"), which has no prior meaning.
  Layout-independent through `keys::hotkey_char`, as every other Ctrl shortcut.
- **What a collapsed call looks like** (C3): the card's **header stays**
  (`⚒ name(args)` — *what* ran is the informative half) and only the argument and
  result blocks fold away; the header then carries the same pill the collapsed
  thoughts block uses (marker, label, keycap), appended to its **last row** rather
  than pushed as a line of its own, so a call is one line collapsed and one line
  of header expanded. Overflowing the width is safe — `build_message_block` wraps
  every body line afterwards. A call with **nothing to hide** (no arguments, no
  result yet) gets no pill, mirroring `push_thoughts` on empty thoughts: promising
  something behind a pill that hides nothing is the small lie this project keeps
  finding and closing.
- **A test of mine was wrong about the code, and the code was right.**
  `tool_card_is_collapsed_by_default` first asserted that a short argument
  disappears — it doesn't: the presenter (`features/tools/present.rs`) puts a
  short scalar into the **header suffix** (`note_save(секрет)`) and only <!-- cyrillic-ok -->
  multi-line/long values into a block. That is the collapsed card's one-line
  summary working as designed, so the assertion moved to a multi-line
  `python_exec` argument, which is genuinely a block.
- **Tests**: collapsed by default (header kept, arguments and result gone, pill
  present, **exactly one row per call**), the toggle revealing them and hiding
  them again (and dropping the pill when expanded — the `⚒` header is the marker
  then), no pill when there is nothing to reveal, the pill localized across every
  built-in locale; the two keys reporting the whole new view and staying
  independent (plus `Ctrl+щ`, the physical `O`); activation applying the chat's
  stored state; and at the orchestrator level the write rules (flagged for saving,
  `modified_at` untouched, an identical state a no-op) plus a full round trip
  through the real `run` loop — expand in chat A, a new chat B opens collapsed,
  switching back restores A, and the choice is still on disk after a restart. The
  existing `cache_matches_fresh_render` gained the tools flag, so the cache is
  checked against all four combinations, and the entity has its own serde test for
  the no-migration claim, and one pinning both formerly-hardcoded labels across
  locales, plus nine for the expanded layout. **1833 unit tests green**
  (+19), 76
  `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`/i18n gates
  clean.
- **All seven load-bearing behaviours were mutation-tested** — never collapsing,
  dropping the "nothing to hide" guard, taking the state out of the cache key,
  bumping `modified_at`, activation ignoring the stored state, the key not
  reporting it back, and dropping `skip_serializing_if` — each fails its own test
  and only that one. (One mutation landed on the *first* `if !expanded` in the
  file, which is `push_thoughts` — so the thoughts tests got audited for free.)
- **A live run isn't required** (AGENTS.md §3): rendering, key handling and a
  per-chat field — no engine, memory, tool or provider path is touched. Two
  things were nonetheless checked against reality rather than reasoned about: the
  collapsed and expanded forms were **dumped side by side and read** before the
  tests were written (which is what showed the pill sits on the header rather
  than needing a line of its own), and a throwaway probe ran **all 182 chat files
  of the real dev data root** through the new type — every one loads as collapsed
  and writes back **without** a `feed_view` key, i.e. the additive claim holds on
  actual user data, not just on a fixture.
- **The `git checkout <file>` trap bit for a third time** (already recorded twice
  in this journal): reverting a mutation that way discarded the *whole*
  uncommitted change in `input.rs`, not just the mutated line, and the suite
  stayed red until it was re-applied by hand. Back the file up first — `cp` — or
  commit before mutating.
- **Two unlocalized strings found next door and fixed on the user's say-so**
  (they were spotted here and first recorded as groundwork; the user asked for
  them in the same change): the **expanded** thoughts label
  (`format!("{} мысли", …)`, while the collapsed pill correctly used <!-- cyrillic-ok -->
  `ui.feed.thoughts`) and the console exit-code label in `push_console`
  (`"код возврата: {code}"`). Both showed Russian under an `en` interface. <!-- cyrillic-ok -->
  - **The exit-code label got its own key rather than reusing
    `python.console.exit`**: that one is **axis A** — the label the *model* reads
    inside the tool result, written in the profile's language — while the feed
    re-renders it for the *human* after `present::parse_console` has stripped it,
    which is **axis B**. Same text today, different axis, so a Russian interface
    reading an English-profile chat now says "код возврата: 3" rather than <!-- cyrillic-ok -->
    inheriting the agent's language. New key `ui.feed.exit_code`; `push_block`/
    `push_console` gained the locale to reach it.
  - **Why the gate missed them, checked rather than assumed**: `cyrillic_scan.py`
    sets `in_test` on the first `#[cfg(test)]` it sees and **never unsets it**,
    and `message_feed.rs` has `#[cfg(test)]` **test accessors** at line ~564 — so
    every production line below them was scanned as test code, where Cyrillic in
    code position is legitimate fixture data. The strings are fixed; the
    scanner's blind spot is recorded in the roadmap (an attribute on a single
    item shouldn't mean "the rest of the file is tests").
  - Pinned by `expanded_thoughts_and_exit_code_are_localized`, which renders an
    **English-profile tool result under a Russian interface** and vice versa, and
    asserts no Cyrillic leaks into an `en` feed. Mutation-tested: restoring
    either literal fails it.
- **The collapsed pill reads like the thoughts pill** (asked for after the first
  live look): `▸ детали · Ctrl+O` against `▸ мысли · 9 стр. · Ctrl+T` — the <!-- cyrillic-ok -->
  separator is explicit here, since the thoughts pill gets its own from inside
  `ui.feed.thoughts_lines` and this pill has no count to show. The first
  mutation run **survived** — nothing asserted the separator — so
  `tool_card_is_collapsed_by_default` gained the assertion, which then failed
  under the same mutation.
- **An expanded card is a different presentation, not a longer one** (fork C4,
  the layout specified by the user after the same live look; collapsed
  unchanged): the tool's **name alone** in the header, every argument enumerated
  below one per line, a blank row, then the result.
  - **Why it was needed**: the header is a *title* — `truncate_header` flattens
    whitespace and cuts at `HEADER_MAX_CHARS = 100`, and `scalar_str` drops
    anything that is not a scalar. So a long query ended in `…` and an
    **array/object argument never appeared at all, in either mode**. That second
    half was the more interesting find: `set_sampling`'s `samplers`/
    `dry_sequence_breakers` and any MCP tool with structured arguments were
    simply invisible.
  - **`ArgDetail::{Compact, Full}` on the presenter**, split into
    `compact_args`/`full_args` rather than one function with a flag threaded
    through it — they are genuinely different presentations. It belongs there and
    not in the widget: which field is code, which is large, what may be folded
    into a header is knowledge `features/tools/present.rs` already owns, and the
    widget stays generic (spec §11.3).
  - **A value that cannot share a line with its key** — code, a large or
    multiline string — goes under a `key:` label as its own block, so
    `python_exec`'s code keeps its highlighting *and* is still named. Field order
    is `serde_json::Map`'s (alphabetical): the wire format does not preserve the
    model's own order for us, and stability is what matters for something read
    repeatedly. The doc comment says so, after checking `Cargo.toml` for
    `preserve_order` rather than assuming arrival order.
  - **My first attempt was conditional** — keep the compact header, and add the
    missing detail only when it truncated or dropped something. The user replaced
    it with the layout above, and was right: the conditional version made a
    card's structure depend on how long its values happened to be, so two
    neighbouring calls could look different for no visible reason.
  - The confirmation popup (§9.8) keeps `Compact` — a decision prompt, not a
    viewer, which is the decision the journal already recorded for it.
  - The existing presenter tests describe the compact form, so they got a
    **shadowing `present`** in `mod tests` (the `calc`/`datetime` precedent) —
    zero call-site churn, and `Full` has its own seven.
  - **A probe over realistic calls came before the tests**, and it corrected two
    fixtures I would otherwise have written wrong: a truncation needs **two**
    medium values whose join overflows (any single scalar over the ceiling is
    `is_big` and becomes a block instead, so a lone long value never reaches the
    truncation), and one widget test anchored on "the first row containing `1`",
    which the argument listing now matches before the result does.
  - Four mutations, each failing only its own test: `Full` falling back to the
    compact header, the blank row removed, the blank row appearing with no
    arguments, and a code argument losing its label.

### Deferred beyond M3
- **Per-message collapse/selection** in the feed — "thoughts" (`Ctrl+T`) and tool
  calls (`Ctrl+O`) collapse **for the whole feed at once**, with the state stored
  per chat; folding one *particular* block still needs a way to select it.
  **Account for the mouse toggle** (`Ctrl+W`, see post-M9): per-message selection/copy
  in the feed must coexist with wheel capture — either as in-app selection
  (mouse clicks go to the app when capture is on), or relying on native
  terminal selection when it's off (then copying is a plain drag-select).
- **Spellcheck dictionaries** (Hunspell pairs `*.aff`+`*.dic`: `en_US`/`en_GB`/`ru_RU`)
  live in the project root's `dictionaries/` **and are committed to the repository**. `build.rs`
  copies them to `target/<profile>/data/dictionaries/` at build time (out-of-the-box
  dev spellcheck); the release workflow packs them into the archive as
  `data/dictionaries/` (out-of-the-box spellcheck in the release too). Open `[R]`: ru_RU
  quality (in practice — it works).
- ~~Remove `#![allow(dead_code)]` from `main.rs`~~ — **done at M9**: dead code
  removed or marked pointwise (`#[allow(dead_code)]`/`#[cfg(test)]`).

## Pitfalls
- **The TUI "hangs" on a headless launch** (no TTY) — this is expected; clean exit
  via `q`/`Esc`/`Ctrl+C` is checked by the `map_key` unit tests. Live verification needs
  a real terminal.
- Pointwise `#[allow(dead_code)]` (with a comment) on deliberate ahead-of-consumer
  API: `ApiRole::System`, `Db::note_delete/rag_count`, `ToolRegistry::get`. The
  crate-wide allow was removed at M9. (`ToolContext.chat_id` left this list in the
  attachment-index stage — it now scopes `attachment_search`.)
- The app's data is portable: it lives next to the binary (in dev — `target/debug/`).
