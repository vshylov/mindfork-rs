# Research: full-text search over chat content

**Status:** research, pre-decision. Roadmap item *"Search within chat content (not
just the title) — full-text search via SQLite FTS"*.

**Question.** Chats stay in JSON (`chats/{id}.json`, spec §5.2). Search needs an
index. The user's requirement: put it in a **separate database** that can be
deleted with no risk of losing anything important, kept in sync with the chat
files in the background, and available to hold other non-precious data later.

**Verdict: feasible, cheap, and no new dependency.** Everything below was
*measured* against the real corpus in this repo's dev data (171 chats), not
estimated. The open questions are about product behaviour, not feasibility —
see §7.

---

## 1. What was measured (probe, 2026-07-29)

A throwaway probe was added to `shared/storage/db`, run, and removed (the tree is
unchanged). SQLite version — **3.53.2**, the one `rusqlite`'s `bundled` feature
compiles.

### 1.1 FTS5 is already available

`pragma_compile_options()` → `ENABLE_FTS3`, `ENABLE_FTS3_PARENTHESIS`,
**`ENABLE_FTS5`**. So this needs **no `Cargo.toml` change and no new
dependency** — FTS5 is a build-time SQLite option and `libsqlite3-sys` turns it
on. `snippet()`, `highlight()`, `bm25()`, contentless (`content=''`) and
external-content tables are all present.

### 1.2 Cyrillic works correctly

The one thing genuinely worth checking, because tokenizer behaviour outside ASCII
is where this usually breaks. Against a Cyrillic document:

<!-- cyrillic-ok:start — the probe inputs *are* the subject: these rows record how
     the tokenizers fold and match Cyrillic, which ASCII cannot demonstrate. -->

| query | tokenizer | result |
|---|---|---|
| `привет` vs stored `Привет` | unicode61 | ✅ matches (full Unicode case folding) |
| `МИР` vs stored `МИР` | unicode61 | ✅ |
| `тест*` | unicode61 | ✅ prefix |
| `sqlite AND мир` | unicode61 | ✅ boolean, mixed scripts |
| `"это тестовое"` | unicode61 | ✅ phrase |
| `snippet()` / `bm25()` | unicode61 | ✅ correct highlight on Cyrillic |
| `естов` (infix), `ЕСТОВ` | trigram | ✅ — trigram *is* case-insensitive for Cyrillic here |

<!-- cyrillic-ok:end -->

The trigram result is worth calling out: trigram case folding was historically
ASCII-only, so this had to be verified rather than assumed. On 3.53.2 it folds
Cyrillic.

### 1.3 Cost, on the real corpus

Dev data: **171 chats, 1540 messages, 13.5 MB of JSON, 4.9 MB of message text**.

| | unicode61 | trigram |
|---|---|---|
| index build (insert all) | **258 ms** | 474 ms |
| index size on disk | **7.0 MB** | 13.7 MB |
| typical query | 27–342 µs | 133–339 µs |
| ranked top-5 + `snippet()` | 15 ms | 7 ms |

Plus, measured separately:

- **load + parse all 171 chats into typed `Chat`: 94 ms.**
- **`stat`-only walk of `chats/`: 319 µs.**

So a **full index rebuild from scratch costs ~350 ms** (94 ms read + 258 ms
index), and checking whether anything changed costs **a third of a millisecond**.
At 10× the corpus that is ~3.5 s to rebuild and 70 MB of index (unicode61) —
still comfortable, and the incremental path below means a rebuild is rare.

These numbers are what make the design simple: at this scale we do not need to be
clever.

---

## 2. Shape: a second database, `cache.db`

`Paths` gains `cache_db()` → `<root>/cache.db`, and `Storage` gains a third
member alongside `json` + `db`.

Why a separate file is the right call beyond the user's stated reason:

- **It is derived data, and that changes the rules.** `data.db` holds notes, the
  self-model and RAG — irreplaceable user content, hence the schema-versioning
  machinery of [ADR 0006](../decisions/0006-data-schema-versioning.md) (steps in
  transactions, downgrade guards, pre-migration backups). A search index needs
  **none of that**: a version mismatch, a corrupt file, or a schema change is
  answered by `delete the file and rebuild` (~350 ms). That is a genuine
  reduction in machinery, not just tidiness.
- **Backup already excludes it, by construction.** `features/backup.rs` uses an
  **allowlist** (`TOP_FILES` / `TOP_DIRS`), so a new file at the root is left out
  of the archive with **no code change**. A restore therefore lands without an
  index and rebuilds — correct behaviour, obtained for free.
- **Deleting it is a supported repair.** With `data.db` that would be data loss;
  with `cache.db` it is a documented "turn it off and on again". Worth stating in
  `install.md`.
- **It is a home for other cheap-to-recompute state.** The user explicitly wants
  this. Candidates already visible in the codebase: chat-list summaries (to avoid
  parsing 13.5 MB at startup), a token-count cache, recently-used state.

Isolation invariant (spec §9.5): notes/RAG are strictly per-`profile_id`. The
chat list, by contrast, is **already global** (`bootstrap` loads every visible
chat regardless of profile), so a global search is consistent with what exists —
but see fork **F5**.

---

## 3. Sync: the writer is already single

This is the part that turns out easier than it sounds, because of an invariant
the project already maintains.

**The orchestrator is the sole writer of chats** (architecture §1; every write
funnels through `flush_saves` → `JsonStore::save_chat`). So for everything that
happens *inside the running app* there is an exact hook and no polling is needed:
index the chat right after it is saved. The 800 ms save debounce already
coalesces a burst of streaming updates into one write, so it coalesces indexing
too.

That leaves changes made **outside** the app — `mindfork import`, `restore`, a
hand-edited file, data synced from another machine, or simply a `cache.db` that
was deleted. Those are covered by a **startup reconciliation**:

```
for each chats/*.json:  stat → (mtime, size)          # 319 µs for the whole dir
  compare against indexed_files(chat_id, mtime, size)
  differs or absent → parse + reindex that chat
indexed but file gone → drop from the index
```

Cheap enough to run unconditionally at every startup, in a background task, so it
never delays the UI. `(mtime, size)` is the standard cheap change signal; a
content hash would be exact but requires reading all 13.5 MB, and the failure
mode it protects against (a same-size edit within the mtime resolution) is not
realistic for a JSON file the app itself rewrites wholesale.

Correctness note: the index must be usable *while* it is being built, so the
reconciliation should write per chat (one transaction per chat), and search
results should be understood as "everything indexed so far". A first run on an
empty index is ~350 ms, so the window is small.

---

## 4. The pitfall that will actually bite: query syntax

**Raw user input cannot be handed to `MATCH`.** This is the single most important
practical finding, and it is not a corner case — it fires on ordinary text.
Measured, unescaped:

| user types | result |
|---|---|
| `C++` | ❌ `fts5: syntax error near "+"` |
| `cost-benefit` | ❌ `no such column: benefit` |
| `50%` | ❌ `fts5: syntax error near "%"` |
| `AND` | ❌ `fts5: syntax error near "AND"` |
| `(` | ❌ `fts5: syntax error near ""` |
| `"quoted` (unbalanced) | ❌ `unterminated string` |
| `a:b` | ❌ `no such column: a` |

Note `cost-benefit` and `a:b` are the nastiest: FTS5 reads them as *column
filters*, so the error blames a column the user never mentioned.

The fix is to treat input as **text, not syntax** — quote every token and double
any inner quote:

```rust
fn escape(q: &str) -> String {
    q.split_whitespace()
     .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
     .collect::<Vec<_>>().join(" ")
}
```

Verified: all seven failures above become clean, correct searches. Multiple
tokens then mean implicit AND, which is what users expect.

This does mean FTS5's own operators (`OR`, `NEAR`, prefix `*`) become literal
text — a deliberate trade, and the subject of fork **F2**.

---

## 5. The real fork: tokenizer (what the user *feels*)

This is the decision that changes behaviour, and it deserves the user's call
rather than a default.

<!-- cyrillic-ok:start — same reason as §1.2: the Russian word forms *are* the
     evidence. `память`/`памяти` is the morphology case FTS5 has no stemmer for,
     and an English example would not exhibit it. -->

**Today's chat filter is substring** (`title.to_lowercase().contains(&needle)`,
`features/chat_search_sort.rs`). Users are trained on it: typing `естов` finds
`тестовое`. A word-based index does not behave that way.

**And FTS5 has no Russian stemmer.** The built-in tokenizers are `unicode61`,
`ascii`, `porter` (English only) and `trigram`. So with `unicode61`, searching
`память` does **not** find `памяти` or `памятью`. Russian morphology makes this a
frequent, not occasional, miss.

| | `unicode61` | `trigram` |
|---|---|---|
| substring / infix (`естов` → `тестовое`) | ❌ | ✅ |
| Russian morphology (`память` → `памяти`) | ❌ (see below) | ✅ |
| prefix (`памят*`) | ✅ | n/a (implicit) |
| phrase, boolean | ✅ | ✅ |
| relevance ranking (`bm25`) | meaningful | weak |
| `snippet()` quality | good at default budget | needs a ~40–64 token budget, else `…им [память] и…` |
| index size (this corpus) | 7.0 MB | 13.7 MB (**2×**) |
| minimum query length | 1 char | **3 chars** |

Half-measure worth knowing about: with `unicode61`, auto-appending `*` to the
last token turns `памят` into `памят*` and matches `память`/`памяти`/`памятью`.
It covers the common case — but only if the user stops short of the full word:
`память*` still misses `памяти`, because they diverge at the final letter. So it
mitigates the morphology gap, it does not close it.

<!-- cyrillic-ok:end -->

Honest reading of the trade: **trigram matches what users already expect from
this app** and sidesteps morphology entirely, at 2× index size, a 3-character
minimum, and weaker ranking. `unicode61` is the "proper" full-text choice with
better ranking and snippets, and will feel subtly broken on Russian. Both are one
line of DDL, and switching later is just a rebuild — which is exactly the freedom
a disposable `cache.db` buys.

---

## 6. Scope of the index

`Message` carries more than `text`: `thoughts` (CoT), and `tool_calls` with
`arguments` (JSON) and `result`. `Chat` additionally carries `deleted`
(archived exchanges, spec §11.7), `draft`, and `attachments[].text` (file
snapshots — often the largest text in a chat).

Indexing everything would inflate the index and, more importantly, add noise:
tool-call JSON and CoT match on words the user never wrote. Indexing only
`text` risks "I know I saw it in a tool result". See fork **F3**.

---

## 7. Forks — **decided by the user 2026-07-29**

All six adopted as recommended (F2 and F5 were stated rather than asked, their
recommendation being unambiguous):

| | decision |
|---|---|
| **F1** tokenizer | **`trigram`** — substring, as today's filter; immune to Russian morphology |
| **F2** query syntax | **always literal** — every token quoted, implicit AND, never errors |
| **F3** scope | **`message.text` only** for stage 1 |
| **F4** UI | **extend the chat-list screen** with a title/content toggle |
| **F5** search scope | **global**, matching today's global chat list |
| **F6** staging | **two PRs** — stage 1 index + sync + minimal UI; stage 2 message-level screen |

The full options and their trade-offs are kept below as the record of *why*.



**F1 — tokenizer.**
(a) `trigram` — substring like today's filter, immune to Russian morphology; 2×
index, 3-char minimum, weak ranking. *(recommended: it matches the behaviour
users already have, and morphology is the failure mode they'd hit daily)*
(b) `unicode61` + auto-prefix on the last token — proper ranking and snippets,
smaller index; misses infix matches and some morphology.
(c) Both indexes, user-selectable — most flexible, ~21 MB, two code paths.

**F2 — query syntax.** Escaping (§4) is mandatory either way; the question is
whether power users get FTS5 operators.
(a) Always literal: every token quoted, implicit AND. Never errors. *(recommended)*
(b) Literal by default, with a documented escape hatch (e.g. a leading `=` means
"raw FTS5 query") for `OR`/`NEAR`/column filters.

**F3 — what to index.**
(a) `message.text` only — smallest, cleanest, no noise. *(recommended for a first
stage; the rest can be added without a migration, since the index is disposable)*
(b) `text` + `thoughts`.
(c) `text` + `thoughts` + tool arguments/results.
(d) also attachment snapshots and `deleted` exchanges.

**F4 — UI.**
(a) Extend the existing chat-list screen (`Esc`) with a title/content toggle;
results stay a *chat* list, ranked by best-matching message. Minimal, reuses
everything. *(recommended for stage 1)*
(b) A dedicated search screen with **message-level** hits: snippet, chat, date,
and `Enter` jumps to that message in the feed. Much more useful, but needs
"scroll the feed to message N", which the roadmap's separate *In-feed text
search* item also wants — arguably they should be designed together.

**F5 — scope.** (a) Global across all profiles, matching today's global chat list
*(recommended)*; (b) restricted to the active profile, matching the notes/RAG
isolation invariant; (c) global with a toggle.

**F6 — staging.** (a) One PR: index + sync + chat-list integration.
(b) Two: stage 1 `cache.db` + index + background sync + minimal UI (F4a); stage 2
the message-level search screen (F4b) together with in-feed search. *(recommended
— stage 1 is self-contained and useful, and stage 2's design depends on the feed
jump)*

---

## 8. Sketch of the shape (for reference, not a commitment)

```sql
-- cache.db
PRAGMA user_version = 1;               -- mismatch → delete the file, rebuild

CREATE TABLE indexed_chats (           -- reconciliation bookkeeping
  chat_id  TEXT PRIMARY KEY,
  mtime_ms INTEGER NOT NULL,
  size     INTEGER NOT NULL,
  title    TEXT NOT NULL
);

CREATE VIRTUAL TABLE messages USING fts5(
  text,
  chat_id   UNINDEXED,
  message_id UNINDEXED,
  role      UNINDEXED,
  ts        UNINDEXED,
  tokenize = '<F1>'
);
```

- `shared/storage/cache/` — `CacheDb` (open/rebuild/reconcile/index_chat/search),
  mirroring `db/`'s layout.
- `features/chat_search.rs` — the pure part: query escaping (§4), result types.
  Testable with no DB, in the spirit of `chat_search_sort.rs`.
- `app/orchestrator/search.rs` — the background reconciliation task + the
  post-save hook, mirroring `rag.rs`.

No FSD violation: `shared` owns the storage, `features` the pure logic, `app` the
orchestration.

---

## 9. Open groundwork noted along the way

- `cache.db` could also hold **chat-list summaries**, removing the 94 ms
  parse-everything cost at startup (and it grows linearly with the corpus).
- Search hit → **jump to the message in the feed** is shared with the roadmap's
  *In-feed text search*.
- Highlighting matches inside the feed reuses `snippet()`/`highlight()` offsets
  only loosely — the feed renders markdown, so highlight positions do not map
  directly. Worth a look before promising it.
