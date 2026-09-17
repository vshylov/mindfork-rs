# Research: readiness for the first public release

**Status:** audit done (§2), 2026-09-17. Stage 1 (§3) forks decided by the user on
2026-09-17 — F1(a), F2(a), F4(a) at the recommendation, **F3(b) against it** — and
stage 1 implemented. Later stages (§4) listed with their scope, each to get its own
forks before it starts.

The repository is private at 0.9.9, with ten releases on GitHub and a public
website. Making it public changes three things at once: strangers run the app with
its **defaults** and no one beside them, the **repository and its history** become
readable, and **CI** starts receiving pull requests from forks. This document is the
result of one read-only audit of all three, and the plan that follows from it.

**Method.** Six parallel read-only audits — repository hygiene and git history;
public-facing documents; CI, release pipeline and supply chain; security of the
shipped defaults; first-run experience and code robustness; website and branding —
plus the SonarQube Cloud state of `main`. The claims that carry a blocker were
re-read in the code before being written here; the rest is tagged by how it was
established: **[code]** read in the repository, **[measured]** run on this machine,
**[gh]** read from the GitHub API, **[unverified]** a finding an audit could not
confirm.

**Related:** AGENTS.md §6 (release process),
[release-engineering.md](../history/release-engineering.md),
[installers.md](../history/installers.md), [code-signing.md](code-signing.md)
(signing waits on the flip), [mindfork-io-website.md](mindfork-io-website.md),
SECURITY.md, PRIVACY.md, ADR 0005, ADR 0008.

## 1. Where the project stands

- **Code quality is not the problem.** Sonar on `main`: quality gate OK, 0 open
  issues, 0 hotspots to review, coverage 87.1 %, duplication 0.8 % over 156.7 k lines.
  No `TODO`/`FIXME` in `src/`, no crate-level `#![allow]`, `en`/`ru` locales at full
  key parity (2047 keys) under test.
- **No secrets anywhere in history** [code]: every key shape (`sk-`, `sk-ant-`,
  `AIza`, `xai-`, `hf_`, `ghp_`, `tvly-`, AWS ids, private-key blocks, JWTs) searched
  with `git log --all -G`; every hit is a test placeholder.
- **The work is around the flip itself**, the first minute of a stranger's run, and
  two defaults that are safe on a pristine install but not one switch later.

## 2. Findings

Numbering of the blockers follows the summary the user was given, so "B8" means the
same thing in the conversation and here.

### 2.1 Blockers — the flip itself (owner's actions, no code)

- **B1. Order of operations.** mindfork.io is public (`robots.txt` allows) while the
  repository is private [gh], so every GitHub link on the site — the Download button
  (`site/templates/index.html:14`), nav/footer, ~35 links in content — is a 404 for an
  anonymous visitor. Flip first, confirm `/releases` answers 200 anonymously, then
  announce.
- **B2. Releases v0.9.0–v0.9.8 ship the spellcheck dictionaries without their
  licence texts** [code][gh]: `dictionaries/licenses/` first appears on 2026-09-02
  (`4f37e3d1`), and en_GB is LGPL-3.0-or-later, ru_RU and en_US BSD-style — both
  require the notice to travel with a redistribution. Delete those releases' assets
  (the tags can stay).
- **B3. Repository settings the documents already assume** [gh]: private
  vulnerability reporting is off (`…/private-vulnerability-reporting` → 404) though it
  is SECURITY.md's only channel; no Dependabot alerts, no secret scanning or push
  protection, no immutable releases; branch protection and rulesets are unavailable
  on a private Free repository (403), so `Tests`/`Lints`/`SonarQube Cloud` cannot be
  required yet. On the flip: enable all of them, a `main` ruleset, a `v*` tag
  ruleset, and fork-PR workflow approval for outside collaborators.
- **B4. The crates.io name `mindfork` is unclaimed** [gh] while the About dialog
  links to it (`src/shared/credits.rs:31`, pinned by a test in `help_dialog.rs`) and
  PRIVACY.md mentions "a crates.io page". Once public, anyone can take the name the
  app points users at. Reserve it before the flip, or drop the link.
- **B5. The Sonar job fails every fork and Dependabot pull request** [code]:
  `.github/workflows/ci.yml:295` is guarded only by `docs_only`, and the step passes
  `secrets.SONAR_TOKEN` with `sonar.qualitygate.wait=true`; a fork run gets no
  secrets. Guard it on the head repository.
- **B6. A pushed `v*` tag publishes a live release with nothing checking it**
  [code]: `release.yml:215` runs `gh release create` without `--draft` and without
  checking the tag against `Cargo.toml`; and `nfpm`, which builds the shipped
  deb/rpm/Arch packages, comes from `deb [trusted=yes] https://repo.goreleaser.com/apt/`
  unpinned and unverified (`release.yml:75`, `packaging.yml:59`) while Inno Setup and
  Zola are hash- or attestation-pinned.
- **B7. Not audited here:** the bodies and comments of 566 pull requests and the
  Actions run logs also become public. Skim the e2e-live runs for the Hugging Face
  namespace before the flip.

### 2.2 Blockers — code and documents

- **B8. A launch without a terminal hangs** [code][measured]. Nothing checks
  `is_terminal()` before `ratatui::init()` (`src/app/runtime/mod.rs:300`); the only
  checks in `src/` serve the OSC 11 query and the password prompt. Measured on
  Windows from a shell with no TTY, the debug binary with stdin from `/dev/null` and
  stdout to a file: it switched to the alternate screen, wrote 4 677 bytes of escape
  sequences into the file, created `data/` next to itself, and ran until `timeout`
  killed it at 20 s (exit 124). README.md documents the hang instead of preventing it.
  `mindfork demo` goes through the same `launch_tui`.
- **B9. A stranger's first run is a dead end** [code]. There is no first-run guidance:
  the default engine is `Managed` with no binary → `ServerStatus::NotConfigured`
  (`src/app/supervisor.rs:461`); the empty feed still says *"Start a conversation —
  type a message below."* (`ui.feed.empty`), and sending answers *"LLM server is not
  configured"* (`ui.err.server.not_configured`, `engines.rs:386`) — which names
  neither the settings (`Ctrl+P` / `/settings` → Model/server) nor the two routes
  (a cloud key plus a model name; a local `llama-server` plus a GGUF) nor
  `mindfork demo`. The send path itself is already sound: the typed text is given back
  (`generation.rs:282-286`). In README.md `mindfork demo` sits 295 lines down, the
  cloud route never says a model name is required, and neither README nor install.md
  says where a GGUF comes from.
- **B10. One switch gives a model unconfirmed read and write of the whole disk**
  [code]. On a pristine install every dangerous tool is off (`config.rs:1168-1190`),
  so an injected page cannot even be fetched. But `fs_root` defaults to `None`,
  and `FsRoot::resolve` then returns the raw path (`features/tools/fs.rs:53-55`);
  `confirm_dangerous` defaults to `false` (`config.rs:1190`); profile toggles default
  on (`enabled_by_default()`), so the global switch is the only gate; nothing
  protects the app's own data directory, so `fs_write` can rewrite `settings.json`
  (an MCP server command, the managed `binary`) — code execution at the next launch;
  and `fs_read` → `fetch_url` exfiltration is never confirmed even with confirmation
  on, since only writers are marked `danger()`. That confirmation is off by default was
  a deliberate decision (tool-confirmation.md); what changes is the audience.
- **B11. PRIVACY.md is inaccurate, and the Windows installer shows it** [code]. Its
  §3 calls itself the complete list of what leaves the machine, but omits
  `mindfork llama backends|setup` (GitHub API + release downloads,
  `features/llama_setup.rs:34`), which also sends a `GITHUB_TOKEN` from the environment
  (`llama_setup.rs:357`) against §3.3's "No variable name is assumed"; it says
  "thirteen Python wheels" where the lock list has 34. The full line-by-line check
  is §3.3 of this document.
- **B12. The Windows binary imports `vcruntime140.dll`** [code]: no `crt-static`, and
  the installer carries no VC++ redistributable. Checked on the debug binary; no
  profile changes the linkage. A clean Windows without the runtime would fail to
  start. `-C target-feature=+crt-static` for the Windows release build, then the
  installer on a clean VM.

### 2.3 Should

- **Robustness.** A panic in a tokio task restores the terminal (the hook chains
  ratatui's) but the process lives on: the orchestrator's `JoinHandle` is dropped
  (`main.rs:304`) and the UI loop's `while let Ok(..) = evt_rx.try_recv()` reads a
  closed channel as "no events". Found independently by two audits. Quit on a
  disconnected channel.
- **Security, beyond B10.** `fetch_url` reads the body without a byte ceiling
  (`shared/http_text.rs:71`; `image_fetch.rs` already streams under one). The Python
  sandbox's network defaults on and is a bare `--net` (`sandbox.rs:754`), outside the
  address policy [unverified: what wasmer's `--net` permits]. On Linux the data root
  is created without 0700/0600, and the key derivation input (`/etc/machine-id` +
  `$USER`) is world-readable — ADR 0008 is honest that other-user protection is
  Windows-only. Child processes (MCP, workspace commands, local Python) inherit the
  full environment, API-key variables included.
- **Licences.** No third-party notices for 558 locked packages (12 MPL-2.0);
  `syntaxes/licenses/` (22 files) never reaches an archive. `cargo about` in
  `release.yml`, plus copying the grammar licences next to the dictionaries'.
- **Supply chain.** Actions pinned by mutable tag (including the one that receives
  `SONAR_TOKEN` and the one with `id-token: write`); no `dependabot.yml`; no build
  provenance attestation (free on a public repository); two `deny.toml` ignores
  (RUSTSEC-2026-0194/0195) now have a fix in `quick-xml` ≥ 0.41; `chacha20 0.10.1` in
  the lock file is yanked.
- **Windows install.** The installer does not put `mindfork` on `PATH`, while README
  and CLI hints tell users to run `mindfork llama setup`; a fatal startup error on a
  double-click vanishes with its console window; "already running" exits 0.
- **Documents.** No user manual and no `docs/` index (the only map is CLAUDE.md, an
  agent router); six loose plans at the top of `docs/`; README is 42 KB; stale lines
  (`install.md:1051` "Ctrl+C — quit", `install.md:721` "Only *.txt and *.md", test
  counts disagreeing in three places). CONTRIBUTING.md routes to AGENTS.md ("rules for
  AI agents") with no lighter path for a contributor without a GPU; no issue forms,
  no CODE_OF_CONDUCT, no repository topics. `Cargo.toml` lacks `description`,
  `keywords`, `categories`, `rust-version`, `include` (the package would be 26 MiB).
- **CHANGELOG and version.** `[Unreleased]` has `### Fixed` twice; the 0.9.9 section
  (53 KB) becomes the release body verbatim — the first public release wants a short
  highlights paragraph. Decide 0.10.0 vs 1.0.0: AGENTS.md §6's conditions for 1.0.0
  are met, and 1.0 turns "data survives updates" into a promise.
- **Website.** No install page (Download lands on seven unexplained assets, nothing
  about SmartScreen); the self-model — the flagship per the roadmap — is card 3 of 12;
  no `og:title`/`og:description`, no 1280×640 card; the one-line description differs
  in six places, two of them omitting Grok; no licence/disclaimer pages; no launch
  post; no community channel.
- **Defaults.** Settings hints suggest `gpt-4o`, `gemini-2.5-pro` (`en.json:1410`)
  [unverified: whether retired]; no model picker from `/v1/models`;
  `REFERENCE = Ru` (`i18n.rs:63`) makes an external locale fall back to Russian;
  `max_tokens: 2048` with thinking on may truncate reasoning models [unverified];
  `terminal_compat` is off on legacy conhost.
- **Repository leftovers.** `run_all_tests.bat` (a LAN address) and `to_main.bat`
  tracked at the root; `docs/ui-design/` (an unreferenced 850 KB design-tool export
  with a generated third-party runtime); `.claude/` ignored only by machine-local
  rules; six merged remote branches; the owner's Outlook address on 1160 commits
  (already published on purpose in PRIVACY.md — a decision, not a leak).

### 2.4 Nice to have

arm64 and musl builds; `NO_COLOR`; a "terminal too small" message; a port-in-use
diagnosis for the managed server; per-release debuginfo; the commit in `--version`;
a demo recording; `linguist-vendored` for `syntaxes/` and `dictionaries/`; a warning
that restoring a stranger's backup restores its MCP commands; MCP stdout lines read
unbounded; a CSP header and the Download button's 4.4:1 contrast on the site;
`generation.rs` at 5.5 k production lines as a barrier for contributors.

### 2.5 Verified solid

No `pull_request_target`, no self-hosted runners, secrets unreachable from fork
runs, the paid HF gate triggerable only by the owner, caches saved only from `main`;
all dependencies from crates.io, release builds `--locked`; `fetch_url`'s address
policy checked inside the DNS resolver and on every redirect hop; no shell anywhere
in production spawns; `/file open` behind an extension allowlist; zip-slip checked
before a restore touches anything; control characters stripped before the terminal
(ratatui's `set_stringn`), OSC 52 only from the user's own copy; API keys in headers,
never in URLs or logs; no telemetry and no update check; CLI help localized and
unknown flags exit 2; `link_check`, `doc_index_check`, `site_legal_pages --check`,
`wizard_rtf --check` all green.

## 3. Stage 1 — B8, B9, B11

The three blockers that are small, independent of the flip and of each other, and
cost a stranger their first minute.

### 3.1 B8 — refuse to start the TUI without a terminal

Before anything touches the disk — at the top of `real_main` for `Run` and `Demo`,
the way `--help` and `--version` already touch nothing — check the terminal, and on
failure print one localized line to stderr naming what the app needs and that
`mindfork --help` lists the commands that work without one, then exit.

**Why stdout** [code]: the screen is drawn to stdout, so a redirected stdout is the
case that turns into escape codes in a file (§2.2, measured). Keys do not come from
stdin: crossterm opens the console device itself (`CONIN$` on Windows, `/dev/tty` on
unix when stdin is not a terminal), so `echo | mindfork` works today and a stdin
check would refuse a working launch.

Tests: the decision as a pure function; the message key in both locales (the parity
gate covers it); no engine involved, so no live run — the headless measurement above
is re-run on the fixed binary and recorded.

**Implemented** [measured]: `refuses_tui_launch` at the top of `real_main`, key
`cli.tui.no_terminal`. The same headless run on the fixed binary: `mindfork` and
`mindfork demo` both exit 2 in about a second, 0 bytes on stdout, the reason on
stderr, no `data/` created; `mindfork llama installed` with redirected output still
exits 0 with its listing. The crossterm claim was read in the vendored sources
rather than assumed: `crossterm_winapi` 0.9.1 `Handle::current_in_handle` opens
`CONIN$`, and crossterm 0.29 `tty_fd` opens `/dev/tty` when stdin is not a tty.

### 3.2 B9 — guidance where a first run lands

When the chat server reports `NotConfigured` and the open chat is empty, the feed
shows the ways to start instead of *"Start a conversation"*: a cloud model through
settings (`Ctrl+P` or `/settings` → Model/server: a provider, an API key, a model
name); a local model (`mindfork llama setup` for llama.cpp, then the GGUF in the same
section); and `mindfork demo` to look around first. The send error gains the same
pointer. No flash for configured users [code]: the chat screen starts at
`Connecting` (`screens/chat/mod.rs:725`) and only a reported `NotConfigured` shows it.

README: `mindfork demo` onto the first screen, the cloud route saying a model name is
required, and where a GGUF comes from (Hugging Face, with the models the project's
own docker stack already fetches).

Pure UI plus documents — render tests over `TestBackend`, no engine, no live run; a
look in a real terminal by the owner before merge.

**Implemented** [code]: `MessageFeed::set_engine_missing`, threaded by
`ChatScreen::set_server_status`; keys `ui.feed.no_engine.*`, and
`ui.err.server.not_configured` reworded to name the settings. Tests: the feed with
and without the flag, a chat with messages showing no routes, and the screen
following a reported status (nothing at its starting `Connecting`).

**Found on the way, left for stage 4:** the local route's second step. With a build
downloaded, an empty binary field resolves to it (spec §3.4) and the managed server
starts **without `-m`** when no GGUF is set. A recent `llama-server` may read a
missing model as its router mode rather than an error [unverified], so what the user
then sees —
a server that is ready with nothing loaded, or the misleading "corrupt GGUF or out of
memory?" of `ui.err.managed.early_exit` — is unmeasured. Measure before changing the
message (lessons §3).

### 3.3 B11 — PRIVACY.md matches the code

A line-by-line check of PRIVACY.md against the code, then one pass over the text,
regenerating its two copies (`python tools/site_legal_pages.py`,
`python tools/wizard_rtf.py`) and the Russian translation if one exists.

**The check** [code] found 16 statements false, incomplete or stale. Each was
re-read in the code before the text changed:

| § | The policy said | The code does |
|---|---|---|
| 3 | "the complete list" | `mindfork llama backends` / `llama setup` (GitHub API, release and CUDA-runtime downloads) was absent → new §3.8 |
| 3.1 | "Cloud providers are never probed" | Grok runs on `OpenAiClient` (`supervisor.rs:599`) and receives `/props` and `/models` with the key, at engine setup and on an image attach |
| 3.1 | `/models` only when no model is named | asked whenever the catalogue feeds the window, vision or parameters |
| 3.1 | the engine mode you chose | also the impersonation engine, and the `MINDFORK_*` variables that replace the settings |
| 3.2 | notes, documents, attachments, search | also self-model traits, the `rag_search`/`note_recall`/`attachment_search` queries, the canary sentence and the calibration set |
| 3.3 | `fetch_url` sends ≤ 12 000 characters to summarize | the text itself when not summarized; a page over the budget is attached whole (≤ 400 000) |
| 3.3 | a "Gemini video key", "the video's page" | the Gemini provider's key or a named variable; the watch page, then oEmbed |
| 3.3 | "No variable name is assumed" | contradicted by `GITHUB_TOKEN` in `llama_setup.rs` → the read removed (F3(b)) |
| 3.5 | "a `file://`-style address" | only `http`/`https` are accepted (`image_fetch.rs`) |
| 3.6 | tokens passed as variables | the server also inherits the whole environment (no `env_clear`) |
| 3.7 | "thirteen Python wheels" | 34 (25 from `files.pythonhosted.org`, 9 from `pythonindex.wasix.org`); an unfinished setup can make `wasmer` pull `python/python` from the registry; local mode has the host's network |
| 3.8 | OSC 52 over a remote session | also when the local clipboard fails (`osc52.rs:63`) |
| 2 | the storage table | `files/` also holds attached originals; `llama/` missing; each key entry is labelled with the computer's name |
| 4 | "on unless you turn them off" | compaction and tool images (`python_images`, `mcp_images`) missing |
| 6 | "No message text … is written" | a failed chat search logged its query → **the code fixed** (the length is logged); file names are logged; MCP stderr only at debug, non-protocol stdout at warn; files are `mindfork.log.YYYY-MM-DD` |
| 7, 8 | backup contents; the temp file of a Python run | backups also hold `files/`, `workspace/`, dictionaries, locales, `.bak` and an inner `fs_root`; a run gets `mindfork-sbx-<id>/` with the script and the named chat files, swept after 24 h, the interpreter given a path |

Everything else in §3 matched the code (managed on `127.0.0.1` only, the cloud hosts,
the web switch and its default, the keyless search chain and its headers, Tavily
without a default variable, the 800-character rerank, the address guard on
redirects, TTS on `/tts` only, stdio-only MCP, the pinned sandbox downloads, no
telemetry or update check, local spellcheck, the key encryption of §7).

Section numbers §3.3 and §4, which the code and other documents cite, are unchanged;
the clipboard moved from §3.8 to §3.9, cited nowhere.

### 3.4 Forks

**Decided by the user, 2026-09-17:** F1(a), F2(a), F3(b), F4(a).

**F1. What counts as "no terminal" (B8) — recommendation (a).**
(a) stdout is not a terminal → refuse, exit 2 (the code the CLI already uses for a
wrong invocation). Covers the measured case and refuses nothing that works.
(b) stdin **or** stdout is not a terminal. Stricter to explain, but refuses
`echo | mindfork`, which works.

**F2. The form of first-run guidance (B9) — recommendation (a).**
(a) The empty feed explains the routes while the chat server is `NotConfigured`,
plus an actionable send error. Small, disappears by itself once an engine is set,
touches no stored data.
(b) A first-run wizard screen (provider → key → model). The friendliest, but a new
screen with its own validation, i18n and tests — a track of its own, better after
the release on real feedback.
(c) Open settings on the Model/server section at the first launch. Puts the user in
the right place, but in a dense form with no explanation of what to choose.

**F3. `GITHUB_TOKEN` in `llama setup` (B11) — recommendation (a); chosen (b).**
(a) Keep reading it and document it: the standard convention (as `gh` and
`cargo-binstall` do), sent as `Authorization` only to GitHub — reqwest drops the
header on a cross-host redirect to the download CDN — and it lifts the anonymous
limit of 60 API requests an hour, which a shared address reaches.
(b) Stop reading it: PRIVACY.md's "no variable name is assumed" stays literally true,
and `llama setup` stays anonymous behind a shared address.

**F4. How stage 1 lands — recommendation (a).**
(a) One pull request for the stage: this document, B8, B9 and B11 — one design
behind all three, each small, and AGENTS.md §2 allows one PR per track stage.
(b) Three pull requests — `fix/` B8, `feat/` B9, `docs/` this document and B11.

## 4. Later stages

Each gets its forks written and confirmed before it starts.

| Stage | Scope | Why this order |
|---|---|---|
| 2 — safe defaults | B10; the fetch byte ceiling; the sandbox network; Linux 0700/0600; child environments — measured and forked in [safe-defaults.md](safe-defaults.md); 2a (the file tools) done | The largest real risk once strangers switch tools on; needs its own forks and a live run (tools) |
| 3 — release pipeline | B5, B6, B12; third-party notices and grammar licences; SHA pins, `dependabot.yml`, provenance; `quick-xml`, `chacha20` | All in `.github/` and `packaging/`, validated by `packaging.yml` and a tag rehearsal |
| 4 — robustness and defaults | the dead-orchestrator loop; installer `PATH`; fatal errors on a double-click; `REFERENCE = En`; model hints; the managed server with a build but no GGUF (§3.2, measure first) | Small independent fixes |
| 5 — public documents | user manual and `docs/` index; README trim; CONTRIBUTING's path without a GPU; issue forms, CODE_OF_CONDUCT; `Cargo.toml` metadata; CHANGELOG highlights and the version decision; the website's install page, hero, Open Graph, legal pages, launch post | Describes the result of stages 1–4, so it goes last |
| 6 — the flip (owner) | B4 → B2 → B7 → repository leftovers → flip → B3 → B1's anonymous check → announce | Irreversible and outward-facing; a checklist, not a PR |
