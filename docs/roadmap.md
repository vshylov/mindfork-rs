# Roadmap

> What may come next, what is waiting on something outside the repository, and
> what was considered and deliberately left out — with the reason. Finished work
> does not stay here: what was done, why, and what was measured lives in
> [docs/journal/](journal/) (by subsystem) and [docs/history/](history/), and
> [CHANGELOG.md](../CHANGELOG.md) says what each release changed. The
> [closed list](#closed) at the end is an index of pointers, nothing more.

## How to read this file

Every open item is one bullet, tagged with why it is still open:

| Tag | Meaning |
|---|---|
| **idea** | nobody has asked for it yet; ideas are of equal weight, unprioritized |
| **deferred** | designed or measured, and consciously put off — the bullet says what would bring it back |
| **blocked** | waits on something outside this repository: an upstream release, an approval, a plan |
| **on demand** | small and understood; built the day someone hits the wall |

A track (multi-stage work with a design doc) sits here only while it is open.
When it finishes it moves to [Closed](#closed) as one line, and whatever it left
open stays above as bullets of its own — a finished track's leftovers are where
most new items come from. An item that was measured and turned down goes to
[Decided against](#decided-against), so that it is not re-proposed without new
evidence. Adding or closing an item is part of every task's documentation step
([AGENTS.md §4](../AGENTS.md)).

## Most valuable next

- **A fetched page searchable in the turn that fetched it** — stage 2 of
  [attachment-birth-turn.md](research/attachment-birth-turn.md), chosen by the user
  on 2026-09-26 as the next stage. Stage 1 made the birth turn honest (no search
  offered, a search tried anyway names the file); the turn still has to find its
  place by sampling pages, and on the live runs two birth turns of six spent the
  whole 8-round budget that way where one search call would have answered.

The other tracks this section carried — the first public release, images,
retry/backoff and the prompt-caching measurement — have all closed. The lists
below are otherwise an idea bank of equal weight.

## Open

### Engine and providers

- **Provider bridges — what remains.** The "any OpenAI-compatible endpoint"
  pattern (external + LiteLLM/OpenRouter) is documented in install.md §3,
  with the key such a gateway needs ([external-api-key.md](history/external-api-key.md))
  and the model name it routes on ([external-model-name.md](research/external-model-name.md));
  the OpenRouter review is closed. Left:
  - **on demand** — a **managed custom command**: the supervisor launching an
    arbitrary sidecar proxy ([plugin-system.md §6](research/plugin-system.md)).
  - **deferred** — **embeddings through a gateway**: the `Embedder` half sends
    `model`, so the "Embeddings" tab can point at one, but no run has
    ([openrouter-external.md §4.1](research/openrouter-external.md)).
  - **on demand** — **OpenRouter's own request knobs** (`provider` routing, the
    `models` fallback, `transforms`, attribution headers, `usage.cost`) —
    deliberately not exposed (§6 there). `provider` routing is the one with a
    measured use: a routed provider that misses its model's tool template, whose
    special tokens then end the turn as reply text (§8.1 there).
  - **deferred** — **xAI's server-side Live Search / X Search**, and the
    Responses route as the cheap way to xAI-only features
    ([grok-xai-provider.md §5](research/grok-xai-provider.md), F1 and F6).
- **An embedder context setting** — **on demand**. The managed embedder's
  context is the hardcoded `DEFAULT_CONTEXT_SIZE` in `supervisor.rs`
  ([cloud-provisioning.md §9](research/cloud-provisioning.md)). Reachable
  since 2026-09-26 as `-c N` in the embedder's *Extra arguments*
  ([managed-extra-args.md](research/managed-extra-args.md)) — the last
  occurrence wins, and upstream has deprecated repeating a flag, so a field of
  its own is still the lasting answer.
- **Raw arguments — leftovers** ([managed-extra-args.md](research/managed-extra-args.md);
  closed 2026-09-26) — **on demand**, each: the refusal table is read from one
  llama.cpp build and a rename upstream quietly turns a refusal into "the last
  occurrence wins" until `the_table_matches_the_binary_live` is run against the
  new build; a `MINDFORK_LLAMA_ARGS` variable for the dev launch, which
  `setup --set` covers today.
- **`MINDFORK_MODEL` is ignored unless `MINDFORK_LLAMA_BIN` is set** —
  **on demand**. An adjacent finding of the provisioning track (§2.2, §9 there):
  an empty binary has resolved by itself since the download track, so the
  variable could stand alone. A small fix in its own PR, or a line in install.md.
- **Configurable health-check cadence** — **on demand**. The monitor's intervals
  (`HEALTHY_POLL` 60 s / `RECHECK_POLL` 5 s), the failure streak (3) and the
  relaunch budget (3 per 5 min; the MCP host keeps its own copy) are constants.
  If a setup appears where they are wrong (a very slow link, a deliberately
  flaky server), they go in the "Model" section next to the engine fields.
- **The batch on the settings screen** — **idea**. The measured prefill
  throughput shown beside the Batch (`-b`) field, and setting the batch for the
  user (F4b) — [slow-prefill-detection.md §7](research/slow-prefill-detection.md).
- **On-the-fly model switching** — **idea**. A quick selector in the chat,
  without a settings-section restart. The settings screen already asks the
  provider what it serves (`Enter` on a model row, 0.10.0,
  [model-picker.md](research/model-picker.md)); the Video model row is not
  covered by it yet.
- **The Gemma stack's slow prefill** — **deferred, one measurement short**.
  ~50 tok/s on b10791 with the projector, against ~2200 tok/s for Qwen on the
  same line; the same model without the projector on a rented L40S prefills at
  ~2 100 tok/s ([parallel-subagents.md §3.7](research/parallel-subagents.md)),
  so it is the projector or the Windows CUDA build, not the model's path. The
  LAN line without `-mm` is the one remaining check.
- **Admission by budget — leftovers** (§8 of
  [admission-by-budget.md](research/admission-by-budget.md); the track is
  closed) — **on demand**, each: the external shape typed, a `pool` field for a
  split server (F4b); a "waiting for room" label on the status chip and the
  transcript row (F6b — the tasks screen already says *waiting* for the app's
  own tasks); `--kv-unified-per-slot` as a managed option (F7b — reachable
  now through *Extra arguments*, without the app knowing the per-slot limit);
  a retry that
  re-enters `acquire` instead of waiting on a timer (the honest fix is the
  estimate); a per-client test of an error object arriving after a text delta,
  for the Anthropic and Gemini in-stream envelopes; the parked-set bound of the
  RAM prompt cache.
- **Retry/backoff — leftovers** ([cloud-retry-backoff.md](research/cloud-retry-backoff.md);
  the track is closed) — **deferred**: the same policy for the **other HTTP
  callers** (F9) — embeddings above all, since `/reindex` re-embeds hundreds of
  chunks and one `429` voids a batch, plus TTS and `youtube_watch`; each has its
  own degradation story, so none is broken, they simply do not recover. And
  **quota-vs-rate `429` discrimination** (F5): a billing-flavoured `429` costs
  two short waits before the body is shown; a marker blacklist in the
  `OVERFLOW_MARKERS` shape would skip them — left out until the wasted seconds
  bite.
- **API keys — storage extensions** (ADR 0008) — **idea**. An OS keychain as an
  additional `scheme`; UI management of other machines' entries ("forget this
  computer"); an explicit cleanup of MCP secrets orphaned by a rename or delete
  — deliberately not collected today (spec §9.6,
  [mcp-server-editor.md](history/mcp-server-editor.md) S4(b)).
- **Provisioning — optional measurements and the wider box** — **on demand**.
  `tools/pod_probe.sh` on an A100/H100 for the JIT, a network volume's read
  speed, `machine-id` across a stop
  ([cloud-provisioning.md §8](research/cloud-provisioning.md), stage 2); and,
  from its §9, Vulkan in containers, source builds, a CUDA build of our own.

### Context and tokens

- **Prompt caching** — **deferred** (user's decision 2026-08-08; the contracts,
  three live measurements and the decision points are in
  [prompt-caching.md](research/prompt-caching.md)). Caching is automatic on
  llama.cpp, OpenAI and Gemini and opt-in only on Anthropic, which caches
  nothing today. The gain hinges on a stable prefix, and our volatile block
  sits at the end of `system` — measured, a change to it costs the whole
  conversation (llama.cpp 0% vs 99% reuse; OpenAI 0% vs 81%; Anthropic a
  2304-token write per turn vs none). Relocating the block is
  [decided against](#decided-against). Still available and layout-independent:
  Anthropic breakpoints (the head alone is worth ~5k tokens a turn), the cache
  share in the token counter, `prompt_cache_key`. The promising way to the rest
  is a **stable observation selection** rather than a relocation (§5 F1(e)
  there) — untested.
- **`cached_tokens` in the token counter** — **idea, and the instrument**. All
  four providers report the cache-hit share and the client drops it (llama.cpp
  `timings.cache_n`, present since 2025-09 and not gated by `include_usage`;
  OpenAI `prompt_tokens_details.cached_tokens`; Anthropic
  `cache_read_input_tokens`; Gemini `cachedContentTokenCount`). Beyond the
  display: a prefix broken by some future change is otherwise invisible. The
  accounting trap is measured in [prompt-caching.md §3.3](research/prompt-caching.md):
  with caching on, Anthropic's `input_tokens` is only the *uncached remainder* —
  13 tokens for a 7.4k prompt — so the counter must sum it with the cache
  fields.
- **History compression — leftovers** ([history-compression.md](research/history-compression.md),
  spec §6.7; the track is closed) — **deferred**: a **semantic index over the
  folded range** — sub-decision S11 chose the full-text index the app already
  keeps and recorded the embedding variant as the answer if a live run ever
  shows lexical misses; **learning the window from the 400 body** (S3) is
  redundant with `/props`, since the body shape that carries `n_ctx` is
  llama.cpp's own. (The estimate's blind spot for `req.tools`, once listed
  here, was found and fixed on 2026-09-09 —
  [roll-usage-calibration.md §2.1](research/roll-usage-calibration.md).)

### Tools and MCP

- **MCP host — groundwork.** The core is done (spec §9.6,
  [ADR 0007](decisions/0007-plugins-mcp-host-import-format.md): double opt-in,
  TOFU pinning, statuses in settings, the server editor); the client is stdio
  and tools-only. **idea**, each: HTTP transport; resources and prompts;
  `notifications/tools/list_changed` (live catalog re-listing); deferred
  schemas ("tool search") for the context budget with many servers; a
  per-server tool-count ceiling; the server's `instructions` into the system
  prompt; audio and resource result blocks (images are done —
  [mcp-tool-images.md](research/mcp-tool-images.md); the other two are
  placeholders, and nothing downstream can carry them yet); a WASM sandbox for
  untrusted tools; localization of the client's wire errors (a technical layer
  today).
- **Concurrent tools — what §8 left** ([concurrent-tools.md](research/concurrent-tools.md),
  [ADR 0012](decisions/0012-concurrent-tool-calls.md); the track is closed) —
  **deferred**: `web_search` in the marked set, after a measured probe of three
  concurrent searches on the keyless chain or per provider (Tavily first; F4
  there); MCP tools on `readOnlyHint` only as a per-server opt-in the user
  types, never the server's word alone (§4.9 there); `youtube_watch` once a
  marked tool with an effect has exercised the effect ordering, `note_recall`
  once the vector backfill leaves the read path or becomes a single flight.
- **Sub-agents and dialogues — leftovers** (the tracks are closed) — **idea**,
  each: notifications across profiles, and a searchable history of runs beyond
  the tasks screen's cap ([background-subagents.md §8](research/background-subagents.md),
  [tasks-screen.md §8](research/tasks-screen.md)); a "stop all" key on the
  tasks screen ([tasks-stop-all.md §7](research/tasks-stop-all.md)); a roll
  that re-plans against the conversation at its retry
  ([silent-preemption.md §8](research/silent-preemption.md)); dialogue
  participants with tools (ADR 0011 F4), `Message.author` as the growth path
  past two participants, the director editing an arbitrary earlier message,
  and a director's pre-brief before the first line — leaning no
  ([two-agent-dialogue.md §8](research/two-agent-dialogue.md),
  [background-dialogues.md §8](research/background-dialogues.md)). Per-speaker
  voices over a dialogue transcript are under Speech; a parent's JSON export
  not carrying its transcripts is under Chats, search and data.
- **Web search — the keyed providers** — **deferred**. Serper and the cloud
  providers' server-side search tools; SearXNG was not adopted and its design
  stays on record ([web-search-keyed-providers.md §7](research/web-search-keyed-providers.md),
  F2).
- **`fetch_url` — tables in rich extraction** — **deferred**. Headings and code
  blocks survive ([fetch-url-fidelity.md](history/fetch-url-fidelity.md));
  tables need a layout decision, Markdown table vs flattened rows. (A persistent
  scratch directory for `python_exec`, once listed here, was met another way:
  files a call saves to `/w/out` stay with the chat and go back in via `files`
  — [sandbox-file-exchange.md](history/sandbox-file-exchange.md); a directory
  shared across calls is out of scope there, §1.1 D4.)
- **The sandbox's network: plain `http://` to an off-machine host fails under
  `--net`** while HTTPS works — **deferred**
  ([safe-defaults.md §7](research/safe-defaults.md)).
- **YouTube — leftovers** ([youtube-integration.md](research/youtube-integration.md),
  spec §9.9; both stages are closed) — **deferred**: **cross-chat caching** of
  an expensive watch (R8b) — within one chat a follow-up is free, the result is
  in the history; only a *different* chat re-pays, and `cache.db` is the wrong
  home, being disposable by design; a transcript path that needs **no cloud
  key** (R7) — everything free is PoToken-gated, so this means a paid captions
  vendor or a `yt-dlp` sidecar; **default-model rot** — the default moved from
  `gemini-2.5-flash-lite` (a 404 for new users) to `gemini-3.5-flash`, and
  whatever ships as the default eventually stops existing; the settings picker
  does not cover the Video row yet.

### Memory, notes and RAG

- **vec0 for notes and observations as their count grows** — **deferred**
  (premature optimization, decision 2026-07). Cosine is brute-force in Rust
  (`db::cosine`, `note_search_semantic`); for tens to hundreds of notes that is
  single-digit ms per `note_recall`, and up to thousands still cheap. vec0 would
  speed up only the **query path**, not the O(n²) consolidation, and needs a
  schema migration for a stable integer rowid (notes have a `TEXT` uuid PK).
  Revisit once the note count actually grows into the thousands; plan ready —
  [notes-vec0.md](notes-vec0.md).
- **Re-chunking attachments after a chunking change** — **deferred**. A model
  change needs no walk over the chats — `/reindex` re-embeds from the chunk
  text the DB already holds — but a change of chunk parameters still would
  ([embedding-model-change-reindex.md](research/embedding-model-change-reindex.md)).

### Feed and rendering

- **Syntax grammars — a user overlay and more languages.** 22 grammars are
  vendored ([vendored-syntaxes.md](history/vendored-syntaxes.md)), which covers
  what a model usually tags a block with; the rest of the long tail (Fortran,
  COBOL, Vala, Haxe, …) is one manifest row plus one file each — **on demand**.
  **V** is **blocked** on licensing: its only grammar has none, so the label
  maps to Go. A user dropping a grammar into `data/syntaxes/`, the way external
  locales work, is **deferred** by the same plan (F5): the overlay would parse
  and re-link at runtime, paying back the ~130 ms the build-time dump removes,
  so it needs its own measurement. Unexplored: replacing the base bundle with
  the current `sublimehq/Packages` (198 syntaxes, newer versions of what we
  ship).
- **Per-message collapse, select and copy** — **idea**, deferred past M3
  ("account for the mouse toggle `Ctrl+W`"). Collapsing is done for the *whole
  feed* (`Ctrl+T` thoughts, `Ctrl+O` tool calls, stored per chat, spec §11.3),
  and `F5`/`/copy` take the whole conversation; folding or copying one
  particular message or block needs block selection first.
- **Horizontal scroll for wide tables** — **idea**, instead of the clip with
  `…`; groundwork from [ADR 0003](decisions/0003-own-markdown-renderer.md).
- **Mermaid — whitelist expansion** — **deferred**. flowchart and sequence
  render ([mermaid-ascii-rendering.md](research/mermaid-ascii-rendering.md));
  stateDiagram, class and er are candidates once their text rendering reads
  well. The upstream groundwork this item used to carry is done: `max_width` as
  a hard budget shipped in `mermaid-text` 0.57.0 (`max_width_strict`, from our
  feature request), and the app keeps its own width check on purpose.
- **Wide glyphs on conhost: the half-background** — **blocked on a ratatui
  release**, and the block has moved. Both issues we filed are fixed on
  upstream `main`: [ratatui#2651](https://github.com/ratatui/ratatui/issues/2651)
  (`last_pos` ignoring a glyph's width, so a write after a wide glyph drifted
  a column) by PR #2721, merged 2026-08-24 for the 0.30.3 milestone, and
  [ratatui#2652](https://github.com/ratatui/ratatui/issues/2652) (the trailing
  cell of a wide glyph never repainted on a style-only change, so the old
  background stayed in its second half; not VS16-specific, `😀` too) by
  PR #2743, merged 2026-09-03. The newest release is still 0.30.2
  (2026-06-19), the one in `Cargo.lock`. Once 0.30.3 ships: bump, re-run the
  byte-level probes on conhost, and remove the workarounds #2651 forced — the
  full redraw on screen switch and popup close, and the VS16 emoji swap in the
  grid (mechanics in `shared::ui` and `widgets::emoji_picker`, whose canary
  test asserts the upstream defect and will fail when the fix arrives).
- **Rendering images in the feed** — **idea**. kitty/sixel/iTerm2, or a
  halfblock renderer over the decoded pixels; today a sent image leaves no mark
  in the message, and the only chips are for images waiting to be sent and on
  a tool's card ([multimodal-images.md §5](research/multimodal-images.md),
  spec §9.10).
- **Regeneration with variations** — **idea**. Not just `Ctrl+R`/`/regen` with
  the same request: different sampling, or picking from several response
  variants. Today a replaced reply is only archived.
- **Editing any message, not just the last, with history branching** —
  **idea**. `Ctrl+E`/`/takeback` edits the last exchange and `/clone` copies a
  whole chat; there is no fork at a chosen message.
- **`chat://` addresses — leftovers** ([chat-uri-links.md](research/chat-uri-links.md),
  spec §9.11, §11.3; the track is closed) — **deferred**: message- and
  page-level addresses (`chat://<id>/p3`) — `chat_search` names the page,
  `HistoryView::locate` maps a page back to a message and `OpenChatAt` takes a
  message uuid, so a hit could open exactly where it matched; left out until the
  address format has proved itself without a path component. A back-stack
  deeper than one step — a chain A → B → C steps back to B and no further, the
  shape the search half has always had; a true stack would have to answer what
  an ordinary chat switch does to its *middle*, which nothing has asked for.
  An address split across two rendered rows is styled but not clickable
  (`Ctrl+L` still follows it) — carrying link identity through the wrap is a
  span-metadata problem ratatui gives no room for. Other organs' schemes
  (`note://`, `attachment://`) — until there is a second live consumer.
- **A theme from a color configuration** — **idea**. A user palette layered
  over auto/dark/light; today `Theme` is those three, each a built-in palette.

### Input, keys and the terminal

- **Layout-independent hotkeys on unix beyond Cyrillic** — **blocked on an
  upstream release**. The kitty keyboard protocol carries the answer (the *base
  layout key* of the "report alternate keys" enhancement), but crossterm 0.29
  parses only the shifted alternate and drops it
  ([#968](https://github.com/crossterm-rs/crossterm/issues/968)). The parsing
  + API patch is written, verified on Linux and submitted as
  [crossterm#1074](https://github.com/crossterm-rs/crossterm/pull/1074) — still
  open as of 2026-09-25, and crossterm's last release is 0.29.0 of 2025-04-05,
  so this may sit for a while. Once it ships: bump, push
  `REPORT_ALTERNATE_KEYS` alongside `DISAMBIGUATE_ESCAPE_CODES` in
  `app/runtime/mod.rs`, add the tier-0 branch in `shared/keys.rs` **after** the
  ASCII short-circuit (the AZERTY case in the research) — no call-site changes.
  Windows already works on every layout (stage 1); VTE-family terminals need
  nothing. [layout-independent-hotkeys.md](research/layout-independent-hotkeys.md).
- **Custom key bindings** — **idea**. Every chord is hardcoded and nothing in
  the settings describes one (the M8-era "custom layout" field is gone). No
  longer the answer to a host stealing keys — the typed routes are
  ([command-only-control.md](history/command-only-control.md)) — so this is a
  convenience for power users rather than an accessibility gap.
- **A clipboard for JupyterLab's terminal** — **blocked upstream**. It embeds
  xterm.js *without* `@xterm/addon-clipboard`, so it drops OSC 52
  ([osc52-clipboard.md](history/osc52-clipboard.md)); nothing the app can send
  fixes that. `/export` is the route that works there, and the copy's message
  points at it.
- **`terminal_compat` on legacy conhost** — **deferred**. Whether the switch
  could be detected rather than set is a question with no measurement yet
  ([robustness-and-defaults.md §1](research/robustness-and-defaults.md)).

### Chats, search and data

- **Merging two copies of the data** — **idea**. `mindfork stats --compare`
  says what each copy holds that the other lacks and deliberately stops there
  (G8, [data-stats.md §5](history/data-stats.md), spec §12.4): a chat file
  names a profile and may own `files/` and `workspace/` directories and an
  attachment index, so carrying one by hand is not a promised path. A `merge`
  would be the track that makes it one — additive only (a chat or note that
  exists on one side), with a diverged chat left for the user. Smaller and
  **on demand**: `stats` pointed at an arbitrary data directory, and per-profile
  breakdowns (§3 there).
- **Folders or tags for chats**, **pinning** important ones at the top of the
  list, **prompt templates / snippets** for frequently used system messages or
  seeds — **idea**, each; none has a field yet.
- **Widening what chat search indexes, and ranking** — **idea**. The index
  covers message text of every role — tool results included, since each is a
  `Tool`-role message, and sub-agent transcripts since 0.9.8 — but not thoughts,
  tool-call arguments or attachments; the index is disposable, so widening it
  costs a rebuild rather than a migration. Ranking is the other open question:
  nothing ranks today, the search screen groups by chat precisely to sidestep
  it, and the research measured trigram's `bm25` as a weak though non-degenerate
  proxy. Both pay twice now: the assistant-facing `chat_search`/`chat_read` pair
  reads the same index and inherits the same limits
  ([cross-chat-search-tool.md](research/cross-chat-search-tool.md), spec §9.11;
  [chat-search-stage2.md](history/chat-search-stage2.md) F3).
- **`cache.db` for chat-list summaries — a lazy orchestrator** — **deferred by
  measurement** (2026-07-30, [chat-content-search.md §9.1](research/chat-content-search.md)).
  The chat list is built by parsing every `chats/*.json` at startup: **36.5 ms
  in release** and ≈ 7 MB resident on the dev corpus (171 chats, 1540 messages,
  14 MB of JSON). The parse feeds `Orchestrator.chats: Vec<Chat>`, which holds
  every chat's full history, so removing it means making the orchestrator
  **lazy** — a multi-stage track through the "sole owner of `Chat`" invariant,
  with one structural obstacle (`search.rs::group_hits`) and a clean way past
  it (store the message's position in the index). Cost and payoff both grow
  linearly: ×10 is ~365 ms and ~70 MB. Revisit at ~1000 chats, or a release
  parse over ~300 ms.
- **`/export json` carries no tool calls and no sub-agent transcripts** —
  **deferred**. The `mindfork-import` v1 document has no place for them; every
  such export says so, and `md` keeps everything
  ([chat-export-file.md](history/chat-export-file.md)).
- **`MINDFORK_BACKUP_PASSWORD`** for scripted or CI backups — **deferred** (F7
  of [backup-password.md](history/backup-password.md)): trivial to add, left
  out so there is one fewer place a password can come from.

### Speech

- **Message playback (TTS) — groundwork.** The `/tts` stage shipped on
  2026-07-23 (OpenAI `gpt-4o-mini-tts`, Gemini and any OpenAI-compatible server;
  the Markdown speech extractor, the `rodio` player, a "Playback" tab —
  [tts.md §13](research/tts.md), [ADR 0009](decisions/0009-tts-speech-synthesis.md)).
  **idea**, each: a **local engine** (vosk-tts with a Russian frontend of our
  own, a managed sidecar `mindfork tts setup` on the ADR 0005 pattern — for an
  offline audience; ElevenLabs/Azure skipped, they need their own signup);
  stitching chunks across speech-block boundaries; SSE streaming within a
  chunk; an audio cache; voice selection by message language (today by role);
  native Gemini multi-speaker (one request, several voices — today two
  requests, which works on every provider) and per-speaker voices over a
  dialogue transcript; OS TTS.
- **Voice input (STT)** — **idea**; the other half of "voice", and nothing
  exists yet.

### Localization

- **Hot-reload of external locales** — **deferred**. Editing
  `data/locales/*.json` applies on restart (the registry is `&'static`-leaked,
  a repeat `init` is ignored); reloading live needs a different ownership model
  ([i18n-external-locales.md](history/i18n-external-locales.md)).

### First run, setup and distribution

- **A first-run wizard in the TUI** — **idea**, a track of its own: provider →
  key → model, with `mindfork setup` as what it would call
  ([public-release-readiness.md §3.4](research/public-release-readiness.md)
  F2(b), [cloud-provisioning.md §9](research/cloud-provisioning.md)).
- **Windows code signing** — **blocked on reputation**. Designed end to end
  ([code-signing.md](research/code-signing.md): SignPath Foundation, the
  metadata fixed in stages 2–5, the site's policy pages), and the site's policy
  page says "not signed yet". The application (stage 7) is gated on SignPath's
  mandatory *Reputation* field, not on our readiness — the order decided on
  2026-09-02 (F2 there) is public repository → a public release → an article →
  stars, downloads and discussion accumulate → apply; weeks, not days. Stage 8
  is then the two signing requests in `release.yml`. The site's IP allowlist
  (stage 6) came off on the day of the first public release, so nothing on our
  side stands before the application. Countable places that feed the field:
  crates.io (done), an AUR package and a winget manifest — the next two items.
- **winget manifest** — **deferred**: pointing at the portable zip until the
  setup executable is signed ([installers.md §5.4](history/installers.md)).
- **AUR `mindfork-rs-bin`** — **on demand**; the release already ships an Arch
  package built by nfpm.
- **MSI for GPO/Intune** — **on demand**.
- **GPG signing of the Linux packages** — **idea**
  ([code-signing.md §6.4](research/code-signing.md), out of scope there).
- **Auto-update** — **idea**: self-update, musl-static and arm64 builds (the
  matrix is x86_64 glibc Linux and Windows; `install.sh` refuses anything
  else), a "new version is available" notice in the TUI. Two things to know
  first: [PRIVACY.md](../PRIVACY.md) promises no update check and no version
  ping, so a notice changes the policy; and on Linux re-running `install.sh`
  already upgrades in place. Groundwork from
  [release-engineering.md §5](history/release-engineering.md).
- **docs.rs shows no documentation** — **deferred**. The crate has no library
  target; the `documentation` field points at the manual, which is what the
  crates.io page links.
- **Small things from the public-release audit** — **on demand**
  ([public-release-readiness.md §2.4](research/public-release-readiness.md),
  "nice to have"): `NO_COLOR`; a "terminal too small" message; a port-in-use
  diagnosis for the managed server (only `setup --verify` refuses a busy port
  today); per-release debuginfo; the commit hash in `--version`; a demo
  recording; `linguist-vendored` for the vendored grammars; a warning that
  restoring a stranger's backup restores its MCP commands; MCP stdout lines read
  unbounded; the site's CSP header and the Download button's contrast;
  `generation.rs`'s size as a contributor barrier.

### Website

- **A Russian version of the site** — **deferred**, on F4's own terms
  ([mindfork-io-website.md §8](research/mindfork-io-website.md), S4).
- **CloudFront's flat-rate plan with WAF** — **deferred** until traffic asks
  for it (§3.2 and §7 F1(b) there).

### Testing, CI and quality

- **Live `#[ignore]` smokes in CI.** The llama.cpp set runs on the rented gate
  by hand ([remote-e2e-hf.md](history/remote-e2e-hf.md)); a scheduled run is
  [decided against](#decided-against). **blocked** — the cloud-key smokes need
  their own credentials, which no workflow holds; the Python-sandbox and
  managed-server smokes need local assets and a child process of our own, so
  they cannot run remotely at all.
- **Remote gate: chat-free runs** — **on demand**. `run` always creates the
  L40S chat endpoint, even for a filter that only exercises the embedders
  (~$0.10 wasted per such iteration); the probe has `--embed-only`, the runner
  has `--no-embed` but no `--no-chat`. Minor — the full gate always needs chat.
- **Hot-path benchmarks** — **idea**. Feed rendering (markdown + syntect
  cache), line wrapping, brute-force memory cosine — a performance regression
  detector; there is no `benches/` yet.
- **`cyrillic_scan.py`: `in_test` is never unset** — **on demand**. It flips
  on the first test marker in a file (`#[cfg(test)]`, `mod tests`, `#[test]`,
  `#[tokio::test]`) and treats everything below as test code, where Cyrillic in
  code position is legitimate fixture data. That is how two production strings
  in `message_feed.rs` (a file with `#[cfg(test)]` test accessors mid-file)
  stayed Russian through the whole English-source migration; they are fixed,
  the scanner's blind spot is not. An attribute on a single item should not
  mean "the rest of the file is tests".
- **SonarQube Cloud — leftovers** — **deferred**. The gate is blocking
  (2026-08-05) and runs the custom "Sonar way without new-code coverage"
  (2026-08-06); the badge landed on 2026-09-19, once the project was public.
  Folding the coverage run into the Linux `test` job would buy wall clock only
  once `sonar` is on the critical path — measured, its median is ~4 min against
  the Windows job's ~6 (down from 19 before the fixture work). Also unreviewed:
  the project's **New Code definition**, which scopes the issue and duplication
  checks.
- **Coverage is measured but not enforced** — **deferred**. Dropping
  `new_coverage` from the gate closed a structural false alarm, at the price
  that a genuinely untested new feature can pass. Two things would restore
  enforcement without the false alarm, neither cheap enough to do on spec:
  running the `#[ignore]` live suite under instrumentation (it needs a real
  engine, so the honest home is the remote runner, not every PR), and a
  coverage view that separates "untestable here" from "untested" instead of
  folding both into one percentage.

## Decided against

Measured or reasoned through, and turned down. Each line says what would reopen
it; re-proposing one without that repeats a day of work whose answer is written
down.

- **A semantic index over the attached project** (2026-08-21, fork F4 of the
  code workspace, [code-workspace.md §7.9](history/code-workspace.md)). A probe
  indexed this repository and answered eight user-vocabulary questions beside
  `code_grep`; across 48 turns per arm the index could not be shown to improve
  correctness or reduce rounds, and the sign of the difference depended on how
  an unusable turn was counted. Not "never" — **not on this evidence**. Reopens
  with a judge-graded measurement at n≈80 per arm instead of a keyword grader,
  or a corpus with sparse comments, where `code_grep` has far less to match on.
- **Moving the volatile self-model block after the conversation** — the change
  the prompt-caching numbers argue for (2026-08-08, standing since 2026-07-03;
  [prompt-caching.md](research/prompt-caching.md)). Many models read data
  appended to a *user* message as part of the user's request, and that
  behavioural risk in the self-model track outweighs a bounded token saving.
  What stays possible without it is the "Prompt caching" item above.
- **`cargo-nextest`** (2026-08-05, measured). Slower than `cargo test` on
  Windows — 46.3 s against 37.4 s at full parallelism, 52.3 s against 45.4 s at
  the runner's four threads — because the crate's tests (1833 at the time) live
  in one executable and nextest runs each in its own process, which Windows
  charges for; it does nothing about compilation, the larger half of the Windows
  job, and its `--partition` sharding multiplies the compile cost that the cache
  warming had just bought back. Revisit only if the crate is ever split, or the
  suite grows enough that sharding beats a warm single build.
- **A scheduled live run of the `#[ignore]` suite** (R5a,
  [remote-e2e-hf.md](history/remote-e2e-hf.md)). Every run costs ~$1, and the
  non-hermetic smokes would flake unattended — which is how a nightly gate stops
  being read. The gate runs by hand, before the PR that needs it.
- **In-feed search on `/`** ([in-feed-search.md](history/in-feed-search.md)).
  Unimplementable: the chat's input box is always focused and `/` in an empty
  box is exactly how a command starts, so gating on empty input does not rescue
  it. `Ctrl+F` is the key.
- **Threading source ranges through the renderer**, so in-feed search would
  also match text the renderer transformed (fork S3(c), rejected 2026-07-30,
  [in-feed-search.md §5](history/in-feed-search.md)). Measured: 65 words of
  186 805 (0.035%), all LaTeX command names, mermaid syntax and markup tokens.
- **Narrowing the tool profile to the one tool under test in the compliance
  probes** ([lessons.md §9](lessons.md)). It looked like the "remove the
  alternative" rule and measured 7 failures in 20; the fix was the probes'
  wording (0 in 20 on both model families).
- **Echoing `reasoning_details` back to a gateway**
  ([openrouter-external.md](research/openrouter-external.md)). Measured: a
  garbage signature is a `400` through the gateway and direct, and sending no
  blocks never is — there is nothing to build.
- **A model download stream of our own** (fork F4 of the provisioning track,
  closed 2026-09-22, [cloud-provisioning.md](research/cloud-provisioning.md)).
  `hf download` brings a 34 GB pair in minutes where a `curl` of the `resolve`
  link took hours; a stream of ours would be that stream. install.md §3.4
  documents the command instead.
- **An outer encrypted container for backups** (ChaCha20-Poly1305 + Argon2id;
  fork F1 of [backup-password.md](history/backup-password.md)). It would hide
  the archive's file names and sizes and replace the zip format's fixed
  PBKDF2-HMAC-SHA1/1000, at the price of opening the archive in 7-Zip by hand.
  Reopens only if someone needs to hide *which* chats exist rather than what is
  in them.
- **Localized CLI subcommand and flag names.** Part of the protocol —
  translating `backup` into the interface language is bad practice, especially
  with an English source. Likewise **grammar-aware pluralization**: the plain
  `tf` is number-neutral by design, and real grammar is only needed on axis B
  if a language with complex plurals shows up.
- **`Retry-After` as an HTTP-date** — read as "no hint": no provider we speak
  to sends that form, so parsing it would be code with no caller
  ([cloud-retry-backoff.md](research/cloud-retry-backoff.md)).

## Closed

An index, newest first — one line per track, with the release that shipped it
and where its story is told. What a track left open is a bullet above, under
its area.

- **Raw `llama-server` arguments in managed settings** — unreleased
  (2026-09-26): an *Extra arguments* field in each managed section, the
  flags that break the app or arm the server refused, the child's `LLAMA_*`
  side door closed, and a refused launch said in llama.cpp's words.
  [managed-extra-args.md](research/managed-extra-args.md), spec §3.4.
- **One command from a bare GPU box to a chat** — 0.11.0–0.11.1 (2026-09-21/22):
  `mindfork setup`, `install.sh` as an attested release asset, and the README's
  line run on a RunPod pod end to end (`ready in 4 s — context 131072`).
  [cloud-provisioning.md](research/cloud-provisioning.md), install.md §1,
  §3.3–§3.4.
- **`mindfork stats` and `--compare`** — 0.10.2 (2026-09-20): a read-only
  summary of the live data or a backup archive, and a comparison by message
  ids. [data-stats.md](history/data-stats.md), spec §12.4.
- **Publishing to crates.io as `mindfork`** — 0.10.1 (2026-09-19; the current
  release is there too): the GitHub release publishes it (`crates-io.yml`),
  not a hand. [binary-rename.md §10](research/binary-rename.md).
- **Required checks** — 2026-09-19: the `main` ruleset requires one context,
  `CI gate`, which covers Lints, Tests and SonarQube Cloud (requiring the test
  jobs by name had made a docs-only PR unmergeable). journal/ci.md, "one
  context the branch ruleset can require".
- **The first public release** — 0.10.0 (2026-09-18), the flip on 2026-09-19,
  six stages: the first minute of a stranger's run, safe defaults for the file
  tools, the release pipeline, robustness and the shipped defaults, the model
  picker, the documents a stranger meets, and the flip itself (9 229 blobs and
  582 pull requests scanned for credentials, zero found).
  [public-release-readiness.md §5](research/public-release-readiness.md),
  [safe-defaults.md](research/safe-defaults.md),
  [release-pipeline.md](research/release-pipeline.md),
  [robustness-and-defaults.md](research/robustness-and-defaults.md),
  [model-picker.md](research/model-picker.md),
  [public-documents.md](research/public-documents.md).
- **Images on an engine that takes none** — 0.10.0, two stages: the gateway's
  catalogue says whether the model sees images, and a chat already holding one
  goes on working after a switch to such an engine.
  [gateway-vision-catalogue.md](research/gateway-vision-catalogue.md),
  [history-images-no-vision.md](research/history-images-no-vision.md), spec §9.10.
- **`external` against OpenRouter** — 0.10.0, a review and three tracks: the
  gateway's thoughts and its thinking switch, the context window and the
  honest sampling list from the catalogue, a tool's images re-homed and
  `/continue` gated by the routed vendor.
  [openrouter-external.md](research/openrouter-external.md),
  [gateway-capabilities.md](history/gateway-capabilities.md),
  [gateway-images-and-continue.md](history/gateway-images-and-continue.md),
  [gateway-thinking-switch.md](history/gateway-thinking-switch.md).
- **A reply the content filter stopped says so** — 0.10.0.
  [content-filter-finish.md](research/content-filter-finish.md), spec §6.4.
- **Sandbox file exchange** — 0.9.9–0.10.0, five stages: files into, out of
  and between `python_exec` calls. [sandbox-file-exchange.md](history/sandbox-file-exchange.md).
- **`/continue`** — 0.9.9: an interrupted reply resumes in place via assistant
  prefill where the provider allows it, and refuses with the route that works
  where not. [continue-generation.md](research/continue-generation.md).
- **Background sub-agents, the tasks screen and the silent tasks** — 0.9.9
  (2026-09-05…08): `start_subagent` and `start_dialogue`, the `F7`/`/tasks`
  screen, `F6` and `/tasks stop <kind>`, the app-wide session budget with its
  silent lane, the silent stream yielding to the turn, the batch on a CPU-only
  host. [background-subagents.md](research/background-subagents.md),
  [background-dialogues.md](research/background-dialogues.md),
  [tasks-screen.md](research/tasks-screen.md),
  [stop-silent-task.md](research/stop-silent-task.md),
  [tasks-stop-command.md](research/tasks-stop-command.md),
  [tasks-stop-all.md](research/tasks-stop-all.md),
  [silent-tasks-budget.md](research/silent-tasks-budget.md),
  [silent-preemption.md](research/silent-preemption.md),
  [cpu-batch.md](research/cpu-batch.md).
- **Parallel sub-agents, concurrent ordinary tools, admission by budget** —
  0.9.9 (2026-09-04): `sessions` per engine section and the session semaphore;
  the round's read-only calls run at once (`Tool::concurrent()`); every stream
  reserves its estimate against the unified KV pool.
  [parallel-subagents.md](research/parallel-subagents.md),
  [concurrent-tools.md](research/concurrent-tools.md),
  [admission-by-budget.md](research/admission-by-budget.md),
  [ADR 0010](decisions/0010-subagent-nested-turn.md),
  [ADR 0012](decisions/0012-concurrent-tool-calls.md).
- **The two-agent dialogue** — 0.9.9: `run_dialogue`, a directed scene as a
  scripted multi-context run. [two-agent-dialogue.md](research/two-agent-dialogue.md),
  [ADR 0011](decisions/0011-dialogue-directed-run.md), spec §9.13.
- **Images from MCP tools** — 0.9.6. [mcp-tool-images.md](research/mcp-tool-images.md).
- **Subagent chats** — 0.9.8 (2026-08-27), 8 PRs: `call_subagent` as a nested
  turn with the agent's tools, the transcript on the call's record, in the
  list, in search, and streamed while it runs.
  [subagent-chats.md](research/subagent-chats.md),
  [subagent-live.md](history/subagent-live.md).
- **Model-compliance probes vs the live gate** — the flake was in the probes'
  own wording, not in the mechanism; 0 in 20 on both model families, and no
  exclusion mechanism was built. journal/ci.md, [lessons.md §9](lessons.md).
- **Export a conversation to a file** — 0.9.7: `/export [md|json] [path]`.
  [chat-export-file.md](history/chat-export-file.md), spec §11.7.
- **OSC 52 — copying to the client's clipboard** — 0.9.7.
  [osc52-clipboard.md](history/osc52-clipboard.md), spec §11.7.
- **Command-only control** — 0.9.7, two stages: every chord has a typed route.
  [command-only-control.md](history/command-only-control.md), spec §11.7.
- **Navigable `chat://` references** — 0.9.7, two stages.
  [chat-uri-links.md](research/chat-uri-links.md), spec §9.11, §11.3.
- **The cross-chat search tools** (`chat_search`/`chat_read`) — 0.9.7.
  [cross-chat-search-tool.md](research/cross-chat-search-tool.md), spec §9.11.
- **The external server's key, and the model name it routes on** — 0.9.7 and
  0.9.9. [external-api-key.md](history/external-api-key.md),
  [external-model-name.md](research/external-model-name.md).
- **Images in a message** — 0.9.6 (2026-08-13), two stages plus clipboard paste
  and attach by URL. [multimodal-images.md](research/multimodal-images.md),
  [image-url-attach.md](research/image-url-attach.md), spec §9.10.
- **`fetch_url` — an address policy** — 0.9.6 (2026-08-13), and it covers
  `web_search`'s page fetches too.
  [fetch-url-address-policy.md](research/fetch-url-address-policy.md), spec §9.3.
- **Retry/backoff on cloud errors** — 0.9.6 (2026-08-12), two stages.
  [cloud-retry-backoff.md](research/cloud-retry-backoff.md), spec §6.8.
- **History compression / rolling summary** — 0.9.5 (2026-08-09), stages 0–3.
  [history-compression.md](research/history-compression.md), spec §6.7.
- **YouTube: what a video says and shows, and the words as a chat attachment**
  — 0.9.5, two stages. [youtube-integration.md](research/youtube-integration.md),
  [youtube-transcript.md](history/youtube-transcript.md), spec §9.9.
- **Page fidelity for `fetch_url`** — 0.9.5.
  [fetch-url-fidelity.md](history/fetch-url-fidelity.md), spec §9.3.1.
- **Password-protected backups** — 0.9.5.
  [backup-password.md](history/backup-password.md), spec §12.3.
- **Confirmation for dangerous tools** — 0.9.5.
  [tool-confirmation.md](history/tool-confirmation.md), spec §9.8.
- **The MCP server editor** — 0.9.5, two stages.
  [mcp-server-editor.md](history/mcp-server-editor.md), spec §9.6.
- **In-feed text search (`Ctrl+F`)** — 0.9.5.
  [in-feed-search.md](history/in-feed-search.md).
- **Chat content search** — 0.9.5, two stages, on the disposable `cache.db`.
  [chat-content-search.md](research/chat-content-search.md),
  [chat-search-stage2.md](history/chat-search-stage2.md).
- **Embedding-model change** — 0.9.5, stages 1–3: the canary vector,
  `/reindex`, similarity gates calibrated per model.
  [embedding-model-change-reindex.md](research/embedding-model-change-reindex.md),
  spec §9.3.4.
- **The remote live e2e gate** — stages 0–3: `tools/e2e_hf.py` rents the stack
  on HF Inference Endpoints and verifies the deletion.
  [remote-e2e-hf.md](history/remote-e2e-hf.md),
  [remote-e2e-gpu.md](research/remote-e2e-gpu.md).
- **The code workspace** — the project attached to a chat.
  [code-workspace.md](history/code-workspace.md), spec §9.12.
- **Message playback (TTS)** — 0.9.2–0.9.3 (2026-07-23): `/tts` on the OpenAI,
  Gemini and external engines. [tts.md §13](research/tts.md),
  [ADR 0009](decisions/0009-tts-speech-synthesis.md).
- **Earlier**: the English source ([english-source-migration.md](history/english-source-migration.md));
  API keys in settings ([ADR 0008](decisions/0008-api-key-storage.md),
  [api-key-storage.md](research/api-key-storage.md)); self-model consolidation
  ([self-model-consolidation.md](history/self-model-consolidation.md)); RAG
  sources and retrieval ([rag-sources-retrieval.md](history/rag-sources-retrieval.md));
  plugins / MCP host and the import format
  ([ADR 0007](decisions/0007-plugins-mcp-host-import-format.md),
  [import-format.md](import-format.md)); installers
  ([installers.md](history/installers.md)); release engineering
  ([release-engineering.md](history/release-engineering.md)); multilingualism
  ([i18n.md](history/i18n.md) and its neighbours); OS-locale language
  detection (`i18n::detect_os_language`); Mermaid rendering
  ([mermaid-ascii-rendering.md](research/mermaid-ascii-rendering.md)).
