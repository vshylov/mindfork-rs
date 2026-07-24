# Research: Windows and Linux installers

**Status: track COMPLETE** (research 2026-07-15; decision points R1–R9 confirmed by
the user 2026-07-16 per the recommendations [rec.]; stages 1–3 implemented and verified,
2026-07-16). The plan moved to `docs/history/`. Outcome: **code prerequisites** (dictionary
fallback, locale-driven language, BOM — branch `feat/installed-mode-prereqs`), **Linux
packages** deb/rpm/pkg.tar.zst via nfpm (`feat/linux-packages`), **Windows installer** on
Inno Setup (`feat/windows-installer`) — all verified (Windows installer — a live run against
a real Inno Setup 6.7.3: compilation, the GUI wizard, `system`/`portable`/`path` modes with
Cyrillic-path escaping, a round trip of reading via the binary, an upgrade, uninstall; during
the live run, a `{app}` crash in `ShouldSkipPage` was found and fixed). **Stage 4 "Code
signing" deferred** by the user's decision — the repository is private, there's no site/logo/
icon yet; revisit when preparing for a public launch (that will open path (a) SignPath
Foundation). Groundwork: a winget manifest (portable zip until signing), an AUR
`mindfork-rs-bin`, an MSI for GPO/Intune if demand arises.

The order was: **Windows** installers (msi or exe) and **Linux** (deb, rpm, pkg.tar.zst);
where the format supports it, installation lets the user pick the **interface language**
and the **user-data location** (see `defaults.json`); an open question — whether the
Windows binary and installer need to be signed with a certificate and where to get one
(§5).

Related documents: [docs/history/release-engineering.md](../history/release-engineering.md)
(the release pipeline; installers are its groundwork item §5), [docs/install.md](../install.md)
(the portable install today), [spec §5.2, §12.1](../../spec.md) (data location,
`defaults.json`), [docs/roadmap.md](../roadmap.md) ("Auto-update / installers"). Web facts
cross-checked by three parallel surveys (Windows tools, Linux packaging, code signing)
against primary sources — key links in §10.

---

## 1. The task and current state

Today's release (`release.yml`, tag `v*`) builds **portable archives**
`mindfork-rs-vX.Y.Z-x86_64-{windows.zip,linux.tar.gz}` (the binary + docs + dictionaries in
`data/dictionaries/`) + `sha256sums.txt`. There's no installation as such: the user unpacks
the archive, data lives next to the binary (portable mode by default).

`defaults.json` next to the binary ([shared/paths.rs](../../src/shared/paths.rs)) is already
designed **for an installer** (a code comment: "the installer will fill it in per the user's
choice at install time"):

- `mode` — the data-storage mode: `portable` (a `data/` subdirectory next to the binary,
  the default) / `system` (Windows `%APPDATA%\mindfork-rs\data`, Linux
  `~/.local/share/mindfork-rs`) / `path` (an arbitrary directory);
- `default_language` — the language new profiles are created with (axis A), **and** the
  interface language on a fresh install (no `settings.json` → `config.interface.language`
  is taken from here, `main.rs`), and the CLI language before settings exist. One choice at
  install time covers all of this — exactly what the order asks for ("choose the interface
  language").

Installers are, essentially, delivering the binary + dictionaries + a **correct
`defaults.json`** + registering with the system (Add/Remove Programs, an uninstaller,
upgrades).

---

## 2. Application facts that drive the install design

1. **The portable default is incompatible with a system-wide install.** With no
   `defaults.json`, the app writes data to `exe_dir/data` — impossible in
   `C:\Program Files\…` or `/usr/bin` (no permissions) → `ensure_dirs` fails at startup. So
   every installer/package **must** place `defaults.json` next to the binary (typically
   `{"mode":"system"}`).
2. **Spellcheck dictionaries load only from the data directory**
   (`paths.dictionaries_dir()` = `<root>/dictionaries`,
   [features/spellcheck/dict.rs](../../src/features/spellcheck/dict.rs)). In portable mode
   `root = exe_dir/data`, and the release archive puts dictionaries there. With
   `mode=system` root is a user folder that a package can't place files into (a Linux
   package even less so, for all users at once). Without a fix, spellcheck in an installed
   setup is **silently disabled**. → Prerequisite P1 (§6): a fallback dictionary lookup in
   `exe_dir/data/dictionaries` (the portable layout as a source of read-only resources).
3. **The binary's dependencies are trivial**: reqwest on `rustls` (no OpenSSL), rusqlite
   `bundled` (SQLite inside), arboard on `x11rb` (pure Rust). The Linux binary effectively
   only depends on glibc (+libgcc) — package Depends can be written in one line.
4. **Path resolution goes through `std::env::current_exe()`.** On Linux this is
   `/proc/self/exe` — it returns the path of the **real file**, not the symlink the program
   was launched through ([rust-lang/rust#43617](https://github.com/rust-lang/rust/issues/43617)).
   This legitimizes the layout "the real binary + sidecars in `/usr/lib/<pkg>/`, a symlink
   from `/usr/bin`" **with no code changes** (§4.2).
5. **`defaults.json` must survive an upgrade** — it's the user's choice at install time; a
   reinstall/update must not overwrite it.
6. **Uninstall doesn't touch user data** (`%APPDATA%`/XDG) — only what was installed gets
   removed. Portable data (`{app}\data`) isn't removed by Inno either way (it only removes
   what it installed).
7. **`Defaults::read` today doesn't tolerate a UTF-8 BOM** (a whitespace check + a direct
   `serde_json::from_slice`) — a file written "with a BOM" (some editors, Inno's Pascal
   helpers) would crash the startup. The project already drops the BOM in two places
   (`rag_ingest::read_text`, the LameLLaMA importer) → Prerequisite P3 (§6).

---

## 3. Windows: exe (Inno Setup) vs. msi (WiX)

### 3.1 Landscape (July 2026)

| Tool | Version | Status | On GHA `windows-latest` |
|---|---|---|---|
| **Inno Setup 6** | 6.7.3 (2026-05) | active; free for OSS (since 6.5.0 there's an *optional* commercial license) | **yes** (6.7.1; more reliable to install via `choco install innosetup`) |
| Inno Setup 7 | 7.0.2 (2026-07-13) | just released; "full backward compatibility" with 6 | no |
| **WiX Toolset** | v7.0.0 (2026-04) | active; **v3/v4 — EOL since 02.2025**; since v6 — an Open Source Maintenance Fee (an EULA fee for paying users) | only **v3.14** (the EOL line) |
| NSIS | 3.11 (2025-03) | alive, infrequent releases | no (removed from the windows-2025 image) |
| cargo-wix | 0.3.9 (2025-03) | alive; targets WiX v3 by default | — |
| cargo-dist | 0.32.0 (2026-05) | releases keep shipping, but axo.dev has wound down (the domain is for sale) — a maintenance risk | — |
| cargo-packager | 0.11.8 (2024-11) | **stalled for ~1.5 years** | — |

Rust wrappers (cargo-dist / cargo-packager / tauri-bundler) produce **templated**
installers: custom wizard pages "language + data folder" and writing `defaults.json` per the
user's choice aren't expressible through them (or they'd require writing the whole
.nsi/.wxs template by hand — the wrapper's benefit disappears). Not suitable as the primary
tool.

### 3.2 Our five "special" requirements, line by line

| Requirement | Inno Setup | WiX/MSI |
|---|---|---|
| An "application language" page (radio buttons) | built in: `CreateInputOptionPage(Exclusive:=True)` | a custom Dialog + RadioButtonGroup + editing the WixUI publish graph |
| A "data folder" page (radio + a folder picker for a folder ≠ the install directory) | built in: `CreateInputDirPage` | painful: `BrowseDlg` via an indirect `_BrowseProperty` + a dummy Directory entry |
| Writing `defaults.json` per the choice | `[Code]`: `SaveStrings…File` in `ssPostInstall` | a deferred CustomAction (PowerShell/cmd/DTF) or a third-party extension; there's no built-in `util:JsonFile` ([wix#7711](https://github.com/orgs/wixtoolset/discussions/7711)) |
| Don't overwrite on upgrade | `if not FileExists(...)` — one line | extra logic in a CustomAction against MSI's component rules |
| A bilingual installer UI (ru+en) | built in: `[Languages]` + `ShowLanguageDialog`; **Russian.isl — an official translation** (since 6.5.0) | MSI is single-culture: either 2 msi's or a trick embedding language transforms (torch); no built-in support ([wix#7544](https://github.com/wixtoolset/issues/issues/7544)) |

A plus for Inno: a per-user install with no UAC (`PrivilegesRequired=lowest`,
`{autopf}` → `%LOCALAPPDATA%\Programs`), a "for me / for everyone" dialog
(`PrivilegesRequiredOverridesAllowed=dialog`), a silent mode
(`/VERYSILENT /DIR= /LANG=` + custom `{param:…}`), upgrading under the same `AppId` with
`UsePrevious*`. winget accepts `InstallerType: inno` and already knows its silent flags.

**Effort estimate: Inno — 1–2 days for the script + CI; MSI — days to weeks** (UI dialogs,
a CustomAction with rollback, multilingualism via transforms, dual-context per-user is
finicky, and the fork "EOL v3 free vs. v6/v7 with a Maintenance-Fee EULA").

### 3.3 Recommendation and design sketch (R1, R2)

**Inno Setup 6.7.x, exe format** (R1). Add MSI only if there's real demand for
enterprise delivery (GPO/Intune) — groundwork. NSIS — a fallback option with no advantages.
(The winget nuance for an unsigned exe — see §5.4.)

Sketch of `packaging/windows/mindfork.iss`:

- `[Setup]`: `AppId={{…GUID…}}`, `DefaultDirName={autopf}\mindfork-rs`,
  `PrivilegesRequired=lowest`, `PrivilegesRequiredOverridesAllowed=dialog`,
  `ArchitecturesInstallIn64BitMode=x64compatible`; version — `/DAppVersion=X.Y.Z` from
  the tag in CI; `OutputBaseFilename=mindfork-rs-vX.Y.Z-x86_64-setup`.
- `[Languages]`: `en` + `ru` (the official `Russian.isl`; in case it's absent from the
  GHA distribution, we vendor the .isl into `packaging/windows/`).
- `[Files]`: `mindfork-rs.exe`, `README/CHANGELOG/LICENSE/install.md` → `{app}`;
  dictionaries → `{app}\data\dictionaries` (the portable layout as a source of read-only
  resources, P1).
- **Page 1 "Application language"** (radio buttons: Russian / English — the installer's
  language as the default). Written to `default_language`.
- **Page 2 "Where to store data"** (radio buttons): "Standard user folder
  (recommended)" → `system`; "Portable, next to the program" → `portable`
  (**hidden for a per-machine install** — Program Files isn't for data); "Another
  folder…" → `path` + `CreateInputDirPage`.
- Both pages **are skipped on an upgrade** (`ShouldSkipPage`, if `{app}\defaults.json`
  already exists).
- `CurStepChanged(ssPostInstall)`: if `defaults.json` doesn't exist — assemble the JSON
  (escaping `\` → `\\` in the path) and write it as UTF-8 (`SaveStringsToUTF8File`; the BOM
  is neutralized by prerequisite P3).
- Silent mode: `/VERYSILENT /LANG=russian /DataMode=system|portable|path
  /DataDir="…" /AppLang=ru|en` (read via `{param:…}`; defaults — system + the installer's
  language) — also needed for winget.
- Uninstall: removes `{app}` (the binary, dictionaries, `defaults.json`); doesn't touch
  data in `%APPDATA%`/a custom path; Inno doesn't remove portable `{app}\data` on its own
  (it didn't install them) — mention this on the final page/in the README.

### 3.4 CI

A new job in `release.yml` (after `build`): a windows runner → download the binary
artifact → `iscc packaging\windows\mindfork.iss /DAppVersion=%VERSION%` → an artifact
`…-setup.exe` → into the shared `release` job (the archives stay — the portable scenario
isn't going anywhere, it's our default flavor). `sha256sums.txt` covers the new artifacts
automatically.

---

## 4. Linux: deb + rpm + pkg.tar.zst

### 4.1 Tools (July 2026)

| Tool | Version | Formats | Auto-Depends | Note |
|---|---|---|---|---|
| **nfpm** (goreleaser) | v2.47.0 (2026-06) | **deb, rpm, archlinux (.pkg.tar.zst)**, apk, ipk | no (by hand) | one YAML → all formats; `type: symlink`/`config|noreplace` out of the box; a single Go binary |
| cargo-deb | 3.7.0 (2026-05) | deb | yes (`dpkg-shlibdeps`) | metadata in Cargo.toml; `--no-build` for a ready binary |
| cargo-generate-rpm | 0.21.0 (2026-05) | rpm | yes (`--auto-req`) | no rpmbuild, pure Rust |
| cargo-aur | 1.7.1 (2024-03) | a PKGBUILD (-bin) | — | only an AUR recipe, not a .pkg.tar.zst |

**Recommendation (R4): one nfpm for all three formats.** Arguments: we already have a
prebuilt binary from `release.yml` (exactly what nfpm is for); the **identical layout**
across all three formats is described once (including archlinux, which the cargo tools
don't cover); auto-Depends isn't needed — the "rustls + bundled SQLite + x11rb" stack
narrows dependencies down to `libc6 (>= 2.35)` in deb (rpm/arch need not declare it at
all). The alternative "cargo-deb + cargo-generate-rpm (+ something for arch)" is
legitimate for Cargo.toml metadata and honest `dpkg-shlibdeps`, but that's three configs
instead of one for a one-line payoff.

### 4.2 Package layout: `defaults.json` can't go in `/usr/bin`

FHS 3.0/Debian Policy forbid data in `/usr/bin` (a flat namespace for executable commands;
a generic name like `defaults.json` there is unthinkable). A proven pattern:

```
/usr/lib/mindfork-rs/mindfork-rs          the real binary
/usr/lib/mindfork-rs/defaults.json        {"mode":"system"} (config|noreplace)
/usr/lib/mindfork-rs/data/dictionaries/   dictionaries (.aff/.dic; read via P1)
/usr/bin/mindfork-rs -> ../lib/mindfork-rs/mindfork-rs    (type: symlink in nfpm)
/usr/share/doc/mindfork-rs/               README, CHANGELOG, LICENSE, install.md
```

Works **with no changes to path-resolution code**: `current_exe()` on Linux returns the
symlink's target (§2 item 4) → `exe_dir = /usr/lib/mindfork-rs` → `defaults.json` is
found → data goes into `~/.local/share/mindfork-rs`. Debian Policy §9.1.1 explicitly
permits a `/usr/lib/<pkg>` subdirectory with mixed (including architecture-independent)
content; a precedent for "vendor defaults as data under /usr/lib" is `/usr/lib/os-release`.
`defaults.json` is marked `config|noreplace` (nfpm: deb-conffile / rpm
`%config(noreplace)`) — a user edit survives an upgrade. Dictionaries go into
`data/dictionaries` next to the binary — the same portable layout as the zip and the
Windows installer (one invariant across all platforms, P1).

Alternative (R5b): the binary stays in `/usr/bin`, and the app learns to read
`/etc/mindfork-rs/defaults.json` (+ dictionaries from `/usr/share/mindfork-rs/`).
FHS-cleaner and a "proper" conffile, but adds a platform-specific branch to path resolution
and a second lookup mechanism — while the symlink layout is fully standard. Not
recommended.

### 4.3 There are no interactive choices on Linux

deb/rpm/pacman install **non-interactively** (debconf — for system configuration and not
really meant for per-user preferences; rpm/pacman have no such mechanism at all). The
order's requirement "choose language and location" doesn't apply here — packages fix
`{"mode":"system"}`, and the language is decided by the app on first launch. Today "no
`default_language` in `defaults.json`" = `ru` (the serde default) — a bad default for an
international user of deb/rpm. → Prerequisite P2 (§6): auto-detect the language from the
OS locale (also closes the roadmap groundwork item "Detect language from the system OS
locale").

### 4.4 The glibc baseline (R9)

A build on `ubuntu-22.04` = glibc **2.35**: Ubuntu 22.04+/Debian 12+/Fedora 36+/RHEL 10 —
yes; **RHEL/Rocky/Alma 9 (glibc 2.34) — no**. Recommendation: accept and declare this
(the niche of a TUI app — Fedora/Ubuntu/Arch desktops; an EL9 desktop is exotic).
Alternatives if EL9 is needed: `cargo-zigbuild` pinned to
`x86_64-unknown-linux-gnu.2.34` (not verified) or a static musl build (careful: musl's
allocator degrades multithreaded tokio several-fold — mimalloc is needed; crossterm
doesn't need terminfo, so nothing about static musl gets in the way there).

### 4.5 No package signing needed; checksums already exist

`dpkg -i`/`apt install ./x.deb` don't verify signatures at all (trust in the deb world
lives at the repository level; `dpkg-sig` was removed from Debian 12+ entirely); `rpm`
only verifies with an imported key; `pacman -U` with the default
`LocalFileSigLevel = Optional` installs unsigned packages. OSS practice for GitHub
Releases is checksums (we already have `sha256sums.txt`; GitHub itself shows asset
digests since 2025). Optional and cheap: **GitHub Artifact Attestations**
(`actions/attest-build-provenance`, Sigstore provenance, free for public repos;
verified via `gh attestation verify`). Not worth setting up GPG signing until we have our
own apt/dnf repository.

### 4.6 Arch: .pkg.tar.zst + AUR as groundwork

nfpm builds an archlinux package that installs via `pacman -U` (no systematic complaints
in nfpm's tracker; nfpm itself is distributed via AUR as `nfpm-bin`). Before the first
release — a one-off smoke test in an `archlinux:latest` container (easy to automate in CI
— §8, DoD). **The idiomatic channel for Arch is still AUR** (`mindfork-rs-bin`: a
PKGBUILD + .SRCINFO pointing at GitHub Releases; the `-bin` suffix is mandatory under AUR
rules; updates reach the user via an AUR helper, and the PKGBUILD's `sha256sums` also
verify our artifacts) — a separate small groundwork item after the first package release.

---

## 5. Windows code signing (the order's open question)

### 5.1 What happens without signing

Two layers of friction: the browser (Edge/Chrome "isn't commonly downloaded" → Keep → Keep
anyway) and launching (Mark-of-the-Web → SmartScreen "Windows protected your PC", the
"Run anyway" button hidden behind "More info", the publisher shown as "Unknown
publisher"). Key mechanics (Microsoft, 2026): **without signing, reputation accrues
against the file hash and resets with every release**; with signing, it accrues against
the certificate and carries across releases. The threshold isn't published ("a few weeks
and hundreds of clean installs"). A tightening: **Smart App Control** on recent Windows 11
blocks unsigned binaries with **no** bypass button. For a niche TUI audience
(developers), launching unsigned is standard practice, but every release will be
"yellow."

### 5.2 The 2026 landscape: what's changed

- Since 06.2023 (CA/B Forum) a signing key must live in FIPS-certified hardware/an HSM —
  "just a .pfx in CI secrets" no longer exists; since 03.2026 the maximum certificate
  lifetime is **460 days** (annual renewal is the norm).
- **EV no longer gives instant SmartScreen reputation** — Microsoft has explicitly
  documented this (a change ~2024): "Paying a premium for EV solely to avoid SmartScreen
  warnings is no longer justified." OV = EV from SmartScreen's point of view. EV is also
  only sold to legal entities.
- Timestamping (RFC 3161) is always mandatory — the signature survives after the
  certificate expires.

### 5.3 Options (where to get a certificate)

| Option | Price/year | Available to | CI (GitHub Actions) | Notes |
|---|---|---|---|---|
| **No signing** | 0 | everyone | — | reputation from scratch every release; Smart App Control blocks it; for winget see §5.4 |
| **SignPath Foundation** | **0** | **public OSS projects** (an OSI license, a public repo, releases, MFA, a "code signing policy" on the project page) | an official Action; only builds from a trusted CI get signed; **manual approval of every release** | the publisher shown in dialogs is "SignPath Foundation" (not your name); **officially recommended by Microsoft** for OSS |
| **Certum Open Source** | ~€69 the first year, **~€29 renewal** (+€35 for a physical card, or the SimplySign cloud — without it) | **individuals from almost any country** (video verification, ~2 days) | possible, but hacky (SimplySign: an interactive OTP → a TOTP script/container; a session ~2 h) | subject: "Open Source Developer, &lt;Name&gt;"; a limit of 5000 signatures/month; the cheapest "your own" certificate |
| **Azure Artifact Signing** (formerly Trusted Signing; GA 01.2026) | $9.99/mo (≈$120/year) | **individuals: US/Canada only**; organizations: US/Canada/EU/UK (+ a paid Azure subscription) | excellent: an official Action, OIDC, signs anything signtool can | short-lived certificates from a managed HSM; identity = your name; geography is promised to expand — recheck |
| Commercial OV | ~$150–300 | individuals/organizations, worldwide | a token — self-hosted only; cloud (eSigner/KeyLocker) — yes, at extra cost | reputation accrues, initial warnings will still appear |
| EV | ~$280–700 | legal entities only (D-U-N-S) | same as OV | **no longer has an advantage over OV** — don't buy |

### 5.4 The winget nuance

winget **doesn't require** signing (manifests pin SHA256, validation runs antivirus
scans), but it also **doesn't bypass** SmartScreen: a recent case (halloy, 06.2026) —
`winget install` of an unsigned **Inno exe** hangs on a SmartScreen block; this doesn't
happen with MSI/portable-zip. Practical conclusion: until signing is in place for
winget, publish a **portable zip** (type `zip`/`portable` — we already have this), not
an inno-exe; once signed — an exe works too.

### 5.5 Recommendation (R8)

Signing is **not a blocker** for the first installer releases, but is desirable as a
separate stage. Decision tree:

1. The repository is public (or we're willing to make it public) → **SignPath Foundation**:
   free, recommended by Microsoft; the cost — "SignPath Foundation" instead of a personal
   name in dialogs + manual release approval + process requirements (MFA, a signing policy
   on the project page).
2. A certificate is needed **in your own name**, an individual outside the US/Canada →
   **Certum Open Source** (~€69/€29) — the only cheap path; automation in CI is possible,
   but via workarounds.
3. **Azure Artifact Signing** — the best price/automation, **as soon as** it's available
   for the relevant geography (an individual in the US/Canada, or an organization in
   EU/UK/US/Canada); recheck the regional status.
4. Don't buy EV. In any variant: an RFC3161 timestamp, sign every release with one
   identity, sign both `mindfork-rs.exe` and the setup itself (with Inno, a
   `SignTool=` directive signs the uninstaller too).

---

## 6. Prerequisites in the app's code (small, ahead of the installers)

- **P1. A dictionary fallback onto the portable layout next to the binary.**
  `dict::load` takes one directory; add a second source —
  `exe_dir/data/dictionaries` (when root ≠ `exe_dir/data`): pairs whose base name isn't
  found in the data directory are loaded from the exe directory (a user dictionary of the
  same name wins). One invariant across all platforms: the release zip, the Windows
  installer, and Linux packages all place dictionaries the same way. (~30 lines +
  `Paths::bundled_dictionaries_dir()` + tests.)
- **P2. Auto-detect the language from the OS locale** when `default_language` isn't set
  explicitly: `Defaults.default_language: Lang` → `Option<Lang>`; `None` → detect from the
  locale (crate `sys-locale`, tiny and clean; `ru*` → `Ru`, else `En`; extensible to
  Tier 3's external locales). Only changes the behavior of fresh installs and new profiles
  when the field is absent (today — always `Ru`); the Windows installer writes the
  language explicitly, Linux packages and a bare zip get a sensible default. Closes the
  roadmap groundwork item "Detect language from the OS system locale."
- **P3. Make `Defaults::read` tolerant of a UTF-8 BOM** (drop it before parsing —
  precedents `rag_ingest::read_text`, the LameLLaMA importer). Guards against Inno's
  Pascal helpers and manual file edits by editors that write a BOM. (2 lines + a test.)

All three are additive, no migrations (ADR 0006 isn't touched).

---

## 7. Decision points (confirm before implementation)

- **R1. Windows installer format.**
  - (a) **[rec.]** Inno Setup 6.7.x (exe): all five special requirements are built-in
    features, 1–2 days of work, an official Russian translation, free for OSS, available
    on GHA. MSI — groundwork if there's demand for GPO/Intune.
  - (b) WiX MSI: the "real" Windows installer format, but every one of our requirements
    is a workaround (custom dialogs, JSON via a CustomAction, multilingualism via
    transforms), days-to-weeks of work + the fork EOL-v3/paid-v6.
  - (c) Both at once — double the maintenance cost with no demand for it.
- **R2. Windows install mode.**
  - (a) **[rec.]** per-user by default (`PrivilegesRequired=lowest`, no UAC,
    `%LOCALAPPDATA%\Programs`) + a "for me / for everyone" dialog
    (`…OverridesAllowed=dialog`). With per-machine, the "portable" option is hidden.
  - (b) per-machine only (Program Files, UAC) — the portable data option becomes
    unavailable, with no benefit.
- **R3. What the installer writes to `default_language`.**
  - (a) **[rec.]** a separate "application language" page, defaulting to the installer's
    language (the installer's language choice ≠ a mandatory app-language choice, but a
    good default).
  - (b) silently take the installer's language with no page — fewer clicks, but an
    implicit choice.
- **R4. Linux packaging tool.**
  - (a) **[rec.]** nfpm: one YAML → deb+rpm+archlinux, an identical layout,
    symlink/config|noreplace out of the box, very active maintenance.
  - (b) cargo-deb + cargo-generate-rpm (+ a separate solution for arch): metadata in
    Cargo.toml and auto-Depends, but three configs and arch isn't covered.
- **R5. Linux package layout.**
  - (a) **[rec.]** `/usr/lib/mindfork-rs/` (the binary + `defaults.json` +
    `data/dictionaries`) + a symlink `/usr/bin/mindfork-rs` — zero changes to
    path-resolution code, Policy-compatible.
  - (b) the binary in `/usr/bin` + the app learns to read
    `/etc/mindfork-rs/defaults.json` and `/usr/share/mindfork-rs/` — FHS purism at the
    cost of a second lookup mechanism in the code.
- **R6. Language for a non-interactive install (P2).**
  - (a) **[rec.]** `default_language: Option<Lang>` + auto-detect from the OS locale on
    `None` (Linux packages and the zip get the system's language; closes the roadmap
    groundwork item).
  - (b) Linux packages write a fixed `"en"` — predictable, but a Russian-language system
    gets an English first profile.
  - (c) leave it as-is (`ru` when the field is absent) — a bad default for international
    packages.
- **R7. Dictionaries in an installed setup (P1).**
  - (a) **[rec.]** a fallback directory `exe_dir/data/dictionaries` (read-only resources
    next to the binary; packages and the installer place dictionaries there).
  - (b) copy dictionaries into the data directory on first launch — duplicates on disk,
    copies going stale.
  - (c) don't fix it — spellcheck stays silently disabled in an installed setup (current
    behavior).
- **R8. Windows signing.**
  - (a) **[rec. if the repository is public]** SignPath Foundation (free, a Microsoft
    recommendation; the publisher shows as "SignPath Foundation", manual release
    approval).
  - (b) **[rec. otherwise / "your own name"]** start with no signing, as a separate
    stage — Certum Open Source (~€69/€29, an individual from any country); publish a
    portable zip in winget until signing is in place, not the inno-exe (§5.4).
  - (c) Azure Artifact Signing $9.99/mo — once it clears geographically (an individual in
    the US/Canada; an organization +EU/UK); recheck the status.
  - (d) commercial OV (~$150–300) / EV — EV gives nothing extra, don't get it.
- **R9. glibc baseline for deb/rpm.**
  - (a) **[rec.]** keep `ubuntu-22.04` (glibc 2.35), honestly declaring "RHEL/Rocky 9 not
    supported" (as install.md already does).
  - (b) `cargo-zigbuild` pinned to glibc 2.34 for EL9 (not verified).
  - (c) a static musl build (needs mimalloc against allocator degradation).

Micro-decisions I propose locking in without a decision point (say if this is wrong): the
choice pages are skipped on an upgrade (`defaults.json` exists); the uninstaller doesn't
touch data; artifacts are named `mindfork-rs-vX.Y.Z-x86_64-setup.exe` + conventional
package names (`mindfork-rs_X.Y.Z-1_amd64.deb`, `mindfork-rs-X.Y.Z-1.x86_64.rpm`,
`mindfork-rs-X.Y.Z-1-x86_64.pkg.tar.zst`); everything ends up in `sha256sums.txt`.

---

## 8. Stage plan (after decision points are confirmed; a stage = a branch/PR)

| Stage | Branch | Contents | DoD |
|---|---|---|---|
| 1. Code prerequisites | `feat/installed-mode-prereqs` | P1 (dictionary fallback) + P2 (`Option<Lang>` + locale auto-detect) + P3 (BOM) + docs (install.md §2.1, spec §5.2) | unit tests; a manual smoke: the binary + `defaults.json {"mode":"system"}` in a clean directory → dictionaries picked up from `data/`, language from the locale |
| 2. Linux packages | `feat/linux-packages` | `packaging/nfpm.yaml` (layout §4.2), a job in `release.yml` (nfpm → deb/rpm/archlinux → artifacts + sha256) | **CI install smoke**: containers `ubuntu:24.04` (`apt install ./…deb`), `fedora:latest` (`dnf install ./…rpm`), `archlinux:latest` (`pacman -U`) → `mindfork-rs --version` as a regular user; `defaults.json` visible through the symlink |
| 3. Windows installer | `feat/windows-installer` | `packaging/windows/mindfork.iss` (design §3.3), a job in `release.yml` (iscc → setup.exe) | a manual smoke on Windows: a fresh per-user install (both pages, all 3 data modes), an upgrade on top (pages skipped, `defaults.json` intact), a silent install, uninstall doesn't touch data |
| 4. (opt.) Signing | `feat/windows-signing` | per R8: a SignPath/Certum integration in `release.yml` (signing the exe + setup, timestamp) | a signed artifact: `signtool verify /pa`, file properties show the publisher |
| 5. (groundwork) | — | a winget manifest (zip until signing, §5.4), AUR `mindfork-rs-bin`, GitHub Artifact Attestations, MSI on demand, apt/COPR repositories | — |

The track needs no live engine run (the engine/memory/tools aren't touched) — install
smokes from the DoD stand in for it. Docs per AGENTS.md §4: install.md (new install
methods), README (badges/links), roadmap (groundwork item closed), CHANGELOG
(`[Unreleased]` → Added), the CLAUDE.md journal.

---

## 9. Out of scope

- Auto-update (self-update) and an "a new version is available" notice in the TUI — a
  separate track (release-engineering §5).
- macOS (dmg/homebrew), arm64 builds, flatpak/snap/AppImage.
- Our own repositories (an apt PPA, a dnf COPR, a pacman repo) — GPG then too.
- Installer localization beyond ru/en (Inno supports 30+ languages — a couple of lines to
  add once external app locales exist for those languages).

---

## 10. Key sources (checked 2026-07-15)

**Windows tools:** [Inno Setup downloads](https://jrsoftware.org/isdl.php) ·
[official translations (Russian.isl)](https://jrsoftware.org/files/istrans/) ·
[CreateInputOptionPage](https://jrsoftware.org/ishelp/topic_isxfunc_createinputoptionpage.htm) /
[CreateInputDirPage](https://jrsoftware.org/ishelp/topic_isxfunc_createinputdirpage.htm) /
[script events](https://jrsoftware.org/ishelp/topic_scriptevents.htm) ·
[PrivilegesRequiredOverridesAllowed](https://jrsoftware.org/ishelp/topic_setup_privilegesrequiredoverridesallowed.htm) ·
[Setup Command Line](https://jrsoftware.org/ishelp/topic_setupcmdline.htm) ·
[Inno commercial licenses (6.5.0+)](https://jrsoftware.org/isorder.php) ·
[WiX releases](https://github.com/wixtoolset/wix/releases) ·
[EOL WiX v3/v4](https://www.firegiant.com/blog/2025/2/6/wix-v3-and-wix-v4-are-no-longer-in-community-support/) ·
[WiX Maintenance Fee](https://www.firegiant.com/blog/2025/4/7/wix-v600-available/) ·
[no util:JsonFile](https://github.com/orgs/wixtoolset/discussions/7711) ·
[multilingual MSI — WIP](https://github.com/wixtoolset/issues/issues/7544) ·
[GHA windows-2025 contents](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md) ·
[cargo-wix](https://github.com/volks73/cargo-wix/releases) ·
[cargo-dist](https://github.com/axodotdev/cargo-dist/releases) ·
[cargo-packager](https://github.com/crabnebula-dev/cargo-packager/releases) ·
[winget: manifests/types](https://learn.microsoft.com/en-us/windows/package-manager/package/manifest)

**Linux packaging:** [nfpm: configuration/formats](https://nfpm.goreleaser.com/docs/configuration/) ·
[nfpm releases](https://github.com/goreleaser/nfpm/releases) ·
[cargo-deb](https://github.com/kornelski/cargo-deb) ·
[cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm) ·
[FHS 3.0 /usr/bin](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/ch04s04.html) /
[/usr/lib](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/ch04s06.html) ·
[Debian Policy §9.1.1](https://www.debian.org/doc/debian-policy/ch-opersys.html) ·
[current_exe resolves a symlink on Linux](https://github.com/rust-lang/rust/issues/43617) ·
[AUR submission guidelines (-bin)](https://wiki.archlinux.org/title/AUR_submission_guidelines) ·
[debconf-devel(7): not a registry](https://manpages.debian.org/unstable/debconf-doc/debconf-devel.7.en.html) ·
[Securing Debian: signing a deb](https://www.debian.org/doc/manuals/securing-debian-manual/deb-pack-sign.en.html) ·
[pacman SigLevel](https://wiki.archlinux.org/title/Pacman/Package_signing) ·
[Artifact Attestations GA](https://github.blog/changelog/2024-06-25-artifact-attestations-is-generally-available/) ·
[glibc RHEL10/2.39](https://lwn.net/Articles/1021827/) ·
[the musl allocator and performance](https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance/)

**Code signing:** [SmartScreen reputation (MS, 2026)](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) ·
[Code signing options (MS, 2026; recommends SignPath for OSS)](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options) ·
[Artifact Signing FAQ (geography)](https://learn.microsoft.com/en-us/azure/artifact-signing/faq) ·
[azure/artifact-signing-action](https://github.com/Azure/trusted-signing-action) ·
[SignPath Foundation: terms](https://signpath.org/terms.html) ·
[SignPath GitHub Action](https://github.com/SignPath/github-action-submit-signing-request) ·
[Certum Open Source](https://www.certum.eu/en/code-signing-certificates/) ·
[hands-on Certum 10.2025 (pricing)](https://piers.rocks/2025/10/30/certum-open-source-code-sign.html) ·
[CA/B: hardware keys since 06.2023](https://cabforum.org/working-groups/code-signing/requirements/) ·
[CSC-31: 460 days since 03.2026](https://cabforum.org/2025/11/17/ballot-csc-31-maximum-validity-reduction/) ·
[EV doesn't bypass SmartScreen (ToDesktop)](https://www.todesktop.com/blog/posts/windows-apps-psa-ev-certs-do-not-grant-immediate-reputation-anymore) ·
[winget: signing not required](https://github.com/microsoft/winget-cli/discussions/4327) ·
[the halloy case: an unsigned inno-exe in winget](https://github.com/microsoft/winget-pkgs/issues/385483)
