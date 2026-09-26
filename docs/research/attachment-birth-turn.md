# Research: an attachment born in the turn — the search it cannot have, and a cut it does not locate

**Status:** complete — stage 1 (§2–§4a) and stage 2 (§5), each live **GO**. Forks
decided by the user on 2026-09-26, all at the recommendation (the user's decision on
the split: "1 and 3 in one go, 2 as a separate stage").

**Related:** spec §9.3.1 (`fetch_url`), §9.7 (chat attachments), §9.9
(`youtube_watch`); [docs/history/fetch-url-fidelity.md](../history/fetch-url-fidelity.md)
(S3 — the 400 000-character ceiling); [docs/lessons.md](../lessons.md) §4 ("a
message must close the door", corollary *only advertise what exists*);
[docs/journal/rag.md](../journal/rag.md), [docs/journal/tools.md](../journal/tools.md).

## 1. What happened **[measured]**

Chat `9e4eae47-1715-4c5d-b231-9e24b7b1ec64` (cloud engine `gpt-6-sol`, local
`bge-m3` embedder). The user gave the assistant the project's own `spec.md`; the
assistant read it as 20 pages in the order 1 · 9, 22, 38, 62 · 17, 19, 24, 70 ·
72, 71, 13, 20 · 21, 68, 69, 11, and the user asked why, and whether the model's
context had been shuffled too.

**The context was not shuffled.** Every result is paired to its call by
`tool_call_id`, opens with `… — page N of 72`, and sits in the call order; nothing
reorders messages by time (the tool rows carry timestamps a few microseconds
*earlier* than their assistant row, so a sort would have broken it — there is
none). The order was the model's choice. What pushed it into that choice:

1. `fetch_url` attached the page (72 pages, ~101 000 tokens) and its result said
   *read it page by page with `attachment_read` **or find the right place by
   meaning with `attachment_search`***.
2. The model took the second route. Both searches answered *the files of this
   chat cannot be searched (no search index was built) … page 1 first, then
   onwards*.
3. Walking 72 pages in order being out of the question, it read page 1 — the
   table of contents, seventeen chapters — and sampled the rest by guessing
   positions. Its final reply says so: it read "the relevant sections, not all 72
   pages".

**The search could not have worked in that turn, by construction.** A
tool-produced attachment reaches the turn's snapshot at the end of its round
(`sync_attachments`, `generation.rs`), which is why `attachment_read` works at
once; but indexing is spawned by `insert_attachment`, which runs only when the
turn **lands** (`generation.rs`, the `landed.attached` loop). The log:

| time (UTC) | event |
|---|---|
| 01:36:10.095 | the page attached (tool result) |
| 01:36:14.502 | `attachment_search` ×2 → `not_indexed` |
| 01:36:48.824 | the turn's last reply |
| 01:36:48.826 | the indexer's `"ping"` reaches the embedder (3 tokens) |
| 01:37:12 | the last chunk embedded — the file searchable, 62 s after it was attached |

So `fetch_url` (and `youtube_watch`, which carries the same sentence) advertises
a tool that, for this file, in this turn, always refuses — the exact trap
lessons §4 names: *an attachment entry names the search tool only for a file that
really has an index*. The pinned attachments block already obeys it
(`end_excerpt` vs `end_excerpt_search`, chosen on `attachment_indexed_ids`); the
two tool results do not, and neither does spec §9.3.1, which says such an
attachment "is immediately paged and searchable".

**And the file was not the whole spec.** `MAX_EXTRACT_CHARS = 400_000`;
`spec.md` is 448 525 characters. The text stops at character 399 961, inside
**§12.3 Backup and deletion** — §13–§17 are absent, **§17 Self-model** among them,
the chapter most relevant to the user's question ("what would you want for
yourself"). The table of contents on page 1 promised it; pages 62 and 68–72 look
like the model hunting the end of the document for it. It had been told only
*"the page text was cut off at the size limit — this is NOT the whole page"*:
not where the text ends, not what is missing, not whether anything can fetch the
rest. And the attachment itself carries no mark at all — page 72 simply stops
mid-word (`the pre-image of every file the assista`).

A smaller untruth rides along: the attachment header says *"Text extracted from
HTML: markup removed … images, tables and navigation did not make it"*. This page
was `text/plain`, taken verbatim — its tables are all there.

## 2. Forks for stage 1

### F1. What the two tool results say about search — **(a), decided**

- **(a) Advertise only the page route, and name the dead one.** `fetch_url` and
  `youtube_watch` stop offering `attachment_search`, and say instead that search
  over this file is not available in this turn (its index is built after the
  reply, when an embedder is configured). Naming it matters: the tool's own
  description tells the model to prefer search for "a large file marked *only the
  beginning is shown*", which is precisely this file. **Recommended.**
- (b) Only delete the clause. Shorter, but leaves the tool description's pull
  toward search unanswered.

### F2. What `attachment_search` answers when a file has no index — **(a), decided**

Today one sentence covers four different situations — no embedder, a file born
this turn, every file inline, and (in a mixed chat) nothing at all: when *some*
file is indexed the search runs and silently omits the rest, so "nothing found"
can mean "the file you mean was never searched".

- **(a) Name the files and why.** The turn's context records which attachments
  its own tools produced (`ToolContext.born_this_turn`, filled by
  `sync_attachments`; a sub-agent inherits it with the clone). The answer lists
  each by-reference file without an index — *attached in this turn: its index is
  built after the reply* or *has no search index* — with its page count and the
  `attachment_read` route; when every file is inline it says they are shown in
  full in the attachments block. When the search does run but some by-reference
  file is not indexed, the result ends with the same list, marked *not searched*.
  **Recommended.**
- (b) Only a birth-turn sentence; the mixed-chat omission stays.

### F3. How the cut is stated — **(a), decided**

- **(a) Where, what, and what next.** The result's note gains: the limit in
  characters; the **last heading** before the cut (a Markdown heading outside a
  code fence — both `extract_rich`'s `## …` and a plain `.md` qualify) and the
  page it falls on; for a `text/plain`-family body, how many characters were
  left out (known there — the whole body is in hand; for HTML it is not, and the
  note does not guess); and the door: *fetching this URL again returns the same
  beginning, and no other tool reaches the rest — if the missing part matters,
  tell the user which part it is.* The **attachment itself** carries the cut
  twice: a line in its header (so the pinned excerpt shows it every turn, and
  page 1 opens with it) and a bracketed end marker where the text stops (so the
  reader who reaches the last page meets the cliff, not a word cut in half).
  **Recommended.**
- (b) The result's note only. The attachment stays unmarked, and every later
  turn — which sees the pinned block, not the old tool result — is back to not
  knowing.

### F4. The ceiling itself — **(b), decided**

`/file attach` already accepts a by-reference text file up to 32 MB; the page
ceiling is 400 000 characters. The one case at hand is the project's own spec,
at 448 525 and growing a few KB per pull request.

- (a) Keep 400 000; F3 makes the cut honest.
- **(b) Raise it to 1 000 000.** Covers the spec with room (2.2×). Costs,
  scaled from this run: the chat file grows by up to ~1 MB per such page (it is
  rewritten atomically on each save), indexing a full page takes ~60 s on the
  local `bge-m3` (24 s measured for 400 000), ~180 pages. `join_blocks` counts
  the growing output's characters once per block — quadratic — so it becomes a
  running count in the same change. **Recommended.**
- (c) Align with `/file attach` (bounded only by the 32 MB body). A model-initiated
  fetch is not a user's deliberate attach; a runaway page would index for minutes.

### F5. The header's "extracted from HTML" on a plain-text page — **(a), decided**

- **(a) Say what was done.** `PageText` records whether the body was taken
  verbatim (`text/plain`, JSON, CSV, JS) or extracted from HTML, and the header
  says which. **Recommended** — it is the same line F3 already edits.
- (b) Leave it.

## 3. Decided without a fork

- **Wording lives in the locales**, both `en` and `ru`, each a full sentence; the
  new keys get the existing completeness gate for free.
- **No data format change.** The markers are text inside the attachment; the
  chat schema, the index and the settings are untouched.
- **The inline (non-attached) path keeps its note**, extended by F3's *where*
  when a page under the attachment budget is somehow cut — which only a ceiling
  below the budget could cause, so in practice it never fires; the code path is
  shared anyway.

## 4. Verification

Unit tests next to each change (the result texts in both locales, the
birth-turn/mixed-chat answers, last-heading detection with fences, both markers
in the attachment, the `join_blocks` count). Live: a turn against a real engine
asked to find a passage in a large fetched page — the result must not offer
search, and a search the model tries anyway must get the birth-turn answer; and a
network smoke on a page over the ceiling for the markers.

## 4a. Found while implementing, and what the live run showed

- **`text/markdown` could not be fetched at all.** Testing F5 found it: a Markdown
  body went down the HTML path, had no `<p>` to extract and failed as "no readable
  text". Any `text/*` that is not HTML/XML is taken verbatim now.
- **"The whole page" is said only when it is.** The attached result opened with
  it even for a cut page, contradicting the cut note right after; a cut page is
  attached as "what was received".
- **The `join_blocks` estimate was wrong.** F4 predicted seconds at 1 000 000; measured
  on the Rust book's `print.html` (4 698 blocks) the quadratic count cost 87 ms
  against 17 ms for the running one (release). Fixed anyway — it costs nothing.
- **Searching again is named as pointless** in the birth-turn answer. The first live
  run searched three times in one turn, each answer the same. It does not stop the
  model reliably: on `gemma-4-26B-A4B` the first search comes right after the fetch,
  before any search answer, and eleven runs searched 1–3 times (mean 1.5). Every one
  of those searches got the birth-turn answer and the model moved on to the pages.
- **The birth turn still samples pages** — the thing stage 2 is for. Asked about §17.5,
  four birth turns of the six kept found it by paging; two spent the default 8 rounds
  (one fetch, one search, six pages) and ended on the round cap.
- The rest of the live record — network smokes on three real pages, the two-turn smoke
  and its runs — is in [docs/journal/rag.md](../journal/rag.md).

## 5. Stage 2 — indexing inside the turn

**Status:** implemented, live **GO** (§5.4) — forks decided by the user on
2026-09-26, all six at the recommendation (G1a–G6a). The track is complete.

### 5.1. What is there to build on **[code]**

- The loop already mirrors a tool's `AddAttachment` into its snapshot at the end of
  the round (`sync_attachments`), and it has everything indexing needs: `ctx.embedder`,
  `ctx.storage`, `ctx.chunk_params` (the same `config.rag` the landing uses) and the
  event channel. `index_attachment` is the one writer both paths already share.
- A sub-agent's `AddAttachment` goes to its parent, which mirrors it at its own
  round's end; a background run's lands through `insert_attachment` when the run
  lands. So one attachment can reach the "start indexing" point up to three times —
  child loop, parent loop, landing — and needs one owner.
- **A feed note in the middle of a stream splits the reply.** `push_note` appends
  after the streaming bubble, and the next chunk, finding the note last, opens a new
  bubble (`ensure_streaming_bubble`). Today that only happens when a `/file attach`
  index happens to finish mid-reply; indexing inside the turn would make it routine.
- Measured in §1: 976 fragments in 24 s on a local GPU `bge-m3`. A turn's next
  round comes 3–5 s after the one that fetched, so a search there meets an index
  roughly a sixth built.

### 5.2. Forks

**G1. Search that meets a file still being indexed — (a), decided.**
- **(a) Wait for it, bounded; then search.** Up to 120 s (a CPU embedder is about ten
  times slower than the measured GPU one), cancelled with the turn; the progress is
  on screen meanwhile, in the banner indexing already uses. Past the bound, search
  what is indexed and say how much of the file that covers (fragments done/total —
  chunks are written in document order). **Recommended.**
- (b) Answer at once with the progress, no search. The model has to come back, and
  §4a shows it does not reliably wait for anything.
- (c) Search the partial index at once. The spec's §17 is at the end of the file:
  the head a sixth-built index covers is exactly the part not asked about.

**G2. Where indexing of a tool's attachment starts — (a), decided.**
- **(a) At the end of the round that produced it**, next to `sync_attachments`, with a
  small index board (`Arc`, shared by the orchestrator and the turn's context) that
  records what was started — so the child, the parent and the landing start it once
  — and the progress `attachment_search` reads and waits on. The landing starts only
  what nobody has. **Recommended.**
- (b) Lazily, on the first search. Saves nothing — a by-reference attachment is
  indexed at landing anyway — and makes the first search wait for the whole file
  rather than its remainder.

**G3. The note that an index is done — (a), decided.**
- **(a) Held until the attachment lands.** The banner shows progress live; the
  "searchable — N fragments" note (or "no index — reason") is emitted by the landing
  right after "attached", or by the index task when it finishes later. The reply is
  never split, and the notes read in order. **Recommended.**
- (b) As it happens — splits the streaming reply (5.1).
- (c) No note for a tool's attachment.

**G4. What `fetch_url` and `youtube_watch` say about search — (a), decided.**
- **(a) Depends on the embedder:** configured — the file is being indexed for
  `attachment_search`, a search waits for the index, and `attachment_read` reads any
  page at once; not configured — pages only, and no index will be built.
  **Recommended** — "only advertise what exists".
- (b) One neutral sentence for both.

**G5. The pinned block for a file whose index is still being built** (a large file,
a slow embedder, the next turn starts early) **— (a), decided.**
- **(a) Offer search**, as for an indexed file — the search waits. **Recommended.**
- (b) Pages only until the index is done.

**G6. Stage 1's birth-turn machinery** (`ToolContext.born_this_turn`, the "attached in
this turn" line and its "searching again is pointless") **— (a), decided.**
- **(a) Remove it.** A file born in the turn is then being indexed, indexed, or
  without an index like any other, and the answer says which. **Recommended.**
- (b) Keep it for a turn without an embedder — where "no index" already says it.

### 5.3. Verification

Unit: the board (start once, progress, a held note released by the landing or by a
late finish), a search that waits and then finds, a search past the bound that
reports coverage, cancellation ending the wait, the loop starting the index at the
round's end, the landing not starting it twice, the pinned block offering search
for a file in progress, the texts with and without an embedder. Live: stage 1's
two-turn smoke turned into one turn — the birth turn finds §17.5 by one search — with
stage 1's runs as the control (four of six found it by paging, two ran out of rounds).

### 5.4. What the live run showed

Six runs of the birth-turn smoke on `gemma-4-26B-A4B` + `bge-m3`: six GO, each finding
§17.5 with **one** search and **no** page read, 28.5–33.2 s per turn with the page
summary included. Against stage 1's runs of the same question — one to three searches
answered "not indexed yet", four to six pages read, two of six turns out of rounds — the
wait costs less than the sampling it replaces. The full record is in
[docs/journal/rag.md](../journal/rag.md).
