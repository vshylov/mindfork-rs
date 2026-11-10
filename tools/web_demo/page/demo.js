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
    const ended = $("ended");
    const terminalBox = $("terminal");

    // What the page keeps for itself around the terminal's grid, in pixels:
    // the window's title bar and padding, and the status line under it.
    const CHROME_HEIGHT = 96;
    const CHROME_WIDTH = 30;
    // The application leaves the alternate screen when it quits.
    const LEAVES_ALT_SCREEN = "\x1b[?1049l";

    // What the probe measures, readable from the console and by a test.
    // `keys`: for each key pressed, when it was pressed and how many
    // milliseconds later each piece of the answer had been handed to the
    // terminal - which paints it on the browser's next frame.
    const stats = { clickedAt: null, readyAt: null, firstFrameAt: null, downloaded: 0, fontSize: null, keys: [] };
    window.__demo = { stats };

    let machine = null;
    let term = null;
    let emulator = null;
    let loaded = null;

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

    // The emulator, the terminal and what both need - once per page.
    function loadParts() {
        if (!loaded) {
            loaded = Promise.all([
                fetch("machine.json").then((r) => r.json()),
                fetch("primer.bin").then((r) => r.arrayBuffer()),
                loadScript("vendor/xterm.js"),
                loadStyle("vendor/xterm.css"),
                loadScript("vendor/libv86.js"),
                document.fonts.load('400 14px "JetBrains Mono"'),
                document.fonts.load('700 14px "JetBrains Mono"'),
                document.fonts.load('italic 400 14px "JetBrains Mono"'),
            ]).then(([machineJson, primer]) => ({ machine: machineJson, primer: new Uint8Array(primer) }));
        }
        return loaded;
    }

    // A cell's width and height as fractions of the font size, measured from
    // the face itself: the terminal lays its grid out by the same two numbers.
    let cell = null;
    function cellRatio() {
        if (!cell) {
            const probe = document.createElement("span");
            probe.style.cssText =
                'position:absolute;visibility:hidden;white-space:pre;line-height:normal;font:100px "JetBrains Mono",monospace';
            probe.textContent = "MMMMMMMMMM";
            document.body.appendChild(probe);
            const box = probe.getBoundingClientRect();
            probe.remove();
            cell = { width: box.width / 1000, height: box.height / 100 };
        }
        return cell;
    }

    // The largest whole font size at which the whole grid fits the window:
    // its width, and its height too - the input box is the grid's last rows.
    function fontSizeFor(cols, rows) {
        const ratio = cellRatio();
        const width = Math.min(document.querySelector("main").clientWidth, window.innerWidth) - CHROME_WIDTH;
        const height = window.innerHeight - CHROME_HEIGHT;
        const size = Math.floor(Math.min(width / (cols * ratio.width), height / (rows * ratio.height)));
        return Math.max(6, Math.min(18, size));
    }

    function megabytes(bytes) {
        return (bytes / 1048576).toFixed(1);
    }

    function makeTerminal() {
        const t = new window.Terminal({
            cols: machine.cols,
            rows: machine.rows,
            fontFamily: '"JetBrains Mono", ui-monospace, "DejaVu Sans Mono", Menlo, Consolas, monospace',
            fontSize: (stats.fontSize = fontSizeFor(machine.cols, machine.rows)),
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
        ended.hidden = true;
        stage.hidden = false;
        keys.hidden = true;
        status.textContent = "Loading…";

        let primer;
        try {
            ({ machine, primer } = await loadParts());
        } catch (error) {
            status.textContent = "The demo could not be loaded: " + error.message;
            startButton.disabled = false;
            return;
        }
        if (!term) term = makeTerminal();
        term.reset();

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

        const parts = {};
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
        window.__demo.term = term;

        emulator.add_listener("download-progress", (e) => {
            parts[e.file_name] = e.loaded;
            const total = Object.values(parts).reduce((a, b) => a + b, 0);
            stats.downloaded = total;
            if (!primed) status.textContent = "Loading… " + megabytes(total) + " MB";
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
                status.textContent =
                    "Running. Ready " + ((stats.firstFrameAt - stats.clickedAt) / 1000).toFixed(1) + " s after the click.";
                keys.hidden = false;
                stage.scrollIntoView({ block: "start" });
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
            terminalBox.textContent = "";
            ended.hidden = false;
            againButton.disabled = false;
            status.textContent = "";
            againButton.focus();
        }, 150);
    }

    window.addEventListener("resize", () => {
        if (term && machine) term.options.fontSize = stats.fontSize = fontSizeFor(machine.cols, machine.rows);
    });
    startButton.addEventListener("click", () => {
        document.getElementById("start").hidden = true;
        start();
    });
    againButton.addEventListener("click", start);
})();
