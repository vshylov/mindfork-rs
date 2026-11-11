// The web demo's page (docs/research/web-demo.md): a saved machine with
// `mindfork demo` already running is restored into an x86 emulator in this
// tab, and its console is joined to a terminal the size of the window.
//
// Nothing heavy is loaded before Start is pressed: the emulator, the terminal
// and the machine's state are fetched on the click.
//
// A module: its names are its own, it runs once the page is parsed, and it
// finds its files beside itself - so it works wherever the asset is put, in a
// page that is somewhere else (docs/research/web-demo.md §14.2). The markup it
// drives is stage.html, which travels with it; every id there is `wd-`.

const beside = (name) => new URL(name, import.meta.url).href;
const $ = (id) => document.getElementById("wd-" + id);
const root = $("root");
const startBox = $("start");
const startButton = $("start-button");
const againButton = $("again-button");
const stage = $("stage");
const statusLine = $("status");
const keys = $("keys");
const loading = $("loading");
const loadingText = $("loading-text");
const loadingBar = $("loading-bar");
const ended = $("ended");
const terminalBox = $("terminal");
const frame = stage.querySelector(".wd-window");
const bar = stage.querySelector(".term-bar");

// The grid the application was saved at, and what a click downloads:
// written into the fragment when it is built, so the progress has its scale
// before anything has been fetched.
const SAVED = { cols: Number(root.dataset.cols), rows: Number(root.dataset.rows) };
const ALL_BYTES = Number(root.dataset.bytes);
const MACHINE_BYTES = Number(root.dataset.machineBytes);

// The terminal box's padding, as demo.css sets it, and the room kept under
// the window for the status line.
const PADDING = 6;
const UNDER = 44;
// The text is this size wherever the window gives a grid of at least
// SMALLEST at it, and smaller only where it does not. LARGEST is where a
// large window stops adding cells: the interface is a conversation, and
// stretched across a wide screen it is a column of text beside an empty
// field - the owner's finding on the first page that let it stretch.
const FONT = 15;
const MIN_FONT = 8;
const SMALLEST = { cols: 80, rows: 24 };
const LARGEST = { cols: 120, rows: 50 };
// After the application is told the window's grid, the cover stays until it
// has drawn for it. Told a new size, it first clears the screen - a few bytes
// - and draws the frame a fifth of a second later, a kilobyte at a time
// (measured in the page; longer on a slower machine). So the wait is for a
// frame's worth of output, FRAME_BYTES, and then for QUIET of silence;
// REDRAW_LIMIT ends it if that never comes.
const FRAME_BYTES = 1024;
const QUIET = 120;
const REDRAW_LIMIT = 2500;
// The application leaves the alternate screen when it quits.
const LEAVES_ALT_SCREEN = "\x1b[?1049l";
// The address of a page that starts the demo without waiting for a click:
// what the reload behind "Start again" comes back to. In the address and not
// in the browser's storage - the page keeps nothing (§14.4).
const AUTOSTART = "#start";
const FACE = '"JetBrains Mono"';

// What the page measures, readable from the console and by a test.
// `keys`: for each key pressed, when it was pressed and how many
// milliseconds later each piece of the answer had been handed to the
// terminal - which paints it on the browser's next frame.
const stats = { clickedAt: null, readyAt: null, firstFrameAt: null, shownAt: null, downloaded: 0, grid: null, renderer: null, keys: [] };
window.__demo = { stats };

let term = null;
let emulator = null;
let machine = null;
let parts = null;
let grid = null;

// Below this the smallest grid needs smaller text than MIN_FONT.
if (window.innerWidth < 760 || window.innerHeight < 460 || window.matchMedia("(pointer: coarse)").matches) {
    $("narrow").hidden = false;
}

function loadScript(src) {
    return new Promise((resolve, reject) => {
        const script = document.createElement("script");
        script.src = src;
        script.onload = resolve;
        script.onerror = () => reject(new Error("could not load " + src));
        document.head.appendChild(script);
    });
}

function loadStyle(href) {
    return new Promise((resolve, reject) => {
        const link = document.createElement("link");
        link.rel = "stylesheet";
        link.href = href;
        link.onload = resolve;
        link.onerror = () => reject(new Error("could not load " + href));
        document.head.appendChild(link);
    });
}

// The three faces the terminal draws with. The grid is measured from them,
// so they come before anything is sized.
function loadFaces() {
    return Promise.all([
        document.fonts.load("400 14px " + FACE),
        document.fonts.load("700 14px " + FACE),
        document.fonts.load("italic 400 14px " + FACE),
    ]);
}

// The emulator, the terminal and what both need - once per page.
function loadParts() {
    if (!parts) {
        parts = Promise.all([
            fetch(beside("machine.json")).then((r) => r.json()),
            fetch(beside("primer.bin")).then((r) => r.arrayBuffer()),
            loadScript(beside("vendor/xterm.js")),
            loadScript(beside("vendor/xterm-webgl.js")),
            loadStyle(beside("vendor/xterm.css")),
            loadScript(beside("vendor/libv86.js")),
        ]).then(([machineJson, primer]) => ({ machine: machineJson, primer: new Uint8Array(primer) }));
    }
    return parts;
}

// Whether the terminal's cells are whole device pixels wide. They are with
// the WebGL renderer, which draws on the screen's own grid; the default one
// keeps the face's advance as it is. Until there is a terminal to ask, the
// guess is that a browser with WebGL 2 will get that renderer.
let onDevicePixels = Boolean(document.createElement("canvas").getContext("webgl2"));

// A cell at a font size, worked out the way the terminal does it: the
// face's advance and line box measured on a canvas at that very size -
// where a browser may round them - the line box rounded up to whole device
// pixels, and the advance rounded down to them where the renderer does.
const ruler = document.createElement("canvas").getContext("2d");
function cellAt(size) {
    ruler.font = size + "px " + FACE + ", monospace";
    const metrics = ruler.measureText("W");
    const lineBox = metrics.fontBoundingBoxAscent + metrics.fontBoundingBoxDescent || size * 1.32;
    const dpr = window.devicePixelRatio || 1;
    const width = onDevicePixels ? Math.floor(metrics.width * dpr) / dpr : metrics.width;
    return { width, height: Math.ceil(lineBox * dpr) / dpr };
}

// The pixels the grid may take: the width the page gives the demo, and the
// height of the browser's window - less what the window around the terminal
// takes itself, and less what the page keeps at the top of the screen for a
// header that stays there (`--wd-top`, the page's to set; nothing by default).
function room() {
    const edges = getComputedStyle(frame);
    const across = Number.parseFloat(edges.borderLeftWidth) + Number.parseFloat(edges.borderRightWidth);
    const down = Number.parseFloat(edges.borderTopWidth) + Number.parseFloat(edges.borderBottomWidth);
    const top = Number.parseFloat(getComputedStyle(root).getPropertyValue("--wd-top")) || 0;
    stage.style.scrollMarginTop = top + "px";
    return {
        width: Math.min(root.clientWidth, window.innerWidth) - across - 2 * PADDING,
        height: window.innerHeight - top - bar.offsetHeight - down - 2 * PADDING - UNDER,
    };
}

// The grid for this window: as many cells as fit at FONT, and smaller text
// only where even the smallest grid would not fit.
function layout() {
    const { width, height } = room();
    for (let size = FONT; ; size -= 1) {
        const cell = cellAt(size);
        const cols = Math.min(LARGEST.cols, Math.floor(width / cell.width));
        const rows = Math.min(LARGEST.rows, Math.floor(height / cell.height));
        if ((cols >= SMALLEST.cols && rows >= SMALLEST.rows) || size === MIN_FONT) {
            return { size, cols: Math.max(cols, 20), rows: Math.max(rows, 6), cell };
        }
    }
}

// The window takes the grid's size whether or not a terminal is in it:
// while the demo loads, and after it has ended.
function setBox(width, height) {
    terminalBox.style.width = width + 2 * PADDING + "px";
    terminalBox.style.height = height + 2 * PADDING + "px";
}

function sizeWindow() {
    grid = layout();
    stats.grid = { size: grid.size, cols: grid.cols, rows: grid.rows };
    setBox(Math.round(grid.cell.width * grid.cols), Math.round(grid.cell.height * grid.rows));
}

// The terminal and the application both take the window's grid; says whether
// the application had to be told a new one. The message's two numbers go in
// the order the machine's kernel reads them (machine.json, and
// docs/research/web-demo.md §13.2).
function applyGrid() {
    if (!term || !emulator) return false;
    term.options.fontSize = grid.size;
    const changed = term.cols !== grid.cols || term.rows !== grid.rows;
    if (changed) {
        term.resize(grid.cols, grid.rows);
        emulator.bus.send("virtio-console0-resize", machine.resize_sends.map((name) => grid[name]));
    }
    requestAnimationFrame(() => {
        // Should the terminal's own grid come out other than worked out
        // above, the window takes the real one.
        const screen = terminalBox.querySelector(".xterm-screen");
        if (screen) setBox(screen.offsetWidth, screen.offsetHeight);
    });
    return changed;
}

function megabytes(bytes) {
    return (bytes / 1e6).toFixed(1);
}

function showProgress(bytes) {
    stats.downloaded = bytes;
    loadingBar.value = Math.min(1, bytes / ALL_BYTES);
    loadingText.textContent = "Loading the demo… " + megabytes(bytes) + " of " + megabytes(ALL_BYTES) + " MB";
}

function makeTerminal() {
    // At the size the application was saved at: the primer is that frame.
    const t = new window.Terminal({
        cols: SAVED.cols,
        rows: SAVED.rows,
        fontFamily: FACE + ', ui-monospace, "DejaVu Sans Mono", Menlo, Consolas, monospace',
        fontSize: grid.size,
        lineHeight: 1,
        scrollback: 0,
        cursorBlink: false,
        drawBoldTextInBrightColors: false,
        macOptionIsMeta: true,
        theme: { background: "#09090b", foreground: "#e4e4e7", cursor: "#e4e4e7", selectionBackground: "#52525b" },
    });
    // The developer tools' keys stay the browser's; everything else the
    // terminal turns into input, which also cancels the browser's own use
    // of it (F1, F3, F5, Ctrl+P, Ctrl+F, Ctrl+R).
    t.attachCustomKeyEventHandler((event) => {
        const devtools = event.key === "F12" || (event.ctrlKey && event.shiftKey && /^[ijc]$/i.test(event.key));
        return !devtools;
    });
    t.open(terminalBox);
    const webgl = drawnByWebgl(t);
    stats.renderer = webgl ? "webgl" : "dom";
    // The window was sized on a guess of the renderer; where the guess was
    // wrong, the cells are another width and the grid is worked out again.
    if (webgl !== onDevicePixels) {
        onDevicePixels = webgl;
        sizeWindow();
    }
    return t;
}

// The terminal's WebGL renderer in place of its default one. The default sets
// box-drawing and block characters as the typeface's glyphs, and a glyph does
// not land on the edges of a cell whose size is rounded to the screen's
// pixels: at a display scale of 100% solid bars break between rows, and at
// 125% a table's lines meet in teeth (the owner's screenshots, research
// §14.7). This one draws those characters itself, on the grid. Where it cannot
// start - no WebGL 2 - or loses its context later, the default one stays.
function drawnByWebgl(t) {
    try {
        const renderer = new window.WebglAddon.WebglAddon();
        renderer.onContextLoss(() => {
            // The terminal goes back to its default renderer, whose cells
            // are another width: the grid is worked out for them again.
            renderer.dispose();
            stats.renderer = "dom";
            onDevicePixels = false;
            sizeWindow();
            applyGrid();
        });
        t.loadAddon(renderer);
        return true;
    } catch {
        return false;
    }
}

// Shown in the window when the demo cannot start, with Start back in reach.
function failed(error) {
    loadingText.textContent = "The demo could not be loaded: " + error.message;
    startButton.disabled = false;
    startBox.hidden = false;
}

// The window first, at its final size, with the progress in it. It is shown
// before it is sized: what it measures has no size while hidden.
function showWindow() {
    ended.hidden = true;
    loading.hidden = false;
    stage.hidden = false;
    sizeWindow();
    showProgress(0);
    stage.scrollIntoView({ block: "start" });
}

async function start() {
    stats.clickedAt = performance.now();
    stats.readyAt = stats.firstFrameAt = stats.shownAt = null;
    startBox.hidden = true;
    startButton.disabled = true;
    againButton.disabled = true;
    statusLine.textContent = "";
    keys.hidden = true;

    await loadFaces();
    showWindow();
    const loaded = await loadParts();
    machine = loaded.machine;
    if (machine.cols !== SAVED.cols || machine.rows !== SAVED.rows) {
        throw new Error("the page and the machine disagree on the grid");
    }
    run(loaded.primer);
}

// One run of the machine, from its restore to the application's exit.
function run(primer) {
    const session = {
        machine: new window.V86({
            wasm_path: beside("vendor/v86.wasm"),
            memory_size: machine.memory_size,
            vga_memory_size: machine.vga_memory_size,
            cmdline: machine.cmdline,
            virtio_console: true,
            bios: { url: beside("vendor/seabios.bin") },
            vga_bios: { url: beside("vendor/vgabios.bin") },
            initial_state: { url: beside("state.bin.zst") },
            autostart: true,
            disable_keyboard: true,
            disable_mouse: true,
            disable_speaker: true,
        }),
        // What the machine writes before the terminal has been shown the
        // primer waits here.
        pending: [],
        primed: false,
        // The end of what the machine wrote, to see the application leave.
        tail: "",
        // The key being answered, for the page's own measurements.
        key: null,
        // The timer that takes the cover off, while the application is
        // drawing for the window's grid, and how much it has drawn since.
        redrawing: null,
        drawn: 0,
        // The scripts and the two small files are in by now; the emulator
        // reports the rest as it fetches it.
        fetched: {},
        before: ALL_BYTES - MACHINE_BYTES,
    };
    emulator = session.machine;
    window.__demo.emulator = session.machine;
    showProgress(session.before);
    session.machine.add_listener("download-progress", (event) => onProgress(session, event));
    session.machine.add_listener("virtio-console0-output-bytes", (bytes) => onOutput(session, bytes));
    session.machine.add_listener("emulator-ready", () => onReady(session, primer));
}

function onProgress(session, event) {
    session.fetched[event.file_name] = event.loaded;
    if (!session.primed) showProgress(session.before + Object.values(session.fetched).reduce((a, b) => a + b, 0));
}

const latin1 = new TextDecoder("latin1");

function onOutput(session, bytes) {
    // The emulator reuses its buffer: the terminal gets a copy.
    const chunk = new Uint8Array(bytes);
    const end = latin1.decode(chunk.subarray(-LEAVES_ALT_SCREEN.length));
    session.tail = (session.tail + end).slice(-LEAVES_ALT_SCREEN.length);
    if (session.primed) write(session, chunk);
    else session.pending.push(chunk);
    // The frame for the window's grid is arriving: shown once it is whole.
    if (session.redrawing) {
        session.drawn += chunk.length;
        if (session.drawn >= FRAME_BYTES) revealAfter(session, QUIET);
    }
    if (session.tail === LEAVES_ALT_SCREEN) finish(session.machine);
}

function revealAfter(session, ms) {
    clearTimeout(session.redrawing);
    session.redrawing = setTimeout(() => reveal(session), ms);
}

// The cover comes off: what is under it is the application at the window's
// grid, and from here on a visitor is watching it.
function reveal(session) {
    session.redrawing = null;
    if (emulator !== session.machine) return;
    stats.shownAt = performance.now();
    loading.hidden = true;
    statusLine.textContent = "Running. Ready " + ((stats.shownAt - stats.clickedAt) / 1000).toFixed(1) + " s after the click.";
    keys.hidden = false;
    term.focus();
}

function write(session, chunk) {
    const answered = session.key;
    term.write(chunk, () => {
        if (answered) answered.written.push(performance.now() - answered.at);
    });
}

function send(session, bytes) {
    session.machine.bus.send("virtio-console0-input-bytes", bytes);
}

function onReady(session, primer) {
    stats.readyAt = performance.now();
    term = makeTerminal();
    window.__demo.term = term;
    // The application drew this frame before the machine was saved and will
    // now send only what changes. The terminal answers the queries inside it;
    // nothing is listening to the answers yet, which is right - the
    // application asked them long ago.
    term.write(primer, () => onPrimed(session));
}

function onPrimed(session) {
    stats.firstFrameAt = performance.now();
    session.primed = true;
    term.onData((data) => {
        session.key = { at: performance.now(), written: [] };
        stats.keys.push(session.key);
        send(session, new TextEncoder().encode(data));
    });
    term.onBinary((data) => send(session, Uint8Array.from(data, (unit) => unit.codePointAt(0))));
    session.pending.forEach((chunk) => write(session, chunk));
    session.pending = [];
    // And now the window's own grid. Where it is not the grid the machine was
    // saved at, the application redraws for it - under the cover: a frame of
    // one size replaced by another a moment later is not a thing to watch.
    session.drawn = 0;
    if (applyGrid()) revealAfter(session, REDRAW_LIMIT);
    else reveal(session);
}

// The application quit: the machine has nothing more to show.
function finish(done) {
    if (emulator !== done) return;
    emulator = null;
    setTimeout(() => end(done).catch(failed), 150);
}

async function end(done) {
    await done.destroy();
    // A terminal keeps its listeners; a fresh one is made for the next run.
    if (term) term.dispose();
    term = null;
    window.__demo.term = null;
    terminalBox.textContent = "";
    ended.hidden = false;
    againButton.disabled = false;
    statusLine.textContent = "";
    againButton.focus();
}

// A window being dragged fires many times a second; the application is told
// once it has stopped.
let resizing = null;
window.addEventListener("resize", () => {
    if (stage.hidden) return;
    clearTimeout(resizing);
    resizing = setTimeout(() => {
        sizeWindow();
        applyGrid();
    }, 150);
});
startButton.addEventListener("click", () => start().catch(failed));
// A second machine in a page that has already run one can sit silent for
// seconds before it answers (docs/research/web-demo.md §13.4), so the next
// demo gets a page of its own: a reload, with everything it needs already in
// the browser's cache, at the address that starts by itself.
againButton.addEventListener("click", () => {
    location.hash = AUTOSTART;
    location.reload();
});
if (location.hash === AUTOSTART) {
    // The address goes back to the page's own: a reload by hand shows Start.
    history.replaceState(null, "", location.pathname + location.search);
    try {
        await start();
    } catch (error) {
        failed(error);
    }
}
