#!/usr/bin/env node
// Boot the guest, put the 32-bit binary and its libraries into the guest's
// RAM, start `mindfork demo` on the serial terminal, and save the machine once
// the first frame is drawn (docs/research/web-demo.md, stage 1).
//
//   node tools/web_demo/make_state.mjs <out-dir> <binary> <dir-with-32-bit-libs>...
//
// Writes into <out-dir>:
//   state.bin   the machine; restore it with `OPTIONS` of guest.mjs
//   primer.bin  what the application wrote from its start to the save. The
//               application believes that frame is on the screen and will only
//               send what changes, so a terminal has to be shown it first.
//
// With MEASURE=1 the keys of `BEATS` are pressed before the save is taken, and
// their timings printed - the save itself is of a machine nobody has typed in.
import fs from "node:fs";
import path from "node:path";
import { boot, pressBeats, COLS, ROWS } from "./guest.mjs";

const [outDir, binary, ...libDirs] = process.argv.slice(2);
if (!outDir || !binary || libDirs.length === 0) {
    console.error("usage: make_state.mjs <out-dir> <binary> <dir-with-32-bit-libs>...");
    process.exit(2);
}
// What the binary is linked against, and the loader that starts it.
const LIBS = ["ld-linux.so.2", "libc.so.6", "libm.so.6", "libgcc_s.so.1", "libasound.so.2"];
// The application builds its HTTP client when it starts and refuses to without
// a certificate bundle - network or no network.
const CA_BUNDLE = process.env.CA_BUNDLE ?? "/etc/ssl/certs/ca-certificates.crt";
const HOME = "/opt/mindfork";

fs.mkdirSync(outDir, { recursive: true });
const serialLog = path.join(outDir, "serial.log");
const g = await boot({ log: serialLog });
const say = (s) => console.log(`[${g.since().toFixed(2)} s] ${s}`);
let failed = false;

try {
    await g.until(/[#%$] $/, 240000);
    say("the guest's shell is up");

    for (const lib of LIBS) {
        const from = libDirs.map((d) => path.join(d, lib)).find((p) => fs.existsSync(p));
        if (!from) throw new Error(`no ${lib} in ${libDirs.join(", ")}`);
        await g.emulator.create_file("/" + lib, new Uint8Array(fs.readFileSync(from)));
    }
    await g.emulator.create_file("/mindfork", new Uint8Array(fs.readFileSync(binary)));
    await g.emulator.create_file("/ca-certificates.crt", new Uint8Array(fs.readFileSync(CA_BUNDLE)));

    // Out of the 9p share and into a RAM disk of the guest's own: the state
    // then holds one copy of each file and needs no file server beside it.
    let r = await g.sh(
        `mkdir -p ${HOME} /etc/ssl/certs && mount -t tmpfs -o size=44m tmpfs ${HOME} && ` +
            `cp /mnt/ca-certificates.crt /etc/ssl/certs/ && ` +
            `cp /mnt/ld-linux.so.2 /mnt/lib*.so.* /mnt/mindfork ${HOME}/ && chmod 755 ${HOME}/* && rm -f /mnt/*`,
        300000,
    );
    if (r.code !== 0) throw new Error(`installing into the guest failed: ${r.text}`);

    const run = `${HOME}/ld-linux.so.2 --library-path ${HOME} ${HOME}/mindfork`;
    r = await g.sh(`${run} --version`, 300000);
    if (r.code !== 0) throw new Error(`the binary does not run in the guest: ${r.text}`);
    say(`in the guest: ${r.text.split("\n").pop()}`);

    r = await g.sh(`stty -F /dev/ttyS0 rows ${ROWS} cols ${COLS} && stty -F /dev/ttyS0 size`);
    if (r.code !== 0) throw new Error(`the serial line took no size: ${r.text}`);

    // Once the shell has become the application, nothing of the image's own
    // userland is needed: its compiler runtimes and tools are 14 MB of state.
    r = await g.sh("rm -rf /usr /root /var/* && sync && echo 3 >/proc/sys/vm/drop_caches");
    if (r.code !== 0) throw new Error(`trimming the guest failed: ${r.text}`);

    // `exec`: the shell becomes the application, so one process reads the line.
    // The mark is printed on a line of its own; the shell's echo of this
    // command holds it only inside quotes, so the pattern finds the print.
    const MARK = "@@demo-starts@@";
    const start = `export TERM=xterm-256color COLORTERM=truecolor; printf '\\n${MARK}\\n'; exec ${run} demo </dev/ttyS0 >/dev/ttyS0 2>&1`;
    g.clear();
    g.send(start + "\n");
    await g.until(new RegExp(`\\n${MARK}\\r?\\n`), 30000);
    let s = await g.settle(2500, 120000);
    say(`the demo drew its first frame: ${s.bytes} B, the last byte ${s.ms.toFixed(0)} ms after the command`);
    if (s.bytes < 2000) throw new Error("the application drew no frame - see serial.log");

    if (process.env.MEASURE === "1") {
        await pressBeats(g);
    } else {
        const state = await g.emulator.save_state();
        fs.writeFileSync(path.join(outDir, "state.bin"), new Uint8Array(state));
        const log = fs.readFileSync(serialLog);
        const mark = Buffer.from(`\n${MARK}\r\n`);
        const at = log.lastIndexOf(mark);
        if (at < 0) throw new Error("the start mark is not in serial.log");
        const primer = log.subarray(at + mark.length);
        fs.writeFileSync(path.join(outDir, "primer.bin"), primer);
        say(`state.bin: ${state.byteLength} B; primer.bin: ${primer.length} B`);
    }
} catch (e) {
    console.error("FAILED:", e.message);
    failed = true;
}
await g.emulator.destroy();
process.exit(failed ? 1 : 0);
