# Stage 5 — the documents a stranger meets

The last stage before the flip
([public-release-readiness.md](public-release-readiness.md) §4): *"user manual
and `docs/` index; README trim; CONTRIBUTING's path without a GPU; issue forms,
CODE_OF_CONDUCT; `Cargo.toml` metadata; CHANGELOG highlights and the version
decision; the website's install page, hero, Open Graph, legal pages, launch
post"*. It goes last because it describes the result of stages 1–4, all of which
are now merged and live-verified.

Everything here is prose, metadata and a static site: **no live run applies**
(§5), and the honest gates are the repository's own plus a real Zola build.

## 1. What a stranger actually meets today

Three doors, and all three were measured rather than remembered (§2): the
repository's front page, the website, and — for anyone who runs it — the app's
own `F1` help. The audit's §2.3 lists the gaps; some of them have closed since it
was written (CONTRIBUTING *does* now document a GPU-free path), and the ones that
have not are below.

## 2. Measurements

Taken 2026-09-18 on `main` at `b00ca6e7`, after stages 1–4 merged.

### 2.1 The repository's documents

| File | Size | State |
|---|---:|---|
| `README.md` | 43,386 B, 690 lines | 4 badges, 5 screenshots (dark/light pairs), 9 sections; **the status line still says 3217 unit tests / 176 live smokes** (it is 3323 / 196) |
| `CONTRIBUTING.md` | 5,054 B | already carries the GPU-free path (`docker compose up --build`, a CPU stack + JupyterLab terminal) and the "a real terminal is required" warning |
| `SECURITY.md` | 6,947 B | complete: private reporting, supported versions, scope, a 7-day acknowledgment |
| `CODE_OF_CONDUCT.md` | — | **absent** |
| `.github/ISSUE_TEMPLATE/` | — | **absent**; a `pull_request_template.md` and `dependabot.yml` exist |
| `docs/README.md` | — | **absent**; the only map is `CLAUDE.md`, which is an agent router by design |

README's weight, by section: Features 229 lines, Getting started 117, Keys and
commands 115, How it's built 64, Development 36, Documentation 23, License 27,
Project status 11, masthead 68.

`Cargo.toml`'s `[package]` carries `name`, `version`, `edition`, `license`,
`homepage`, `repository` — and **nothing else**: no `description`, `keywords`,
`categories`, `rust-version`, `readme`, `include`.

**The package that would be published** (`cargo package --list`, measured):
**700 files, 26.8 MiB, 9.0 MiB compressed** — under crates.io's 10 MiB limit with
no headroom, and it ships what nobody installing the crate needs: `docs/`
(6.06 MiB), `assets/` (3.91 MiB), `site/` (0.52 MiB), `packaging/`, `tools/`,
`docker/`, and `to_main.bat`. What a build actually needs is `src/` (8.2 MiB),
`locales/`, `syntaxes/`, `dictionaries/` (5.2 MiB), `build.rs`.

**Tracked leftovers**: `run_all_tests.bat` (carries a LAN address),
`to_main.bat`, and `docs/ui-design/` — 7 files, ~850 KB of a design-tool export
including a generated third-party runtime and a Cyrillic-named HTML file.

### 2.2 There is no user manual

Measured by looking for one rather than by assuming: no file under `docs/` is a
usage document. The manual-shaped content is in three places, none of them a
manual:

- **the binary** — `F1` opens About / Hotkeys / Commands / License / Legal /
  Components. A *reference*, and a good one; it explains no screen and no
  workflow;
- **README §"Keys and commands"** (115 lines) — three tables: ~24 hotkeys, ~17
  slash commands, ~18 command↔key equivalents. Also a reference;
- **`spec.md` §11** (1,832 lines) — the complete description of every screen, in
  the voice of an engineering spec written for implementers.

The *concepts* (self-model, RAG, the sandbox, the workspace, trust boundaries)
are covered by nine articles on the website. What is missing is the
task-oriented middle: what the screens are, and how a day with the app goes.

### 2.3 The website

19 content pages, English only, Zola 0.23.6, live behind an IP allowlist until
the repository is public.

- **The hero** has two buttons — `Download` → the GitHub releases page, and
  `Source on GitHub` — and a neofetch-style panel. It names no version, no
  platform requirements and no install page, **because there is no install
  page**: `site/content/` has no `install.md`. A visitor who clicks Download
  meets seven unexplained assets.
- **Open Graph is three tags**: `og:site_name`, `og:image` (the **256×256** app
  icon) and `twitter:card = summary`. No `og:title`, `og:description`,
  `og:type`, `og:url`, no `twitter:title`/`description`/`image`, no canonical,
  and **no 1200×630 card anywhere** under `site/static/`. A link posted anywhere
  social renders as a small square icon with the site-wide description.
- **The feature grid is 12 cards** in the order: Local first · Four cloud
  providers · Memory that persists · … — the **self-model, the flagship per the
  roadmap, is card 3**, and the article about it is weight 3 of 9.
- **Legal**: `/privacy/` (generated from `PRIVACY.md` by
  `tools/site_legal_pages.py`, gated by `--check` in CI) and
  `/code-signing-policy/` (hand-written, a SignPath condition). **No licence
  page, no disclaimer page** — though both texts exist at the repository root and
  are already shown on the `F1` → Legal tab and in the Windows installer.
- **One sentence, several spellings.** `zola.toml`'s description names all four
  clouds; `packaging/nfpm.yaml` names "OpenAI, Gemini, Anthropic" and **omits
  Grok**; `Cargo.toml` has no description at all.
- **No launch post.** The newest post is `0.9.9` (2026-09-13); the site's
  `[extra] app_version` is `0.9.9`.

### 2.4 The version, against the project's own rule

AGENTS.md §6: promotion to 1.0.0 comes *"once the track is proven in production
(CI green on both OSes + migration scaffolding merged + release pipeline has
shipped ≥1 release)"*. Two of the three hold today. The third does not: the
pipeline has been rehearsed twice (`v0.9.9-rc1`, `rc2` — both drafts, both
deleted) and has **never published a release**. The CHANGELOG's `[0.9.9]` section
is 53 KB; `[Unreleased]` is ~6 KB and is what the next release's notes would be.

## 3. Decisions taken without a fork

- **N1. The issue forms ask what CONTRIBUTING already says to include** — app
  version, OS **and terminal emulator**, engine mode and model, steps, the log
  under `logs/`. A bug form, a feature form, and a `config.yml` routing security
  to the private channel `SECURITY.md` names. One source for that list, so the
  form and the prose cannot drift.
- **N2. CODE_OF_CONDUCT is the Contributor Covenant 2.1**, unmodified, with the
  contact being the address already published on purpose (PRIVACY.md, and 1160
  commits carry it). An unmodified covenant is what a reader expects to skim and
  recognise; a bespoke one invites reading.
- **N3. `Cargo.toml` gains `description`, `keywords`, `categories`,
  `rust-version` (from `rust-toolchain.toml`), `readme` and an `include` list.**
  The include is the measured one: `src/`, `locales/`, `syntaxes/`,
  `dictionaries/`, `build.rs`, the licence and the readme — which is what a build
  needs and nothing else.
- **N4. One sentence describes the app**, and it is the one in `zola.toml`
  (it already names all four clouds). `Cargo.toml`, `nfpm.yaml` and the README
  masthead repeat it; the nfpm description stops omitting Grok.
- **N5. The licence and the disclaimer become site pages through the tool that
  already makes the privacy page** — `tools/site_legal_pages.py` with its
  `--check` gate, not hand-copied text that can drift from the root documents.
- **N6. The leftovers go now, not at the flip.** `run_all_tests.bat`,
  `to_main.bat` and `docs/ui-design/` are tracked files, so removing them is a
  pull request; stage 6 is a checklist of owner actions, and leaving code work in
  it would mean editing the repository between the checklist's steps.
- **N7. The stale numbers get a single source.** The README's status line is the
  only place outside `CLAUDE.md` that counts tests; it is corrected and the pair
  is stated once.
- **N8. The CHANGELOG's next section opens with a highlights paragraph** — four
  or five sentences a stranger can read, above the categorised list. The release
  body is that section verbatim (`tools/release_guard.py`), so the paragraph is
  what the release page opens with.

## 4. Forks

**All four decided by the user on 2026-09-18, each at the recommendation:**
F1 (a) 0.10.0, F2 (a) a manual in the repository, F3 (a) a deep trim, F4 (a)
everything on the site except the community channel.

### F1. The version of the first public release

- **(a) 0.10.0** — the project's own rule (§2.4) puts 1.0.0 *after* the pipeline
  has shipped a release, and this would be that release. It leaves room for a
  fast 0.10.1 when strangers' environments find what ours did not, and 1.0.0
  follows a few weeks later with the promise earned in public. *Recommended.*
- (b) **1.0.0** — the first artifact strangers install carries the strongest
  version number and, with it, the contractual promise that their data survives
  updates. Everything that promise rests on exists (schema versions with real
  migration steps, backups, two-OS CI); what is missing is only the evidence of a
  release having shipped.

### F2. The user manual

- **(a) A manual in the repository** — `docs/manual.md`, task-oriented: the
  first chat, the screens, profiles, memory and notes, the tools and what each
  switch opens, the workspace, settings worth knowing, troubleshooting. The
  README's three keymap tables move into it (which is also most of F3's trim),
  and `docs/README.md` becomes the human index `CLAUDE.md` deliberately is not.
  *Recommended.*
- (b) **A manual on the website** — the same content as a `/docs/` section, where
  a visitor who has not cloned anything can read it. Better placed for readers,
  worse for contributors, and it needs a section, templates and navigation the
  site does not have.
- (c) **No manual**: `docs/README.md` as an index, the keymap stays in the README,
  and `F1` plus the site's articles remain the whole story.

### F3. How far the README is trimmed

- **(a) Deep — to roughly 15 KB.** Masthead, screenshots, a short "what it is",
  quick start (install → connect a model → first chat), a compact feature list,
  and links out. Keys and commands → the manual; How it's built → the site's
  articles and `docs/architecture.md`; Development → CONTRIBUTING. *Recommended.*
- (b) **Moderate — to roughly 30 KB**: the keymap moves out, Features is
  tightened, everything else stays.
- (c) **Leave it.** 43 KB is long for a front page but every line of it is true
  and someone may prefer one document to four.

### F4. How much of the website is in this stage

- **(a) Everything the audit lists except the community channel** — the install
  page, the Open Graph tags and a 1200×630 card, the licence and disclaimer
  pages, the hero and feature order (the self-model first), one description
  everywhere, and a launch post left as a **draft** for the owner to edit and
  date. *Recommended.*
- (b) **Only what a first visit meets**: the install page and Open Graph. The
  legal pages, the reorder and the post come later.
- (c) **Nothing on the site this stage** — repository documents only.

## 5. Live run

**Not applicable, and stated rather than skipped** (AGENTS.md §3): this stage
touches no engine, memory or tool path. What replaces it:

- the repository's eight gates, `link_check` and `doc_index_check` in particular,
  since this stage moves documents between files;
- `site_legal_pages.py --check` for the pages generated from the root documents,
  extended to the two new ones;
- a real **`zola build`** of the site (the pinned 0.23.6) plus a look at the
  rendered install page and the Open Graph tags in the built HTML;
- `cargo package --list` to confirm the `include` list ships what a build needs
  and nothing else.

## 6. Plan

**Stage 5a — the repository's documents** (one pull request): the manual and the
`docs/` index, the README trim, CONTRIBUTING's first-time path, the issue forms
and CODE_OF_CONDUCT, `Cargo.toml`'s metadata and `include`, the leftovers, the
stale numbers, the CHANGELOG highlights and the version bump.

**Stage 5b — the website** (its own pull request): the install page, Open Graph
and the card, the legal pages through the existing generator, the hero and
feature order, one description everywhere, and the launch post as a draft.

**Stage 5c — the release pull request** (`chore/release-0.10.0`), last: the
version bump in `Cargo.toml`, `Cargo.lock` and `site/zola.toml`, `[Unreleased]`
renamed to `[0.10.0]` with the highlights paragraph on top, and the comparison
links. It is deliberately **not** folded into 5a: the checklist in AGENTS.md §6
wants that rename to happen when the section is final, and 5b still has entries
to add to it.

Splitting them keeps a review of prose separate from a review of a site build,
and lets 5a land while the site's page is still being written.

## 7. Outcome

**Stage 5a — done, 2026-09-18.** The manual ([manual.md](../manual.md)) and the
index ([docs/README.md](../README.md)); the README at **12,278 bytes**, down from
43,386; `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1) and the issue forms;
CONTRIBUTING's *"your first build, with nothing installed"*; `Cargo.toml`'s
metadata and `include` — the package is **367 files, 15.4 MiB, 3.8 MiB
compressed**, down from 700 / 26.8 / 9.0; the leftovers deleted; and the README's
test count corrected from 3217/176 to 3323/196.

**What the verification build caught**, and neither `--list` nor `--no-verify`
would have: `docs/legal/*` is `include_str!`-ed into the binary, so leaving it out
of `include` is a compile error rather than a missing document; and dropping
`assets/` ships a Windows binary **without its icon** (`build.rs` embeds
`assets/mindfork.ico`, and its absence is only a warning). Both are in the list,
and the trap is in [lessons.md](../lessons.md) §1.

**Stage 5b — done, 2026-09-18.** An install page (`/install/`) naming every
artifact, the SmartScreen warning, the checksum commands and what `mindfork demo`
is for, with the hero's primary button pointing at it; Open Graph as a
`{% block og %}` every template overrides, plus `og:url`, `og:image:*`, a
canonical link and `summary_large_image` over a **1200×630** card drawn by
`tools/og_card.py`; the licence and disclaimer generated into pages by the tool
that already made the privacy page (`legal.html` → `doc.html`, since an install
guide has no byline either); the self-model moved to card 1 of the feature grid;
and a launch post committed as a **draft** for the owner to date. Built with the
pinned Zola 0.23.6 — 19 pages, 0 orphans — and the rendered `<head>` read back.

**Stage 5c (the release pull request): pending.**
