# English source-language migration — design doc

Migrate the project's *development* language (comments, doc-comments, all
documentation, non-user-facing strings) from Russian to English, as a focused
effort on branch `chore/english-source`. This is **not** an i18n rollback:
user-facing text stays localizable through axes A/B (Russian UI/agent remains a
supported locale). Came from the roadmap direction "English source (prep for
open source)", now closed (`docs/roadmap.md` §"Recently closed").

## Scope
Translate:
- All comments and doc-comments (`//`, `///`, `//!`) in `.rs`, and comments in
  `.py`, `.sh`, `.yml`/`.yaml`, `.toml`, `.iss`, `Cargo.toml` metadata.
- All Markdown documentation (`README`, `spec`, `docs/**`, `AGENTS.md`,
  `CLAUDE.md`, `CHANGELOG.md`, ADRs, history, research).
- **Non-user-facing** production string literals (tracing logs, `build.rs`
  warnings, internal diagnostics) → inline English.
- **User-facing** production string literals still living as raw Russian (i18n
  gaps) → move into the locale bundles (`ru.json` keeps Russian, `en.json` gains
  English), wired through `loc`/`ctx.loc` on the correct axis.

Keep Russian (do not touch): `locales/ru.json` values, Hunspell dictionaries,
in-test assertion strings that check `ru` output, intentional `[ru]` desktop /
`ru.` installer localization, binary blobs.

## Safety facts
- **Binary crate → doc-comment code blocks are not compiled as doctests.**
  Translating comments/doc-comments cannot break `cargo test`.
- `rustfmt` does not reflow comment text by default → `cargo fmt --check` stays
  clean after comment edits.
- The only test-coupled Russian is the ~1,966 in-test assertion strings, which
  we deliberately leave alone.

## Tooling
- `tools/cyrillic_scan.py` — worklist + acceptance gate. Reports every
  non-allowlisted Cyrillic line; exits non-zero while any remain. Baseline at
  start: ~31,047 lines / 239 files. Post-flip it became a CI lint (a step in
  the `lint` job of `.github/workflows/ci.yml`).
- `glossary-ru-en.md` — canonical term table; **every translator used it**.
  Lived at `tools/glossary-ru-en.md` during the migration; archived next to
  this doc.

## Phases (one commit each, on `chore/english-source`)
0. **Tooling + glossary + 2 validation samples**, then pause for glossary review.
1. **Living docs** — README, spec, architecture, install, import-format, AGENTS,
   roadmap, CHANGELOG, ADRs, PR template, assets/README.
2. **CLAUDE.md** (7k-line journal) via split/translate/reassemble.
3. **Archive docs** — `docs/history/*`, `docs/research/*`, `docs/notes-vec0.md`.
4. **Code comments** (~11.4k lines) — subagents batched by directory cluster;
   `cargo build` after each batch, full gate after all.
5. **Production strings** — triage table; inline-translate diagnostics, bundle
   user-facing gaps; i18n gate tests must pass.
6. **Convention flip** — `CLAUDE.md`/`AGENTS.md` "comments & docs in Russian"
   rule → English; commit-message-language rule; mark roadmap direction done;
   archive this doc + glossary.
7. **Acceptance** — scanner clean (allowlist only); `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test` green; diff review.

## Large-file mechanism (CLAUDE.md / architecture.md / spec.md)
A script splits the file at heading boundaries into disjoint chunk files in the
scratchpad → N subagents translate chunks in parallel (disjoint files = no
conflicts) → a script reassembles in order. The glossary guarantees cross-chunk
consistency.

## Subagent instruction template (Phases 1–5)
Each translation subagent is given: the target file path(s), the absolute path
to the glossary (then at `tools/glossary-ru-en.md`), and these rules:

> Translate the Russian **comments and prose** in the listed file(s) to English,
> in place, editing only what is Russian. Follow the glossary
> exactly for terminology.
> - Translate: `//`/`///`/`//!` comments (including in test code), Markdown
>   prose, and — per routing below — string literals.
> - Never change: code, identifiers, types, control flow, formatting, line
>   structure, code fences, links, anchors, `spec §X` cross-refs, numbers,
>   protocol/CLI/JSON/locale-key tokens, `#[attrs]`, macros.
> - String literals: **do not touch** strings inside test code, `ru.json`, or
>   intentional `[ru]`/`ru.` localization. For production strings, follow the
>   glossary "routing" section (inline-translate diagnostics; flag user-facing
>   gaps for bundle work rather than translating them inline).
> - Keep the terse voice; do not expand comments into prose. American English.
> - After editing, the file must still be byte-identical in structure — only
>   Russian text changed. Report any string you were unsure how to route.
>
> Verification the agent runs before reporting done: for `.rs`, confirm the file
> still parses (no stray edits) by a mental diff; the harness runs `cargo build`
> and `python tools/cyrillic_scan.py` per batch.

## Status — done

Branch `chore/english-source`. Every commit was verified with `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings`, and the full test suite
(1267 passing, 58 `#[ignore]`). Cyrillic went **30,890 -> 0** non-allowlisted
lines; `python tools/cyrillic_scan.py` exits 0.

All phases landed: Phase 0 (tooling/glossary/samples), Wave 1 (living docs),
Wave 2 (CLAUDE.md), Wave 3 (archive docs), Wave 4 (all code comments), Phase 5a
(non-user-facing strings inline), Phase 5b (the last ~36 user-facing strings
routed into the locale bundles), Phase 6 (convention flip) and Phase 7
(acceptance).

### How Phases 6-7 resolved
- **Convention flips** were applied during the waves rather than as a separate
  step, and verified at the end: `CLAUDE.md` §Conventions and `AGENTS.md` §3
  both say "Comments and docs — in English", `AGENTS.md` §5 requires English
  commit messages, and the README Conventions paragraph matches.
- **Roadmap**: §"English source (prep for open source)" moved into "Recently
  closed"; this design doc and the glossary moved to `docs/history/`.
- **CHANGELOG**: an `[Unreleased]` entry covers the two user-visible effects —
  `CharacterNames::default()` seeding new profiles with English names, and the
  previously unlocalized strings that gained locale keys.
- **Migration tooling**: `tools/cyrillic_scan.py` **stays** and is wired into
  CI (a step in the `lint` job) — it is what stops Russian creeping back into
  comments and docs now that the convention is English. The glossary was
  archived rather than deleted: it stays useful as a terminology reference for
  writing English docs/comments, and keeps this doc's references live.

### Notes worth keeping
- The scanner needed four corrections, all false-positive sources rather than
  real work: multi-line test fixtures; `tests.rs` files whose `#[cfg(test)]`
  marker lives in the parent `mod.rs`; `//` inside string literals (a
  protocol-relative URL in a fixture); and its own Cyrillic ranges. Roughly 3.2k
  of the original count was never translation debt.
- Deliberately kept Cyrillic: `locales/ru.json`, Hunspell dictionaries, in-test
  fixtures and `ru`-locale assertions, the `.desktop` `[ru]` keys and `.iss`
  `ru.` messages, `keys.rs` JCUKEN char data, and a few `cyrillic-ok`-marked
  endonyms. `docs/research/mermaid-ascii-rendering.md` is skipped wholesale
  because its repro inputs must be multibyte to demonstrate the bug.
- Pre-existing defects surfaced and fixed along the way: 32 `spec.md` anchors
  broken by heading translation, tool-call artifacts committed into two history
  docs, and `present.rs` matching a Russian prefix to detect a failed tool
  result. An unlocalized thoughts-label and console exit-code line in
  `message_feed.rs` were routed into bundles.

## Process
Branch `chore/english-source` off clean `main` (working tree was clean — no
conflict with in-flight work). Commits carry the model-attribution trailer per
AGENTS.md §5. Delivered as phased commits on one branch.
