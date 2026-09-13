# Journal — The mindfork.io website

The project website: stack research, the Zola site under `site/`, its design
system, and the AWS deployment (S3 + CloudFront) once it ships.

**Reference documents for this area:** [docs/research/mindfork-io-website.md](../research/mindfork-io-website.md), AGENTS.md §6

Entries record what was done, why, what was measured and what was rejected —
the reasoning behind the site, not its current shape. For the current shape
read the research/design doc above; for the traps that recur across areas
read [lessons.md](../lessons.md).

## Entries (15)

- Post-M9: website — research + S1 scaffold (Zola, terminal-styled) (done)
- Post-M9: website — S2 infra: one CloudFormation stack, mindfork.io live (done)
- Post-M9: website — S3 CI deploy (site.yml: PR gate + OIDC deploy) (done)
- Post-M9: website — S4: vector screenshots + the engine article (done)
- Post-M9: website — the maintenance IP allowlist (done)
- Post-M9: website — the hero fetch panel (done)
- Post-M9: website — what 0.9.6 changed on the landing page (done)
- Post-M9: website — two articles: the self-model and vector search (done)
- Post-M9: website — two more articles: local speed and the Python sandbox (done)
- Post-M9: website — the 0.9.7 release post (done)
- Post-M9: website — the 0.9.8 release post and the twelve-card landing (done)
- Post-M9: the site gets its legal pair — `/privacy/` and `/code-signing-policy/` (done)
- Post-M9: website — the 0.9.9 release post and three refreshed cards (done)
- Post-M9: website — the overview rebuilt, a screenshot shortcode and the trust-boundaries article (done)
- Post-M9: website — two more articles: the code workspace, and background runs on the context pool (done)

### Post-M9: website — research + S1 scaffold (Zola, terminal-styled) (done)

- **The first stage of the mindfork.io track** (research/design doc
  [docs/research/mindfork-io-website.md](../research/mindfork-io-website.md),
  six forks decided by the user 2026-08-10; branch `feat/website-scaffold`).
  Research verified against primary sources: hosting = **private S3 behind
  CloudFront (OAC) + ACM + Route53, one CloudFormation stack in us-east-1**
  — still the canonical answer in 2026 and ≈ $0.01/mo incremental for this
  site (CloudFront's 1 TB/mo always-free tier holds; Route53 alias queries
  are free). Amplify was rejected on numbers: ~$2.10/mo for a pre-2025
  account (its free tier no longer applies) plus a hand-rolled Zola build
  spec anyway. Generator = **Zola** (v0.23.2 three days old at review;
  Cobalt in maintenance-only, mdBook the wrong shape, Marmite too minimal,
  Astro rejected on Node toolchain weight in a pure-Rust repo).
- **Zola is pinned to 0.22.1 — measured, not preferred**: 0.23.0–0.23.2 do
  not discover `templates/` on Windows at all, reproduced on a pristine
  `zola init` site and tracked upstream as getzola/zola#3229 (fix merged,
  unreleased). Hence the config is named `config.toml` (the only name 0.22
  reads; 0.23 reads it as a fallback) and the templates are v1/v2-compatible
  Tera with no shortcodes (0.23 removed them) — the future bump is a version
  number, not a migration. The 0.22.1 file-watcher also never fires on
  Windows, so the dev loop is "edit → restart `zola serve`" — worth knowing
  before doubting one's own CSS.
- **S1 scaffold**: a custom theme on the brand system (assets/README.md) —
  dark/light with a toggle (localStorage, `prefers-color-scheme` fallback
  for no-JS; the wordmark and every screenshot swap per theme), JetBrains
  Mono woff2 self-hosted (Cyrillic subset included for the future `/ru/`),
  the generated screenshots framed in terminal-window chrome, a
  `$ mindfork▊` hero with a blinking block cursor, `# `-prefixed headings,
  pixel-square card bullets, a `cat: /404` error page; landing + blog
  (Atom feed, sitemap) + articles, with the first news post and the first
  article. No JS framework — the theme toggle is the site's only script.
- **Design review (live preview, 2026-08-10): approved with one
  amendment** — the terminal chrome originally wore macOS traffic-light
  dots; the app runs on Windows and Linux only, so the frames were reworked
  to right-aligned caption glyphs (`─ □ ✕`) to not suggest a platform the
  app does not support.
- **Repo integration**: brand assets under `site/static/` are derived data
  mirrored from `assets/` by `tools/site_sync_assets.py` (gitignored, so
  the screenshots' single source of truth stays rot-gated in `assets/`);
  `link_check.py` skips `site/` (its links live in Zola's URL space, which
  `zola build` checks itself); `cyrillic_scan.py` skips `.woff2` (binary
  fonts legitimately carry Cyrillic glyph bytes). Tests: **no Rust
  touched** — 1977 unit tests / 85 `#[ignore]` unchanged, all gates green;
  an engine live run does not apply (static output), and the stage's gate
  was the user reviewing the local preview.

### Post-M9: website — S2 infra: one CloudFormation stack, mindfork.io live (done)

- **One CloudFormation stack** (`infra/website.cfn.yaml`, us-east-1, stack
  `mindfork-website`; branch `feat/website-infra`) creates the whole serving
  path: a private S3 bucket (BPA, SSE, auto-generated name — a dotted name
  would break TLS between CloudFront and the regional S3 endpoint, since the
  wildcard cert covers a single label), CloudFront with OAC, the managed
  CachingOptimized and SecurityHeadersPolicy policies and custom 403/404 →
  `/404.html`, a viewer-request function (www→apex 301 + directory-index
  rewrite for Zola's trailing-slash URLs), an ACM apex+wildcard certificate
  DNS-validated into the zone by the stack itself, four Route53 aliases
  (A/AAAA × apex/www), and the GitHub OIDC provider plus the
  `mindfork-site-deploy` role trusted for
  `repo:vshylov/mindfork-rs:ref:refs/heads/main` only, allowed s3 sync into
  the site bucket plus one CloudFront invalidation and nothing else.
- **The first deploy failed and taught the template a lesson**: the apex and
  `*.apex` share the identical ACM validation CNAME, and listing both in
  `DomainValidationOptions` makes the handler write the same record twice —
  Route53 answers 400 "invalid set of changes for a resource record set" and
  the stack rolls back whole. One options entry (the apex, with
  `HostedZoneId`) validates both names; the reasoning is a comment next to
  the resource so the trap cannot be reintroduced silently.
- **First deploy by hand** (the CI workflow is the next stage):
  `zola build` → `aws s3 sync` — 29 files, 1.9 MiB; `.woff2` uploaded as
  `font/woff2` (the CLI's mimetype table is current). No invalidation needed
  on a fresh distribution.
- **Live checks, all green**: `https://mindfork.io/` 200; `/blog/` 200
  through the rewrite function; a missing page answers the site's own 404;
  `www` 301 → apex with the path preserved; http 301 → https; HSTS
  `max-age=31536000` from the managed headers policy; `/atom.xml` and
  `/sitemap.xml` 200. Operational outputs for the CI stage: bucket
  `mindfork-website-sitebucket-ioafb7vyycso`, distribution `E8EC9ICZSRYKB`,
  role `arn:aws:iam::976877302614:role/mindfork-site-deploy`.
- No Rust touched — 1977 unit tests / 85 `#[ignore]` unchanged, gates green;
  the stage's live run is the deployed site itself.

### Post-M9: website — S3 CI deploy (site.yml: PR gate + OIDC deploy) (done)

- **`.github/workflows/site.yml`** (branch `feat/website-ci`): pull requests
  touching `site/**`, `assets/**`, the asset-mirror script or the workflow
  itself get a **build gate** — pinned Zola 0.22.1
  (`taiki-e/install-action`, matching local dev until getzola/zola#3229
  ships), the asset mirror, `zola check --skip-external-links` (internal
  links only: external checking would put CI at the mercy of other people's
  servers), `zola build`. Push to main on the same paths, plus
  `workflow_dispatch` for a manual redeploy, runs the **deploy**: build →
  `configure-aws-credentials@v6` assumes `mindfork-site-deploy` through the
  job's OIDC token — no stored keys anywhere — then `aws s3 sync --delete`
  and one `/*` invalidation (a single path, inside the free 1000/month).
  The bucket and distribution ids are the stack's outputs, recorded as job
  env next to a comment pointing at `infra/website.cfn.yaml`. Superseded PR
  builds cancel each other; main deploys queue and never cancel mid-sync.
- **Proven end to end the same day**: the gate leg ran green on its own PR
  (#294 — the workflow sits in its own path filter), and the merge commit
  fired the deploy leg: run success, the deploy job took **15 s** (the gate
  job correctly skipped on push), and the site answered 200 after. What
  remains of the track is S4 — the screenshot SVG writer and content.
- No Rust touched — 1977 unit tests / 85 `#[ignore]` unchanged, gates
  green; the stage's live run is the deploy run itself.

### Post-M9: website — S4: vector screenshots + the engine article (done)

- **All ten screenshots on the landing are now inline SVG** (the writer
  itself — the release journal's "demo screenshots — stage 4" entry):
  Zola's `load_data(format="plain")` inlines both theme variants of each
  frame, and the pairing is pure CSS (`.only-dark`/`.only-light`), so
  the image-swap script is gone — the theme toggle is once again the
  site's only JavaScript. `index.html` grew to 133 KB raw (~30 KB over
  the wire — CloudFront compresses), buying zero image requests on the
  landing and text that stays crisp at any zoom; the PNGs remain for the
  README and as the byte-exact reference.
- **One cascade lesson, measured in the preview**: the visibility
  utilities lost to a later component rule — `.wordmark { display:block
  }` out-cascaded `.only-light { display:none }` at equal specificity,
  showing both wordmarks side by side. The utilities now carry
  `!important`, which is exactly the tool's intended use.
- JetBrainsMono-**Italic**.woff2 joined the site fonts (thinking blocks
  are italic); the asset mirror copies `*.svg` alongside `*.png`.
- **Second article** — "Why the engine is a server, not a library": the
  engine-contract story (embedded runtime tried and rejected,
  managed/external modes, cloud providers as sibling implementations,
  honest sampling, EOS by token id), sourced from ADR 0004 and CLAUDE.md.
  `ru` stays deferred on F4's own terms — content stabilizes in English
  first.
- No Rust touched — 1977 unit tests / 85 `#[ignore]` unchanged; the
  stage's check is the built site itself, verified in the local preview
  in both themes.

### Post-M9: website — the maintenance IP allowlist (done)

- **The site can now be closed to everyone but a listed viewer** while it
  is still being built, matching the repository being private: the stack
  gained an `AllowedIps` parameter (comma-separated, empty = public), and
  the existing viewer-request function refuses anything else with a 403
  and a short branded page. Branch `feat/site-ip-allowlist`.
- **A CloudFront Function, not a WAF web ACL** — decided on cost and on
  reversibility. WAF is ~$5/month per web ACL plus per-rule and
  per-request charges; this site's entire bill is ~$0.01/month (research
  §5.6), so the guard would have cost 500× the thing it guards. The
  function is a fraction of a cent per million invocations, it is the
  same resource that already does the www→apex redirect, and lifting the
  lockdown is `AllowedIps=""` — one parameter, no resource to delete.
- **Viewer-request, so caching is not a hole**: the gate runs before the
  cache lookup, which means an already-cached page is refused too and no
  invalidation is part of either direction of the switch.
- **IPv6 goes off while a list is set** (`IPV6Enabled: !If [LockedDown,
  false, true]`). The address the function compares is whichever family
  the browser actually connected over, so a dual-stack visitor holding an
  IPv4-only allowlist would 403 itself — the classic way to lock yourself
  out of your own maintenance page. Dropping AAAA from the distribution
  forces every viewer onto IPv4; the Route53 AAAA aliases can stay, they
  answer NODATA and clients fall back.
- **Verified before deploying, since a mistake here is a locked door**:
  the template was parsed and the function rendered through its `!Sub`
  (1412 bytes, well inside CloudFront's 10 KB limit), then the handler
  was exercised in both modes — allowed viewer passes and still gets the
  index rewrite and the www redirect; a stranger gets 403 on apex, on
  www and over IPv6; with `AllowedIps` empty every case behaves exactly
  as it did before the change.
- **Deployed and checked against the live edge** (2026-08-11, stack
  `mindfork-website` → `UPDATE_COMPLETE`, distribution `Deployed`, the
  four pre-existing parameters preserved by `deploy`'s use-previous
  behaviour). From the allowlisted address: `/`, `/blog/`, `/atom.xml`
  200 and `www` 301 → apex; `mindfork.io` no longer answers AAAA, as the
  IPv6 switch intends. The published LIVE function was then exercised
  through `aws cloudfront test-function` — the only way to put an
  arbitrary viewer IP in front of it: a stranger gets 403 with the
  branded body on apex and on www, while the allowlisted viewer still
  gets the index rewrite and the www redirect.
- No Rust touched — 1977 unit tests / 85 `#[ignore]` unchanged; the
  stage's live run is the stack deploy itself.

### Post-M9: website — the hero fetch panel (done)

- **The hero's top-right quarter was empty on wide screens** — the copy is
  capped (h1 800px, lede 690px) inside the 1100px wrap, so everything
  right of the headline down to the chat screenshot was blank. It now
  holds a neofetch-style **"at a glance" card** in the standard `.term`
  chrome (branch `feat/site-hero-fetch-panel`): a box-drawing rendition
  of the brand icon's topology (accent trunk, two `--branch` merges — the
  same two-tone fork as `assets/`) beside seven key→value rows: engine /
  cloud / memory / tools / ui / runs on / license. Every value restates a
  fact the features section already claims; the panel deliberately shows
  **no fake command line** (its window title is "mindfork — at a glance")
  — the only typed commands the site shows remain the real `$ mindfork`
  and `$ mindfork demo`. Pure HTML+CSS; the theme toggle stays the site's
  only JavaScript, and the same accent/branch hexes work in both palettes.
- **Layout**: `.hero-cols` is `grid-template-columns: minmax(0,1fr) auto`
  with the panel centered against the copy; the headline now wraps to
  three lines at full width, which reads as intended. Below 1024px the
  grid collapses and the panel is `display: none` — the single-column
  hero is untouched, and the panel's facts all exist in the features
  cards, so narrow viewports lose decoration, not information.
- **Box-drawing art needs `line-height: 1`.** At the panel's original
  1.45 the glyph boxes do not touch vertically, so the trunk and branches
  rendered with a gap per row — the art must set its own line-height and
  let the glyphs connect (same reason terminals draw box characters at
  cell height exactly).
- **A headless screenshot below ~500px wide lied** — see the lessons
  entry; the DOM (scrollWidth = viewport, panel `display:none`) is what
  proved the mobile layout unchanged.
- No Rust touched — 1977 unit tests / 85 `#[ignore]` green
  (`fmt`/`clippy`/`test` run for the record); an engine live run does not
  apply (static output). Verified in the local preview (Zola 0.22.1) in
  both themes at 1440/1280/1024/800px and via DOM checks at 375px.


### Post-M9: website — what 0.9.6 changed on the landing page (done)

**What.** The release of 0.9.6 needed the site to stop describing 0.9.5: a card for image
input, a release post, and one clause about the new address policy.

**The feature grid went from six cards to nine, and the reason is not decoration.** Adding
the images card alone made it seven, and `card-grid` is
`repeat(auto-fit, minmax(280px, 1fr))` — at desktop width that is three columns, so the
seventh card sat alone in a third row with two columns of empty space. Rather than pad the
grid for its own sake, the two capabilities the landing page had never mentioned were
written up: **history compression** (a long conversation folds into a rolling summary, and
the model can read the folded range back) and **speech** (`/tts` through OpenAI or Gemini).
Both are real, user-visible and were missing; 3×3 is what they happen to make. Verified by
rendering, not by reasoning about the CSS — headless Edge against a local `zola serve`, the
recipe from the earlier website work.

**The privacy card gained a sentence** rather than a card of its own: "the assistant's web
tools stay on the public internet — your own machine and network are off limits unless you
say otherwise". The address policy is a property of the thing the card already describes,
and a separate card would have advertised a *restriction* as a feature.

**The release post** (`blog/2026-08-13-mindfork-0-9-6.md`) leads with images and gives the
address policy its own section, because the reasoning is the interesting part for a reader:
the assistant's links are rarely typed by the user — they come off a page it just fetched —
and that is why the default had to change. Zola derives the post's date from the filename
prefix and its slug from the rest, so the URL is `/blog/mindfork-0-9-6/`.

No live run: pure site content. The build was verified with the pinned Zola 0.22.1
(0.23.x still cannot discover `templates/` on Windows — lessons §6).

### Post-M9: website — two articles: the self-model and vector search (done)

**What.** The "how mindfork is put together" series grew from two articles to
four: `articles/self-model.md` (weight 3) and `articles/vector-search.md`
(weight 4). Both are written for the person the site targets — someone able to
install, configure and use the feature — so they explain the *why* of the
design at user altitude and leave constants, file paths and thresholds to
spec §17 / §9.5 and the journals they were sourced from
([self-model.md](self-model.md), [rag.md](rag.md)).

**What each article chose to carry.** The self-model piece is built around
the snapshot-vs-biography split (summary/goals/user-model as the present
tense, `@self` observations as the scars), the opt-in and `F3`/`/self`
controls, and the bug history — the "mood swing" wholesale-replace bug and
the write-once goals are told as the reason merge semantics and `#id`
handles exist. The vector-search piece leads with the dedicated embedding
server (ADR 0002) and the `-ub`/`-b` batch trap a user actually hits, walks
the four consumers (knowledge base, large attachments, notes/self-model,
web rerank), names the one deliberate non-consumer (cross-chat search is
trigram FTS), and gives the canary/generation/`/reindex` story a section —
it is the best "boring and reliable" material this area has. Both articles
close every claim loop inside the series by cross-linking each other.

**One Cyrillic exception, marked in place.** The vector article demonstrates
cross-lingual retrieval with a Russian query beside its English chunk; the
line carries a same-line `cyrillic-ok` HTML comment (invisible in rendered
Markdown), which is exactly the narrow use the scanner's markers exist for.
The site remains English-only; the `/ru/` mirror stays a deferred fork.

No live run: pure site content, no Rust touched. Gates
(`cyrillic_scan`/`link_check`/`doc_index_check`) green; `zola` is not
available in the working environment, so the build check rides on the
`site.yml` PR gate (pinned 0.22.1), which these files exercise.

### Post-M9: website — two more articles: local speed and the Python sandbox (done)

**What.** The "how mindfork is put together" series grew from four articles to
six: `articles/local-speed.md` (weight 5 — FlashAttention and speculative
decoding in managed mode) and `articles/python-sandbox.md` (weight 6 — the
Wasmer/WASIX sandbox against Docker and the system interpreter). Same altitude
as the previous pair: the *why* of the design for a reader able to install and
use the feature, with constants and file paths left to the sources (the engine
journal's FlashAttention/spec-decoding entry;
[ADR 0005](../decisions/0005-python-sandbox-wasmer.md) and the sandbox
research log).

**What each article chose to carry.** The speed piece is built around one rule
stated before any mechanism — neither lever is a quality dial — and explains
both at user altitude: FlashAttention as the same attention computed in a
better order (why it pays with context length, why `auto` passes no flag and
defers to llama.cpp), speculative decoding as draft-and-verify (the draft
never decides; acceptance rate makes it a lever to *measure*, and the honest
caveat that low acceptance can cost more than it saves). The drafter menu maps
the `SpecType` families — sibling GGUF, EAGLE-3, MTP companions, the
zero-setup ngram group — and the managed-mode section carries the project's
own guarantees: defaults pass no flags (byte-for-byte launch line), the draft
path is preflight-checked before spawn, the embedder is excluded (generates no
tokens), external/cloud untouched. The sandbox piece frames the three roads
(system Python / Docker / wasm) and lets the capability argument carry the
Docker comparison — files and sockets *absent*, not forbidden — with the
sidecar choice told as the third occurrence of the separate-process-behind-a-
contract pattern (engine, embedder, sandbox) and the `TCP_NODELAY` shim
closing as the proven-live war story. Both articles keep the series habit of
closing every claim loop by cross-linking earlier pieces.

**Facts sourced, not invented.** Package set and versions (numpy/pandas as
native WASIX wheels, the requests stack, beautifulsoup4, CPython 3.13) from
the `sandbox_setup` lock list; the ~⅓ s interruption, the V8-only-on-Windows
embedding argument and the security posture from ADR 0005; the flag set,
`auto`-passes-nothing semantics and the `-md` preflight from
`shared/api/managed.rs`, `shared/config.rs` and the engine journal entry.

No live run: pure site content, no Rust touched. Gates
(`cyrillic_scan`/`link_check`/`doc_index_check`) green; built locally with the
pinned Zola 0.22.1 — both new pages render and `zola check` passes internal
links.

### Post-M9: website — the 0.9.7 release post (done)

**What.** `blog/2026-08-17-mindfork-0-9-7.md`, published with the release, plus
one line of the landing page. The post is organised around what the release
actually is rather than the changelog's order: **reach** — running the app in a
terminal that belongs to something else, and giving conversations an address.
Four sections carry the four tracks that share that theme (command-only
control, OSC 52, `/export`, the cross-chat pair with `chat://` links), and a
short "also in this release" list carries automatic titling, the external
server's key, the model name in the feed and the `F1` help layout.

**The landing grid stayed at nine.** The `0.9.6` release had deliberately made
it 3×3 (a seventh card left a row two-thirds empty), so a tenth card would
reopen exactly that problem; the one 0.9.7 claim the landing did not make —
that the assistant can look through your *other* conversations — was folded
into the existing "Memory that persists" card instead, with the "if you let
it" that the tools' off-by-default posture requires. The reach story is left
to the post: it is a story about *where* you run the app, not a capability
card.

**What the post refuses to overclaim.** The OSC 52 section names the terminals
that do not implement it (GNOME Terminal, Terminal.app, JupyterLab) and says
the text was *sent*, not received — the same honesty the feature's own note
carries, since the protocol acknowledges nothing. The VS Code key list is the
measured one from the track's research, not an impression. The cross-chat
tools are introduced as off by default in the same sentence that describes
them, so no reader arrives at the settings screen expecting them to be on.

No live run: site content plus a one-line landing edit, no Rust touched. Gates
(`cyrillic_scan`/`link_check`/`doc_index_check`) green.

### Post-M9: website — the 0.9.8 release post and the twelve-card landing (done)

**What.** `blog/2026-08-27-mindfork-0-9-8.md`, published with the release, plus
the landing page catching up with everything shipped since 0.9.7. The post is
organised around the release's actual theme — **work**: handing the assistant
something real to do and watching it done. Three full sections carry the code
workspace (attach → read/edit → the user's own build/test command lines → the
`F4` diff-and-revert screen), the grown-up sub-agents (tools, transcripts as
live nested conversations, the migration of old calls) and the Tavily-backed
web search with the measured free-engine block story; an "also" list carries
the binary rename, OSC 11 theme detection, per-screen `F1`, the Russian legal
texts, split GGUF, the stage-3 commands and the missing-database warning. A
closing note tells the reader about the two schema migrations in the terms the
CHANGELOG's Data rubric uses — automatic, after a pre-migration backup,
refused by an older build rather than misread.

**The landing grid went 9 → 12, on the 0.9.6 precedent.** Two capabilities of
this release deserved cards no existing card could absorb — the attached
project and delegation — and `card-grid` is three columns at desktop width, so
eleven cards would leave the last row a card short. Instead of padding, the
strongest capability the landing had never mentioned was written up: search —
inside the open conversation and across every chat's messages, sub-agent
transcripts included. The rows now read as themes: engine and memory, tools
and work, interface and search, endurance/speech/privacy. Alongside the grid:
the hero line trades "an agentic tool loop" for "an agentic tool loop that
reaches your own code project", the fetch panel's `tools` row gains
`project`, and the at-a-glance article's loop section gains a paragraph on
the workspace and delegation — the article is the site's architectural tour,
and the loop's two biggest tools were missing from it.

**What the post refuses to overclaim.** The sub-agent's tool grant names its
exclusions (no sub-agents of its own, no folded history, no self-model) in
the same sentence that grants the rest. The workspace section puts the
containment story — attaching *is* the permission, commands are only ever the
user's own lines — ahead of the capabilities. The Tavily numbers (free
engines answering about twice in a row, 1000 searches a month on the free
tier) are the changelog's measured claims, not fresh impressions. The theme
item names the terminals that answer OSC 11 and the one that does not
(legacy conhost, treated as the dark background it is).

No live run: site content plus landing/template edits, no Rust touched. The
build was verified with the pinned Zola 0.22.1 (0.23.x still cannot discover
`templates/` on Windows — lessons §6): 10 pages, the post at
`/blog/mindfork-0-9-8/`, all twelve cards rendering. Gates
(`cyrillic_scan`/`link_check`/`doc_index_check`) green.

### Post-M9: the site gets its legal pair — `/privacy/` and `/code-signing-policy/` (done)
- **Stage 6 of the code-signing track** ([code-signing.md](../research/code-signing.md)
  §7). SignPath's conditions are specific about the second page: the words
  **"Code signing policy"** have to appear on the project's home page, and the
  page has to carry their attribution line, the team roles and a privacy
  statement. The footer carries both links on every page, which is how the home
  page acquires the term without a section of its own.
- **`/privacy/` is generated, not copied** — `tools/site_legal_pages.py`, which
  turns `PRIVACY.md` into `site/content/privacy.md`. The reason is not tidiness:
  the policy links to `SECURITY.md`, `docs/install.md`, `infra/website.cfn.yaml`
  and five others, and those targets resolve in a checkout and on GitHub and to
  nothing at all on a static site. The generator rewrites every
  repository-relative link to an absolute GitHub URL (nine of them) and drops the
  document's own `#` heading, which the template renders from `page.title`.
- **Committed and gated, like the RTFs — not gitignored, like the brand assets.**
  The site already has one mirror (`tools/site_sync_assets.py`, whose output is
  derived binary data nobody reads in a diff). This is prose that lands on a
  public page, so it follows the installer's precedent instead: the generated
  file is committed, and `--check` runs in CI's lint job beside
  `wizard_rtf.py --check`. The policy now exists in three places — repository,
  installer, website — and exactly one of them is editable.
- **A third template.** `legal.html` is `page.html` without its byline: a policy
  has no author line, and "12 min read" over a legal text reads as a warning
  rather than a service. Each of these documents states its own date in its first
  paragraph, which is the line a reader actually wants.
- **The status paragraph on the signing page is deliberate.** Nothing is signed
  yet — the page exists *because* a policy has to be published before anyone can
  review one — so it says so in its first line, above the attribution. Publishing
  "Free code signing provided by SignPath.io" as a bare statement of fact before
  the application is even filed would be the one thing on that page that is not
  true.
- **The IP allowlist stays on.** The stack's `AllowedIps` is untouched: the user
  will open the site when the program is tested and the repository is public
  (2026-09-02). The pages deploy either way; they are simply not reachable from
  outside yet — which also means the application cannot be filed until that
  happens, since a reviewer following the link would meet a 403.
- **Verified by building, not by reading**: `zola check` clean, `zola build`
  produces both pages, and each was rendered through headless Edge against a
  local `zola serve` — the footer's two links resolve to absolute URLs, the
  policy's rewritten links point at GitHub, and the §2 table survives the
  transform as a real `<table>`. 2745 unit tests green, 129 `#[ignore]` — no
  Rust changed.

### Post-M9: website — the 0.9.9 release post and three refreshed cards (done)

**What.** `blog/2026-09-13-mindfork-0-9-9.md`, published with the release, plus
three cards of the landing page and a paragraph of the at-a-glance article. The
post is organised around the release's theme — **keeping the conversation
moving**: five full sections carry the Python file exchange (files in by `#N`,
`/w/out` back into the chat, the chart shown to the model, `/file open`'s
allowlist, the local interpreter on the same contract, the read-only package
image), the background work (background sub-agents and dialogues with task
notifications, the `F7` tasks screen and `/tasks stop`, the three parallelism
settings, the yielding background requests), the directed dialogue, the engine
download (`mindfork llama backends|setup|installed|remove`, the empty binary
field) and `/continue` with the external server's model name; a bulleted "off
until you say so" section carries the security and privacy items (web tools
off in a fresh install, the Tavily key no longer read from the environment, the
verified Python download, the privacy policy and the Legal tab, the
dictionaries' provenance), and an "also" list the rest. The closing note tells
the reader about the `files/` directory and the chat schema 2 → 4 in the
CHANGELOG's Data terms.

**The landing grid stays at twelve; three cards catch up.** No 0.9.9 capability
needed a card of its own — each is a growth of one the grid already has — so
three cards were extended in place instead of the grid being reshaped: *Local
first* gains the one-command engine download, *Real tools* says the Python
runtime reads the chat's files and hands back what it saves, *It can delegate*
gains "several at once, or in the background while it keeps talking to you".
The at-a-glance article's loop section gains a third paragraph on the same two
things — the file exchange and background runs — since the article is the
site's architectural tour and both change what the loop is.

**What the post refuses to overclaim.** Every number is the changelog's
measured one: the RTX 4090 sessions figures, the 46 s → 24 s and 5.6 s →
under 2 s waits, en_GB's 14 000 stems; the withheld-chart measurement is quoted
as "nearly every time with the picture and never without" (4/5 and 5/5 against
0/5), with the two models named. The parallelism paragraph says which two of
the three settings default to 1 and that tool calls default to four on a cloud
engine only. The Protected View item says Office and a workbook or document —
the CSV exception is the changelog's, and the post does not extend the claim to
it. The web-tools item says an existing installation keeps its settings in the
same sentence that announces the new default.

**Zola was not run locally**: the pinned 0.22.1 is not installed on this
machine and installing it was not part of the task — the site workflow's PR
gate runs `zola check` and `zola build` at that version on every PR touching
`site/`, so the build is verified there before the merge. No live run: site
content plus content edits, no Rust touched. Gates
(`cyrillic_scan`/`link_check`/`doc_index_check`) green.

### Post-M9: website — the overview rebuilt, a screenshot shortcode and the trust-boundaries article (done)

**What.** `articles/mindfork-at-a-glance.md` rewritten against the 0.9.9
application, a seventh article, `articles/trust-boundaries.md`, and the
site's first two shortcodes. The overview had been written on 2026-08-10 for
0.9.5 and touched only by the two release-day paragraphs that grew its loop
section by accretion; every other section still described 0.9.5. Now: the
engine section gains the one-command download and the empty binary field;
the loop section's tool set is a list by kind (web, files and the project,
Python, delegation and the dialogue, plugins/speech/introspection) followed
by one paragraph on what runs in the background — runs *and* the app's own
requests, one at a time, yielding, reserving their place in the pool; a new
**"One turn, start to finish"** section shows the assembly of a request as a
mono-block diagram and a paragraph (the self-model block chosen by embedding
the message, attachments inline or by reference, notes and RAG reached
through tools rather than injected — checked against spec §9: `note_recall`
and `rag_search` are tools, nothing injects them); the memory list gains
cross-chat search and the model-history record; storage gains `files/`,
`workspace/`, the schema versions and the backup-then-migrate rule; a new
**"Where the trust boundaries are"** section carries the network, the
machine, consent, and secrets/the author in four bullets; the TUI section
gains the `auto` theme, pictures in and speech out, typed commands, OSC 52,
`/export` and per-screen `F1`. Every section ends with a "Full story" link to
its sister article, and the page ends with a "Read on" list — the overview
had linked to none of the five articles that all link to it.

**The trust-boundaries article.** The landing's "Private by construction"
card was the only capability card with no article behind it, and the
security posture is the most architectural subject the site had not written
up. The article is written from `PRIVACY.md` and spec §9.8, not from the
landing: what leaves the machine (endpoint by endpoint, the web tools off in
a fresh install, the keyed provider only where the key was put, MCP as local
subprocesses), the public-internet guard with its five-hop redirect
re-check, what the sandbox and the project tools can touch (the caps, the
read-only package image, the typed command lines, the empty `fs_root`
warning, `/file open`'s allowlist, the mark of the web), when the app asks
(what `confirm_dangerous` gates and why a server's annotations are not
consulted; background runs never ask), secrets (DPAPI / machine-id key,
what ADR 0008 does *not* defend against), what the author receives, and a
closing "what this does not claim" — a process boundary is not a VM, local
Python has no boundary, plugins run as you, the binaries are unsigned yet.

**The shortcodes.** `screenshot(name, title)` with a caption body renders the
landing's `figure.term` markup — both themes via `load_data` over the synced
`static/screenshots/`, so the article gets the `chat`, `self-model` and
`settings-tools` renders in the same chrome as the gallery; `.prose
figure.term` gets a margin the grid never needed. `app_version()` prints
`[extra] app_version` from `config.toml`, so the overview can say which
release it describes without a hand-edited number: the release checklist
(AGENTS.md §6 step 1) now bumps that key with `Cargo.toml`. The config
comment that said "templates use no shortcodes" is amended — the shortcodes
use the same v1/v2-compatible Tera, so the 0.23 bump stays a version number.

**What the prose refuses to overclaim.** The turn diagram says readers *may*
run together (four at a time on a cloud engine, one after another on a local
one until the setting is raised). The local interpreter's memory limit is
"of its own", not the sandbox's. The `fs_root` paragraph says an empty root
leaves the tools unrestricted, as the privacy policy does. The signing
paragraph repeats the policy page's order — unsigned first. The landing is
untouched: no card gained, none reworded.

**Verified three ways, because the first one lied.** The user installed Zola
mid-PR — 0.23.6 — and on it every shortcode failed with `Unknown tag` /
`Unknown function` out of `__tera_one_off`, while a copy of `main` (no
shortcodes) built cleanly; a one-line shortcode dropped into that copy failed
the same way, so 0.23.6 on Windows discovers `templates/*.html` but not
`templates/shortcodes/` (lessons §6 amended). The `site.yml` PR gate — 0.22.1
on Linux — was green on the same files, and 0.22.1 built from its git tag
(crates.io does not carry Zola) then confirmed it locally: `check` and
`build` green, 14 pages, the overview with its three figures as six inlined
SVGs, the stamp rendered as `0.9.9`, no Tera syntax leaking into either page,
every internal link of both articles resolving under `public/`, the pages
served and opened. Reading times as built: the overview 9 min (the review
budgeted seven to eight; the diagram and the two lists are the difference,
and both earn it), the trust article 8. No live run: site content and
templates, no Rust touched. Gates (`cyrillic_scan`/`link_check`/
`doc_index_check`) green.

### Post-M9: website — two more articles: the code workspace, and background runs on the context pool (done)

**What.** The two articles the overview's review named and the first PR left
out, written into the same PR at the user's request:
`articles/code-workspace.md` (weight 8) and `articles/background-runs.md`
(weight 9), plus their links from the overview's loop section and its
"Read on" list. Both are written from the repository's own records rather
than from the release posts — the finished plan
`docs/history/code-workspace.md` (its decided forks, §3.3's execution
rules, §5's "deliberately not doing", §7.1–§7.2's probe and §7.9's no-go,
§7.10's audit) and spec §9.12 for the first; `docs/research/`'s
admission-by-budget, silent-tasks-budget, silent-preemption, cpu-batch,
slow-prefill-detection, roll-usage-calibration, parallel-subagents,
concurrent-tools and background-subagents, with spec §6.3, §9.3.2 and
§11.10, for the second.

**The code workspace article** is organised as *containment before
capability*, the order the plan's security posture puts them in: attaching
as the consent (byte-identical requests without a project; the contrast
with the global file tools), the five reading and editing tools with the
fidelity rules (exact-once fragments, EOL/BOM/encoding round trip, the
windows-1251 damage that motivated it), the three command slots (no
arguments by schema, no shell, the pipeline refused when the line is set,
the process tree killed, partial output kept as the deliberate inverse of
the sandbox, head-and-tail truncation, one at a time), the pre-image
journal and the `F4` screen with the reattach defect the audit found, then
the probe (5/5 on both families, the five-line fragment with the `12→`
prefixes stripped and widened past the duplicate), and a full section on
the semantic index that did not ship — the two denominators that point in
opposite directions, the six instrument defects, the one effect that
survived (answering without looking, 15 → 8) and why shipping on it would
have been shipping unmeasured.

**The background-runs article** follows the tracks in the order they were
built: the second tool over the flag (Claude narrating a background it had
not asked for, 0/3 against 3/3 everywhere with a required field or a second
tool), what a run owns and why it never asks for confirmation, then the
pool — the two collisions reproduced on a 2048 pool (the prefill collision
that ends the healthy slot too, the growth collision two prompts that fit
still cause) and the per-slot cap rejected for its silent `length` cut —
admission by budget with the estimator's 2.19× and the per-kind
calibration (85 against 4358), the silent lane (the roll at three quarters
of the window beside the next turn; 88 s / 16.5 s after), preemption (46 s
→ 24 s, 5.6 s → under 2 s), the CPU batch (23 s → 6.5 s at a seventh slower
prefill), the slow-prefill note, the sessions numbers per model on one 4090
(2.8× small, 1.22× Gemma 4 31B, 2.9× Qwen 3.6 27B; the RAM prompt cache
making interleaving free; queueing past the slots), parallel tool calls
(26/26), and the stop/quit window rules.

**What the prose refuses to overclaim.** Every figure is the research
document's, with its stand named (the CPU build, the LAN stack, one RTX
4090) where the number depends on it; the 31B's 1.22× is reported beside
the 27B's 2.9× rather than either alone, and "above two buys nothing a user
would feel" is quoted for that card and that model. The index section keeps
both denominators and says the effect changes sign with the definition. The
workspace article says read-before-edit is taught, not enforced, and that
git integration is absent by choice.

**Verified** with the pinned Zola 0.22.1, now on this machine's PATH:
`check` and `build` green, 16 pages, both articles at their slugs with
every internal link resolving, no Tera syntax leaking; reading times as
built are in the PR. Gates (`cyrillic_scan`/`link_check`/`doc_index_check`)
green. No live run: site content, no Rust touched.
