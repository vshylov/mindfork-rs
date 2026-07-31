# Vendored syntax grammars — design plan

Status: **forks decided (user's decision, 2026-07-31 — all four as
recommended: F1 the curated set, F2 vendored files + manifest, F3 a build-time
dump, F5 no user overlay for now); implementation in progress**. Follows the
`fix/zig-code-highlighting` fix, which established the cause and closed the
symptom with an approximation (Zig on the Rust grammar). This plan is about the
real fix: shipping actual grammars.

## 1. Problem

Code-block highlighting is `syntect` over `SyntaxSet::load_defaults_newlines`
([ADR 0003](../decisions/0003-own-markdown-renderer.md)) — **75 syntaxes**, a
snapshot of Sublime Text's default packages from syntect's own assets. Of ~60
language labels models routinely emit, **36 do not resolve**: `zig`, `toml`,
`dockerfile`, `powershell`, `swift`, `scss`, `graphql`, `terraform`, `julia`,
`solidity`, `protobuf`, `cmake`, `nginx`, `ini`, `asm`, `nim`, `dart`, `elixir`,
`vue`, `svelte`, `fortran`, `cobol`, `jsonc`, `json5`, `regex`, `env`,
`gitignore`, `awk`, `sed`, `hcl`, `vim`, `csv`, `less`, `plaintext`, `text`,
`ps1`. Such a block renders as flat text on the reverse-video rectangle.

The `canonical_lang` alias table can only approximate — `typescript → js`,
`kotlin → java`, `zig → rs`. That is strictly better than grey text, but it is a
different language's grammar: the alias misses `try`/`defer`/`var` in Zig, all
of TypeScript's type syntax, and everything Kotlin does not share with Java. For
`toml`/`dockerfile`/`powershell` there is no relative in the set at all.

## 2. What the probe measured

A temporary test (`vendor_probe_tmp`, removed after the run) against 20
candidate grammars fetched from upstream. **Everything below is measured, not
assumed.**

### 2.1 Hard constraints of syntect

- **Only `.sublime-syntax` (YAML) is loadable.** syntect has no `.tmLanguage`
  syntax loader (`plist-load` is for *themes*). Several popular repositories —
  `PowerShell/EditorSyntax`, `quiqueg/Swift-Sublime-Package`,
  `Microsoft/TypeScript-Sublime-Plugin`, `wmertens/sublime-nix` — ship only
  `.tmLanguage` and are therefore unusable as-is.
- **`extends:` is not supported.** sublime-syntax v2 inheritance fails to load:
  `sublimehq/Packages`' `TypeScript.sublime-syntax` (`extends:
  JavaScript.sublime-syntax`) → *"Missing mandatory key in YAML file: match"*,
  `TSX` → *"Context 'main' is missing"*, `alexlouden`'s `HCL` → *"Missing
  mandatory key: contexts"*. **Every vendored grammar must be self-contained.**
  This also rules out cherry-picking from the current `sublimehq/Packages`,
  which is otherwise tempting (it carries TypeScript, TSX, TOML, Git*, and newer
  versions of what we already have).
- A grammar's own `file_extensions` are what `find_syntax_by_token` matches, so
  a vendored grammar makes its label work with no table entry.

### 2.2 The candidate set — 18/18 load

| Label | Grammar (`name`) | Source | License |
|---|---|---|---|
| `zig` | Zig | codeberg `ziglang/sublime-zig-language` | see repo LICENSE |
| `ts`/`typescript` | TypeScript | `sharkdp/bat` (converted) | upstream: MS plugin |
| `toml` | TOML | `jasonwilliams/sublime_toml_highlighting` | MIT |
| `dockerfile` | Dockerfile | `asbjornenge/Docker.tmbundle` | MIT |
| `powershell`/`ps1` | PowerShell | `SublimeText/PowerShell` | LICENSE.txt |
| `swift` | Swift | `colinta/decent-swift-syntax` | MIT |
| `kotlin`/`kt` | Kotlin | `guille/sublime-kotlin` | Unlicense |
| `scss` | SCSS | `braver/SublimeSass` (bat's pinned commit) | MIT |
| `graphql` | GraphQL | `dncrews/GraphQL-SublimeText3` | WTFPL |
| `terraform`/`tf` | Terraform | `alexlouden/Terraform.tmLanguage` | MIT |
| `elixir`/`ex` | Elixir | `princemaple/elixir-sublime-syntax` | MIT |
| `solidity`/`sol` | Solidity | `davidhq/SublimeEthereum` | MIT |
| `julia`/`jl` | Julia | `JuliaEditorSupport/Julia-sublime` | LICENSE |
| `nix` | Nix | `sharkdp/bat` (converted) | upstream: sublime-nix |
| `dart` | Dart | `sharkdp/bat` (converted) | upstream: Dartlight |
| `protobuf`/`proto` | Protocol Buffer | `VcamX/protobuf-syntax-highlighting` | MIT |
| `cmake` | CMake | `zyxar/Sublime-CMakeLists` | MIT |
| `nginx` | nginx | `SublimeText/nginx` | MIT |

Total YAML ≈ 660 KiB (Elixir 99 KiB and TypeScript 202 KiB dominate). Every one
of the 18 parses and links, and a real highlight pass through the vendored Zig
grammar produced 19 styled spans for one line.

`sharkdp/bat` is a useful shortlist source (its `.gitmodules` is a curated,
syntect-verified list) and it keeps standalone converted `.sublime-syntax` files
for grammars whose upstream ships only `.tmLanguage`. Taking a file from bat
means inheriting the **original** upstream license, not bat's Apache-2.0 — so
each such file needs its true provenance recorded, or a swap to a real upstream
that ships `.sublime-syntax` (already found for PowerShell and Swift).

### 2.3 Cost — the decisive numbers (release build)

| Path | Time | Size |
|---|---|---|
| Today: `load_defaults_newlines` (75) | **0.57 ms** | 359 KiB dump inside syntect |
| Runtime: `into_builder` + 18 × `add` + `build` (93) | **130 ms** (24 unlink + 105 build) | — |
| Prebuilt **compressed** dump (93) | 4.1 ms | 456 KiB |
| Prebuilt **uncompressed** dump (93) | **0.55 ms** | 501 KiB |

So a build-time uncompressed dump costs **nothing measurable at runtime** — it
is exactly what syntect does for its own defaults — and grows the binary by
~142 KiB *if we also drop syntect's now-unused default dump* (feature
`default-syntaxes`). Building the set at runtime, by contrast, puts a ~130 ms
stall on the first code block rendered.

### 2.4 A consequence, not a fork: the alias table shadows real grammars

Measured: with the Zig grammar loaded, `resolve_syntax("zig")` still returned
**Rust** — `canonical_lang` maps `zig → rs` *before* the lookup, and the same
holds for `kotlin → java` and `typescript → js`. Whichever options are chosen
below, the approximations that become real must be deleted from the table; the
genuine aliases (`csharp → cs`, `cpp → c++`, `golang → go`, …) stay.

## 3. Forks

**F1 — how many languages.**
- (a) Zig plus two or three most-wanted.
- (b) **The curated 18 of §2.2** — the labels that actually show up in a chat
  with an LLM. ~660 KiB of YAML, ~142 KiB of binary. *(recommended)*
- (c) bat-scale (~80 repositories, hundreds of grammars): more binary, and each
  one is a licence and an update to track for a language nobody will paste here.

Adding a language later costs one file plus one manifest row, so (b) is not a
one-way door.

**F2 — how the files live in the repository.**
- (a) git submodules, as bat does: exact provenance, but 18 submodules,
  `--recursive` clones, a CI step, and whole repositories dragged in for one
  file each.
- (b) **The `.sublime-syntax` files vendored directly** into `syntaxes/`, next
  to the existing `dictionaries/` and `locales/`, with a manifest recording
  repository, commit sha and licence, and `tools/fetch_syntaxes.py` to refresh
  them reproducibly. *(recommended — the project already vendors dictionaries as
  files, and a pinned sha gives the provenance a submodule would)*

**F3 — when the set is assembled.**
- (a) At runtime in the `LazyLock`: no build step, but ~130 ms on the first code
  block and a broken grammar becomes a runtime surprise.
- (b) **At build time** (`build.rs` → uncompressed dump → `include_bytes!` +
  `from_uncompressed_data`): 0.55 ms at runtime, and a grammar that fails to
  load **fails the build** instead of silently disappearing. *(recommended)*

**F4 — licence attribution.** Recommended and not put as a question: a
`syntaxes/SOURCES.md` manifest (repository, commit, licence, licence text) plus
a section in the `F1` → *Components* tab, which exists for exactly this. The
existing `components_cover_direct_dependencies` gate is about Cargo
dependencies and is unaffected; grammars get their own gate ("every vendored
file has a manifest row").

**F5 — user-supplied grammars in `data/syntaxes/`.**
- (a) **Not now.** *(recommended)* The overlay would have to parse and re-link
  at runtime — precisely the ~130 ms F3 exists to avoid — so it should be its
  own decision with its own measurement.
- (b) Yes, mirroring external locales (`data/locales/`, i18n Tier 3): a power
  user drops a grammar in and gets highlighting without a rebuild.

## 4. Stages (if the recommended options are taken)

1. **`syntaxes/` + fetch tool + manifest.** Vendor the 18 files with their
   licences; `tools/fetch_syntaxes.py` re-fetches from the pinned shas; a gate
   test cross-checks files against the manifest.
2. **Build-time dump.** `build.rs` (syntect as a build-dependency with
   `regex-fancy`, so oniguruma is not compiled for the host too) reads
   `syntaxes/`, adds them to the defaults, writes an uncompressed dump into
   `OUT_DIR`; `SYNTAX_SET` becomes `from_uncompressed_data(include_bytes!(…))`.
   Verify whether syntect's `default-syntaxes` feature can be dropped without
   losing `highlighting` — if not, the only cost is a dead 359 KiB in the
   binary.
3. **Prune the alias table** (§2.4), update the tests, add a behavioural test
   per new label, refresh docs (ADR 0003 note, spec §11.4, README, CHANGELOG).

## 5. Risks

- **Licences.** Anything without a clear permissive licence is dropped rather
  than vendored. The files taken from bat need their true upstream licence
  recorded or a source swap.
- **Grammar quality is not ours.** A vendored grammar can mis-highlight; the
  fallback is the same unhighlighted path as today, so the worst case is the
  current behaviour.
- **`extends:` on update.** An upstream that migrates to v2 inheritance stops
  loading — with F3(b) that fails the build, visibly, rather than silently
  dropping a language.
- **Build-time cost.** syntect compiled once more for the host; the dump build
  is ~100 ms.

## 6. Out of scope

- Replacing the base bundle with the current `sublimehq/Packages` (198
  syntaxes, newer versions of what we ship) — a separate, larger question.
- Themes: we build our own from the [`Palette`](../../src/shared/theme.rs) (ADR
  0003) and vendor none.
- Highlighting inside the `attachment_search`/`rag_search` result fragments —
  those render verbatim by design.
