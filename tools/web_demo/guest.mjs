// A v86 guest under Node, driven over its serial line: what the probe's
// scripts share (docs/research/web-demo.md, stage 1).
//
// The options a machine is created with are part of its saved state: a state
// restores only into a machine made with the same memory sizes and devices,
// by the same emulator build. machine.json is that set - read here and by the
// page, so the two cannot drift apart.
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const here = url.fileURLToPath(new URL(".", import.meta.url));
export const ROOT = path.resolve(here, "..", "..");
export const VENDOR = path.join(ROOT, "target", "web-demo", "vendor");

const machine = JSON.parse(fs.readFileSync(path.join(here, "machine.json"), "utf8"));

// The size of the terminal the application is started in. A serial line
// carries no size, so the page's terminal has to be this one.
export const COLS = machine.cols;
export const ROWS = machine.rows;

export const OPTIONS = {
    memory_size: machine.memory_size,
    vga_memory_size: machine.vga_memory_size,
    // The kernel's options hold `init_on_free=1`: a page is zeroed when it is
    // freed, so what the boot and the copy into RAM left behind is not saved
    // with the machine. Measured: 33.6 MB of compressed state became 23.5 MB.
    cmdline: machine.cmdline,
    filesystem: {},
    autostart: true,
};

/// Starts a machine: from the kernel image, or - with `state` - from a saved
/// state, in which case no kernel image is needed.
export async function boot({ state = null, log = null } = {}) {
    const { V86 } = await import(url.pathToFileURL(path.join(VENDOR, "libv86.mjs")).href);
    const t0 = performance.now();
    const logFile = log ? fs.openSync(log, "w") : null;
    const emulator = new V86({
        ...OPTIONS,
        wasm_path: path.join(VENDOR, "v86.wasm"),
        bios: { url: path.join(VENDOR, "seabios.bin") },
        vga_bios: { url: path.join(VENDOR, "vgabios.bin") },
        ...(state ? { initial_state: { url: state } } : { bzimage: { url: path.join(VENDOR, "buildroot-bzimage68.bin") } }),
    });

    let out = "";
    let bytes = 0;
    let lastByteAt = t0;
    let stamps = null;
    const waiters = new Set();
    const matches = (needle) => (needle instanceof RegExp ? needle.test(out) : out.includes(needle));
    emulator.add_listener("serial0-output-byte", (byte) => {
        if (logFile !== null) fs.writeSync(logFile, Buffer.from([byte]));
        out += String.fromCharCode(byte);
        bytes += 1;
        lastByteAt = performance.now();
        if (stamps) stamps.push(lastByteAt);
        for (const w of waiters) {
            if (matches(w.needle)) {
                waiters.delete(w);
                clearTimeout(w.timer);
                w.resolve();
            }
        }
    });

    const guest = {
        emulator,
        since: () => (performance.now() - t0) / 1000,
        clear: () => {
            out = "";
        },
        send: (text) => emulator.serial0_send(text),
        ready: () => new Promise((resolve) => emulator.add_listener("emulator-ready", resolve)),
        // Resolves once `needle` (a string or a RegExp) matches what the guest
        // wrote since the last `clear`.
        until: (needle, ms = 120000) =>
            new Promise((resolve, reject) => {
                if (matches(needle)) return resolve();
                const w = { needle, resolve };
                w.timer = setTimeout(() => {
                    waiters.delete(w);
                    reject(new Error(`no ${needle} in ${ms} ms; the guest's last words: ${JSON.stringify(out.slice(-300))}`));
                }, ms);
                waiters.add(w);
            }),
        // Resolves when the guest has written nothing for `quiet` ms; gives the
        // time from the call to the last byte, and the bytes written meanwhile.
        settle: (quiet = 400, limit = 60000) =>
            new Promise((resolve) => {
                const start = performance.now();
                const before = bytes;
                const first = lastByteAt;
                const tick = setInterval(() => {
                    const now = performance.now();
                    if (now - Math.max(lastByteAt, start) > quiet || now - start > limit) {
                        clearInterval(tick);
                        const last = lastByteAt > first ? lastByteAt : start;
                        resolve({ ms: Math.max(0, last - start), bytes: bytes - before });
                    }
                }, 5);
            }),
        // Sends keys and times what comes back: the first byte, the end of the
        // first burst (a gap of `gap` ms ends it) and the last byte before
        // `quiet` ms of silence.
        trace: async (keys, quiet = 500, gap = 60) => {
            stamps = [];
            const start = performance.now();
            emulator.serial0_send(keys);
            await guest.settle(quiet, 30000);
            const got = stamps;
            stamps = null;
            if (got.length === 0) return { first: null, burst: null, last: null, bytes: 0 };
            let burstEnd = got[0];
            for (let i = 1; i < got.length && got[i] - got[i - 1] < gap; i += 1) burstEnd = got[i];
            return { first: got[0] - start, burst: burstEnd - start, last: got[got.length - 1] - start, bytes: got.length };
        },
        // Runs a shell command and resolves with its exit code and output.
        sh: async (cmd, ms = 120000) => {
            const mark = `__done_${Math.random().toString(36).slice(2, 8)}__`;
            out = "";
            emulator.serial0_send(`${cmd}; echo ${mark}$?\n`);
            // The echo of the typed line holds the mark too, followed by `$?`;
            // the command's own is followed by its exit code.
            const done = new RegExp(`${mark}(\\d+)`);
            await guest.until(done, ms);
            const m = out.match(done);
            const body = out.slice(0, m.index).replace(/\r/g, "");
            return { code: Number(m[1]), text: body.slice(body.indexOf("$?\n") + 3).trimEnd() };
        },
    };
    return guest;
}

/// The keys the probe presses, the same in every measurement: typing, a sent
/// message with its scripted reply, scrolling, and three screens opened and
/// closed.
export const BEATS = [
    ["type 'H'", "H", 700],
    ["type 'i'", "i", 700],
    ["Enter (send, scripted reply)", "\r", 2500],
    ["PageUp", "\x1b[5~", 500],
    ["PageDown", "\x1b[6~", 500],
    ["F1 help", "\x1bOP", 500],
    ["Esc", "\x1b", 700],
    ["F3 self-model", "\x1bOR", 500],
    ["Esc", "\x1b", 700],
    ["Ctrl+P settings", "\x10", 500],
    ["Down", "\x1b[B", 500],
    ["Esc", "\x1b", 700],
];

export async function pressBeats(guest, rounds = 3) {
    const f = (n) => (n === null ? "-" : n.toFixed(0)).padStart(6);
    for (let round = 1; round <= rounds; round += 1) {
        console.log(`--- round ${round}: ms from the key to the first byte, the end of the first burst, the last byte`);
        for (const [name, keys, quiet] of BEATS) {
            const t = await guest.trace(keys, quiet);
            console.log(`${name.padEnd(30)} first ${f(t.first)}  burst ${f(t.burst)}  last ${f(t.last)} ms  ${String(t.bytes).padStart(6)} B`);
        }
    }
}
