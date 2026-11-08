# Research: a name for a pasted image, and what the paste still misses

**Status:** research and a live probe done (2026-11-08). **User's decision,
2026-11-08: every fork in §5 goes with its recommendation** — A1 (a) a copied
file staged as an attach, A2 (a) the refusal over SSH names the distance,
F1 (a) the chat's own engine, F2 (a) named at paste, F3 (a) English names,
F4 (a) a send waits for a pending name up to 10 s, F5 (a)
`clipboard/<slug>.png`, F6 (a) unique across the chat with the switch on or
off, F7 (a) `/image paste <name>`, F8 (a) pasted images only, F9 (a) a failure
is quiet. **The track is complete** (2026-11-08), in the two pull requests of §6:
stage 1, the paste's gaps (A1, A2), and stage 2, the names — docs/journal/ui-input.md,
"pasting a copied picture file, and the paste over SSH" and "a name for a pasted
image". What it left open is in §7 and on the roadmap.

The ask (the owner, 2026-11-08): look into pasting images into the chat from the
clipboard; where possible, give a pasted image a name that says what it shows —
something like `clipboard/summer-mountain.png`. The naming must be a setting and
**off by default**, since it costs either time or money. And check how far local
models carry it: Gemma 4 31B, Qwen 3.6 27B and Gemma 4 E2B, each with its
projector (`gemma-4-31B-it-mmproj.gguf`, `mmproj-Qwen3.6-27B-Q8_0.gguf`,
`mmproj-google_gemma-4-E2B-it-f16.gguf`).

Related: spec §9.10 (images in a message), §11.2 (the automatic title, the
template this follows), [multimodal-images.md](multimodal-images.md) (the image
track), [image-url-attach.md](image-url-attach.md) F4 (how a URL's image is
named), [silent-tasks-budget.md](silent-tasks-budget.md) and
[silent-preemption.md](silent-preemption.md) (the lane a background request
runs in), [docs/journal/ui-input.md](../journal/ui-input.md) "pasting an image
from the clipboard" (how the paste was built).

## 1. Summary

- **Pasting already works** — `/image paste` in every terminal, `Ctrl+V` where the
  terminal forwards the key (spec §9.10, since 2026-08-13). What it lacks is a name:
  every pasted image is `clipboard.png`.
- **The name is worth more than looks.** A sent image is an item of the chat's
  list — `/file list`, the `files` of `python_exec`, `/w/in` — and the uniqueness
  of `clipboard.png` is checked only among the images *waiting* to be sent. A chat
  with three pasted screenshots has three items called `clipboard.png`, a name the
  shared-name rule refuses; the model reads `Image #1 — "clipboard.png"` on each.
  A descriptive name makes a pasted image addressable by name (§2.2).
- **Every model asked names well** (§3.2): the three local pairs and three cheap
  clouds gave a well-formed name in **240 of 240** answers (192 local, 48 cloud),
  each on the image's subject. Locally on an RTX 4090 a name takes **0.17 s** (E2B) to **~1 s**
  (31B, Qwen 3.6) at the median, 1.6 s at most; on a cheap cloud 1.3–1.6 s and
  **$0.00013** an image.
- **Muting works on images.** The title's sampling (thinking off, effort `none`,
  budget 0) left **0** characters of thoughts on all three local models.
- **Two gaps in the paste itself** turned up (§3.1): a file copied in a file
  manager is refused as "no image on the clipboard", though the clipboard library
  hands over its path on every platform; and over SSH — this project's main
  audience on rented pods — the refusal blames the clipboard rather than the
  distance.
- **The design** (§4) follows the automatic title: one muted request to the
  chat's own engine, in the silent lane, failures quiet. The image is staged at
  once under `clipboard.png` and renamed when the name lands; a send while the
  name is pending waits for it, up to a ceiling.

## 2. What the code does today

Read first, because it moved the design (lessons §3).

### 2.1 The paste

- `runtime` reads the clipboard (`arboard` 3.7.0, feature `image-data`):
  `clipboard_image` → raw RGBA, normalized by arboard from `CF_DIB` / `image/png` /
  `NSImage` (`app/runtime/clipboard.rs`). The image is checked **first**, then
  `Ctrl+V` falls back to text; a typed `/image paste` with no image is refused
  with `ui.err.image_no_clipboard` (`app/runtime/input.rs`).
- The orchestrator gets pixels (`AppCommand::ImagePaste`), picks the first free
  `clipboard*.png` **among the chat's staged images**, and stages through the one
  path every image takes: the count cap, the vision probe, the encode on the
  blocking pool (`app/orchestrator/images.rs`, `stage_image`). A pasted image is
  always png and its source is `clipboard:<uuid>`, so two pastes never dedupe.
- The encode takes tens of milliseconds, so between the paste and the image being
  staged there is practically no window in which a send would leave without it.
  A name that takes a second must not widen that window (§4.1).

### 2.2 Where the name goes

`MessageImage::name` is a display string, and it reaches five places:

1. **The label the model reads** before the image —
   `Image #{n} — "{name}":` (`prompt.images.label`, in the profile language),
   on that turn and on every later one, since an image is replayed as history.
2. **`/image list`** and the status-bar chip of what is staged.
3. **The stored message** (`Message.images[].name` in the chat file). A string
   already — a longer one is no schema change.
4. **The chat's list of inputs** (`features/chat_inputs.rs`): `/file list`,
   `/file remove`, the pinned block that tells the model what it may name, and
   the `files` argument of `python_exec`. An image's `/w/in` name is
   `sanitize_name(name)` made unique; the **handle** is the name itself, and a
   name two items share resolves to nothing (docs/research/remove-by-shared-name.md).
5. **The feed's notes** for the staging and the removal.

So today, in a chat where the person pasted three screenshots over three
messages, the list holds three `clipboard.png`; `python_exec` given
`files: ["clipboard.png"]` is refused as ambiguous, and only `#N` reaches one.

### 2.3 The template: the automatic title

`app/orchestrator/title.rs` already does what naming needs, minus the image: a
one-shot request on the chat's engine (`backend_if_ready`), its own system
message, **muted** sampling (`thinking: false`, `reasoning_effort: none`,
`reasoning_budget: 0` — which the OpenAI-compatible wire also sends as
`chat_template_kwargs.enable_thinking: false`), `max_tokens` 2048 because some
GGUFs think anyway, a fallback that reads the answer out of the thoughts, a
60-second limit, the silent lane of the session budget (`Shape::Title`), and
failures that go to the log when nobody asked (`TitleOrigin::Auto`). The gateway
already asks again when a model refuses to stop thinking
(`Reasoning is mandatory…`, `shared/api/openai/client.rs`) — measured below on
Gemini 3.8 Flash.

## 3. Measured (2026-11-08)

### 3.1 The paste itself

On Windows 11, with a throwaway program on the same `arboard` 3.7.0 and the
same feature set (the owner's clipboard saved first and put back after):

| What is on the clipboard | `get_image` | `get().file_list()` | today's `/image paste` |
|---|---|---|---|
| a bitmap (a screenshot, a browser's *Copy image*) | 760×380 RGBA | — | staged |
| a file copied in Explorer (`CF_HDROP`) | **error** | `["…\photo-sunset-lake.jpg"]` | **"there is no image on the clipboard"** |

- **A copied file is refused, and the path was right there.** arboard 3.7 reads a
  file list on all three platforms — `CF_HDROP` on Windows, `text/uri-list` on X11
  and Wayland, `NSURL` on macOS (by its source; measured on Windows only). Copying
  a picture *file* and pressing paste is a natural thing to do; the refusal tells
  the person to copy an image, which is what they did.
- **Over SSH the clipboard is on the wrong machine.** On a pod reached by SSH the
  app's clipboard is the remote one — on a headless box `arboard::Clipboard::new`
  fails outright (no display) — while the screenshot sits in the clipboard of the
  machine the person types on. No terminal protocol carries an image the other
  way: OSC 52 is text only, and reading through it is off in most terminals
  ([osc52-clipboard.md](../history/osc52-clipboard.md)). The refusal says "copy
  one", which sends the person to do again what they already did. A message
  has to close the door (lessons §4): the session is remote, and the routes that
  work are `/image attach` after copying the file over, or a URL.
- **Wayland**, measured after the track (§3.3): the build left arboard's
  `wayland-data-control` feature off, which cost more than the session without
  XWayland it was recorded for.

### 3.2 Naming, live

**Images** — 16, the shapes a clipboard holds: three photographs and two abstract
renders (Windows' own wallpapers), a rendered portrait (a picture an earlier probe
drew), the app's chat and settings screens, a bar chart a `python_exec` call drew,
the site's card and the essay's chart, and five drawn for the probe — Rust code, a
CUDA out-of-memory traceback, an invoice table, a sign-in form, and a shopping list
written in Russian. Each was prepared as a paste is: the long edge cut to 1568,
encoded as png.

**The request** — the title's shape with the image as the one content part:

```
system: You name image files. Look at the image and reply with a short file name
        for it: two to four lowercase English words that say what the image shows,
        joined by hyphens. No file extension, no quotes, nothing else.
        Example: summer-mountain-lake
user:   [image] Name this image.
```

with the title's muted sampling and `temperature: 0.3`. A second pass asked, in
Russian, for Russian words. **Local:** llama.cpp b12191 (`e95426f16`) on the RTX
4090, `-ngl 99 -c 16384 -np 1 --jinja --mmproj …`, a server the probe started and
stopped by its own PID; three English passes and one Russian. **Cloud:** through
OpenRouter, one English pass. *Well-formed* means the raw answer, untouched, is
2–5 lowercase words joined by hyphens.

| Model (projector) | Well-formed | Thoughts | Median / max, first pass | Prompt tokens, a 1568×882 photo | Per image |
|---|---|---|---|---|---|
| Gemma 4 E2B Q8_0 (`f16`) | 64 / 64 | 0 | **0.17 s** / 0.85 s | 734 | — |
| Gemma 4 31B Q4_0 | 64 / 64 | 0 | **0.99 s** / 1.58 s | 554 | — |
| Qwen 3.6 27B Q4_K_M (`Q8_0`) | 64 / 64 | 0 | **1.00 s** / 1.62 s | 1597 | — |
| Claude Haiku 5.5 | 16 / 16 | 0 | 1.35 s / 3.35 s | 2049 | $0.000125 |
| GPT-6 Luna | 16 / 16 | 0 | 1.63 s / 3.08 s | 1892 | $0.000134 |
| Gemini 3.8 Flash (effort `low`) | 16 / 16 | 5 of 16 | 3.04 s / 14.44 s | 1150 | $0.00125 |

The names, first English pass:

| Image | Gemma 4 E2B | Gemma 4 31B | Qwen 3.6 27B | Claude Haiku 5.5 |
|---|---|---|---|---|
| photo: sunset over a lake, mountains | sunset-mountain-lake | sunset-mountain-lake | sunset-lake-reflection | sunset-lake-snowy-mountains |
| photo: a lake, the sun low | snowy-mountain-lake-sunset | snowy-mountain-lake | serene-lake-sunset | calm-lake-sunrise |
| photo: beach, blossom, lake side by side | cherry-blossoms-water-landscape | nature-collage-scenes | three-nature-scenes | spring-blossoms-lake-mountains |
| render: a woman reading by candlelight | woman-portrait-painting | woman-reading-books | woman-reading-book | woman-reading-night-library |
| abstract: a blue bloom | blue-fluid-waves | blue-abstract-waves | blue-abstract-flow | blue-abstract-swirl-waves |
| abstract: coloured ribbons | colorful-paper-swirl | colorful-abstract-waves | colorful-layered-ribbon | purple-swirling-ribbons |
| the app's chat screen | gpu-decision-diagram | llm-quantization-guide | gpu-memory-decision-diagram | gemma-gpu-quant-decision |
| the app's settings screen | model-server-configuration | ai-model-settings-menu | llama-cpp-settings | llama-server-settings |
| a bar chart of monthly sales | bar-chart-sales-data | total-sales-chart | monthly-sales-bar-chart | monthly-sales-bar-chart |
| Rust code (Fibonacci) | fn-fibonacci-function | rust-fibonacci-code | fibonacci-code-snippet | rust-fibonacci-code |
| a CUDA out-of-memory traceback | python-traceback-error | cuda-out-of-memory-error | cuda-out-of-memory-error | cuda-out-of-memory-error |
| an invoice table | invoice-client-date-amount | invoice-table-list | invoice-summary-table | invoice-summary-table |
| a sign-in form | sign-in-form | sign-in-form | sign-in-form | login-form-sign-in |
| a shopping list in Russian | grocery-list-shopping-items | shopping-list-note | weekend-shopping-list | weekend-shopping-list |
| the essay's chart | code-metrics-graph-dashboard | code-metrics-graph | code-ownership-metrics-chart | nobody-owned-code-metrics-chart |
| the site's card | mindfork-ai-chat-model | mindfork-ai-chat | mindfork-ai-chat | ai-chat-mindfork-banner |

What this settles:

- **The feature does not need a big model.** E2B is the weakest reader — it named
  the code by its first token (`fn-fibonacci-function`), the invoice by its column
  headers, the render a "painting" — yet never left the subject. The 31B and Qwen
  3.6 read text in screenshots about as well as the clouds (`cuda-out-of-memory-error`,
  `llama-cpp-settings`, `weekend-shopping-list` from Russian text).
- **The names vary in wording, not in subject.** At `temperature: 0.3` the same
  image got the same name in all three passes 7 (E2B), 10 (31B) and 9 (Qwen) times
  of 16; the rest differ like `woman-reading-book` / `woman-reading-books`. No name
  is promised to be stable, so this is enough.
- **The first pass is the honest latency.** Qwen's second and third passes ran at
  0.47 s: llama.cpp's prompt cache served the repeated image. A pasted image is new
  every time, so the table reports the first pass.
- **The cost locally is the slot's time, not memory**: the chat's own server, with
  its projector already loaded, is busy for about a second per paste (0.2 s on
  E2B). A naming request and a turn on a one-slot server run one after the other.
- **The cost in the clouds is small and scales with the chat model.** The cheap
  tier is about $0.00013 a paste — a thousand pastes for 13 cents. On a flagship
  chat model the same ~2 000 prompt tokens cost more: about $0.004 on Sonnet 5.5
  and $0.008 on Opus 5.5 (estimated from Haiku's token count at the catalogue's
  prices on 2026-11-08). Gemini 3.8 Flash cannot stop thinking — "Reasoning is
  mandatory for this endpoint" — and asked at `low` it names correctly at ten
  times Haiku's price and 3 s; one of its answers took 14 s.
- **Russian names work, a little worse.** All 48 Russian answers were well formed
  and on the subject (`zakat-nad-gornym-ozerom`, "sunset over a mountain lake",
  from the 31B), but E2B's grammar slips (`zimnee-zakat-ozero-gory`, an adjective
  agreeing with nothing) and Qwen transliterated the brand
  (`interfeys-programmy-maynfork`). English names were clean on every model.

### 3.3 The Wayland clipboard (2026-11-09, after the track)

Asked by the owner once the track had closed: the roadmap's last item, the clipboard
on a Wayland session with no XWayland. Measured in Docker on the Linux terminal stand
(`mf-notify-lx3` + `wl-clipboard`, XWayland and `xclip`): Arch Linux, headless sway
1.12 (wlroots, pixman), a picture put on the clipboard by `wl-copy` — what a Wayland
app does — or by `xclip` for an X app. Two builds of a checker on arboard 3.7.0 making
the paste's own calls (`get_image`, `get().file_list()`, `get_text`): the app as
shipped (X11 only), and with `wayland-data-control`.

| Session | As shipped | With `wayland-data-control` |
|---|---|---|
| Wayland, no XWayland: a picture | the clipboard does not open (no X server) | 760×380 RGBA |
| Wayland, no XWayland: a copied file (`text/uri-list`) | does not open | the path |
| Wayland, no XWayland: the app copying text | does not open — OSC 52 was the only copy | copied; readable while the app runs |
| sway **with** XWayland: a picture a Wayland app copied | **nothing** — the X selection stays empty | 760×380 RGBA |
| sway with XWayland: a picture an X app copied | 760×380 RGBA | 760×380 RGBA |

- **The gap was wider than the session it was recorded for.** wlroots syncs the
  Wayland selection into X only towards a focused X window; an app inside a terminal
  has none, so on sway or Hyprland a picture a browser or a screenshot tool copied
  never reached the shipped build even with XWayland running.
- **No new system library.** `wl-clipboard-rs` and `wayland-client` are pure Rust: the
  checker links the same `libc`/`libgcc_s` as before and runs in a bare `ubuntu:24.04`
  with no `libwayland`; opening fails at once (0.00 s) there and falls back to X11,
  as arboard does whenever the compositor offers no data-control protocol — the path
  every session took before. About 0.7 MB of binary in the checker.
- **Copying changes in one way.** Text the app copies is served by the app, so after
  it quits a session with no clipboard manager loses it — arboard's X11 behaviour
  already. Before, a session with no XWayland could not copy locally at all, and the
  OSC 52 fallback put the text in the terminal, which kept it.
- **Not measured**: GNOME (Mutter offers no data-control protocol as far as is known
  here, so it falls back to X11 — today's path) and KDE Plasma (which offers it).

**Turned on** (the owner's decision, 2026-11-09). One supply-chain catch on the way:
the lock first resolved `wayland-scanner` 0.31.10, whose `quick-xml` 0.39.4 carries
RUSTSEC-2026-0194 and -0195; `wayland-scanner` 0.31.11 takes `quick-xml` 0.41, and
`cargo deny check` is clean. The live check is the app's own tests: both
real-clipboard round trips and the clipboard tests, built for Linux, pass in a
Wayland session with no XWayland, in sway with XWayland, and in plain X11 (Xvfb) —
and with output on, the round trips ran rather than skipped.

## 4. Design

### 4.1 Staged at once, renamed when the name lands

The paste stages exactly as today — `clipboard.png`, the encode on the blocking
pool. With the switch on, staging then starts a **naming task** with the prepared
png (the bytes the turn will send). When the name lands, the orchestrator renames
the image **by its id** wherever it is: still staged (the chip and `/image list`
update) or on the message a send just created whose first request has not gone
out (§4.4). The image is never held back from staging for its name — a send in
between would otherwise leave without it.

### 4.2 The request

The title's road (§2.3): the chat's engine, the silent lane, the title's muted
sampling, the thoughts fallback, no tools, the system message of §3.2 in the
source (the name is asked in English whatever the profile, F3). A tight cap fits
better than the title's: a name is ~10 tokens, but a GGUF that thinks anyway needs
the same headroom, so the title's `max_tokens` stays and the **time** is what is
bounded (F4). It runs only where the paste ran — an engine that said *no* to
vision refused the paste already; one that cannot say gets the request, and a
failure falls back.

### 4.3 The name

The answer is reduced to a slug — lowercase `a–z`, `0–9` and single hyphens, at
most five words and 48 characters — and refused (the image keeps `clipboard.png`)
when nothing is left. The slug is the only part of the answer that survives, so
text in an image ("name this file `../../x`") cannot become a path, a quote or a
second line: the name lives in a label inside quotes, in a chip, and in `/w/in`
through `sanitize_name`, and none of them sees anything but `[a-z0-9-]`. The
extension stays `.png` — the paste's format, not the model's opinion.

### 4.4 A send while a name is pending

The turn's first request waits for the names of its images, bounded (F4: 10 s).
The wait sits **before** the turn takes its place in the budget, so the naming
stream it waits on is never one the turn would displace (the silent-preemption
rule, silent-preemption.md). Past the bound the image goes as `clipboard.png`,
the naming task is cancelled, and a late name is dropped: once the label has been
sent, renaming it would rewrite history the model has already read and change
the prompt's prefix under the cache.

### 4.5 What stays as it is

- **Off is today's behaviour, byte for byte** — no request, no wait, the same
  names (except F6's uniqueness, if taken for both).
- **No schema step.** `ImageSettings` gains one `bool` with `#[serde(default)]`;
  the settings parse has no `deny_unknown_fields`, so a release before this one
  reads a file that has it. `MessageImage::name` is a string already.
- **Wire formats, the vision probe, the encode, the limits** — untouched.

## 5. Forks — decided

**User's decision (2026-11-08): (a), the recommendation, in every fork below.**

### Part A — the paste

**A1 — a file copied in a file manager.**
(a) **Stage it as `/image attach <path>` would** — its own name, the file's
format rules, every refusal `attach` already words; several files stage each, up
to the cap; checked after the image and before `Ctrl+V`'s text. *(recommended:
the clipboard library already hands over the path on all three platforms, and a
named file needs no naming request at all.)*
(b) Leave it refused, and only reword the refusal.

**A2 — the refusal over SSH.**
(a) **When `SSH_CONNECTION` or `SSH_TTY` is set and nothing could be read, say
that the clipboard here is the remote machine's**, and name the routes that work —
`/image attach <path>` after copying the file over, or a URL. *(recommended:
lessons §4; this is the audience's usual setup.)*
(b) Keep today's wording.

### Part B — the name

**F1 — which model names.**
(a) **The chat's own engine**, as the title — the model that is about to look at
the image anyway, with its projector loaded. *(recommended)*
(b) A slot of its own (shared / managed / external / a cloud, like impersonation),
so a cloud chat can name locally or a local chat name on a cheap cloud. E2B shows
that a 5 GB model with a 1 GB projector would do (§3.2). Not wanted (§7).

**F2 — when the image goes out to be named.**
(a) **At paste.** The name is usually ready before the person finishes typing.
With a cloud chat engine the image reaches the provider before the message is
sent — one removed before sending has been seen anyway — and the setting's
description says so. *(recommended: the switch is off until turned on, and the
alternative taxes every send.)*
(b) At send — nothing leaves early, but every message with a pasted image waits
1–3 s more.

**F3 — the name's language.**
(a) **English, whatever the profile** — clean on every model, typeable on any
keyboard layout, and the shape of the owner's own example. *(recommended)*
(b) The profile's language (axis A) — measured workable, weaker on small models
(§3.2).

**F4 — a send while the name is pending.**
(a) **The turn waits for the name, at most 10 s**, then goes with
`clipboard.png` and drops the late name (§4.4). Measured, the wait is ≤ 1.6 s
locally and ≤ 3.4 s on the cheap clouds. *(recommended: paste-then-Enter is the
common gesture, and without the wait it would lose the name most often.)*
(b) No wait: the send takes whatever name the image has, and a pending name is
cancelled.

**F5 — the name's shape.**
(a) **`clipboard/<slug>.png`**, the owner's example: the origin stays visible in
the chip, the list and the label; `remove` and `files` also accept the part after
the slash; `/w/in` gets `<slug>.png` (`sanitize_name` keeps the last segment).
*(recommended)*
(b) `<slug>.png` alone; the origin only in the source.

**F6 — unique among what.**
(a) **Unique across the chat's whole list** — staged images, sent images, stored
files — with `-2`, `-3`; and the same for `clipboard*.png` when the switch is off,
so every paste is reachable by name either way. *(recommended: the uniqueness of
§2.2 is a defect of its own, and fixing it costs nothing.)*
(b) As today, among the staged images only.

**F7 — a name typed by hand.**
(a) **`/image paste <name>`** — the free route, on every engine and with the
switch off; the typed name goes through the same reduction and uniqueness.
*(recommended)*
(b) No.

**F8 — which images are named.**
(a) **Pasted ones only.** *(recommended — the ask; a file and a URL bring a name.)*
(b) Also files whose names say nothing (`IMG_1234.jpg`, `Screenshot 2026-…png`) —
a heuristic, recorded for later.

**F9 — when naming fails.**
(a) **Quietly**: a log line, the image keeps `clipboard.png` — the automatic
title's rule for a request nobody asked for. *(recommended)*
(b) A note in the feed.

**The switch itself** is the ask, not a fork: `images.name_pasted`, **off** by
default, one row in Settings → Images — "Name pasted images" — whose description
says that the image is sent to the chat's model once more as soon as it is
pasted, which costs a second of the local server or a fraction of a cent in the
cloud.

## 6. Stages, tests, live run

Two pull requests, independent of each other:

1. **The paste's gaps** (A1, A2): `file_list` after `get_image`; the SSH
   wording. Unit tests over the routing (image → files → text) with the clipboard
   behind a seam; the refusal in both locales. A clipboard test under `#[ignore]`
   beside `clipboard_image_round_trip`: a file list put on the real clipboard,
   read back and staged. No model involved — a live model run is not required.
2. **The name** (F1–F9): the switch and its row; the naming task and its landing;
   the slug reduction (property tests over hostile answers — paths, quotes,
   Cyrillic, empty, 400 characters, thoughts only); the rename by id, staged and
   just-sent; the bounded wait and its control arm (no wait with the switch off);
   uniqueness; `/image paste <name>`; the request with the switch off pinned
   byte-identical. Live, under `#[ignore]`: llama.cpp with Gemma 4 E2B and its
   projector — a paste is named, a send right after it waits and carries the
   name — and the same through OpenRouter on a cheap vision model.

Documentation per AGENTS.md §4: spec §9.10 and §12.1, README, the help overlay
(`/image paste [name]`), CHANGELOG, docs/journal/ui-input.md.

## 7. Not in this track

- **A naming slot of its own** (F1 b) — **not wanted** (the owner, 2026-11-08).
  Gemma 4 E2B was in the ask to see whether a very small model copes with the
  task, not as a namer beside the chat model; it copes (§3.2). It would not fit
  beside a big local model on one 24 GB card anyway: by file sizes alone, E2B with
  its projector (6.0 GB) next to Gemma 4 31B Q4_0 with its own (18.9 GB) or Qwen
  3.6 27B Q4_K_M with its own (19.7 GB) is 23–24 GiB of weights before any
  context.
- **Wayland without XWayland** — done after the track: `wayland-data-control` is on
  (§3.3).
- **A paste across SSH**: no protocol brings an image from the person's
  clipboard to a remote app; the routes stay a copied file, a URL, and a picture
  sent to the Telegram bridge (ADR 0013).
- **Rendering images in the feed** — a roadmap item of its own.
