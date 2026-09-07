# Possible Roadmap

> Live list of ideas. Implemented items don't come back here — track history
> lives in [docs/journal/](journal/) (the post-M9 log, split by subsystem) and
> [docs/history/](history/).
> A compact summary of what recently closed is at the end of the file.


### Rejected: a semantic index over the attached project

Measured and turned down on 2026-08-21 (fork F4 of the code-workspace track,
[docs/history/code-workspace.md](history/code-workspace.md) §7.9). A probe
indexed this repository and answered eight user-vocabulary questions beside
`code_grep`; across 48 turns per arm the index could not be shown to improve
correctness or reduce rounds, and the sign of the difference depends on how an
unusable turn is counted.

Not "never" — **not on this evidence**. What would change it: a judge-graded
measurement at n≈80 per arm instead of a keyword grader, or a corpus with sparse
comments, where `code_grep` has far less to match on. Either is a new probe.
Re-proposing it without one repeats a day of work whose answer is written down.

## Most valuable next
The unprioritized list below is an idea bank of equal weight; this section calls
out the highest-payoff tracks (real user pain / direct savings). **It is empty
right now** — the three that stood here have all been closed or settled, and the
next one is whatever the next round of use argues for.

**Multimodality (images)** left this list on 2026-08-13 — **track complete**,
both stages: `/image attach|remove|list` works against a local
`llama-server --mmproj` and all four clouds
([multimodal-images.md](research/multimodal-images.md), spec §9.10). What was
deliberately left open is below, under "Feed and chat UI" and "Tools".

**Retry/backoff on cloud errors** left this list on 2026-08-12 — **done**, both
stages (see "Recently closed").

**Prompt caching** left this list on 2026-08-08: it is researched and measured,
but the change the numbers argue for was rejected on behavioural grounds, so
what remains is a partial gain rather than the headline one
([prompt-caching.md](research/prompt-caching.md)).

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
- **History compression — groundwork** (the track itself is **done**, stages
  0–3, see "Recently closed";
  [history-compression.md](research/history-compression.md), spec §6.7):
  - a **semantic index over the folded range** — sub-decision S11 chose the
    full-text index the app already keeps, and recorded the embedding variant as
    the answer if a live run ever shows lexical misses;
  - **learning the window from the 400 body** — sub-decision S3, deferred as
    redundant with `/props`, since the body shape that carries `n_ctx` is
    llama.cpp's own;
  - `estimate_prompt_tokens` still ignores `req.tools`, which matters for the
    `~` figure though no longer for the trigger.
- **Prompt caching** — **researched and measured; implementation deferred**
  (user's decision 2026-08-08). Full contracts, three live measurements and the
  decision points: [prompt-caching.md](research/prompt-caching.md).
  In short: caching is automatic on llama.cpp, OpenAI and Gemini and **opt-in
  only** on Anthropic, which caches nothing today. The gain hinges on a stable
  prefix, and our volatile block sits at the end of `system` — measured, a
  change to it costs the whole conversation (llama.cpp 0% vs 99% reuse; OpenAI
  0% vs 81%; Anthropic a 2304-token write per turn vs none). Moving it after the
  conversation is what the numbers argue for and **what was rejected**: many
  models read data appended to a *user* message as part of the user's request,
  and that behavioural risk in the self-model track outweighs a bounded token
  saving. So the 2026-07-03 decision stands. **Still available and
  layout-independent, if revisited:** Anthropic breakpoints (the head alone is
  worth ~5k tokens a turn), the cache share in the token counter, and
  `prompt_cache_key`. The promising way to get the rest is to make the
  observation selection **stable** rather than to relocate the block
  (§5 F1(e)) — untested.
- **`cached_tokens` in the token counter** — show the cache-hit share. All four
  providers report it and we drop it (llama.cpp: `timings.cache_n`, present
  since 2025-09 and not gated by `include_usage`). Beyond the display, it is the
  **instrument**: a prefix broken by some future change is otherwise invisible.
  Note the accounting trap measured in
  [prompt-caching.md §3.3](research/prompt-caching.md): with caching on,
  Anthropic's `input_tokens` is only the *uncached remainder* — 13 tokens for a
  7.4k prompt — so the counter must sum it with the cache fields.

## Tools
- **Parallel sub-agents — groundwork** (the track itself is **done**, both
  stages, 2026-09-04: `sessions` per engine section and the session
  semaphore, `-np N --kv-unified`, the `parallel_slots` hint; the round's
  parallel group under `tools.subagent_parallel`, `TurnShared` immutable
  with the confirmation lock, the mirror keyed by run id, the description
  with the number — [docs/research/parallel-subagents.md](research/parallel-subagents.md),
  ADR 0010 amended). What the research left for later (its §8):
  - **background sub-agents** — a child that outlives its round, the parent
    notified in a later round (Claude Code's background agents): a
    "task notification" round shape and a place for a result that arrives
    after the turn ends. **Done, both stages** (2026-09-05: stage 1 —
    `start_subagent`, the run outside the turn, the notification and the
    wake, the app-wide session budget, stop/delete/quit; stage 2 — `F6`
    stops the run on its open transcript where `Esc` only goes back, and a
    chat whose run landed while it was closed is marked unread in the list;
    forks F1–F10 at their recommendations, F3 → no confirmations; go on
    Qwen 3.6 27B and Gemma 4 31B):
    [docs/research/background-subagents.md](research/background-subagents.md).
    **The tasks screen is done** (2026-09-06: `F7`/`/tasks`, every run
    across every chat with its position or outcome and the silent tasks
    below, [docs/research/tasks-screen.md](research/tasks-screen.md), every
    fork at its recommendation). **Stopping a silent task from it — done**
    (2026-09-07: `F6` on a running or waiting task row, a third outcome
    *cancelled* that touches neither the failure streak nor the spawn-time
    bookkeeping, one notice for a `/compact` the user typed,
    [docs/research/stop-silent-task.md](research/stop-silent-task.md), every
    fork at its recommendation). **The silent tasks under the app-wide
    budget are done** (2026-09-07: the budget's silent lane — one permit
    for the app's own requests over the same pool sum — and the pool known
    at one session, where a `llama-server` without `-np` runs four unified
    slots; measured before designing: the compaction roll beside a
    background run ended both at the default,
    [docs/research/silent-tasks-budget.md](research/silent-tasks-budget.md),
    every fork at its recommendation). **The silent stream yields to the
    turn — done** (2026-09-07: a silent reservation's child token,
    cancelled by an interactive waiter that would then fit and admitted
    ahead of the task's retry, the retry inside the task, at most three
    yields; measured first — a one-word turn waited 45.9 s behind the roll
    on the CPU build where a cancelled stream's room is free in 0.79 s,
    [docs/research/silent-preemption.md](research/silent-preemption.md),
    every fork at its recommendation). **The batch on a CPU-only host —
    done** (2026-09-07: the batch a cancel waits for is linear in `-b`,
    measured on five launch lines; `-b 256 -ub 256` at `-ngl 0` unless a
    number is typed, a GPU host's line untouched,
    [docs/research/cpu-batch.md](research/cpu-batch.md), every fork at its
    recommendation). **Background dialogues are
    done** (2026-09-06: `start_dialogue` behind the same switch, the
    director's brief snapshotted at the call, every dialogue stream priced by
    the session budget — which also closed a live defect, a foreground scene
    overlapping a background run — and the wake turn's muted re-ask;
    [docs/research/background-dialogues.md](research/background-dialogues.md),
    ADR 0011 amended);
  - **concurrent ordinary tools** — **done** (2026-09-04, see "Recently
    closed"; [docs/research/concurrent-tools.md](research/concurrent-tools.md),
    ADR 0012). What its §8 left for later:
    - **`web_search` in the marked set** — after a measured probe of three
      concurrent searches on the keyless chain, or per provider (Tavily
      first; F4 there);
    - **MCP tools on `readOnlyHint`** — only as a per-server opt-in the
      user types, never the server's word alone (§4.9 there);
    - **`youtube_watch` and `note_recall`** — the first once a marked tool
      with an effect has exercised the effect ordering, the second once the
      vector backfill leaves the read path or becomes a single flight;
  - **admission by budget** for the unified KV pool — **done** (2026-09-04:
    every stream reserves its calibrated prompt estimate plus its reply cap
    and waits for room; the collective failure reproduced and then
    unreachable on the CPU build —
    [docs/research/admission-by-budget.md](research/admission-by-budget.md)).
    What its §8 left for later: the external shape typed (a `pool` field for
    a split server); a "waiting for room" label on the chip; the background
    tasks under an app-wide budget; `--kv-unified-per-slot` as a managed
    option;
  - **the Gemma stack's slow prefill** (~50 tok/s on b10791 with the
    projector, against ~2200 tok/s for Qwen on the same line) — the Gemma
    line without `-mm` and one cold request would tell whether it is the
    projector or the model's path; not this track's, but it made the e2e
    set take 72 minutes. *Narrowed 2026-09-05:* the same model without
    the projector on a rented L40S (`server-cuda-b10795`) prefills at
    ~2 100 tok/s (research §3.7), so it is the projector or the Windows
    CUDA build, not the model's path; the LAN line without `-mm` is the
    one remaining check.
- **MCP host — groundwork** (core is **done**: spec §9.6,
  [ADR 0007](decisions/0007-plugins-mcp-host-import-format.md); double opt-in,
  TOFU pinning, statuses/descriptions in settings, and the **server editor**
  ([mcp-server-editor.md](history/mcp-server-editor.md)) — both stages, so a
  server is authored, given its tokens and imported from another client's config
  without leaving the window):
  - HTTP transport (currently stdio only).
  - resources/prompts (currently tools only).
  - `notifications/tools/list_changed` — live catalog re-listing.
  - deferred schemas ("tool search") — context budget with many servers.
  - per-server tool count ceiling.
  - server `instructions` → system prompt.
  - non-text result blocks: **images are done** (spec §9.10,
    [mcp-tool-images.md](research/mcp-tool-images.md)); audio and resource blocks
    are still placeholders, and nothing downstream can carry them yet.
  - WASM sandbox for untrusted tools.
  - localization of client wire errors (currently — a technical layer).
- **`fetch_url`: no address policy** left this list on 2026-08-13 — **done**, and
  it covers `web_search`'s page fetches too: the check lives inside the client's
  DNS resolver (so the approved address is the connected one), an IP literal is
  judged before the request, every redirect hop is re-checked, and
  `tools.web_allow_private` (off by default) is the way back in
  ([fetch-url-address-policy.md](research/fetch-url-address-policy.md), spec §9.3).

## Feed and chat UI
- **Syntax grammars: user overlay and more languages** — 22 grammars are now
  vendored ([plan](history/vendored-syntaxes.md)), which covers what a model
  usually tags a block with; the rest of the long tail (Fortran, COBOL, Vala,
  Haxe, …) is one manifest row plus one file each. **V** is blocked on
  licensing — its only grammar has none, so the label maps to Go for now. Deferred by the
  same plan (fork F5): letting a user drop a grammar into `data/syntaxes/`, the
  way external locales work — the overlay would have to parse and re-link at
  runtime, i.e. pay back the ~130 ms the build-time dump removes, so it needs
  its own measurement. Also unexplored: replacing the base bundle with the
  current `sublimehq/Packages` (198 syntaxes, newer versions of what we ship).
- **Per-message collapse/select/copy** — explicitly deferred past M3, noted
  "account for the mouse toggle `Ctrl+W`". Selecting a single message,
  copying a single block. Collapsing itself is done for the *whole feed*
  (`Ctrl+T` — "thoughts", `Ctrl+O` — tool calls, stored per chat, spec §11.3);
  what is still missing is a way to fold one *particular* block, which needs
  block selection first.
- **`cyrillic_scan.py`: `in_test` is never unset** — it flips on the first
  `#[cfg(test)]` in a file and treats everything below as test code, where
  Cyrillic in code position is legitimate fixture data. That is how two
  production strings in `message_feed.rs` (a file with `#[cfg(test)]` **test
  accessors** at line ~564) stayed Russian through the whole English-source
  migration; they are fixed, the scanner's blind spot is not. An attribute on a
  single item shouldn't mean "the rest of the file is tests".
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
- **Regeneration with variations** — not just `Ctrl+R` with the same request,
  but with different sampling / picking from several response variants.
- **Editing any (not just the last) message** with history branching.
- **`chat://` addresses — groundwork** (the track itself is **done**, both
  stages, see "Recently closed";
  [chat-uri-links.md](research/chat-uri-links.md), spec §9.11, §11.3):
  - **message- and page-level addresses** (`chat://<id>/p3`): `chat_search`
    already names the page, `HistoryView::locate` maps a page back to a message
    and `OpenChatAt` takes a message uuid — so a hit could open exactly where it
    matched. Left out until the address format has proved itself without a path
    component.
  - **a back-stack deeper than one step**: `Esc` after following a reference now
    returns to the conversation it was followed from (fork F6, reopened by use
    and shipped), but a chain A → B → C steps back to B and no further — the
    shape the search half has always had. A true stack would have to answer what
    an ordinary chat switch does to its *middle*, which nothing has asked for.
  - **an address split across two rendered rows is styled but not clickable**
    (`Ctrl+L` still follows it). Fixing it means carrying link identity through
    the wrap, which is a span-metadata problem ratatui gives no room for.
  - **other organs' schemes** (`note://`, `attachment://`): the same mechanism
    would serve them, deferred until there is a second live consumer — a URI
    vocabulary invented ahead of one is a vocabulary nobody speaks.
## Chat and profile management
- **Folders/tags for chats** — grouping in the list (`Esc`).
- **Pin important chats** at the top of the list.
- **Prompt templates / snippets** — quick inserts of frequently used system
  messages or seeds.
- **Widening what chat search indexes** — the index covers `message.text` only;
  "thoughts" and tool results are the obvious candidates. Since the index is
  disposable, that costs a rebuild rather than a migration. Ranking is the other
  open question: the search screen groups by chat precisely to sidestep it, and
  trigram's `bm25` is a weak (though measurably non-degenerate) proxy for
  relevance. Both would now also pay off twice: the assistant-facing
  `chat_search`/`chat_read` pair (spec §9.11,
  [cross-chat-search-tool.md](research/cross-chat-search-tool.md)) reads the
  same index and inherits the same limits. The search itself is done — see
  "Recently closed", [chat-content-search.md](research/chat-content-search.md)
  and [chat-search-stage2.md](history/chat-search-stage2.md).
- **`cache.db` for chat-list summaries** — the disposable cache introduced for
  content search is a natural home for other cheap-to-recompute state; the chat
  list is currently built by parsing every `chats/*.json` at startup. **Measured
  before building (2026-07-30), and the measurement postponed it.** The parse
  costs **36.5 ms in release** and ≈ 7 MB resident on the dev corpus (171 chats,
  1540 messages, 14 MB of JSON) — the 94 ms this entry used to quote is the
  *debug* figure. And the title understates the work: the parse feeds
  `Orchestrator.chats: Vec<Chat>`, which holds every chat's full history, so
  removing it means making the orchestrator **lazy** — a multi-stage track
  through the "sole owner of `Chat`" invariant, with one structural obstacle
  (`search.rs::group_hits`) and a clean way past it (store the message's position
  in the index). Cost and payoff both grow linearly: ×10 is ~365 ms and ~70 MB.
  **Revisit at ~1000 chats, or a release parse over ~300 ms.** Numbers and scope:
  [chat-content-search.md §9.1](research/chat-content-search.md).

## Engine and reliability
- **Provider bridges** — the "any OpenAI-compatible endpoint" pattern
  (external + LiteLLM/OpenRouter) is now documented in install.md §3.1, together
  with the key such a gateway needs
  ([external-api-key.md](history/external-api-key.md)), and the request now
  actually carries the configured model name, without which none of those
  gateways would route at all
  ([external-model-name.md](research/external-model-name.md)); what remains is the
  optional managed-custom-command (supervisor launches an arbitrary sidecar
  proxy) — on demand, and a **live run against a real multi-model endpoint**
  (`llama-server --router` or a LiteLLM container): the request body is pinned by
  a unit test, the gateway's side of it has only been read, not exercised. See
  [plugin research §6](research/plugin-system.md).
- **API keys: storage extensions** (ADR 0008) — an OS keychain as an
  additional `scheme`; UI management of other machines' entries ("forget this
  computer") — also where an explicit cleanup of MCP secrets orphaned by a rename
  or delete belongs. Two of this item's halves are **done**: the MCP `env` map
  (spec §9.6) and the **external server's key**
  ([external-api-key.md](history/external-api-key.md), spec §11.6 — addressed per
  slot rather than per provider, which is what ADR 0008 could not do).
- **Retry/backoff — groundwork** (the track itself is **done**, both stages, see
  "Recently closed"; [cloud-retry-backoff.md](research/cloud-retry-backoff.md)):
  - the same policy for the **other HTTP callers** (fork F9, recorded rather than
    built): embeddings above all — `/reindex` re-embeds hundreds of chunks and one
    `429` currently voids a batch — plus TTS and `youtube_watch`. Each has its own
    degradation story today, so none is broken; they simply do not recover.
  - **quota-vs-rate `429` discrimination** (F5): today a billing-flavoured `429`
    costs two short waits before the body is shown. A marker blacklist in the
    `OVERFLOW_MARKERS` shape would skip them; left out until the wasted seconds
    actually bite.
  - **`Retry-After` as an HTTP-date** is read as "no hint" — no provider we speak
    to sends that form, so parsing it would be code with no caller.
- **Configurable health-check cadence.** The monitor's intervals
  (`HEALTHY_POLL` 60 s / `RECHECK_POLL` 5 s), the failure streak (3) and the
  relaunch budget (≤3 per 5 min) are constants. Nothing has asked for them to be
  settings yet; if a setup appears where they're wrong (a very slow link, a
  deliberately flaky server), they'd go in the "Model" section next to the
  engine fields.
- **Raw `llama-server` arguments in managed settings.** `ManagedConfig` already
  carries an `extra_args` tail and `build_args` appends it, but the supervisor
  hands it an empty vector — there is no setting behind it, so managed mode can
  send only the flags that have a field of their own. Where that bites is a
  large MoE model: fitting one on a consumer GPU is `--n-cpu-moe`/`-ot` work,
  and the only route to those flags today is to start `llama-server` by hand and
  switch to external mode. The build is small (one text field, split by
  `shared::cmdline::split`, a restart on change, hidden behind the existing
  managed group); the cost is a field that can break a launch in ways no
  preflight can check, and whose failures arrive as the server's exit code. Left
  written down rather than built for that reason — the escape hatch exists and
  nobody has hit the wall yet. Noted while checking that a multi-file GGUF loads
  in managed mode (journal/engine.md, 2026-08-23).
- **On-the-fly model switching** without a full settings-section restart (a
  quick model selector right in the chat).
- **Multimodality — what the track left open** (both stages **done**, and
  clipboard paste since; see "Recently closed";
  [multimodal-images.md](research/multimodal-images.md) §5, spec §9.10):
  - **rendering images in the feed** (kitty/sixel/iTerm2, or a halfblock
    renderer over the decoded pixels) — today a sent image shows as a chip.
  **Attach by URL** left this list on 2026-08-13 — **done**: the bytes are
  downloaded client-side, so all five engines are served by one path and a dead
  link cannot break a stored conversation
  ([image-url-attach.md](research/image-url-attach.md), spec §9.10).
- **YouTube — groundwork** (stages 1 and 2 are **done**, see "Recently closed";
  [youtube-integration.md](research/youtube-integration.md),
  [youtube-transcript.md](history/youtube-transcript.md), spec §9.9):
  - **cross-chat caching** of an expensive watch (R8b). Within one chat a
    follow-up is already free — the result is in the history; only a *different*
    chat re-pays. `cache.db` is deliberately the wrong home: it is disposable by
    design and a video read is expensive to recompute;
  - a transcript path that needs **no cloud key at all** (R7) — the one user
    story stage 1 does not serve. Everything free is PoToken-gated, so this means
    a paid captions vendor or a `yt-dlp` sidecar;
  - **default-model rot**: `gemini-2.5-flash-lite` already 404s for new users, so
    whatever ships as the default eventually stops existing.

## Testing, CI, quality
- **Live `#[ignore]` smokes in CI** — the llama.cpp set is covered (see
  "Recently closed"). What is left is deliberate or out of reach: a *scheduled*
  run is **not** wired (R5a — every run costs ~$1, and the non-hermetic smokes
  would flake unattended, which is how a nightly gate stops being read); the
  cloud-key smokes need their own credentials; the Python-sandbox and
  managed-server ones need local assets and a child process of our own, so they
  cannot run remotely at all.
- **Remote gate: chat-free runs** — `run` always creates the L40S chat endpoint,
  even for a filter that only exercises the embedders (~$0.10 wasted per such
  iteration). The probe has `--embed-only`; the runner has no `--no-chat`.
  Minor, and the full gate always needs chat.
- **Hot-path benchmarks** — feed rendering (markdown+syntect cache), line
  wrapping, brute-force memory cosine — a performance regression detector.
- **cargo-nextest — evaluated and rejected** (2026-08-05, measured, so that it
  is not re-litigated from first principles). Locally on Windows it is *slower*
  than `cargo test`: 46.3s against 37.4s at full parallelism and 52.3s against
  45.4s at the runner's four threads. The reason is structural rather than
  incidental — this is a single binary crate whose 1833 tests live in one
  executable, and nextest runs each test in its own process, which Windows
  charges for. It also does nothing about compilation, which was the larger half
  of the Windows job. Its one genuinely attractive feature here is
  `--partition` sharding, but that multiplies the compile cost across shards,
  i.e. it trades away exactly what the cache warming just bought. Worth
  revisiting only if the crate is ever split, or if the suite grows enough that
  sharding beats a warm single build.
- **SonarQube Cloud leftovers** — the gate is blocking as of 2026-08-05
  (`sonar.qualitygate.wait`, see the journal) and since 2026-08-06 runs the
  custom **"Sonar way without new-code coverage"**, so what remains is smaller: a
  quality-gate badge in the README (a **private** project's badge needs a token
  to render for anonymous readers), and — if the measurements say the duplicated
  instrumented test run is the expensive half — folding the coverage run into the
  Linux `test` job instead of a job of its own. On that last one there is now a
  measurement: the whole `sonar` job runs in ~3.5 min against the Windows job's
  19, so it was never on the critical path and merging it would buy wall clock
  only once Windows drops below it. Also unreviewed: the project's
  **New Code definition**, which decides what the gate judges — less pressing now
  that coverage is not one of the conditions, but it still scopes the issue and
  duplication checks.
- **Coverage is measured but no longer enforced** — dropping `new_coverage` from
  the gate closed a structural false alarm, at the price that a genuinely
  untested new feature can pass. Two things would restore enforcement without
  bringing the false alarm back, neither cheap enough to do on spec: teaching CI
  to run the `#[ignore]` live suite under instrumentation (it needs a real
  engine, so the honest home is the remote HF runner, not every PR), and a
  coverage view that separates "untestable here" from "untested" instead of
  folding both into one percentage.
- **Make the checks *required*** — "blocking" currently stops at the job: a failed
  gate reddens `SonarQube Cloud`, but GitHub still lets a red pull request be
  merged, because that is branch protection's job and it is **unavailable for a
  private repository on the Free plan** (the API answers 403; noticed earlier
  while looking for required checks during the runner-minutes work). So today the
  gate is a signal plus discipline, not mechanical enforcement. On GitHub Pro or
  Team it is one setting — mark `SonarQube Cloud`, `Tests`, and `Lints` required.

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
- **Backup encryption — groundwork** (core is **done**, see "Recently closed"):
  the archive's **file names and sizes stay visible** and the key derivation is
  fixed by the zip format at PBKDF2-HMAC-SHA1/1000 — both are properties of
  WinZip AES, so improving either means an outer container of our own
  (ChaCha20-Poly1305 + Argon2id), which costs the ability to open the archive in
  7-Zip by hand. That trade was made deliberately, not by omission
  ([backup-password.md](history/backup-password.md) F1), and would only be worth
  revisiting if someone needs to hide *which* chats exist rather than what is in
  them. Also deferred: `MINDFORK_BACKUP_PASSWORD` for scripted/CI backups (F7) —
  trivial to add, left out so there is one fewer place a password can come from.
- **Installers — groundwork** (core is **done**,
  [docs/history/installers.md](history/installers.md): Windows Inno Setup +
  Linux nfpm, shipped with releases): Windows code signing — R8 was deferred
  there "until public launch", and is now **designed**
  ([research/code-signing.md](research/code-signing.md): SignPath Foundation,
  the metadata that has to be fixed first, and the two signing steps in
  `release.yml`), waiting only on the repository going public; winget manifest
  (portable-zip until signing — unblocked once the setup executable is signed);
  AUR `mindfork-rs-bin`; MSI for GPO/Intune on demand.
- **Publishing to crates.io as `mindfork`** — the short name is **still free**
  (checked 2026-07-26; the registry API 404s on it), and the "About" dialog
  (`F1`) already lists `crates.io/crates/mindfork` as the future home. The
  binary/command is already `mindfork`
  ([docs/research/binary-rename.md](research/binary-rename.md)), so what
  remains is the *package*: the crate name is the publish name, so claiming it
  means `name = "mindfork"` in `Cargo.toml` (the `[[bin]]` override then
  becomes redundant) — a metadata-only change now that every user-facing
  surface carries the brand. Until it is actually published, the URL stays
  informational: a `crates.io` version badge in the README or a
  `cargo install mindfork` line in `install.md` would be visibly broken.
- **Auto-update** — self-update, musl-static and arm64 builds, an "a new
  version is available" notice in the TUI. Groundwork from the finished
  "release engineering" track
  ([docs/history/release-engineering.md](history/release-engineering.md) §5);
  the release pipeline (tag → archives + sha256 + packages + installer on
  GitHub Releases) already exists.
- **Message playback (TTS)** — **the `feat/tts` stage is implemented**
  (2026-07-23, [research/tts.md §13](research/tts.md),
  [journal/tools.md](journal/tools.md)): the `/tts` command (`/tts N`/`all`/`stop`), a
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
  application isn't done. Note that it is no longer the *answer* to a host
  stealing keys — the typed routes are (see "Recently closed") — so this is
  now a convenience for power users rather than an accessibility gap.
- **A clipboard for JupyterLab's terminal** — the one host OSC 52 cannot serve
  (see "Recently closed"): it embeds xterm.js *without* `@xterm/addon-clipboard`,
  so the escape is dropped. Nothing the app can send fixes that; the routes left
  are upstream (ask JupyterLab to load the addon) or sideways (write the
  conversation to a file the user can open — which is the existing "Export chat
  to a file" item, and would serve this case too).
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
- **Concurrent ordinary tools** (complete, one PR): the round's consecutive
  read-only calls run at once — `Tool::concurrent()` (default off; the file,
  project, attachment, chat, history and introspection readers, and
  `fetch_url`), the loop's *segment* with results, records and effects in the
  model's order, `concurrent_calls` per engine section (1 local, 4 cloud) with
  a "Parallel tool calls" row beside `sessions`, and `fetch_url`'s summary
  under `sessions`. Every model emitted the calls unprompted, 26/26. Design:
  [docs/research/concurrent-tools.md](research/concurrent-tools.md),
  [ADR 0012](decisions/0012-concurrent-tool-calls.md). What stays open: its
  §8 — `web_search`, MCP `readOnlyHint`, `youtube_watch`/`note_recall` — as
  sub-bullets of the parallel sub-agents groundwork item above.
- **`/continue`** (complete, probe + 2 stages): an interrupted reply resumes
  in place via assistant prefill — managed/external (llama.cpp/vLLM, the
  server's echo stripped byte-exactly), Gemini, and Claude up to the 4.5
  generation (allowlist-by-version gate; thinking dropped and the prefill
  right-trimmed on the wire); OpenAI/Grok refuse with the route that works.
  `MessageFinish` records why every reply ended; a tool-result tail resumes
  the loop; a length-cut reply finally gets a note. Design and measurements:
  [docs/research/continue-generation.md](research/continue-generation.md).
  What stays open: nothing recorded — external non-llama.cpp/vLLM servers
  keep the documented-fields-plus-honest-note stance (research §10).
- **Subagent chats** (complete, 8 PRs): `call_subagent` as a nested turn with
  the agent's tools (ADR 0010), the transcript on the call's record, the
  migration of old calls, the transcript in the list, in search, titled at
  landing, visible while it runs, and a tool call's card that opens when the
  call starts. Design: [docs/research/subagent-chats.md](research/subagent-chats.md),
  stage 2: [docs/history/subagent-live.md](history/subagent-live.md). What
  stays open: a parent's JSON export not carrying its transcripts (the v1
  import document has no place for tool calls). **The two-agent dialogue is
  complete, both stages** (`run_dialogue`, spec §9.13,
  [ADR 0011](decisions/0011-dialogue-directed-run.md), design
  [two-agent-dialogue.md](research/two-agent-dialogue.md)); the subagent's
  token-by-token transcript streaming, which this list still carried as open,
  had in fact shipped with the live track's addendum (subagent-live.md §8,
  spec §9.3.2) — corrected here rather than left to rot.
- **Model-compliance probes vs the gate** (complete): opened when
  `rewrite_tool_e2e_live` took down a Gemma dispatch of the live gate, and
  expected to need a split — a deterministic mechanism test for the gate, the
  compliance probe kept manual. Neither was needed. The mechanism already had
  strictly stronger deterministic coverage (`rewrite_tool_discards_partial_and_
  saves_it`, on a `MockBackend`), and the flake was in **the test's own wording**:
  asking the model to demonstrate the tool "strictly by steps" invited it to
  narrate the call as prose instead of making it. The two families then turned
  out to flake for two *different* reasons — Gemma by narrating (fixed by a
  closed instruction, 1 in 8 → 0 in 30), Qwen by producing nothing at all, the
  whole turn spent thinking (fixed by muting thinking, 1 in 20 → 0 in 20).
  Final configuration: **0 in 20 on each family**. Narrowing the profile to the
  one tool under test was tried and *rejected* — it looked like the "remove the
  alternative" rule and measured 7 failures in 20. No exclusion mechanism, no
  "manual probe" convention, and the end-to-end path stays in the gate.
  `control_tools_are_callable` also stopped showing the model
  `rewrite_current_message`'s schema without ever checking it. See
  [docs/lessons.md](lessons.md) §9.
- **Export a conversation to a file** (complete): `/export [md|json] [path]`.
  Markdown is byte-for-byte what `F5` copies — the user's call, and a good one:
  the content is Markdown already because that is how models write, so there is
  one formatter and no way for the file and the clipboard to drift. JSON is the
  `mindfork-import` v1 document the app already reads, with explicit ids, so an
  export imports back onto the same chat; it carries no tool calls and every
  such export says so. Paths (and the generated `<date>-<slug>` name) resolve
  against the current directory, and an existing file is refused. This is also
  the route out of JupyterLab's terminal, which the entry below could not serve.
  See [chat-export-file.md](history/chat-export-file.md), spec §11.7.
- **OSC 52 — copying to the client's clipboard** (complete): over SSH `arboard`
  wrote the server's clipboard, and on a headless server it failed outright, so
  `/copy` there produced only an error. A copy now also goes to the terminal's
  own clipboard, automatically when the session looks remote
  (`interface.clipboard_osc52`: `auto`/`always`/`off`). The research corrected
  the item's own premise: **JupyterLab drops OSC 52** (xterm.js without the
  clipboard addon), while VS Code supports it but usually runs the pty locally —
  so the beneficiary is plain SSH, which is also where the old behaviour was
  worst. The protocol acknowledges nothing and its support cannot be queried, so
  the note says the text was *sent*, and a conversation past the 74 994-byte
  ceiling is refused with an explanation rather than silently halved. tmux gets
  DCS passthrough; `screen` does not. See
  [osc52-clipboard.md](history/osc52-clipboard.md), spec §11.7.
- **Command-only control** (stages 1–2, complete): the app is now operable
  without a single chord, for terminals embedded in a host that claims them.
  The item did not exist on this list — it came from the question "could the app
  be driven by commands alone?", and the survey answered it with a measurement:
  VS Code's default `commandsToSkipShell` takes `Ctrl+P`, `Ctrl+E`, `Ctrl+F`,
  `Ctrl+K`, `F1`, `F3`, `F5` and both quit keys, so six features were already
  unreachable there, while a browser tab reserves `Ctrl+N`/`Ctrl+T`/`Ctrl+W` —
  the last of which closes the session's own tab. Nineteen commands close the
  class; each reaches its action through the **same handler its chord uses**, so
  there is one behaviour behind two routes, and each answers when a precondition
  blocks it, where a key is simply silent. The registry is one table (copied
  parsers would have been the duplication seam again) and the help tab derives
  its rows from it. What made the scope small was noticing that typing itself
  survives every host, and that every `Esc` chain already ends at the chat
  screen: safe keys navigate, commands act. Stage 2 then took the two actions
  that lived inside *other* screens — `/profile list|new|delete` and
  `/self clear` — and is the one place the track breaks parity on purpose: both
  always confirm, because a typed prefix can name a profile the user did not
  picture where the screen would have shown it selected. Verified live in VS
  Code's integrated terminal. See
  [command-only-control.md](history/command-only-control.md), spec §11.7.
- **Navigable `chat://` references** (stages 1–2, complete): a conversation now
  has **one address**, and the assistant knows it. The item asked only for the
  second half — recognize a reference and resolve it — and the survey moved its
  centre of gravity: nothing in the repository minted or mentioned the scheme, so
  the feature rested on one model's habit (grok-4.6 invented `chat://` out of the
  bracketed address the tools printed). So the tools now print `chat://<id>`,
  their descriptions ask the model to cite it **when it mentions a conversation
  to the user**, and `chat_read` reads it back — which fixed a latent defect on
  the way past: it used to fail its own scheme, because `resolve` stripped only
  `-` before the hex test. Live: a **local** gemma-4-31B cited the conversation
  it had read, unprompted beyond the descriptions — the habit is transferable
  once the format is actually taught. The feed styles only addresses that
  **resolve**, against the current profile's chats, so a link is never a dead end
  and the profile boundary needs no separate check; recognition runs in the block
  builder *before* the wrap, because an address is 15 columns and a narrow panel
  splits it where the post-render matching used by in-feed search would miss it.
  `Ctrl+L` follows a reference and — stage 2 — so does a click, the first
  clickable thing in the feed, from a map derived at render time for the viewport
  only; `Esc` retraces the step, through the same one-deep back-stack the search
  screen uses (fork F6, reopened by use once following a link and losing the way
  back read as a trap). See [chat-uri-links.md](research/chat-uri-links.md), spec §9.11, §11.3.
- **Images in a message** (stages 1–2, complete): `/image attach|remove|list`
  shows a picture to a local `llama-server --mmproj` and to all four clouds. The
  decision that shaped everything else was **not** copying `/file`: an image is
  staged for the *next message* rather than pinned to the chat, because that is
  what every provider's wire format models and because a chat-scoped set would
  rewrite the head of the prompt — measured, the append-only shape keeps
  llama.cpp's prefix cache across an image turn. A request carrying no images
  stays byte-identical to what the app sent before the feature, in all four
  formats, which is what makes the change safe for every existing conversation.
  Capability is *asked* (`/props` `modalities.vision`), never inferred from a
  model name, and an engine that cannot say is trusted rather than refused.
  Images are downscaled and normalized to png/jpeg once, at attach time, since
  xAI takes nothing else and an unscaled photo would ride every later turn.
  **Clipboard paste** followed (`/image paste`, plus `Ctrl+V` where the terminal
  forwards it): the app never had a `Ctrl+V` handler at all — pasting works
  because the *terminal* injects text, and an image injects nothing — so the
  command is the route that works everywhere and the key is the convenience. See
  [multimodal-images.md](research/multimodal-images.md), spec §9.10.
- **Retry/backoff on cloud errors** (stages 1–2, complete): a transient provider
  failure no longer costs the turn. The one-line roadmap item turned out to be
  **three** defects, two of them invisible, and stage 1 closed those first: a
  failure arriving *after* the answer started was silent — the partial reply stayed
  on screen with nothing marking it a fragment, and on Claude an in-stream
  `error` event (how a `529` arrives once a stream is accepted) was swallowed
  entirely, so an overloaded truncation was byte-for-byte a normal completion;
  the initial POST had no connect timeout and ignored `Esc` until the first bytes;
  and the status was thrown away, which is what made a retry unimplementable.
  Stage 2 added the retry itself as a **decorator** over the cloud and external
  backends — three attempts, ~1 s then ~2 s with downward jitter, `Retry-After`
  honoured as given up to 30 s and refused beyond it, the wait shown as a
  status-bar chip and interruptible by `Esc`. The safety property falls out of one
  rule rather than needing its own: a retry is allowed only while the turn is
  *uncommitted*, and since a tool call counts as content, a round that emitted
  calls can never be replayed — so no tool effect fires twice. Managed servers are
  deliberately left to the health monitor and relaunch budget, which is the honest
  recovery for a child that reloads for minutes. See
  [cloud-retry-backoff.md](research/cloud-retry-backoff.md), spec §6.8.
- **History compression / rolling summary** (stages 0–3, complete): a long
  conversation keeps fitting the model's context window. The older part is
  folded into a rolling summary — on request (`/compact`) and, once a turn's
  **exact** prompt reaches a share of the resolved window, by itself. The
  decision that dissolved most of the hard problems: compression changes what a
  *request* carries and never what the chat holds, so the feed, full-text
  search, the `F5` export and the reflection watermark needed no changes at all;
  the feed marks the boundary with a foldable divider. The window is resolved
  from an explicit setting, a managed server's `-c`, or the engine's own
  `/props`, and the trigger reads the server's exact `usage` because the byte
  estimate's error **changes sign** by content type (+68% on Russian prose,
  −20% on code) — unsafe on exactly the tool-heavy chats that overflow first.
  A summary is lossy, so `history_read` walks the folded range page by page and
  `history_search` says where to look; search runs over the full-text index the
  app already keeps rather than over embeddings, since an embedding server is
  separate and routinely unconfigured, and the local user with a small window is
  precisely who the track exists for. Along the way it uncovered that the client
  had been **discarding the server's exact token counts** on every turn —
  llama.cpp sends `usage` *after* the chunk carrying `finish_reason`, and the
  stream ended on that chunk. See
  [history-compression.md](research/history-compression.md), spec §6.7.
- **YouTube: the words, as a chat attachment** (stage 2, complete):
  `transcript: true` also brings back the spoken words with timestamps, in the
  **same** provider call as the description (the video is ingested either way, so
  a second call would double the expensive half) — the parameter says so, or the
  model treats the words as a cheap extra. A transcript past the per-file
  attachment budget lands as a **chat attachment** (spec §9.7), already paged and
  searchable, instead of going into the context whole; the threshold is the
  existing budget, which also makes such a transcript by-reference by
  construction. The stage's real work was the contract: `ChatEffect` gained a
  generic `AddAttachment`, and since effects only reach `Chat` when the turn ends
  while `ToolContext.attachments` is a turn snapshot, the loop mirrors the effect
  into that snapshot — otherwise "attached as X, read it with `attachment_read`"
  would be an instruction the turn could not carry out. Timestamps are corrected
  to be absolute: measured live, the same model numbered a clip from zero on one
  run and absolutely on the next, so the shift is decided rather than applied
  blindly. See [youtube-transcript.md](history/youtube-transcript.md), spec §9.9.
- **Page fidelity for `fetch_url`** (complete): a fetched page keeps its
  **headings and code blocks** (`extract_rich`, `fetch_url` only — `web_search`'s
  1500-character budget exists for ranking), and a page over the attachment
  budget becomes a **chat attachment** instead of being cut silently at 12 000
  characters mid-word. Along the way, two neighbours of the same defect class:
  `web_search` stopped reporting "no results" when a provider was actually
  serving a captcha behind HTTP 200, and `python_exec` now says that each call
  gets a fresh sandbox (nothing, `/tmp` included, survives a call). Specified by
  one real transcript in which a single page fetch turned into six `python_exec`
  rounds and 16 MB downloaded twice. See
  [fetch-url-fidelity.md](history/fetch-url-fidelity.md), spec §9.3.1.
  **Groundwork:** tables in rich extraction (needs a layout decision — Markdown
  table vs flattened rows); a persistent per-chat scratch directory for
  `python_exec` (its own lifecycle and cleanup questions).
- **YouTube: what a video says and shows** (stage 1, complete): `youtube_watch`
  describes a video's frames *and* audio with timestamps, narrowed by `focus`,
  and `start`/`end` clip a long one to a segment. It calls **Gemini out of band**
  rather than through the chat engine (the ADR 0009 shape), so it works on a
  local `llama-server` too — which matters because measurement, not preference,
  left one option: every free caption route is now behind YouTube's PoToken gate
  (a *signed* `timedtext` URL returns 200 with an **empty body**),
  `captions.download` needs the video owner's OAuth, and OpenAI and Anthropic
  take no video at all. Billed per second of footage (~103 tok/s at low detail,
  measured), so the tool refuses past a configurable ceiling and says how to ask
  for a segment; with no key it degrades to free metadata and states what is
  missing, and `fetch_url` stopped dead-ending on YouTube links. See
  [youtube-integration.md](research/youtube-integration.md), spec §9.9.
- **Password-protected backups** (complete): `--password` on `backup`/`restore`,
  or a password set once in settings → "Data", stored machine-bound exactly like
  a cloud API key (ADR 0008) — including for the copies the app makes itself
  before a restore or a data migration. The archive is standard AES-256, so
  7-Zip still opens it by hand; restore takes an encrypted **and** an
  unencrypted copy with nothing to switch (the zip layer discards a password an
  entry does not need), and a wrong one is refused before anything is replaced
  (the password is verified when an entry is opened, not after reading it). Both
  of those are measured properties, not assumptions. See
  [backup-password.md](history/backup-password.md), spec §12.3.
- **Confirmation for dangerous tools** (complete): `tools.confirm_dangerous` —
  off by default — parks a call to `python_exec`, `fs_write` or any MCP tool and
  shows it, formatted the way the feed will show it afterwards, before it runs;
  `Enter` runs it, `A` allows that tool for the rest of the turn, `Esc` declines
  without cancelling the turn (the model is told and carries on). What counts as
  dangerous is declared by the tool itself, next to its group and gate; MCP
  servers' own `destructiveHint` annotations are **not** consulted — untrusted
  input can only ever relax a decision, which is the attack. The load-bearing
  piece is the confirmation channel: the agentic loop is a background task, and
  this is the first thing that sends anything back *into* one, with replies from
  a stale turn or another call of the same round dropped. Turned off, the loop is
  byte-for-byte its old self. See
  [tool-confirmation.md](history/tool-confirmation.md), spec §9.8.
A compact summary (details — in [docs/journal/](journal/) and
[docs/history/](history/)):
- **In-feed text search** (complete): **`Ctrl+F`** inside a chat searches that
  conversation — every match highlighted, a `match n of total` counter,
  `Enter`/`↓` and `Shift+Enter`/`↑` to step, `Esc` to close; the message you are
  writing is untouched. **Not `/`, which this item asked for and which turned out
  to be unimplementable**: the chat's input box is always focused, and `/` in an
  empty box is exactly how a command starts (`/rag`, `/file`, …), so gating on
  empty input does not rescue it either. next/prev goes to the matched **line**,
  re-derived every frame — the largest real message is 38,782 characters, so
  jumping to the message start would leave the viewport unmoved. Matching runs
  over what is **drawn**, which is why the counter can equal the highlights;
  the same reason means it covers thoughts and tool cards, and misses text the
  renderer transformed — **measured and closed 2026-07-30**: 65 words of 186 805
  (0.035%), all of them LaTeX command names, mermaid syntax and markup tokens, so
  threading source ranges through the renderer (fork S3(c)) was rejected rather
  than deferred; the same measurement turned up the dropped-HTML defect and got it
  fixed ([in-feed-search.md §5](history/in-feed-search.md)).
  Prerequisite shipped with it: the highlight left
  `CacheKey`, so typing costs a warm frame (17 ms) instead of re-rendering the
  chat (39 ms). See [in-feed-search.md](history/in-feed-search.md).
- **Chat content search** (stages 1–2, complete): the chat list's search box
  toggles between titles and **message text** (`Ctrl+F`) and filters to the chats
  containing a match, with the existing sort still ordering them; from there
  `Ctrl+G` opens the **messages themselves** — grouped by chat, each with a
  snippet built in Rust with the match highlighted, its role and date — and
  `Enter` opens the chat *at* that message, which is marked in the feed **with the
  query highlighted inside it**, and `Esc` there comes back to the results whole —
  same selection and scroll — before going on to the chat list
  (`Enter` on a chat in content mode does the same, at its first match). The index
  is a separate, disposable `cache.db` — derived data, so a version mismatch or a
  corrupt file is answered by deleting and rebuilding rather than by ADR 0006's
  migration machinery, and the backup allowlist leaves it out with no code change.
  Trigram tokenizer, so matching is by fragment, like the title filter users
  already have — which also sidesteps Russian morphology, for which FTS5 has no
  stemmer; and every token is quoted, so ordinary text (`C++`, `cost-benefit`)
  searches for itself instead of raising an FTS5 syntax error. The jump is
  infrastructure in its own right: it is necessarily deferred into `render` (the
  block cache only exists there), it anchors to the *message* so it survives a
  resize, and it forced the seven "scroll to the bottom" sites to be split by who
  asked — so content arriving on its own no longer yanks a reader back. A live run
  revised one decision on the spot: fork S3 shipped as (b), post-render span
  matching, because a feed that didn't highlight what the results list did read as
  inconsistent — approximate by design, since it matches what was drawn rather than
  the source. **Still open**: in-feed `/`-search, which the jump *and* that
  highlight now make cheap — §Feed and chat UI.
  See [chat-content-search.md](research/chat-content-search.md)
  and [chat-search-stage2.md](history/chat-search-stage2.md).
- **Remote live e2e gate** (stages 0–3, complete): the mandatory live gate
  (AGENTS.md §3) stopped depending on one machine at one LAN address.
  `tools/e2e_hf.py` rents a real `llama-server` (HF Inference Endpoints'
  llama.cpp engine) plus **two** embedding models, runs the `#[ignore]` suite
  against them, and deletes them — **verifying** the deletion, since a leaked
  endpoint is the one outcome that costs money. In CI as a
  `workflow_dispatch` job with a scheduled sweeper as the backstop; two
  consecutive green runs (66 passed / 0 failed). The platform was chosen for
  its *cleanup guarantee* rather than its price — idle scale-to-zero is a
  dead-man's switch no rented pod offers. See
  [remote-e2e-hf.md](history/remote-e2e-hf.md), research
  [remote-e2e-gpu.md](research/remote-e2e-gpu.md).
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
