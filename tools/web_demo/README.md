# The web demo

[docs/research/web-demo.md](../../docs/research/web-demo.md): the release's own
binary, built for 32-bit x86, running `mindfork demo` inside an x86 emulator
([v86](https://github.com/copy/v86)) in a browser tab. Stage 1 was the probe
(§12 there); stage 2 made it what a release builds (§13); stage 3 is its page on
the site, mindfork.io/try/ (§14). Nothing it downloads or builds is committed:
all of it lives under `target/web-demo/`.

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

- `target/web-demo/site/` — the asset's own page with everything it loads;
- `target/web-demo/mindfork-webdemo.tar` — the same tree as one archive, the
  asset a release carries. Unpack it anywhere, serve the directory, and that
  release's demo is in a tab.

To see it as the site shows it — Zola at the version `site.yml` pins:

```
python tools/site_webdemo.py --local      # the build's asset, where the site's build finds it
python tools/site_sync_assets.py
zola --root site build -o <dir> -u http://127.0.0.1:8770
python -m http.server --directory <dir> --bind 127.0.0.1 8770     # then /try/
```

A release builds it with `build.py --release`, which refuses to pack without
the application's third-party notices (`cargo about generate about.hbs -o
THIRD-PARTY-NOTICES.md`); a build at a desk works without them and says less
about the application's own dependencies.

## What each file does

| File | What it is |
|---|---|
| `paths.py` | where every step reads and writes — all of it under `target/web-demo/` |
| `fetch.py` | downloads the emulator, the terminal ([xterm.js](https://github.com/xtermjs/xterm.js)) with its WebGL renderer, the BIOS images, Alpine's two release images and musl's release, each pinned by a hash and checked on every run; takes the kernel out of the ISO, and BusyBox, musl and the image's record of its packages out of the base archive |
| `build_i686.sh` | unpacks the 32-bit libraries the binary links against into a sysroot — the distribution's own packages, kept beside them — and builds the application for `i686-unknown-linux-gnu` |
| `initramfs.py` | writes the guest: an `/init` of a dozen lines, the shell, the application and the libraries it loads — everything the machine runs beside its kernel |
| `machine.json` | the emulated machine's options, the terminal the application starts on and at what size, and the order a resize's two numbers go in — read by the scripts and by the page, because a saved state restores only into the machine that saved it |
| `guest.mjs` | a guest under Node, driven over its console; the keys every measurement presses; the size of the frame a burst of output draws |
| `make_state.mjs` | boots the guest and saves the machine once the demo has drawn its first frame, refusing a frame of the wrong size; `MEASURE=1` times the keys instead |
| `verify_state.mjs` | restores the saved machine in a fresh emulator, types into it and resizes it, and fails if the frame that comes back is not the size asked for |
| `manifest.py` | every redistributed piece — name, version, licence, licence texts, where its exact source is — read out of the files the build packs; `--self-test` runs its readers on fixtures |
| `build.py` | runs the above, puts the page together in `target/web-demo/site/` and packs the asset; refuses an asset whose fragment borrows a name the site's stylesheet does not define |
| `page/stage.html` | the markup the script drives — **Start**, the window, the covers over the terminal, the status line, the key hints — the same fragment in the asset's own page and in the site's |
| `page/demo.js`, `page/demo.css` | the script, which finds its files beside itself, and the styles of the demo's own parts; everything of theirs is `wd-` |
| `page/index.html`, `page/standalone.css` | the asset's own page around the fragment, and the few names it borrows from the site's stylesheet, defined for a page that has no site around it |
| `../site_webdemo.py` | the site's side: puts a release's asset — or, with `--local`, this build's — where the site's build finds it |

## The manifest

`manifest.json`, in the asset beside the files it describes:

- `format` — the shape's number, raised when a field changes meaning;
- `application`, `commit` — the version and the commit the binary was built from;
- `fragment`, `script`, `stylesheet` — what a page that shows the demo takes from
  the asset; `standalone` — what only the asset's own page needs, which the site
  does not publish;
- `on_start`, `on_start_bytes`, `by_emulator_bytes` — what a press of **Start**
  downloads, and the part of it the emulator fetches and reports progress on;
- `files` — every file of the asset but the manifest, with its size and sha256;
- `pieces` — one entry per redistributed work: `name`, `version`, `part`
  (`page`, `standalone` or `machine`), `role`, `licence` (an SPDX expression), `homepage`,
  `texts` (the licence texts, as paths in the asset), `sources` (what and
  where), and `package` — the distribution package it was taken out of.

Nothing in `pieces` is typed in beside the pin it describes. The emulator's and
the terminal's entries come from their npm packages' `package.json`; the
kernel's version from its boot header and the BIOS's from its image; BusyBox and
musl from the Alpine image's own package record, down to the commit of Alpine's
build files; the three C libraries from the packages in the sysroot. What no
file says — the libraries' licence expressions — is one table in `manifest.py`,
and a source package the table does not know stops the build.

## The page and the site

The asset is built from a tag and the site from `main`, so what they share is
kept small (research §14.1). The asset brings the fragment, the script and the
styles of its own parts; the site's template (`site/templates/try.html`) brings
the page around them and renders the licence list from `manifest.json`. The
fragment's window and buttons are the site's own `.term` and `.btn`, and
`demo.css` uses four of the site's palette variables; `build.py` reads those
borrowed names out of the two files and refuses to pack if the site's Sass, or
`standalone.css`, does not define one. The manifest's `format` is the other half
of the contract: the site's deploy passes over an asset of a format its template
does not read.

## What is still to come

- **No published release carries the asset yet.** The first one after stage 3
  merges brings the demo to mindfork.io/try/; until then the page says so.
- Safari is untested; the terminal is dark in both themes of the site; the page
  is in English only.
