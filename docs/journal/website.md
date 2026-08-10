# Journal — The mindfork.io website

The project website: stack research, the Zola site under `site/`, its design
system, and the AWS deployment (S3 + CloudFront) once it ships.

**Reference documents for this area:** [docs/research/mindfork-io-website.md](../research/mindfork-io-website.md), AGENTS.md §6

Entries record what was done, why, what was measured and what was rejected —
the reasoning behind the site, not its current shape. For the current shape
read the research/design doc above; for the traps that recur across areas
read [lessons.md](../lessons.md).

## Entries (2)

- Post-M9: website — research + S1 scaffold (Zola, terminal-styled) (done)
- Post-M9: website — S2 infra: one CloudFormation stack, mindfork.io live (done)

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
- **S1 scaffold**: a custom theme on the brand system (artwork/README.md) —
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
  mirrored from `artwork/` by `tools/site_sync_assets.py` (gitignored, so
  the screenshots' single source of truth stays rot-gated in `artwork/`);
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
