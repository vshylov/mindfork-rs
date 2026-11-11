#!/usr/bin/env node
// Restore the saved machine in a fresh emulator and use it: the check that a
// state made by make_state.mjs is one a visitor's tab can start from
// (docs/research/web-demo.md §13).
//
//   node tools/web_demo/verify_state.mjs
//
// Reads target/web-demo/state/state.bin.zst. Three things have to hold. The
// application answers a key with a frame. A resize reaches it, and the frame
// it draws is the size that was asked for - the emulator and the kernel
// disagree on the order of the two numbers in the message, so a kernel that
// changes its mind would otherwise turn every visitor's terminal on its side.
// And the application is back at the size it was saved at, which is what the
// page's primer shows.
import { boot, pressBeats, frameSize, COLS, ROWS, SAVED_STATE } from "./guest.mjs";

const t0 = performance.now();
const g = await boot({ state: SAVED_STATE });
await g.ready();
console.log(`restored: ${(performance.now() - t0).toFixed(0)} ms from nothing to a running machine`);

// Asks the application for a grid and gives the frame it then draws.
async function resized(cols, rows) {
    const answer = await g.trace(() => g.resize(cols, rows), 800);
    return { ...frameSize(answer.bytes), ms: answer.last };
}

async function check() {
    const help = await g.trace(() => g.send("\x1bOP"), 600);
    await g.trace(() => g.send("\x1b"), 700);
    if (help.bytes.length < 1000) throw new Error(`F1 drew ${help.bytes.length} B - the application is not answering`);

    // Wider and shorter at once: a swapped pair cannot pass for the right one.
    const wide = { cols: COLS + 14, rows: ROWS - 6 };
    const drawn = await resized(wide.cols, wide.rows);
    if (drawn.cols !== wide.cols || drawn.rows !== wide.rows) {
        throw new Error(`asked for ${wide.cols}x${wide.rows}, the application drew ${drawn.cols}x${drawn.rows} - check machine.json's resize_sends against this kernel`);
    }
    console.log(`resize: ${wide.cols}x${wide.rows} asked, ${drawn.cols}x${drawn.rows} drawn, ${drawn.ms.toFixed(0)} ms`);
    const again = await resized(COLS, ROWS);
    if (again.cols !== COLS || again.rows !== ROWS) {
        throw new Error(`back at ${COLS}x${ROWS} the application drew ${again.cols}x${again.rows}`);
    }
    await pressBeats(g, 2);
}

let failed = false;
try {
    await check();
} catch (error) {
    console.error("FAILED:", error.message);
    failed = true;
}
await g.emulator.destroy();
process.exit(failed ? 1 : 0);
