#!/usr/bin/env node
// Boot the guest - the pinned kernel and the initramfs that holds the
// application - and save the machine once `mindfork demo` has drawn its first
// frame (docs/research/web-demo.md §13).
//
//   node tools/web_demo/make_state.mjs
//
// Reads target/web-demo/guest/initramfs.cpio.gz; writes into
// target/web-demo/state/:
//   state.bin    the machine; restore it with `OPTIONS` of guest.mjs
//   primer.bin   what the application wrote from its start to the save. The
//                application believes that frame is on the screen and will only
//                send what changes, so a terminal has to be shown it first.
//   console.log  everything the guest wrote - where to look when this fails.
//
// With MEASURE=1 the keys of `BEATS` are pressed and timed instead - the save
// is always of a machine nobody has typed in.
import fs from "node:fs";
import path from "node:path";
import { boot, pressBeats, frameSize, COLS, ROWS, START_MARK, INITRD, STATE_DIR } from "./guest.mjs";

fs.mkdirSync(STATE_DIR, { recursive: true });
const g = await boot({ initrd: INITRD, log: path.join(STATE_DIR, "console.log") });
const say = (s) => console.log(`[${g.since().toFixed(2)} s] ${s}`);

// What the application wrote from the moment /init handed it the terminal.
async function firstFrame() {
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
    return primer;
}

async function save(primer) {
    const state = await g.emulator.save_state();
    fs.writeFileSync(path.join(STATE_DIR, "state.bin"), new Uint8Array(state));
    fs.writeFileSync(path.join(STATE_DIR, "primer.bin"), primer);
    say(`state.bin: ${state.byteLength} B; primer.bin: ${primer.length} B`);
}

let failed = false;
try {
    const primer = await firstFrame();
    await (process.env.MEASURE === "1" ? pressBeats(g) : save(primer));
} catch (error) {
    console.error("FAILED:", error.message);
    failed = true;
}
await g.emulator.destroy();
process.exit(failed ? 1 : 0);
