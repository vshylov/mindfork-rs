# Design plan — page fidelity for `fetch_url` (+ two neighbouring traps)

Status: **implemented** (one PR, branch `feat/fetch-url-fidelity`). Behaviour —
spec §9.3; the tools involved are `fetch_url`, `web_search`, `python_exec`.

## 1. Where this came from

Not from a roadmap item but from **one real chat**
(`1ec2acda-dfcf-4f55-9937-1033d99a5fad`): the user pasted
`https://docs.vlang.io/memory-management.html` and asked the assistant to read it
and compare the memory-management modes. What should have been one `fetch_url`
call turned into 20 messages — 1 `fetch_url`, 3 fruitless `web_search`es and
**6** `python_exec` rounds, including downloading the same 16 MB tarball of the V
repository twice.

The transcript is the specification for this task: each detour has a cause in our
code, and the causes chain.

## 2. The four problems, as measured

### P1 — `fetch_url` throws away code blocks and headings (the root cause)

`web::extract_readable` selects only `p, li`. Measured against the actual page:
**0 `<pre>`**, code examples live in `<div class="language-v">…</div>`, section
titles in `<h2>` — all dropped. The result the model received has visible holes:

> …define a `free()` method on custom data types: ⏎ Just as the compiler frees C
> data types…

The example between those two sentences is gone, as is every other one. The model
saw documentation whose "here is an example:" sentences lead nowhere, concluded
the page had arrived incomplete, and went looking for the source elsewhere. Every
later detour follows from this one.

### P2 — the text is truncated at 12 000 characters, silently, mid-word

The recorded result is exactly 12 000 characters of content and ends on
`…produce an unpredictable final out`. Nothing marks it as partial and nothing
can fetch the rest, so a long page is indistinguishable from a complete one.

### P3 — `python_exec` gives a fresh sandbox per call, and never says so

`WasmerSandbox::run` creates a `JobDir` per run and drops it afterwards; only
`/w` and `/sp` are mounted, and the guest's `/tmp` dies with the process. The
model wrote a 16 MB download to `/tmp/v.tar.gz` and the next call answered
`FileNotFoundError` — one wasted round plus 16 MB re-downloaded. The tool
description says "no access to the machine's files" but says nothing about state
not surviving a call.

Same defect class the journal already records twice (the by-reference attachment
block; `youtube_watch`'s unconfigured path): the message describes a situation
without stating what is possible next.

### P4 — `web_search` reported "no results" three times while it was blocked

Measured live from this machine, same queries:

| provider | response | `is_throttled` |
|---|---|---|
| DuckDuckGo lite | 202 + `anomaly` | detected |
| Ecosia | 403 | detected |
| **Mojeek** | **200 + "Verification required. Please complete the challenge"** | **missed** |

Mojeek serves its captcha with HTTP 200 and no `anomaly` marker, so
`got_clean_page` is set, the honest "all providers are throttled" error is
suppressed, and the model is told the web has nothing on V's memory management.
It then reasonably went to `python_exec` + `requests` — detour three.

## 3. Forks

**User's decision, 2026-08-03: all four, F1(a), F2(b).**

- **F1 — how far does richer extraction go?**
  (a) **`fetch_url` only** — `web_search` keeps prose-only extraction. *Adopted.*
  (b) Both. Rejected by the user: `web_search` budgets 1500 characters per
  result page, and headings plus code would eat that budget without helping
  ranking, which is what those snippets are for.
- **F2 — what to do with a page over the budget?**
  (a) A truncation marker in the result.
  (b) **Attach the page to the chat.** *Adopted.* `ChatEffect::AddAttachment`
  already exists (built for `youtube_watch(transcript:)`), and an attachment is
  already paged (`attachment_read`) and searchable (`attachment_search`) — this
  is the mechanism's second consumer, not a new one.

## 4. Sub-decisions (taken while implementing, not put to the user)

- **S1 — the threshold is `attachments.max_file_tokens`, exactly as
  `youtube_watch` uses it.** Below it the text goes into the result (the model
  needs no second call, and attaching would put the same text in the pinned block
  *and* in the history); above it, it is attached. One consequence carries over
  for free: an attachment made this way is **always by reference**, because
  inline requires `est <= max_file_tokens` — the other side of the same
  threshold. The mode still goes through `decide_mode`, so it cannot drift from
  what `/file attach` decides.
- **S2 — `summarize: true` still summarizes, and still attaches.** The summary is
  made from the head of the text (`SUMMARY_INPUT_CHARS`), and the result says the
  full page is attached. Summarizing 250 000 characters is not an option (the
  summarizer is a single-turn subagent), and a summary alone would repeat P2 in a
  politer form — the model would have no way to reach what the summary skipped.
- **S3 — a hard ceiling stays, far higher, and is now announced.**
  `MAX_EXTRACT_CHARS = 400_000` bounds what one page may put into a chat; on
  reaching it the text carries an explicit truncation line. So P2's silence is
  closed at both ends: below the ceiling nothing is lost, at the ceiling it is
  stated.
- **S4 — rich extraction keeps document order and dedupes by ancestry.** One
  selector run yields document order; a container that already emitted its
  content (a `<div class="language-*">` wrapping a `<pre>`) must not emit it
  twice, so an element whose ancestor was already emitted is skipped. Applied to
  the rich path only — the prose path stays byte-identical (F1a).
- **S5 — code and headings are exempt from `MIN_FRAGMENT_CHARS`.** That floor
  exists to drop navigation chrome from search snippets; a one-line code example
  and a two-word heading are content.
- **S6 — code is fenced with its language, whitespace preserved.** `collapse_ws`
  is right for prose and destroys code. The language comes from a
  `language-*`/`lang-*` class on the element or its inner `<code>`.
- **S7 — P4 is fixed by a challenge check that runs only on an empty parse.** A
  page that yielded results is never called a challenge, so a genuine search for
  "captcha" cannot be misread. Strictly narrower than adding the markers to
  `is_throttled` itself.

## 4a. Found by the live run, fixed on the same branch

The user re-ran the original request against the fixed build (chat
`28cdf212-932f-41ac-9e76-0e1c1c3436ce`) and it came out clean: 15 messages
instead of 20, `fetch_url` → **all four** attachment pages read in one round →
one `python_exec` call, no re-download. All four fixes visible in the transcript,
including P4 firing for real — the "every provider is throttling us" error in
place of "no results".

But the attachment came back named **"V Documentation"**, and that is the
site-wide `<title>` — measured, `docs.vlang.io` gives every page the same one,
while `<h1>` is the actual page ("Memory management" / "Concurrency"). Two pages
of one site would therefore have carried one name, and `attachment_read`
resolves a name with `.find()` — **the first match**, silently. A wrong page read
without a word is exactly the failure class this change exists to remove, so:

- **S8 — the name comes from `<h1>` first, `<title>` second.** Better names too,
  and it fixes the collision at its source for the common documentation layout.
- **S9 — a name already taken by a *different* page gets the URL's last segment
  appended.** For a site whose `<h1>` is as constant as its `<title>`. Compared
  against the turn snapshot and deterministic (no counters), so re-fetching the
  same page keeps its name and replaces its own attachment.
- **S10 — `attachment_read` reports an ambiguous name** with each candidate's
  source, instead of reading the first. The two guards above make a collision
  unlikely; this one makes it *loud* when it happens anyway, including for
  attachments that arrived some other way. Addressing by source keeps working.

## 5. Out of scope

- Tables (`<table>`) in rich extraction — a real gap on some doc sites, but it
  needs a layout decision (Markdown table vs flattened rows) of its own.
- A persistent per-chat scratch directory for `python_exec` (P3's deep fix). The
  description closes the trap; a mounted, surviving `/w` is a feature with its
  own lifecycle and cleanup questions.
- The `site:github.com/vlang/v` query from the transcript (a path inside `site:`)
  genuinely returns nothing on every provider. Secondary to the captcha, and not
  ours to fix.

## 6. Verification (outcome)

- **P1/P2 — live GO.** `live_documentation_page_keeps_its_code` against the same
  page the transcript used: **18 907 characters** extracted (against the 12 000
  truncated before), carrying `## Control`, a fenced ```v block and
  `fn (data &MyType) free()`, landing as a **4-page by-reference attachment**
  whose result points at `attachment_read`. Both claims depend on the real
  markup, which is why they are asserted live rather than on a fixture.
- **P4** — the classifier is pinned against the real interstitial text and
  against two results-page fixtures. The provider loop's three-line branch has no
  unit test: the providers are hardcoded URLs, and a captcha cannot be summoned
  on demand.
- **P3** — a description string, pinned per locale on the concrete word `/tmp`.
- **S8–S10 — live GO on the same page**: the attachment is now named
  **"Memory management"** (its `<h1>`) rather than "V Documentation" (the
  site-wide `<title>`), with everything else unchanged.
- **Mutation-tested**: dropping ancestry dedup, neutering the challenge
  classifier, dropping the attachment effect, removing the `/tmp` sentence,
  reverting `fetch_url` to prose extraction, preferring `<title>` over `<h1>`,
  dropping the name disambiguation, and dropping the ambiguity guard each fail
  exactly their own test.
- **Model-level regression — GO** (Gemma 4 31B q4_0 + bge-m3, external
  `llama-server`, `--jinja`): **26 of 26** orchestrator e2e live smokes green in
  556 s — memory/self-model/notes/graph/cross-organ links/RAG/attachments/control
  tools/i18n/MCP/tool confirmation. Run once the LAN stack came back up; two
  cloud-key smokes skipped for want of credentials, as they do.
