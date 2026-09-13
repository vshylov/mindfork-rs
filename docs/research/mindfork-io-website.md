# mindfork.io — the project website: stack research and track design

Status: **track complete (2026-08-11)** — S1–S4 done, the site is live at
https://mindfork.io; decisions in §7, stage outcomes in §8. Originally:
research + design plan (2026-08-10).
Verified facts below carry their sources; AWS and Zola facts were checked
against primary documentation and release pages on 2026-08-10, and the account
state was inspected with the read-only `aws` CLI the same day.

## 1. Context and goal

The repository already expects this site to exist:

- the README masthead carries a `mindfork.io` badge linking to the (currently
  unserved) domain;
- the demo-screenshots track deliberately deferred its stage 4 — an SVG writer
  for the generated screenshots — "to the site work, which will set the sizes
  and themes it must serve" ([docs/roadmap.md](../roadmap.md));
- the domain **mindfork.io is registered at Route53** and its public hosted
  zone exists (`Z0725539OZLHCKB3LW5G`) with only the NS/SOA records — nothing
  is serving it.

Requirements (user, 2026-08-10):

- hosted on **AWS** (the domain is registered there);
- a simple site: tells the product's story, has a **simple blog** for project
  news, and supports **standalone article pages**;
- adding a news post or an article must be trivial for the maintainer —
  ideally "drop a Markdown file, push";
- the design must be **stylish and reflect what the product is** — a terminal
  application;
- the dev machine has the `aws` CLI (authenticated) and Docker.

Account state (inspected 2026-08-10): account `976877302614`, default region
`us-east-1` — which is exactly where a CloudFront viewer certificate must
live, so no cross-region stack split is needed. No CloudFront distributions
exist. One caveat: the CLI session is the **root** user; the deploy pipeline
must get its own least-privilege IAM role instead (§5.3), and nothing in this
track should encourage day-to-day root usage.

## 2. What the repository already provides

The site does not start from a blank page — three finished tracks feed it:

- **Brand identity** ([assets/README.md](../../assets/README.md),
  [docs/branding.md](../branding.md)): palette (accent `#c25a27`, branches
  `#5c6370`, backplate `#09090b`, text `#e4e4e7`/`#18181b`, tagline
  `#71717a`), **JetBrains Mono ExtraBold** (OFL 1.1), the pixel 16×16 glyph
  (drawable with half-blocks — i.e. also with CSS), five wordmark lockups as
  self-contained SVGs (dark/light/mono/stacked/tagline), icons 16–256 px and a
  ready `.ico` favicon, the tagline `TUI · RUST · LOCAL LLM`.
- **Generated screenshots** ([docs/history/demo-screenshots.md](../history/demo-screenshots.md)):
  `assets/screenshots/*.png` — five screens × dark/light, produced from JSON
  frame dumps by `tools/screenshots.py` and guarded against rot by a unit
  test. The site can embed these PNGs on day one; the deferred **SVG writer
  (stage 4)** belongs to this track's later stages — the site decides the
  sizes and themes it needs, then the writer is built to serve them.
- **CI conventions** (`.github/workflows/`, one file per concern): a site
  workflow is a new `site.yml` with a `paths: [site/**]` filter — the cargo
  gates are untouched.

## 3. Hosting on AWS — options (verified 2026-08-10)

### 3.1 Option A — S3 (private) + CloudFront (OAC) + ACM + Route53 · recommended

Still the canonical AWS answer for a static site in 2026: a **private** S3
bucket as a REST origin behind CloudFront with **Origin Access Control** (OAI
is legacy; S3 "website hosting" mode is explicitly not used), an ACM
certificate, Route53 alias records.
([repost.aws](https://repost.aws/knowledge-center/cloudfront-serve-static-website),
[CloudFront docs](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/private-content-restricting-access-to-s3.html))

Verified specifics that shape the implementation:

- **The ACM certificate must be in `us-east-1`**
  ([docs](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/cnames-and-https-requirements.html)).
  Our default region already is `us-east-1` → **one** CloudFormation stack.
- **Directory URLs need a URI rewrite.** CloudFront's default root object
  applies only at `/`; with a private REST origin, `/blog/post/` must be
  rewritten to `/blog/post/index.html` by a **CloudFront Function**
  (viewer-request, `cloudfront-js-2.0`). This matches Zola's trailing-slash
  output exactly, and is the canonical AWS pattern
  ([AWS blog](https://aws.amazon.com/blogs/networking-and-content-delivery/implementing-default-directory-indexes-in-amazon-s3-backed-amazon-cloudfront-origins-using-cloudfront-functions)).
  Falling back to the public S3 *website endpoint* to avoid the function
  would lose OAC and HTTPS-to-origin — rejected.
- **CloudFront's always-free tier is current in 2026**: 1 TB egress + 10 M
  requests + 2 M function invocations, monthly, no expiry
  ([pricing](https://aws.amazon.com/cloudfront/pricing/pay-as-you-go/)).
  Invalidations: first 1 000 paths/month free; `/*` counts as one path.
- **Route53**: $0.50/mo per hosted zone (already being paid for mindfork.io);
  **alias queries to CloudFront are free**
  ([pricing](https://aws.amazon.com/route53/pricing/)).
- **Everything is CloudFormation-able**, including DNS-validated ACM issuance
  (`DomainValidationOptions.HostedZoneId` — the stack waits until the cert is
  issued; note the zone id is passed *without* the `/hostedzone/` prefix),
  `AWS::CloudFront::OriginAccessControl`, and `AWS::CloudFront::Function`
  with `AutoPublish: true`.

**Cost for this site** (<100 MB, ~10k visits/mo): CloudFront $0.00 (deep
inside always-free), ACM $0.00, S3 ~$0.01, Route53 $0.50 — already paid.
**≈ $0.01/mo incremental.** Headroom: ~100× traffic growth before the first
dollar.

### 3.2 Option A+ — CloudFront flat-rate **Free plan** (optional add-on, later)

Launched 2025-11-18: per-distribution flat-rate plans; the **Free $0/mo** plan
covers 1 M requests + 100 GB/mo (soft caps, no overage billing), includes a
WAF ACL (mandatory on the plan), the TLS cert, CloudFront Functions — and
**absorbs the attached Route53 hosted zone's fee**
([announcement](https://aws.amazon.com/about-aws/whats-new/2025/11/aws-flat-rate-pricing-plans),
[docs](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/flat-rate-pricing-plan.html)).
Caveats: subscription is console/CLI only — **no CloudFormation support yet**;
the mandatory WAF ACL is extra config surface; allowances are far below
pay-as-you-go always-free (100 GB vs 1 TB). Net effect for us: −$0.50/mo.
Verdict: **not part of the design**; can be toggled on later without touching
the architecture. Pay-as-you-go stays the IaC-clean default.

### 3.3 Option B — AWS Amplify Hosting · rejected

Managed Git-connected hosting: auto cert, apex+www redirects, PR previews,
atomic deploys. Verified against 2026 pricing: build $0.01/min, **data served
$0.15/GB with no always-free cushion for an account older than 12 months**
(the July 2025 free-tier revamp changed nothing for pre-2025 accounts — the
legacy 12-month tier simply expired long ago); CloudFront's 1 TB free tier
does **not** apply to Amplify. Zola has no first-class build preset — the
build spec would curl a release tarball anyway, erasing much of the
convenience. **≈ $2.10/mo incremental** and weaker IaC coherence for a repo
whose CI is already GitHub Actions. Rejected: pay more to control less.

### 3.4 Dismissed

- **CloudFront Hosting Toolkit** (awslabs) — dormant: last push 2025-04, 16
  months stale. Not adopted.
- **New CloudFront console guided setup** (2025-06) — produces exactly the
  Option A architecture by hand; superseded by CloudFormation here.
- **Lightsail CDN** — fixed fee from ~$2.50/mo, strictly worse than $0.
- **App Runner / EC2 / containers** — a running server for static files;
  wrong shape entirely.
- **Non-AWS hosts** (GitHub/Cloudflare Pages) — out of scope by requirement:
  the domain and the account live at AWS.

### 3.5 Deploy pipeline — GitHub Actions → AWS via OIDC

Confirmed current best practice: **OIDC federation** — no long-lived keys in
GitHub secrets. `aws-actions/configure-aws-credentials` is on **v6** (v6.2.3,
2026-07-22). The IAM role trusts `token.actions.githubusercontent.com`
scoped by `sub` to this repository and branch `main`, and carries only
`s3:PutObject/DeleteObject/ListBucket` on the site bucket +
`cloudfront:CreateInvalidation` on the one distribution.
Flow: `zola build` → `aws s3 sync public/ s3://… --delete` →
`create-invalidation --paths "/*"` (one path — free).

## 4. Static site generator — options (verified 2026-08-10)

### 4.1 Option A — Zola · recommended

Rust, single binary, no runtime dependencies — the same shape as the product
itself. Verified: **v0.23.2 (2026-08-07)**, actively maintained (17.3k stars,
~two feature releases a year plus fast patches), signed release binaries for
Windows/Linux/macOS, `winget install getzola.zola`, official pinned Docker
image `ghcr.io/getzola/zola` (**no `latest` tag by policy**; use ≥ v0.23.1 —
the v0.23.0 image shipped without CA certificates). Tool license EUPL-1.2 —
applies to the binary, not to site content.

Covers every requirement natively, each verified in the official docs:
Markdown content with a sections/pages model (a paginated blog section plus
standalone article pages is the docs' own canonical layout), Tera templates,
built-in Sass, syntax highlighting (Giallo engine since 0.22, separate
light/dark CSS output), taxonomies (tags), per-language Atom/RSS feeds, a
search index (elasticlunr/fuse formats; Russian needs only the
lunr-languages stemmer JS), `zola serve` live reload (0.23 specifically
improved file-watching on Windows), and **multilingual sites**:
`[languages.ru]` in the config, the `page.md` + `page.ru.md` filename
convention, output under `/ru/…`, per-language feeds/search/taxonomies. One
documented property to plan around: **no language fallback** — a page
appears in `/ru/` only when its `.ru.md` exists. Adding a news post is
exactly "drop a `.md` file into `site/content/blog/`".

**Version decision.** Zola **0.23.0 (2026-08-05) is a deliberately breaking
release** — the changelog calls it "probably the most breaking version of
Zola that will happen": shortcodes are removed in favor of Tera v2
"components", the highlighter config moved at 0.22 (syntect → Giallo), and
the config file is now `zola.toml` (`config.toml` read as a fallback). Most
community themes still target ≤ 0.22 (tabi's 0.23 migration is an open
issue). Since this site uses **custom templates anyway (§4.3), we start
clean on 0.23.x** and inherit none of the legacy-theme lag; the alternative
— pinning 0.22.1 to borrow a theme — buys nothing this design wants.

**S1 amendment (2026-08-10, measured):** 0.23.0–0.23.2 turn out not to
discover `templates/` **on Windows at all** — reproduced on a pristine
`zola init` site, and tracked upstream as
[getzola/zola#3229](https://github.com/getzola/zola/issues/3229) (the fix,
[#3222](https://github.com/getzola/zola/pull/3222), is merged but
unreleased). So S1 ships **pinned to 0.22.1**, with the config named
`config.toml` (0.22's only name; 0.23 reads it as a fallback) and the
templates written in v1/v2-compatible Tera with no shortcodes — the future
bump to 0.23.x is a version number, not a migration.

**Second amendment (2026-09-14, done):** the bump happened at 0.23.6, and it was
a small migration after all, because the overview had gained two shortcodes in
the meantime: they became Tera 2 components in `templates/components.html`
(a `screenshot` with a body; the version stamp is `{{ config.extra.app_version }}`
directly in the content, since every page is a Tera template now), the config
took its canonical name `zola.toml`, and the templates needed nothing — they use
only `load_data`, `get_url`, `get_section`, `safe` and the block syntax, none of
what Tera 2 removed (`concat`, `macro`, `map`, `filter`, `slice`, the `date`
formats). Measured on the migrated copy against the 0.22.1 build: the same 16
pages, the figures and the stamp intact, no component wrapped in `<p>` (upstream
#3242 does not reach this markup), and only cosmetic differences — hrefs no
longer entity-escaped, the feed's `<name>` untangled, a per-language reading
time. What the bump buys: `zola serve`'s watcher works on Windows (a change
detected and rebuilt in 92 ms; 0.22.1's never fired there), the maintained line
(0.22.1 is from January), and components usable in templates too. What it costs:
a literal `{{`/`{%` in content now breaks the build unless wrapped in
`{% raw %}`, and the 0.23 line is moving fast (six patch releases in five weeks),
so the pin stays exact. The CI install moved off `taiki-e/install-action`, whose
manifest lagged the tag (0.23.5 on the day 0.23.6 was two days old), to the
release asset itself, verified with `gh attestation verify --owner getzola`
(SLSA provenance from getzola/zola's release workflow at the tag).

### 4.2 Rejected alternatives

- **Cobalt** (Rust) — alive but patch-level maintenance only (v0.20.4,
  2026-02); Liquid templates; no documented multilingual, search-index or
  taxonomy support. A generation behind Zola for this shape.
- **mdBook** (Rust, rust-lang org, v0.5.4 2026-07) — book/docs-shaped:
  chapters and a sidebar, no landing layout, no blog/feeds/taxonomies.
  Wrong fit here; noted as the natural engine for a future
  `docs.mindfork.io`.
- **Marmite** (Rust, appeared late 2024; v0.4.2) — deliberately minimal
  flat-blog generator ("don't expect complex features" — its own README),
  AGPL-3.0 tool license; no room for a custom-designed landing.
- **Astro 6** (Node 22+, 2026-03) — the mainstream non-Rust choice:
  best-in-class DX and a huge ecosystem, but it drags a Node toolchain and
  its supply-chain surface into a repo that has none, for a site that needs
  no islands/JS framework, with faster major-version churn (5→6 in ~15
  months) than Zola's. Rejected on toolchain weight and identity: a Rust
  project's site built by the Rust SSG is part of the story.
- **Hand-rolled generator** — the project builds its own markdown renderer
  for the TUI, but a website generator is undifferentiated heavy lifting;
  rejected.

### 4.3 Theme

Custom templates, not a stock theme — and the theme survey confirms it: no
maintained theme delivers a terminal aesthetic (**terminimal**, the classic
of the genre, is stale since 2024-08 and tested against Zola 0.19;
**zola.386**'s retro-DOS look is staler yet), while the strong maintained
themes (**tabi** — which does prove `ru` localization in the wild,
**abridge**) are general dev-blog designs still on ≤ 0.22. Meanwhile the
brand system the site must express (palette, JetBrains Mono, the pixel
glyph, dark/light) is already fully specified in
[assets/README.md](../../assets/README.md) — a stock theme would fight
it. A Zola theme is a handful of Tera templates plus Sass; terminimal
serves as visual prior art, not a dependency (MIT, so borrowing fragments
is clean).

## 5. Proposed architecture

### 5.1 Repository layout (fork F3)

```
site/
  zola.toml              # base_url = "https://mindfork.io", multilingual-ready
  content/
    _index.md            # landing
    blog/_index.md       # news section: paginated, Atom feed
    blog/2026-08-10-v0-9-5.md
    articles/_index.md   # standalone long-form pages
    articles/<slug>.md
  templates/             # base, index, section, page, blog
  sass/                  # brand palette as variables; dark/light via
                         # prefers-color-scheme + manual toggle
  static/
    screenshots/ → sourced from assets/screenshots (see §5.4)
    fonts/               # JetBrainsMono woff2 subset, self-hosted
    favicon.ico → assets/mindfork.ico
infra/
  website.cfn.yaml       # the one CloudFormation stack (us-east-1)
.github/workflows/site.yml
```

### 5.2 URL scheme

`/` · `/blog/` (+ pagination) · `/blog/<slug>/` · `/articles/<slug>/` ·
`/privacy/` · `/code-signing-policy/` ·
`/atom.xml` · `/sitemap.xml` · `/404.html` · later `/ru/…` mirrors.
The last two are the legal pair added for the code-signing track
([code-signing.md](code-signing.md) §7): both use `legal.html` (`page.html`
without its reading-time byline), both are linked from the footer on every page
— which is also how the *home* page comes to carry the words "Code signing
policy", a condition of the arrangement — and `/privacy/` is **generated from
`PRIVACY.md`** by `tools/site_legal_pages.py`, committed and gated, since the
policy is edited in the repository and published in three places.
`www.mindfork.io` → 301 to apex (same CloudFront Function that does the
index rewrite). Trailing-slash directory URLs — Zola's native output.

### 5.3 The CloudFormation stack (one file, us-east-1)

- `AWS::S3::Bucket` — private, BPA on, SSE-S3, no website mode;
- `AWS::S3::BucketPolicy` — `cloudfront.amazonaws.com` principal with
  `AWS:SourceArn` = the distribution;
- `AWS::CloudFront::OriginAccessControl` (s3 / sigv4);
- `AWS::CertificateManager::Certificate` — `mindfork.io` +
  `*.mindfork.io` SAN (future `docs.`/`demo.` subdomains cost nothing),
  DNS-validated into the existing zone, auto-issued;
- `AWS::CloudFront::Function` (`cloudfront-js-2.0`, AutoPublish) — index
  rewrite + www→apex redirect + the optional maintenance IP allowlist
  (stack parameter `AllowedIps`, empty = public; a non-empty list also
  turns the distribution's IPv6 off so an IPv4 allowlist can't lock its
  own author out). Chosen over a WAF web ACL on cost: WAF is ~$5/month
  per ACL against §5.6's whole-site cent, and the function runs ahead of
  the cache, so no invalidation is needed either way;
- `AWS::CloudFront::Distribution` — aliases apex+www, the cert, HTTP/3,
  IPv6, compression, managed `CachingOptimized` policy, managed
  `SecurityHeadersPolicy` response headers, custom error responses
  403/404 → `/404.html`;
- `AWS::Route53::RecordSet` ×4 — A/AAAA alias apex + www;
- OIDC provider (`token.actions.githubusercontent.com`) +
  `AWS::IAM::Role` for the deploy workflow (§3.5), trust scoped to
  `repo:vshylov/mindfork-rs:ref:refs/heads/main`.

Deploy: `aws cloudformation deploy --region us-east-1 …` — one command,
idempotent, the whole site infra reviewable in one PR.

### 5.4 Screenshots flow

Stage 1 embeds the existing PNGs (copied into `site/static/` at build time —
single source of truth stays `assets/screenshots/`). The later stage builds
the deferred **SVG writer** in `tools/screenshots.py` with the exact sizes
and the dark/light switching the site's design settles on — closing the
roadmap item as designed, from the consumer side.

### 5.5 CI (`site.yml`)

- **PR** touching `site/**`: install the pinned Zola (`ZOLA_VERSION` in the
  workflow, 0.23.6 — the release asset downloaded by tag and verified with
  `gh attestation verify --owner getzola`; `taiki-e/install-action` was the
  route until its manifest lagged a tag, second amendment in §4.1) → `zola check`
  (broken links) → `zola build` — a build gate, no deploy.
- **Push to `main`** touching `site/**` or `assets/screenshots/**`:
  build → OIDC role → `s3 sync --delete` → invalidate `/*`.
- Repo gates: `site/` needs a `cyrillic_scan.py` carve-out only when `/ru/`
  content lands (same class of exemption as `locales/*.json`).

### 5.6 Cost, privacy

≈ **$0.01/mo incremental** (§3.1). No analytics at launch — zero cookies,
zero third-party requests, no consent banner needed; if demand appears,
CloudFront standard logs or a cookieless counter can be added without
consent surface. All assets self-hosted (fonts included) — no CDN calls out.

## 6. Design direction

The product is a terminal — the site should read as one, without cosplaying
a fake shell session so hard it stops being a website:

- **Palette = the brand palette**: `#09090b` background, zinc text scale,
  `#c25a27` accent strictly in the brand's role (the fork, links, calls to
  action); light theme mirrors it per the wordmark rules; switch by
  `prefers-color-scheme` + a manual toggle (persisted in `localStorage` —
  the one piece of JS the site needs).
- **Type**: JetBrains Mono (self-hosted woff2 subset) for display/headings
  and chrome; a readable system/humanist stack for body prose — long
  articles in pure mono tire the eye.
- **Hero**: the wordmark, the tagline `TUI · RUST · LOCAL LLM`, a
  prompt-style `$ mindfork` line with a blinking block cursor (CSS only),
  and the real chat screenshot in a slim terminal window frame —
  dark/light swapping with the theme, exactly as the README does it.
- **The pixel glyph** as the structural motif: section markers, list
  bullets, the 404 page — it was designed to be drawn from squares.
- **Weight**: no JS framework, no trackers; target — each page well under
  100 KB before images, Lighthouse ~100 across the board. The fastest way a
  site about a fast local TUI can prove its point is to load like one.

## 7. Forks (user decision required)

- **F1 — hosting**: **(a) S3 + CloudFront, pay-as-you-go, one CFN stack —
  recommended**; (b) same + flat-rate Free plan add-on (later, console-side,
  −$0.50/mo, +WAF); (c) Amplify Hosting (rejected — §3.3).
- **F2 — generator**: **(a) Zola 0.23.x, custom templates — recommended**;
  (b) Astro; (c) other.
- **F3 — where the site lives**: **(a) `site/` in this repository —
  recommended** (screenshots and their rot gate live here; one PR flow; a
  site deploy is a path-filtered workflow); (b) a separate repo (cleaner
  separation, but the screenshot pipeline and brand assets would need
  syncing, and the repo goes public soon anyway).
- **F4 — languages**: **(a) English now, structure multilingual-ready, `ru`
  when content stabilizes — recommended** (content is written once, not
  twice, while the site is young); (b) en + ru from day one.
- **F5 — IaC flavor**: **(a) CloudFormation — recommended** (zero new
  tooling: the `aws` CLI deploys it); (b) Terraform/OpenTofu; (c) console
  by hand (rejected: unreviewable).
- **F6 — how far this track goes right now**: (a) **stage S1 only: scaffold
  + design + local preview, then review — recommended**; (b) S1–S3 straight
  through first deploy (infra + DNS + CI); (c) stop after this document.

**User's decisions (2026-08-10):** F1(a) — S3 + CloudFront pay-as-you-go;
F2(a) — Zola 0.23.x with custom templates; F3(a) — `site/` in this
repository; F4(a) — English now, multilingual-ready structure, `ru` when
content stabilizes; F5(a) — CloudFormation (implied by the accepted F1
wording); F6(a) — stage S1 first: scaffold + design + local preview, AWS
untouched until the design is reviewed.

## 8. Stages

- **S1 — scaffold and design** (`feat/website-scaffold`): `site/` with
  config, templates, Sass on the brand palette, landing + blog + articles
  skeletons, first news post; local `zola serve` preview. Gate: user
  reviews the design live. **Done 2026-08-10 — design approved**, with one
  review amendment: the terminal chrome dropped its macOS traffic-light
  dots for right-aligned Windows/Linux caption glyphs (`─ □ ✕`) — the app
  does not run on macOS, and the frames must not suggest it.
- **S2 — infra** (`feat/website-infra`): `infra/website.cfn.yaml`, stack
  deployed, cert issued, DNS live, first manual `s3 sync` — https://mindfork.io
  answers. Gate: the site is up. **Done 2026-08-10 — the site is live**
  (stack `mindfork-website`, us-east-1; bucket
  `mindfork-website-sitebucket-ioafb7vyycso`, distribution `E8EC9ICZSRYKB`,
  deploy role `mindfork-site-deploy`). One measured correction along the
  way: apex and wildcard share the identical ACM validation CNAME, so
  `DomainValidationOptions` must list the apex **once** — listing both made
  the handler write the same record twice and Route53 answered 400 (the
  first stack rolled back; the template carries the comment).
- **S3 — CI deploy** (`feat/website-ci`): `site.yml` (PR build gate +
  main deploy via OIDC role from the stack). **Done 2026-08-10** — the gate
  leg ran green on PR #294 itself (the workflow sits in its own path
  filter) and the merge commit fired the first deploy: success, 15 s, the
  site answered 200 after. No stored keys — the job's OIDC token assumes
  the stack's `mindfork-site-deploy` role.
- **S4 — screenshots SVG writer + content**: the deferred stage 4 of the
  demo-screenshots track, built to the site's settled sizes/themes; content
  fill (features page, first articles), `ru` when F4 says so.
  **Done 2026-08-11** — the writer shipped (release journal, "demo
  screenshots — stage 4": true-advance grid from the font tables,
  `textLength` insurance, id-scoped styles), all ten frames are inlined on
  the landing with pure-CSS theme pairing (the image-swap script is gone),
  and the second article is up. `ru` remains open on F4's own terms.

Each stage is its own branch/PR per AGENTS.md §2; S1 needs no live-stack run
(pure static output — stated explicitly per AGENTS.md §3), S2's "live run"
is the deployed site itself.

## 9. Sources

- AWS: [serve a static website](https://repost.aws/knowledge-center/cloudfront-serve-static-website) ·
  [OAC](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/private-content-restricting-access-to-s3.html) ·
  [cert must be us-east-1](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/cnames-and-https-requirements.html) ·
  [directory indexes via CloudFront Functions](https://aws.amazon.com/blogs/networking-and-content-delivery/implementing-default-directory-indexes-in-amazon-s3-backed-amazon-cloudfront-origins-using-cloudfront-functions) ·
  [CloudFront pricing](https://aws.amazon.com/cloudfront/pricing/pay-as-you-go/) ·
  [flat-rate plans](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/flat-rate-pricing-plan.html) ·
  [Route53 pricing](https://aws.amazon.com/route53/pricing/) ·
  [Amplify pricing](https://aws.amazon.com/amplify/pricing/) ·
  [GH OIDC](https://docs.github.com/en/actions/deployment/security-hardening-your-deployments/configuring-openid-connect-in-amazon-web-services) ·
  [configure-aws-credentials](https://github.com/aws-actions/configure-aws-credentials)
- Zola: [releases](https://github.com/getzola/zola/releases) (v0.23.6,
  2026-09-12; the site pins it) ·
  [CHANGELOG](https://github.com/getzola/zola/blob/master/CHANGELOG.md)
  (the 0.23 breaking notes) ·
  [multilingual](https://www.getzola.org/documentation/content/multilingual/) ·
  [search](https://www.getzola.org/documentation/content/search/) ·
  [installation](https://www.getzola.org/documentation/getting-started/installation/) ·
  [Docker image](https://github.com/getzola/zola/pkgs/container/zola) ·
  themes: [terminimal](https://github.com/pawroman/zola-theme-terminimal) ·
  [tabi](https://github.com/welpo/tabi) ·
  [abridge](https://github.com/Jieiku/abridge)
- Others: [mdBook](https://github.com/rust-lang/mdBook) ·
  [Cobalt](https://github.com/cobalt-org/cobalt.rs) ·
  [Marmite](https://github.com/rochacbruno/marmite) ·
  [Astro 6](https://astro.build/blog/astro-6/)
