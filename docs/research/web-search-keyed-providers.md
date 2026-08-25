# `web_search` under anti-bot: a detection bug, and keyed providers

Research, pre-decision (AGENTS.md §1). Area: tools —
[architecture §8](../architecture.md), [spec §9.3.1](../../spec.md),
[journal/tools.md](../journal/tools.md).

**The ask.** `web_search` succeeds in roughly half to two thirds of calls; on the
rest the model announces that search is being cut off by anti-bot measures and
that it will go straight to the primary sources it already knows — and works
around the tool. Can this be fixed — perhaps by adding providers that take an
API key?

**The short answer.** Yes, and there are two separate problems wearing one coat.
One is a **defect**: a blocked provider is currently reported to the model as
*"the search returned no results"*, which is the single worst thing this tool can
say. The other is the honest capacity problem, and there the ask is right — the
keyless space is exhausted, and a key is the only durable fix. The defect is
cheap, independent, and should land first.

---

## 1. What was measured

All numbers below are from the user's own machine (residential IP, Windows),
**2026-08-25**. Method: replicate `web.rs::fetch` request-for-request against
each provider, then read the app's own live smoke. Probe scripts are throwaway
(scratchpad); what they showed is reproduced here.

### 1.1 The chain under agentic-loop load

Six back-to-back requests per provider, 0.4 s apart — the pattern of the
transcript in the report, where the model issued four `web_search` calls in one
round and the loop runs them one after another
(`generation.rs`, `for call in &out.calls`):

| Provider | Behaviour | Detected today? |
|---|---|---|
| DuckDuckGo lite | 2 clean answers, then `HTTP 202` + `anomaly` | yes |
| DuckDuckGo html | `HTTP 202` immediately (same infrastructure as lite — one throttle covers both) | yes |
| Mojeek | 2 clean answers, then **`HTTP 200`, `<title>Captcha</title>`** | **no — see §2** |
| Ecosia | `HTTP 403` | yes |

**Recovery is far slower than the code assumes.** `web.rs` documents throttling
as "short-lived (per-IP)". Measured: **fifteen minutes after all load stopped,
every one of the four still blocked** — DDG lite and html `202`, Mojeek the
`200` captcha, Ecosia `403`, on two different queries. A minute-scale cooldown
would be too short to be worth having.

**One question this IP could not settle.** With every provider serving a block
page, there is no way to tell a *stale CSS selector* from a *blocked response* —
both parse to zero results. `a.result-link`, `a.result__a`, `a.title` and the
Ecosia `data-test-id` set are therefore **unverified**, and PR1 must check them
from a clean IP before assuming §2 is the whole story. Recording the gap rather
than guessing past it.

Two further conclusions:

- **The chain is three independent providers, not four.** DDG lite and DDG html
  share a throttle, so the first two entries are one slot. When DDG and Mojeek
  are spent, only Ecosia remains — and Ecosia was the first to 403.
- **Every call restarts the chain at DDG lite.** Once DDG is throttled, each
  subsequent search in the turn pays two dead round-trips before reaching a
  provider that could answer. That is both latency and additional pressure on a
  throttle that is already sticky.

### 1.2 The app's own live smoke

`cargo test --bin mindfork live_search_returns_results -- --ignored`:

<!-- cyrillic-ok:start — the failure text *is* the measurement: this is the
     literal `tool.web_search.result.no_results` bundle value the run printed,
     and paraphrasing it into English would lose which branch was taken. -->
```
test features::tools::web::tests::live_search_returns_results ...
panicked: got: Поиск не дал результатов.
```
<!-- cyrillic-ok:end -->

It failed with **"no results"** — not with the "every provider is throttling us"
error the smoke is written to skip on. The smoke's own comment says a throttle is
an infrastructure condition rather than a regression; it never fired, because the
tool did not know it was blocked.

---

## 2. The defect: Mojeek's captcha is invisible again

`is_challenge_page` looks for five whole phrases in the body
(`web.rs::CHALLENGE_MARKERS`), added after the last time this bit
(journal/tools.md, P4 — *"Verification required. Please complete the
challenge"*). Measured today, Mojeek's block page contains **none of them**:

```
mojeek: HTTP 200  title='Captcha'
   current CHALLENGE markers present: []
   'captcha' anywhere: True   'anomaly': False
```

So the page parses to zero results, is not recognised as a challenge,
`got_clean_page` is set, and `no_results_outcome` takes the honest-emptiness
branch. The model is told the web has nothing on the subject — and reasonably
concludes the tool is useless and routes around it. This is exactly the
recurring class in [lessons §4](../lessons.md): *a message must close the door*,
and this one closes the wrong one.

The phrase list was the right shape of fix and the wrong anchor. A blocked page's
**`<title>`** is a far more stable signal than its prose:

| Page | `<title>` |
|---|---|
| Mojeek block | `Captcha` |
| DDG block | `DuckDuckGo` (already caught by 202/`anomaly`) |
| Cloudflare interstitial | `Attention Required! \| Cloudflare` |
| A real Mojeek results page | `<query> - Mojeek Search` |

**Proposal.** Classify on the title element, not on body prose: a title that
*is* `captcha`/`verification required`/`attention required`/`just a moment`
(equality or prefix, not substring-anywhere) means blocked. This is strictly
narrower than the current check — snippet text can no longer reach the
classifier at all, so the "a search for *captcha* keeps working" guarantee gets
stronger, not weaker — and it survives the next rewording of the body. Keep the
existing phrase list as a second signal; it costs nothing and covers pages whose
title is generic.

**This is a bug fix, it needs no key, no setting and no user decision, and it
helps every user of the tool.** It should land on its own branch first.

---

## 3. The keyless space is exhausted

One request each to every keyless candidate worth trying, same machine, same day:

| Candidate | Result |
|---|---|
| Yep.com JSON API | `HTTP 403` (Cloudflare) |
| Marginalia | `HTTP 200`, but the body is the *"Wait A Moment"* queue interstitial, not results; index is small and indie-scoped |
| Stract | `HTTP 404` at the documented search path |
| Startpage | `HTTP 200` with a captcha page |
| Brave (HTML, no key) | `HTTP 429` + captcha |
| Public SearXNG instances (`searx.be`, `searxng.site`, `priv.au`) | JSON format disabled or `HTTP 403` |

Nothing here is worth a slot in the chain. **The user's instinct is correct: a
key is the only durable fix.** What remains free is worth doing, but it raises
the hit rate rather than removing the ceiling — see §5.

---

## 4. Keyed providers — the survey

Verified 2026-08-25 against each vendor's own pricing/API pages.

| Provider | Free tier | Paid | Card for the free tier | Auth | Returns page content |
|---|---|---|---|---|---|
| **Tavily** | 1 000 credits/month, **no credit card** | $0.008/credit (≈ $8 / 1 000) | no | `Authorization: Bearer tvly-…` | **yes** — `include_raw_content` returns cleaned, parsed text |
| **Brave Search API** | $5/month of credits (≈ 1 000 requests) | $5 / 1 000 | **yes** (anti-fraud verification; not charged) | `X-Subscription-Token` | snippets only |
| **SearXNG**, self-hosted | free, unlimited | — | — | none | snippets only |
| Serper | 2 500 queries, no card | per-query, cheap | no | `X-API-KEY` | snippets (Google SERP) |
| Google Custom Search JSON | 100/day | $5 / 1 000 | — | `key` + `cx` | snippets |
| Anthropic `web_search` server tool | — | $10 / 1 000 + tokens | uses the existing key | Messages API | **no** — `encrypted_content` is opaque to the caller |

Two entries are **rejected outright**:

- **Google Custom Search JSON API** — closed to new customers since 2025 and
  discontinued **2027-01-01**. A new integration would be dead on arrival.
- **Bing Web Search API** — retired **2025-08-11**. Microsoft's replacement
  ("Grounding with Bing Search" inside Azure AI Agents) is not a search API at
  all: it hands web context to an agent rather than returning results.

**Serper** works and is cheap, but it resells Google SERPs; that is a licensing
question this project should not take on for a shipped default. Keep it as a
possible later slot, not a stage-1 one.

**The cloud providers' own search tools** (Anthropic `web_search_20260209`,
Gemini grounding, OpenAI Responses `web_search`, Grok Live Search) deserve their
own line, because the four cloud keys already exist here. They are a poor fit
for *this* tool, for three reasons that hold across all four: a search costs a
whole model round trip, not an HTTP request; the results come back **without
readable content** (Anthropic's `encrypted_content` can only be handed back to
Anthropic), so the tool would still fetch every page itself; and it welds
`web_search` to a cloud key, which is exactly what a local-first app should not
do. Worth revisiting only as a fallback slot.

**Recommendation: Tavily first, Brave second, SearXNG as the self-hosted
escape hatch.** (Written pre-decision. Brave was built and then removed on
availability grounds — see §7 F2.) Tavily because the free tier is real and cardless, the response
is shaped for exactly this use (title, url, snippet, score, and optional cleaned
page text), and the returned content lets the tool **skip its own N parallel page
fetches** — a latency win and one less thing for a site's anti-bot to see. Brave
because it is a genuinely independent index rather than a reseller.

### 4.1 SearXNG deserves special mention here

This machine already runs the reference stack on the LAN (`llama-server`, the
embedder). A SearXNG container beside them gives unlimited, key-free,
anti-bot-free search with an official JSON API (`/search?q=…&format=json`,
with `formats: [json]` enabled in `settings.yml` — public instances disable it,
which is why §3 saw 403s, but a private one is a two-line config).

There is an **address-policy interaction** that must be got right: a LAN SearXNG
URL is a private address, and `tools.web_allow_private` is off by default. But
this URL is typed by the *user in settings*, not chosen by the model — the same
category as the engine addresses, which
[fetch-url-address-policy.md](fetch-url-address-policy.md) already exempts by
design. So the SearXNG request goes through the **unguarded** client, while the
result pages it returns keep going through the guarded one. That distinction is
the whole safety argument and needs a comment and a test, not a config flag.

---

## 5. Free-side work that still pays

Independent of any key, and each one small:

- **F-1 — provider cooldown memory.** Remember "this provider threw a throttle at
  time *T*" for a few minutes and move it to the back of the order instead of
  re-hitting it. Today every call restarts at DDG lite and pays two dead
  round-trips (§1.1). Sticky per-IP throttling is already the documented model in
  `web.rs`; this just stops fighting it.
- **F-2 — count DDG once.** lite and html share a throttle, so "all providers
  unavailable" is reached one provider later than it should be, and the order
  spends two slots on one infrastructure.
- **F-3 — browser-like headers on the provider request.** `fetch` sends only
  `User-Agent`; `fetch_content` already sends `Accept` and `Accept-Language`. A
  request with a Chrome UA and no `Accept` is a classic bot signature. **Expect
  little from this, and do not sell it as the fix.** Two measurements argue
  against it: the intended A/B was confounded by throttle carry-over between
  passes and measured nothing, and — the telling one — once the IP was in the
  penalty box, **a real Chrome driving the same page got the same block**
  (`lite.duckduckgo.com` → `anomaly` present; `mojeek.com` → `<title>Captcha`).
  So this is IP-rate blocking, not fingerprinting: better headers might delay the
  trigger, they cannot recover from it. Worth doing as hygiene, worth no
  promises.
- **F-4 — do not fetch content when the API already returned it** (Tavily), and
  keep the existing embedder reranking on top either way.

None of these lifts the ceiling. Together they should stop wasting the budget the
free providers do grant.

---

## 6. How it lands in the code

`PROVIDERS: &[Provider]` is today a table of *HTML-scraping* descriptions
(endpoint, method, three CSS selectors). Keyed providers return JSON, so the
table needs to become a small sum type rather than gain nullable fields:

```
enum Backend {
    Scraped(&'static Provider),   // today's four, unchanged
    Api(ApiProvider),             // tavily | brave | searxng
}
```

Each yields `Vec<SearchResult>` and the same three-way outcome the loop already
understands (`results` / throttled / error), so `run_providers`,
`no_results_outcome`, `enrich_with_content` and `rerank_by_embeddings` keep their
shapes. Concretely:

- **Order**: configured keyed backends, then the keyless chain. With no key
  configured the request path is **byte-identical to today** — that is the
  invariant to pin with a test.
- **Config** (`shared/config.rs`, `ToolSettings`): `web_provider`
  (`auto` | `tavily` | `brave` | `searxng` | `free_only`) and `web_searxng_url`
  — the `web_*` prefix the three existing switches already use.
  Global, like the rest of `tools.*` — not per-profile; the profile toggle is
  about *whether* the model may search, not *how* the search is bought.
- **The key** (`shared/secrets.rs`): `SecretKey::Search(SearchSlot)`, mirroring
  `SecretKey::External(ExternalSlot)` exactly — a closed set of slots, an
  unambiguous `search-<slot>` storage name, machine-bound like every other key
  (ADR 0008). The precedent is a direct fit: an arbitrary provider addressed by
  *slot* is what `ExternalSlot` already solved.
- **Settings UI**: the existing `ui.settings.group.websearch` group gains a
  Choice row plus the API-key row and its `…_API_KEY` env-fallback row — the same
  triple every engine sub-section already renders (`catalog.rs`).
- **Failure semantics unchanged**: a keyed provider answering 401/402/429 falls
  through to the next backend; all backends failing still produces the honest
  "everything is unavailable" error rather than "no results". A **401 is worth
  distinguishing** in the message, though — a bad key is a user problem, and
  telling the model "throttled" would hide it.

---

## 7. Forks — decided

**User's decision, 2026-08-25**, on F1–F3; F4/F5 keep their recommendations
unless contradicted.

- **F1. Scope of stage 1 — two PRs, the defect first.** PR1: the §2 detection
  fix plus the §5 free-side work — no key, helps everyone, and it must not wait
  behind a vendor discussion. PR2: the keyed backends. PR1 also carries the
  selector verification left open in §1.1.
- **F2. Which providers ship — Tavily. (Brave reversed the same day.)** Both
  were built; **Brave was then removed on the user's decision, 2026-08-25**,
  because they could not register for it at all — the service is not offered in
  every country. The reason that decided it is stronger than one person's
  access, and is why it came out rather than shipping disabled: a provider
  nobody here can obtain a key for is one the live gate can never cover, and an
  untestable second backend is a liability rather than a fallback. The card
  requirement, noted below as acceptable, turned out not to be the obstacle —
  availability was. Re-adding Brave, or any other provider, is a `SearchSlot`
  variant plus a parser under the `Backend::Api` shape, so this is not a
  one-way door. **SearXNG is not adopted** — the design in §4.1 stands as written and
  the address-policy argument it settles stays on record, but a self-hosted
  instance is a deployment this track will not ask for. Serper and the cloud
  server tools remain deferred.
- **F3. `auto` order — keyed first when a key exists.** A dead round trip
  through a blocked scraper costs seconds; a Tavily credit costs cents, and
  §1.1's fifteen-minute persistence means the scraper is likely to be blocked
  rather than merely slow. The free chain stays as the last fallback and as the
  no-key default; a `free_first` choice is not being added.
- **F4. Does the free chain stay?** Recommend **yes**, as the last fallback and
  as the no-key default. Removing it would make a fresh install search-less.
- **F5. Should `web_search` say which backend answered?** Recommend **yes, in the
  tool result** (one line). The model currently cannot tell a thin answer from a
  degraded one, and the transcript in the report shows it reasoning out loud
  about the tool's health.

---

## 8. Go/no-go for the live run — met

The keyed track's criterion, per AGENTS.md §1 (MVP probe before tiers): with a
Tavily key configured, **ten `web_search` calls in one turn, all ten returning
results**, against the reference stack — the exact load pattern that today
degrades to two.

**Result (2026-08-25): GO — ten for ten in 10.5 seconds**, every one answered by
Tavily and none falling through, measured on the same IP that every keyless
provider was still blocking at that moment. A second smoke settles Tavily's
`include_raw_content` field name live, because getting it wrong would look
exactly like success while the tool quietly fetched every page itself. And for the §2 fix, the criterion is narrower and does not need
a model at all: `live_search_returns_results` must fail with the **throttled
error**, never with "no results", when the chain is blocked.
