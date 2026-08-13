# Cross-chat search as an assistant tool (`chat_search` / `chat_read`)

Status: **accepted 2026-08-14** — every fork decided (the recommendations,
confirmed by the user); implemented in `feat/cross-chat-search-tool`.
Date: 2026-08-14.

## 1. What and why

Give the assistant a tool to find information in the *other* chats of the
current profile — the model-facing counterpart of the chat-list content search
the user already has (spec §11.2.1). The motivating story: "we discussed this
in another conversation — find it and use it", today answerable only by the
user manually re-opening old chats and pasting text.

The user's stated constraint shapes the whole design: **some models may abuse
such a tool, so it must be toggleable and OFF by default.** Everything else
follows the two precedents the codebase already ships: the current-chat
read-back pair `history_search`/`history_read` (spec §6.7, the folded range)
and the attachment pair `attachment_search`/`attachment_read` (spec §9.7).

Not on the roadmap today; closest neighbours are "widening what chat search
indexes" and "ranking" (docs/roadmap.md §Chat and profile management), which
this design inherits as-is rather than solves.

## 2. What already exists (inventory)

Everything heavy is already built; the tool is mostly glue.

- **The index.** `cache.db` — external-content FTS5, trigram tokenizer, over
  `message.text` of all four roles; disposable derived data
  (architecture §7, `src/shared/storage/cache/mod.rs`). Titles, thoughts and
  tool results are *not* indexed (roadmap item). Hidden chats drop out at
  index time (`index_saved_chat` → `forget_chat`; startup pass stores them
  with an empty message set).
- **Query API.** `CacheDb::search_messages(fts, limit) → Vec<MessageHit
  {chat_id, message_id, role, ts, text}>` (`cache/mod.rs:337`), plus
  `count_matching_messages` for the honest total. Reachable from a tool as
  `ctx.storage.cache()`.
- **Escaping.** The single source `to_fts_query`
  (`src/features/chat_search.rs:52`): quote every token, double inner quotes,
  3-char trigram floor counted in characters. Raw text never reaches `MATCH`
  (docs/lessons.md §8).
- **Snippets.** `build_snippet` (character-budget window centred on the first
  match); the model-reader budget precedent is `SNIPPET_BUDGET_CHARS * 3 =
  480` (`tools/history.rs:48`).
- **Tool contract.** Shared `search_parameters`/`search_args`
  (`tools/mod.rs:228,256`, `DEFAULT_TOP_K = 5`) — extracted precisely so
  search tools cannot drift (journal engine.md, stage 3); a third search tool
  must reuse them.
- **The off-by-default knob.** `Tool::enabled_by_default() = false`
  (`tools/mod.rs:406`): listed in the settings catalog, absent from
  `default_tool_ids()`, **never advertised to the model** while off
  (`effective_tool_ids` → `schemas_for`), and `reconcile_tools` does *not*
  auto-add optional tools to existing profiles. A blind call is refused with
  the localized `loop.tool_disabled` (defensive gate,
  `orchestrator/generation.rs:1087`).

Two things exist **nowhere** in the current search stack and are this
design's actual new work (both flagged by the code survey):

1. **Profile isolation.** `cache.db` has no `profile_id` — the index spans
   every chat of every profile, and the UI search is cross-profile by design.
   A tool must enforce spec §9.5 itself.
2. **Current-chat exclusion.** The UI search includes the open chat; the tool
   must not (its visible half is the model's own context, its folded half
   belongs to `history_search`).

## 3. Design

### 3.1 The tools

**`chat_search`** — search message text across the other chats of the current
profile. Args: shared `{query: string, top_k?: int}` (`search_args`, default
5 hits). Pipeline:

1. `to_fts_query(query)`; sub-trigram → localized `too_short` (as
   `history_search`).
2. Query scoped to the profile's chat ids (fork F4), cap `top_k`, honest
   total counted only when the cap bites (orchestrator precedent).
3. Group hits by chat (the UI's S2 decision — grouping instead of ranking);
   chats ordered by recency, hits within a chat by `ts`. Each hit: role,
   date, 480-char snippet. Each chat header: title + short id (fork F5) +
   date.
4. Trailing hint names the route onward: `chat_read` pages the chat (fork
   F2). "No other chats in this profile" (nothing here) and "no matches"
   (reword) are distinct answers — lessons §4.

The description states the honest scope: message text only — thoughts and
tool results are not searched.

**`chat_read`** — read one other chat of the profile, page by page. Args:
`{chat: string, page?: int}`. `chat` resolves by short-id prefix, then exact
title, then the `attachment_read` ambiguity ladder (ambiguous → list
candidates with dates; unknown → say so and point back at `chat_search`).
Loads that chat's JSON from storage (one file; other chats are quiescent
during a turn — only one generation runs at a time), renders a transcript the
way `HistoryView` renders the folded range, paginates by
`ctx.history_page_tokens` (one knob for "assistant reads history-like
text"). Bad page states the real total (`history_read` precedent).

Both tools: group `Conversation`, `danger() = false` (read-only, no effects),
no `ToolGate` (fork F1), `enabled_by_default() = false`.

### 3.2 The profile-scoped snapshot

`ToolContext` gains a turn-snapshot field following the `attachments` /
`history` precedent (S9): `other_chats: Option<Arc<Vec<ChatRef>>>` with
`ChatRef {id, title, updated_at}` — built in `TurnInfo` from the
orchestrator's in-memory chat list, already filtered to `profile_id ==
current`, `id != current chat`, non-hidden (the orchestrator's list holds
only non-hidden chats, and the index drops them too — belt and braces, same
as `group_hits`). Cost: a Vec of (Uuid, String, ts) clones per turn —
negligible; may be gated on the tools being in the turn's allowed set.

The snapshot is also the **address book**: `chat_search` filters/scopes by
its ids, `chat_read` resolves names against it, and a chat that appears in a
result is guaranteed readable by the companion tool (lessons §4: only
advertise what exists).

### 3.3 Storage addition

`CacheDb` gains `search_messages_in(fts, chat_ids, limit)` and
`count_matching_messages_in(fts, chat_ids)` — `WHERE chat_id IN (...)` with
the escaped query, so the cap is honest *within the profile* (post-filtering
a global `LIMIT` would let another profile's hits starve this one's).
`CacheDb` keeps taking an already-escaped query and raw ids — no `features`
dependency, FSD intact.

### 3.4 Abuse containment (the user's core requirement)

- **Off by default, per profile** — enabling is a deliberate act in settings
  → Tools, per profile (exactly the granularity that matters when different
  profiles run different models).
- **Invisible while off** — no schema in the prompt, zero token cost, no
  temptation (the S12 rationale).
- **Bounded output** — `top_k` default 5, 480-char snippets, honest totals,
  pages of `history_page_tokens`; truncation is stated, never silent.
- **No new per-turn call limit** — deliberately: the global
  `max_tool_rounds` already bounds rounds, and no per-tool limit precedent
  exists. Recorded here so it is not re-litigated.
- **Steering by description** — "search past conversations of this profile
  when the user refers to something discussed before", not an invitation to
  browse.

Privacy: cross-chat visibility *is* the feature; the per-profile toggle is
the consent. The profile boundary (spec §9.5) stays hard. Content arriving
from other chats is the user's own prior conversation — the same trust domain
`history_read` already re-injects; no new injection class, and snippets stay
plainly framed as quoted search results.

## 4. Forks

- **F1. Gating axis.**
  (a) **Profile axis only**: `enabled_by_default() = false`, no config field,
  no `ToolGate` — one switch, in the place users already manage tools;
  precedent: self-model and control tools. *(recommended)*
  (b) Additionally a global `tools.chat_search_enabled` boolean + gate
  (python/fs pattern) — a second kill switch, but two switches for one tool
  invite the "enabled in profile, still off" confusion the lessons warn
  about.
  User's decision: **(a) — per-profile only** (2026-08-14).
- **F2. Shape.**
  (a) **Ship the pair** `chat_search` + `chat_read` in one stage — both
  precedents are pairs, and search-without-read leaves the model with
  snippets and no stated route onward (the youtube lesson: a result that
  cannot close the door costs eight flailing calls). *(recommended)*
  (b) Search-only MVP; then the result text must say full chats cannot be
  read, and a second stage adds the reader.
  User's decision: **(a) — the pair, one stage** (2026-08-14).
- **F3. Current chat.** Exclude *(recommended — context + `history_*` own
  it)* / include for symmetry with the UI.
  User's decision: **exclude** (2026-08-14).
- **F4. Profile scoping of the query.**
  (a) **New `search_messages_in(fts, ids, limit)`** — honest per-profile cap,
  one query. *(recommended)*
  (b) Global `search_messages` + post-filter — a global LIMIT starves the
  profile.
  (c) Per-chat loop over `matching_messages_in_chat` — N queries, no shared
  cap.
  User's decision: **(a)** (2026-08-14).
- **F5. Chat addressing in results/`chat_read`.**
  (a) **Short id (uuid prefix, ~8 hex) shown next to the title in
  `chat_search` output; `chat_read` resolves id-prefix → exact title →
  ambiguity ladder.** Robust under duplicate titles. *(recommended)*
  (b) Title-only with the ladder — human-readable, collision-prone.
  User's decision: **(a)** (2026-08-14).
- **F6. Result order.** Group by chat, chats by recency, hits by timestamp
  *(recommended — mirrors the UI's S2 "group, don't rank")* / flat hit list.
  User's decision: **grouped** (2026-08-14).
- **Considered and rejected here:** semantic/embedding search (S11 already
  decided FTS for read-back — the embedder is routinely unconfigured, and
  trigram substring match is what identifiers need); widening the index to
  thoughts/tool results (existing roadmap item, orthogonal); a per-tool
  call-rate limit (above).

## 5. Test plan

Unit (in-memory storage testkit, `ctx_with_storage`):
- profile isolation — seed two profiles' chats into the index, assert only
  the caller's profile surfaces (negative test, `ctx_with_deps` pattern);
- current-chat exclusion; hidden chats absent;
- cap + honest total; grouped order; snippet budget;
- `chat_read` addressing: id-prefix, exact title, ambiguous → candidates,
  unknown → door closed toward `chat_search`; bad page → real total;
- distinct "no other chats" vs "no matches" wording; localization en/ru (no
  `{}` leftovers, no Cyrillic in en); shared-arg errors (empty query hard
  `Err`).
Registry gates (schema, description localization, `ui.tool.label.*`) pass by
construction once the two tools register.

Live smoke (`live.rs`, `MINDFORK_ENGINE_URL`-gated): a profile with **only**
this pair enabled; seed a second chat holding a nonsense token the model
cannot know; ask a question answerable only from that chat; assert the tool
was actually called (lessons §2/§9) and the answer carries the token. This is
the track's go/no-go.

## 6. Scope and documentation

One stage, one PR (`feat/cross-chat-search-tool`): new
`src/features/tools/chats.rs`; registration + snapshot field in
`tools/mod.rs`; snapshot build in `orchestrator/generation.rs`; two `CacheDb`
methods; `locales/{en,ru}.json` keys (`tool.chat_search.*`,
`tool.chat_read.*`, `ui.tool.label.*`).

Docs per AGENTS.md §4: spec **§9.11** (new) + a row in the §9.3 roster + one
line in §9.5 (the tool honours profile isolation over a profile-blind index);
architecture §8 group table (Conversation row) + an implementation-notes
bullet (the snapshot, the profile join); journal **tools.md** entry;
CHANGELOG (Added); README tools/settings mention. Roadmap: add a groundwork
note that ranking and index-widening would improve this tool for free.
