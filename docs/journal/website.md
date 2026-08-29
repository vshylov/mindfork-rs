# Journal — The mindfork.io website

The project website: stack research, the Zola site under `site/`, its design
system, and the AWS deployment (S3 + CloudFront) once it ships.

**Reference documents for this area:** [docs/research/mindfork-io-website.md](../research/mindfork-io-website.md), AGENTS.md §6

Entries record what was done, why, what was measured and what was rejected —
the reasoning behind the site, not its current shape. For the current shape
read the research/design doc above; for the traps that recur across areas
read [lessons.md](../lessons.md).

## Entries (11)

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
