// A v86 guest under Node, driven over its console: what the web demo's
// scripts share (docs/research/web-demo.md §12-§13).
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
// The build's fixed places (paths.py names the same ones).
const WORK = path.join(ROOT, "target", "web-demo");
export const VENDOR = path.join(WORK, "vendor");
export const INITRD = path.join(WORK, "guest", "initramfs.cpio.gz");
export const STATE_DIR = path.join(WORK, "state");
export const SAVED_STATE = path.join(STATE_DIR, "state.bin.zst");
export const MACHINE = JSON.parse(fs.readFileSync(path.join(here, "machine.json"), "utf8"));

// The size of the terminal the application is started at.
export const COLS = MACHINE.cols;
export const ROWS = MACHINE.rows;
// Printed by /init the moment before the application takes the terminal
// (initramfs.py): what follows it is the application's output from byte one.
export const START_MARK = "@@demo-starts@@";

// The machine. `init_on_free=1` in its kernel's options: a page is zeroed when
// it is freed, so what the boot left behind is not saved with the machine -
// measured, 33.6 MB of compressed state became 23.5 MB. The application's
// terminal is the virtio console, the one that can be told a new size.
export const OPTIONS = {
    memory_size: MACHINE.memory_size,
    vga_memory_size: MACHINE.vga_memory_size,
    cmdline: MACHINE.cmdline,
    virtio_console: true,
    autostart: true,
};

/// Starts a machine: from the kernel and `initrd`, or - with `state` - from a
/// saved state, which needs neither.
export async function boot({ state = null, initrd = null, log = null } = {}) {
    const { V86 } = await import(url.pathToFileURL(path.join(VENDOR, "libv86.mjs")).href);
    const t0 = performance.now();
    const logFile = log ? fs.openSync(log, "w") : null;
    const emulator = new V86({
        ...OPTIONS,
        wasm_path: path.join(VENDOR, "v86.wasm"),
        bios: { url: path.join(VENDOR, "seabios.bin") },
        vga_bios: { url: path.join(VENDOR, "vgabios.bin") },
        ...(state
            ? { initial_state: { url: state } }
            : { bzimage: { url: path.join(VENDOR, "vmlinuz-virt") }, initrd: { url: initrd } }),
    });

    let out = Buffer.alloc(0);
    let lastByteAt = t0;
    let stamps = null;
    const waiters = new Set();
    emulator.add_listener("virtio-console0-output-bytes", (bytes) => {
        const chunk = Buffer.from(bytes);
        if (logFile !== null) fs.writeSync(logFile, chunk);
        out = Buffer.concat([out, chunk]);
        lastByteAt = performance.now();
        if (stamps) stamps.push([lastByteAt, chunk.length]);
        for (const w of waiters) {
            if (out.includes(w.needle)) {
                waiters.delete(w);
                clearTimeout(w.timer);
                w.resolve();
            }
        }
    });

    const guest = {
        emulator,
        since: () => (performance.now() - t0) / 1000,
        output: () => out,
        clear: () => {
            out = Buffer.alloc(0);
        },
        send: (text) => emulator.bus.send("virtio-console0-input-bytes", new Uint8Array(Buffer.from(text, "binary"))),
        // Tells the application's terminal its size. The emulator and the
        // kernel disagree on the order of the two numbers in the message
        // (research §13.2); machine.json says which goes first, and
        // verify_state.mjs proves it on every build.
        resize: (cols, rows) => {
            const size = { cols, rows };
            emulator.bus.send("virtio-console0-resize", MACHINE.resize_sends.map((name) => size[name]));
        },
        ready: () => new Promise((resolve) => emulator.add_listener("emulator-ready", resolve)),
        // Resolves once `needle` is in what the guest wrote since the last `clear`.
        until: (needle, ms = 120000) =>
            new Promise((resolve, reject) => {
                if (out.includes(needle)) return resolve();
                const w = { needle, resolve };
                w.timer = setTimeout(() => {
                    waiters.delete(w);
                    reject(new Error(`no ${JSON.stringify(needle)} in ${ms} ms; the guest's last words: ${JSON.stringify(out.subarray(-300).toString("latin1"))}`));
                }, ms);
                waiters.add(w);
            }),
        // Resolves when the guest has written nothing for `quiet` ms; gives the
        // time from the call to the last byte, and the bytes written meanwhile.
        settle: (quiet = 400, limit = 60000) =>
            new Promise((resolve) => {
                const start = performance.now();
                const before = out.length;
                const first = lastByteAt;
                const tick = setInterval(() => {
                    const now = performance.now();
                    if (now - Math.max(lastByteAt, start) > quiet || now - start > limit) {
                        clearInterval(tick);
                        const last = lastByteAt > first ? lastByteAt : start;
                        resolve({ ms: Math.max(0, last - start), bytes: out.length - before });
                    }
                }, 5);
            }),
        // Does `act` and times what comes back: the first byte, the end of the
        // first burst (a gap of `gap` ms ends it) and the last byte before
        // `quiet` ms of silence - and gives the bytes themselves.
        trace: async (act, quiet = 500, gap = 60) => {
            stamps = [];
            const from = out.length;
            const start = performance.now();
            act();
            await guest.settle(quiet, 30000);
            const got = stamps;
            stamps = null;
            const bytes = out.subarray(from);
            if (got.length === 0) return { first: null, burst: null, last: null, bytes };
            let burstEnd = got[0][0];
            for (let i = 1; i < got.length && got[i][0] - got[i - 1][0] < gap; i += 1) burstEnd = got[i][0];
            return { first: got[0][0] - start, burst: burstEnd - start, last: got.at(-1)[0] - start, bytes };
        },
    };
    return guest;
}

const ESC = "\x1b";
const BEL = "\x07";

// A control sequence whose parameters start at `from`: where it ends, and -
// for a cursor position - the cell it names.
function controlSequence(text, from) {
    let end = from;
    while (end < text.length && (text[end] < "@" || text[end] > "~")) end += 1;
    const final = text[end];
    if (final !== "H" && final !== "f") return { end };
    const [row, col] = text.slice(from, end).split(";");
    return { end, cell: { row: Number(row || 1), col: Number(col || 1) } };
}

// An operating system command that starts before `from`: where its
// terminator - a bell, or an escape and a backslash - ends.
function commandEnd(text, from) {
    const bell = text.indexOf(BEL, from);
    const terminator = text.indexOf(ESC + "\\", from);
    if (terminator !== -1 && (bell === -1 || terminator < bell)) return terminator + 1;
    return bell === -1 ? text.length : bell;
}

// The escape sequence at `at`: its last character's place, and a cell if it
// moves the cursor to one.
function escapeAt(text, at) {
    const kind = text[at + 1];
    if (kind === "[") return controlSequence(text, at + 2);
    if (kind === "]") return { end: commandEnd(text, at + 2) };
    return { end: at + 1 };
}

// A character that is not part of an escape sequence, put at the cursor.
function put(ch, cursor, size) {
    if (ch === "\r") {
        cursor.col = 1;
    } else if (ch === "\n") {
        cursor.row += 1;
    } else if (ch >= " ") {
        size.cols = Math.max(size.cols, cursor.col);
        size.rows = Math.max(size.rows, cursor.row);
        cursor.col += 1;
    }
}

/// The size of the frame a burst of terminal output draws: the last column and
/// the last row a character is put in. Enough of a terminal for that - cursor
/// positioning, and text that moves the cursor one cell a character - which is
/// what the application's frames are made of.
export function frameSize(bytes) {
    const text = Buffer.from(bytes).toString("utf8");
    const cursor = { row: 1, col: 1 };
    const size = { cols: 0, rows: 0 };
    let at = 0;
    while (at < text.length) {
        if (text[at] === ESC) {
            const sequence = escapeAt(text, at);
            if (sequence.cell) Object.assign(cursor, sequence.cell);
            at = sequence.end + 1;
        } else {
            // A whole character, even one that takes two units of the string.
            const ch = String.fromCodePoint(text.codePointAt(at));
            put(ch, cursor, size);
            at += ch.length;
        }
    }
    return size;
}

/// Does `act` to each item, one after another: a key is pressed only when the
/// one before it has been answered.
export function inTurn(items, act) {
    return items.reduce((before, item) => before.then(() => act(item)), Promise.resolve());
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

export function pressBeats(guest, rounds = 3) {
    const f = (n) => (n === null ? "-" : n.toFixed(0)).padStart(6);
    const press = async ([name, keys, quiet]) => {
        const t = await guest.trace(() => guest.send(keys), quiet);
        console.log(`${name.padEnd(30)} first ${f(t.first)}  burst ${f(t.burst)}  last ${f(t.last)} ms  ${String(t.bytes.length).padStart(6)} B`);
    };
    const numbers = Array.from({ length: rounds }, (_, index) => index + 1);
    return inTurn(numbers, (round) => {
        console.log(`--- round ${round}: ms from the key to the first byte, the end of the first burst, the last byte`);
        return inTurn(BEATS, press);
    });
}
