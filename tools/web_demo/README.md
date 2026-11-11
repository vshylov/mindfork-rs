# The web demo

[docs/research/web-demo.md](../../docs/research/web-demo.md): the release's own
binary, built for 32-bit x86, running `mindfork demo` inside an x86 emulator
([v86](https://github.com/copy/v86)) in a browser tab. Stage 1 was the probe
(§12 there); stage 2 is turning it into what a release builds (§13). Nothing
here is deployed yet, and nothing it downloads or builds is committed: all of
it lives under `target/web-demo/`.

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
| `fetch.py` | downloads the emulator, the terminal ([xterm.js](https://github.com/xtermjs/xterm.js)), the BIOS images and Alpine's two release images, each pinned by a hash and checked on every run; takes the kernel out of the ISO and BusyBox with musl out of the base archive |
| `build_i686.sh` | builds the application for `i686-unknown-linux-gnu`; `--make-sysroot` unpacks the 32-bit libraries it links against, without root |
| `initramfs.py` | writes the guest: an `/init` of a dozen lines, the shell, the application and the libraries it loads — everything the machine runs beside its kernel |
| `machine.json` | the emulated machine's options, the terminal the application starts on and at what size, and the order a resize's two numbers go in — read by the scripts and by the page, because a saved state restores only into the machine that saved it |
| `guest.mjs` | a guest under Node, driven over its console; the keys every measurement presses; the size of the frame a burst of output draws |
| `make_state.mjs` | boots the guest and saves the machine once the demo has drawn its first frame, refusing a frame of the wrong size; `MEASURE=1` times the keys instead |
| `verify_state.mjs` | restores a saved machine in a fresh emulator, types into it and resizes it, and fails if the frame that comes back is not the size asked for |
| `build.py` | runs the above and puts the page together in `target/web-demo/site/` |
| `page/` | the page: the notice, **Start**, the terminal at the window's size, the licences |

## What is still a probe's

- **The page stands alone.** It is not the site's template and is not linked
  from anywhere; stage 3 moves it to `/try/`, in the site's own styles.
- **The licence list is prose.** The build does not yet write the manifest the
  page should take it from.
- **No release builds it.** The `web-demo` job and its asset are the rest of
  stage 2.
