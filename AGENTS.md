# AGENTS.md — task workflow for AI agents

Mandatory rules for AI agents (Claude Code and any others) working on this
repository. Project orientation (what this is, architecture, changelog of what's
done) — [CLAUDE.md](CLAUDE.md) and [docs/architecture.md](docs/architecture.md);
here — **how to run a task**: from spec to PR. The rules are derived from the
project's actual practice (see the "Post-M9" journal in CLAUDE.md) — follow them
instead of reinventing the process.

## Definition of Done checklist

A task isn't considered complete until all of this is done:

- [ ] work was done in a **separate branch** named per convention (§2), not in `main`;
- [ ] for a complex task, there's a **design doc/research**, with forks confirmed
      by the user **before** implementation (§1);
- [ ] `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` +
      `cargo test` — green;
- [ ] tests written alongside the code as it's written; functionality touching
      the engine/memory/tools is covered by an `#[ignore]` smoke test and **run
      against a live stack** (§3); for pure UI/refactor — explicitly noted that a
      live run isn't required;
- [ ] documentation updated per the table in §4 (at minimum — an entry in the
      CLAUDE.md journal);
- [ ] commits carry a trailer with the **actual model**, PR description has a
      "Models" section (§5).

## 1. Orientation and design (before code)

**Read before working:** [CLAUDE.md](CLAUDE.md) (orientation + journal),
[docs/architecture.md](docs/architecture.md) (code map — affected §s),
relevant [ADRs](docs/decisions/) and sections of [spec.md](spec.md) on the topic.

**Complex task → write a doc first, then code.** Signs of a complex task
(one is enough): a multi-stage/PR track; an architectural decision or new
external technology/protocol/crate; non-obvious forks requiring the user's
choice; a change to contracts between layers. Simple tasks (bug fix, polish,
mechanical refactor per an existing playbook) don't need a doc — a branch and
a journal entry is enough.

| Genre | Where | Precedents |
|---|---|---|
| Technology/protocol research (pre-decision) | `docs/research/<topic>.md` | `python-wasmer-sandbox.md`, `openai-responses-client.md` |
| Track design plan (stages, scope, forks) | `docs/<topic>.md`; once the track is done — moves to `docs/history/` | `summary-as-snapshot.md`, `input-selection-undo-mouse.md` |
| Adopted architectural decision (outcome) | `docs/decisions/NNNN-<slug>.md` (ADR) | `0005-python-sandbox-wasmer.md` |

Design doc rules:
- **Forks are spelled out explicitly** (as a list, with options and a
  recommendation) and **confirmed by the user before implementation**; what's
  adopted is recorded (`"user's decision: …"`, with a date for important ones).
- A big track starts with an **MVP probe** with a **go/no-go** criterion
  against a live model (self-model / notes-connectivity pattern); tiers come
  after go.
- Track stages get separate branches/PRs. A finished track: the plan moves to
  `docs/history/` in a dedicated docs branch, references to it in
  CLAUDE.md/architecture.md get updated.

## 2. Branch (before the first commit)

We don't commit directly to `main`. The branch is created off fresh `main`
**before the first commit**:

| Prefix | For |
|---|---|
| `feat/<slug>` | new functionality |
| `fix/<slug>` | defect fix |
| `refactor/<slug>` | refactor without behavior change |
| `docs/<slug>` | docs only |
| `spike/<slug>` | probe/experiment |

Slug — short English kebab-case (`feat/input-mouse`,
`refactor/db-module-split`). One PR = one task or one track stage.
**Mechanical refactor and behavior change don't mix** in one PR.

## 3. Development

Code conventions are in [CLAUDE.md §Conventions](CLAUDE.md); key points and
additions:

- **Before every commit**: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` — all green.
- Tests alongside the code (`#[cfg(test)]`) **as you go**, not at the end; a
  real server/model — `#[ignore]` smokes (silently skipped without the needed
  env var).
- **A live run is mandatory** for functionality touching the engine / memory /
  tools / provider protocols: run the relevant `#[ignore]` smokes against a
  live stack (`cargo test -- --ignored --nocapture --test-threads=1`; env —
  see CLAUDE.md §Commands) and record in the journal the **model/stack and
  outcome** ("Smoke — GO" pattern). A pure UI/refactor doesn't need a live
  run — say so explicitly.
- **FSD**: dependencies strictly downward (`app → screens → widgets →
  features → entities → shared`); `screens`/`widgets` don't import `app`.
- New config/entity fields — `#[serde(default)]` (old JSON reads without
  migration); SQLite schema — `CREATE TABLE IF NOT EXISTS`.
- Comments and docs — **in English**; reference spec/architecture sections. So
  is the **developer-facing test log**: `println!`/`eprintln!` format strings are
  English even inside tests, where fixture data and `ru`-locale assertion strings
  legitimately stay Cyrillic — interpolate a value (`{SOURCE:?}`) instead of
  spelling it into the label. `tools/cyrillic_scan.py` enforces both rules in CI.
- Errors: `anyhow` in application layers, `thiserror` in `shared`. Logs —
  file only (`logs/`), no `println!` (stdout is taken by the TUI).
- New keys/commands — go straight into the help overlay (`F1`/`?`,
  `HELP_KEYS`) and the key tables in README / spec §11.7.
- Playbook for mechanically splitting large files —
  [docs/history/refactoring-god-objects.md](docs/history/refactoring-god-objects.md).

## 4. Finalization: documentation

Before the PR, update the docs per the table (this is part of the task, not
"later"):

| What changed | What to update |
|---|---|
| Any completed task | **CLAUDE.md** — journal entry (`### Post-M9: <topic> (done)`: what/why/key decisions/test count) + test count and date in the "## Status" header |
| User-visible effect (feature/change/fix/removal affecting the user; data format change) | **CHANGELOG.md** — item in the `[Unreleased]` section, category Added/Changed/Fixed/Removed/**Data**/Security; one to two lines in user language (not an internals log). Purely internal refactor/tests — skip. See docs/history/release-engineering.md §3.2 |
| Code structure, modules, flows, invariants | **docs/architecture.md** — affected §s |
| Behavior, contracts, "what and why" | **spec.md** — affected §s |
| User-facing functionality (keys, commands, settings) | **README.md** (+ the help overlay in code, see §3) |
| Install, run, env, engine | **docs/install.md** |
| An architectural decision was adopted | new ADR in `docs/decisions/` + links from CLAUDE.md and architecture.md |
| A groundwork item was closed / a new one appeared | **docs/roadmap.md** |
| A track with a design plan finished | plan → `docs/history/`, references updated (§1) |

If a row in the table isn't affected — don't invent anything; but the
CLAUDE.md journal is updated **always** (except pure documentation PRs —
there it's enough to fix the docs themselves and the links).

## 5. Commit and PR: model attribution

**Commits**: message in English, conventional-commits style
(`feat(tools): …`, `fix: …`, `refactor: …`, `docs: …`). The last line — a
trailer with the **actual model that wrote the code** (the model is named in
the agent's system prompt — don't guess and don't copy someone else's
trailer):

```
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

If multiple models wrote code in the commit — multiple trailers.

**PR**: body — per the template [.github/pull_request_template.md](.github/pull_request_template.md).
Required parts:
- what was done and why, linking the design doc/research/ADR (if any);
- test summary (number of green unit tests, `#[ignore]`; which live smokes
  were run and against what stack);
- documentation checklist (mirrors the table in §4);
- a **"Models"** section — every model that wrote code/tests/docs in this
  PR, with each one's role. Example:

```
## Models
- Claude Fable 5 — implementation and tests
- Claude Opus 4.8 — design doc, review
```

Merging the PR is up to the user; the agent doesn't merge or push to `main`
itself.

## 6. Release

Versioning — **SemVer**, the version's source of truth is `Cargo.toml`
(`env!("CARGO_PKG_VERSION")`); a release = git tag `vX.Y.Z` on the merge
commit into `main`. While at `0.x`: MINOR — features/tracks, PATCH — fixes.
Full process and decisions —
[docs/history/release-engineering.md](docs/history/release-engineering.md).

**Discipline between releases:** every PR with a user-visible effect or a
data format change adds an item to `CHANGELOG.md` → `[Unreleased]` (§4).

**Release checklist**:

1. **Release PR** (branch `docs/release-X.Y.Z` or `chore/release-X.Y.Z`): bump
   `Cargo.toml` (+ rebuild `Cargo.lock`) to `X.Y.Z`; in `CHANGELOG.md` rename
   `[Unreleased]` → `[X.Y.Z] — <date>`, start a fresh empty `[Unreleased]`, and
   update the comparison links at the bottom of the file. Gates
   (`fmt`/`clippy`/`test`) green, CI on the PR green.
2. **Merge** the release PR (up to the user).
3. **Tag**: the user sets `git tag vX.Y.Z <merge-commit>` and pushes
   (`git push origin vX.Y.Z`). The agent doesn't push tags/`main` itself (§5).
4. The tag triggers **`.github/workflows/release.yml`**: `--release` build on
   `windows-latest` + `ubuntu-22.04` → archives `mindfork-rs-vX.Y.Z-x86_64-{windows.zip,
   linux.tar.gz}` (binary + README/CHANGELOG/LICENSE/install + dictionaries
   `data/dictionaries/`) + **Linux packages** (`nfpm` from `packaging/nfpm.yaml`:
   `mindfork-rs_X.Y.Z-1_amd64.deb`, `mindfork-rs-X.Y.Z-1.x86_64.rpm`,
   `mindfork-rs-X.Y.Z-1-x86_64.pkg.tar.zst`) + **Windows installer**
   (`mindfork-rs-vX.Y.Z-x86_64-setup.exe`, Inno Setup from `packaging/windows/mindfork.iss`)
   + `sha256sums.txt` → `gh release create` with notes = the `[X.Y.Z]` section from the CHANGELOG.
   Packaging changes (`packaging/**`) are validated on the PR by a separate `packaging.yml`
   (Linux: package build + install smoke in Ubuntu/Fedora/Arch containers; Windows:
   `.iss` compilation).
5. **Artifact smoke test**: download the archive, `mindfork-rs --version` (matches
   the tag), run the TUI on a copy of the data; optionally — install the package/
   installer in a VM.

Promotion to **`1.0.0`** — once the track is proven in production (CI green on
both OSes + migration scaffolding merged + release pipeline has shipped ≥1
release): from 1.0 "data survives updates" becomes a contractual promise.
