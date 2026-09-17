# Research: the release pipeline before the first public release

**Status:** measurements done, 2026-09-17; the §5 forks were decided by the user
the same day — **R1(a), R2(a), R3(a), R4(a), R5(a)**, each at its recommendation.
Stage 3 of [public-release-readiness.md](public-release-readiness.md) §4 — the
blockers B5, B6 and B12 plus the "Should" lines about licences, the supply chain
and the dependency graph. Stages 1 and 2 are merged.

What this stage is about: **what a stranger downloads, and what produced it.**
Everything here lives in `.github/`, `packaging/` and `Cargo.*`; the application's
own behaviour changes in exactly one way (the Windows binary's C runtime).

**Related:** AGENTS.md §6 (the release checklist),
[release-engineering.md](../history/release-engineering.md) (the process and its
original forks), [installers.md](../history/installers.md),
[inno-setup-7.md](inno-setup-7.md) (the pinning precedent),
[docs/journal/ci.md](../journal/ci.md), [docs/journal/release.md](../journal/release.md),
[docs/lessons.md](../lessons.md) §10.

## 1. What this stage covers

| # | Item | From |
|---|---|---|
| B5 | the Sonar job fails every fork and Dependabot pull request | §2.1 of the audit |
| B6 | a pushed tag publishes a live release unchecked; `nfpm` is unpinned and unverified | §2.1 |
| B12 | the Windows binary needs a Visual C++ redistributable | §2.2 |
| L1 | no third-party notices for the locked graph; the grammar licences reach no artifact | §2.3 "Licences" |
| L2 | `PRIVACY.md` is in the archives and the installer but not in the Linux packages | found here (§3.3) |
| S1 | every action is pinned by a mutable tag; no `dependabot.yml`; no build provenance | §2.3 "Supply chain" |
| S2 | two `deny.toml` ignores now have a fix; a yanked crate sits in the lock file | §2.3 "Supply chain" |

Out of scope, and why: repository settings (private vulnerability reporting,
secret scanning, rulesets, immutable releases) are B3 — the owner's, after the
flip; deleting the v0.9.0–v0.9.8 assets is B2, same; code signing waits on the
flip ([code-signing.md](code-signing.md)); the CHANGELOG's release-notes shape
and the version decision are stage 5.

## 2. Measurements

Run on this machine and against the GitHub API on 2026-09-17, before any design
was written (lessons §3).

### 2.1 The Windows binary and the C runtime

`target/release/mindfork.exe` (0.9.9, LTO, the release profile) imports
[measured]: `VCRUNTIME140.dll`, eleven `api-ms-win-crt-*` stubs, and fifteen
ordinary system libraries (`kernel32`, `user32`, `advapi32`, `crypt32`,
`bcryptprimitives`, `combase`, `ole32`, `oleaut32`, `shell32`, `userenv`,
`ws2_32`, `dbghelp`, `ntdll`…). The `api-ms-win-crt-*` family is the Universal
CRT, a Windows component since Windows 10 [docs]; `VCRUNTIME140.dll` is the
Visual C++ redistributable's, and a clean Windows does not carry it. So the gap
is exactly one DLL, and the app would fail to start before printing anything.

**A full release build with `-C target-feature=+crt-static`** [measured]: it
links. The graph's C dependencies (`onig` for syntect, `libsqlite3-sys`,
`sqlite-vec`) compile against the static runtime without a flag of their own —
the `cc` crate reads the target feature. The result imports **neither
`VCRUNTIME140.dll` nor any `api-ms-win-crt-*`**: fourteen system DLLs and
nothing else. `mindfork --version` and `mindfork llama installed` answer
normally. Size 26 808 KB against 26 443 KB — **+365 KB, +1.4 %**.

### 2.2 The dependency graph

- **`quick-xml`** is 0.39.4 and, as of syntect 5.3.0 taken with
  `default-features = false`, it is **ours alone**: `cargo tree --invert
  quick-xml` prints one edge, to `mindfork-rs` [measured]. The comment in
  `Cargo.toml:106` that pins the version to "the one syntect → plist resolves"
  describes a graph that no longer exists. Latest is 0.42.0; both advisories
  (RUSTSEC-2026-0194, -0195) are fixed from 0.41. Our call sites are three:
  `Reader`, `Event`, `escape::unescape` (`features/doc_extract.rs`).
- **`chacha20 0.10.1`** (yanked) arrives through `rand 0.10.2` ← `lopdf 0.42.0` ←
  `pdf-extract 0.12.0` [measured]; 0.10.2 is published, and nothing in the graph
  pins the yanked one. Our own `chacha20poly1305 0.10.1` holds `chacha20 0.9.1`,
  a separate line and not yanked.
- **`ttf-parser`** (RUSTSEC-2026-0192, unmaintained) has no successor in
  `pdf-extract`'s tree; the ignore stays, with its reasoning.
- The lock file holds **558 packages** [measured].

### 2.3 What a release actually ships

| File | Archives | deb/rpm/arch | Installer |
|---|---|---|---|
| README, CHANGELOG, LICENSE, DISCLAIMER, install.md, the two `.ru` files | yes | yes | yes |
| `PRIVACY.md` + `PRIVACY.ru.md` | yes | **no** | yes |
| dictionaries + `dictionaries/licenses/*.txt` | yes | yes | yes |
| `syntaxes/licenses/*.txt` — 22 grammar licences | **no** | **no** | **no** |
| notices for the 558 locked packages | **no** | **no** | **no** |

The dictionaries' licences travel everywhere, which is the pattern the other two
rows are missing. The grammars are compiled into the binary by `build.rs`, so
their licences have no natural neighbour on disk — the About dialog names them
(`credits.rs::GRAMMARS`, parsed from `syntaxes/SOURCES.md`) but carries no text.
For the crates the About dialog is equally truthful about the **direct**
dependencies (`COMPONENTS`, held against `Cargo.toml`/`Cargo.lock` by gate tests)
and equally silent about the texts and about everything transitive.

### 2.4 What the pipeline installs, and how it is verified

| Tool | Where | Pinned | Verified |
|---|---|---|---|
| Inno Setup 7.1.0 | `tools/install_inno.ps1` (release + packaging) | version | SHA-256 |
| zola 0.22.1 | `site.yml` | version | `gh attestation verify` |
| **nfpm** | `release.yml:75`, `packaging.yml:59` | **no** | **no** — `deb [trusted=yes]` |
| 8 distinct actions | all five workflows | mutable tag | — |

`[trusted=yes]` tells apt to accept the repository **without checking its
signature**, and the version installed is whatever `repo.goreleaser.com` serves
that day — into the job that builds the packages a stranger installs. Upstream
publishes `nfpm_2.47.0_Linux_x86_64.tar.gz`, and the GitHub API reports its
digest [measured], so the Inno pattern transfers directly.

The actions, by tag: `actions/checkout@v5`, `actions/upload-artifact@v5`,
`actions/download-artifact@v5`, `Swatinem/rust-cache@v2`,
`SonarSource/sonarqube-scan-action@v8` (receives `SONAR_TOKEN`),
`taiki-e/install-action@cargo-llvm-cov` (the tag is the *tool name* — a moving
alias), `EmbarkStudios/cargo-deny-action@v2`,
`aws-actions/configure-aws-credentials@v6` (assumes the site's deploy role with
`id-token: write`). Every one resolves to a commit through the API [measured].

### 2.5 Who gets the secrets

A workflow run from a **fork** receives no repository secrets [docs; also
"Verified solid" in the audit], and a **Dependabot** pull request runs with
Dependabot secrets rather than Actions secrets [docs] — so
`secrets.SONAR_TOKEN` is empty in both, and the scanner fails rather than
skipping, with `sonar.qualitygate.wait=true` on top. The job's only guard today
is `docs_only` (`ci.yml:295`). Nothing else in CI reads a secret.

## 3. Decisions taken without a fork

- **N1. The Sonar job is guarded on the head repository *and* on the actor.**
  Two conditions, because they are two different holes: a fork PR (`head.repo
  != github.repository`) and a Dependabot PR (same repo, different secret store).
  Everything else in `ci.yml` keeps running for both, so a fork PR is still
  gated by lints and the full test matrix. Consequence to carry into B3: GitHub
  counts a **skipped** job as a passing required check, which is what lets
  `SonarQube Cloud` stay required while fork PRs skip it — the same property the
  docs-only `Tests` skip already depends on.
- **N2. `nfpm` is installed by `tools/install_nfpm.sh`**, a pinned version and a
  SHA-256, shared by `release.yml` and `packaging.yml` — one file for the same
  reason `install_inno.ps1` is one file: the gate and the release must not drift
  onto different builders (inno-setup-7.md §4).
- **N3. Build provenance is added now and guarded by the repository's
  visibility.** Artifact attestations are free on a public repository and
  unavailable on a private one below Enterprise [docs], so an unguarded step
  would fail the next release; `if: github.event.repository.visibility ==
  'public'` turns it on at the flip with nothing left to remember.
- **N4. `quick-xml` to the latest that clears both advisories**, the two ignores
  deleted from `deny.toml` and the stale "pinned to syntect's" comment with them;
  `cargo update -p chacha20` for the yanked one. Both are lock-file moves with a
  test suite behind them, not judgement calls.
- **N5. `yanked = "deny"`** in `deny.toml` once the lock is clean, and the audit
  also runs on a pull request that touches `Cargo.toml`/`Cargo.lock`/`deny.toml`
  (~1 minute). On a public repository the first thing a stranger's PR can do is
  add a dependency; finding out weekly is finding out after the merge.
- **N6. `PRIVACY.md` and `PRIVACY.ru.md` join the Linux packages** (L2). The
  installer shows the policy and the archives carry it; a `deb` user reading
  `/usr/share/doc/mindfork-rs/` should not be the one person sent to the website.

## 4. What is being weighed, and the shape of the answers

The five questions below are in §5. Two of them (R1, R2) change what a user
downloads; the rest change how the pipeline is run and reviewed.

## 5. Forks

**Decided by the user, 2026-09-17: R1(a), R2(a), R3(a), R4(a), R5(a).**

**R1. Where `+crt-static` is spelled.**
(a) `.cargo/config.toml`, `[target.x86_64-pc-windows-msvc]` — every build on
every machine links the same way, and the 3288 tests run against the linkage the
release ships. A developer who exports `RUSTFLAGS` overrides the file wholesale
(cargo does not merge them), which is worth a comment in place.
(b) `RUSTFLAGS` in `release.yml`'s Windows build only — the artifact changes and
nothing else does, but then CI tests one linkage and users run another, which is
the shape of defect this project has been bitten by before.
**Recommendation: (a).**

**R2. Third-party notices — how they are produced.**
(a) **Generated at release time** by a pinned `cargo about` into
`THIRD-PARTY-NOTICES.md`, shipped in all three artifact kinds, with the same
generation run by `packaging.yml` (which already triggers on `Cargo.lock`) so a
dependency change that breaks it fails on the pull request. The file then always
matches the exact locked graph of the release it travels with.
(b) **Generated and committed**, with a `--check` gate in the `lint` job — the
pattern `wizard_rtf.py` and `site_legal_pages.py` already use. The repository
then states its notices, at the cost of ~1.5 MB of licence text in git and a
regenerated blob in every dependency bump.
(c) Defer the whole item to stage 5.
**Recommendation: (a)** — this is derived data of one build, and (b)'s gate buys
visibility of a file nobody reads in a diff.

**R3. What a tag publishes.**
(a) `gh release create --draft`, plus a guard that refuses before any build when
the tag is not `v<Cargo.toml version>` and when the CHANGELOG has no section for
it. AGENTS.md §6 step 5 (download the artifact, check `--version`, run it) then
happens **before** anyone can download it, and publishing is one click. A tag
with a prerelease suffix (`v0.9.9-rc1`) is allowed, marked `--prerelease`, and
falls back to the `[Unreleased]` notes — which is also what makes a **rehearsal**
possible (§6).
(b) Keep publishing immediately; add the version guard only.
**Recommendation: (a).**

**R4. Dependabot's scope and cadence.** Every pull request it opens spends
runner minutes (a Rust PR ≈ 20 billable minutes on this matrix).
(a) `github-actions` **weekly** (cheap, and the SHA pins of R-N/S1 stop being a
thing anyone updates by hand) + `cargo` **monthly, grouped** by minor/patch into
one pull request. Security advisories arrive through Dependabot alerts
regardless of the schedule, once B3 enables them.
(b) `github-actions` only — the Rust graph is watched weekly by `cargo-deny`, and
558 packages of bot noise is not what this repository needs on day one.
(c) Both weekly.
**Recommendation: (a).**

**R5. How the stage lands.**
(a) **One pull request**, one rehearsal tag. The rehearsal exercises the whole
pipeline at once, which is the only way it is ever run.
(b) **Two** — 3a: the workflow guards, the pins, `dependabot.yml`, the
dependency moves; 3b: `crt-static`, the notices, the licences in the artifacts.
Each is reviewable alone, but a rehearsal after each doubles the ~45 minutes of
runner time, and a rehearsal of 3a alone tests half a pipeline.
**Recommendation: (a).**

## 6. The rehearsal — what replaces a live run

This stage touches no engine, memory or tool path, so the `#[ignore]` smokes have
nothing to say about it (AGENTS.md §3). What it does touch is a workflow that
runs **once per release and nowhere else**, and reading YAML is not evidence
(lessons §10: validate by rendering, not by reading).

So the stage's live run is a **rehearsal tag**, pushed by the user (the agent
pushes no tags, AGENTS.md §5) at the branch head once CI is green:

```
git tag v0.9.9-rc1 <branch head> && git push origin v0.9.9-rc1
```

That is a real `release.yml` run: both builds, the packages, the installer, the
checksums, the notices, and a **draft prerelease** nobody can see. What is then
checked, and recorded in the journal entry:

1. the Windows binary in the archive imports no `VCRUNTIME140.dll`;
2. `THIRD-PARTY-NOTICES.md` and `licenses/` are in the archive, the `deb` and the
   installer, and `PRIVACY.md` is in the `deb`;
3. `nfpm` installed at the pinned version and hash, and the packages carry the
   version the tag names;
4. the guard's own arms: the same workflow refuses a tag that disagrees with
   `Cargo.toml` (checked by running the guard's script against stubs locally —
   lessons §10 — rather than by burning a run on it);
5. the attestation step is skipped while the repository is private, and says so.

Then the user deletes the draft and the tag. Cost: one release-shaped run
(~45 minutes of runner time, Windows billed 2x).

Locally, before any of that: the guard scripts are exercised against stubs, and
the whole test suite is run once under the static CRT if R1(a) is chosen.

## 7. Implementation plan

One branch, `feat/release-pipeline`, one pull request (R5a).

1. **`.cargo/config.toml`** — `+crt-static` for `x86_64-pc-windows-msvc` (R1a),
   with the RUSTFLAGS-override caveat written next to it. The whole suite is run
   once under it locally.
2. **`ci.yml`** — the Sonar guard (N1); the two new gates below join the `lint`
   job, which already runs six of them.
3. **`tools/release_guard.py`** — the tag against `Cargo.toml`, the notes out of
   the CHANGELOG, `prerelease` for a suffixed tag (R3a). It replaces the `awk` in
   `release.yml`, and carries a `--self-test` that runs its arms against
   fixtures, so the arms are exercised without spending a release run on them
   (lessons §10).
4. **`tools/install_nfpm.sh`** — the pinned version and hash (N2), called by
   `release.yml` and `packaging.yml`.
5. **`tools/actions_pin_check.py`** — every `uses:` is `owner/repo@<40 hex>`
   with the version in a trailing comment; the eight actions are pinned to the
   commits §2.4 resolved.
6. **`about.toml` + `about.hbs`** — `cargo about` (installed by the same
   `taiki-e/install-action` the coverage tool uses) writes
   `THIRD-PARTY-NOTICES.md` in `release.yml`; `packaging.yml` generates it too,
   so a dependency change fails on the pull request (R2a).
7. **The artifacts** — the notices and `syntaxes/licenses/*.txt` into the
   archives, the packages and the installer; `PRIVACY.md` + `PRIVACY.ru.md` into
   the packages (N6).
8. **`release.yml`** — the guard job before the builds, `--draft`
   (`--prerelease` on a suffixed tag), and the attestation step guarded by the
   repository's visibility (N3).
9. **`.github/dependabot.yml`** — `github-actions` weekly, `cargo` monthly and
   grouped (R4a).
10. **Dependencies** — `quick-xml` to the version that clears both advisories,
    `cargo update -p chacha20`, the two ignores out of `deny.toml`, `yanked =
    "deny"`, and the audit on dependency pull requests (N4, N5).
11. **Documents** — AGENTS.md §6 (the checklist gains "publish the draft"),
    `docs/install.md` if the Windows note about a runtime exists, the journal
    entries (`release.md`, `ci.md`), CHANGELOG, architecture §12, and this
    document's §8 with the rehearsal's outcome.

## 8. Outcome

**Rehearsal 1 — `v0.9.9-rc1`, 2026-09-17.** The guard accepted the tag, both
builds, the Linux packages and the notices came out right, and **the Windows
installer job failed** — which is the whole reason a rehearsal exists.

What it proved, from the run's own artifacts:

- **B12 on a real release artifact**: the `mindfork.exe` CI built imports neither
  `VCRUNTIME140.dll` nor any `api-ms-win-crt-*` — fourteen system DLLs, 26 796 KB.
- **The notices**: 359 KB, 215 licence sections, generated on the runner from the
  tag's own `Cargo.lock`, byte-identical to the copy inside the `.deb`.
- **The packages**: `/usr/share/doc/mindfork-rs/` carries `PRIVACY.md`,
  `PRIVACY.ru.md`, `THIRD-PARTY-NOTICES.md` (367 812 bytes) and
  `licenses/syntaxes/` — the three gaps of §2.3 closed in the artifact a stranger
  installs.
- **The pinned builder**: `Downloading nfpm 2.47.0 / SHA-256 verified /
  nfpm 2.47.0 at /usr/local/bin/nfpm`, in place of an unsigned repository.

**What broke, and why it is a real finding rather than a rehearsal artefact.**
Inno Setup aborted with `Value of [Setup] section directive "VersionInfoVersion"
is invalid`: Windows file metadata takes digits and dots, and the script fed it
`AppVersion` — `0.9.9-rc1`. So the prerelease mechanism §6 defines as *the* way to
exercise this workflow could never have reached the installer, the one artifact
whose payload is otherwise unverifiable from outside. The fix derives a numeric
`NumericVersion` for the two `VersionInfo*` fields while `AppVersion` keeps the
suffix for what the user sees and what names the file; the gate test
`credits::the_installer_and_the_binary_declare_the_same_product` now pins both the
fields and the derivation. Verified locally against the pinned Inno Setup 7.1.0,
compiling both `0.9.9-rc2` and `0.9.9`, with `PRIVACY.md`,
`THIRD-PARTY-NOTICES.md` and the 22 grammar licences in the payload.

Because the installer job failed, `release` was skipped and no draft was created —
the archives, the checksums, the draft flag and the skipped attestation are
unverified, so a second rehearsal follows.
