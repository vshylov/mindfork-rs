# Research: renaming the binary to `mindfork`

**Status:** track started 2026-08-24. User's decisions, 2026-08-24: **rename
the binary/command to `mindfork`**; the project, repository and Cargo package
**stay `mindfork-rs`**; and — since the program has had **no public release**
— the cut is clean: no transitional `/usr/bin/mindfork-rs` symlink, no
installer cleanup of a previously-installed `mindfork-rs.exe`, no upgrade
caveats in user docs. Micro-decisions locked in without a decision point are
in §7 — say if one is wrong. Implementation log:
[docs/journal/release.md](../journal/release.md).

Related: [../journal/release.md](../journal/release.md) (the crate-URL entry
that first mapped this rename's ripple), [../roadmap.md](../roadmap.md)
(publishing to crates.io as `mindfork` — a separate, still-open item),
[../history/installers.md](../history/installers.md) (the packaging layout and
naming conventions this doc leaves in place),
[../branding.md](../branding.md) (the wordmark/identity the rename completes).

## 1. Problem — one identity, two names

The user-facing identity is already the short **mindfork**: the wordmark, the
`F1` About dialog and the terminal title (`credits::APP_NAME`), the site
`mindfork.io`, the reserved crate page `crates.io/crates/mindfork`. But the
command the user actually types is `mindfork-rs`, because `Cargo.toml` has no
`[[bin]]` section and the binary inherits the package name. Documentation had
already started drifting toward the brand (`CLAUDE.md` says `mindfork demo`
and `mindfork sandbox setup`). The rename closes that gap: **the command is
the brand; the project keeps its name**.

Precedent for package ≠ binary is ordinary in the ecosystem (`ripgrep` → `rg`,
`fd-find` → `fdfind`); nothing in Cargo, nfpm, or Inno requires the three
names (package, binary, artifact prefix) to agree.

## 2. Where the name comes from, and where it surfaces

The mechanism is one Cargo stanza — the package name stays, the target is
renamed:

```toml
[[bin]]
name = "mindfork"
path = "src/main.rs"   # required: an explicit [[bin]] disables autodiscovery
```

Nothing in the repository invokes the built binary by name (`CARGO_BIN_EXE_*`
is unused; tests and `tools/*.py` don't reference it; `current_exe()` is used
only for its *directory*), so `cargo build/run/test` are unaffected. The full
inventory of surfaces that spell the old name (314 occurrences in 67 files)
splits into three groups:

| Group | Surfaces |
|---|---|
| **Renamed** (user-visible name) | CLI usage/`--version` strings ([cli.rs](../../src/features/cli.rs), 5 locale keys in each of `locales/{en,ru}.json`), the empty-chat title fallback ([render.rs](../../src/screens/chat/render.rs) — now `credits::APP_NAME`), startup/exit log lines ([main.rs](../../src/main.rs)), MCP `clientInfo.name` ([mcp.rs](../../src/shared/mcp.rs)), the sandbox-setup `User-Agent` ([sandbox_setup.rs](../../src/features/sandbox_setup.rs)), the Linux staged binary + `/usr/bin` symlink + `.desktop` + hicolor icons ([nfpm.yaml](../../packaging/nfpm.yaml), [mindfork.desktop](../../packaging/linux/mindfork.desktop)), the Windows installer's exe/`AppName`/shortcuts ([mindfork.iss](../../packaging/windows/mindfork.iss)), CI binary paths ([release.yml](../../.github/workflows/release.yml), [packaging.yml](../../.github/workflows/packaging.yml)), command examples in docs ([install.md](../install.md), [import-format.md](../import-format.md), README alt text, AGENTS.md §6 smoke) |
| **Kept** (identity constants and project-named things) | §3 below |
| **Historical** (journal, history, CHANGELOG, blog posts, spec's attempt-#1 prose) | untouched — logs describe the past |

## 3. What deliberately does NOT change

These all *contain* the string `mindfork-rs` and none of them follows the
binary name; each now carries a source comment saying the survival is
deliberate.

- **`ProjectDirs::from("", "", "mindfork-rs")`**
  ([paths.rs](../../src/shared/paths.rs)) — the `system`-mode data root
  (`%APPDATA%\mindfork-rs`, `~/.local/share/mindfork-rs`). It names the
  existing per-user data folder; changing it would strand that data on every
  current install (including the developer's own).
- **The `secrets.rs` v1 constants** (`CHECK_PLAINTEXT`, `ENTROPY`,
  `HKDF_INFO`) — protocol inputs of the stored-API-key encryption. The exe
  name/path is *not* an input (machine-id/DPAPI plus these constants are), so
  the rename cannot break decryption — but "aligning" the constants would
  orphan every stored key.
- **`INSTANCE_ID`** ([instance.rs](../../src/shared/instance.rs)) — keeping
  the historical id means a pre-rename build and a current one still exclude
  each other (they are the same app).
- **The Inno `AppId` GUID** — upgrade continuity on the developer's machines.
- **Package names and artifact names** — `mindfork-rs` deb/rpm/archlinux
  packages, `mindfork-rs-vX.Y.Z-…` archives and setup exe, the GitHub release
  title: these are the *project's* name, per the user's decision.
- **Directory names** — `/usr/lib/mindfork-rs/` (also: moving it would orphan
  the `config|noreplace` `defaults.json` on upgrade), `/usr/share/doc/mindfork-rs/`
  (Debian policy: doc dir follows the package name), `{autopf}\mindfork-rs`,
  the `{userdocs}\mindfork-rs` suggestion on the installer's directory page —
  all consistent with the data-root names above.

## 4. Linux assessment

The original question was whether the rename causes problems on Linux. It
does not:

- **No name collision.** No distribution packages a `mindfork`
  binary/package (checked via Repology/distro search, 2026-08-24). The
  nearest neighbour is MindForger (`mindforger`) — a different name, no file
  conflict, at worst tab-completion adjacency. The GitHub org "mindfork" is
  not a distro namespace.
- **Package upgrades are clean by construction.** dpkg/rpm/pacman remove
  paths that left the package (`/usr/bin/mindfork-rs`, the old `.desktop`,
  old icon names) and install the new ones; no conffile sits at any renamed
  path (`defaults.json` stays where it was).
- **Path resolution is name-independent.** `current_exe()` resolves
  `/proc/self/exe` to `/usr/lib/mindfork-rs/<whatever>`, so `defaults.json`
  and the bundled dictionaries are found exactly as before; `ps` simply shows
  `mindfork`.
- **Voided transition costs** (recorded for completeness; all moot with no
  public release): user scripts/aliases spelling `mindfork-rs`, desktop-file
  id changing under a pinned dock icon, and a tarball unpacked over an old
  portable install leaving the stale `mindfork-rs` binary next to `mindfork`.

## 5. Windows assessment

The one real trap would have been upgrades: Inno does not delete files that
merely left `[Files]`, so an upgrade over an existing install would leave the
old `mindfork-rs.exe` in `{app}` with the old shortcuts still pointing at it.
**User's decision 2026-08-24: there has been no public release, so this is
not a trap** — the `.iss` renames cleanly (`[Files]`, `[Icons]`, `[Run]`,
`UninstallDisplayIcon`, `AppName`/`UninstallDisplayName`/`DefaultGroupName` →
`mindfork`), with no `[InstallDelete]` archaeology. `AppId`, `DefaultDirName`
and `OutputBaseFilename` stay (§3).

## 6. Change surface

| Area | Files | Change |
|---|---|---|
| Cargo | `Cargo.toml` | the `[[bin]]` stanza (§2) |
| CLI | `src/features/cli.rs`, `locales/{en,ru}.json` | usage/version/guard strings → `mindfork` (keys unchanged) |
| UI | `src/screens/chat/render.rs` | empty-title fallback → `credits::APP_NAME` |
| Cosmetics | `src/main.rs`, `src/shared/mcp.rs`, `src/features/sandbox_setup.rs` | log lines, `clientInfo.name`, `User-Agent` |
| Guard comments | `src/shared/{paths,secrets,instance}.rs`, `src/shared/credits.rs` | "stays `mindfork-rs` deliberately" notes; APP_NAME doc updated |
| Gate | `src/shared/credits.rs` tests | `[[bin]] name` ≡ `APP_NAME`, package ≡ `mindfork-rs` |
| Linux packaging | `packaging/nfpm.yaml`, `packaging/linux/mindfork.desktop` (renamed file), `packaging/linux/build-packages.sh` | staged binary, symlink, `.desktop` (`Name`/`Exec`/`Icon`), icon file names |
| Windows packaging | `packaging/windows/mindfork.iss` | §5 |
| CI | `.github/workflows/{release,packaging}.yml` | built-binary paths, container-smoke assertions, stub name |
| Docs | `docs/install.md`, `docs/import-format.md`, `README.md`, `AGENTS.md` §6, `spec.md` (import example + a CLI-name sentence), `CHANGELOG.md`, `docs/roadmap.md` (crates.io item re-premised), `CLAUDE.md` status, `docs/journal/release.md` | command examples and layout descriptions |

## 7. Micro-decisions locked in (say if one is wrong)

1. **Names → `mindfork`; identifiers, directories and artifacts →
   `mindfork-rs`.** "Names" = anything that *labels the app to a person or a
   peer*: CLI strings, `.desktop` `Name=`, installer `AppName`/shortcut/Start
   Menu group, log lines, MCP `clientInfo`, the download `User-Agent`.
   "Identifiers" = anything that *keys stored data or a namespace*: §3's list.
2. **The version line reads `mindfork {version}`** (locale
   `cli.version.line`) — it is the brand speaking, and AGENTS.md §6's
   artifact smoke now greps for that.
3. **The gate test lives in `credits.rs`** next to `APP_NAME`, parsing
   `Cargo.toml` the way the components gate already does — the brand constant
   and the command the user types cannot drift apart.
4. **crates.io stays out of scope.** Publishing as `mindfork` still requires
   renaming the *package* (the crate name is the publish name); the roadmap
   item is updated to its new premise, not closed.

## 8. Verification

- `cargo fmt --check`, `clippy --all-targets -- -D warnings`, `cargo test`
  green; the new gate test pins `[[bin]] name` ↔ `APP_NAME`.
- Local artifact check: `target/debug/mindfork(.exe)` exists;
  `mindfork --version` prints `mindfork <version>` (the peek phase — safe
  headless).
- The Linux half is validated **on the PR** by `packaging.yml`'s container
  smokes (Ubuntu/Fedora/Arch install + layout + `su tester -c 'mindfork
  --version'`) — the assertions themselves are updated by this track; the
  `.iss` compiles under the pinned Inno 7 in the same workflow.
- **No live engine run needed**: the engine, memory and tools are untouched
  (a name string in `clientInfo`/`User-Agent` does not change any protocol).

## 9. Out of scope

- Publishing to crates.io as `mindfork` (package rename) — roadmap.
  **Reopened and done in §10; the publish itself stays the user's.**
- AUR groundwork keeps its `mindfork-rs-bin` working name (package-named,
  like the deb/rpm).
- Any rename of data directories or secret/lock identifiers (§3) — never.

## 10. 2026-09-18 — the package becomes `mindfork`

**This revises §1 and §7**, where the user's 2026-08-24 decision was that the
project, the repository *and the Cargo package* stay `mindfork-rs`, and closes
the first line of §9. His decision of 2026-09-18: the crate on crates.io is
`mindfork`, because the `-rs` is a **repository** name — it says the project is
written in Rust and keeps it apart from an unrelated company of the same name —
and a registry has no such ambiguity to resolve.

Cargo leaves no third way: it publishes strictly under `[package] name`, with no
alias, so `crates.io/crates/mindfork` — the URL the About dialog has pointed at
since the brand was chosen (`credits::CRATE_URL`) — requires the package to be
called that. Both names were free that day: the registry API answered 404 for
`mindfork` **and** for `mindfork-rs` (measured 2026-09-18, four days after §1's
"still free" was last checked).

### What moved

Everything Cargo derives from `package.name`, and nothing else:

- `Cargo.toml` (`name`), `Cargo.lock` (one line);
- `credits.rs` — the gate now reads `assert_eq!(env!("CARGO_PKG_NAME"), APP_NAME)`
  instead of a literal, so the package, the `[[bin]]` target and the brand cannot
  drift apart in any direction;
- `logging.rs` — the documented `MINDFORK_LOG` example. The filter's target is the
  **crate** name, so `mindfork_rs=debug` would have silently matched nothing after
  the rename: the one place where the old spelling would have misled a user rather
  than merely looked stale;
- `build.rs` — the comments explaining why `PRODUCT_NAME` is spelled out rather
  than taken from `package.name`. The reason changed (the default now happens to
  agree) but the decision did not: a registry identity and the product name
  Windows shows in the UAC dialog are different things, and code signing pins the
  latter (§6.2 of code-signing.md);
- `tools/release_guard.py` — the manifest fixture, so the self-test keeps testing
  against a manifest that looks like ours.

The `[[bin]]` section is now redundant — the roadmap predicted that — and stays
anyway: it is what the gate above reads, and it keeps the command from following
a future package rename by accident.

### What did not move, and could not

The three identifiers that name something already written to a user's disk are
frozen literals with comments saying so, none of them derived from
`package.name`: the data directory (`ProjectDirs::from("", "", "mindfork-rs")`),
the API-key crypto strings (`ENTROPY`, `HKDF_INFO`, `CHECK_PLAINTEXT`) and the
single-instance lock id. So this rename moves no user data and invalidates no
stored key — which is what §3 promised, and the reason it was written down.
`nfpm.yaml`'s package name, `/usr/lib/mindfork-rs/`, the release asset prefixes
and the Sonar project key keep the repository's name too.

### Verification

- **`cargo publish --dry-run`** (and `cargo package` before it): 368 files,
  15.4 MiB, 3.8 MiB compressed, the verification build of `mindfork v0.10.0`
  green, upload aborted by the dry run. That is the publish itself, minus the
  upload;
- `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, **3326 unit
  tests**, 196 `#[ignore]`;
- **no live run applies** — §8's reasoning holds unchanged: nothing here reaches
  an engine, a file format or a protocol. A package name is read by Cargo and by
  no one else at runtime.

### What was left, and how it closed

The publish. `cargo publish` is the user's (AGENTS.md §5 — the agent publishes
nothing), and it belonged to the **next version bump**, not to 0.10.0: the tag
`v0.10.0` already points at a tree whose package is `mindfork-rs`, so publishing
`mindfork 0.10.0` from a later commit would have put a version on the registry
that no tag matches. The CHANGELOG line, the README badge and the `cargo install
mindfork` line in `install.md` belonged to that release too — until the crate is
actually there, every one of them is a broken promise, which is exactly what the
roadmap said.

**Closed on 2026-09-19 with 0.10.1**, and by a workflow rather than by a typed
command: publishing the release starts `.github/workflows/crates-io.yml`
(AGENTS.md §6 step 7), so the version on the registry is the tree the released
binaries were built from. `mindfork 0.10.1` is live — 3 941 402 bytes, MIT, one
binary target, `rust-version` 1.96 — and the three promises are kept in the same
release that made them true. The only thing the registry does not show is
documentation: docs.rs builds nothing for a crate without a library target, so
the `documentation` field points at the manual, which is what the crates.io page
links.
