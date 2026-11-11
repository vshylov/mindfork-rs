# The web demo

[docs/research/web-demo.md](../../docs/research/web-demo.md): the release's own
binary, built for 32-bit x86, running `mindfork demo` inside an x86 emulator
([v86](https://github.com/copy/v86)) in a browser tab. Stage 1 was the probe
(§12 there); stage 2 made it what a release builds (§13). It is not deployed
yet — the page on mindfork.io is stage 3 — and nothing it downloads or builds
is committed: all of it lives under `target/web-demo/`.

## Building it

On Ubuntu 24.04, without root:

```
tools/web_demo/build_i686.sh        # the 32-bit binary and its libraries, ~5 min
python tools/web_demo/build.py      # the machine, its state, the page, the asset, ~1 min
python -m http.server --directory target/web-demo/site --bind 127.0.0.1 8765
```

Then open `http://127.0.0.1:8765/` — from another machine, through
`ssh -L 8765:127.0.0.1:8765 <host>`. `build.py` needs Node 20 or newer and the
`zstd` command on the `PATH`. No tool here takes a path: each step reads what
the one before it wrote, in the places `paths.py` names.

What comes out:

- `target/web-demo/site/` — the page with everything it loads;
- `target/web-demo/mindfork-webdemo.tar` — the same tree as one archive, the
  asset a release carries. Unpack it anywhere, serve the directory, and that
  release's demo is in a tab.

A release builds it with `build.py --release`, which refuses to pack without
the application's third-party notices (`cargo about generate about.hbs -o
THIRD-PARTY-NOTICES.md`); a build at a desk works without them and says less
about the application's own dependencies.

## What each file does

| File | What it is |
|---|---|
| `paths.py` | where every step reads and writes — all of it under `target/web-demo/` |
| `fetch.py` | downloads the emulator, the terminal ([xterm.js](https://github.com/xtermjs/xterm.js)), the BIOS images, Alpine's two release images and musl's release, each pinned by a hash and checked on every run; takes the kernel out of the ISO, and BusyBox, musl and the image's record of its packages out of the base archive |
| `build_i686.sh` | unpacks the 32-bit libraries the binary links against into a sysroot — the distribution's own packages, kept beside them — and builds the application for `i686-unknown-linux-gnu` |
| `initramfs.py` | writes the guest: an `/init` of a dozen lines, the shell, the application and the libraries it loads — everything the machine runs beside its kernel |
| `machine.json` | the emulated machine's options, the terminal the application starts on and at what size, and the order a resize's two numbers go in — read by the scripts and by the page, because a saved state restores only into the machine that saved it |
| `guest.mjs` | a guest under Node, driven over its console; the keys every measurement presses; the size of the frame a burst of output draws |
| `make_state.mjs` | boots the guest and saves the machine once the demo has drawn its first frame, refusing a frame of the wrong size; `MEASURE=1` times the keys instead |
| `verify_state.mjs` | restores the saved machine in a fresh emulator, types into it and resizes it, and fails if the frame that comes back is not the size asked for |
| `manifest.py` | every redistributed piece — name, version, licence, licence texts, where its exact source is — read out of the files the build packs; `--self-test` runs its readers on fixtures |
| `build.py` | runs the above, puts the page together in `target/web-demo/site/` and packs the asset |
| `page/` | the page as it stands alone: the notice, **Start**, the terminal at the window's size, the licence list the manifest fills |

## The manifest

`manifest.json`, in the asset beside the files it describes:

- `format` — the shape's number, raised when a field changes meaning;
- `application`, `commit` — the version and the commit the binary was built from;
- `on_start`, `on_start_bytes`, `by_emulator_bytes` — what a press of **Start**
  downloads, and the part of it the emulator fetches and reports progress on;
- `files` — every file of the asset but the manifest, with its size and sha256;
- `pieces` — one entry per redistributed work: `name`, `version`, `part`
  (`page` or `machine`), `role`, `licence` (an SPDX expression), `homepage`,
  `texts` (the licence texts, as paths in the asset), `sources` (what and
  where), and `package` — the distribution package it was taken out of.

Nothing in `pieces` is typed in beside the pin it describes. The emulator's and
the terminal's entries come from their npm packages' `package.json`; the
kernel's version from its boot header and the BIOS's from its image; BusyBox and
musl from the Alpine image's own package record, down to the commit of Alpine's
build files; the three C libraries from the packages in the sysroot. What no
file says — the libraries' licence expressions — is one table in `manifest.py`,
and a source package the table does not know stops the build.

## What is still to come

- **The page stands alone.** It is not the site's template and is not linked
  from anywhere; stage 3 moves it to `/try/`, in the site's own styles, with the
  deploy fetching the asset from the newest release that carries one.
- **No release has built it yet.** The `web-demo` job is in `release.yml`, and
  its steps run on a pull request that changes these tools (`web-demo.yml`); the
  first tag to carry the asset is still ahead.
