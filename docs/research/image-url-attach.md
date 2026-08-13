# Research: attaching an image by URL

**Status:** design complete; **user's decision, 2026-08-13: all seven forks go
with the recommendations below** (F1 = always download client-side, F2 = scheme
and bounds without an address filter, F3 = auto-detect inside `attach`, F4 =
name from the last path segment with the URL as `source`, F5 = `Content-Length`
plus a streaming cap, F6 = sniff the bytes and put the served `Content-Type`
into the refusal, F7 = `tools.web_enabled` does not gate it). Implementation
follows. Roadmap item:
"Multimodality — what the track left open → attach by URL". This is the smaller
of the two items the multimodality track deliberately left open; the other one
(rendering images in the feed) is untouched here.

Related: [multimodal-images.md](multimodal-images.md) §5 (the track, and the
"Deferred" note that named this item), [mcp-tool-images.md](mcp-tool-images.md)
(the sibling follow-up), spec §9.10 (image input), spec §9.3 (`fetch_url`),
spec §13.4 (untrusted content).

## 1. Problem

`/image attach <path>` stages a file. An image the user is looking at in a
browser has to be saved to disk first, then attached by path — two steps for
something the providers themselves treat as one. `/image paste` covers the
screenshot case; it does not cover "this diagram on a wiki page".

## 2. What the code actually does today

Read first, because it moved the design (lessons §3).

**The staging path is already source-agnostic.** `orchestrator/images.rs` has
one staging function, `stage_image(ImageSource)`, and `ImageSource` is a
two-variant enum — `File(path)` and `Clipboard{…}` — whose only job is to
produce bytes on the blocking pool. The cap check, the `/props` capability
probe, the background encode, the staging slot, the chip and the notes are all
*below* that seam and are shared. A URL is a third variant, not a third
pipeline. This matters beyond tidiness: PR #308 (clipboard paste) failed the
Sonar duplication gate at 8.2% against a 3% bar precisely because the first
attempt copied the attach handler, and the fix was this seam.

**`prepare()` takes `&[u8]`.** Normalization (xAI takes png/jpeg only),
downscaling to `images.downscale_px`, the byte-for-byte pass-through for a png
already within the ceiling, and the measurement that feeds the chip — all of it
already works on bytes from anywhere.

**There is no SSRF guard to reuse.** The deferred note in
[multimodal-images.md](multimodal-images.md) §5 says a client-side download
"drags in SSRF concerns `fetch_url` already had to solve". That premise is
wrong: `features/tools/fetch.rs` validates the *scheme* and nothing else —

```rust
if !(url.starts_with("http://") || url.starts_with("https://")) {
    anyhow::bail!(ctx.loc.t("tool.fetch_url.err.url_scheme"));
}
```

— and its `reqwest::Client` follows redirects with the default policy. No
address is inspected, private ranges included. So F2 below is not "reuse the
existing guard"; it is "decide whether this project wants one at all, and where"
— and the honest answer differs between a **user-typed** command and a
**model-chosen** URL. See §6.

**No `url` crate dependency is needed.** `reqwest::Url` is the `url` crate's
type, re-exported; `reqwest` is already built with `stream`, which is what a
streaming byte cap needs.

## 3. Where it lands

| Layer | Change |
|---|---|
| `features/image_command.rs` | none — `attach` already takes an arbitrary argument |
| `features/image_fetch.rs` *(new)* | download to bytes: scheme check, redirect cap, timeout, streaming byte cap, an error that names what went wrong |
| `app/orchestrator/images.rs` | `ImageSource::Url(String)` + a `prepare_url` next to `prepare_image`; `handle_image_attach` picks the variant by scheme |
| `entities/message_image.rs` | none — `source` is a free-form string; the URL is its own canonical source |
| wire adapters | **none**: what is staged is an ordinary `MessageImage`, so all four request shapes and the byte-identical-without-images guarantee are untouched |
| locales, help, README, spec §9.10 | the new argument form and its error messages |

## 4. Forks (need the user's decision before implementation)

### F1 — download the bytes, or hand the provider the URL? *(the structural one)*

| | A. Always download at attach time | B. Pass the URL through natively, download only for Gemini |
|---|---|---|
| Providers covered | all five, one code path | four natively, one by download anyway |
| Normalization/downscale | applies (xAI's png/jpeg-only matrix stays satisfied) | skipped on the pass-through arm — a webp URL works on some providers, not others |
| Wire shapes | unchanged | a second per-provider shape to write and test |
| Link rot | the pixels live in the chat, like every other attach | a dead link silently breaks a *stored* conversation on replay |
| Privacy | the URL never leaves the machine | the provider is told what the user is looking at, and fetches it themselves |
| First-turn bytes | uploaded once, then replayed like any attach | saved on the first turn |

**Recommendation: A.** B buys one turn's upload and pays for it with a second
wire shape per provider, a support matrix the user discovers at send time, and a
history that can rot. A also makes the whole feature invisible below the
staging seam — which is what keeps this a small PR.

### F2 — what network policy does a *user-typed* URL get?

| | A. Scheme + bounds only | B. Block private/loopback/link-local | C. B with an opt-out setting |
|---|---|---|---|
| Blocks the metadata endpoint (`169.254.169.254`) | no | yes | yes by default |
| Blocks the user's own NAS / LAN web server | no | **yes** | only until they find the setting |
| Blocks this feature's own unit tests | no | yes (they serve from `127.0.0.1`) | needs a test-only bypass |
| Matches the trust level of the sibling command | yes — `/image attach <path>` reads any file the user names | no | no |

**Recommendation: A**, with the bounds spelled out and enforced: `http`/`https`
only (re-checked after every redirect), at most 5 redirects, a connect+read
timeout, and a hard byte cap enforced *while streaming* — plus §6, which moves
the address question to where it actually bites.

The reasoning is who chooses the URL. `/image attach` is typed by the user, in
the same input box as `/image attach <path>`, which reads any file on the disk;
an address filter there defends the user against themselves while breaking a
real use — this project's own users run a llama-server on a LAN address, and
"attach the chart my local dashboard is serving" is the same shape. The moment
a *model* can stage an image by URL, that reasoning inverts, and the doc says so
in §6 rather than leaving it to be rediscovered.

### F3 — command surface

**Recommendation: auto-detect.** `/image attach <url>` when the argument starts
with `http://` or `https://`, `/image attach <path>` otherwise. No path on
Windows or unix begins with those, and a separate `/image url` subcommand would
be a second thing to document, translate, and put in the usage line for no gain.
`list` and `remove` need nothing: the URL is the image's `source`, so `#N`, the
display name and dedupe-by-source all work as they stand.

### F4 — what the staged image is called

**Recommendation:** display name = the last path segment of the URL,
percent-decoded and capped at 64 characters; when there is no usable segment
(`https://host/`, a query-only image endpoint), fall back to the host plus the
extension the content sniffs to. `source` = the URL as typed, so attaching the
same URL twice replaces rather than duplicates — the behaviour a file attach
already has.

### F5 — the size cap

**Recommendation:** refuse on `Content-Length` before downloading when the
header is present and over `images.max_bytes`, *and* enforce the same cap on the
stream regardless — a header can be absent or a lie, and the cap is the thing
that keeps a hostile URL from filling memory. Same limit and same message as the
file path, whose ceiling is also measured before the decoder ever sees the
bytes.

### F6 — a wrong content type should say what is wrong

**Recommendation:** sniff the bytes (that is what `prepare()` already does), but
keep the response's `Content-Type` for the *message*: "that URL served
`text/html` — a page, not an image; link the image itself" closes the door,
where "unsupported or corrupt image format" sends the user to re-save the file
(lessons §4). This is the single most likely user error in the feature.

### F7 — does `tools.web_enabled` gate it?

**Recommendation: no.** That switch governs what the *tools* may do on the
user's behalf while the model drives. Typing a URL into the input box is the
user reaching the network themselves, in a session where they also type
`/image attach` against their own disk. Worth stating in spec §9.10 so it is a
decision rather than an omission.

## 5. Test plan

- **Unit, no network**: a `std::net::TcpListener` stub (the project's idiom —
  `shared/api/http.rs`, `openai/client.rs`) serving a generated png. Cases: a
  plain 200; a 302 chain within the cap; a chain over the cap; a redirect from
  `https` to a non-http scheme; a 404; a body over `max_bytes` **with a truthful
  `Content-Length`** and again with **none**, to prove the streaming cap and not
  just the header check (a header-only test passes with the stream cap deleted —
  lessons §2 on gates that pass for the wrong reason); `text/html` served at an
  image URL; a name derived from a URL with a query string and one with no path
  segment.
- **Localization gate**: every new error rendered under all bundled languages,
  no unsubstituted placeholders, no Cyrillic in `en` — the existing
  `*_are_localized_for_all_langs` shape.
- **Orchestrator**: a URL attach reaches the same staging slot, obeys
  `max_count`, and dedupes against a second attach of the same URL.
- **Live** (`#[ignore]`, mandatory per AGENTS.md §3 — this touches the engine
  path): serve the existing generated fixture (blue field, white square) from a
  local listener, attach it **by URL**, and ask the model what it sees, with a
  **control arm that stages nothing** and must fail to answer. The control is
  not optional: the last two image tracks both produced a probe that looked
  green and was a hallucination (mcp-tool-images §2.1).

## 6. Recorded for later: `fetch_url` picks its URLs from model input

Out of scope for this PR, and written down because §2 found it rather than
assumed it: `fetch_url` applies no address policy, and its URL can come from a
page it fetched a moment earlier — the classic injection route to a link-local
metadata endpoint or an unauthenticated service on the developer's own machine.
The user-typed reasoning of F2 does not cover it. Whether that deserves a guard,
and of what shape (resolve-then-connect against the resolved address, so DNS
rebinding cannot slip between the check and the connection), belongs in its own
item next to spec §13.4 — not smuggled into an image command.

## 7. Scope

One PR: the new `image_fetch` module, one `ImageSource` variant, the locale
keys, the help/README/spec lines, unit tests as above, and one live smoke.
Roughly a day including the live run.
