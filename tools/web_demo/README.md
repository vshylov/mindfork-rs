# The web demo probe

Stage 1 of [docs/research/web-demo.md](../../docs/research/web-demo.md): the
release's own binary, built for 32-bit x86, running `mindfork demo` inside an
x86 emulator ([v86](https://github.com/copy/v86)) in a browser tab. It is a
**probe** — a local page for a go/no-go on how it feels. Nothing here is
deployed, nothing is part of a release, and nothing it downloads or builds is
committed: all of it lives under `target/web-demo/`.

## Building the page

```
tools/web_demo/build_i686.sh --make-sysroot target/web-demo/sysroot   # once
SYSROOT=target/web-demo/sysroot tools/web_demo/build_i686.sh          # ~6 min
python tools/web_demo/build.py \
    --binary target/web-demo/cargo/i686-unknown-linux-gnu/release/mindfork \
    --libs target/web-demo/sysroot/usr/lib32 \
    --libs target/web-demo/sysroot/usr/lib/i386-linux-gnu
python -m http.server --directory target/web-demo/site --bind 127.0.0.1 8765
```

Then open `http://127.0.0.1:8765/` — from another machine, through
`ssh -L 8765:127.0.0.1:8765 <host>`. It needs Node 20 or newer (`--node` or
`$NODE` names a binary that is not on the `PATH`) and the `zstd` command.

## What each file does

| File | What it is |
|---|---|
| `fetch.py` | downloads the emulator, the terminal ([xterm.js](https://github.com/xtermjs/xterm.js)), the BIOS images and the guest's kernel image, each pinned by a hash and checked on every run |
| `build_i686.sh` | builds the application for `i686-unknown-linux-gnu`; `--make-sysroot` unpacks the 32-bit libraries it links against, without root |
| `machine.json` | the emulated machine's options and the terminal's size — read by the scripts and by the page, because a saved state restores only into the machine that saved it |
| `guest.mjs` | a guest under Node, driven over its serial line; the keys every measurement presses |
| `make_state.mjs` | boots the guest, puts the binary into its RAM, starts the demo and saves the machine once the first frame is drawn; `MEASURE=1` times the keys instead |
| `verify_state.mjs` | restores a saved machine in a fresh emulator and types into it |
| `build.py` | runs the three above and puts the page together in `target/web-demo/site/` |
| `page/` | the page: the notice, **Start**, the terminal, the licences |

## What the probe took for granted

- **The guest is not the final one.** It is the Buildroot image v86's own site
  boots, with the binary's four libraries and loader from Ubuntu's i386
  packages beside it. Stage 2 assembles the guest from named packages
  (research §9, F2).
- **The terminal has one size**, 116×44, the size the application was started
  at; the page scales the font to fit the window. A serial line carries no
  size, so resizing is stage 2's.
- **The page stands alone.** It is not the site's template and is not linked
  from anywhere.
