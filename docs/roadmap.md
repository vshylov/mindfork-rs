# Possible Roadmap

> Live list of ideas. Implemented items don't come back here — track history
> lives in [CLAUDE.md](../CLAUDE.md) (post-M9 journal) and [docs/history/](history/).
> A compact summary of what recently closed is at the end of the file.

## Most valuable next
The unprioritized list below is an idea bank of equal weight; these five are
called out as the highest-payoff tracks (real user pain / direct savings):
1. **History compression / rolling summary** (§Context and tokens) — already
   hits the 8k-context ceiling of local models.
2. **Prompt caching** (§Context and tokens) — direct token/latency savings,
   unlocked by the stable daily self-model injection.
3. **Retry/backoff on cloud errors** (§Engine and reliability) — cloud
   reliability.
4. **Confirmation for dangerous tools** (§Tools) — human-in-the-loop before
   `python_exec`/file/destructive MCP calls.
5. **Multimodality (images)** (§Engine and reliability) — a big feature with
   demand.

## Memory, self-model, knowledge
The project's flagship track (self-model / notes / connectivity / RAG). The
core, the **self-model consolidation** (A), **RAG: sources and retrieval** (B)
and **embedding-model change** tracks are done (see "Recently closed" below and
`docs/history/`). **Deferred** groundwork remains:
- **vec0 for notes and observations as their count grows** — cosine is
  currently computed brute-force in Rust (`db::cosine`, `note_search_semantic`);
  for tens–hundreds (and up to thousands) of notes this is cheap (single-digit
  ms per `note_recall`). **Deferred** (premature optimization, decision
  2026-07): vec0 would only speed up the **query path**, not the O(n²)
  consolidation, and the implementation requires a schema migration for a
  stable integer rowid (notes have a `TEXT` uuid PK). Revisit **once the note
  count actually grows into the thousands**. Plan ready:
  [notes-vec0](notes-vec0.md).

## Context and tokens
- **History compression / rolling summary** — right now the whole conversation
  is sent on every request (`build_conversation_digest` only exists for
  titles). For local models with 8k context this hits a ceiling; a candidate
  is auto-summarizing old messages past a token threshold, next to the
  existing token counter.
- **Prompt caching** — `cache_control` (Anthropic) / `cached_tokens` +
  `prompt_cache_key` (OpenAI) aren't in the code. The self-model injection
  into `system` is stable within a day — a direct candidate for prompt
  caching (token and latency savings). See groundwork in
  [openai-responses-client](research/openai-responses-client.md).
- **`cached_tokens` in the token counter** — show the cache-hit share.

## Tools
- **MCP host — groundwork** (core is **done**: spec §9.6,
  [ADR 0007](decisions/0007-plugins-mcp-host-import-format.md); servers from
  `settings.json`, double opt-in, TOFU pinning, statuses/descriptions in
  settings):
  - HTTP transport (currently stdio only).
  - resources/prompts (currently tools only).
  - `notifications/tools/list_changed` — live catalog re-listing.
  - deferred schemas ("tool search") — context budget with many servers.
  - per-server tool count ceiling.
  - server editor UI (currently — edit `settings.json` by hand).
  - server `instructions` → system prompt.
  - non-text result blocks (currently — a placeholder).
  - WASM sandbox for untrusted tools.
  - localization of client wire errors (currently — a technical layer).
- **Confirmation for dangerous tools** — an interactive prompt before
  `python_exec`/file operations/destructive MCP calls (human-in-the-loop;
  `destructiveHint` annotations from MCP servers — untrusted, but usable as a
  hint). Reuses the `ConfirmAction` popup (the same mechanism already wired
  to `interface.confirm_destructive_keys` for `Ctrl+R`/`Ctrl+E`).

## Feed and chat UI
- **Per-message collapse/select/copy** — explicitly deferred past M3, noted
  "account for the mouse toggle `Ctrl+W`". Selecting a single message,
  copying a single block.
- **Horizontal scroll for wide tables** (instead of the current clip with
  "…") — groundwork from ADR 0003.
- **Mermaid: whitelist expansion** — flowchart/sequence already render
  ([implemented](research/mermaid-ascii-rendering.md) after our upstream fix
  to `mermaid-text` 0.56.1); stateDiagram/class/er are candidates once their
  text rendering becomes readable. Plus upstream groundwork: `max_width` as
  a hard budget (issue promised to the maintainer).
- **Wide glyphs on conhost: half-background** (upstream,
  [ratatui#2652](https://github.com/ratatui/ratatui/issues/2652) — filed by
  us with a byte-level repro). The trailing cell of a wide glyph carries the
  default style and is marked `skip`, so on a **style-only** edit (the glyph
  stays wide — e.g. the selection background moves off it) the diff only
  emits the leading cell, and the old background stays in the second half.
  Not VS16-specific: also reproduces on `😀`. **Can't be fixed locally** — the
  only workaround requires writing into the second half of the wide glyph,
  and that's exactly what
  [ratatui#2651](https://github.com/ratatui/ratatui/issues/2651) makes unsafe
  (`last_pos` ignoring width → the write drifts a column and shifts the row).
  Closing #2651 (tracking `last_pos` by width) would also remove our
  workarounds: the full redraw on screen switch / popup close and the VS16
  emoji swap in the grid. The mechanics and terminal-independent probes are
  in `shared::ui` and `widgets::emoji_picker`.
- **In-feed text search** (`/`-search with highlighting and jumps).
- **Regeneration with variations** — not just `Ctrl+R` with the same request,
  but with different sampling / picking from several response variants.
- **Editing any (not just the last) message** with history branching.

## Chat and profile management
- **Folders/tags for chats** — grouping in the list (`Esc`).
- **Export chat to a file** (Markdown/JSON) — currently there's only
  clipboard copy (`F5`); add saving to disk.
- **Pin important chats** at the top of the list.
- **Prompt templates / snippets** — quick inserts of frequently used system
  messages or seeds.
- **Search within chat content** (not just the title) — full-text search via
  SQLite FTS.

## Engine and reliability
- **Provider bridges** — document in install.md the "any OpenAI-compatible
  endpoint" pattern (external + LiteLLM/OpenRouter); optional
  managed-custom-command (supervisor launches an arbitrary sidecar proxy) —
  on demand. See [plugin research §6](research/plugin-system.md).
- **API keys: storage extensions** (ADR 0008) — an OS keychain as an
  additional `scheme`; the same mechanism for the external proxy and MCP
  server env maps; UI management of other machines' entries ("forget this
  computer").
- **Retry/backoff on cloud provider network errors** — clients currently
  surface the error body but don't retry (transient 429/5xx/timeouts).
- **Configurable health-check cadence.** The monitor's intervals
  (`HEALTHY_POLL` 60 s / `RECHECK_POLL` 5 s), the failure streak (3) and the
  relaunch budget (≤3 per 5 min) are constants. Nothing has asked for them to be
  settings yet; if a setup appears where they're wrong (a very slow link, a
  deliberately flaky server), they'd go in the "Model" section next to the
  engine fields.
- **On-the-fly model switching** without a full settings-section restart (a
  quick model selector right in the chat).
- **Multimodality support** (images) — if the model/`llama-server` supports
  vision; passing images from clipboard/file.

## Testing, CI, quality
- **Live `#[ignore]` smokes in CI** — **closed for the llama.cpp set**:
  `tools/e2e_hf.py` + the `workflow_dispatch` **Live e2e** workflow run it
  against ephemeral HF Inference Endpoints (chat + two embedding models), so the
  gate no longer needs the one GPU machine
  ([history/remote-e2e-hf.md](history/remote-e2e-hf.md)). Still open: a *scheduled* run
  (deliberately not wired — R5a; every run costs ~$1 and the non-hermetic smokes
  would flake unattended), and the cloud-key and Python-sandbox smokes, which
  need different credentials and assets.
- **Remote gate: chat-free runs** — `run` always creates the L40S chat endpoint,
  even for a filter that only exercises the embedders (~$0.10 wasted per such
  iteration). The probe has `--embed-only`; the runner has no `--no-chat`.
  Minor, and the full gate always needs chat.
- **Hot-path benchmarks** — feed rendering (markdown+syntect cache), line
  wrapping, brute-force memory cosine — a performance regression detector.

## Multilingualism (i18n)

> **Track finished** (axis A tiers 1–3 + axis B + i18n CLI stages 1–3): agent
> language ([docs/history/i18n.md](history/i18n.md)) and external locales
> ([docs/history/i18n-external-locales.md](history/i18n-external-locales.md)),
> interface language ([docs/history/i18n-ui.md](history/i18n-ui.md)), all CLI
> text ([docs/history/i18n-cli.md](history/i18n-cli.md)). Below — only the
> remaining groundwork.

- **Hot-reload of external locales** — editing `data/locales/*.json` applies
  on restart (the registry is `&'static`-leaked); reloading live would
  require a different ownership model.

> **Not-roadmap, recorded as a decision:** CLI subcommand/flag names are
> **not** localized (part of the protocol — translating a command like
> `backup` into the interface language is bad practice, especially now that
> the source itself is English). Pluralization is intentionally
> number-neutral (a plain `tf` without grammar) — real grammar is only
> needed on axis B, if a language with complex pluralization shows up.

## Other
- **Installers — groundwork** (core is **done**,
  [docs/history/installers.md](history/installers.md): Windows Inno Setup +
  Linux nfpm, shipped with releases): Windows code signing (deferred until
  public launch — SignPath/Certum/Azure Artifact Signing, §5 of the doc);
  winget manifest (portable-zip until signing); AUR `mindfork-rs-bin`; MSI
  for GPO/Intune on demand.
- **Publishing to crates.io as `mindfork`** — the short name is **still free**
  (checked 2026-07-26; the registry API 404s on it), and the "About" dialog
  (`F1`) already lists `crates.io/crates/mindfork` as the future home. Two ways
  to take it: rename the package outright — which also renames the binary and
  ripples into the installer, the nfpm layout, the `.desktop` `Exec=`, the
  `/usr/bin` symlink, the docs and CI artifact names — or keep the binary name
  and set `name = "mindfork"` + `[[bin]] name = "mindfork-rs"`, which claims the
  crate name with a far smaller blast radius. Until it is actually published,
  the URL stays informational: a `crates.io` version badge in the README or a
  `cargo install mindfork` line in `install.md` would be visibly broken.
- **Auto-update** — self-update, musl-static and arm64 builds, an "a new
  version is available" notice in the TUI. Groundwork from the finished
  "release engineering" track
  ([docs/history/release-engineering.md](history/release-engineering.md) §5);
  the release pipeline (tag → archives + sha256 + packages + installer on
  GitHub Releases) already exists.
- **Message playback (TTS)** — **the `feat/tts` stage is implemented**
  (2026-07-23, [research/tts.md §13](research/tts.md),
  [CLAUDE.md](../CLAUDE.md)): the `/tts` command (`/tts N`/`all`/`stop`), a
  Markdown speech extractor (`shared/markdown/speak.rs` — code/mermaid/tables/
  formulas get a voice note), the `rodio` player, stop points, a "Playback"
  settings tab. **Primary engine — OpenAI TTS** (`gpt-4o-mini-tts`, `onyx`
  voice) — not a new vendor (the key is already stored, ADR 0008); plus
  `gemini` and `external` (any OpenAI-compatible server). Groundwork: a
  **local engine** (vosk-tts + our own Russian Rust frontend, a managed
  sidecar `mindfork tts setup` modeled on ADR 0005 — for an offline/non-OpenAI
  audience; ElevenLabs/Azure are skipped — they'd need their own signup);
  **stitching chunks across speech-block boundaries** (even smoother
  multi-paragraph playback); SSE streaming within a chunk; audio caching;
  auto voice selection by message language; **native Gemini multi-speaker**
  (one request, several voices — right now different voices per role are done
  with two requests/engines, which works across all providers); OS TTS.
- **Voice input (STT)** — the other half of "voice"; a notable feature for a
  TUI chat.
- **Custom keyboard layout** — there's already a field for this under
  "Interface" (marked as groundwork since M9), but actual custom-binding
  application isn't done.
- **Layout-independent hotkeys on unix beyond Cyrillic** — **blocked on an
  upstream release.** The kitty keyboard protocol carries the answer (the *base
  layout key* of the "report alternate keys" enhancement), but crossterm 0.29
  parses only the shifted alternate and drops it
  ([#968](https://github.com/crossterm-rs/crossterm/issues/968)). The parsing +
  API patch is written, verified on Linux and submitted as
  [crossterm#1074](https://github.com/crossterm-rs/crossterm/pull/1074); an
  end-to-end spike through our resolver is validated. Remaining once it ships on
  crates.io: bump, push `REPORT_ALTERNATE_KEYS` alongside
  `DISAMBIGUATE_ESCAPE_CODES` in `app/runtime/mod.rs`, add the tier-0 branch in
  `shared/keys.rs` (**after** the ASCII short-circuit — see the AZERTY case in
  the research doc) — no call site changes. Note that crossterm merges PRs
  regularly but last released in April 2025, so this may sit for a while. On
  Windows every layout already works (stage 1); VTE-family terminals need
  nothing. See
  [docs/research/layout-independent-hotkeys.md](research/layout-independent-hotkeys.md).
- **Applying a theme from a color configuration** — a user palette layered
  over auto/dark/light.

---

## Recently closed
A compact summary (details — in [CLAUDE.md](../CLAUDE.md) and
[docs/history/](history/)):
- **Embedding-model change** (stages 1–3, complete): a swap is detected
  behaviourally by a canary vector — dimensionality is *not* identity —
  `/reindex` re-embeds every stored vector in place from the text the DB already
  holds, and the memory similarity gates are read as positions in a reference
  scale, calibrated automatically per model (so they follow the model instead of
  one fixed calibration, and stay exactly as they are until a model actually
  changes). This also closed the older "re-indexing chat attachments after a
  model change" item: its "needs a walk over all chats" premise was wrong for
  this purpose (`attachment_documents.chunk_text` is in the DB, so a walk is
  only needed to re-*chunk*), and attachment indexes now come back without
  re-attaching each file. See
  [embedding-model-change-reindex.md](research/embedding-model-change-reindex.md),
  spec §9.3.4.
- **English source** (prep for open source: all docs, comments and
  non-user-facing strings moved RU -> EN; the convention flipped and guarded in
  CI by `tools/cyrillic_scan.py`) —
  [english-source-migration.md](history/english-source-migration.md).
- **API keys in settings** (entry in the settings window, machine-bound
  encryption in the config, per-machine entries) —
  [ADR 0008](decisions/0008-api-key-storage.md), research
  [api-key-storage.md](research/api-key-storage.md).
- **Self-model consolidation** (auto-"sleep" on a timer + summary↔observation
  semantics + interest aging) —
  [self-model-consolidation.md](history/self-model-consolidation.md).
- **RAG: sources and retrieval** (html/pdf/docx + per-chunk indexing progress +
  cross-source dedup) —
  [rag-sources-retrieval.md](history/rag-sources-retrieval.md).
- **Plugins / MCP host** + generic import (`mindfork import`, the
  [mindfork-import](import-format.md) format) —
  [ADR 0007](decisions/0007-plugins-mcp-host-import-format.md).
- **Installers** (Windows Inno Setup + Linux nfpm) —
  [installers.md](history/installers.md).
- **Release engineering** (versions/CHANGELOG/CI/schema migrations) —
  [release-engineering.md](history/release-engineering.md).
- **Multilingualism** (axis A, axis B, CLI) — [i18n.md](history/i18n.md) and
  neighbors.
- **OS-locale language detection** (`i18n::detect_os_language`) — installer
  stage 1.
- **Mermaid rendering** (flowchart/sequence) —
  [mermaid-ascii-rendering.md](research/mermaid-ascii-rendering.md).
