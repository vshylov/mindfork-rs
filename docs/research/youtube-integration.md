# Research: YouTube integration — telling the agent what a video says and shows

> **Genre:** technology research, pre-decision (AGENTS.md §1). Nothing is
> implemented. §8 lists the decision points that need the user's answer before
> any code is written.
>
> **Task (user's framing, 2026-08-01):** *"research the possibility of
> integrating the program with YouTube, so the AI assistant can get a
> description of what is being talked about or shown in a video."*
>
> Note the wording: **talked about *or shown***. Those are two different
> capabilities with two different price tags, and only one provider currently
> answers the second one. That distinction turned out to be the whole story —
> see §3.

Everything load-bearing here was **measured on 2026-08-01** from this machine
(a residential IP, Windows) against the live services, not taken from
documentation. Where a claim is unmeasured it says so.

---

## 1. What happens today

Nothing useful, and this was verified rather than assumed.

`fetch_url` (spec §9.3) will happily accept a YouTube link — it starts with
`https://` — fetch the watch page (1.27 MB of HTML, HTTP 200) and hand it to
`web::extract_readable`. That extractor collects `<p>`/`<li>` fragments of ≥40
characters. On the watch page there are **zero** of either (measured: `0`
paragraphs, `0` list items — the page is a JavaScript shell). So `body_to_text`
returns `None` and the tool answers `tool.fetch_url.err.no_text` — "failed to
extract readable text".

So today the agent cannot say anything at all about a YouTube link, not even
its title.

**What already exists that is relevant:**

| Piece | Why it matters here |
|---|---|
| `features/tools/fetch.rs` | The exact pattern to copy: own `reqwest` client → extract → *summarize through a single-turn model call* → return the summary, with `focus` narrowing it. Also the graceful-degradation shape (summarization fails → return the raw text). |
| `shared/tts/gemini.rs` (ADR 0009) | **The precedent for this whole design.** A Gemini `generateContent` call that lives *outside* `EngineBackend`, in its own `shared/` module, with its own config section, reusing the stored provider key. TTS did exactly what a video tool needs to do. |
| `shared/secrets.rs` + ADR 0008 | Keys are **provider-centric**, not slot-centric: `stored_key(CloudProvider::Gemini)` already returns the user's Gemini key whatever the chat engine is set to. A video tool needs no new key plumbing. |
| MCP host (spec §9.6, ADR 0007) | The honest "do nothing" baseline — see §7. |
| `config.attachments` / `entities/attachment.rs` | Precedent for "large external text, budgeted in estimated tokens, inline vs by reference". |

---

## 2. The capability ladder

Three genuinely different things get called "YouTube integration". They are
worth separating because they fail independently:

1. **Metadata** — title, channel, duration, view count, keywords, and the
   author's own description. Answers *"what does this video claim to be
   about"*. Free, no key, works with a local model.
2. **Transcript** — the words that are said. Answers *"what is talked about"*.
3. **Video understanding** — frames plus audio. Answers *"what is **shown**"*,
   which the user asked for explicitly and which no transcript can give
   (a silent demo, a chart on screen, a face, a UI walkthrough).

---

## 3. Measured: which paths actually work (2026-08-01)

| Path | Result | Detail |
|---|---|---|
| oEmbed `youtube.com/oembed?url=…` | ✅ **200** | title, channel, channel URL, thumbnail. No key, no scraping. |
| Watch-page HTML → `ytInitialPlayerResponse` | ✅ **200** | title, author, `lengthSeconds`, `viewCount`, `keywords`, and **`shortDescription` (2375 chars on the probe video)** — plus the caption **track list** (6 tracks: manual `en`, ASR `en`, `de-DE`, `ja`, `pt-BR`, `es-419`). |
| `timedtext` signed URL taken from that same page | ❌ **HTTP 200, body 0 bytes** | Tried bare, `&c=WEB`, `&fmt=srv3`, `&fmt=vtt`, `&fmt=json3`, with browser `User-Agent` + `Referer`. Always empty. This is YouTube's **PoToken** gate (§3.1). |
| InnerTube `POST /youtubei/v1/player`, `WEB`/`MWEB` client | ❌ | `playabilityStatus: UNPLAYABLE`, `captionTracks: 0` (title still returned). |
| Same, `ANDROID` / `IOS` client | ❌ | `400 FAILED_PRECONDITION` ("Precondition check failed") — the client-identity shortcut is closed. |
| Same, `TVHTML5_SIMPLY_EMBEDDED_PLAYER` | ❌ | `playabilityStatus: ERROR`. |
| InnerTube `POST /youtubei/v1/get_transcript` | ❌ | `400 Bad Request`. |
| **YouTube Data API v3 `captions.download`** | ❌ by design | Requires an **OAuth token from the video's owner**. Not usable for third-party videos, at any price. An API key gets you metadata only. |
| **Gemini `generateContent` + `file_data.file_uri` = YouTube URL** | ✅ **works, measured end to end** | §3.2. |

### 3.1 The transcript problem: PoToken

The free caption path is not "fragile", it is **currently closed**. The watch
page still hands out a fully signed `timedtext` URL, and that URL now returns
`200` with an **empty body** unless the request carries a `pot` (proof-of-origin
token) produced by YouTube's BotGuard. This is the 2025–2026 addition to
YouTube's bot-detection stack; `youtube-transcript-api` added a dedicated
`PoTokenRequired` exception for exactly this response.

Two consequences for us:

- **This is not a datacenter-IP problem.** The measurement above is from a
  residential IP, the sympathetic case. Cloud IPs are blocked *harder* (that is
  a separate, well-documented layer), but a home user is already affected.
- **Any Rust crate that scrapes `timedtext` inherits this.** The candidates
  (`yt-transcript-rs`, `ytranscript`, `ytt`, `youtube-transcript`) all target the
  InnerTube/`timedtext` surface — the surface that just returned zero bytes. A
  dependency does not change the measurement; it only moves where the failure is
  printed. (Their maintenance status was not audited, because the transport they
  wrap is the blocker.)

Working transcript paths that remain, all with a cost:

- **A paid transcript API** (Supadata ≈ $1.57–5.67 per 1000 transcripts;
  TranscriptAPI, Apify ≈ $0.025 each). They run the proxy/token infrastructure
  for you. New vendor, new signup, new key.
- **`yt-dlp` as a sidecar** plus a PO-token provider plugin. Heavy (ADR 0005
  precedent exists for a sidecar, but that one bought a whole Python sandbox);
  a moving target by nature — its job is to track YouTube's countermeasures.
- **Rotating residential proxies.** Cost, plus a second service to maintain.
- **Gemini** — which transcribes as a side effect of watching (§3.2). The
  probe's answer included the sung line with its timestamp.

### 3.2 Gemini native video understanding — measured live

The project already speaks native `generateContent` (`shared/api/gemini/`), and
that endpoint accepts a YouTube URL directly:

```json
{"contents":[{"role":"user","parts":[
  {"text":"…"},
  {"file_data": {"file_uri": "https://www.youtube.com/watch?v=VIDEO_ID"},
   "video_metadata": {"start_offset":"0s","end_offset":"20s","fps":1}}
]}],
 "generationConfig":{"mediaResolution":"MEDIA_RESOLUTION_LOW"}}
```

**Probe 1** — a 20-second clip, `MEDIA_RESOLUTION_LOW`, `gemini-2.5-flash`,
plain API key, `v1beta:generateContent` (the exact endpoint family the project
already uses):

- HTTP 200 in **3.4 s**;
- prompt **2098 tokens** — `VIDEO 1420`, `AUDIO 640`, `TEXT 38`;
- the answer described **both** halves of the user's question: a timestamped
  scene-by-scene account of what is on screen (clothing, setting, camera cuts,
  a second person entering at 00:09) *and* the words that begin at 00:18.

**Probe 2** — the same video in full (213 s), no clipping:

- HTTP 200 in **8.0 s**;
- prompt **22 050 tokens** — `VIDEO 15 194`, `AUDIO 6819`;
- a summary, a timestamped visual outline, and a transcript (truncated only by
  our own `maxOutputTokens: 1500`).

**Model coverage** (measured, same request against each): `gemini-2.5-flash` ✅,
`gemini-3.1-flash-lite` ✅, `gemini-3.6-flash` ✅. `gemini-2.5-flash-lite` returns
`404 … no longer available to new users` — a reminder that a *default model name*
in config is a thing that rots.

**URL forms** (measured): `watch?v=…` ✅, `youtu.be/…` ✅, `youtube.com/shorts/…`
✅ — all accepted verbatim, so no URL normalization is needed. An invalid video
id returns a flat `400 INVALID_ARGUMENT` with no explanation, so a useful error
message has to be ours.

**Documented limits** (not measured): public videos only — no private or
unlisted; up to 10 videos per request from Gemini 2.5 on; free tier capped at
**8 hours of YouTube per day**, paid tier uncapped; a 1M-context model handles
~1 h at default resolution or ~3 h at low.

### 3.3 The cost model (measured, then extrapolated)

Both probes agree on the rate: **audio 32 tok/s** exactly, **video ≈ 71 tok/s**
at `MEDIA_RESOLUTION_LOW` and 1 fps (66 tok/frame per the docs) → **≈ 103 tok/s
total**. Default resolution is ~3× the video part → ≈ 330 tok/s.

| Video length | LOW (≈103 tok/s) | default (≈330 tok/s) |
|---|---|---|
| 3.5 min | 22 k *(measured)* | ~70 k |
| 10 min | ~62 k | ~198 k |
| 30 min | ~186 k | ~594 k |
| 60 min | ~372 k | ~1.19 M — **over a 1M window** |

At a Flash-class list price of ~$0.30/M input, a 10-minute video at LOW is
**~$0.02** and an hour is **~$0.11**. Cheap in money; **not** cheap in context —
which is the real constraint, because the project's primary target is a local
model with an 8k window. That single fact decides §8 R5.

### 3.4 The other providers

- **OpenAI Responses API** — no video input at all (images, PDFs, documents,
  spreadsheets, code files). An open feature request notes the gap versus
  Gemini. No YouTube URL ingestion of any kind.
- **Anthropic** — "All current Claude models support text and image input" per
  the official model overview. No video. (A blog claiming a "Claude real-time
  video API" turned up in search and is wrong — checked against the primary
  source, which is why it is named here rather than cited.)
- **Local `llama-server`** — no video, and no path to one: reaching frames would
  require downloading the video, which runs into the same signature/PoToken
  machinery as captions do.

**So: "what is shown" is a Gemini-only capability today.** This is not a
preference between vendors, it is the entire available field.

---

## 4. Architecture: where would this live?

Two shapes, and the project has already chosen between them once.

### (a) A tool with its own Gemini client — *recommended*

A new tool (`youtube_watch`) in `features/tools/`, holding a small
video-capable Gemini client under `shared/`, called out-of-band exactly the way
`fetch_url` calls the model to summarize and the way **TTS** calls Gemini
(ADR 0009). Consequences:

- **works whatever the chat engine is** — a user on a local `llama-server` or on
  Claude still gets video understanding, because the *tool* talks to Gemini and
  returns text into the conversation;
- the provider-agnostic `EngineBackend`/`ChatRequest` contract is untouched;
- the key already exists (`CloudProvider::Gemini`, ADR 0008), so config grows by
  a model name and a couple of caps, not a key field;
- one new capability, one gate, one settings group.

### (b) Extend the engine contract with a media part

Add a media part to `ApiMessage`/`ChatRequest` so the chat engine itself sees
the video. Rejected for this task:

- it puts a **one-provider** capability into the **provider-agnostic** contract
  (ADR 0004's boundary), and every other backend has to decide what to do with a
  part it cannot send;
- it only works when the chat engine *is* Gemini — strictly less useful than
  (a) for exactly the users most likely to want it (local-model users);
- it is really the *multimodality (images)* roadmap item wearing a different hat.
  When that track happens, it should be designed as image input first, with
  video as a follow-on — and at that point a `youtube_watch` tool still makes
  sense as the thing that turns a URL into something ingestible.

---

## 5. What the tool would return

Two options, and they differ by an order of magnitude in context cost:

- **The answer, not the material** (the `fetch_url` shape): the tool asks Gemini
  the user's question about the video and returns Gemini's text. A 10-minute
  video costs ~62 k tokens **at Google**, and perhaps 400 tokens **in the
  conversation**. A local 8k-context model can use this.
- **The raw transcript**: 10 minutes of speech is roughly 1.5 k words ≈ 2 k
  tokens — survivable; a 60-minute lecture is not, and a *timestamped visual*
  outline is far larger. This has to be opt-in and budgeted.

The `fetch_url` precedent already solved the same problem the same way (fetch →
summarize → return the summary; raw text only via `summarize=false`), and the
attachment work already solved "text too big for the prompt" (inline vs. by
reference, budgeted in estimated tokens, `attachment_read` paging). If a full
transcript is ever wanted, the honest shape is to **land it as a chat
attachment** rather than as a tool result — that machinery exists and is already
paged and searchable.

---

## 6. Privacy and correctness notes

- Sending a YouTube URL to Google is not a new disclosure of the user's data —
  Google owns YouTube. The *prompt* around it (the user's question) does go to
  Google, like any cloud call, and the tool sits behind the existing
  `web_enabled` gate.
- **Free tier:** Google states that on the free tier prompts and responses may
  be used to improve its products. Worth one line in the settings hint, the way
  the backup-password and API-key hints state their limits.
- A description produced from 1 fps sampling is **a sample, not a viewing**: the
  model sees one frame per second at reduced resolution. It will miss fast text
  on screen. Overselling this in the tool description would train the model to
  assert more than it saw — the same failure mode the self-model work calls
  "accuracy over agreeableness".

---

## 7. The "do nothing" baseline, stated honestly

For the **transcript** half, the user can already have this today with zero
code: the MCP host (spec §9.6) will run any of the several existing
`mcp-youtube-transcript` servers, added to `settings.json`. Those servers hit
the same PoToken-gated surface measured in §3.1 — so they inherit the same
failure — but the ones that proxy a paid API work.

For the **"what is shown"** half there is no baseline: no MCP server can give it
without a video-capable model behind it, and the model the project would use for
that is the one in §3.2.

---

## 8. Decision points (need the user's answer before implementation)

**R1 — Which capability ships first?**
- **(a) Gemini video understanding + free metadata as the fallback** —
  *recommended*. It answers the question as asked ("said **or shown**"), it is
  the only measured-working path, and metadata keeps it useful with no key.
- (b) Transcript first, through a paid third-party API. New vendor, new signup,
  and it answers only half the question.
- (c) Metadata only. Cheap, honest, and much less than what was asked for.

**R2 — Where does the Gemini call live?**
- **(a) A tool with its own client under `shared/`, ADR 0009's shape** —
  *recommended* (§4a): works whatever the chat engine is.
- (b) Extend `ChatRequest` with a media part (§4b) — defer to the multimodality
  track.

**R3 — What does the tool return by default?**
- **(a) Gemini's answer to a `focus` question (the `fetch_url` shape), with a
  timestamped outline** — *recommended*: keeps the conversation cheap, which is
  what a local model needs.
- (b) A full transcript by default — blows an 8k context on a medium video.
- (c) (a) by default, plus `transcript: true` to also request the spoken text,
  and — if it is large — land it as a **chat attachment** rather than a tool
  result. *Recommended as the stage-2 shape*, once (a) is proven.

**R4 — Cost and length control.**
- **(a) `MEDIA_RESOLUTION_LOW` by default + a configurable duration cap
  (suggest 30 min ≈ 186 k tokens) + `start`/`end` arguments so the model can
  watch a segment** — *recommended*. Refuse above the cap with a message that
  names the segment arguments, rather than silently truncating.
- (b) Auto-clip to the first N minutes — silently answers about the wrong part
  of the video.
- (c) No cap — a 3-hour conference talk is a surprise bill and a context
  overflow.

**R5 — Availability and gating.**
- **(a) Gate on `web_enabled` (like `fetch_url`); if no Gemini key is
  configured, the tool still exists and returns a clear "not configured, here is
  the metadata instead"** — *recommended*, mirroring the embedder's graceful
  degradation (ADR 0002).
- (b) Hide the tool entirely without a key — the model then cannot explain to
  the user why it cannot watch the video.

**R6 — Should `fetch_url` stop being a dead end for YouTube links?**
- **(a) Yes: detect a YouTube URL and return the metadata (title/channel/
  duration/description) plus a pointer to `youtube_watch`** — *recommended*.
  Measured today it returns "failed to extract readable text", which is a real
  dead end the model cannot reason its way out of.
- (b) Leave it alone.

**R7 — A separate transcript provider (paid API / `yt-dlp` sidecar)?**
- **(a) Not now** — *recommended*. Gemini transcribes as part of watching
  (measured), so a captions vendor would add a signup and a moving target for
  something already covered. Revisit if a user needs transcripts *without* any
  cloud key, which is the one case (a) does not serve.
- (b) Add one now, behind a config section.

**R8 — Caching.**
- **(a) None in stage 1** — *recommended*. A follow-up question in the same chat
  is already free (the result is in the history); only a *different chat* re-pays.
  Note it as groundwork. `cache.db` is deliberately **not** the home for it: that
  file is disposable by design, and a Gemini summary is expensive to recompute.
- (b) A `youtube_summaries` table in `data.db` keyed by video id + focus.

**R9 — Naming and scope of the tool id.**
- **(a) `youtube_watch`** — *recommended*: says what it does, and leaves room for
  a future `video_watch` that takes any URL Gemini accepts.
- (b) `video_describe` / `watch_video` — provider-neutral from the start, but the
  first version only handles YouTube URLs, so the name would over-promise.

---

## 9. Sketch of the shape (for reference, not a commitment)

Assuming R1(a) / R2(a) / R3(a) / R4(a):

```
shared/video/gemini.rs      GeminiVideoClient: POST {base}/models/{model}:generateContent
                            with file_data.file_uri + video_metadata (start/end/fps)
                            + generationConfig.mediaResolution. Own reqwest client,
                            key from secrets::stored_key(CloudProvider::Gemini) with
                            the env fallback — same resolution TTS already does.

features/tools/youtube.rs   youtube_watch { url, focus?, start?, end?, transcript? }
                            group: ExternalWorld, gate: Web, danger: false
                            (read-only, no side effects outside the turn).
                            Not configured / over the cap / 400 → a text result the
                            model can act on, never a panic (the tool contract).

features/youtube_meta.rs    Free metadata: oEmbed + watch-page ytInitialPlayerResponse
                            (title/channel/duration/description/keywords). Pure parse,
                            testable on a saved fixture. Used by the fallback and by
                            fetch_url (R6).

config.video: VideoSettings model, media_resolution, max_minutes
                            (all #[serde(default)] → no schema bump, ADR 0006 F12)
                            → a group in the settings "Tools" section.
```

Localization: description/parameters/results through the bundles on **axis A**
(`ctx.loc` — the model reads them); the settings field and its hint on **axis
B**. Both gates in CI already cover new keys.

Live smoke (`#[ignore]`, `MINDFORK_GEMINI_KEY`): a short public video, assert the
answer mentions something only visible on screen — that is the half a transcript
could not have produced, so it is the assertion that proves the feature.

---

## 10. Groundwork noted along the way

- **Multimodality (images)** — the roadmap's own item; §4b belongs to it.
- **Voice input (STT)** — a local Whisper-class sidecar would, in principle, also
  transcribe a downloaded video; both are blocked on the same "get the media
  bytes" problem, so if that is ever solved the two tracks share it.
- **Transcript without a cloud key** — the one user story left unserved by R7(a).
- **Cross-chat caching of an expensive video read** — R8(b).
- **Default-model rot** — `gemini-2.5-flash-lite` is already 404 for new users;
  whatever default lands in config needs a "the model went away" path, and the
  settings hint should say the field is editable.

---

## 11. Sources

Measured probes (2026-08-01, this machine) are cited inline in §3 and are not
links. External references:

- [Gemini API — video understanding](https://ai.google.dev/gemini-api/docs/generate-content/video-understanding)
  (YouTube URL shape, `videoMetadata`, token rates, the 8 h/day free-tier limit).
- [Gemini API — video understanding (Interactions API)](https://ai.google.dev/gemini-api/docs/interactions/video-understanding)
  (public-videos-only, 10 videos per request from 2.5 on).
- [YouTube Data API — captions implementation guide](https://developers.google.com/youtube/v3/guides/implementation/captions)
  (`captions.download` requires the owner's OAuth).
- [`youtube-transcript-api` — cloud IP blocking / PoToken](https://github.com/jdepoix/youtube-transcript-api/issues/593)
  and its [releases](https://github.com/jdepoix/youtube-transcript-api/releases)
  (`PoTokenRequired`).
- [Claude — models overview](https://platform.claude.com/docs/en/about-claude/models/overview)
  ("text and image input", no video).
- [OpenAI — feature request: native video input in the Responses API](https://github.com/openai/openai-node/issues/1778).
- Transcript-API vendors and pricing:
  [comparison](https://transcriptapi.com/blog/best-youtube-transcript-apis-compared),
  [Apify video transcript actor](https://apify.com/amrameng/video-transcript-api/input-schema).
- Rust crates on the (currently gated) scraping surface:
  [`yt-transcript-rs`](https://crates.io/crates/yt-transcript-rs),
  [`ytranscript`](https://crates.io/crates/ytranscript),
  [`ytt`](https://crates.io/crates/ytt).
- Existing MCP transcript servers (the §7 baseline):
  [kimtaeyoon83/mcp-server-youtube-transcript](https://github.com/kimtaeyoon83/mcp-server-youtube-transcript),
  [sinco-lab/mcp-youtube-transcript](https://glama.ai/mcp/servers/sinco-lab/mcp-youtube-transcript).
</content>
