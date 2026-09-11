# Local files are read in their own encoding

<!-- cyrillic-ok:start — this doc is *about* reading Russian text files: every
     Cyrillic run below is a measurement's input or output. Transliterating them
     would describe a different experiment. The prose itself is English. -->

> **Status:** implemented (2026-09-11) — the user's decisions of 2026-09-11: every fork
> as recommended — F1a every reading path, F2c the interface language as the hint,
> F3b an edit written back in the file's own encoding behind the round-trip check,
> F4b the encoding named to the model, F5b the order moved to `shared::text_decode`
> in a refactor PR of its own first, F6a reading and writing in one PR. The follow-up that
> [page-charset.md](page-charset.md) §5 left: fetched pages are decoded in their own
> encoding by `shared::http_text`, while every path that reads a file from disk still
> assumes UTF-8 — and a user on Windows keeps Russian text in windows-1251 (Notepad's
> "ANSI"), UTF-16LE (Notepad's "Unicode"), IBM866 (anything `cmd.exe` redirected to a
> file) or KOI8-R (anything from a Unix box of the era).

## 1. Where a local file becomes text

Every path that turns a file's bytes into text, found by a sweep of `src/` for
`read_to_string`, `fs::read`, `from_utf8*`, `BufReader`, `File::open` and
`read_to_end`. The six rows marked **measured** were driven through the tools'
own code (`FsRead::invoke`, `CodeTool::{Read, Grep, Edit}`, `workspace_diff::build`,
`rag_ingest::read_text`) with the fixture files of §2.1; the rest are read from the
code.

| Entry | Where | Bytes → text today | windows-1251, KOI8-R, IBM866 | UTF-16LE + BOM | UTF-8 + BOM |
|---|---|---|---|---|---|
| `fs_read` — **measured** | `tools/fs.rs` `FsRead::invoke` | `from_utf8_lossy`, no binary check | every Cyrillic letter `U+FFFD` | a NUL between the letters | readable, a `U+FEFF` left at the start |
| `code_read` — **measured** | `tools/code.rs` `TextFile::load` | a NUL → "not a text file"; UTF-8 BOM stripped; `from_utf8_lossy` | every letter `U+FFFD` | refused: *"This is not a text file."* | right |
| `code_grep` — **measured** | `code.rs` `searchable` → `TextFile::load` | the same; a file that fails is skipped | *"no line matches"* — the file cannot be found by its own words | skipped; with only such files the answer says **no file matched `*`** | right |
| `code_edit` — **measured** | `code.rs` `edit` | `TextFile::load` → the edit → `TextFile::encode`, which writes **UTF-8** | **the whole file rewritten**: 68 / 65 / 53 letters became `EF BF BD` after an ASCII edit, and the changes screen showed `+1 −1` | refused | byte-exact |
| changes screen (`F4`) — **measured** | `workspace_diff.rs` `diff_file` | both sides `from_utf8_lossy`, compared as strings | shows only the edited line — both sides lost the same letters | not shown (a NUL) | right |
| `/file attach` — **measured** | `rag_ingest::read_text` via `read_source_text` | strict `read_to_string`, UTF-8 BOM stripped | refused: *"could not read the file as text: stream did not contain valid UTF-8"* | refused, the same | right |
| `/rag add`, `/rag rebuild` | the same `read_text` | the same | counted as an error; the reason is in the log only | the same | right |
| `code_write` | `code.rs` `write` | the existing file is read only for its CRLF/BOM shape | the new content replaces the file in UTF-8 | — | kept |
| DOCX | `doc_extract::extract_docx` | quick-xml `decode`, strict UTF-8 | Word writes UTF-8 | — | — |
| PDF | `pdf_extract` | the crate's own decoding | — | — | — |
| `mindfork import` | `features/import.rs` | lossy, then JSON | JSON is UTF-8 by its standard (RFC 8259) | — | — |
| MCP servers import | `orchestrator/mcp.rs` | strict, then JSON | the same | — | — |

The app's own files — settings, profiles, chats, the journal manifest, dictionaries,
external locales, secrets — are written by the app in UTF-8 or are JSON by format,
and are out of scope.

- **`code_edit` corrupts a legacy file, and the one screen built to show what the
  assistant changed hides it.** An ASCII edit in a windows-1251 source rewrote all 68
  Cyrillic letters of its comments as `EF BF BD` — the UTF-8 of `U+FFFD` — and the
  changes screen reported one line added and one removed, because it compares the
  two files lossily and both sides lose the same letters. The journal holds the
  original bytes, so `r` on that screen restores the file — for a user who knows to
  look. This is worse than any unreadable file here: the damage is on disk.
- **Search answers wrongly about the project.** A word in a legacy file draws *no
  line matches*; a project of UTF-16 files draws a claim that no file matched `*`
  (lessons §4 — the message names a situation that is not the one).
- **Attachments and RAG refuse**, as their design says (docs/file-attachments.md §4.7,
  fork F8: "anything that decodes as valid UTF-8"); the refusal names UTF-8 and not
  what the user could do, and `/rag add` shows an error count only.
- **A UTF-16 file is "binary" to the code tools** — the NUL check runs before any
  decoding, so Notepad's "Unicode" never reaches a decoder.

## 2. What was measured

### 2.1. The page decoder on local files

Twenty-four fixture files: four shapes — 300 bytes of prose, an 18-byte single line,
a Rust source whose only Cyrillic is two comments, a three-row CSV — in six
encodings: windows-1251, KOI8-R, IBM866, UTF-16LE with a BOM, UTF-8 with a BOM, and
UTF-8. Lines end in CRLF, as Notepad saves them. Each went through
`http_text::decode(bytes, "", None)` — no header, no TLD — and is compared with what
`String::from_utf8_lossy`, the call `fs_read` makes today, produces.

| Encoding | Decided as (step) | Right | `from_utf8_lossy` today |
|---|---|---|---|
| windows-1251 | windows-1251 (detected) | 4 / 4 | every Cyrillic letter `U+FFFD` (235 in the prose) |
| KOI8-R | KOI8-U (detected) | 4 / 4 | every letter `U+FFFD` but two accidental pairs |
| IBM866 | IBM866 (detected) | 4 / 4 | 166 `U+FFFD`, and 17 runs of two or three letters fused into characters of other scripts |
| UTF-16LE + BOM | UTF-16LE (BOM) | 4 / 4 | a NUL between the letters (65 in the prose); the detector alone names windows-1254 |
| UTF-8 + BOM | UTF-8 (BOM) | 4 / 4 | readable, with a `U+FEFF` left at the start |
| UTF-8 | UTF-8 (bytes) | 4 / 4 | unchanged |

- **Every file comes back whole.** KOI8-U for a KOI8-R file is the same letters, as on
  the web (page-charset.md §2.3).
- **The BOM step is not optional on disk.** Handed a Notepad "Unicode" file, the
  detector names windows-1254; only the mark reads it.
- **The single line and the source file were right** — but one line is one sample,
  which is what §2.2 is for.

### 2.2. Short text is the detector's limit

Thirty-two short Russian strings (a word, a table header, a CSV heading, a code
comment), seven Ukrainian and ten Western ones, each encoded and put through the
decoder with each TLD hint:

| Hint | ru windows-1251 | ru KOI8-R | uk windows-1251 | Western windows-1252 |
|---|---|---|---|---|
| none | 21 / 32 | 26 / 32 | 5 / 7 | 9 / 10 |
| `ru` | **27 / 32** | **29 / 32** | 5 / 7 | 9 / 10 |
| `ua` | 27 / 32 | 29 / 32 | 5 / 7 | 9 / 10 |
| `de` | 22 / 32 | 26 / 32 | 5 / 7 | **10 / 10** |

- **Without a hint a word is a guess.** `да` came back as `äà` (windows-1252),
  `файл не найден` as Hebrew (windows-1255), `жёлтый` as EUC-JP.
- **A language hint helps and costs the other side nothing measured.** `ru` lifts the
  Russian strings by six and three, and the Western ones stay at nine of ten; what
  it leaves wrong is one- and two-word strings, where no statistic has anything to
  count.
- **A local file has no TLD, but the app knows its user's languages** — the interface
  language (`config.interface.language`) and the profile's (`Profile.language`, which
  a tool reads as `ctx.loc.lang()`).
- **The operating system's locale is not a substitute.** Measured on the development
  machine, where Russian files are the subject of this very track: culture `en-US`,
  ANSI code page 1252, OEM 437. A hint taken from it would have steered every short
  Russian file toward windows-1252.
- **`chardetng` has no other lever.** `EncodingDetector::find_score`, which would let a
  caller choose among Cyrillic candidates itself, exists only behind the
  `testing-only-no-semver-guarantees-do-not-use` feature. The TLD is the one input.

### 2.3. IBM866 is the majority rule's worst case

In IBM866 the lower-case `р`–`я` sit at `0xE0`–`0xEF`, UTF-8's three-byte lead bytes,
and `а`–`п` and the capitals at `0x80`–`0xAF`, its continuation bytes — so `р` followed
by two such letters is a valid UTF-8 sequence. Forty-six IBM866 samples (the short
strings, words chosen to start with `р`, lines of `dir` and of a `cmd.exe` error, the
CSV):

- **Two were taken for UTF-8**: `Ёлка` and `рак`, each a single valid sequence with
  nothing broken. No rule that looks at UTF-8 validity alone can refuse them — the
  bytes *are* valid UTF-8.
- **Nothing longer than a word crosses the majority.** `cmd.exe`'s lines decode 1–3
  characters against 6–23 broken sequences; the densest sample, the CSV, 5 against 10.
  On the web the worst measured was GBK at 59 against 218 (page-charset.md §2.1).
- **The other wrong answers are §2.2's**: a word the detector guesses as another
  encoding. The `ru` hint lifts the 32 short Russian strings in IBM866 from 26 to 28.

### 2.4. A wrong guess still gives the bytes back

Writing an edit in the file's own encoding (F3) rests on one property: re-encoding
the unedited text in the chosen encoding reproduces the file byte for byte, **even
when the choice was wrong**. Measured on all 113 short samples of §2.2 and §2.3 —
Russian in windows-1251, KOI8-R and IBM866, Ukrainian, Western:

| Hint | Right | Wrong, round-trips | Wrong, does not round-trip |
|---|---|---|---|
| none | 87 | 26 | 0 |
| `ru` | 98 | 15 | 0 |

- **Every wrong guess round-tripped** — the two multi-byte ones (`жёлтый` as EUC-JP
  and as Big5) included. So a misread file loses nothing outside the fragment that
  was edited: the untouched bytes go back exactly as they came.
- **That is a property of these samples, not a law.** windows-1253 leaves `0xAA`,
  `0xD2` and `0xFF` undefined, and a multi-byte guess can meet a sequence it cannot
  form again. A check before writing — decode, re-encode, compare with the bytes on
  disk — is therefore part of the design, not an optimisation; it is exact and costs
  one more pass over the file.
- **What a wrong guess can still do is confined to what the model wrote.** It sees the
  misread text, and a character it inserts that the wrong encoding can hold is written
  as that encoding's byte — a wrong letter in the edited line, visible in the diff. A
  character the encoding cannot hold is caught by `encode` before anything is written.

## 3. The seam

The decision already exists as a function of bytes —
`http_text::decode(bytes, content_type, tld)`, page-charset.md §3 — so a local file
needs no new order, only four callers and one thing a page never had: a way back.

- **`TextFile` is where the code tools meet** (`tools/code.rs`). `code_read`,
  `code_grep`, `code_edit` and `code_write` all load through `TextFile::load`, and the
  two writers write through `TextFile::encode`. It gains the encoding the file was
  read in: `load` decodes by the order — a BOM first, so a UTF-16 file is recognised
  before the NUL check that today calls it binary, the NUL check then applying to
  the decoded text — and `encode` writes back in that encoding (F3).
- **`rag_ingest::read_text`** is the one function behind `/file attach`, `/rag add`
  and `/rag rebuild`.
- **`FsRead::invoke`** reads through the same decision.
- **The changes screen compares bytes, not decoded text**, for its *changed / left as
  it was* state, and decodes both sides in the current file's encoding for the diff it
  draws — so a byte change can never be hidden again. That is a requirement rather
  than a choice, and it stands whatever F3 decides.
- **A document declaration is looked for only in a file that is markup** — `.html`,
  `.htm`, `.xml` by extension. A local file has no `Content-Type`, and handed an empty
  one the order scans any file, so a Markdown note *about* `<meta charset>` would
  declare an encoding it does not use.
- **The hint** is a language turned into the TLD `chardetng` understands (`ru` → `ru`,
  `uk` → `ua`, `en` → none; F2).

## 4. Forks

- **F1. Which paths decode.** (a) Every path of §1 that reads a user's file — `fs_read`,
  the four code tools, the changes screen, `/file attach`, `/rag add` and
  `/rag rebuild`. (b) The model's tools only. (c) Attachments and RAG only. —
  *Recommended: (a).* They meet in three places (`TextFile`, `read_text`, `FsRead`),
  and leaving one out keeps a file readable to one tool and garbled to its neighbour.
  The two JSON imports stay UTF-8, as their format says; DOCX and PDF are untouched.
- **F2. The detector's hint for a file.** (a) None. (b) The profile's language — what a
  tool already carries as `ctx.loc`. (c) The interface language
  (`config.interface.language`), carried into `ToolContext` for the tools. (d) The
  operating system's locale. — *Recommended: (c).* A file belongs to its user, not to a
  profile's prompt language — a Russian user can run a profile with an English scaffold;
  (d) is measured wrong on this very machine (§2.2); (a) gets a third of short Russian
  files wrong.
- **F3. An edit to a file that is not UTF-8.** Today it destroys the file (§1).
  (a) **Refuse**: reading works, and `code_edit`/`code_write` answer that the file is in
  windows-1251 and stays as it is. (b) **Write back in the file's own encoding**: the
  edit is written only if the unedited file round-trips byte for byte through that
  encoding (§2.4), and refused, naming the character, if the new text holds one the
  encoding cannot (an emoji; `—` in KOI8-R). (c) **Convert the file to UTF-8** on its
  first edit, and say so — every line then changes in the user's version control, and
  whatever reads the file in its old encoding (a `.bat`, a legacy tool's config) breaks.
  — *Recommended: (b).* It is what an editor does; the round-trip check makes a wrong
  guess lose nothing; and a UTF-16 file goes back through the same path (written by
  hand — `encoding_rs` has no UTF-16 encoder).
- **F4. Telling the model the encoding.** (a) Decode silently. (b) Name a non-UTF-8
  encoding in the reading tools' header (`prose.txt, windows-1251, lines 1–4 of 4`)
  and in the attachment's feed note. — *Recommended: (b).* Under F3(b) the model is
  about to write in that encoding, and a guess on a short file is something the user
  should be able to see.
- **F5. Where the order lives.** (a) Expose `decode` from `shared::http_text` as it
  stands. (b) Move the byte-level order to `shared::text_decode`, leaving `http_text` the
  response adapter — a mechanical move, so a small PR of its own first (AGENTS §2). —
  *Recommended: (b).* A module named for HTTP that decodes files on disk misleads
  whoever looks for it next.
- **F6. Stages.** (a) One PR: the reading paths and F3 together. (b) Reading first, with
  an edit of a non-UTF-8 file refused until a second stage writes it back. —
  *Recommended: (a)* if F3 is (b): the round-trip check is a few lines, and (b) would
  ship an interim refusal only to replace it. Either way the corruption ends in the
  first PR that lands.

## 5. What this does not do

- **A file of a word or two stays a guess** (§2.2): with the hint, 98 of 113 short
  samples are read right; a guess that is wrong still loses no bytes (§2.4).
- **UTF-16 without a BOM stays binary.** Notepad writes the mark; a file without it has
  no signal short of statistics on NUL positions, which nothing here measured.
- **The majority rule keeps its blind spot for a single IBM866 word** that happens to
  be valid UTF-8 (`рак`, `Ёлка`, §2.3): the bytes are valid, so no rule on validity
  alone can refuse them, and a length floor would misread a one-word UTF-8 file instead.
- **DOCX, PDF and the JSON imports** keep their decoding.
- **`code_grep`'s answer when every file was skipped** still says no file matched the
  glob; with UTF-16 read as text that answer becomes rare, and the message itself is a
  separate lessons §4 fix.

## 6. Tests and the live run

- **The decision helpers** (`shared::text_decode`): a whole UTF-16 file is text and a
  blob opening `FF FE` is not; a `<meta>` counts only in markup; `encode` names the
  character an encoding lacks and writes UTF-16 by hand; only a lossless read round-trips;
  every hint is a label `chardetng` accepts.
- **Every reading path through its real entry point**, with windows-1251 and UTF-16
  fixtures: `CodeTool::Read` names the encoding, `CodeTool::Grep` finds a Russian word,
  `FsRead` decodes and refuses a binary, `rag_ingest::read_text` and `extract_file` read
  what `/file attach` refused, and `workspace_diff::build` diffs a legacy file in its own
  encoding.
- **The defect itself**: an ASCII `code_edit` on a windows-1251 source leaves every
  other byte as it was; the same edit on UTF-16 comes back in UTF-16; `code_write` keeps
  an existing file's encoding.
- **Both refusals** — a character the encoding lacks, a lossy read — write nothing and
  journal nothing; a byte change that equal lossy text cannot show is still a change.
- **The hint** is the interface language's, through `ToolParams::from_config`.
- **Mutation-tested**: twelve mutations of the load-bearing lines, each killed by the
  test written for it. What no unit test pins is the hint's way into the orchestrator's
  attach, RAG and changes paths: their fixtures read right without it.
- **Live — GO**: `code_edit_legacy_encoding_e2e_live` against a local CPU build of
  llama.cpp serving Gemma 4 12B (the LAN stack was down). The model read the windows-1251
  source with its encoding named in the header, changed the divisor and the comment about
  it in one `code_edit`, and the file came back windows-1251 — no `EF BF BD`, the
  untouched first line byte for byte, the new Russian word stored in windows-1251.
