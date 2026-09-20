# `mindfork stats` — which copy of the data is the newest

A design plan (AGENTS.md §1). Two stages; stage 1 is specified here in full,
stage 2 in outline.

## 1. The problem

The app's data is portable and travels between machines as backup archives
(spec §12.3). With the data on three or four computers two questions have no
answer today:

1. **Which copy is the newest?** Nothing prints "the last message here was
   written on …" without opening the TUI and scrolling the chat list.
2. **Does an older copy hold something the newest one lacks?** A copy that is
   not the newest can still contain changes — a chat continued on the laptop
   after the desktop's backup was taken.

The user's request (2026-09-20): a command-line argument that prints a summary
of the user data — the date of the last message, the number of chats, how many
of them are deleted (soft delete), the total number of messages, the number of
deleted messages, of attached files, of projects "and so on".

## 2. What was measured before designing

Read against the real dev data root (233 chat files, 31 MB, all at chat schema 4):

| Figure | Source | Value there |
|---|---|---|
| chats / soft-deleted | `chats/*.json`, `is_hidden` | 233 / 47 |
| messages (storage rows) | `messages[]` | 2705 |
| deleted messages / exchanges | `deleted[].messages[]` (`Ctrl+E`, `Ctrl+R`, rewrite) | 249 / 57 |
| attached files | `attachments[]` | 57 |
| stored files | `files[]` | 0 |
| images in messages | `messages[].images[]` | 0 |
| chats with a code project | `workspace` | 1 |
| sub-agent runs | `messages[].tool_calls[].subagent` | 30 |
| profiles / soft-deleted | `profiles.json`, `is_hidden` | 2 / 1 |
| notes / superseded | `data.db`: `notes`, `note_superseded` | 113 / 19 |
| note links | `note_links` | 64 |
| knowledge-base sources / chunks | `rag_sources`, `rag_documents` | 2 / 4 |

Every requested figure already exists in the stored data; nothing new has to be
recorded. A Python probe read the whole corpus in 1.3 s; the app's own full
parse of the same corpus is ~0.1 s (`JsonStore::chat_files` doc comment), and
the summary reads less than that (§4.2). The database queries took 8 ms.

Two facts from the code that shape the design:

- `data.db` runs in SQLite's default rollback-journal mode (no `journal_mode`
  pragma anywhere), so a read-only open works next to a running app.
- **Opening a SQLite file can mutate the directory** even read-only
  (docs/lessons.md §8): a non-database must be refused by its header first.
  `db::vacuum_into` already does exactly this; the summary follows it.

## 3. Forks

Confirmed by the user on 2026-09-20:

- **F1 — the name: `mindfork stats`.** *User's decision.*
- **F2 — comparison is stage 2.** Stage 1 prints the summary; telling two
  copies apart chat by chat comes second. *User's decision.*
- **F3 — a backup archive can be summarized too**, and an archive may be
  password-protected. *User's decision.*

Decided here at their recommendation, recorded so they can be challenged:

- **F4 — the archive is read with its password; the summary is not written into
  the manifest.** `manifest.json` is unencrypted by contract ("it holds no user
  data", `features/backup.rs`). The number of chats and the date of the last
  message *are* user data, so putting them there would leak them from an
  encrypted backup. `stats <archive>` therefore settles the password exactly the
  way `restore` does — `--password`, else the one stored in the settings, else
  an interactive prompt (three attempts) when stdin is a terminal — through the
  same function, so the two commands cannot drift apart.
- **F5 — nothing of an archive is written to disk.** Chats are parsed straight
  from the (decrypted) zip entries. The database is loaded into memory through
  SQLite's deserialize interface (`rusqlite`'s `serialize` feature — no new
  crate) rather than extracted to a temporary file: extracting would put a
  decrypted copy of an encrypted backup's database into the OS temp directory.
  The cost is the database's size in RAM for the duration of the command.
- **F6 — `stats` is strictly read-only.** It creates no directories, writes no
  log, takes no single-instance lock and runs no migration — it branches off
  before all of that, the way `demo` does. So it is safe next to a running app,
  on a read-only medium, and on a machine whose binary was updated but never
  started (data still at an older schema).
- **F7 — a tolerant reader instead of the domain types.** The chat files are
  read through a projection that names only the counted fields, every one
  defaulted, the rest skipped without allocation (`serde::de::IgnoredAny` —
  which also skips the base64 image payloads). Of the chat schema steps only
  1→2 touches a counted thing — it *synthesizes* sub-agent transcripts — so an
  unmigrated file reports fewer sub-agent runs and everything else identically;
  a file from a *newer* schema is still counted and the output says the numbers
  may be incomplete.
- **F8 — best effort per source.** An unreadable chat file is counted as such
  and named in the output, not fatal (the app's own startup does the same,
  release-engineering F11). A database that cannot be read leaves the chat half
  of the summary intact and says why the database half is missing.
- **F9 — times are printed in UTC.** The point of the command is laying the
  output of several machines side by side; one zone makes that a string
  comparison.
- **F10 — `--json` ships in stage 1**, carrying the aggregate figures plus one
  row per chat (id, profile, title, hidden, created/modified, message counts,
  last message time), sorted by id and pretty-printed so any `diff` tool already
  answers question 2 by hand. A `format` field versions it for stage 2.
- **Not in this track:** pointing `stats` at an arbitrary data *directory*
  (a different entry point — a follow-up if wanted), per-profile breakdowns,
  and any write (no "merge").

## 4. Stage 1

### 4.1. Surface

```
mindfork stats [ARCHIVE] [OPTIONS]

  [ARCHIVE]                  a backup archive to summarize instead of the live data
  -p, --password <PASSWORD>  password for an encrypted archive
      --json                 machine-readable output
  -h, --help
```

Exit codes: `0` — a summary was printed (an empty or missing data root is a
summary, and says so); `1` — the archive could not be read (missing, not a zip,
not a backup, no/wrong password); `2` — a usage error, as everywhere in the CLI.

### 4.2. What is counted

- **Profiles**: total, soft-deleted.
- **Chats**: total, soft-deleted, unreadable files.
- **Messages**: counted the way the chat list counts them — the bubbles the
  feed draws (`visible_message_count`, spec §11.2), not storage rows: one
  question answered through an agentic loop stores dozens of rows and shows
  two. The rule is not restated: the entity's counter is opened up to take
  `(role, new_bubble)` pairs, and both the domain type and the projection feed
  it. The total is over all chats, with the part sitting in soft-deleted chats
  named; the raw row count is printed next to it, because it moves on changes
  the bubble count hides. Sub-agent transcripts (runs nested on a tool call)
  get their own line — runs and their messages — and are not part of the
  total, as they are not part of the list's.
- **Deleted messages**: the `deleted[]` archive — messages and exchanges.
- **Attached files** (`attachments[]`), **stored files** (`files[]`),
  **images** (`messages[].images[]`).
- **Projects**: distinct attached project directories, and the number of chats
  they are attached to.
- **Last message** (the newest `messages[].timestamp`) and **last change** (the
  newest `modified_at`) — the second catches a rename or a deletion that adds
  no message.
- **Database**: notes (superseded among them), note links, the newest note
  change, knowledge-base sources and chunks, self-models.
- **Sizes**: chat files and the database.
- **Header**: what was read (the data root or the archive), this binary's
  version; for an archive — the version and time from its manifest, and a
  warning when it was made by a newer version.

### 4.3. Code map

| Piece | Where |
|---|---|
| parser, help topic | `features/cli.rs` — `CliCommand::Stats`, `HelpTopic::Stats` |
| database counts (file or in-memory image) | `shared/storage/db/stats.rs` |
| the projection, both collectors, both renderers | `features/data_stats.rs` |
| archive access (open, password, entries) | `features/backup.rs` — a small read seam next to `check_password` |
| dispatch before `ensure_dirs`, password settling | `main.rs` — `run_stats` |
| text | `locales/{en,ru}.json` — `cli.help.*stats*`, `stats.*` |

### 4.4. Verification

Unit tests next to the code: the parser; the projection against a fixture chat
holding every counted thing (and the same chat in a pre-v2 shape); the archive
path round-tripped through the real `create_backup` — plain and encrypted, with
the right, a wrong and no password; the database counts on a file and on an
in-memory image, plus a non-database and a database missing the newer tables;
"the data root is untouched" asserted on a directory listing before and after.
No engine, memory or tool is involved, so there is no live-stack smoke; the
live run is the command itself against the real dev data root and a real
encrypted backup of it, and the numbers must match §2.

### 4.5. Outcome (2026-09-20)

Built as specified; the live run is **GO** — every figure of §2 reproduced on the
real data root and, to the digit, on an AES-256 backup of it, with the root and
`logs/` untouched (docs/journal/storage.md). Three things the work changed:

- **"Messages" is the chat list's number.** The probe's 2705 are storage rows; the
  list's cards sum to 1198 bubbles. The summary reports the second and prints the
  first beside it (§4.2).
- **Two defects on the seam shared with `restore`**, both found by the live run and
  fixed for both commands: a stored password that does not fit was reported as
  "wrong backup password" to a user who typed none, and the password prompt went
  to stdout, where `--json > file` would have hidden it and corrupted the file.
- **Runs do not nest** (ADR 0010 never gives a run `call_subagent`), so the
  sub-agent count is one level deep; a recursion written for it was removed when
  its mutation survived every test.

## 5. Stage 2 — comparison (outline)

`mindfork stats --compare <OTHER>`, where `<OTHER>` is a `--json` snapshot or a
backup archive from the other machine: chats only here, only there, newer here,
newer there; the same for notes. Decided when it starts: what identifies
"newer" for a chat (message count and last message time, not `modified_at`
alone), whether notes get per-row entries in the snapshot, and whether a
one-line fingerprint of the chat set belongs in the stage 1 summary.
