# Making mindfork findable — the promotion plan

Status: **plan accepted by the owner on 2026-10-03; stage 0 in progress.** A
living document: an item's status changes here when it closes, and a channel
tried gets its outcome written next to it. Decisions are in §8.

## 1. Context

The application is far along — fourteen public releases since 0.10.0, Windows
and Linux, six providers, a site with nine articles — and nobody uses it. The
one step taken towards an audience before this plan was making the site
indexable: Google Search Console, Bing Webmaster Tools, IndexNow, structured
data, `/llms.txt` ([docs/journal/website.md](../journal/website.md)).

Indexing alone does not bring users, for two reasons:

- **Search serves people who already search for something.** Nobody searches
  for the name yet, and the generic queries ("terminal AI chat", "llama.cpp
  TUI") are held by established projects — measured on 2026-10-03:
  `simonw/llm` ★12.6k, `sigoden/aichat` ★10.5k, `gptme/gptme` ★4.4k,
  `darrenburns/elia` ★2.5k, `ggozad/oterm` ★2.4k, `pythops/tenere` ★0.7k. A new
  domain with no inbound links ranks under all of them for months.
- **Developer tools are found through communities and curated lists**, then
  through the social proof those produce (stars → GitHub's trending and topic
  pages → more stars). None of those channels has been opened.

There is a second reason this matters beyond users: Windows code signing waits
on SignPath's *Reputation* field — "stars, downloads and discussion accumulate →
apply" ([roadmap](../roadmap.md), *First run, setup and distribution*;
[code-signing.md](code-signing.md)) — and the winget manifest waits on the
signing. Promotion is what unblocks both.

## 2. Baseline (2026-10-03)

GitHub keeps traffic for **14 days only**, so this snapshot is the only record
of the starting point.

| Metric | Value | Note |
|---|---|---|
| Repository views, 14 days | 340 views, **4 unique** visitors | top paths `/pulls`, `/actions`, `/releases` — the owner |
| Clones, 14 days | 2 686, 448 unique | CI and bots, not people |
| External referrers, 14 days | github.com 2, mindfork.io 2 | nothing else |
| Stars / forks / watchers | 0 / 0 / 0 | |
| Issues, ever | 0 | |
| Release downloads (no checksums) | 0.14.1: 15 (one day old) · 0.14.0: 39 · 0.13.0: 76 · 0.12.0: 84 · 0.11.x: 39–43 · 0.10.x: 13–30 | |
| crates.io | 101 in total, ~10 per version | the background of mirrors and bots |
| Search Console / Bing | not captured | the owner's accounts |

Re-measure with:

```
gh api repos/vshylov/mindfork-rs/traffic/views --jq '{count,uniques}'
gh api repos/vshylov/mindfork-rs/traffic/popular/referrers
gh repo view vshylov/mindfork-rs --json stargazerCount,forkCount
gh api repos/vshylov/mindfork-rs/releases --jq '.[] | .tag_name + " " + ([.assets[] | select(.name|test("sha256")|not) | .download_count] | add | tostring)'
curl -s -A "mindfork-research" https://crates.io/api/v1/crates/mindfork | python -c "import sys,json; print(json.load(sys.stdin)['crate']['downloads'])"
```

## 3. Diagnosis

The product is invisible, not rejected — nobody has seen it to reject it. What
stands between a passer-by and a first run:

- **No channel** has been opened (§1).
- **The hook is generic.** The site's hero says "An AI chat that lives in your
  terminal" — the category of a dozen projects above. What no other one does is
  in the README's blockquote: a local Gemma or Qwen that keeps notes about you
  and a self-model it revisits, and grows more interesting to talk to.
- **Nothing moves.** The README shows still screenshots; a terminal app is
  judged by how it feels in motion.
- **Friction on the first try**: no macOS at all (§5); on Windows an unsigned
  installer and no package manager; on Arch no AUR package; Ollama and LM
  Studio — what most local users already run — appear only in a parenthesis.
- **The repository's own storefront is bare**: no topics (GitHub's search and
  topic pages key on them), the default social preview, Discussions off.

## 4. The plan

Stages run in order because the one-shot channels of stage 2 must not be spent
before stages 0–1 have removed what a first visitor would bounce off.

### Stage 0 — the storefront

| Item | Who | Status |
|---|---|---|
| Repository topics (`rust`, `tui`, `ratatui`, `llm`, `local-llm`, `llama-cpp`, `ollama`, `mcp`, `rag`, …) | agent, `gh` | approved 2026-10-03 |
| Discussions enabled | agent, `gh` | approved 2026-10-03 |
| Social preview = `assets/og-card.png` (1200×630) — no API exists, Settings → General → Social preview | owner, one upload | approved 2026-10-03 |
| An animated demo, generated from code (§6) | agent, a track | stages 1–3 done ([demo-reel.md](demo-reel.md)): drawn by every release, played by the site from the first release that carries it, and in Russian for Habr |
| README shortened: hook → animation → install in three lines → features; the long OpenRouter paragraph moves to install.md; the stale test count goes | agent | after the first release that carries the animation — embedded before, it would be a broken image |
| The differentiator first: the site's hero and the README's first line say "memory and a self-model", not "an AI chat in your terminal" | agent | after §6 |

### Stage 1 — friction on the first try

| Item | Status |
|---|---|
| An Ollama / LM Studio recipe at the top of the README and the site (three lines); idea: the engineless chat offers a server it finds on `localhost:11434` or `:1234` | todo |
| A Scoop manifest (own bucket first) — Scoop does not need a signed binary, so Windows gets a package manager now rather than after signing | done 2026-10-03: `vshylov/scoop-bucket` ([package-managers.md](package-managers.md)) |
| AUR `mindfork-rs-bin` — the release already builds an Arch package with nfpm | ready, **blocked**: the AUR's registration is closed (2026-10-04), so there is no account to publish from; every release builds and checks it ([package-managers.md](package-managers.md) §4) |
| macOS (§5) | research first |
| winget | unchanged: waits for signing |

### Stage 2 — communities: the repeatable first, the one-shot last

Repeatable, low-risk — can start as soon as stage 0 is done:

- **awesome-ratatui** — the *AI and Agents* section (Oatmeal, a TUI LLM chat,
  is the precedent); **awesome-tuis** (★20.8k; `elia` and `tenere` are there);
  **awesome-mcp-clients** (★6.6k) — mindfork is an MCP client host.
- **Ratatui's showcase** (Discord / forum).
- **r/rust** — also the only way into *This Week in Rust*: it no longer takes
  pull requests for *Project/Tooling Updates*, its editors pick links from
  r/rust (the TWiR README, read 2026-10-03).

One-shot — only after stages 0–1, with the animation and an honest macOS
answer in hand:

- **r/LocalLLaMA** — exactly the audience: Gemma/Qwen on llama.cpp.
- **Show HN.**
- **Habr**, in Russian — the full `ru` locale is a rare asset; the animation's
  `ru` variant (§6) is made for it.

Later: **awesome-rust** (★59.7k) accepts a project with more than 50 stars or
2 000 crates.io downloads (its CONTRIBUTING.md) — a milestone, not a step.

Rules for every post:

- **Written by the owner, in the owner's voice.** TWiR asks for LLM authorship
  to be disclosed; HN and Reddit react badly to text that reads as generated.
  An agent's draft is raw material, not the post.
- What it is, what is different, the honest limits (no macOS yet), one
  animation — and the author in the comments for the first hours.

### Stage 3 — a steady flow

- **Articles that answer what people type into a search box.** Release posts
  bring no search traffic — nobody searches for a version number. The journals
  hold measured findings that people do search for, error text included:
  CUDA 13 aborting at warmup on a B200 ([engine journal](../journal/engine.md),
  "the CUDA kernel cache lives with the data" and the pod entries before it);
  `claude-haiku-4-5` refusing every turn with a `400` (same journal, "thinking
  on the Claude 4.5 generation"); `gemini-3.7/3.8-flash` refusing `minimal`
  and ten OpenAI models taking five different effort sets
  ([effort-tiers.md](effort-tiers.md) §2); four model catalogues that agree on
  nothing ([model-picker.md](model-picker.md)). Each one links back to the
  application.
- **Notable releases go to r/rust**, not every patch.
- **The first issues get fast answers**; `good first issue` labels once a
  contributor appears.

## 5. macOS without a Mac

The owner has no Mac, so the platform is designed around three layers of
testing, none of which needs one on the desk:

- **Scope.** For a terminal application, Apple's devices are Macs: an iPhone
  or iPad runs no native terminal binaries. Apple Silicon is one target,
  `aarch64-apple-darwin`; Intel Macs are left out at first (Apple announced
  macOS 26 as the last release for Intel Macs).
- **CI** — GitHub's Apple Silicon macOS runners, free for a public repository:
  the build, the unit tests, the headless gates (small windows, screenshots),
  `llama setup` and a managed `llama-server` on the CPU with a tiny model.
  Metal is believed unavailable on those runners — to be verified.
- **A rented Mac for a day** — Apple's licence leases a cloud Mac for 24 hours
  at the least (AWS EC2 Mac, on the account the site already uses; Scaleway;
  MacStadium). One such day before announcing macOS: Terminal.app, iTerm2 and
  Ghostty, the keys, the clipboard, Metal.
- **Testers** — macOS shipped as a *preview*, with Mac users from
  r/LocalLLaMA asked to report.

Traps already known:

- `Ctrl+←/→` switch Spaces by default in macOS, so the word navigation never
  reaches the application; Option-as-Meta is a per-terminal setting.
- Gatekeeper quarantines a binary downloaded **in a browser** until it is
  signed and notarized (the Apple Developer Program, $99 a year; doable from
  CI). `curl | sh`, a Homebrew formula and `cargo install` do not quarantine,
  so a first macOS release can come without notarization.
- The machine-bound encryption of API keys needs a macOS source for the
  machine's identity.

Next step: a research document of its own (`docs/research/macos.md`) whose MVP
probe is a CI job — build and unit tests on an Apple Silicon runner — with a
go/no-go.

## 6. The animated demo — a track

The demo-screenshots plan foresaw it as an optional stage 4: "replaying the
scripted stream frame by frame into an APNG/GIF"
([demo-screenshots.md §4](../history/demo-screenshots.md)). It also closes the
roadmap's "a demo recording" item
([public-release-readiness.md §2.4](public-release-readiness.md)).

Shape, as accepted on 2026-10-03 (the details and the remaining forks are in the
track's own design document, [demo-reel.md](demo-reel.md)):

- **Generated from code, never recorded by hand** — the application changes
  too fast for a manual recording to stay current.
- **A storyboard driven through the real UI path**: keys through the runtime's
  key dispatch, the engine's events through the same function that applies them
  in a session, frames drawn by the same composition — onto a `TestBackend`, as
  [demo_shots.rs](../../src/app/demo_shots.rs) does for the stills. No model,
  no network; every step a frame and a duration on a virtual clock, so the
  sequence is deterministic. Ordinary tests hold the storyboard to the code.
- **Rendered by `tools/screenshots.py`** (Pillow, already its dependency): a GIF
  for the README, an animated WebP for the site.
- **Built in CI at release time and published to mindfork.io**; the README
  embeds it by absolute URL. Nothing binary is committed, no gate turns red on
  every interface change, and the animation always shows the released version.
- **The scenario** (~25 s): a question typed → the thoughts stream and fold →
  the answer with a table → a `note_save` card → `F3`, where the self-model
  holds the new observation. It shows the differentiator (§3) in motion.
- **English and Russian** (the Russian one for Habr), dark; light optional.
- **The main risk**: anything in a frame that reads the wall clock makes the
  frames differ between runs. The spinner already takes its time as a
  parameter; the rest of the chat path is to be checked.

## 7. Measuring

- **Weekly**: unique visitors and referrers, stars, release downloads (§2's
  commands); Search Console impressions and clicks; CloudFront's own reports
  (top referrers, popular objects) — no tracking code on the site, in keeping
  with [PRIVACY.md](../../PRIVACY.md). Idea: a small script that appends the
  week's row to a file.
- **First milestones**: the first issue from a stranger; 50 stars — the
  threshold of awesome-rust, and a reputation SignPath can be shown.

## 8. Decisions (the owner, 2026-10-03)

- The plan as a whole: **accepted**.
- Repository topics, the social preview and Discussions: **approved**.
- The animation: **generated automatically from code, at release, not
  committed** — the recommendation in §6, accepted.
- macOS: **no Mac on hand** — tested through CI, a rented Mac and testers, to
  be designed in its own research document (§5).
- Order: the animation track first, then the README and the site's hero with
  the Ollama recipe, then the macOS research.
