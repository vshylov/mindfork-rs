# Journal — The mindfork.io website

The project website: stack research, the Zola site under `site/`, its design
system, and the AWS deployment (S3 + CloudFront) once it ships.

**Reference documents for this area:** [docs/research/mindfork-io-website.md](../research/mindfork-io-website.md), AGENTS.md §6

Entries record what was done, why, what was measured and what was rejected —
the reasoning behind the site, not its current shape. For the current shape
read the research/design doc above; for the traps that recur across areas
read [lessons.md](../lessons.md).

## Entries (1)

- Post-M9: website — research + S1 scaffold (Zola, terminal-styled) (done)

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
