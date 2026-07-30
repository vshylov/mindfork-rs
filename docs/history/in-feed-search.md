# In-feed text search — design plan

**Status:** **CLOSED** 2026-07-29 — shipped as `Ctrl+F` (not `/`, see §1.1); forks decided the same day. Outcome lives in `CLAUDE.md` and the roadmap's "Recently closed". Closes the roadmap item *"In-feed text
search"*, the last piece left by the chat-content-search track
([research](../research/chat-content-search.md),
[stage 2](chat-search-stage2.md), fork S5 kept it out deliberately).

Search inside the **open chat**: type, see matches highlighted, step through them.
Cross-chat search (`Ctrl+G`) answers *"which conversation was that in?"*; this
answers *"where is it on this page?"* — the browser's `Ctrl+F`, not a database
query. Nothing here touches `cache.db`: the feed already holds its own text.

---

## 1. What the investigation found (2026-07-29)

Four findings shape the work. One invalidates the roadmap's own wording, one is
the bulk of the effort, and one shows the cheap version of next/prev is visibly
broken on real data.

### 1.1 `/` cannot be the trigger — the roadmap is wrong

Both `docs/roadmap.md` and stage 2's fork S5 specify this feature as
"`/`-search". **It is not implementable as written**, for a reason no gating trick
rescues: in the chat screen the input box is always focused, and **typing `/` into
an empty box is exactly the gesture that starts a command** (`/rag`, `/file`,
`/tts`, `/reindex` — parsed on `Enter`). A `/`-on-empty trigger would make command
entry impossible.

The settings screen *does* use `/`, but only because it has a top-level state with
no focused editor — `keys::is_slash_key` documents that precondition itself. The
chat screen never has it. The docs must be corrected along with the code.

### 1.2 The highlight is in `CacheKey` — that is the main piece of work

Highlighting every message instead of one is a one-line change (drop the
`if marked` gate). But `highlight` is part of `CacheKey`, and a key mismatch
clears the **entire** block cache:

> every keystroke in an incremental search field would re-run markdown + syntect +
> LaTeX + mermaid + table layout + wrap over the whole chat — up to 70 blocks and
> 260 K characters.

That is exactly the cost the block cache exists to avoid. The fix is to take the
highlight **out of the cache** and apply it in `render` over the already-cloned
lines. Two consequences: it must then skip the rail span (span 0) and offset its
ranges, because stage 2 deliberately highlights *before* the rail is prepended;
and it can be limited to the visible window.

### 1.3 Message-granular next/prev is visibly broken on real data

The feed can only scroll to a block's **first** line — there is no intra-block
offset anywhere, and stage 2's fork S6 rejected storing one because a rewrap
changes the block's height.

The largest single message in the real corpus is **38,782 characters** — several
hundred wrapped rows. "Next match" inside it would leave the viewport completely
unmoved, repeatedly. So next/prev needs a row resolved *within* a block. That is
tractable precisely because it can be **re-derived every render** instead of
stored, which is what S6 objected to.

### 1.4 Scale: hundreds of matches in a single chat

A common word yields **~200–300 matches inside one chat** (measured on three real
chats), and including tool payloads adds 30–40%. For calibration, the cross-chat
screen caps at 200 hits *across all chats*. So a match **counter** ("3 / 247") is
close to mandatory, and "step through them" means stepping through hundreds.

### 1.5 Two traps that fail silently

- **Paste coalescing.** Any run of ≥2 characters typed quickly becomes a paste
  `Chunk` routed to `ChatScreen::handle_paste`, which early-returns for open
  popups and otherwise inserts into the **message** box. A new mode not added to
  that guard list means **fast typing into the search field lands in the message
  you were writing**. `handle_mouse` has the same guard list.
- **`activate_chat` knows nothing about new state.** `Ctrl+E`, `Ctrl+R`, a rewrite
  round, and a cross-chat jump into the already-open chat all funnel through it;
  it renumbers the feed, calls `clear_focus()` and overwrites the input box. Any
  search state held on `ChatScreen` would survive that untouched and become stale.

---

## 2. Proposed staging

- **Stage 3a — move the highlight out of the cache.** Behaviour-preserving: the
  stage-2 jump highlight looks identical, but changing the query no longer
  invalidates the block cache. Its own measurable property (a query change costs
  no re-render), and it is the prerequisite for anything incremental.
- **Stage 3b — the search mode.** `Ctrl+F`, the query field, matches highlighted
  across the whole feed, a counter, and next/prev with a row resolved inside the
  block.

---

## 3. Forks — **decided by the user 2026-07-29**

All adopted as recommended (F3, F5 and F6 were stated rather than asked, their
recommendation being unambiguous):

| | decision |
|---|---|
| **F1** trigger | **`Ctrl+F`** in the chat screen; the help entry gets split per screen |
| **F2** what matches | **Everything drawn on screen** — counter equals the highlights |
| **F3** query field | **In place of the input box**, so the feed does not rewrap |
| **F4** next/prev | **The matched line**, re-derived every render |
| **F5** keys | `Enter`/`↓` next, `Shift+Enter`/`↑` previous, `Esc` closes; the query is remembered per chat |
| **F6** on close | `Esc` clears the highlight — a highlight with no visible search box would be unexplainable |

The `/` wording in `docs/roadmap.md` and in stage 2's fork S5 is **factually
wrong** (§1.1) and must be corrected as part of this work, not left standing.

The full options and their trade-offs are kept below as the record of *why*.

---

## 3a. Forks for decision (the record)

**F1 — the trigger key** (`/` is impossible, §1.1).
(a) **`Ctrl+F`** — free in the chat screen, and what every application uses for
"find on this page". Cost: the chat's help dialog currently lists `Ctrl+F` as
*chat-list* content search, so that entry must be split per screen — `Ctrl+G` is
already double-listed, so the precedent exists. *(recommended)*
(b) `F4` — completely unused anywhere in the codebase, no conflict at all, but
nobody's finger goes there for "find".
(c) `Ctrl+S` — free, but risks XOFF on some terminals.

**F2 — what counts as a match.**
(a) **The rendered lines** (the cached block lines, the same text the highlight is
applied to). The counter then equals exactly what is highlighted on screen. Two
honest consequences: the count changes when `Ctrl+T` expands or collapses
thoughts, and a match split across a wrap boundary is missed — but it is missed by
the highlight too, so they never disagree. *(recommended)*
(b) `FeedMessage.text` only — a stable count matching the FTS index's scope
(stage 1's fork F3), but then the counter and the visible highlights disagree,
because the existing highlight also covers thoughts and tool cards.

**F3 — where the query field goes.**
(a) **In place of the input box** while search is open, as the impersonation
preview already does — the feed's geometry is untouched, so opening search does
not rewrap the whole chat. The half-typed message is preserved (every existing
mode already preserves it). *(recommended)*
(b) A fifth row in the layout — conventional, but it shrinks the feed area, which
changes the wrap width and rewraps everything on open *and* close.
(c) A floating overlay — no relayout either, but it covers content you may be
searching.

**F4 — next/prev granularity.**
(a) **The exact match's row**, re-derived each render (§1.3). More work, but the
cheap alternative is visibly broken on a 38 K-character message. *(recommended)*
(b) The message containing the match — free (the jump exists), but repeatedly
pressing "next" inside one long message would not move the viewport.

**F5 — keys inside the search mode.** `Enter` = next, `Shift+Enter` = previous,
`↑`/`↓` = previous/next as well (the field is single-line, so the arrows are free
there), `Esc` closes. `Ctrl+F` again — closes, or re-opens with the previous
query? *(recommendation: `Esc` closes and clears the highlight; the query is
remembered for the next `Ctrl+F` in the same chat, the way the emoji picker
remembers its selection.)*

**F6 — does the highlight survive closing the search?** *(recommendation: no —
`Esc` clears it. A stray highlight with no visible search box would be
unexplainable.)* Note the in-feed highlight and the cross-chat jump highlight
share one slot on `MessageFeed`; whichever acts last wins, and `activate_chat`
clears both.

---

## 4. Out of scope

Regex or whole-word matching (the matcher is the shared `match_ranges`, ≥3
characters, case-insensitive — the same rule as everywhere else); searching
collapsed thoughts without expanding them; persisting a query across chats.

## 5. Highlighting text the renderer transformed — **rejected, 2026-07-30**

Fork **S3(c)** — thread source ranges through the renderer so a match survives
LaTeX→unicode, table layout and the rest — was left open here. It was measured
before being built, exhaustively rather than by guessing queries: for every
message in the dev corpus, every alphanumeric run of ≥3 characters in the source
(what the index can match) checked against the rendered output (what the feed can
highlight).

**88 of 186 805 words (0.047%), across 20 messages of 1213.** The words decide
it: `rightarrow` (the LaTeX command name, not the `→` on screen), `e0f2fe` (a hex
colour in a mermaid `style` line), `graph lr`, `500px`, `td`. Nobody searches for
those, and the prose around them highlights fine. S3(c) would rework
`writer`/`latex`/`table`/`code` and the mermaid path to buy them.

The measurement did find something, though — 68 of the 88 came from a *different*
defect in one message: block-level raw HTML was dropped by the renderer, prose
included, so it was invisible in the feed rather than merely unhighlightable.
That was worth fixing on its own (spec §11.4), and it took the gap to **65**
words. The 23 recovered are prose; nearly everything left is markup, formula and
diagram syntax that is deliberately not shown.
