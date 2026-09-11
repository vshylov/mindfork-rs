# A fetched page is read in its own encoding

> **Status:** implemented (2026-09-11) — the user's decision of 2026-09-11:
> "the most effective variant" of the fix offered after the diagnosis, which
> settles F1 (§4); F2–F7 were taken under that instruction, each at the option
> marked below, and are listed so any of them can be overturned. Reported from
> a chat: the assistant read an article on a 2002-era site and got an
> attachment whose name, and nearly every letter of whose text, was `U+FFFD`.

## 1. Why, precisely

**The chat.** The user gave the assistant
`https://sector.biz.ua/mycomp/mid203/aid5.html`, an article from the archive of
a computer weekly. `fetch_url(summarize=false)` found the page over the
attachment budget and attached it, named
`����� ������ ������� «��� ���������» �� 2002 ���`. The model read four pages of
it with `attachment_read`, called the text irrecoverable in its thoughts, and
fetched the page again itself — three `python_exec` calls with `requests` and
`.decode('cp1251')` — before it could answer. Measured in the saved chat: the
attachment's text is 12 549 characters, **9 289 of them `U+FFFD`**; the 112
Cyrillic letters left are the app's own header around the page. The 27 search
fragments the feed reported were embedded from that text.

**The page was not at fault.** It is windows-1251 and says so twice: the server
answers `Content-Type: text/html; charset=windows-1251` (to the app's own
request headers — measured), and the page carries
`<META http-equiv=Content-Type content="text/html; charset=windows-1251">` at
byte 416.

**The client read neither.** `fetch_url` took the body with `resp.text()`
([fetch.rs](../../src/features/tools/fetch.rs)), and `web_search`'s result
fetch did the same ([web.rs](../../src/features/tools/web.rs)). `reqwest` has
been declared with `default-features = false` since M1 (`3f9688e`), to choose
its TLS backend, and its `charset` feature left with the defaults. In reqwest
0.13.4, `Response::text()` without that feature is:

```rust
let full = self.bytes().await?;
let text = String::from_utf8_lossy(&full);
```

Every windows-1251 letter (`0xC0`–`0xFF`) is an invalid UTF-8 sequence, so each
became one `U+FFFD`; the guillemets and `2002` survived because the page writes
them as `&laquo;`/`&raquo;` and digits are ASCII. The proof is exact: the bytes
of the page's `<h1>` decoded as lossy UTF-8, whitespace collapsed the way
`page_name` does, equal the stored attachment name character for character.

**Requirements.**

- **R1.** A page is read in the encoding it declares — in the header **or** in
  the document.
- **R2.** A page that declares nothing is still read, not taken for UTF-8.
- **R3.** A declaration the bytes contradict does not garble the page.
- **R4.** A UTF-8 page decodes exactly as it did.
- **R5.** A body the client cannot decode is reported, never handed to the model
  as text.

## 2. What was measured

### 2.1. Where real pages declare their encoding

Fifteen pages — legacy sites in five encodings and UTF-8 controls — fetched with
the app's request headers by a scratch script; the same corpus is
`http_text::tests::live_charset_corpus`. `ppomppu.co.kr` (EUC-KR) answered 403
and is left out. *Decoded / broken* counts, over the whole body read as UTF-8,
the multi-byte characters that decode and the sequences that do not.

| Page | Header charset | Document declaration (byte) | UTF-8 decoded / broken |
|---|---|---|---|
| sector.biz.ua (the chat's page) | windows-1251 | windows-1251 @ 416 | 0 / 11 861 |
| opennet.ru | koi8-r | koi8-r @ 17 | 6 / 6 049 |
| lib.ru | windows-1251 | — | 0 / 4 776 |
| kulichki.com | windows-1251 | — | 0 / 4 040 |
| abehiroshi.la.coocan.jp | — | Shift_JIS @ 14 | 1 / 13 |
| newsmth.net | GBK | GBK @ 27 | 59 / 218 |
| fourmilab.ch | — | iso-8859-1 @ 218 | 0 / 2 |
| citforum.ru | — | utf-8 @ 271 | 2 132 / 0 |
| people.com.cn | — | UTF-8 @ 43 | 7 919 / 0 |
| 163.com | utf-8 | *gzip — §2.2* | — |
| ixbt.com | utf-8 | utf-8 @ 90 | 13 173 / 0 |
| habr.com | utf-8 | UTF-8 @ **1698** | 43 590 / 0 |
| ru.wikipedia.org | UTF-8 | UTF-8 @ 621 | 15 012 / 0 |
| lemonde.fr | UTF-8 | UTF-8 @ 75 | 2 465 / 0 |

Three findings decided the design:

- **Four pages of fourteen declare only in the document** (abehiroshi,
  fourmilab, citforum, people.com.cn), two only in the header (lib.ru,
  kulichki). `reqwest`'s `charset` feature reads the header alone: turning it on
  would have fixed the chat's page and left a Shift_JIS page and an ISO-8859-1
  page as unreadable as before (R1).
- **A browser's prescan window is short for a reader.** The HTML standard's
  prescan reads 1024 bytes; habr.com's `<meta>` sits at byte 1698. A browser
  recovers from a late `<meta>` by reparsing; a reader holding the whole body can
  look up to `<body>`.
- **Legacy text does not pass for UTF-8.** Single-byte Cyrillic yields almost no
  valid multi-byte sequence (0–6 against thousands). GBK, whose trail bytes land
  in UTF-8's continuation range a third of the time, is the worst case at 59
  against 218 — about a quarter. Every UTF-8 page has 0 broken. A **majority**
  rule — more characters decode than sequences break — separates the two with a
  margin of nearly four on the worst legacy page, and still reads a UTF-8 page
  that carries a few stray bytes.

### 2.2. A body compressed unasked

`www.163.com` answered the corpus request, which carries no `Accept-Encoding`,
with `Content-Encoding: gzip`: 55 632 bytes starting `1f 8b 08 00`, inflating to
276 188 bytes of valid UTF-8 that declares `<meta charset="utf-8">`. `reqwest` is
built without its decompression features too, so `fetch_url` received the gzip
stream itself and read it as text — the same symptom from a different cause.

### 2.3. The detector on real pages

The same corpus through the app's own client (`live_charset_corpus`), each body
handed to the detector whole — it reads no declarations — with the host's TLD and
without:

| Page | Decided (step) | Detector, with TLD | Detector, no TLD |
|---|---|---|---|
| sector.biz.ua | windows-1251 (header) | windows-1251 | windows-1251 |
| opennet.ru | KOI8-R (header) | KOI8-U | KOI8-U |
| lib.ru | KOI8-R (header) | KOI8-U | KOI8-U |
| kulichki.com | KOI8-R (header) | KOI8-U | KOI8-U |
| abehiroshi.la.coocan.jp | Shift_JIS (document) | Shift_JIS | Shift_JIS |
| newsmth.net | GBK (header) | GBK | GBK |
| fourmilab.ch | windows-1252 (document) | windows-1252 | **windows-1257** |
| the seven UTF-8 pages, 163.com unpacked | UTF-8 (bytes) | windows-1252, windows-1251 or GBK | windows-1252 |

- **Every legacy page is guessed right**, with one naming subtlety: for KOI8-R the
  detector answers **KOI8-U**, the superset that differs only in positions Russian
  text does not use. It is why step 4 asks whether the detector's guess *reads the
  bytes like* a declaration rather than *is* it (§3) — compared by identity, a KOI8-R
  page with a wrong `windows-1251` header and a right `<meta>` would have taken the
  header.
- **The TLD hint earns its place on thin evidence.** fourmilab.ch carries two
  non-ASCII bytes; without the hint they read as Baltic windows-1257, with `.ch` as
  windows-1252.
- **The UTF-8 pages show why step 2 comes first.** With UTF-8 excluded, the
  detector names a legacy encoding for each of them; a UTF-8 page must never reach
  it, and none does.
- **Two servers changed their answer between runs.** lib.ru and kulichki.com declared
  `windows-1251` to the scratch script and `KOI8-R` to this run — whose bytes the
  detector also read as KOI8 — so they recode per request, by its headers. Any
  per-host memory of a site's encoding would be wrong.
- **A count of `U+FFFD` proves nothing here.** A single-byte decoder never produces
  one, right or wrong, so the test asserts agreement with the evidence instead: every
  legacy decision reads like the detector's guess, and every UTF-8 decision holds no
  replacement character. All fourteen agree.

## 3. The order

`shared::http_text::read` — one seam, called by `fetch_url` and by `web_search`'s
result fetch — reads the body, undoes the content-coding, and decodes by this
order, first match wins:

1. **A byte-order mark.**
2. **The bytes, when they read as UTF-8**: some non-ASCII, and more characters
   that decode than sequences that do not (§2.1). This comes *before* any
   declaration, because a site moved to UTF-8 routinely keeps its old
   `<meta charset=windows-1251>`, and a UTF-8 page read as windows-1251 is as
   garbled as the reverse (R3, R4).
3. **Pure ASCII**: whatever is declared — only a seven-bit encoding
   (ISO-2022-JP) reads it differently — else UTF-8.
4. **A declaration the bytes do not refute.** The `Content-Type` charset, or the
   document's own: an XML declaration opening it, else the first
   `<meta charset>` or `<meta http-equiv="content-type" content="…charset=…">`
   before `<body>`, comments skipped. With step 2 failed, a declared UTF-8 is set
   aside rather than obeyed — the shape of a server-wide `charset=utf-8` in front
   of a legacy page. When the header and the document name two different
   encodings, the detector decides between them — by whether its guess reads the
   bytes like one of them, since it names a family's superset (§2.3) — and the
   header wins when it reads like neither. As in a browser, a document declaring UTF-16 means UTF-8
   (it was just read as ASCII) and `x-user-defined` means windows-1252; a label
   that maps to the `replacement` encoding (`iso-2022-kr`, `hz-gb-2312`) — which
   decodes any input to a single `U+FFFD` — counts as no declaration (R1, R3).
5. **The detector**: `chardetng`, the one Firefox runs on unlabelled pages, with
   UTF-8 excluded (step 2 already said no) and the host's top-level domain as its
   hint (R2).

**The content-coding comes first.** The request advertises none, so whatever
arrives was applied unasked. `gzip`/`x-gzip` and `deflate` (zlib-wrapped as RFC
9110 has it, or raw as some servers send it) are undone with `flate2`, last
applied first, and the unpacked size is bounded at 32 MiB — a small response
inflating into a large allocation is what a decompression bomb is. Any other
coding, damaged data or an oversized body is an error: `fetch_url` says so and
that fetching again will fail the same way (R5, lessons §4), and `web_search`
skips that page as it skips any page it cannot read.

## 4. Forks

- **F1. Where the fix lives.** (a) Turn on `reqwest`'s `charset` feature — one
  line, the header only: the chat's page is fixed, the four document-only pages
  of §2.1 are not. (b) Decode from the bytes, in one module both readers call. —
  **The user's decision (2026-09-11): the most effective variant, (b).**
- **F2. A page that declares nothing.** (a) UTF-8, lossy — the old behaviour for
  every page. (b) windows-1252, a browser's fallback without a locale guess.
  (c) `chardetng` with the TLD. — **(c)**: (a) is the defect, and (b) is the
  defect for every non-Latin page.
- **F3. Do the bytes outrank a declaration?** (a) Never, as the HTML standard
  says: a wrong declaration garbles the page, as it does in a browser. (b) Valid
  UTF-8 outranks any declaration, and a UTF-8 declaration the bytes refute is set
  aside. — **(b)**: a reader keeps no compatibility with pages that render
  garbled, and §2.1 measured the margin.
- **F4. How far to look for `<meta>`.** (a) 1024 bytes. (b) Up to `<body>`. —
  **(b)**: habr.com.
- **F5. The header and the document disagree.** (a) The header, as the standard
  says. (b) The document. (c) The detector decides, the header when its guess reads
  the bytes like neither. — **(c)**: the classic disagreement is a server default
  (`ISO-8859-1`) in front of a page that knows its own encoding, which (a) gets
  wrong, while (b) is wrong behind a transcoding proxy; the detector is in the
  build anyway.
- **F6. Compressed bodies.** (a) Turn on `reqwest`'s decompression features —
  `Accept-Encoding` then goes out on **every** request the app makes, the
  engines' token streams included, which a compressing proxy would buffer.
  (b) Undo `gzip`/`deflate` in the page path with `flate2`, already a
  dependency. — **(b)**.
- **F7. One PR.** Both defects are one seam, a response body turned into text,
  with one symptom. — **One PR.**

## 5. What this does not do

- **A lone legacy declaration is trusted.** A server sending
  `charset=ISO-8859-1` in front of a windows-1251 page that declares nothing
  still garbles it, as a browser does. Cross-checking a single declaration
  against the detector needs a confidence threshold nothing here measured.
- **An undeclared ISO-2022-JP page** is seven-bit, so it takes step 3 and reads
  as ASCII carrying escape sequences. Such text is mail, not the web.
- **The request still advertises no compression.** Asking for `gzip` would make
  pages cheaper to download; nothing measured says that matters here.
- **The attachment's name** is the page's `<h1>` (`page_name`), and on this site
  the `<h1>` is the archive's banner rather than the article: the name is
  readable now and still not the article's title. A separate question.
- **Local files are another entry point.** `fs_read` reads a file lossily and
  `/rag add` refuses one that is not UTF-8; the same decoding would apply, as a
  separate decision.
- **An attachment garbled before the fix stays garbled.** The lossy decode
  discarded the bytes; nothing in the saved chat can bring them back.

## 6. Tests and the live run

- **The decision table** — 28 rows in one raw-string literal
  (`http_text::tests::CASES`). Each row names the prose's script, the shape of the
  bytes (a label, or a BOM, UTF-16 or stray bytes no encoder makes), the
  `Content-Type`, the document's head and the TLD, and asserts the chosen encoding,
  the step that chose it, and that the prose comes back whole — with exactly the two
  `U+FFFD` a stray-bytes row inserts and none anywhere else.
- **The premise of the GBK row** is its own test: the GBK prose really does form some
  valid UTF-8 pairs, and is outvoted.
- **The content-coding**: `gzip`, `x-gzip`, zlib and raw `deflate`, `deflate, gzip`
  undone in reverse, `identity`; `br`, damaged gzip and an inflation past the bound,
  each an error naming its coding.
- **The TLD hint** only ever takes a form `chardetng` accepts; it panics on an
  upper-case letter or a period.
- **Through the callers** (lessons §2 — a parser's green tests say nothing about
  whether its caller reaches it): `fetch_url`'s `fetch_text` and `web_search`'s
  `fetch_content`, each against a stub serving a windows-1251 page declared only in
  `<meta>` and gzipped unasked — two shapes `resp.text()` could not read even with
  its `charset` feature on.
- **Mutation-tested**: sixteen mutations of the load-bearing lines, each killed by
  the case written for it — through both callers for `read` decoding lossily again
  and for gzip left packed. The one that first survived, the TLD hint dropped (§2.3
  had measured it live only), got its own row: fourmilab.ch's accented letter on an
  undeclared windows-1252 page with the `ch` hint.
- **Live — GO** (network only, 2026-09-11). `live_windows_1251_page_is_readable`
  fetches the chat's page through `FetchUrl::invoke`: attached under the archive's
  `<h1>`, 12 632 characters, the article's title in it, no `U+FFFD`.
  `live_charset_corpus`: the table of §2.3, every decision in agreement with the
  evidence.
