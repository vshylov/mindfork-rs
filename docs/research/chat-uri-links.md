# Navigable `chat://` references in the feed

Status: **accepted 2026-08-14** — the four load-bearing forks decided by the
user (F1, F3, F4, F7, all as recommended); F2, F5 and F6 carry their
recommendation and were not put separately, so they stay open to revision
during implementation. **Stage 1 implemented** in `feat/chat-uri-links`
(the address, the teaching, the styling, the `Ctrl+L` picker); stage 2 (the
mouse) is not started.
Date: 2026-08-14.

Roadmap item: "Navigable chat references in the feed"
([docs/roadmap.md](../roadmap.md) §Feed and chat UI). Neighbours:
[cross-chat-search-tool.md](cross-chat-search-tool.md) (which minted the
addresses), [chat-search-stage2.md](../history/chat-search-stage2.md) (which
built the jump), spec §9.11 and §11.2.1.

## 1. What and why

With the cross-chat pair enabled (spec §9.11), a model that has just read
another conversation naturally cites it back to the user. Observed in real use
(grok-4.6, 2026-08-14, the user's own session), **not designed**: the model
invented `chat://<short-id>` out of the bracketed address `chat_search` prints,
and wrapped it in a markdown link. The renderer draws the address in link
colour — and it goes nowhere.

Two halves, and the second is the one the request emphasises:

1. **The reference should navigate.** The jump infrastructure already exists;
   what is missing is recognising a reference on screen and resolving it.
2. **The assistant should *know* the format and use it when it fits** — today
   it is a coincidence of one model's habits. Another model, or the same one
   tomorrow, writes `[a1b2c3d4]`, `chat 3`, or the title alone, and the feature
   silently does nothing. A format nobody was taught is a format nobody
   emits.

The order matters: (2) is what makes (1) worth building, and (2) is cheap —
the addresses are already minted by two localized strings.

## 2. What already exists (inventory)

Almost everything. The gaps in §3 are small and specific.

**The address.** `SHORT_ID_CHARS = 8`
([chats.rs:63](../../src/features/tools/chats.rs)) and the single producer
`fn short_id(id: Uuid) -> String` ([chats.rs:97-100](../../src/features/tools/chats.rs)) —
the first 8 characters of the uuid's `simple()` form. Printed in exactly three
places, all localized: the `chat_search` group header
(`tool.chat_search.chat` → `Conversation "{title}" [{id}], last active {date}:`),
the `chat_read` page header (`tool.chat_read.header`), and the ambiguity
candidate list ([chats.rs:374](../../src/features/tools/chats.rs)).

**The resolver.** `fn resolve<'a>(refs: &'a [ChatRef], needle: &str) -> Resolved<'a>`
([chats.rs:128-170](../../src/features/tools/chats.rs)): strips `-`, requires
≥ 4 hex characters (half a short id — below that a hex-looking word like
"cafe" would shadow titles), prefix-matches the ids, then falls to exact title,
then title substring; several matches at any rung stop and report the
ambiguity.

**The address book.** `ToolContext::other_chats: Arc<[ChatRef]>`
([tools/mod.rs:114](../../src/features/tools/mod.rs)), a turn snapshot built by
`snapshot_other_chats` ([chats.rs:84-94](../../src/features/tools/chats.rs)):
current profile, current chat excluded, hidden dropped.

**The jump.** `AppCommand::OpenChatAt { chat, message, query }`
([events.rs:79-86](../../src/app/events.rs)) → `handle_open_chat_at`
([orchestrator/chats.rs:67-69](../../src/app/orchestrator/chats.rs)) →
`activate_focused` ([orchestrator/mod.rs:922-942](../../src/app/orchestrator/mod.rs)) →
`AppEvent::ChatActivated { focus: Option<FeedFocus> }` →
`MessageFeed::focus_message` ([message_feed.rs:518-532](../../src/widgets/message_feed.rs)),
which sets anchor + marker + highlight as one value. A chat-level jump needs
only the plainer `AppCommand::SwitchChat(Uuid)`
([events.rs:74](../../src/app/events.rs)).

**Post-render span rewriting.** The in-feed search highlight is the exact
mechanism a reference detector needs: `highlight_line`
([message_feed.rs:1059-1091](../../src/widgets/message_feed.rs)) concatenates a
line's spans into one string, gets byte ranges from
`chat_search::match_ranges` ([chat_search.rs:186-197](../../src/features/chat_search.rs)),
and re-splits the spans at the range boundaries while preserving each span's
own style (`span_cut_points`, [message_feed.rs:1099-1111](../../src/widgets/message_feed.rs)).

**An address book already inside the chat screen.** `ChatScreen.chats:
Vec<ChatSummary>` ([screens/chat/mod.rs:365](../../src/screens/chat/mod.rs)),
kept current by `AppEvent::ChatList` → `set_chat_list`
([mod.rs:708-710](../../src/screens/chat/mod.rs)) — the snapshot the `Esc`
overlay is built from. It carries `id`, `title`, `created_at`, `modified_at`,
`message_count` ([entities/chat.rs:277-283](../../src/entities/chat.rs)).

**A bare-URL tokenizer to copy.** `speak.rs:226-266`
([rewrite_urls / rewrite_token](../../src/shared/markdown/speak.rs)) — guarded
by `text.contains("://")`, strips leading `([«"` and trailing `)].,;!?`. Its
punctuation rules transfer verbatim.

**Prompt injection points.** `build_request`
([orchestrator/request.rs:103-136](../../src/app/orchestrator/request.rs))
appends blocks by volatility: persona → `inject_compaction` (`:120`) →
`inject_attachments` (`:122-128`) → `inject_self_model`
([generation.rs:722-731](../../src/app/orchestrator/generation.rs)) last. Tool
descriptions reach the model through `schemas_for`
([tools/mod.rs:760-770](../../src/features/tools/mod.rs)) and are localized
strings, gate-tested for en/ru.

## 3. What is missing (the four gaps)

1. **Nothing teaches the format.** No `chat://` string exists anywhere in
   `src`, `locales` or `spec.md`. Confirmed by grep — the roadmap line is the
   only occurrence in the repository.
2. **A bare `chat://…` is not even styled.** The markdown parser runs with
   `ENABLE_STRIKETHROUGH | TASKLISTS | MATH | TABLES` and **no GFM autolinks**
   ([markdown/mod.rs:87-91](../../src/shared/markdown/mod.rs)), so a bare
   reference arrives as an ordinary `Event::Text`. Only the markdown form
   `[Title](chat://id)` gets colour, and only on the appended suffix:
   `end_link` prints `Title (chat://id)` and `self.link.take()`
   ([writer.rs:537-543](../../src/shared/markdown/writer.rs)) is where the URL
   stops being an address and becomes drawn characters. There is **no `Link`
   span kind, no side table, and no source→screen offset map** — and that last
   one is structurally unobtainable, as the feed's own doc comment records
   ([message_feed.rs:1037-1054](../../src/widgets/message_feed.rs)).
3. **The feed cannot be clicked.** `handle_mouse`
   ([screens/chat/input.rs:588-619](../../src/screens/chat/input.rs)) routes
   the wheel to the feed and left click/drag to the input box; a click in the
   feed is explicitly a no-op. `MessageFeed` never stores its rendered `Rect`
   (`inner` is a local at [message_feed.rs:679](../../src/widgets/message_feed.rs)),
   so there is no cell→position mapping — `InputBox::last_area` +
   `place_cursor_at` ([input_box.rs:129-135, :778-809](../../src/widgets/input_box.rs))
   is the precedent to mirror. And mouse capture is `Ctrl+W`, **off by
   default**, because turning it on costs native terminal selection.
4. **The tool-side resolver would reject its own scheme.** `resolve` filters
   only `-` before the hex test, so `chat_read("chat://a1b2c3d4")` fails the
   hex rung, falls through to title matching and answers "unknown". A model
   taught to write `chat://` **will** hand it back — this is a latent defect
   the moment (1) is fixed, not a new one this design creates.

## 4. Design

### 4.1 The address

`chat://<short-id>` where `<short-id>` is what `short_id` already prints — 8
lowercase hex. One producer, one format, everywhere: results, prompt, feed.
Detection accepts 4–32 hex (the resolver's existing floor and ceiling) so a
model that pastes the full uuid is not punished.

### 4.2 Teaching the model

The address is minted by the two localized strings that already carry it, so
that is where the format changes (`[a1b2c3d4]` → `chat://a1b2c3d4`), plus one
sentence of steering in `tool.chat_search.hint` and the two `desc` strings:
cite a conversation as `chat://<id>` **when you mention it to the user**, and
the interface turns it into something they can open. "When you mention it" is
load-bearing — the request is *use it as needed*, not decorate every sentence.

This costs **zero tokens when the pair is off**: a disabled tool is not
advertised at all (spec §9.11), and off is the default. It also keeps the
teaching next to the thing that mints addresses, which is the only place the
model can act on it.

`resolve` gains one line — strip a leading `chat://` before the hex test —
closing gap 4 and letting the model round-trip its own citation into
`chat_read`.

### 4.3 Recognising a reference in the feed

A scanner over rendered text, in the shape of `match_ranges`: find
`chat://<hex>` tokens, trim trailing punctuation the way `speak.rs` does,
return byte ranges. Two candidate layers, decided in F3:

- **at block build time**, before wrapping
  ([`build_message_block`](../../src/widgets/message_feed.rs)), so the styling is
  baked into `CachedBlock.lines` and survives the wrap; or
- **post-render, per drawn line**, exactly where the search highlight patches
  cloned lines ([message_feed.rs:815-833](../../src/widgets/message_feed.rs)).

The difference is not stylistic. A reference is 15 columns
(`chat://` + 8 hex) and `wrap::wrap_line` will split it on a narrow terminal;
post-render matching is per line and would miss exactly those — the same
documented limitation the search highlight lives with
([message_feed.rs:1045-1054](../../src/widgets/message_feed.rs)). Build-time
detection has no such hole, because it runs before the wrap. Its price is that
the styling is now cache-dependent state: whether a reference resolves depends
on the address book, so the address book joins `CacheKey` — which is exactly
what `role_names: CharacterNames` already does there
([message_feed.rs:304-339](../../src/widgets/message_feed.rs)), and unlike the
search query it changes on chat create/delete, not on every keystroke.

Everything drawn goes through the same builder, so a reference lights up in
assistant text, user text, thoughts **and** tool cards — including the
`chat_search` result card itself, where the addresses were printed in the first
place.

### 4.4 Resolving

Resolution is the tool resolver's rung 1 only (id prefix), against the
**screen's** address book rather than the turn snapshot: title matching is a
convenience for a model writing prose, not something to infer from a URI.

Scope, mirroring spec §9.5: the current profile's non-hidden chats. The
screen's `chats: Vec<ChatSummary>` is nearly that already — it is missing
`profile_id`, so either the summary gains the field or the orchestrator pushes
a pre-filtered id set (F5). The current chat is *included* in the resolvable
set even though the tools exclude it: a reference to the open conversation
should read as "you are already here", not as a dead link.

**Style only what resolves.** An unknown id is drawn as ordinary text, so the
user never clicks a link that cannot go anywhere — docs/lessons.md §4, "only
advertise what exists". This is also the answer to "what should a reference to
another profile's chat do": nothing, and it does not look like it would.

### 4.5 Activating

`SwitchChat(id)` — a chat-level address names no message. F4 decides how a
reference is activated; the recommendation is a **keyboard route first, mouse
as convenience**, which is the shape `/image paste` vs `Ctrl+V` already settled
in this project: the command works in every terminal, the key is the comfort.
Here the universal route is a picker over the chat's resolvable references
(newest first, deduplicated, title + date), because mouse capture is off by
default and turning it on costs native selection.

### 4.6 Closing the door

- No references in this chat → "nothing here", worded distinctly from "the one
  you meant could not be resolved" (lessons §4).
- A reference to the **open** chat → say so; do not silently do nothing.
- An unresolvable reference is never styled, so it never offers a door at all.

### 4.7 Security and privacy

A `chat://` in *tool output* — a fetched page, an MCP result — would render as
a live link if it happened to name one of the user's chat ids. The exposure is
bounded by construction: the target set is the user's own current-profile
chats, activation is an explicit user action, and the effect is switching the
view. No content leaves the machine, nothing is written, and the scheme is
never handed to a browser opener (the project has none — grep for `osc`,
`open::that`, `webbrowser` returns nothing in `src`). Worth stating in the
spec, not worth a gate.

## 5. Forks (for the user's decision)

- **F1. Scope of the track.**
  (a) **Both halves — teach the format *and* make it navigable** *(recommended)*.
  (b) Teaching only: the model emits `chat://` consistently, the renderer keeps
  drawing it as text. Cheap (two locale strings) but delivers nothing the user
  can do.
  (c) Navigation only: works for whichever model happens to invent the scheme.
  User's decision: **(a) — both halves** (2026-08-14).

- **F2. Which form to teach.**
  (a) **Markdown, `[Title](chat://<id>)`** *(recommended)* — renders as
  `Title (chat://a1b2c3d4)`: a readable label *and* a copyable address, and the
  suffix is already one clean span in link style.
  (b) Bare `chat://<id>` — terser, but on screen it is an opaque hex with no
  hint of which conversation it is.
  Detection handles **both** either way; this fork only decides what the
  strings tell the model to write.
  Not put to the user separately; **(a) carries by recommendation**.

- **F3. Where detection happens.**
  (a) **At block build time**, before the wrap; the address book joins
  `CacheKey` *(recommended)* — no wrap hole, and the work is done once per
  message instead of once per frame.
  (b) Post-render on drawn lines, mirroring the search highlight — no cache
  changes at all, at the price of missing a reference split across two rows.
  User's decision: **(a) — build time, address book in `CacheKey`** (2026-08-14).

- **F4. How a reference is activated.**
  (a) **Keyboard picker first (a free key — `Ctrl+L` is unused in the chat
  screen), mouse click as a second stage** *(recommended)*.
  (b) Mouse click only — needs `MessageFeed::last_area` + a row/column→reference
  map, and is unreachable for every user who leaves `Ctrl+W` off, which is the
  default.
  (c) Keyboard picker only; no mouse ever.
  (d) A typed command (`/chat <ref>`) — cheapest, least discoverable.
  User's decision: **(a) — keyboard first, mouse as stage 2** (2026-08-14).

- **F5. Where the resolvable set comes from.**
  (a) **Add `profile_id` to `ChatSummary`** and filter in the screen
  *(recommended)* — one `#[serde(default)]`-free in-memory field, and the
  chat-list screen may want it later anyway.
  (b) A new `AppEvent` carrying a pre-filtered id set — no entity change, one
  more event and one more thing to keep in sync.
  Not put to the user separately; **(a) carries by recommendation**.

- **F6. Coming back after a jump.**
  (a) **No back-stack in stage 1** *(recommended)* — `Esc` keeps its current
  meaning (the chat list), where the origin chat is one selection away.
  (b) Extend the one-deep back-stack (`SearchReturn`,
  [runtime/mod.rs:122-135](../../src/app/runtime/mod.rs)) with a chat-origin
  variant so `Esc` retraces the link. Real complexity: the `esc_target` hint,
  the `clear_search_return_if_left` funnel and a second stash kind.
  Not put to the user separately; **(a) carries by recommendation**, and a live
  run is the honest judge — if following a link and losing the way back reads
  as a trap in real use, (b) is stage 2's neighbour.

- **F7. Whether the tool output changes its printed address.**
  (a) **Yes — print `chat://a1b2c3d4` instead of `[a1b2c3d4]`** *(recommended)*:
  one address form everywhere, so the model never translates between "what I
  was given" and "what I should cite". `resolve` accepts both.
  (b) No — keep the brackets and teach the citation form separately; two forms
  for one address, which is how a model ends up emitting the wrong one.
  User's decision: **(a) — one address form everywhere** (2026-08-14).

- **Considered and rejected here:** replacing the address on screen with the
  conversation's title (destroys the copyable address, interacts with `Ctrl+F`
  matching, and `shared/markdown` cannot know about chats without breaking
  FSD); OSC 8 terminal hyperlinks (terminal-dependent, and the target is
  in-application, not a URL); message- or page-level addressing
  (`chat://<id>/p3`) — deferred to §7 groundwork; teaching the scheme when the
  cross-chat pair is off (the model would have no addresses to cite).

## 6. Staging

Per the decided forks:

- **Stage 1 — the format and the keyboard route** (`feat/chat-uri-links`):
  locale strings + the `resolve` prefix strip (gap 4), the detector, build-time
  styling, the address book into the feed, the picker and its key, help/README.
- **Stage 2 — the mouse** (`feat/chat-uri-click`): `MessageFeed::last_area`,
  row→block→reference mapping, the `Down(Left)` arm in `handle_mouse`.

Stage 1 stands alone and is the whole user-visible feature; stage 2 is comfort
for users who run with capture on.

## 7. Test plan

Unit:
- detector — bare and markdown forms, trailing punctuation (`chat://id.`,
  `(chat://id)`), upper-case hex, too-short hex, 32-hex full uuid, two
  references on one line, a reference straddling two spans (bold/code), and a
  reference that would be wrap-split at a narrow width;
- unknown id is **not** styled; another profile's id is not styled; the open
  chat's id resolves and reports "already here";
- `resolve` accepts `chat://a1b2c3d4`, `a1b2c3d4` and `A1B2C3D4` alike, and
  still rejects a 3-hex needle (the existing floor);
- picker — feed order, dedup, empty case wording, en/ru with no `{}` leftovers
  and no Cyrillic in en;
- `CacheKey` — a changed address book resets the cache; an unchanged one does
  not.

Live smoke (`MINDFORK_ENGINE_URL`-gated, the track's go/no-go, AGENTS.md §3 —
this touches tool descriptions and the prompt): the existing cross-chat smoke
extended — a profile narrowed to the pair, a planted token in chat A, the
question asked in chat B; assert the tool was called **and** the answer carries
a `chat://` reference whose id resolves to chat A. Run against the user's live
stack and, if the wording proves model-sensitive, against a cloud model too —
"the model uses the format" is a behavioural claim and only a live run settles
it.

## 8. Documentation (AGENTS.md §4)

spec §9.11 (the printed address and the citation instruction) + §11.3 (the
reference in the feed, the key, the degradation) · architecture §10 (the feed's
new input and the detector) · `docs/journal/ui-feed.md` (primary) and
`docs/journal/tools.md` (the address/prompt half) · CHANGELOG (Added) ·
README + the `F1` help overlay for the new key · roadmap: the item leaves the
list, groundwork below stays.

## 9. Groundwork this deliberately leaves open

- **Message- and page-level addresses** (`chat://<id>/p3`): `chat_search`
  already names the page, `HistoryView::locate`
  ([features/compaction.rs:201](../../src/features/compaction.rs)) can map a
  page back to a message, and `OpenChatAt` already takes a message uuid — so a
  hit could open exactly where it matched. Left out because the address format
  should prove itself before it grows a path component.
- **Other organs' addresses** (`note://`, `attachment://`): the same
  mechanism would serve them, and the notes graph already has ids. Deferred
  until the chat scheme is real; a URI vocabulary invented ahead of one live
  consumer is a vocabulary nobody speaks.
- **A reference to another profile's chat**: resolves to nothing by design
  (§4.4). If cross-profile navigation is ever wanted, the chat list already
  switches across profiles — the scope decision, not the mechanism, is what
  would change.
