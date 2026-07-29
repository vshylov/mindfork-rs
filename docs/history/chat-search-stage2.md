# Chat content search — stage 2 design plan

**Status:** **CLOSED**, 2026-07-29 — stages 2a and 2b shipped, so the whole chat
content search track is complete. Historical document: the outcome lives in
`src/screens/search.rs`, `src/features/chat_search.rs` and the feed's
focus/anchor/marker fields in `src/widgets/message_feed.rs`; the behaviour is
specified in [spec.md §11.2–11.3](../../spec.md) and the running log is in the
CLAUDE.md journal.
**Research and decision:** continues
[docs/research/chat-content-search.md](../research/chat-content-search.md)
(stage 1 shipped in PR #232); forks S1–S6 below, all resolved to the recommended
option — *user's decision, 2026-07-29*.

Stage 2 is the half the roadmap deliberately coupled to the separate *In-feed
text search* item, because both need the same thing: **put the feed on a
specific message**. That piece now exists; in-feed search itself remains its own
roadmap item (fork S5).

Stage 1 answers *"which chats mention this?"*. Stage 2 answers *"where exactly,
and take me there."*

---

## 1. What the investigation found (2026-07-29)

Before designing, the feed was investigated for what a jump would actually cost.
Three findings change the shape of the work, and one kills the obvious approach.

### 1.1 The row offset is nearly free — but can only be applied during `render`

`MessageFeed.scroll` is in **visual rows**, and the per-block render cache
already holds exactly what a jump needs (`CachedBlock.lines`, indexed by feed
position). So `row_of(idx) = cache[..idx].lines.len().sum()` — arithmetic, no new
computation.

**But the cache is only valid after a `render` at the current width**, because
width and palette are the cache key and `build_lines` is what fills it. So:

> ⚠ **The obvious approach does not work.** You cannot compute the row in
> `activate_chat` and call a scroll setter — panel width and the cache do not
> exist outside `MessageFeed::render`. A jump is necessarily a **deferred
> request consumed inside the next render**.

### 1.2 `FeedMessage` carries no identity, and the projection is lossy

`FeedMessage { role, text, thoughts, tools, streaming }` — no `Message.id`, no
timestamp. And `from_messages` **merges consecutive assistant messages** of
agentic-loop rounds into one bubble, so N domain messages become M ≤ N feed
items: a many-to-one, order-dependent mapping. `Tool`/`System` messages are
dropped entirely.

The mapping is therefore **unrecoverable after the fact** — it has to be recorded
inside `from_messages`, where the merging happens. Note also that the live
streaming path never goes through `from_messages` at all (it pushes
`FeedMessage` literals), so the in-flight bubble has no id by construction.

### 1.3 Seven places yank the view back to the bottom

`activate_chat`, `push_user_message`, `begin_generation`, `push_tool_call`,
`continue_assistant`, `rewrite_assistant` and `push_note` all call
`scroll_to_bottom()`. Plain streamed text chunks do **not** — but a tool call or
an error note landing mid-turn will snap a jumped-to position away. Any "stay
where I put you" behaviour has to gate these on whether follow was already on.

### 1.4 Highlighting inside the feed by source offset is not feasible

This is the most valuable finding, and it closes off an approach the research doc
only flagged as "worth a look".

The renderer *has* `pulldown-cmark` byte ranges and **deliberately discards
them** (`writer.rs::run` passes the event on and drops the range). Worse, that
would not be enough even if threaded through: `normalize_delimiters` rewrites the
string **before parsing**, so parser ranges do not address `Message.text` at all.
Downstream, seven further transforms independently destroy the correspondence —
LaTeX→unicode substitution, mermaid diagrams replacing their source, table
re-layout, syntect→ANSI→spans, two wrapping passes, and rail prepending.

Compounding it: an assistant bubble is rendered as **several disjoint markdown
fragments** split by tool-call offsets, so a match straddling a tool card spans
two independent parses.

> ⚠ **Do not design around FTS5 `snippet()`/`highlight()` offsets landing in the
> feed.** The realistic options are post-render span matching over what was
> actually rendered, or a genuine `highlight_ranges` threaded through the
> renderer — see fork **S3**.

### 1.5 Resize is unhandled today

Nothing in `src/app/` has a `Resize` arm. On a width change the cache is wiped
and every row offset changes, while `scroll` keeps its stale numeric value and is
merely re-clamped. Only `follow == true` survives a resize correctly today. So a
jumped-to position needs an **anchor** (a feed index), not a row number.

### 1.6 A new screen has nine silent match sites

`ActiveScreen` has 4 exhaustive match sites that force a compile error, and about
nine more that compile silently via `_ =>`/`if let`/`matches!`. One of them —
the `SelfModelView` arm — **silently replaces the active screen**. A new variant
that forgets it would be stolen by an unrelated event.

---

## 2. Measurements (real corpus, 171 chats)

| query | matching messages | chats |
|---|---|---|
| a common word | 163 | 50 |
| a mid-frequency word | 14 | 4 |
| a rare proper noun | 3 | 2 |

- **`bm25` is *not* degenerate on trigram** — contrary to the assumption recorded
  in research §5, the top 8 hits scored 8 distinct, meaningfully-spread values.
  So ranking is a real option, not a foregone loss. (It ranks by trigram
  frequency, which is a weaker proxy for relevance than word frequency — but it
  is not noise.)
- **163 hits for a common word** means a flat list needs either grouping or a
  limit. This is the number that shapes fork **S2**.
- **`snippet()` behaves oddly under trigram**: the budget counts 3-grams, not
  words, so 64 "tokens" yields ~70 characters. SQLite documents 64 as the
  ceiling; this build accepted 100 and returned 106 characters. It works, but
  relying on undocumented behaviour is fragile — see fork **S4**.

---

## 3. Proposed staging

Split along the investigation's fault line: the risky, shared infrastructure
first, then the feature that rides on it.

- **Stage 2a — jump to a message.** The `from_messages` id mapping, a deferred
  focus request consumed in `render`, anchor-based position (survives resize),
  and gating the seven `scroll_to_bottom()` calls. Deliverable on its own:
  `AppEvent::ChatActivated` gains an optional focus, and the feed can be put on a
  message. Testable end to end with no new UI.
- **Stage 2b — the message-level search screen.** Query → hits with snippet, chat
  and date → `Enter` jumps. Builds entirely on 2a.

*In-feed text search* (the separate roadmap item) becomes cheap once 2a exists,
but stays out of this track unless fork **S5** says otherwise.

---

## 4. Forks — **decided by the user 2026-07-29**

All adopted as recommended (S4 and S6 were stated rather than asked, their
recommendation being unambiguous):

| | decision |
|---|---|
| **S1** where results live | **A dedicated screen** (`ActiveScreen::Search`) |
| **S2** shape and order | **Grouped by chat**, chats in the existing sort, messages in chat order |
| **S3** feed highlight | **None for now** — jump and mark the whole message |
| **S4** snippets | **Built in Rust** from the stored text |
| **S5** in-feed search | **Not in this track** — stays its own roadmap item |
| **S6** resize anchor | **The message** (feed index), not the row |

### A behaviour change that falls out of S3/S6

A jump is worthless if the next event drags the view away, so §1.3's seven
`scroll_to_bottom()` calls get split by *who asked*:

- **User-initiated — keep scrolling to the bottom unconditionally**:
  `activate_chat` (unless a focus was requested), `push_user_message`,
  `begin_generation`. You sent something; you want to see it.
- **Arriving on its own — respect `follow`**: `push_tool_call`,
  `continue_assistant`, `rewrite_assistant`, `push_note`. If you have scrolled
  away to read, a tool card landing must not yank you back.

The second half is a **user-visible change beyond the jump itself** and is worth
a CHANGELOG line on its own — it is also the behaviour people already expect of
a chat window.

The full options and their trade-offs are kept below as the record of *why*.

---

## 4a. Forks for decision (the record)

**S1 — where the results live.**
(a) **A new full screen** (`ActiveScreen::Search`), opened from the chat list's
content mode and/or a chat binding. Honest shape for a list of message hits; the
cost is the nine silent match sites (§1.6), which a checklist plus a test can
cover. *(recommended)*
(b) **A third scope in the chat list** — `Ctrl+F` cycles titles → content →
messages. Reuses the search box, list, palette and dispatch wholesale; but the
list's items become a different *kind* of thing in one of three modes, which is
the sort of overloading that later resists change.
(c) An overlay over the chat screen — cheapest, but a cross-chat result list
sitting on top of one particular chat reads oddly.

**S2 — result shape and ordering** (the 163-hit question).
(a) **Grouped by chat**: chat header, then its matching messages in chat order;
chats ordered by your existing sort. Sidesteps ranking entirely, keeps continuity
with stage 1, and reads well when one chat holds many hits. *(recommended)*
(b) **Flat, ranked by `bm25`** — measured usable (§2); "best matches first" is
what people expect from search, but trigram relevance is a weak proxy and the
order will occasionally look arbitrary.
(c) Flat, newest first — predictable, no relevance at all.
In every case a cap (e.g. 200 hits) with an honest "showing N of M" line.

**S3 — highlighting.** The results list is ours to build, so highlighting there
is exact and easy either way; this fork is about **the feed after a jump**.
(a) **No feed highlight for now** — jump and mark the whole message (e.g. its
rail), which the rail already makes natural. Cheapest, honest, no renderer
change. *(recommended for 2a; revisit once it is in use)*
(b) **Post-render span matching** — search the rendered spans and re-split them
to apply a style. Approximate (it matches what is on screen, not what was in the
source — arguably the more useful of the two), cheap, no renderer change.
(c) **`highlight_ranges` threaded through the renderer** — correct, but touches
`writer.rs`, `latex.rs`, `table.rs` and the mermaid path, and still cannot span
the disjoint fragments of a bubble split by tool cards.

**S4 — snippets.** (a) **Build them in Rust** from the text we already store:
exact offsets for highlighting, width-aware, no dependence on `snippet()`'s
trigram-token semantics or its documented 64 ceiling. *(recommended)*
(b) Use SQLite `snippet()` with a budget around 100 — less code, but leans on
behaviour beyond the documented limit.

**S5 — does stage 2 also deliver in-feed search (`/` within the open chat)?**
(a) **No** — keep it as its own roadmap item; 2a makes it cheap later.
*(recommended: it is a different interaction — same-chat, incremental, next/prev
— and bundling it would double 2b's UI surface)*
(b) Yes, fold it in, since 2a is the shared cost.

**S6 — what a jump anchors to across a resize.**
(a) **The message** (feed index): after a rewrap the view returns to the top of
that message. Simple, survives everything. *(recommended)*
(b) The matched line within the message — nicer for a long message, but a rewrap
changes the block's height, so the intra-block offset does not survive and would
need re-deriving from the match rather than from a row number.

---

## 5. Out of scope

Highlighting matches across *all* rendered content (as opposed to the focused
message); paging beyond the cap; searching thoughts/tool calls (fork F3 of stage
1 stands — `message.text` only); ranking tuned per query type.
