# Research: image input for local and cloud models (multimodality)

**Status:** research complete; **user's decision, 2026-08-12: all eight forks go
with the recommendations below** (F1 = staging onto the next message, F2 =
base64 in the chat JSON, F3 = the `image` crate with downscale-on-attach, F4 =
`/props` probe + optimistic unknown, F5 = per-image label parts on, F6 = a chip
line in the feed, F7 = the stated caps, deferred items stay deferred).
**Implemented — both stages, track complete** (stage 1: local + Grok, 2026-08-12;
stage 2: Anthropic, Gemini and OpenAI Responses, 2026-08-13). Outcome recorded in
[docs/journal/engine.md](../journal/engine.md) and spec §9.10; what was left open
on purpose is in [docs/roadmap.md](../roadmap.md). The request: attach images to
messages with commands mirroring the file attachments —
`/image attach <path>`, `/image remove <name|#N>`, `/image list` — so that
vision-capable models can see them: the local Gemma 4 via `llama-server
--mmproj`, and the four clouds.

Everything in §2 was verified **live** on 2026-08-12: the reference stack
(`gemma-4-31B_q4_0-it.gguf` + its `mmproj` from
`google/gemma-4-31B-it-qat-q4_0-gguf`, llama.cpp build `b10322`) and one tiny
real-key request per cloud provider. Wire shapes below are measured, not read
off the docs.

Related: [ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (the
engine contract this slots into), [file-attachments.md](../file-attachments.md)
(the `/file` feature this mirrors — and deliberately differs from, §5 F1),
[grok-xai-provider.md](grok-xai-provider.md) (Grok rides the same client as
llama.cpp — one wire change serves both),
[cloud-retry-backoff.md](cloud-retry-backoff.md) (the retry decorator clones
requests — relevant once they carry megabytes).

## 1. Problem

The app cannot carry an image at any layer today. `Message.text`,
`ApiMessage.content` and three of the four wire formats are bare strings;
MCP tool results already *parse* image blocks but collapse them to
`[image content omitted]` (`shared/mcp.rs`); the markdown renderer prints
`![alt](url)` as text by design (ADR 0003). Unlike the Grok track, there is no
config-only baseline: every path needs the content model widened first.

Meanwhile the engines are ready. llama.cpp has served vision models via
`--mmproj` (libmtmd) for over a year — Gemma 3/4, Qwen 2.5-VL and others — and
every cloud provider the app supports takes image content parts on the exact
endpoints our clients already call.

What this document answers: the wire shapes (verified), where image bytes live,
how capability is detected, what the UX is, and the fork decisions needed
before implementation.

## 2. What we learned live (2026-08-12)

### 2.1 llama-server + mmproj: it all just works

The reference stack (`http://192.168.1.20:8000`, `-c 16384`, one slot) with the
mmproj loaded. `GET /props` now reports — machine-readably:

```json
"modalities": { "vision": true, "video": true, "audio": false }
```

plus a `media_marker` string (the internal placeholder mtmd substitutes for
media in the templated prompt). `/props` is the same endpoint
`context_budget()` already reads — capability detection piggybacks on it (§4.5).

Probes against `POST /v1/chat/completions`, OpenAI content-parts shape,
`temperature: 0`. Images are generated solid-color/geometric PNGs, so the
assertions are objective:

| # | Probe | Result |
|---|---|---|
| P1 | one 128×128 red PNG as `{"type":"image_url","image_url":{"url":"data:image/png;base64,…"}}` + "what color?" | **"Red"** |
| P2 | blue 128×128 with a centered white square, "background color and shape?" | **"The background color is blue, and a white square sits in the center."** |
| P3 | image in the **first** turn, text-only follow-up asking about it (the replay shape every request uses) | **"Red"** |
| P4 | **two** images in one message, "first image color, then second background" | **"Red blue"** |
| P7 | 3000×2000 PNG (1.5 MB) | accepted, resized server-side, answered correctly |

Findings that shape the design:

- **Thoughts pipeline unaffected.** Gemma 4's reasoning arrived in
  `message.reasoning_content` exactly as on text turns. (First probe run used
  `max_tokens: 64` and got empty answers — the known reasoning-starvation trap,
  lessons §3; raising the cap fixed all of them.)
- **Image token cost is dynamic and the server reports it.** Measured by diff
  against a text-only baseline (27 tokens): 128×128 → **+51** prompt tokens;
  1344×896 → **~+258**; 3000×2000 → **~+258** (resized down to the same encoder
  budget). `usage.prompt_tokens` **includes** image tokens, so the
  auto-compaction trigger (spec §6.7 — exact usage only, never the byte
  estimate) keeps working with no change.
- **The prefix cache survives images.** Re-sending the identical conversation:
  `timings.cache_n = 73` of 78 prompt tokens (only the tail re-processed).
  Appending one more turn after an image turn: `cache_n = 73`, `prompt_n = 23`
  — only the new turn was prefilled. mtmd hashes media chunks, so the
  append-only replay of base64 images every turn is **not** re-encoded — the
  same economics as text history. Re-sending bytes costs LAN bandwidth, not
  GPU time.
- **Oversized images do not fail.** The server decodes and resizes internally
  (stb_image: jpeg/png/bmp/gif and friends). No documented size cap; a 1.5 MB
  upload was instant on LAN.
- From the server README, for completeness: `image_url.url` may also be a
  remote URL or a local path (with `--media-path`); `--image-min-tokens` /
  `--image-max-tokens` bound dynamic-resolution encoders. Defaults were right
  in every probe — no knobs proposed.

### 2.2 The four clouds: one probe each, all green

The same 128×128 red PNG, one request per provider, real keys, cheapest
vision-capable models. Every answer was "Red".

| Provider (model) | Wire shape verified | Prompt tokens |
|---|---|---|
| **OpenAI** (`gpt-5-nano`, Responses) | `input` content: `{"type":"input_text",…}` + `{"type":"input_image","image_url":"data:image/png;base64,…"}` | 41 |
| **Anthropic** (`claude-haiku-4-5`, Messages) | content block `{"type":"image","source":{"type":"base64","media_type":"image/png","data":"…"}}` | 46 |
| **Gemini** (`gemini-2.5-flash`, `generateContent`) | part `{"inline_data":{"mime_type":"image/png","data":"…"}}` | 270 |
| **Grok** (`grok-4`, Chat Completions) | `{"type":"image_url","image_url":{"url":"data:…"}}` — **byte-identical to the llama.cpp shape** | 223 |

Documented constraints that matter to us (from the providers' current docs):

| | Formats | Per-image cap | Count cap | Token model |
|---|---|---|---|---|
| OpenAI | png, jpeg, webp, non-animated gif | (512 MB payload total) | 1500 | `detail`: `low`/`high`/`original`/`auto` |
| Anthropic | jpeg, png, gif, webp | **10 MB** base64; 8000×8000 px | 100 (200k-ctx models) | `⌈w/28⌉×⌈h/28⌉`, downscaled to ≤1568 px long edge / 1568 tok (standard tier; 2576 px / 4784 tok on high-res models) |
| Gemini | png, jpeg, webp, heic, heif | 20 MB whole request (inline) | 3600 | 258 tok if ≤384px, else 768×768 tiles à 258 |
| xAI | **jpg, png only** | 20 MiB | unlimited | ~256 measured |

Useful nuances: Anthropic **auto-downscales** oversized images (only >10 MB or
>8000 px is an error) and recommends **images before text** plus a short text
label per image ("Image 1:") when sending several; Gemini inline data counts
against the 20 MB request; xAI ignores unknown fields silently (the
wrong-type probe discipline from the Grok research applies if we ever probe
support); the old text-only models (e.g. `grok-3`) simply error — and the
error-surfacing track already turned that into a readable feed message.

### 2.3 What this settles

- **No new protocol, no new client.** All four existing clients gain image
  parts inside the request builder they already have. The llama.cpp change and
  the Grok change are the *same* change.
- **The replay economics hold.** Append-only history with inline base64 is
  cache-friendly locally (measured) and standard practice on the clouds
  (Anthropic's Files API exists to shrink payloads, not a correctness need).
- **Go.** The MVP probe criterion (a vision answer through the exact request
  shape the app would send, including history replay) passed on the reference
  stack and on every cloud.

## 3. Baseline

None. Unlike Grok ("external mode already works"), nothing usable exists
without code: the content model is text-only end to end. The nearest existing
machinery — `/file attach` — solves a *different* problem (standing text
context in the system prompt); images cannot ride the system prompt at all
(every provider takes them as **message** content parts only).

## 4. Architecture: where it lands

The `/file` slice (parser → intents/commands/events → orchestrator handler →
entity → UI chip/notes → i18n) transfers almost mechanically; the genuinely new
work is the content model (§4.2–4.3). File references below are current `main`.

### 4.1 Domain: images are message-scoped

New entity `MessageImage` (in `entities/`): `id`, `name` (display), `source`
(canonical path at attach time), `mime`, `width`, `height`, `bytes` (original
size), `est_tokens`, and the payload as **base64 `data`** (storage fork — §5
F2). On `Message`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub images: Vec<MessageImage>,
```

Additive per ADR 0006 F12 — old chats read unchanged, a chat with images
opened by an older binary keeps the field intact on rewrite only if the older
binary never saves that chat (the usual additive-field caveat; acceptable, same
as every field added since M0).

**Staging.** `/image attach` does not create a message; it stages the image for
the **next send** (fork F1). The staged list lives in the orchestrator (sole
owner of chat state), keyed by chat id, session-only. On send, staged images
move onto the new user `Message`. `/image list` numbers the staged set (`#N`),
`/image remove <name|#N>` unstages; a status-bar chip shows `▣ 2 (~1.2k)`
while anything is staged.

### 4.2 Contract: `ApiMessage` grows an images field

`shared/api/contract.rs:85` — `ApiMessage.content: String` stays; a new
`images: Vec<ApiImage>` (mime + base64 data, `Arc`-backed so the retry
decorator's `req.clone()` per attempt stays cheap) rides alongside. The
existing constructors keep compiling; `ApiMessage::user_with_images` is the one
addition. `ChatRequest` is untouched.

Domain → contract happens in one place (`app/orchestrator/request.rs:15
message_to_api`), so images flow into requests with a one-line change there —
including impersonation and every agentic-loop round, which build through the
same path.

### 4.3 Wire adapters: the real work, sized per backend

| Backend | Today | Change |
|---|---|---|
| **OpenAI-compat** (`openai/wire.rs:121` — llama-server managed/external **and Grok**) | `WireMessage.content: Option<String>` | widen to `#[serde(untagged)] enum WireContent { Text(String), Parts(Vec<Value>) }`; emit parts **only when images are present** — a no-image request stays byte-identical (prefix caches, existing tests, and the Gemma template path see zero difference; pinned by a serialization test) |
| **Anthropic** (`anthropic/wire.rs:68`) | content is already `Vec<AntBlock>` | add `Image { source }` variant; extend `text_blocks`; the adjacent-role merge already handles mixed blocks. Smallest change |
| **Gemini** (`gemini/wire.rs:280`) | parts are untyped `Vec<Value>` | emit `{"inline_data":{…}}` next to `{"text":…}`; in-tree precedent: `shared/video/gemini.rs:43` already pushes `file_data` parts |
| **OpenAI Responses** (`responses/wire.rs:150`) | `"content": m.content` inside `json!` | emit `[{"type":"input_text",…},{"type":"input_image",…}]`; pure `json!` edit |

Part order: **images first, then text** (Anthropic's documented preference;
harmless elsewhere — P1–P4 used text-first and worked, so this is optimization,
not correctness), with an optional per-image label part (§5 F5).

### 4.4 Managed server: the `--mmproj` flag

Four mechanical edits, mirroring the draft-model flag: `mmproj: Option<PathBuf>`
on `ManagedSettings` (`shared/config.rs:246`) and `ManagedConfig`
(`managed.rs:19`), emitted in `build_args` with the same `is_file()` preflight
as `-md` (`managed.rs:185`), mapped in `supervisor.rs:395`, one settings row
next to the GGUF path (`screens/settings/helpers.rs:165`), plus a
`MINDFORK_MMPROJ` env override next to `MINDFORK_MODEL` (`main.rs:763`) and a
line in install.md.

### 4.5 Capability: ask the engine, never guess from names

New method on `EngineBackend` in the `context_budget` mold (default
`Unknown`):

- `OpenAiClient` reads `modalities.vision` from the **same `/props` fetch**
  `context_budget` already makes — covering managed and external llama.cpp. A
  server without `/props` (vLLM, LM Studio, a proxy) → `Unknown`.
- Cloud backends return a static `Yes` — every current-generation model on all
  four providers takes images; a legacy text-only model produces a clear
  provider error on send, which the error-surfacing track already renders
  properly. Model-name allowlists are rejected for the same reason the Grok
  research rejected name-based reasoning detection: they go stale and lie.
- `RetryBackend` **delegates** — with a unit test, because a decorator
  silently defaulting to `Unknown` is exactly the class of hole the retry
  track's live-coverage lesson documented.
- **Closing the door** (lessons §4): capability is consulted at **attach
  time** — `Known(false)` refuses with "the current engine reports no vision
  support" naming the mmproj setting for managed mode; `Unknown` attaches with
  a neutral note; send-time failures surface the provider's own error. The
  refusal message never points at a command that would itself refuse.

### 4.6 Token accounting

- **Pre-flight estimate** (status chip, `estimate_prompt_tokens` at
  `generation.rs:1344`, where images currently count 0): Anthropic's patch
  formula `⌈w/28⌉ × ⌈h/28⌉` capped at 1568 — measured against reality it
  over-estimates Gemma 4 (~51–258) and roughly matches Gemini/xAI (~256), which
  is the safe direction for a *display* estimate.
- **The trigger stays exact.** Auto-compaction consumes `usage.prompt_tokens`,
  which the server reports **with** image tokens included (measured, §2.1) — no
  change.
- **Compaction interplay:** images live on messages, so a folded range takes
  its images out of the request (that is the relief working as designed). The
  roll digest represents them textually (`[image: chart.png]`), and
  `history_read` returns the same marker — the folded image is *gone* from the
  model's sight, and the digest must say so rather than imply it can be
  re-opened.

### 4.7 The `/image` slice (mechanical, mirrors `/file`)

`features/image_command.rs` (tri-state `parse`, `resolve_target` for `#N`,
localized errors + the `*_for_all_langs` gate), `ChatIntent`/`AppCommand`/
`AppEvent` variants, an orchestrator `images.rs` handler (path validation,
decode/probe off the command loop via `spawn_blocking`, dedupe by source),
`screens/chat` notes + chip, `HELP_COMMANDS` entries, `ui.image.*` keys in
**both** locale files, README key table, spec section. The `/file` map that
grounds this is in the feature's own doc; nothing there needs invention.

### 4.8 Out of scope (this track)

Image **generation** (no provider here generates through these endpoints);
audio/video modalities (the domain type is image-specific on purpose — a
general content-part enum can subsume it later if ever needed); RAG over
images / CLIP embeddings; rendering received images (nothing produces them);
`attachment_search`-style indexing (nothing to index).

## 5. Forks (need the user's decision before implementation)

### F1 — what does an image attach *to*? *(shapes the whole UX)*

| | A. The next message (staging) | B. The chat, standing (like `/file`) |
|---|---|---|
| Semantics | "look at this" — image belongs to the turn that introduced it, replayed as history thereafter | image re-injected every turn regardless of position |
| Provider fit | exactly how every API models images | images cannot ride `system`; would need a synthetic leading user message |
| Cache | append-only, measured cache-friendly | removal/addition rewrites the prefix head — full re-prefill each change |
| `/image remove` | unstages (pre-send only) | takes it out of every future request |
| Precedent | every mainstream chat UI | `/file` |

**Recommendation: A.** It matches both the provider semantics and the user's
mental model from other assistants; B additionally burns the prefix cache on
every mutation. The commands still mirror `/file` in surface (`attach`/
`remove`/`list`, `#N` addressing) — what differs is *when* the image binds.
Corollary (sub-decision): once sent, an image is part of history and `/image
remove` does not reach it — the refusal message states that explicitly.

### F2 — where do the bytes live?

| | A. Base64 inside the chat JSON | B. Blob dir (`media/<chat-id>/…`) + metadata in JSON |
|---|---|---|
| Self-containment | preserved — chat file carries everything (backup, restore, export untouched) | broken — backup `TOP_DIRS`, restore's stale-file sync, and delete-cascade all need the new dir |
| Request build | no I/O (the `/file` doctrine holds) | disk read per request, or an in-memory cache with invalidation |
| Chat JSON weight | +~1.37× image bytes per image; rewritten on **every** save (each message) | unchanged |
| Precedent | `Attachment.text` already puts hundreds-of-KB snapshots there | none |

**Recommendation: A for v1**, made safe by F3's caps: with downscale-on-attach
a typical image is 100–400 KB (≤550 KB as base64), and a per-message count cap
bounds the worst case at a few MB — the same order as the text snapshots the
chat file already accepts. The escape hatch is additive (a later
`data` → `blob_ref` field change), and B's real costs are the three storage
subsystems it touches, each a place to lose user data. Revisit if measurement
shows chat-save latency pain.

### F3 — image processing: which dependency, if any?

| | A. `image` crate (trimmed: jpeg/png/webp/gif/bmp) | B. `imagesize` (header-only dims) | C. nothing |
|---|---|---|---|
| Dimensions for chip + estimate | ✔ | ✔ (tiny, zero-dep) | ✗ |
| Downscale >1568 px long edge | ✔ | ✗ | ✗ |
| Normalize webp/gif/bmp → png/jpeg | ✔ | ✗ | ✗ |
| Cost | a real dependency tree; binary +~1–2 MB (the vendored-grammars lesson says measure it) | ~nothing | ~nothing |

**Recommendation: A.** Two provider facts decide it: xAI takes **only**
jpg/png, so without re-encoding a webp attach must be refused on exactly one
provider (a support-matrix wart users hit at send time); and un-downscaled
photos (a phone shot is 3–12 MB) would ride **every turn** of the conversation
— the clouds resize server-side but bill and upload the original each time,
and F2-A's JSON weight depends on the cap. Downscale once at attach
(long edge → 1568 px, the Anthropic standard tier, comfortably above the
measured ~256-token budgets everywhere else), keep png for png (screenshots:
text fidelity), jpeg q85 for photographic formats, refuse HEIC by name
(decoder patent-encumbered; the message names the formats that work). B is the
fallback if the dependency measurement disappoints.

### F4 — capability handling *(recommendation in §4.5)*

Probe `/props` for llama.cpp modes, static `Yes` for clouds, `Unknown` →
optimistic with clear send-time errors. Rejected alternative: per-model
allowlists (stale-prone; the project has been burned by name-based detection).

### F5 — do images get a text label the model can cite?

Anthropic explicitly recommends `Image 1:`-style labels when several images are
present. Emitting `Image #N — "name":` as a text part before each image gives
the model a stable handle the *user* also sees in `/image list` — and it is the
only way a model can say "in `diagram.png`…" naturally. Cost: a few tokens and
a prompt-content change (axis A localization, like every scaffold string).
**Recommendation: on** — one key in `prompt.images.*`, both locales.

### F6 — what the feed shows for a sent image

**Recommendation for v1: a chip line** in the message bubble —
`▣ photo.jpg — 1.2 MB, 1568×882, ~1.5k tok` (WGL4-safe glyph, one column).
Actual pixel rendering (kitty/sixel/iTerm2 protocols, halfblock fallback) is a
separate later track: `ratatui-image` exists but its compatibility with our
pinned ratatui 0.30 crate set (ADR 0001) is unverified, Windows Terminal only
does sixel and conhost nothing, and the project's own-renderer precedent
(ADR 0003) suggests a ~50-line halfblock renderer over F3-A's decoded pixels
if we ever want an inline preview. Not this track.

### F7 — limits and defaults (`config.images`)

`max_count` per message (default **8**), `max_bytes` pre-encode hard cap
(default **10 MB** — the strictest provider, Anthropic), `downscale_px`
(default **1568**, `0` = keep originals). All three in a settings "Images"
group next to the attachments budgets. Over-cap attach refuses with the
measured numbers in the message. **Recommendation: as stated.**

### Deferred (recorded, not for this track's v1)

- **Clipboard paste** (`Ctrl+V` an image): `arboard` is already a dependency
  with `default-features = false`; the `image-data` feature unlocks
  `get_image()`. Natural stage 3 — the Windows paste pipeline (lessons §6) is
  its own minefield and deserves a dedicated live check.
- **MCP/tool-produced images**: `shared/mcp.rs:821` already parses the wire
  shape it currently drops; a `ChatEffect::AddImage` would let a screenshot
  tool feed the model. Needs the same capability gate.
- **Attach by URL**: llama.cpp/Anthropic/OpenAI accept remote URLs natively,
  Gemini does not — client-side download would make it uniform but drags in
  SSRF concerns `fetch_url` already had to solve. Later.
- **Anthropic Files API / OpenAI `file_id`** upload-once referencing: a payload
  optimization, not a capability — only worth it if long image-heavy cloud
  chats show real latency pain.

## 6. Scope estimate

| Stage | Contents | Size |
|---|---|---|
| **1. Local MVP** (one PR) | `MessageImage` + staging + `/image` slice + `WireContent` widening (llama.cpp+Grok in one) + `--mmproj`/`MINDFORK_MMPROJ` + capability probe + chip/notes/help/i18n + unit tests + live smoke vs the reference stack | ~2–3 days incl. live run |
| **2. Clouds** (one PR) | `AntBlock::Image`, Gemini `inline_data`, Responses `input_image` + one `#[ignore]` smoke per provider | ~1 day incl. live runs |
| **3. Optional follow-ups** | clipboard paste; MCP images; URL attach; preview rendering | separate decisions |

Test plan highlights: a serialization test pinning that a **no-image request is
byte-identical** to today's (the prefix-cache and don't-break-Gemma guarantee);
the `*_are_localized_for_all_langs` gates for the new parser and orchestrator
errors; wire tests per backend asserting the exact part JSON (that is where
request-shape tests live — the `MockBackend` ignores requests by design); an
orchestrator test that staged images land on the sent message and replay into
the next request; live smokes = P1/P3 as `#[ignore]` tests (attach → correct
color word; second turn still sees it), one per configured provider key.

## 7. Live-run results (2026-08-12)

Local (`gemma-4-31B_q4_0-it` + mmproj, b10322, `-c 16384`):

| Check | Result |
|---|---|
| `/props` modalities | `vision: true, video: true, audio: false` |
| single / two-image / composition questions | correct (P1, P2, P4) |
| image replayed from history (turn 2) | correct (P3) |
| 3000×2000, 1.5 MB PNG | accepted, resized server-side |
| image token cost in `usage.prompt_tokens` | yes — +51 (128²), ~+258 (≥1 MP) |
| prefix cache across turns with images | `cache_n=73/78`; appended turn prefills 23 only |
| `reasoning_content` on vision turns | unchanged |

Clouds (one 128×128 PNG each, real keys): OpenAI `gpt-5-nano` (Responses,
`input_image`) ✔ · Anthropic `claude-haiku-4-5` (`image` block) ✔ · Gemini
`gemini-2.5-flash` (`inline_data`) ✔ · xAI `grok-4` (Chat Completions
`image_url`) ✔ — all four answered "Red"; token counts in §2.2.

## 8. Open questions

- **Does chat-save latency with F2-A inline bytes stay invisible?** Expected
  yes under the caps (≤ a few MB per image-bearing chat); measure during stage
  1 and revisit F2-B if not.
- **`--image-min-tokens` / `--image-max-tokens`**: defaults were right on the
  reference stack; not exposed. A power user can ask once real need appears.
- **Visual prompt injection** (instructions embedded in image pixels): the
  attachments' fence trick has no image analog. Mitigation is the existing
  one — tool confirmation gates and the DATA framing of surrounding text;
  worth one line in spec §13, not a blocker.
- **Old binaries and image-bearing chats**: an older build opening such a chat
  ignores the unknown field (additive, ADR 0006 F12) — and would drop it if it
  ever rewrote that chat file. The standard additive-field caveat, same as
  every post-M0 field; no migration needed, worth one line in the CHANGELOG's
  Data category when the field ships.
