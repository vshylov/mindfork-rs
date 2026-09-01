# Vendored spellcheck dictionaries

Hunspell dictionary pairs (`<name>.aff` + `<name>.dic`) for the live spellcheck
in the input box (spec §11.5). They are committed to the repository, copied next
to the binary by [`build.rs`](../build.rs) into `data/dictionaries/`, and shipped
in every release archive, Linux package and the Windows installer — so they are
**redistributed**, and each one is somebody else's work under somebody else's
licence.

**These files are vendored, not ours.** Each row below pins the repository the
file was taken from, the exact commit, the licence it is redistributed under and
the path to the licence text vendored in `licenses/`, exactly as
[`syntaxes/SOURCES.md`](../syntaxes/SOURCES.md) does for the grammars. There is
no re-fetch script here, so each row also carries the **sha256 of the file in
this repository**: `sha256sum dictionaries/*.aff dictionaries/*.dic` is the whole
verification, and a silent swap shows up as a changed digest in the diff.

The provenance below was **established by matching bytes**, not by memory: every
row was confirmed byte-identical to the upstream file named in it. The commit
that added these files (`40d3b49`, 2026-06-29) recorded nothing about where they
came from, which is the gap this file closes
([docs/research/code-signing.md](../docs/research/code-signing.md) §3.1).

## The pins

| Dictionary | Files | Repository | Path | Commit | Licence | Licence text |
|---|---|---|---|---|---|---|
| American English | `en_US.aff`, `en_US.dic` | github:ropensci/hunspell | `inst/dict/` | `bcdf9867a4aa0eb25bd5bb3a705f67c5b014ac9e` (`.aff`), `1a3b794b1f5bd2214626fc72db168748dc21d6f5` (`.dic`) | SCOWL (permissive, BSD-style) | [`licenses/en_US.txt`](licenses/en_US.txt) |
| British English | `en_GB.aff`, `en_GB.dic` | github:ropensci/hunspell | `inst/dict/` | `1a3b794b1f5bd2214626fc72db168748dc21d6f5` | LGPL | [`licenses/en_GB.txt`](licenses/en_GB.txt) |
| Russian | `ru_RU.aff`, `ru_RU.dic` | github:wooorm/dictionaries | `dictionaries/ru/` (`index.aff`, `index.dic`) | `e93d088fe597999d5cb6a2cfe6b34f0583577d59` | BSD 3-clause style | [`licenses/ru_RU.txt`](licenses/ru_RU.txt) |

sha256 of the files as committed here:

| File | sha256 |
|---|---|
| `en_US.aff` | `5b5d3effe419c8078827fe0f297e3beefc0e50d56427707033b2b17ddbaadf4c` |
| `en_US.dic` | `9eb52cdeab6c87a4988df7d2845caaa39cd9bdc93b45bc2e3c228f8070807767` |
| `en_GB.aff` | `c3fd8942dfb60f1eceaaaa192c408f20a094489d990e4c2a566b57256a4d7527` |
| `en_GB.dic` | `9b70004d5bc92b35f02fec5654d4e2f7c43bdda218b668d08cdfc95ad2e06e3d` |
| `ru_RU.aff` | `38ce7d4af78e211e9bafe4bf7e3d6a2c420591136cb738ec6648f8fdf6524cd7` |
| `ru_RU.dic` | `f6047416a0204adbecf3a451b874ec8a97ee37e2cbc714466ef04d8dbcc0d6fc` |

## Where they actually come from

The repositories above are where *we* took the files from. Both are packagers,
and behind both stands the same origin — which matters for the licences, and for
knowing what a version bump would mean.

### English (both variants)

`ropensci/hunspell` is the R `hunspell` package; its `inst/dict/readme.txt` says
the pair was taken from the LibreOffice **"English dictionaries" extension,
release 2018-11.01**, and our files agree with that:

- `en_US.dic` and `en_GB.dic` are **byte-identical to
  `LibreOffice/dictionaries` `en/` at commit `605e1d1441a8e0a67709b4869ecf4c651e81dfd5`**
  (2018-10-25, "Updated the English dictionaries: GB+AU+CA+US"), the commit that
  carries that release;
- `en_GB.aff` states its own version in its header — *"David Bartlett, Andrew
  Brown, Marco A.G.Pinto. V 2.66, 2018-11-01"*.

**Both `.aff` files are modified** relative to that origin, by the packager, and
the modification is the same in each: `WORDCHARS 0123456789’` becomes
`WORDCHARS ’`, carrying the comment *"# Jeroen: removed numbers from WORDCHARS
for R"* — digits stop being word characters, so a token like `2018` is not
offered to the checker as a word. `en_GB.aff` additionally has trailing
whitespace stripped from about a thousand lines. Nothing else differs; the word
lists themselves are untouched.

The **en_US** word list is SCOWL — *"Copyright 2000-2018 by Kevin Atkinson"*,
permissive and BSD-style, with parts derived from Ispell, WordNet 1.6 (Princeton
University) and Alan Beale's 12dicts. All of those notices are in
[`licenses/en_US.txt`](licenses/en_US.txt), which is the upstream
`README_en_US.txt` at the same commit.

The **en_GB** word list began as a subset of Kevin Atkinson's list *"and thus is
covered by his original LGPL licence"* (the upstream README), then was
extensively rewritten by David Bartlett, Brian Kelk, Andrew Brown and Marco
A.G. Pinto. Note that the licence is stated as **"LGPL" without a version** in
the files of this vintage; upstream only started bundling an explicit LGPL v3
text one release later (its changelog: *"2019-03-01 — Added the LGPL_V3
License .txt into the Extension"*). [`licenses/en_GB.txt`](licenses/en_GB.txt) is
the upstream `README_en_GB.txt` at our commit, licence statement and attribution
included. **If a clean LGPL v3 notice matters more than the frozen word list**,
the way to get one is to move to a current upstream release — which is a
behaviour change (upstream's `en_GB.dic` has grown from 996 KB to 1.2 MB since)
and belongs in its own change, not in this record.

### Russian

`wooorm/dictionaries` generates its packages from `LibreOffice/dictionaries`;
our two files are byte-identical to its `dictionaries/ru/index.aff` and
`index.dic`. The dictionary itself is *"Copyright (c) 1997-2008, Alexander I.
Lebedev"* under a BSD 3-clause-style licence, with one recorded later change —
*"2012-08-24: Laszlo Nemeth — ru_RU.aff: add TRY line for better suggestions"*.
Full text in [`licenses/ru_RU.txt`](licenses/ru_RU.txt).

## Rules for this directory

- **A dictionary added here gets a row above, a licence in `licenses/`, and a
  digest** — otherwise the project is redistributing a file it cannot account
  for, which is the state this document was written to end.
- The licences travel with the dictionaries: `licenses/` is copied into the
  release archives, the Linux packages and the Windows installer beside the
  `.aff`/`.dic` pairs. `build.rs` deliberately does **not** copy it into the dev
  build's `data/dictionaries/` — it skips subdirectories, and the spellcheck
  loader only ever looks for an `.aff` with a matching `.dic`, so a `licenses/`
  folder is invisible to it either way.
- A user's own dictionary goes in the **data** directory, not here
  (`data/dictionaries/`, see the `README.txt` written there on first run) — this
  directory is the set the project ships and is answerable for.
