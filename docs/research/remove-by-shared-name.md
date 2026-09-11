# Removing an attachment by a name two of them share

> **Status:** implemented (2026-09-11) — the user's decisions of 2026-09-11: every fork
> as recommended — F1a a shared name refused with its candidates, F2a the source shown
> where a name is shared, F3a `/file` and `/image` together. The item
> [page-attachment-name.md](page-attachment-name.md) §7 left: `/file remove <name>` acts on
> the first attachment of that name, and says nothing about which one it was.

## 1. Where a typed name is resolved

| Entry | Where | A name several items answer to |
|---|---|---|
| `attachment_read` (the model) | `tools/attachment.rs` `AttachmentRead::invoke` | reported, with each candidate's source ([fetch-url-fidelity.md](../history/fetch-url-fidelity.md) S10) |
| `/file remove <name\|path\|#N>` | `file_command::resolve_target` | **the first match, silently** |
| `/image remove <name\|path\|#N>` | `message_image::resolve_target`, documented to mirror `/file`'s | **the first match, silently** |
| `/rag remove <path>` | `Db::rag_delete_under` | not a name: a path, and a directory takes everything under it by design |

## 2. How two items come to share a name

- **`/file attach`** names an attachment by its file name and dedupes by canonical source, so
  `a/notes.md` and `b/notes.md` are two attachments called `notes.md`.
- **`/image attach`** does the same for a path, and names a downloaded image by the URL's last
  segment — `image.png` from two sites.
- **Already kept apart:** a fetched page (`unique_name` appends the URL's last segment) and a
  paste (`free_clipboard_name`: `clipboard.png`, `clipboard-2.png`).
- **Across kinds:** a tool's attachment (a page, a transcript) can still share a name with a
  user's file.

## 3. What was measured

Through the orchestrator — `AppCommand`s sent to a real `Orchestrator`, its events read back:

- **Files.** `a/notes.md` and `b/notes.md`, 13 bytes each. `/file list` answers with two
  identical lines, `#1 notes.md — 13 B, ~4 tok. (in full)` and the same for `#2`.
  `/file remove notes.md` answers "Files: attachment removed — notes.md", and the next request
  carries `b`'s text and not `a`'s: the first was removed, and nothing said which.
- **Images.** `a/chart.png` (32×16) and `b/chart.png` (64×16). `/image remove chart.png`
  answers "Images: chart.png is no longer staged"; the 64×16 one stays. `/image list` told
  them apart by their size alone.

So a typed name can be a guess, and `#N` — which never is — cannot be chosen knowingly
either, because the listing does not say which `#N` is which file when their sizes match.

## 4. Forks

**F1 — what `remove` does with a name several items answer to.**

- (a) **Refuse, and name the candidates — recommended.** Nothing is removed; the message lists
  each one's `#N` and source, so the next command is `#N` or the path. It is the rule the
  model's `attachment_read` already follows, and a removal is not undone from the chat:
  attaching again re-reads a file that may have changed since, so a guess costs more than one
  more command.
- (b) Remove every match. One command, but a user who meant one file loses both.
- (c) Keep removing the first, and name its source in the note. Still a guess — only a visible
  one.
- (d) Keep names apart when attaching, as fetched pages and pastes already are (a second
  `notes.md` becomes `notes.md — b`, after its folder). No name is ever shared, the model's
  attachment block included — but a name then depends on what else was attached first, and a
  file stops being listed under the name it has on disk.

**F2 — whether the listing tells them apart.**

- (a) **The source on a line whose name another item shares — recommended.** The ordinary
  listing stays as it is; where names collide, the path is exactly what tells the lines apart
  and what `remove` accepts. The removal note names the source in the same case.
- (b) The source on every line — a full path on each line of every chat's listing.
- (c) The listing unchanged: `#N` stays unchoosable when sizes match.

**F3 — scope.**

- (a) **`/file` and `/image` together — recommended.** `message_image::resolve_target` is
  documented to address its items the way `/file`'s does, and it has the same defect,
  measured.
- (b) `/file` only, `/image` as a follow-up.

## 5. What this does not do

- The model's attachment block keeps naming both files `notes.md` (unless F1d);
  `attachment_read` already asks for the source when that matters.
- `/rag remove` is by path; there is no name to share.
