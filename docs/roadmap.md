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
4. **Multimodality (images)** (§Engine and reliability) — a big feature with
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
  - non-text result blocks (currently — a placeholder).
  - WASM sandbox for untrusted tools.
  - localization of client wire errors (currently — a technical layer).

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

## Chat and profile management
- **Folders/tags for chats** — grouping in the list (`Esc`).
- **Export chat to a file** (Markdown/JSON) — currently there's only
  clipboard copy (`F5`); add saving to disk.
- **Pin important chats** at the top of the list.
- **Prompt templates / snippets** — quick inserts of frequently used system
  messages or seeds.
- **Widening what chat search indexes** — the index covers `message.text` only;
  "thoughts" and tool results are the obvious candidates. Since the index is
  disposable, that costs a rebuild rather than a migration. Ranking is the other
  open question: the search screen groups by chat precisely to sidestep it, and
  trigram's `bm25` is a weak (though measurably non-degenerate) proxy for
  relevance. The search itself is done — see "Recently closed",
  [chat-content-search.md](research/chat-content-search.md) and
  [chat-search-stage2.md](history/chat-search-stage2.md).
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
- **Provider bridges** — document in install.md the "any OpenAI-compatible
  endpoint" pattern (external + LiteLLM/OpenRouter); optional
  managed-custom-command (supervisor launches an arbitrary sidecar proxy) —
  on demand. See [plugin research §6](research/plugin-system.md).
- **API keys: storage extensions** (ADR 0008) — an OS keychain as an
  additional `scheme`; the same mechanism for the external proxy's key (the MCP
  `env` map half is **done**, spec §9.6); UI management of other machines'
  entries ("forget this computer") — also where an explicit cleanup of MCP
  secrets orphaned by a rename or delete belongs.
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
- **SonarQube Cloud leftovers** — the gate is blocking as of 2026-08-05
  (`sonar.qualitygate.wait`, see the journal), so what remains is smaller: a
  quality-gate badge in the README (a **private** project's badge needs a token
  to render for anonymous readers), and — if the measurements say the duplicated
  instrumented test run is the expensive half — folding the coverage run into the
  Linux `test` job instead of a job of its own. Also unreviewed: the project's
  **New Code definition**, which now decides what the gate judges.

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
A compact summary (details — in [CLAUDE.md](../CLAUDE.md) and
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
  `workflow_dispatch` job with an hourly sweeper as the backstop; two
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
