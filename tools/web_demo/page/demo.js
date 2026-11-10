// The web demo probe's page (docs/research/web-demo.md, stage 1): a saved
// machine with `mindfork demo` already running is restored into an x86
// emulator in this tab, and its serial line is joined to a terminal.
//
// Nothing heavy is loaded before Start is pressed: the emulator, the terminal
// and the machine's state are fetched on the click.
(function () {
    "use strict";

    const $ = (id) => document.getElementById(id);
    const startButton = $("start-button");
    const againButton = $("again-button");
    const stage = $("stage");
    const status = $("status");
    const keys = $("keys");
    const loading = $("loading");
    const loadingText = $("loading-text");
    const loadingBar = $("loading-bar");
    const ended = $("ended");
    const terminalBox = $("terminal");

    // The grid the application was started at, and what a click downloads:
    // written into the page when it is built, so the window can take its size
    // and the progress its scale before anything has been fetched.
    const COLS = Number(stage.dataset.cols);
    const ROWS = Number(stage.dataset.rows);
    const ALL_BYTES = Number(stage.dataset.bytes);
    const MACHINE_BYTES = Number(stage.dataset.machineBytes);

    // What the page keeps for itself around the terminal's grid, in pixels:
    // the window's title bar and padding, and the status line under it.
    const CHROME_HEIGHT = 96;
    const CHROME_WIDTH = 30;
    // The terminal box's padding, as demo.css sets it, and the smallest font.
    const PADDING = 6;
    const MIN_FONT = 6;
    const MAX_FONT = 18;
    // The application leaves the alternate screen when it quits.
    const LEAVES_ALT_SCREEN = "\x1b[?1049l";
    const FACE = '"JetBrains Mono"';

    // What the probe measures, readable from the console and by a test.
    // `keys`: for each key pressed, when it was pressed and how many
    // milliseconds later each piece of the answer had been handed to the
    // terminal - which paints it on the browser's next frame.
    const stats = { clickedAt: null, readyAt: null, firstFrameAt: null, downloaded: 0, fontSize: null, keys: [] };
    window.__demo = { stats };

    let term = null;
    let emulator = null;
    let parts = null;

    // Below this the grid still fits, at a font size under ten pixels.
    if (window.innerWidth < 900 || window.innerHeight < 680 || window.matchMedia("(pointer: coarse)").matches) {
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
                fetch("machine.json").then((r) => r.json()),
                fetch("primer.bin").then((r) => r.arrayBuffer()),
                loadScript("vendor/xterm.js"),
                loadStyle("vendor/xterm.css"),
                loadScript("vendor/libv86.js"),
            ]).then(([machine, primer]) => ({ machine, primer: new Uint8Array(primer) }));
        }
        return parts;
    }

    // The grid's size in pixels at a font size, worked out the way the
    // terminal does it: the face's advance and line box measured on a canvas
    // at that very size - where a browser may round them - and the line box
    // rounded up to whole device pixels.
    const ruler = document.createElement("canvas").getContext("2d");
    function gridAt(size) {
        ruler.font = size + "px " + FACE + ", monospace";
        const metrics = ruler.measureText("W");
        const lineBox = metrics.fontBoundingBoxAscent + metrics.fontBoundingBoxDescent || size * 1.32;
        const dpr = window.devicePixelRatio || 1;
        return {
            width: Math.round(metrics.width * COLS),
            height: Math.round((Math.ceil(lineBox * dpr) * ROWS) / dpr),
        };
    }

    // The room the grid has: the page's width and the window's height, less
    // what the page keeps around it.
    function room() {
        return {
            width: Math.min(document.querySelector("main").clientWidth, window.innerWidth) - CHROME_WIDTH,
            height: window.innerHeight - CHROME_HEIGHT,
        };
    }

    // The largest whole font size at which the whole grid fits the window:
    // its width, and its height too - the input box is the grid's last rows.
    function fontSize() {
        const { width, height } = room();
        for (let size = MAX_FONT; size > MIN_FONT; size -= 1) {
            const grid = gridAt(size);
            if (grid.width <= width && grid.height <= height) return size;
        }
        return MIN_FONT;
    }

    // The window takes the grid's size whether or not a terminal is in it:
    // while the demo loads, and after it has ended.
    function sizeWindow() {
        stats.fontSize = fontSize();
        const grid = gridAt(stats.fontSize);
        setBox(grid.width, grid.height);
        if (term) {
            term.options.fontSize = stats.fontSize;
            requestAnimationFrame(fitTerminal);
        }
    }

    function setBox(width, height) {
        terminalBox.style.width = width + 2 * PADDING + "px";
        terminalBox.style.height = height + 2 * PADDING + "px";
    }

    // Should the terminal's own grid come out other than `gridAt` worked out,
    // the window takes the real one, and the font steps down while it overflows.
    function fitTerminal() {
        const screen = term && terminalBox.querySelector(".xterm-screen");
        if (!screen) return;
        const { width, height } = room();
        if (stats.fontSize > MIN_FONT && (screen.offsetWidth > width || screen.offsetHeight > height)) {
            stats.fontSize -= 1;
            term.options.fontSize = stats.fontSize;
            requestAnimationFrame(fitTerminal);
            return;
        }
        setBox(screen.offsetWidth, screen.offsetHeight);
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
        const t = new window.Terminal({
            cols: COLS,
            rows: ROWS,
            fontFamily: FACE + ', ui-monospace, "DejaVu Sans Mono", Menlo, Consolas, monospace',
            fontSize: stats.fontSize,
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
        return t;
    }

    async function start() {
        stats.clickedAt = performance.now();
        stats.readyAt = stats.firstFrameAt = null;
        startButton.disabled = true;
        againButton.disabled = true;
        status.textContent = "";
        keys.hidden = true;

        // The window first, at its final size, with the progress in it.
        await loadFaces();
        sizeWindow();
        ended.hidden = true;
        loading.hidden = false;
        showProgress(0);
        stage.hidden = false;
        stage.scrollIntoView({ block: "start" });

        let machine;
        let primer;
        try {
            ({ machine, primer } = await loadParts());
        } catch (error) {
            loadingText.textContent = "The demo could not be loaded: " + error.message;
            startButton.disabled = false;
            return;
        }
        if (machine.cols !== COLS || machine.rows !== ROWS) {
            loadingText.textContent = "The demo could not be loaded: the page and the machine disagree on the grid.";
            return;
        }
        // The scripts and the two small files are in; the emulator reports
        // the rest as it fetches it.
        const before = ALL_BYTES - MACHINE_BYTES;
        showProgress(before);

        // What the machine writes before the terminal has been shown the
        // primer waits here; afterwards a slice of emulation ends in one write.
        let pending = [];
        let primed = false;
        let scheduled = false;
        let tail = "";
        let key = null;
        const flush = () => {
            scheduled = false;
            if (!primed || pending.length === 0) return;
            const chunk = new Uint8Array(pending);
            pending = [];
            const answered = key;
            term.write(chunk, () => {
                if (answered) answered.written.push(performance.now() - answered.at);
            });
        };

        const fetched = {};
        emulator = new window.V86({
            wasm_path: "vendor/v86.wasm",
            memory_size: machine.memory_size,
            vga_memory_size: machine.vga_memory_size,
            cmdline: machine.cmdline,
            filesystem: {},
            bios: { url: "vendor/seabios.bin" },
            vga_bios: { url: "vendor/vgabios.bin" },
            initial_state: { url: "state.bin.zst" },
            autostart: true,
            disable_keyboard: true,
            disable_mouse: true,
            disable_speaker: true,
        });
        window.__demo.emulator = emulator;

        emulator.add_listener("download-progress", (e) => {
            fetched[e.file_name] = e.loaded;
            if (!primed) showProgress(before + Object.values(fetched).reduce((a, b) => a + b, 0));
        });
        emulator.add_listener("serial0-output-byte", (byte) => {
            pending.push(byte);
            tail = (tail + String.fromCharCode(byte)).slice(-LEAVES_ALT_SCREEN.length);
            if (tail === LEAVES_ALT_SCREEN) finish();
            if (!scheduled) {
                scheduled = true;
                queueMicrotask(flush);
            }
        });
        emulator.add_listener("emulator-ready", () => {
            stats.readyAt = performance.now();
            term = makeTerminal();
            window.__demo.term = term;
            requestAnimationFrame(fitTerminal);
            // The application drew this frame before the machine was saved and
            // will now send only what changes. The terminal answers the
            // queries inside it; nothing is listening to the answers yet, which
            // is right - the application asked them long ago.
            term.write(primer, () => {
                stats.firstFrameAt = performance.now();
                primed = true;
                term.onData((data) => {
                    key = { at: performance.now(), written: [] };
                    stats.keys.push(key);
                    emulator.serial_send_bytes(0, new TextEncoder().encode(data));
                });
                term.onBinary((data) => {
                    emulator.serial_send_bytes(0, Uint8Array.from(data, (c) => c.charCodeAt(0)));
                });
                flush();
                loading.hidden = true;
                status.textContent =
                    "Running. Ready " + ((stats.firstFrameAt - stats.clickedAt) / 1000).toFixed(1) + " s after the click.";
                keys.hidden = false;
                term.focus();
            });
        });
    }

    // The application quit: the machine has nothing more to show.
    function finish() {
        const done = emulator;
        emulator = null;
        if (!done) return;
        setTimeout(() => {
            done.destroy();
            // A terminal keeps its listeners; a fresh one is made for the next run.
            if (term) term.dispose();
            term = null;
            window.__demo.term = null;
            terminalBox.textContent = "";
            ended.hidden = false;
            againButton.disabled = false;
            status.textContent = "";
            againButton.focus();
        }, 150);
    }

    window.addEventListener("resize", () => {
        if (!stage.hidden) sizeWindow();
    });
    startButton.addEventListener("click", () => {
        $("start").hidden = true;
        start();
    });
    againButton.addEventListener("click", start);
})();
