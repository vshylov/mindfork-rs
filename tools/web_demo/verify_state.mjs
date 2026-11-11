#!/usr/bin/env node
// Restore a saved machine in a fresh emulator and use it: the check that a
// state made by make_state.mjs is one a visitor's tab can start from
// (docs/research/web-demo.md, stage 2).
//
//   node tools/web_demo/verify_state.mjs <state.bin | state.bin.zst>
//
// Three things have to hold. The application answers a key with a frame. A
// resize reaches it, and the frame it draws is the size that was asked for -
// the emulator and the kernel disagree on the order of the two numbers in the
// message, so a kernel that changes its mind would otherwise turn every
// visitor's terminal on its side. And the application is back at the size it
// was saved at, which is what the page's primer shows.
import { boot, pressBeats, frameSize, COLS, ROWS } from "./guest.mjs";

const [state] = process.argv.slice(2);
if (!state) {
    console.error("usage: verify_state.mjs <state.bin | state.bin.zst>");
    process.exit(2);
}
const t0 = performance.now();
const g = await boot({ state });
await g.ready();
console.log(`restored: ${(performance.now() - t0).toFixed(0)} ms from nothing to a running machine`);

const fail = async (message) => {
    console.error("FAILED:", message);
    await g.emulator.destroy();
    process.exit(1);
};

const help = await g.trace(() => g.send("\x1bOP"), 600);
await g.trace(() => g.send("\x1b"), 700);
if (help.bytes.length < 1000) await fail(`F1 drew ${help.bytes.length} B - the application is not answering`);

// Wider and shorter at once: a swapped pair cannot pass for the right one.
const wide = { cols: COLS + 14, rows: ROWS - 6 };
const resized = await g.trace(() => g.resize(wide.cols, wide.rows), 800);
const drawn = frameSize(resized.bytes);
if (drawn.cols !== wide.cols || drawn.rows !== wide.rows) {
    await fail(`asked for ${wide.cols}x${wide.rows}, the application drew ${drawn.cols}x${drawn.rows} - check machine.json's resize_sends against this kernel`);
}
console.log(`resize: ${wide.cols}x${wide.rows} asked, ${drawn.cols}x${drawn.rows} drawn, ${resized.last.toFixed(0)} ms`);
const back = await g.trace(() => g.resize(COLS, ROWS), 800);
const again = frameSize(back.bytes);
if (again.cols !== COLS || again.rows !== ROWS) await fail(`back at ${COLS}x${ROWS} the application drew ${again.cols}x${again.rows}`);

await pressBeats(g, 2);
await g.emulator.destroy();
process.exit(0);
