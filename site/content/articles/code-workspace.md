+++
title = "The code workspace: containment before capability"
description = "A project attached to a chat: what the assistant can read, edit and run there, why the commands are only ever yours, how every change is a diff with a way back — and the semantic index that was measured and did not ship."
weight = 8
+++

*The eighth in a short series on how mindfork is put together. Earlier:
the [overview](/articles/mindfork-at-a-glance/),
[the engine](/articles/engine-as-a-server/),
[the self-model](/articles/self-model/),
[vector search](/articles/vector-search/),
[local speed](/articles/local-speed/),
[the Python sandbox](/articles/python-sandbox/) and
[the trust boundaries](/articles/trust-boundaries/).*

## Attaching is the permission

`/project attach <directory>` gives one chat one directory. That sentence
carries most of the design. The path is canonicalized and stored with the
chat; while it is there the assistant has a family of `code_*` tools, and
when it is not, the tools do not exist — a chat without a project sends a
request byte-identical to what the app sent before the feature was written.
There is no second switch to find, because the act of pointing at a
directory *is* the consent, and making you attach a directory and then
tick five toggles would contradict that. The profile's own per-tool
toggles still apply on top.

This is deliberately not the same thing as the general file tools. Those
are a global capability behind a switch, reaching the whole file system
unless you narrow them to a root. The workspace family is the opposite
shape: scoped to one chat and one directory you pointed at, and every path
it is handed is resolved against that root — `..`, an absolute path
outside, a symlink pointing out, all settled by canonicalization before
anything is read.

## What it can do inside

Five tools read and change the project, and one habit ties them together.

- `code_list` walks the tree the way `.gitignore` says to, skipping `.git/`
  and hidden entries, two levels deep unless asked for more.
- `code_read` returns a file with line numbers in the form `   12→text`, a
  header stating the total, and a window over a long file. The file is read
  in its own encoding, and when that is not UTF-8 the header says which —
  because that is the encoding an edit will be written in.
- `code_grep` is a regular-expression search, smart-case, narrowed by a
  glob, answering `path:line: text` and naming its cap when it truncates. It
  is the search this feature has; the next section but one says why.
- `code_edit` replaces an exact fragment that must occur **exactly once**. A
  fragment that is missing, or occurs twice without `replace_all`, changes
  nothing, and the answer says which of the two it was. The result echoes
  the changed lines, numbered, so a second read is not needed to verify.
- `code_write` creates a file or replaces one whole; its own description
  sends the model to `code_edit` for a change inside an existing file, since
  a whole-file write can silently drop what it did not mention.

The habit is *read before you edit*, and it is taught rather than enforced:
the tool text says so, and enforcing it would need per-turn read tracking
that no live run has yet shown to be necessary. What is enforced is
fidelity on the way back. Models emit `\n`; Windows repositories are
`CRLF`; a file is matched on newline-normalized text and written back in
its own shape — line endings, byte-order mark and encoding — and only when
the unedited file comes back byte for byte through that encoding. A
windows-1251 source once came back from an ASCII edit with every Cyrillic
letter replaced by three bytes of replacement character while the diff
showed one line; now a character the encoding cannot store, or a file read
with loss, is refused with nothing written.

One more rule matters for a real fix: reading and editing the project do
not spend the turn's tool-round budget. A turn that is forty rounds of
read, edit and build still has its full allowance of other tools. The
safety net is the per-call timeout, one command at a time, and `Esc`.

## The commands are yours, verbatim

The assistant can build, run and test the project — through three slots
you fill with command lines: `/project build-cmd cargo build`, and the same
for `run-cmd` and `test-cmd`. The three tools take **no arguments at all**,
by schema. The model sees the text of each line, so it can tell you how to
fix a broken one, but the only thing it can do itself is run the slot
exactly as written; a slot you have not filled is a tool that is not
offered. There is no shell tool and there are no arguments on purpose: a
test filter is the first thing anyone wants and the first injection vector,
and the whole point of three fixed slots is that the model cannot compose a
command.

There is no shell underneath either. The line is split into arguments the
way a shell would split it, quotes honoured, and the program is spawned
directly, completed from `PATHEXT` on Windows so one line works on both
platforms. A pipeline or a redirect therefore cannot run — and the refusal
comes when the line is *set*, naming the character it found and the route
that works (put the steps in a script, name the script), not three turns
later as a program that could not be found. The check runs again at
execution time, because a chat file is JSON on disk and can be edited by
hand.

A command that outruns its limit is killed with its whole process tree — a
Job Object on Windows, a process group on Unix — because killing only the
process the app spawned would leave `cargo`'s `rustc` children compiling.
Whatever the command printed before that is **kept**, and the answer says
it timed out: the deliberate inverse of the Python sandbox, which discards
partial output. A build's first errors arrive in its first second, and
throwing them away because the build was slow wastes the whole wait. Output
is capped per stream and cut from the middle, keeping head and tail, since
a compiler puts its first errors at the top and its summary at the bottom.
One command runs at a time across the application — two builds of one
project would fight over the same output directory.

## Every change is a diff, and a diff is a way back

Before the assistant first touches a file in a chat, the file's original
is journaled: a pre-image on disk under `workspace/<chat-id>/`, with a
manifest naming the root it belongs to. Not in the chat file, which must
not grow by megabytes of source; not in the disposable cache, because a
pre-image is the one thing that cannot be recomputed. A file the assistant
created is journaled as one that did not exist.

`F4`, or `/changes`, is the screen over that journal: every touched file,
each shown as a diff between its pre-image and its current content, and
`r` puts one file back after asking — a created file is removed instead.
Deletion does not otherwise exist: no tool can delete, and the revert of a
created file is the one deletion there is, user-initiated and confirmed.
Attaching a *different* directory starts a fresh journal, and that rule
was the one defect the track's own audit found: it had been implemented
inside the write path, so the reset fired on the assistant's next edit
rather than on the attach. Fixed, and the audit written down. Backups pack
`workspace/` with the rest, so a restored chat can still undo.

There is no git integration in this, by choice. The snapshot journal
answers "what did the assistant do" more precisely than a diff against
`HEAD`, works in a directory that is not a repository, and is what powers
revert. A git mode is possible later; it is not the foundation.

## Measured before it shipped

The track opened with a probe against the two families the project gates
on, gemma-4 and qwen-3.6, and the ground truth was not a string comparison:
the fixture is compiled with `rustc` and **run**, so a "fix" that deletes
the arithmetic cannot pass, and every run asserts that `code_edit` was
actually called, because a model that explains the fix in prose would
otherwise pass a smoke about editing.

Arm A is the compile error a user pastes — the prompt is the `cargo build`
output. Arm B keeps the fragment *out* of the prompt: a median that builds
and prints 6 instead of 5, with no code quoted, so the fragment to replace
can only come from what `code_read` returned — and the obvious one-line
fragment occurs twice in the file by construction, so a naive edit is
refused as ambiguous. Both arms: 5/5 on both families, exactly one edit per
run across all twenty runs, no refusal ever fired. The strongest single
datum is arm B's argument: a five-line fragment reproduced byte for byte,
indentation included, from a read that had line numbers prefixed to every
line — the prefixes stripped, the leading whitespace kept, and the fragment
widened past the duplicate by the model's own choice. The families differed
in the route, not the outcome: qwen wandered more before committing, two
to eight calls against gemma's steady two, one run at 74 s against 20.

## The index that did not ship

The plan's last stage was a semantic index over the project, with its own
go/no-go: ship only if it measurably improves answers or reduces rounds
against `code_grep` alone. It was built as a probe over this repository —
about 21 000 line windows — and measured on eight questions asked in the
user's vocabulary rather than the code's ("why does the app sometimes
shorten the conversation by itself", where the code says *compaction*), on
gemma-4-31b, two arms, three passes of the set per instrument version.

Of the turns that looked at the project, grep alone answered 16 of 22
correctly and grep with search 19 of 29; of all turns, 33% against 40%.
Read those two rows together: by one denominator the index is ahead, by the
other behind, and the choice is a judgement about what an unusable turn
means. An effect that changes sign with a definition is smaller than the
instrument measuring it, and rounds were slightly worse. **No-go.**

The instrument turned out harder than the thing measured, and that is the
finding worth keeping. The first table was 5/5 against 5/5 with the search
tool called zero times — the workspace block tells the model in words which
tools it has, the probe's tool was not in that list, and the model believed
the block. Correctness was a rate, not an outcome: the control arm scored
5/5 and then 3/5 on the same questions. A turn that spent itself on calls
and thinking and emitted no text is a token-budget failure, not a retrieval
one, and counting it against retrieval is a claim made from the wrong
evidence. A keyword grader produced a false positive on a general-knowledge
answer and false negatives on correct ones. The one effect that survived
every version of the instrument — with a search tool available the model
answered *without looking* far less often, eight turns against fifteen —
is not what the criterion asked, and shipping on it would be shipping on an
unmeasured basis, which is what the go/no-go exists to prevent.

What the no-go bought: no new cache schema and its migration, no background
indexing task, no re-index on every edit, no settings toggle, and no hard
dependency on an embedding server for a feature that otherwise does not
need one. What would change the answer is a judge-graded measurement at
around eighty turns per arm, or a corpus with sparse comments, where grep
has less to match on. Either is a new probe, not a resumption of this one.

## What it is deliberately not

A shell. Arguments on the commands. A tree-sitter or language-server
dependency for marginal gain at this model class. Fuzzy edit matching,
which silently corrupts files where an exact match with a good error does
not. A file watcher that re-indexes on external edits. A tool that deletes.
Each of those is a line in the plan's "deliberately not doing" list, with
the reason beside it — and the plan, its stages, the probe results and the
audit of where the shipped code differs from it are in the repository, as
[`docs/history/code-workspace.md`](https://github.com/vshylov/mindfork-rs/blob/main/docs/history/code-workspace.md),
with the contract itself in spec §9.12.
