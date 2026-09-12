# Sandbox file exchange — files into, out of and between `python_exec` calls

Track design plan (stages, scope, forks). Genre per [AGENTS.md](../AGENTS.md) §1;
once the track is done this file moves to `docs/history/`.

**Status:** forks decided 2026-09-11 — every one as recommended except F10, which
drops the per-chat quota (§9). **Stage 0** (a read-only `site-packages`) merged as
#518. **Stage 1**, the MVP probe: GO on both families, Qwen 3.6 27B and Gemma 4 31B
(§10). **Stage 2**, outputs: merged as #520, live GO on Gemma 4 31B (§11).
**Stage 3**, inputs: merged as #521, live GO on Gemma 4 31B (§12).
**Stage 4**, opening: merged as #522, manual gate done on Windows (§13).
**Stage 5**, Local parity: on `feat/sandbox-files-local`, sub-decisions in §14.

The request: `python_exec` in its Wasmer mode is text in, text out — whatever the
code writes dies with the call. The user wants (a) **files out** — what the code
writes reaches the user, and an image also reaches the model; (b) **files in** —
the chat's files are visible to the code, read-only; (c) **files between calls** —
what one call produced is available to later calls in the same chat.

## 1. Context

### 1.1. Why now

- **The starter set is about files.** openpyxl, pypdf, pillow and matplotlib
  joined numpy and pandas (spec §13.2), yet a user's workbook or PDF cannot reach
  the code and nothing the code makes can leave.
- **matplotlib works and is deliberately unnamed.** `MATPLOTLIB_SHIM`
  (`src/shared/sandbox.rs`) renders PNG and SVG under WASIX, but the tool
  description omits it because a chart cannot leave the sandbox (spec §13.2) — why
  it was first rejected ([journal/tools.md](journal/tools.md), "pandas in the
  starter set").
- **The roadmap asks.** "A persistent per-chat scratch directory for
  `python_exec`" is groundwork ([roadmap.md](roadmap.md)) since a model lost a
  16 MB download between calls ([history/fetch-url-fidelity.md](history/fetch-url-fidelity.md)).
  D4 meets that need through stored files, not a shared writable directory.

### 1.2. Not in this track

Pixels in the terminal (the feed keeps its chip); a host directory writable across
calls, or `tools.fs_root`/a workspace mounted into the guest; tools that delete or
rename stored files; SVG rasterization; automatic promotion of an output to a text
attachment; a network allowlist.

## 2. What exists (survey, 2026-09-11)

- **One call, one job directory.** `WasmerSandbox::run` (`src/shared/sandbox.rs`)
  creates `JobDir` (`%TEMP%/mindfork-sbx-<uuid>`, removed on drop), writes
  `job.py` = `build_wrapper(code)`, mounts it at `/w`, and runs the packed image
  (`packed-sandbox.webc`, stage 0) — the job directory is the only host mount.
  Output is lossy-UTF-8 stdout/stderr, discarded on timeout; a single-permit gate
  refuses a concurrent run.
- **The tool** (`src/features/tools/python.rs`): schema `{ code }`, text result
  only, 8000-character cut. Dependencies come from `ToolConfig`
  (`src/features/tools/mod.rs`), which already carries `sandbox_dir` — the precedent
  for a data-root path. The description says every call is fresh, pinned per locale
  by `the_sandbox_description_says_state_does_not_survive_a_call`. **Local mode**
  (`run_local`): `python -c` in the app's working directory, 10 s, no isolation.
- **Tool images, one producer.** `ToolOutcome.images` is filled only by MCP
  (`src/shared/mcp.rs`, behind `tools.mcp_images`, on). `record_call`
  (`src/app/orchestrator/generation.rs`) runs `prepare_tool_images` (decode,
  `images.max_bytes`, normalize to png/jpeg, `tool-image-N`) onto the tool message;
  Gemini gets user parts after `functionResponse` (spec §9.10). Two gaps a new
  producer inherits: the **cap of 4** is `MAX_RESULT_IMAGES` inside the MCP client,
  and **nothing on the tool path asks `EngineBackend::vision`** — only `/image
  attach` does (`src/app/orchestrator/images.rs`). The `image` crate decodes
  png/jpeg/webp/gif/bmp; no SVG.
- **Effects.** `ChatEffect::AddAttachment` → `apply_effects` → `insert_attachment`
  (`src/app/orchestrator/attachments.rs`: dedupe by `source`, persist, prune index,
  feed note, index); `sync_attachments` mirrors mid-turn. Producing-tool precedent:
  `attached_result` (`src/features/tools/fetch.rs`).
- **An attachment is text.** `Attachment` (`src/entities/attachment.rs`) is a
  decoded snapshot in `chats/{id}.json`. `extract_file` reads up to 32 MB, decodes
  text or extracts html/pdf/docx, and **refuses a binary** — an xlsx cannot be
  attached. `resolve_handle`: `#N`/name/path, shared name refused.
- **An image is message-scoped.** `MessageImage`
  (`src/entities/message_image.rs`): base64 in the chat file, png/jpeg, downscaled;
  `config.images` limits (8, 10 MB).
- **Per-chat directories.** The workspace journal `data/workspace/<chat-id>/`
  (`src/shared/paths.rs`) is written from inside a tool via
  `ToolContext.workspace_journal` and cleared only on a new project or detach; soft
  delete (`src/app/orchestrator/chats.rs`) removes no directory. **Backups skip it**:
  `TOP_DIRS` is `chats`, `dictionaries`, `locales` (`src/features/backup.rs`),
  despite the comment in `paths.rs`. Confinement precedent:
  `src/features/tools/code.rs`.
- **Nothing opens a file in the OS** — no `explorer`/`xdg-open`/`ShellExecute`, no
  crate; `windows-sys` lacks `Win32_UI_Shell`.
- **Callers.** A sub-agent's `AddAttachment` lands in the parent; a background run
  lands its attachments at landing (`src/app/orchestrator/background_runs.rs`) and
  **never confirms** (spec §9.3.2). Silent tasks build an empty snapshot and do not
  offer `python_exec`.
- **The confirmation popup** shows name and presented arguments only
  (`src/screens/chat/popups.rs`, spec §9.8).

## 3. The user's decisions (2026-09-11)

- **D1 — outputs.** User's decision: a per-chat folder (`data/files/<chat-id>/`);
  the feed shows each file's name and path, and a command opens the file or its
  folder in the OS.
- **D2 — the model sees its charts.** User's decision: when the model/provider
  accepts images, they return through `ToolOutcome.images`, behind a settings
  switch like `tools.mcp_images` — the cost is image tokens.
- **D3 — inputs.** User's decision: the chat's attachments and images as copies
  under `/w/in`, **and** binary files from disk (xlsx, pdf, docx as they are —
  today `/file attach` keeps only decoded text).
- **D4 — persistence.** User's decision: through chat files — outputs become chat
  files that later calls see in `/w/in`; no shared writable host directory.
- **F1–F13** (§5). User's decision: every fork as recommended, except **F10 — no
  per-chat quota**, "because nothing may be lost".

## 4. Design outline

1. **Staging.** The tool copies the selected chat files (F7) into the job
   directory's `in/` — copies, never links; attachments and images come from what
   the chat stores (F3).
2. **Run.** Unchanged, with an empty `out/` beside `job.py`.
3. **Collection.** After `wasmer` exits (no race with the guest),
   `shared/sandbox.rs` walks `out/` under F4 and returns the bytes in
   `SandboxOutput.files` plus what was skipped and why; `JobDir` drops. `shared`
   knows nothing about chats.
4. **Storing.** The tool writes each file into the chat's folder
   (`ToolContext.files_dir`, a sibling of `workspace_journal`, `None` without a
   chat), and returns one `ChatEffect::AddChatFile` per file, images in
   `ToolOutcome.images` (F5) and a result listing handles (F6). Bytes are written
   from the tool as the journal's are; the listing changes only through the
   effect, so the orchestrator stays `Chat`'s sole owner.
5. **Applying.** The orchestrator applies `AddChatFile` like an attachment
   (persist, a feed note with name and absolute path, the chip) and mirrors it into
   the turn's snapshot, so the next call in the turn can stage it.

Contract changes: `SandboxRunner::run` takes a job (code, inputs, collection
limits); the schema gains `files` (F7), and ADR 0005 §3's mode-independent schema
then needs F11's parity.

## 5. Forks

### F1. The chat file store

- **(a)** A new entity `ChatFile { id, name, stored, origin, mime, bytes, sha256,
  added_at }` in `Chat.files` (additive, [ADR 0006](decisions/0006-data-schema-versioning.md)
  F12), bytes under `data/files/<chat-id>/`, `origin` = `Sandbox { call_id }` |
  `Disk { source }`. `Attachment` gains only an optional `file_id` (F8).
- **(b)** `Attachment` gains `kind: Text | Binary` and a blob reference.
- **(c)** Text attachments and message images move into the store too, the old
  types becoming views; a migration.

**Recommendation: (a).** An attachment is "text shown every turn under a budget" —
pinned block, `decide_mode`, index, `attachment_read`/`_search`, the chip. A binary
has none of it, so (b) spreads `if binary` through every consumer; (c) migrates
every chat for no visible gain. **User's decision: (a).**

### F2. File names on disk

- **(a)** Readable: `<sanitized name>`, collisions as `chart (2).png`; `sha256` in
  JSON for dedupe.
- **(b)** Content-addressed: `<sha256>`, the name only in JSON.

**Recommendation: (a)** — D1 opens the folder in the OS, and hashes tell the user
nothing. `stored` is the **file name only**, never an absolute path, so a restored
data root resolves on any machine ([lessons.md](lessons.md) §6). **User's decision:
(a).**

### F3. Existing attachments and images as inputs

- **(a)** Staged from the chat's snapshots: an attachment as UTF-8 text (an
  extracted `report.pdf` as `report.pdf.txt`), an image as its prepared png/jpeg.
- **(b)** Copied into the store on first use, staged from there.
- **(c)** Not staged — only `ChatFile`s.

**Recommendation: (a).** D3 names both; no migration; one write per staged call.
The listing says the limits (extracted text, downscaled image); originals come in
through F8. **User's decision: (a).**

### F4. Output collection rules

- **Where:** (a) regular files directly in `/w/out/`; (b) any new file under `/w`.
  **Recommendation: (a)** — one sentence in the description, no scratch clutter; a
  subdirectory is skipped and named ("write files directly into /w/out").
- **Links and special files:** `symlink_metadata`, never followed; anything but a
  regular file is skipped and named. Inputs are copies, so even a hard link inside
  `/w` reaches only a copy.
- **Caps:** 10 files, 25 MB each, 50 MB per call; read at most cap + 1 bytes;
  extras skipped and said out loud, as MCP's image cap is.
- **Names:** basename only; control characters and `<>:"/\|?*` replaced; `.`,
  `..`, empty refused; device names (`CON`, `NUL`, `COM1`…) prefixed; trailing dots
  and spaces trimmed; ≤ 120 characters keeping the extension; split on **both**
  separators.
- **The same name again:** (a) versioned (`chart (2).png`), compared
  case-insensitively, identical bytes a no-op; (b) replace.
  **Recommendation: (a)** — replacing a file the user opened loses work silently;
  `/file remove` bounds clutter.
- **Failed runs:** (a) collect after any exit code, nothing after a timeout
  (truncated writes), saying so; (b) exit 0 only. **Recommendation: (a)** — a
  script that saved its chart and then failed on a `print` still made the chart.

**User's decision: as recommended**, the per-call caps included.

### F5. What each output becomes

- **Images:** stored as written; when the switch is on and the model can see,
  prepared into `ToolOutcome.images`, at most 4, extras named.
  - Switch: (a) reuse `tools.mcp_images`; (b) new `tools.python_images`, **on**,
    in the Python group; (c) none. **Recommendation: (b)** — sharing MCP's would
    make "stop paying for my charts" also cut a screenshot server; the hazards
    differ (own drawing by default, third-party pixels only with network on, spec
    §13.4).
  - Vision: **Recommendation:** the orchestrator drops tool images on
    `VisionSupport::Unsupported` and says so — fixing MCP in the same place — and
    sends on `Unknown`, as `/image attach` does.
  - The cap leaves `shared/mcp.rs` for one constant both producers read.
- **SVG:** (a) stored only; the description says "save as PNG to see it"; (b)
  rasterized with `resvg` (a dependency with its own font story).
  **Recommendation: (a).**
- **Text-like** (csv, json, md, txt, py): (a) stored, size listed; (b) also a text
  attachment via `decide_mode` (`fetch_url` precedent); (c) stored, plus a ~1 KB
  head excerpt in the result. **Recommendation: (c)** — an attachment per output
  puts every CSV into every later request's pinned block and re-prefills the
  prefix whenever the set changes (spec §6.6); `/w/in` gives the rest back.
- **Everything else** (xlsx, pdf, zip): stored, size and MIME listed.

**User's decision: as recommended.**

### F6. How the model is told

- **Description:** `/w/in` holds read-only copies, `/w/out` is collected, a PNG
  there is shown, matplotlib named, the caps, variables still do not survive.
- **Result:** after the console block, a localized section — `#N`, name, size,
  MIME (and "shown below" for an image) per stored file, the reason per skipped
  one; `present::parse_console` learns it so the feed card lists paths (D1).
- **Which files exist:** (a) a "chat files" section in the pinned system block,
  only while files exist and `python_exec` is offered; (b) a list in every result;
  (c) `os.listdir('/w/in')`. **Recommendation: (a)** — handles are needed before
  the first call; the block changes only with the set, as attachments do.

**User's decision: as recommended.**

### F7. Which files are staged

- **(a)** Every chat file, every call.
- **(b)** Only those named in an optional `files: ["#3", "sales.xlsx"]`; an
  unknown handle fails with the valid ones listed.
- **(c)** Every file unless `files` narrows it.

Links are out: a write-through hard link would change the store, and `wasmer`
7.2.0 has no read-only volume.

**Recommendation: (b).** Copies are per call (five 20 MB workbooks = 100 MB
before every `print`); with network on, (a) exposes every file to any call —
including one a fetched page talked the model into — invisibly, while a named
argument shows in the popup and the card for free. The risk, a small model not
naming its file, is what stage 1 measures; a no-go switches to (c). **User's
decision: (b), (c) on a stage-1 no-go.**

### F8. Binary files from disk

- **(a)** `/file attach` keeps the original when its text is not the file: a
  binary (xlsx) becomes a `ChatFile` instead of a refusal; an extracted
  pdf/docx/html becomes the attachment **and** a linked `ChatFile`; plain text
  stays an attachment.
- **(b)** A new `/file add <path>` always storing; `/file attach` unchanged.
- **(c)** `/file attach` stores every original, text included.

**Recommendation: (a)** — one command, nothing changes meaning, and the refusal it
lifts is D3's. Text needs no second copy (F3). The 32 MB ceiling of a single
attach covers both halves. **User's decision: (a).**

### F9. Opening a file in the OS

- **Name:** (a) `/file open <#N|name>` + `/file folder`; (b) `/open #N`; (c)
  `/file open` bare meaning the folder. **Recommendation: (a)** — `/file` owns the
  chat's files, handles are `resolve_handle`'s, and an argument's absence should
  not change the target.
- **Mechanism:** (a) own `shared/os_open.rs` — `ShellExecuteW` (adds
  `Win32_UI_Shell`), `xdg-open`, macOS `open`; detached, one argument, no shell;
  (b) the `opener` crate; (c) `cmd /c start`. **Recommendation: (a)** — ~40 lines
  against a dependency; (c) re-parses arguments outside Rust's escaping (lessons
  §6).
- **What may open:** a written `run.bat`, `.lnk` or scripted `.html` would *run*
  under a default handler. **Recommendation:** an allowlist of document types
  (images, pdf, csv, xlsx, docx, txt, md, json) opens directly; anything else opens
  the folder, and the note says why. The path is always printed, so a failed launch
  (no `xdg-open`) is one copy away from working.

**User's decision: as recommended.**

### F10. Lifecycle

- **Soft-deleted chat:** (a) the folder stays, like `workspace/`, so a restore
  brings files back; (b) deleted on hide. **Recommendation: (a)**, recording "no
  purge of a hidden chat's directories" as a gap shared with `workspace/`.
- **Backups:** (a) `files` joins `TOP_DIRS`; (b) left out; (c) included with
  `--no-files`. **Recommendation: (a)** — outputs cannot be recomputed.
  `workspace/` is missing from that list too — flagged as a separate fix.
- **`/file remove` on a stored file:** (a) drops the listing and deletes our copy
  (never the user's original; the note says which); (b) listing only.
  **Recommendation: (a)** — as removing an attachment drops its text; an unlisted
  copy is a leak.
- **Quota:** proposed `files.max_chat_mb` = 200, a write past it refused.
  **User's decision: no quota** — nothing a call produced may be refused for the
  chat's size. Growth is bounded per call (F4) and by `/file remove`; the store and
  the backups grow with what the user keeps. Opening a chat still sweeps files it
  does not list (a run cancelled between write and landing), since those were never
  anyone's.

**User's decision: (a), (a), (a), and no quota.**

### F11. Local mode

- **(a)** Wasmer only; Local's description says files are unsupported.
- **(b)** Parity: Local runs in a temp job directory with `in/`/`out/`, the same
  staging and collection; the wrapper `chdir`s there in both modes, so relative
  `in/`/`out/` work everywhere.
- **(c)** Outputs only.

**Recommendation: (b), last.** Staging and collection are host-side and shared,
and ADR 0005 §3 promises one schema for both modes; a Local refusal of `files`
breaks it. Local stays unisolated, as described. **User's decision: (b), last.**

### F12. Sub-agents and background runs

- **Store:** the parent chat's, along `AddAttachment`'s routes, mirrored into the
  run's snapshot ([ADR 0010](decisions/0010-subagent-nested-turn.md)).
- **No confirmation in background runs:** (a) the foreground contract — the
  profile's tool set is the control, as the user decided
  ([research/background-subagents.md](research/background-subagents.md) F3);
  (b) inputs refused there, outputs allowed; (c) no `python_exec` there.
  **Recommendation: (a)** — such a run can already read every attachment's text
  and hand it to networked code; inputs add binaries, not a new kind of reach. (b)
  is the conservative fallback.

**User's decision: as recommended.**

### F13. The confirmation popup

- **(a)** Nothing new — under F7(b) `files` is already shown.
- **(b)** A resolved line: names and sizes going in (not `#3`), and network on/off.
- **(c)** (b) plus the files *not* going in.

**Recommendation: (b)** — the popup is where exfiltration is consented to; `#3`
means nothing mid-decision, and network is half the decision. The orchestrator,
owning the list, puts the line on `ToolConfirm`. **User's decision: (b).**

## 6. Risks and invariants

### 6.1. Prerequisite (done): `site-packages` was writable

Measured 2026-09-11: `/sp` was a plain `--volume`, and `wasmer` 7.2.0 has no
read-only volume (`HOST:GUEST:ro` is rejected, the source has no such mode). One
call wrote `/sp/sitecustomize.py`; the next clean call executed it. With this track
that would have been an implant: code persisted into every later call, in every
chat and profile, seeing whatever F7 stages and, with network on, sending it
anywhere.

Stage 0 (#518) packs CPython and `site-packages` into one self-contained image at
`sandbox setup` (`packed-sandbox.webc`: the unpacked `python.webc` with `"/sp" =
"../site-packages"` added to its `[fs]`) and runs only that image. Writes land in
memory: the injected file did not survive, the host directory stayed clean, and it
runs offline on a fresh cache. A first probe — a second package depending on
`python/python`, run with `--include-webc` — failed offline on a fresh cache,
because dependency resolution queries the registry; hence one package. ADR 0005 §5
is amended.

### 6.2. Invariants

- **One host-writable directory per call, dropped after.** Inputs are copies;
  nothing in `in/` is collected.
- **Collection after exit**, by `symlink_metadata`, regular files only,
  size-checked while reading.
- **Stored paths are file names**; absolute paths are computed, and any name from
  the chat is re-checked to resolve under its folder (`code.rs` confinement).
- **Only F9's allowlist reaches a default handler.**
- **Exfiltration needs three visible things:** `python_exec` on (off by default),
  network on, and the file named in a call the popup shows (F7, F13); background
  runs are the documented exception (F12).
- **Image tokens are opt-out and capped** (F5).
- **Sizes bounded per call** (F4) and per attach (32 MB). A chat's store has no
  cap (F10), so it and the backups grow with what the user keeps — the feed note
  and `/file list` show sizes, so the growth is visible.
- **Cancellation leaves no listing** — `Esc` or a timeout drops the job; an
  unlanded file is swept.
- **Privacy:** no new outbound address; [PRIVACY.md](../PRIVACY.md) lists the
  store as local data.

## 7. Stages

One branch and PR each (AGENTS.md §2); docs land with the stage that makes them
true (AGENTS.md §4).

- **Stage 0 — read-only `site-packages`** — merged (#518): `sandbox setup` packs
  CPython and `site-packages` into one image and starts it once; the runtime runs
  only the image, and a provisioned directory with no image is refused with the
  command that packs it.
- **Stage 1 — MVP probe** (`spike/sandbox-files-probe`, throwaway): F4 collection,
  images into `ToolOutcome.images`, bytes into a per-chat folder, the result
  listing, `files` staging from attachments, matplotlib named. **Go/no-go**, 5 runs
  per live-gate family (Qwen 3.6 27B, Gemma 4 31B, each with its vision projector)
  of "`sales.csv` is attached: chart the monthly totals and tell me which month is
  highest": (1) a valid PNG lands in the folder; (2) the model sees the chart — as
  drafted, "names the right month and quotes the title the code set", which measured
  nothing (§10), replaced by a plotting-area colour only the image carries, against a
  blind arm; (3) the call named `sales.csv` in `files` unaided. **Go at ≥3/5 on each
  criterion.** No-go on (3) alone → F7 becomes (c), rerun; on (2) with a seeing model
  → images to the model deferred, files still ship; on (1) → redesign the output
  contract first. **GO on both families (§10).**
- **Stage 2 — outputs** (`feat/sandbox-files-out`): `ChatFile`/`Chat.files`, the
  store (naming, write, sweep), `AddChatFile` apply + mirror, the job contract and
  collection, images with `tools.python_images`, the shared cap and the vision gate
  (MCP included), result section and feed card with paths, the description,
  `/file list`/`remove` over files, `files` in backups; sub-decisions and implementation notes in §11. Docs: spec
  §9.7/§9.10/§13.2, architecture §7/§8, ADR 0005 §5 amended ("no host directories"
  → "one per-call job directory: copies in, collected out"), CHANGELOG, journal.
- **Stage 3 — inputs** (`feat/sandbox-files-in`): `/w/in` staging, `files`, the
  system-block section, the popup line, binary `/file attach`, sub-agent and
  background routing; sub-decisions in §12. Docs: spec §9.7/§9.8/§13.2, architecture
  §8, ADR 0005 §3 and §5 amended, CHANGELOG, journal. **Done**, live GO (§12).
- **Stage 4 — opening** (`feat/file-open`): `/file open`, `/file folder`,
  `shared/os_open.rs`, the allowlist; sub-decisions in §13. Docs: spec §9.7, README,
  architecture §3, CHANGELOG, journal. No model run — the gate is manual (§8). **Done**,
  gate in §13.
- **Stage 5 — Local parity** (`feat/sandbox-files-local`): `LocalSandbox` behind the same
  `SandboxRunner`, a job directory with `in/`/`out/`, the schema and the block in both
  modes; sub-decisions in §14. Docs: spec §9.3/§13.2, ADR 0005 §3 amended again,
  architecture §3/§8, CHANGELOG, journal.
- **Close:** this file to `docs/history/`; roadmap item and CLAUDE.md map updated.

## 8. Tests and live runs

| Stage | Unit tests beside the code | `#[ignore]` smokes / live gate |
|---|---|---|
| 0 | the launch plan runs the image and mounts no `/sp`; an unpacked directory is refused with the command; the image manifest gains the volume | two-call `sitecustomize` regression; setup packs and starts the image; all sandbox smokes green |
| 1 | — | the go/no-go, both families, recorded here |
| 2 | symlink (unix; Windows with privilege), subdirectory, caps said out loud, timeout collects nothing, non-zero exit collects; sanitizer table (device names, both separators, `..`, control chars, long names); case-insensitive versioning; same bytes no-op; `ChatFile` additive round-trip; apply + mirror; vision gate; backup lists `files/`; a Windows-shaped name read on Linux; `MockSandbox` files; keys in `en`/`ru` | stage 1's scenario on production code; `python_images` off → the result says so |
| 3 | staging of each kind; unknown handle lists valid ones; shared name refused; binary attach stores, pdf stores both linked, text single; remove deletes our copy only; sub-agent file lands in parent; popup line | xlsx from disk → pandas → right totals; turn 1 writes `out/clean.csv`, turn 2 names and uses it |
| 4 | handle resolution; allowlist per extension; per-platform argv (pure) | no model run — manual open of a file and a folder on Windows and Linux, noted in the PR |
| 5 | Local staging/collection with a mock interpreter path | `runs_real_python_local` variant writing `out/` |

Every stage: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`, the repository gates, sandbox smokes with `MINDFORK_SANDBOX_DIR`.

## 9. Decision checklist

| Fork | Question | Decided (2026-09-11) |
|---|---|---|
| F1 | Store entity | (a) new `ChatFile` in `Chat.files`, bytes in `data/files/<chat-id>/` |
| F2 | Names on disk | (a) readable, `name (2).ext` on collision, file name only in JSON |
| F3 | Attachments/images as inputs | (a) staged from stored snapshots |
| F4 | Collection | regular files directly in `/w/out`, no links, 10 / 25 MB / 50 MB per call, sanitized, versioned, any exit but not a timeout |
| F5 | What outputs become | images stored + shown via new `tools.python_images` (on), vision gate, shared cap 4; SVG stored only; text stored + head excerpt; rest stored |
| F6 | Telling the model | description + result section + "chat files" in the system block |
| F7 | Staging | (b) named in an optional `files` argument; (c) on a probe no-go |
| F8 | Binary from disk | (a) `/file attach` keeps the original when the text is not the file |
| F9 | Opening | `/file open` + `/file folder`, own `os_open.rs`, document allowlist |
| F10 | Lifecycle | kept on soft delete; in backups; remove deletes our copy; **no per-chat quota** (changed from the recommended 200 MB) |
| F11 | Local mode | (b) parity, last stage |
| F12 | Sub-agents/background | parent's store; foreground contract in background runs |
| F13 | Popup | (b) resolved names and sizes + network state |

## 10. Stage 1 — the probe (2026-09-11)

Branch `spike/sandbox-files-probe` (throwaway, pushed for reference): `python_exec`
takes `files` (chat attachments copied into `/w/in`), collects regular files from
`/w/out` into `data/files/<chat-id>/` with `name (N).ext` versioning, and returns
PNG/JPEG outputs as tool images. `sandbox_files_probe_e2e_live` drives the scenario
through the real orchestrator and a live model; `MINDFORK_PROBE_NO_IMAGES` withholds
the image — the blind control arm.

**Setup.** Two families, each with its vision projector (`/props` `vision: true`), on
llama.cpp b10807: Qwen 3.6 27B Q4_K_M at `-np 4 -c 16384`, Gemma 4 31B q4_0 at one slot
of 16384. The packed sandbox of stage 0; `sales.csv`, two years of daily sales
(10 976 bytes), attached **by reference**, so no number is in the prompt, with a
different month lifted to the highest total each run; tools `python_exec`,
`attachment_read`, `attachment_search`; network off; `max_tokens` 4096.

**v1 — criterion (2) as drafted measured nothing** (run on Qwen). Every batch stored its PNG and
named `sales.csv` unaided, and the blind arm named the right month as often as the
seeing one. The raw trials say why: every run's code computed and printed the peak
month ("to answer accurately"), although the request asked it not to, so the answer
was on stdout for both arms. The draft's other half of (2), quoting the chart's title,
could not have done better — the title is in the model's own code. And the blind
replies said "Looking at the chart…" about a chart they were never shown.

**v2 — a property only the image carries.** The probe appends `axes.facecolor: yellow`
to the run's `matplotlibrc` and, after the chart turn, asks with no tools: "what colour
is the background of the plotting area in the chart you just made?" Neither the code nor
its output names the colour; a run whose code set a face colour or a style is flagged.

| Batch (5 runs) | (1) PNG stored | (3) named `sales.csv` | (2, v1) month in the reply | (2, v2) plotting-area colour |
|---|---|---|---|---|
| Qwen v1, image shown | 5/5 | 5/5 | 5/5 | — |
| Qwen v1, blind | 5/5 | 5/5 | 5/5 | — |
| Qwen v2, image shown | 5/5 | 5/5 | 5/5 | **4/5**, the fifth flagged |
| Qwen v2, blind | 5/5 | 5/5 | 5/5 | **0/5** |
| Gemma v2, image shown | 5/5 | 5/5 | 5/5 | **5/5** |
| Gemma v2, blind | 5/5 | 5/5 | 5/5 | **0/5** |

- **The flagged run is the feature working** (Qwen). Its first chart came out with hairline
  bars (a datetime x-axis at the default bar width) on the yellow background; the model
  looked, rewrote the chart with a real bar width and an explicit white background, and
  the second PNG was stored beside the first as `sales_by_month (2).png`. "White" was the
  right answer about the chart it ended with. Both PNGs were inspected by eye, as were
  the yellow ones of the other runs.
- **A failed script still made the chart, and the chart carried the answer** (Gemma).
  One run's code raised `AttributeError` on the line building the peak month's label —
  after `savefig`, before anything was printed. The PNG was collected all the same (F4:
  any exit code but a timeout), and the model named 2024-11 from the image: the one run
  of Gemma's ten in which the month never reached stdout. That chart and one other of
  Gemma's were inspected by eye, both yellow.
- **Blind, the model does not say that it cannot see.** Qwen: three answered "white",
  one "based on the code… white (default matplotlib setting)", one came back empty (the
  stack's known Qwen flake), and v1's replies said "Looking at the chart…". Gemma never
  claimed to have looked, yet answered "white" five times of five, once as a deduction
  from its code. So a requirement for stage 2: when an image is not shown — the switch
  off, a model without vision — the result has to say so in words (lessons §4), or the
  model describes a chart it has not seen.
- **Naming the file was never the difficulty** with a by-reference file — 30 of 30
  across both families — so F7(b) stands. A small file attached inline, whose numbers a
  model could paste into its code instead, was not tried.
- Time: Qwen 2.6 and 2.7 minutes for the v1 batches, 6.9 and 4.4 for v2's two turns a
  run; Gemma 2.9 and 2.1 for v2.

**Verdict: GO on both families** on (1), (2) and (3) — the colour named from the image
in 4/5 runs on Qwen (the fifth redrew its chart after looking) and 5/5 on Gemma, against
0/5 blind on both. Stage 2 goes ahead with F7(b) and images shown to the model.

## 11. Stage 2 — sub-decisions (2026-09-11)

Taken from the code survey before implementing, and recorded so the stage can be read
back against them (lessons §3).

- **S1 — the job contract grows by its output only.** `SandboxRunner::run` keeps its
  signature; `SandboxOutput` gains the collected `files` and the `skipped` outputs with a
  reason each. Inputs, and with them the signature, change in stage 3 — changing it now
  would mean changing it twice.
- **S2 — collection order and caps.** The entries of `out/` are taken in name order, so
  which ones a cap keeps does not depend on the file system. An entry that is not a
  regular file by `symlink_metadata` is skipped — a directory said as one, with "write
  files directly into /w/out". A file over 25 MB is skipped after reading at most
  25 MB + 1 bytes. Past 10 files, and for a file that would take the call over 50 MB, the
  output is skipped, and later ones are still tried against the total. After a timeout
  nothing is read, and what `out/` held is named as not kept.
- **S3 — one name.** `ChatFile.name` is the file's name in the chat's folder: sanitized,
  versioned on a collision. It is the handle and the name on disk at once, so F1's
  separate `stored` field is dropped — two versions sharing a display name would make
  `/file remove chart.png` refuse every time.
- **S4 — the sanitizer is pure and lives with the entity** (`entities/chat_file.rs`):
  F4's rules; a device name is matched on the stem, case-insensitively (`con.txt` is
  reserved on Windows too); the 120-character cap keeps an extension of up to 16
  characters; a name that is not UTF-8 arrives lossy and stays so.
- **S5 — the tool writes, and versions against the listing and the disk.** Bytes go to
  `ToolContext.files_dir` (`data/files/<chat-id>/`; a sub-agent's or a background run's
  context is a clone of its parent turn's, so their files land in the parent's folder;
  `None` for silent tasks) with `create_new`, so an existing file is never overwritten. A
  listed name with the same SHA-256 is a no-op, said as "unchanged"; a listed name with
  other bytes, or a file already on disk, moves on to ` (2)`, ` (3)`… The context's
  `files` snapshot is mirrored from `AddChatFile` every round, as attachments are, so a
  turn's second call versions against its first.
- **S6 — no sweep: unlisted files are adopted at startup** (replaces F10's "opening a chat
  still sweeps files it does not list"). Chat saves are debounced: a crash after a call
  wrote its chart and before the chat was saved leaves a file the user may already have
  seen on the card, and a sweep would delete it — against "nothing may be lost". A
  background run's files are on disk before the run lands, so a sweep on activation would
  race it as well. At startup no run is in flight: every regular file in a loaded chat's
  folder that the chat does not list is listed, origin `Recovered`. A listed file missing
  from disk stays listed, and `/file list` marks it.
- **S7 — landing.** `ChatEffect::AddChatFile` is applied by `apply_effects` (a name
  already listed is skipped, so a landing is idempotent), routed to the parent from a
  sub-agent as `AddAttachment` is, and handled by a background run's landing, whose
  `if let` becomes a `match`. One feed note per landing names the files and the folder.
  No status-bar chip: the attachment chip is a price, and a stored file costs no tokens.
- **S8 — images.** An output whose bytes — not its extension — are PNG, JPEG, GIF, WebP or
  BMP is returned to the model when `tools.python_images` is on (default on, in the Python
  group, Wasmer only), at most `MAX_TOOL_RESULT_IMAGES` = 4 per call; the constant moves
  from `shared/mcp.rs` to `shared/config.rs`, beside the image limits, and both producers
  read it. The switch lives on the tool (the registry is rebuilt on any `tools` change),
  so the description and the behaviour read one value. In `record_call` the orchestrator
  drops a result's images when the engine reports `VisionSupport::Unsupported`, and says
  so in the result; it says so too when `prepare_tool_images` drops one (over
  `images.max_bytes`, undecodable) — both silent until now, for MCP as well. Every "not
  shown" names its reason and that the model has not seen the image (§10).
- **S9 — the result section.** After the console block, a `files:` section — the label
  universal, like `stdout:`; its lines localized (axis A). Its first line names the chat's
  folder by absolute path: where the call saved, so the model can tell the user and the
  card can show it without the feed knowing the data root; the index keeps file names
  only (F2). Then one line per output: name, size, MIME, shown or not and why, versioned
  or unchanged; a text-like output (csv, tsv, json, md, txt, py, xml, html, yaml, log)
  gets its head, up to 1 KB, indented. `parse_console` keeps the section as a block of its
  own (a result of that section alone parses too), and the card draws it under the
  console.
- **S10 — the description** gains a paragraph built from the tool's config: write
  directly into `/w/out`; the caps; the files are saved to the chat for the user, and a
  later call cannot read them yet (stage 3 rewrites that clause); with the switch on, a
  PNG or JPEG is shown after the call, and an SVG is only saved. Local's description stays
  as it is until stage 5.
- **S11 — `/file list` and `/file remove`.** One numbered list: attachments first, then
  stored files, `#N` continuing; a name shared across the two refuses, as it does today.
  Removing a stored file deletes our copy first and drops the listing second: a failed
  delete keeps the listing and says why — retryable, with nothing lost (lessons §8, the
  order of two writes).
- **S12 — backups:** `files` joins `TOP_DIRS`, and a restore replaces it with the rest.
  `workspace/` stays the separate fix (F10).
- **S13 — the live seam:** the orchestrator smoke points a temporary data root at a
  provisioned sandbox through a test-only `Paths` override, instead of the probe's
  environment read inside `build_registry`.

**Implementation notes (2026-09-12)**, read back against S1–S13 — two things the
decisions did not name:

- **S7's note is for the open chat.** A landing in a chat that is not the open one
  lists its files without a feed note: the call's card, or the run's transcript,
  already shows them, and a note in another chat's feed would name files the user
  cannot see there.
- **A guest link never reaches the host** (measured under wasmer 7.2.0 on Windows).
  `os.symlink` and `os.link` into `/w/out` succeed in the guest, which reads through
  the symlink, while the host `out/` stays empty — the links live in wasmer's own
  filesystem layer. S2's `symlink_metadata` check is defence in depth for another
  host or version, and the smoke asserts the property (nothing a link names is kept)
  rather than that mechanism.

## 12. Stage 3 — sub-decisions (2026-09-12)

Taken from the code survey before implementing, and recorded so the stage can be read
back against them (lessons §3), as §11 was for stage 2. **T4 is the user's**, asked
because the survey found a decision (D3) resting on something that does not exist.

- **T1 — the job contract changes once.** `SandboxRunner::run` takes a `SandboxJob`
  (`code`, `inputs`, `net`, `timeout`) instead of its four arguments — the change S1
  deferred to this stage, made once. The collection limits stay `OutputLimits::DEFAULT`
  rather than becoming a field of the job as §4 sketched: nothing would ever set them
  differently, and the tool's description is built from the same constant, so a field
  would be a second source of truth for a value that has one. `MockSandbox` records the
  staged inputs beside the code, so a test can assert what went in; provisioning's warmup
  and verify pass a job with none.
- **T2 — one list of what a call can name.** Attachments, then stored files, then the
  chat's images: one numbering — the one `/file list` already uses (§11 S11), extended by
  the images — so `#N` means the same thing in `/file list`, in the system block and in
  `files`. A pure `features::chat_inputs` builds that list once and its four consumers
  read it: the block, the argument's resolver, the popup line and the staging itself.
  They cannot disagree about what `#3` is, which is the property the popup exists for.
  `/file remove` on an image handle refuses and says why — an image belongs to the
  message that carries it, and `/image remove` is about images not yet sent.
- **T3 — the guest name is decided with the list, not during staging.** The model writes
  `/w/in/<name>` into its code *before* any result exists, so a name chosen while staging
  could never reach it. Each item's staged name is therefore computed with the list and
  shown in the block: sanitized (`sanitize_name`), and made unique across the whole list
  case-insensitively with `versioned` — two `notes.md` from two folders are `notes.md` and
  `notes (2).md`. An attachment is staged as its **extracted text**, so it gains `.txt`
  exactly where the text is not the file — a name whose extension the extractors claim
  (pdf, docx, html/htm) or no extension at all; `main.rs` and `notes.md` keep their names.
  An image is staged under its prepared format's extension.
- **T4 — the chat's images are in the list** (user's decision, 2026-09-12). D3 named them,
  and the survey found they have no handle: `/image list` numbers only what is staged for
  the *next* message, and no snapshot of a chat's images reaches a tool. They now join the
  numbering above, `/file list` grows an images tail so the user sees what the model sees,
  and what is staged is the **prepared** PNG/JPEG — the downscaled bytes the model was
  shown, not the original, which comes in through F8 when the user attaches it. The
  snapshot (`Arc<[MessageImage]>`, base64 payload included) is built **only when the turn
  offers `python_exec` in Wasmer mode**: with the tool off, which is the default, a chat's
  images are not copied into a context that has no use for them.
- **T5 — the system block** (`inject_files`, after the attachments and before the
  workspace, by the same volatility order): the numbering, each item's staged name, size
  and type, and the two sentences that close the door — the copies are under `/w/in`, and
  `/w/out` is the way anything comes back. It exists only while the chat has something
  stageable **and** the turn offers `python_exec` in Wasmer mode: a block naming a tool the
  turn does not have is this project's most-repeated defect (lessons §4), and the converse
  is just as strong — a tool the block does not name goes unused.
- **T6 — the popup says what is going in, because it cannot say it otherwise.** The
  confirmation popup presents arguments in the compact view, which drops arrays outright
  (`present::scalar_str` returns `None` for one), so `files: ["#3"]` would not appear at
  all. F13(b) is therefore not a nicety: `ToolConfirmRequest` carries the **resolved**
  inputs — each named item's staged name and size, each handle that resolves to nothing,
  and the sandbox's network state — and the screen renders them in the interface language
  (axis B), as it renders every other note. The orchestrator resolves them with T2's
  function, the one the call itself uses.
- **T7 — an unresolved handle runs nothing.** An unknown handle, a name two items share,
  or a stored file the chat lists that its folder no longer holds: the call is refused
  before the sandbox starts, the reason names the item, and the valid handles are listed.
  Staging the rest would be worse than refusing — a script that asked for four files and
  got three answers confidently from three (lessons §4).
- **T8 — no cap of its own on what goes in.** Growth is already bounded per attach
  (32 MB), per collected output (25 MB) and by the chat's own store; the copies live in the
  job directory and die with it. A cap here would be the one place where naming your own
  file fails, which is what F10 refused for the store. The popup line shows the sizes
  before the copy is made, and the description says copies are per call.
- **T9 — `/file attach` keeps the original (F8(a)), and the pair is one item.** The
  blocking read now ends in one of three outcomes: plain text — an attachment, as today;
  an extractor's text (pdf, docx, html) — an attachment **and** the original stored,
  linked by a new additive `Attachment.file_id`; bytes that decode as nothing — stored
  only, with no attachment, which is the refusal D3 asked to lift. A linked pair is one
  item in `/file list`, in the numbering and in `files`, or the very name the user typed
  would resolve to two items and be refused by our own shared-name rule. Removing it
  deletes our copy first and drops both listings second (§11 S11's order). Re-attaching
  replaces it: identical bytes are `Unchanged` and nothing moves; different bytes are
  stored first, the listing swapped, our old copy deleted last — a failed delete leaves an
  unlisted file, which startup adopts (§11 S6) rather than loses. A binary's feed note says
  what can read it, and says so whether or not `python_exec` is currently on.
- **T10 — the description, and Local until stage 5.** The Wasmer description gains `/w/in`
  and replaces S10's clause "a later call cannot read what an earlier one saved" with the
  route that now exists: name it in `files`. Local's schema stays `{ code }` until F11's
  parity lands in stage 5 — a model in Local mode is never offered an argument its mode
  cannot honour — and ADR 0005 §3's "the schema is mode-independent" is amended with that
  divergence and the stage that ends it.
- **T11 — copies, not a read-only mount.** `/w/in` sits inside the one job directory,
  which `wasmer` 7.2.0 mounts writable because it has no read-only volume at all (§6.1).
  The guest can overwrite a staged copy; nothing follows from that, since the chat's store
  is untouched and only `/w/out` is collected. So the description says **copies**, not
  "read-only": a promise the sandbox cannot enforce is the wrong promise to make, and the
  true sentence is just as short.
- **T12 — staging is host-side, and `shared` stays chat-blind.** An input is either bytes
  or a path to copy: a stored file and an attached original are copied from the chat's
  folder (`tokio::fs::copy`, no read into memory), while an attachment's text and an
  image's base64 are decoded by the **tool** and handed over as bytes. `shared::sandbox`
  writes what it is given into `in/` and knows nothing about chats, as it knows nothing
  about them on the way out.
- **T13 — sub-agents and background runs inherit the turn's list** (F12), because their
  context is a clone of the parent's and their request is built from the parent's
  environment: the same items, the same folder, the same block. A background run still
  never asks for confirmation — the foreground contract, as decided.
- **T14 — tests.** Unit: the item list and its staged names (both separators, a shared
  name, an extractor's `.txt`, an image); an unknown handle lists the valid ones; a shared
  name and a missing stored file refuse with nothing run; the popup's resolved inputs; the
  block absent without `python_exec` and without Wasmer mode; binary attach stores and
  lists, a pdf stores both linked, plain text stores nothing extra; removing a pair deletes
  one copy; a sub-agent stages from the parent's list; `MockSandbox` sees the staged
  inputs. Live: an xlsx attached from disk, read with pandas, totals matching; and turn 1
  writing `out/clean.csv`, turn 2 naming it in `files` and using it — D4's persistence,
  end to end.

**Live — GO (2026-09-12)**, Gemma 4 31B q4_0 with its projector on llama.cpp b10807, one
slot, against the packed sandbox of stage 0. Two smokes, both written so the answer cannot
arrive through another channel (§10's lesson):

- **`sandbox_inputs_e2e_live`** — turn 1 built an Excel workbook with openpyxl from numbers
  the prompt defines but tells it not to print, and saved it to `/w/out`: 4.9 KB stored in
  the chat's folder as `sales.xlsx`. Turn 2 named that workbook in `files`, read the copy in
  `/w/in` with pandas and printed **4706** — which is Σ(i²·7+13) over twelve months — and
  the reply carried that number. A workbook rather than a CSV on purpose: its bytes have to
  survive storing and staging unchanged or openpyxl cannot open them at all. D4's
  persistence, end to end, in 19 s.
- **`attached_binary_reaches_the_sandbox_live`** — a generated 512×512 PNG attached from
  disk was **kept** rather than refused (`StoredFile`, `image/png`, no attachment made:
  fork F8a's half the model never sees), and a call that named it opened the copy with
  pillow and printed `(512, 512)`, which the reply repeated. 7 s.

The first smoke also shows the description reads as intended: the model saved to `/w/out`
in one turn and, a turn later, named the file rather than assuming the sandbox still held
it.

## 13. Stage 4 — sub-decisions (2026-09-12)

Fork F9 decided the shape — `/file open <#N|name>` + `/file folder`, our own
`os_open.rs`, an allowlist of document types — and these are the decisions the code
survey added under it, recorded before implementing as §11 and §12 were.

- **U1 — one resolver, the list of §12 T2.** `/file open` takes the handle `/file list`
  shows (`#N`, a name, a path) and resolves it through `chat_inputs::resolve`, the
  function `/file remove` and the tool's `files` already use: a name two items share is
  refused with each candidate's `#N` and source, exactly as a removal refuses it. `/file
  folder` takes no argument — an argument's absence must not silently change the target
  (F9) — and opens the chat's stored-files folder.
- **U2 — what a handle opens is decided purely, and the disk is consulted once.**
  `chat_inputs::open_path` returns the path an item means: the chat's own copy
  (`dir/<file>`) for a stored file **and** for an attached document that kept its original
  — our copy is the half that is guaranteed to be there, while the user's path may have
  moved since the attach — and the `source` for everything else, which is an attachment's
  own file or an image's. The caller then checks that path once. A source that is not a
  file opens nothing and the note prints it: a pasted image is `clipboard:<uuid>`, a
  fetched page's attachment is a URL, and a listed copy can be gone from the folder. No
  copy is written to make an open work — a command the user reads as "show me this" must
  not put a new file on their disk.
- **U3 — the allowlist decides *what* opens, never *whether* something happens.** The
  document types open directly: `png jpg jpeg gif bmp webp tif tiff pdf csv tsv txt md
  json xlsx docx`. Anything else opens the containing folder instead, and the note says
  why. Deliberately out: `svg` and `html` — both are shapes a `python_exec` call writes
  and both are scripted documents a browser executes; the macro-enabled `docm`/`xlsm`;
  and every executable shape (`bat`, `cmd`, `ps1`, `sh`, `lnk`, `exe`), which is the
  attack F9 named. The model writes into this folder, so the rule has to hold for a name
  the model chose: the fallback means a file we refuse to launch still gets the user one
  double-click away from it, with the reason said out loud.
- **U4 — the launch is ~60 lines of ours** (`shared/os_open.rs`): `ShellExecuteW` on
  Windows (windows-sys gains `Win32_UI_Shell`), `xdg-open` on Linux, `open` on macOS —
  one argument, no shell. Not `cmd /c start`, which re-parses the argument outside Rust's
  escaping (lessons §6), and not a crate for three calls. The part that can be tested
  everywhere is pure (`launcher(Platform)`, `is_document(name)`); the spawn itself is the
  stage's manual gate.
- **U5 — launching runs on the blocking pool.** `ShellExecuteW` returns only once the
  shell has started the handler, and `xdg-open` is a script that execs another; the
  orchestrator's command loop waits for neither. `spawn_blocking`, and the outcome comes
  back as a `FileProgress` event like every other file note.
- **U6 — the path is always printed**, on success and on failure (F9). A machine with no
  `xdg-open`, or no handler for `.xlsx`, then leaves the user one copy-paste from the
  file instead of one error message away from nothing.
- **U7 — `/file folder` on a chat that has stored nothing** says so and prints the path
  rather than creating the directory: the folder is made when the first file lands, and a
  command that reports on the chat's files should not be the thing that creates an empty
  directory for a chat that has none.
- **U8 — no `explorer /select,`** (rejected). Highlighting the file inside its folder
  would be nicer on Windows and means a second, argv-shaped launch path on one platform
  only — while the folder is the fallback precisely for the files we will not launch,
  where "the folder opened" is already the whole message.
- **U9 — both commands are on `FileList`'s side of the Esc back-stack**
  (`works_on_the_open_chat` = false): opening a viewer changes nothing in the
  conversation. `/file` stays blocked as a whole prefix in a sub-agent transcript, so
  neither command is reachable from one.
- **U10 — the gate is manual** (§8): no model runs anywhere in this stage. A document, a
  refused type and the folder, opened by hand on Windows and on Linux, with the result in
  the PR.

**Gate — manual, no model run (2026-09-12).** On Windows 11,
`MINDFORK_OPEN_LIVE=1 cargo test -- --ignored opens_a_document_and_a_folder_live` opened a
viewer on `mindfork open gate.txt` — a name with spaces, handed over whole — and a file
manager on the folder standing in for `run.bat`, both `ShellExecuteW` calls returning above
32. The Linux half runs in CI rather than by hand: the unix launch takes its launcher as an
argument, so a stub script records what it was given, and the test asserts the path arrives
unsplit (`my chart (1).png`) and that a launcher which is not installed comes back as an
error. What is **not** verified here is a GUI launch on a Linux desktop — the development
machine has none, and WSL carries only docker's utility distribution.

## 14. Stage 5 — sub-decisions (2026-09-12)

Fork F11 decided parity (b): Local runs in a job directory with `in/`/`out/`, the same
staging and the same collection, and the two modes stop diverging. These are the decisions
the code survey added under it, recorded before implementing as §11–§13 were.

- **V1 — Local becomes a `SandboxRunner`, not a second code path.** `LocalSandbox` in
  `shared/sandbox.rs` answers the same contract as `WasmerSandbox`: a job directory per
  call holding the script, `in/` and `out/`; the interpreter started **with that directory
  as its working directory**; the same `collect_outputs` under the same
  `OutputLimits::DEFAULT`; the same rule that any exit code collects and a timeout collects
  nothing. `python_exec` then has one path, and `PythonMode` decides only two things —
  which runner the registry builds, and the wording. That is what makes ADR 0005 §3's "one
  schema for both modes" true again instead of merely promised; the alternative, a second
  staging/collection written beside the first, is the defect that document keeps warning
  about.
- **V2 — relative paths work in both modes, and the Wasmer prompt does not change.** F11(b)
  asks for `in/`/`out/` to work everywhere, and they do: the working directory is the job
  directory in both — mounted at `/w` in the guest, the process's cwd on the host. What the
  model *reads*, though, keeps each mode's own form — `/w/in` and `/w/out` for Wasmer, `in`
  and `out` for Local — through `{in}`/`{out}` placeholders in the pinned block and the
  description. The Wasmer rendering stays byte-identical to the text stages 2 and 3
  measured; spending a live GO to make two strings look alike would buy nothing.
- **V3 — the script is a file, not `-c`.** Local passed the code as a command-line argument;
  now it is written to `job.py` beside `in/` and `out/` and the interpreter is given the
  path. The command line stops being somewhere a long script can overflow (Windows caps
  it), a traceback names a file, and both modes build the same directory. The WASIX shims
  stay Wasmer's: `setsockopt` and `MPLCONFIGDIR` are about the guest's libc and its
  FreeType build, and nothing on the host wants them.
- **V4 — Local's availability is a path check, not a probe.** An interpreter named with a
  separator (`D:\py\python.exe`) is checked as a file, so a wrong setting is reported
  before the call in the same place a missing `wasmer` is; a bare `python3`/`python` is left
  to `PATH`, where a failure surfaces as the spawn error it already was. No `--version`
  probe — a process per turn to answer a question the run answers anyway.
- **V5 — `net` stays a Wasmer word, and the popup stops implying otherwise.** Local has no
  network switch and never had one: the code runs with the user's own permissions. Its
  description says the mode is unisolated rather than naming a network state, and the
  confirmation popup's network line reads `true` in Local mode, because that is what is
  true there — `python_net` keeps meaning "the sandbox's flag".
- **V6 — each mode keeps its own timeout** (Local's 10 s, Wasmer's configured one): the
  numbers are about what the two cost to start, not about the contract they share.
- **V7 — what a unit test may reach.** The job directory's preparation is host-side and
  shared, so it is tested directly — the staged copies under their names, `out/` present
  even when nothing was staged, a name with a separator refused — and the collection
  already has stage 2's table. Spawning a *real* interpreter is the one part a unit test
  must not require, so it stays the `#[ignore]` pair, with `runs_real_python_local` grown
  into the round trip the stage is about: a file staged into `in/`, read by the code, a
  file written to `out/`, stored with the chat.
