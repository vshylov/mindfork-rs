# Impersonation on Gemma's template — the swapped conversation must open with the assistant's silence

> **Status:** proposed (2026-09-09); stage 0 is the measurement in §2.1
> (five role shapes against Gemma 4 31B on the LAN stack and Gemma 3 4B
> on the CPU build, then three impersonation shapes on both). The defect
> the one-shot samples track found in passing
> ([oneshot-samples.md](oneshot-samples.md) §7): impersonation swaps the
> roles of the history, so a chat that opens with the user's message
> becomes a conversation that opens with an **assistant** turn, and the
> Gemma 3 chat template refuses it — `Conversation roles must alternate
> user/assistant/…`, a `400` before any prefill. Measured, the template
> refuses two same-role turns in a row too, and delivers the system
> prompt only inside a leading user turn: without one the persona never
> reaches the model. Gemma 4's template accepts every shape.

## 1. Why, precisely

Impersonation (spec §11.8) builds its request by swapping user and
assistant in the history under the persona's system prompt: the model
plays the human, the assistant's replies come to it as user turns. Every
chat the user opens therefore swaps into a conversation whose first turn
is an assistant's — the human's opening line. The spec records that
shape as accepted by Anthropic, native Gemini and OpenAI Responses
(measured 2026-08-08), and Qwen's template on `llama-server` takes it as
well. Gemma 3's does not: its Jinja template checks `(role == 'user') !=
(index % 2 == 0)` and raises, and `llama-server` answers `400` before a
token is processed — so on that template family `Ctrl+U` fails on every
chat the user opened, which is nearly every chat.

Two more facts the same probe showed (§2.1):

- **Two same-role turns in a row are refused too.** `swap_role_message`
  drops system, tool and empty messages, so a reply that was nothing but
  tool calls, or a cancelled empty one, leaves two user messages adjacent
  in the history — two assistant turns after the swap. Gemma 3 raises on
  those the same way.
- **The system prompt travels inside the first user turn, and only
  there.** Gemma 3's template has no system turn: it prepends the system
  text to the first user message. A conversation with no leading user
  turn — a lone assistant turn, or no turns at all — loses the persona
  entirely: 4 tokens of prompt for a 54-token system, and the model
  invented a task. Where the swapped history *is* accepted, then, the
  persona still has to open the conversation.

The fix is the shape of the swapped conversation, not the model: it has
to alternate, and it has to open with a user turn. Two ways do that
(§3), and measured on both Gemma 4 and Gemma 3 they cost the same tokens
and give the same reply as the natural shape gives where it is accepted.

## 2. What is there

### 2.1 Measured (stage 0)

Gemma 4 31B (`gemma-4-31B_q4_0-it`, the LAN stack, one slot over
16 384) and Gemma 3 4B (`gemma-3-4b-it-q8_0`, the CPU build), through
`/v1/chat/completions` directly, reasoning off, the persona as the
system. First, which shapes each template takes:

| shape | Gemma 4 | Gemma 3 |
|---|---|---|
| system + `[assistant, user]` — a user-opened chat, swapped | a reply, 84 tokens of prompt | **400**, roles must alternate |
| system + `[assistant]` — only the user's opening, swapped | a reply (the opening repeated) | a reply, **18** tokens of prompt: the system dropped |
| system + `[user, user]` — two same-role turns | a reply | **400** |
| system + `[user, assistant, user]` — the ordinary shape | a reply | a reply, the system inside the first user turn |
| system only, no turns | a reply in the persona's voice, 54 tokens | a reply about a ceramic mug, **4** tokens: the system dropped |

Gemma 4's template (16.9 KB, `<|turn>` markup) has no `raise_exception`
and renders `assistant` as `model` wherever it stands. Gemma 3's folds
the system into the first user turn and raises on the parity of every
other.

Then the impersonation shapes on a four-message chat the user opened
(`U1` "a book about lighthouses", `A1`, `U2` "history", `A2` "shall I
find a copy?"), the model asked for `U3`:

| shape | Gemma 4 | Gemma 3 |
|---|---|---|
| (i) natural: `[a(U1), u(A1), a(U2), u(A2)]` | 130 tokens, *"That sounds perfect. Yes, please find a copy for me!"* | **400** |
| (ii) the opening folded into the system, `[u(A1), a(U2), u(A2)]` | 135 tokens, the same reply | 127 tokens, *"Yes, please do! That sounds perfect."* |
| (iii) an empty opening user turn, `[u(""), a(U1), u(A1), a(U2), u(A2)]` | 135 tokens, the same reply | 127 tokens, *"Yes, that sounds perfect, thank you!"* |
| (iv) only the opening: the fold + an empty user turn | a follow-up in the human's voice | a reply in the **assistant's** voice |
| (v) system + `[u("")]` — does the persona arrive? | 73 tokens, the human's voice | 65 tokens, the human's voice |

Both repairs give Gemma 4 the reply the natural shape gives, at five
tokens more; both give Gemma 3 a reply where the natural shape gave a
`400`. The one divergence is the opening-only chat under the fold
(iv): the 4B model answered as the assistant — one sample, and the
shape §3 keeps for that case is the natural one, which both templates
accept (§3.3).

### 2.2 The code

- `build_impersonation_request` (`impersonation.rs:185`): the persona as
  the system (the compaction summary and the user hint appended), the
  history from the compaction cut through `swap_role_message` — user ↔
  assistant, system/tool/empty dropped — the seed as a continuation hint
  in the system, no tools, reasoning muted.
- `swap_role_message` (`:266`), and the tests that pin the shape:
  `impersonation_request_swaps_roles_and_sets_system` (the first swapped
  turn is the assistant's), `compacted_impersonation_starts_with_assistant_and_ends_with_user`
  (a compaction cut lands on a user message, so the swap opens with an
  assistant turn; the tail ends on `user` because a trailing assistant
  turn reads as a prefill).
- spec §11.8: the leading assistant turn recorded as accepted by the
  three clouds — the Gemma 3 template unmeasured until now.
- The wire (`openai/wire.rs`): the messages as given; `reasoning_budget`
  and `enable_thinking` both sent. Nothing reshapes a conversation for a
  template today.
- `prompt.impersonation.default` / `.continue` in both locales — the
  persona's default and the seed's hint.

## 3. Design

### 3.1 The swapped conversation alternates, and opens with a user turn

One pure function on the swapped list, `alternate_for_template`
(`impersonation.rs`, beside `swap_role_message`), applied by
`build_impersonation_request` before the seed's hint:

1. **Adjacent same-role turns are merged** — texts joined with a blank
   line — so the list alternates. Two of the human's messages with no
   reply between them are one assistant turn after the swap; two
   assistant replies with no user message between them (the one
   between was tool calls only) one user turn.
2. **A leading assistant turn that a user turn follows is folded into
   the system prompt**: the persona gains one localized paragraph —
   `prompt.impersonation.opening`, *"Your first message in this
   conversation was:"* and the text — and the list opens with the first
   user turn, the assistant's reply. Content and order are what they
   were; only the opening line moves from a turn into the persona's own
   words about itself.

The result is the shape (ii) of §2.1: accepted by Gemma 3, five tokens
more on Gemma 4 for the same reply, and — since the system text is where
every provider takes anything — the same request on the clouds, which
took the natural shape already.

### 3.2 Why the fold and not the empty turn

Shape (iii) — an empty user turn ahead of the human's opening — reads
the same to Gemma 3 (the system folds into it) and gave the same reply
on both models. It is not the recommendation for one reason: Anthropic
rejects a message with empty text, so it would need a provider-specific
branch or a visible marker the model reads as the assistant's utterance.
The fold is one sentence in the system prompt, and the system prompt is
the one place every provider accepts as is.

### 3.3 The opening-only chat, and the tail

A chat with only the user's opening swaps into a lone assistant turn,
which both templates accept (§2.1), and the model **continues** that
turn — the trailing-assistant rule the existing test pins, and what
happens today on every provider. §3.1 leaves it alone: there is no user
turn to lead, and the fold with an empty user turn behind it made the
4B model answer as the assistant (iv). On Gemma 3 the persona does not
reach the model in that shape (the system is dropped); the continuation
is then the model's own — recorded in §7 as the one shape this track
does not repair. The tail is untouched: a compaction cut or a reply
still ends the list on a user turn, as before.

### 3.4 What changes in the numbers

On Gemma 3, every `Ctrl+U` on a user-opened chat: a reply instead of a
`400`. On Gemma 4, Qwen and the clouds: the same reply for five more
tokens of system prompt, on a request whose prompt is the whole
conversation (10 642 tokens in the last track's session). The estimate
smoke's impersonation ratio moves by a rounding.

## 4. Difficult spots

- **The compaction cut.** The cut lands on a user message; after the
  swap the list opens with an assistant turn *and* the system already
  carries the summary block. The fold appends the opening line after
  the summary and the user hint, before the seed's hint: the persona,
  what was folded away, who the human is, what the human first said,
  what to continue — in that order.
- **The seed.** A seed is a hint in the system, not a trailing assistant
  turn; the fold and the seed do not meet in the message list.
- **Merging is by text.** Two turns become one string with a blank line
  between; nothing of the message metadata matters to the request.
- **The tests that pin the leading assistant turn** pin the defect: they
  are rewritten to pin the fold (§6).
- **Clouds.** The natural shape was measured accepted on the three
  clouds in August; the folded shape is a strict subset of it plus a
  system sentence. Not re-measured on the clouds in this track: nothing
  in §3.1 is provider-specific.

## 5. Forks

- **F1. The leading assistant turn.** (a) **Folded into the system
  prompt when a user turn follows** *(recommended — one localized
  sentence, provider-agnostic, measured on both Gemma templates)*. (b)
  An empty opening user turn — the same replies, but Anthropic rejects
  empty text, so a branch or a marker. (c) Dropped — the human's opening
  lost. (d) Refused on a strict template — the feature stays broken
  there.
- **F2. Where the shape is repaired.** (a) **Always, in
  `build_impersonation_request`, for every provider** *(recommended —
  one request shape, one set of tests; the system sentence costs five
  tokens where the natural shape worked)*. (b) Local servers only
  (`Managed`/`External`, the shared engine's mode) — a second shape to
  test, for no measured gain. (c) On a `400` that says "alternate",
  resend repaired — error-string matching, and a second code path.
- **F3. Adjacent same-role turns.** (a) **Merged with a blank line**
  *(recommended — Gemma 3 refuses them; a lenient template reads the
  merge as it read the pair)*. (b) Left as they are.
- **F4. The opening-only chat.** (a) **Unchanged — a lone assistant
  turn, continued** *(recommended — accepted by both templates, today's
  behaviour everywhere; the fold with an empty user turn misfired once)*.
  (b) The fold plus an empty user turn — a new message rather than a
  continuation, at the cost of (iv). (c) Refuse until the assistant has
  replied.
- **F5. The live run.** (a) **The one-shot smoke's seed loses its
  assistant opener** — the natural, user-opened chat — **and runs on
  Gemma 4 (the LAN stack) and Gemma 3 (the CPU build)**; the estimate
  smoke's impersonation on Gemma 4 for the ratio *(recommended)*. (b) A
  new smoke — nothing the existing ones do not already send.

## 6. Tests and the live run

- `tests/impersonation.rs`: `alternate_for_template` — a user-opened
  chat folds its opening into the system and opens with the assistant's
  reply; adjacent same-role turns merge, in both roles; a chat the
  assistant opened is untouched; the opening-only chat stays a lone
  assistant turn. `impersonation_request_swaps_roles_and_sets_system`
  and `compacted_impersonation_starts_with_assistant_and_ends_with_user`
  rewritten to pin the fold: the list opens with `user`, the system
  carries the opening after the summary and hint, the tail still ends on
  `user`.
- Locales: `prompt.impersonation.opening` in `en` and `ru`.
- Live (F5a): `impersonation_prefill_e2e_live` with the natural seed on
  Gemma 4 (a reply, no `400`) and on Gemma 3 on the CPU build (a reply,
  the note); `prompt_estimate_e2e_live` on Gemma 4 for the ratio.

### 6.1 Runs

Stage 1.

## 7. Not in this track

- **The opening-only chat on Gemma 3** — the lone assistant turn is
  accepted but the persona is dropped with it (§2.1); a shape with no
  right answer measured (iv), left as today's continuation.
- **A dialogue's first line** — a scene's opening request may carry a
  system and no turns; on Gemma 3 that drops the persona the same way.
  Unmeasured; its own question ([ADR 0011](../decisions/0011-dialogue-directed-run.md)).
- **The clouds re-measured** — the folded shape is the natural one plus
  a system sentence; the August measurement stands.

## 8. Documentation touch list (AGENTS.md §4)

- spec §11.8 (the request's shape: alternating, opening with a user
  turn; the Gemma 3 template's rules).
- architecture §5 or §9 wherever `build_impersonation_request` is
  described (grep at stage 1).
- CHANGELOG (Fixed: impersonation on Gemma 3 answered `400` on every
  chat the user opened).
- [oneshot-samples.md](oneshot-samples.md) §7 (the item done here),
  journal `engine.md`, CLAUDE.md's status line and count; locales.
