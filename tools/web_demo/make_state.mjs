#!/usr/bin/env node
// Boot the guest - the pinned kernel and the initramfs that holds the
// application - and save the machine once `mindfork demo` has drawn its first
// frame (docs/research/web-demo.md, stage 2).
//
//   node tools/web_demo/make_state.mjs <out-dir> <initramfs.cpio.gz>
//
// Writes into <out-dir>:
//   state.bin   the machine; restore it with `OPTIONS` of guest.mjs
//   primer.bin  what the application wrote from its start to the save. The
//               application believes that frame is on the screen and will only
//               send what changes, so a terminal has to be shown it first.
//
// With MEASURE=1 the keys of `BEATS` are pressed and timed instead - the save
// is always of a machine nobody has typed in.
import fs from "node:fs";
import path from "node:path";
import { boot, pressBeats, frameSize, COLS, ROWS, START_MARK } from "./guest.mjs";

const [outDir, initrd] = process.argv.slice(2);
if (!outDir || !initrd) {
    console.error("usage: make_state.mjs <out-dir> <initramfs.cpio.gz>");
    process.exit(2);
}
fs.mkdirSync(outDir, { recursive: true });
const g = await boot({ initrd, log: path.join(outDir, "console.log") });
const say = (s) => console.log(`[${g.since().toFixed(2)} s] ${s}`);
let failed = false;

try {
    const mark = `\n${START_MARK}\r\n`;
    await g.until(mark, 120000);
    say("/init has handed the terminal to the application");
    await g.settle(2500, 120000);
    const out = g.output();
    const primer = out.subarray(out.lastIndexOf(mark) + mark.length);
    // A frame of the wrong size is a terminal that forgot its size, or an
    // application that did not start: either way not a state to publish.
    const drawn = frameSize(primer);
    if (drawn.cols !== COLS || drawn.rows !== ROWS) {
        throw new Error(`the first frame is ${drawn.cols}x${drawn.rows}, not ${COLS}x${ROWS} - see console.log`);
    }
    say(`the demo drew its first frame: ${primer.length} B, ${drawn.cols}x${drawn.rows}`);

    if (process.env.MEASURE === "1") {
        await pressBeats(g);
    } else {
        const state = await g.emulator.save_state();
        fs.writeFileSync(path.join(outDir, "state.bin"), new Uint8Array(state));
        fs.writeFileSync(path.join(outDir, "primer.bin"), primer);
        say(`state.bin: ${state.byteLength} B; primer.bin: ${primer.length} B`);
    }
} catch (e) {
    console.error("FAILED:", e.message);
    failed = true;
}
await g.emulator.destroy();
process.exit(failed ? 1 : 0);
