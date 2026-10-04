# Scoop and the AUR — two package managers before winget

Status: **built and checked 2026-10-03** — the Scoop bucket is live
(`vshylov/scoop-bucket`). **The AUR package is ready and blocked**: the AUR has
closed new-account registration (§4), so it has no account to be published
from; every release still builds and checks it, and the user-facing documents
do not mention it until it is published. Part of the promotion plan's stage 1,
the friction of a first try ([promotion.md §4](promotion.md)).

## 1. Why these two, and why now

A Windows user who lives in a terminal installs with Scoop or winget; an Arch
user installs from the AUR. Neither could get mindfork that way. winget waits
for a signed installer ([installers.md §5.4](../history/installers.md)), and the
signing waits for reputation ([code-signing.md](code-signing.md)) — which these
channels help to build. **Scoop takes the portable zip and asks for no
signature**, so Windows gets a package manager now rather than after signing;
the AUR was an "on demand" item whose release already builds the Arch package
it needs.

## 2. Scoop

### 2.1 What the manifest does

`bucket/mindfork.json` in **`vshylov/scoop-bucket`**:

- the release's **portable Windows zip**, its hash checked — on install, against
  the manifest; on an update, Excavator takes it from the release's own
  `sha256sums.txt`;
- **`defaults.json` with `{ "mode": "system" }`** written beside the binary by
  the manifest's installer script, so the data lives in
  `%APPDATA%\mindfork-rs\data` (install.md §2.1). Scoop installs every version
  into a directory of its own, and the portable default — data beside the
  binary — would leave the user's chats behind in the previous version's folder
  on the first update. This is the Linux packages' choice and the one the
  Windows installer recommends; the dictionaries keep loading from beside the
  binary (the P1 fallback);
- `mindfork` on `PATH` (a shim), and a Start-menu shortcut;
- `checkver` from the GitHub releases and `autoupdate` with the hash from
  `$baseurl/sha256sums.txt`. Scoop's built-in extraction rule —
  `([a-fA-F0-9]{32,128})[\x20\t]+.*$basename` — reads the release's
  `<hash>  ./<name>` lines as they are, so no rule of our own is needed.

### 2.2 The bucket is a repository of its own

From the official `ScoopInstaller/BucketTemplate`: its CI (Scoop's own manifest
tests) and **Excavator**, which every four hours checks for a new release and
commits the manifest for it — no secret, nothing to run from this repository.
Topic `scoop-bucket`, so scoop.sh indexes it.

### 2.3 Checked (Windows 11, Scoop installed)

- `scoop install` of the manifest: the zip downloaded (12.8 MB), the hash
  matched, the installer script wrote `defaults.json`, the shim and the shortcut
  made; `mindfork --version` → `mindfork 0.14.1`; `mindfork stats` reports the
  data root `C:\Users\…\AppData\Roaming\mindfork-rs\data`.
- `scoop uninstall`: the shim, the shortcut and the app directory gone;
  `%APPDATA%\mindfork-rs` was never created (`--version` and `stats` create no
  data).
- **Autoupdate**: the manifest set back to 0.14.0 with a zeroed hash, Scoop's
  `checkver.ps1 -Update` found 0.14.1, found the hash in `sha256sums.txt`
  ("Extract Mode") and wrote it — the exact digest the release lists.
- Scoop's `formatjson.ps1` left the manifest as written.

## 3. The AUR — `mindfork-rs-bin`

### 3.1 The package

`packaging/aur/PKGBUILD.in`, rendered per release by `tools/aur_package.py`:

- **It repackages the release's own Arch package** (`.pkg.tar.zst`, built by
  nfpm and smoke-tested by `packaging.yml`), so the layout is the tested one to
  the file: the binary and `defaults.json` (`"system"`) in `/usr/lib/mindfork-rs`,
  the link in `/usr/bin`, the dictionaries, the desktop entry, the icons, the
  docs. `-bin`, as the AUR requires of prebuilt deliverables.
- `depends` are what `namcap` finds: `alsa-lib`, `glibc`, `libgcc` (the
  `libgcc_s` every Rust binary links; current Arch splits it from `gcc-libs`,
  which namcap called unneeded) and `hicolor-icon-theme` (namcap's one error on
  the first draft: the icons' hierarchy).
- The licence is a **copy** in `/usr/share/licenses/mindfork-rs-bin/`, not a
  link into `/usr/share/doc`: the archlinux image — and any system with the
  common `NoExtract` for documentation — drops the docs, and the link dangled
  there (the first run of the check caught it).
- `!strip`, `!debug`: the release's binary is already stripped
  (`profile.release`), attested and checksummed as it is.
- `backup` keeps an edited `defaults.json`; `provides`/`conflicts`
  `mindfork-rs`, the release's own package's name.
- The AUR repository also gets a `LICENSE` for the packaging files (0BSD, as the
  submission guidelines encourage).

### 3.2 Built and run before it is pushed

`packaging/aur/check.sh` runs in `archlinux:base-devel`: a system upgrade, the
PKGBUILD's dependencies, `makepkg` as an unprivileged user, `namcap` (an `E:`
fails), `pacman -U`, the layout, and **`mindfork --version` as that user must
name the release**; then `makepkg --printsrcinfo` writes `.SRCINFO` — makepkg's
file, not a hand-made one. Checked here on 0.14.1: namcap silent, `installed:
mindfork 0.14.1`.

### 3.3 Published by the release

`.github/workflows/aur.yml`, beside crates.io: on `release: published` (never a
prerelease) and on `workflow_dispatch` with a tag. It renders, runs the check,
and pushes `PKGBUILD`, `.SRCINFO` and `LICENSE` to
`ssh://aur@aur.archlinux.org/mindfork-rs-bin.git` with
`AUR_SSH_PRIVATE_KEY`. **The AUR's host key is pinned**: `ssh-keyscan`'s Ed25519
key matched the fingerprint the AUR publishes on its home page
(`SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4`). Without the secret the
run builds and checks and says so in a notice. `packaging.yml` runs the same
check on every pull request that touches the packaging, against the latest
release.

`tools/aur_package.py --self-test` runs in `ci.yml`'s lint: a prerelease tag and
a package `sha256sums.txt` names other than exactly once are refused.

## 4. What the owner does once — blocked on the AUR's registration

**Blocked (2026-10-04).** The AUR's registration page answers `503` with *"New
account registration is temporarily closed"*: paused during a wave of automated
account creation, no manual queue, the reopening to be announced on
`aur-general` and the Arch news feed — the owner reports it opens only now and
then, for a couple of weeks. Until then:

- **`aur.yml` keeps running on every published release** — render, build, lint,
  install, run, `.SRCINFO` — and stops at the push with a notice, so the PKGBUILD
  is known to work on the day an account exists rather than found broken then.
- **No user-facing document names the AUR** (install.md, the README, the site's
  install page, the CHANGELOG): a `yay -S` that answers "target not found" is the
  door-closing message this project keeps finding (lessons §4). They gain the
  line in the pull request that follows the first push.
- **On Arch meanwhile**, the release's own package: download the
  `.pkg.tar.zst` and `pacman -U ./…` it, as install.md says. Not
  `pacman -U <url>` — measured in `archlinux:base`: pacman then fetches a
  `.sig` beside the URL, gets a `404` (the packages are unsigned) and installs
  nothing.
- **Another route, if one turns up**: anyone with an AUR account can submit the
  rendered PKGBUILD and add the owner as a co-maintainer later; `aur.yml` would
  then push with that account's key in the secret.

The steps, once registration reopens — the agent creates no accounts and
handles no keys:

1. An account at <https://aur.archlinux.org/register>.
2. A key for CI alone: `ssh-keygen -t ed25519 -f aur_mindfork -C mindfork-aur -N ""`.
3. The public half (`aur_mindfork.pub`) into the AUR account's *SSH Public Key*.
4. The private half into this repository's secrets:
   `gh secret set AUR_SSH_PRIVATE_KEY < aur_mindfork`, then delete the local copy.
5. Run *AUR* from the Actions tab (or `gh workflow run aur.yml`) — the first
   push creates the package for the current release; every published release
   after that updates it by itself.

## 5. Forks

- **F1 — where the Scoop bucket lives.** (a) a repository of its own;
  (b) a `bucket/` directory here — `scoop bucket add` clones the whole
  46 MB repository; (c) ScoopInstaller's Extras — its notability bar is not
  met yet. **Owner's decision 2026-10-03: (a), `vshylov/scoop-bucket`.**
- **F2 — where Scoop keeps the data.** (a) `defaults.json` → `system`;
  (b) portable with Scoop's `persist: "data"` — but a persisted directory is
  never refreshed from a new version, and the bundled dictionaries live there.
  **Chosen: (a)**, the Linux packages' and the installer's choice.
- **F3 — what the AUR package installs from.** (a) the release's Arch package;
  (b) the portable tar.gz, laid out by hand — a second layout to keep equal to
  nfpm's; (c) a source package built with cargo — a different package
  (`mindfork-rs`), later if asked. **Chosen: (a).**
- **F4 — who updates them.** Scoop: Excavator in the bucket, no secret. AUR: this
  repository's release, with a key only the owner can register. **Owner's
  decision 2026-10-03: the owner registers the account and the key.**
- **F5 — what to do while the AUR's registration is closed.** (a) keep the
  machinery, checked on every release, and say nothing to users until it is
  published; (b) take it out until an account exists. **Owner's report
  2026-10-04: registration closed for weeks or months — (a).**
