# Sandbox file exchange — files into, out of and between `python_exec` calls

Track design plan (stages, scope, forks). Genre per [AGENTS.md](../AGENTS.md) §1;
once the track is done this file moves to `docs/history/`.

**Status:** forks decided 2026-09-11 — every one as recommended except F10, which
drops the per-chat quota (§9). **Stage 0** (a read-only `site-packages`) merged as
#518. **Stage 1** — the MVP probe with its go/no-go — is next (§7).

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
  of "`sales.csv` is attached: chart it, then tell me which month is highest and
  what the chart's title says": (1) a valid PNG lands in the folder; (2) the reply
  names the right month and quotes the title the code set — scored where the
  server has a vision projector; (3) the call named `sales.csv` in `files` unaided.
  **Go at ≥3/5 on each applicable criterion.** No-go on (3) alone → F7 becomes (c),
  rerun; on (2) with a seeing model → images to the model deferred, files still
  ship; on (1) → redesign the output contract first.
- **Stage 2 — outputs** (`feat/sandbox-files-out`): `ChatFile`/`Chat.files`, the
  store (naming, write, sweep), `AddChatFile` apply + mirror, the job contract and
  collection, images with `tools.python_images`, the shared cap and the vision gate
  (MCP included), result section and feed card with paths, the description,
  `/file list`/`remove` over files, `files` in backups. Docs: spec
  §9.7/§9.10/§13.2, architecture §7/§8, ADR 0005 §5 amended ("no host directories"
  → "one per-call job directory: copies in, collected out"), CHANGELOG, journal.
- **Stage 3 — inputs** (`feat/sandbox-files-in`): `/w/in` staging, `files`, the
  system-block section, the popup line, binary `/file attach`, sub-agent and
  background routing.
- **Stage 4 — opening** (`feat/file-open`): `/file open`, `/file folder`,
  `shared/os_open.rs`, the allowlist.
- **Stage 5 — Local parity** (`feat/sandbox-files-local`).
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
