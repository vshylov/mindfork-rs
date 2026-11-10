#!/usr/bin/env node
// Restore a saved machine in a fresh emulator and press the probe's keys: the
// check that a state made by make_state.mjs is one a visitor's tab can start
// from, and the timings of that (docs/research/web-demo.md, stage 1).
//
//   node tools/web_demo/verify_state.mjs <state.bin | state.bin.zst>
import { boot, pressBeats } from "./guest.mjs";

const [state] = process.argv.slice(2);
if (!state) {
    console.error("usage: verify_state.mjs <state.bin | state.bin.zst>");
    process.exit(2);
}
const t0 = performance.now();
const g = await boot({ state });
await g.ready();
console.log(`restored: ${(performance.now() - t0).toFixed(0)} ms from nothing to a running machine`);

// A restored application answers a key with a frame; one that did not survive
// the save answers with nothing, and that is the failure this looks for.
const probe = await g.trace("\x1bOP", 600);
await g.trace("\x1b", 700);
if (probe.bytes < 1000) {
    console.error(`FAILED: F1 drew ${probe.bytes} B - the application is not answering`);
    await g.emulator.destroy();
    process.exit(1);
}
await pressBeats(g, 2);
await g.emulator.destroy();
process.exit(0);
