#!/usr/bin/env python3
"""Console probe: drive the TUI in a real Windows console, and read the screen back.

Why — the app needs a real terminal, so a run started from an agent's shell or
from CI exits with code 2, and every "does it look right" question has been
answered by a person looking. Some of those questions are not about looks at
all: *is every cell painted*, *what background did the cell behind a wide glyph
end up with*. A console keeps a buffer of characters and attributes, and the
console API reads it — so those can be measured.

How — the app is started in a **new, hidden console** (`CREATE_NEW_CONSOLE` with
`SW_HIDE`: the inbox console host, no window on the desktop, no hand-off to
Windows Terminal), the probe attaches to that console, injects key events
(`WriteConsoleInputW`) and reads the active screen buffer
(`ReadConsoleOutputW`) — characters and attributes, cell by cell.

What it can and cannot see:

* A cell's colours come back as the **nearest of the 16 legacy colours**. So
  the scenarios use the *light* full theme: its canvas reads as bright white
  (15) and the console's own background as black (0), and a cell the app failed
  to paint is a black one. The selection backdrop also reads as white, and the
  dark canvas as black — neither can be told from its surroundings this way.
* It measures the **inbox console host of the machine it runs on**. Windows
  Terminal and other hosts are not console buffers the API can read.
* A hidden console does not resize, so a resize cannot be exercised here.

Scenarios (`--scenario`):

* `full-mode` (default) — `mindfork demo` (throwaway data root, scripted
  engine, no instance guard): Settings → Interface → Colour mode → full,
  Theme → light; then the chat, the emoji picker (44 wide glyphs), the help
  dialog, and back to the system mode. Every cell must be painted in the full
  mode, and the console's own background must be back in the system mode.
* `first-frame` — a copy of the binary in a scratch directory with its own data
  root, started **already in** the full light mode with wide glyphs in its very
  first frame (restored into the input box from the saved draft). The cells
  behind those glyphs are never written by the app (ratatui leaves a wide
  glyph's trailing cell out of the diff), so this is where a terminal's own
  choice shows. Takes the app's single-instance lock: close a running mindfork
  first.

It is a **spike tool**, not part of the app and not run in CI: run it by hand
after `cargo build`, and paste its output into the journal entry.

Usage:

    python tools/console_probe.py
    python tools/console_probe.py --scenario first-frame
    python tools/console_probe.py --exe target/release/mindfork.exe

Exit code: 0 — every check passed, 1 — a check failed, 2 — cannot run here.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import shutil
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path

if sys.platform != "win32":
    print("console_probe: Windows only — it reads a Windows console's buffer")
    sys.exit(2)

import ctypes.wintypes as wt  # noqa: E402  (after the platform gate)

KERNEL32 = ctypes.WinDLL("kernel32", use_last_error=True)
USER32 = ctypes.WinDLL("user32", use_last_error=True)

GENERIC_READ_WRITE = 0x80000000 | 0x40000000
FILE_SHARE_READ_WRITE = 1 | 2
OPEN_EXISTING = 3
INVALID_HANDLE = wt.HANDLE(-1).value
KEY_EVENT = 0x0001
LEFT_CTRL_PRESSED = 0x0008
ENHANCED_KEY = 0x0100
# A wide glyph takes two cells: the console marks the second one.
TRAILING_CELL = 0x0200
# Legacy colour indexes, as `ReadConsoleOutputW` reports a background.
BLACK = 0
BRIGHT_WHITE = 15

# How long the app gets to draw after a key (the loop ticks every 50 ms; a
# screen switch or a mode change repaints everything).
SETTLE = 0.35
SETTLE_REPAINT = 0.8
STARTUP = 3.5


class Coord(ctypes.Structure):
    _fields_ = [("X", wt.SHORT), ("Y", wt.SHORT)]


class SmallRect(ctypes.Structure):
    _fields_ = [
        ("Left", wt.SHORT),
        ("Top", wt.SHORT),
        ("Right", wt.SHORT),
        ("Bottom", wt.SHORT),
    ]


class CharInfo(ctypes.Structure):
    _fields_ = [("Char", wt.WCHAR), ("Attributes", wt.WORD)]


class ScreenBufferInfo(ctypes.Structure):
    _fields_ = [
        ("dwSize", Coord),
        ("dwCursorPosition", Coord),
        ("wAttributes", wt.WORD),
        ("srWindow", SmallRect),
        ("dwMaximumWindowSize", Coord),
    ]


class KeyEventRecord(ctypes.Structure):
    _fields_ = [
        ("bKeyDown", wt.BOOL),
        ("wRepeatCount", wt.WORD),
        ("wVirtualKeyCode", wt.WORD),
        ("wVirtualScanCode", wt.WORD),
        ("uChar", wt.WCHAR),
        ("dwControlKeyState", wt.DWORD),
    ]


class EventUnion(ctypes.Union):
    _fields_ = [("KeyEvent", KeyEventRecord), ("pad", ctypes.c_byte * 16)]


class InputRecord(ctypes.Structure):
    _fields_ = [("EventType", wt.WORD), ("Event", EventUnion)]


KERNEL32.CreateFileW.restype = wt.HANDLE
KERNEL32.CreateFileW.argtypes = [
    wt.LPCWSTR,
    wt.DWORD,
    wt.DWORD,
    ctypes.c_void_p,
    wt.DWORD,
    wt.DWORD,
    wt.HANDLE,
]
KERNEL32.ReadConsoleOutputW.argtypes = [
    wt.HANDLE,
    ctypes.POINTER(CharInfo),
    Coord,
    Coord,
    ctypes.POINTER(SmallRect),
]
KERNEL32.WriteConsoleInputW.argtypes = [
    wt.HANDLE,
    ctypes.POINTER(InputRecord),
    wt.DWORD,
    ctypes.POINTER(wt.DWORD),
]
KERNEL32.GetConsoleScreenBufferInfo.argtypes = [
    wt.HANDLE,
    ctypes.POINTER(ScreenBufferInfo),
]

# name -> (virtual key, character, control-key state)
KEYS = {
    "tab": (0x09, "\t", 0),
    "enter": (0x0D, "\r", 0),
    "esc": (0x1B, "\x1b", 0),
    "left": (0x25, "\0", ENHANCED_KEY),
    "right": (0x27, "\0", ENHANCED_KEY),
    "down": (0x28, "\0", ENHANCED_KEY),
    "f1": (0x70, "\0", 0),
    "ctrl+b": (0x42, "\x02", LEFT_CTRL_PRESSED),
    "ctrl+p": (0x50, "\x10", LEFT_CTRL_PRESSED),
    "ctrl+q": (0x51, "\x11", LEFT_CTRL_PRESSED),
}

# Settings opens on the first section; "Interface" is the eighth.
TABS_TO_INTERFACE = 7


def _check(ok: object, what: str) -> None:
    if not ok:
        raise OSError(f"{what} failed: error {ctypes.get_last_error()}")


def _open(name: str) -> int:
    handle = KERNEL32.CreateFileW(
        name, GENERIC_READ_WRITE, FILE_SHARE_READ_WRITE, None, OPEN_EXISTING, 0, None
    )
    _check(handle not in (None, INVALID_HANDLE), f"opening {name}")
    return handle


def _send(vk: int, char: str, state: int, settle: float) -> None:
    """One key, down and up, into the attached console's input buffer.

    One key at a time, with a pause: the app reads a burst of events in one
    drain as a paste."""
    records = (InputRecord * 2)()
    for record, down in zip(records, (True, False)):
        record.EventType = KEY_EVENT
        key = record.Event.KeyEvent
        key.bKeyDown = down
        key.wRepeatCount = 1
        key.wVirtualKeyCode = vk
        key.wVirtualScanCode = USER32.MapVirtualKeyW(vk, 0) if vk else 0
        key.uChar = char
        key.dwControlKeyState = state
    handle = _open("CONIN$")
    try:
        written = wt.DWORD(0)
        _check(
            KERNEL32.WriteConsoleInputW(handle, records, 2, ctypes.byref(written)),
            "WriteConsoleInputW",
        )
    finally:
        KERNEL32.CloseHandle(handle)
    time.sleep(settle)


def press(name: str, settle: float = SETTLE) -> None:
    vk, char, state = KEYS[name]
    _send(vk, char, state, settle)


def type_text(text: str) -> None:
    """Characters of the Basic Multilingual Plane, as typed. A surrogate pair
    injected this way is not taken as input — use the emoji picker for those."""
    for char in text:
        _send(0, char, 0, 0.25)


def read_screen() -> list[list[tuple[str, int]]]:
    """The visible screen: rows of `(character, attributes)`."""
    handle = _open("CONOUT$")
    try:
        info = ScreenBufferInfo()
        _check(
            KERNEL32.GetConsoleScreenBufferInfo(handle, ctypes.byref(info)),
            "GetConsoleScreenBufferInfo",
        )
        window = info.srWindow
        width = window.Right - window.Left + 1
        rows = []
        for y in range(window.Top, window.Bottom + 1):
            cells = (CharInfo * width)()
            rect = SmallRect(window.Left, y, window.Right, y)
            _check(
                KERNEL32.ReadConsoleOutputW(
                    handle, cells, Coord(width, 1), Coord(0, 0), ctypes.byref(rect)
                ),
                "ReadConsoleOutputW",
            )
            rows.append([(cell.Char, cell.Attributes) for cell in cells])
        return rows
    finally:
        KERNEL32.CloseHandle(handle)


def background(attributes: int) -> int:
    return (attributes >> 4) & 0xF


class Session:
    """The app in its own hidden console, with this process attached to it."""

    def __init__(self, exe: Path, args: list[str], cwd: Path | None = None):
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0  # SW_HIDE
        self.process = subprocess.Popen(
            [str(exe), *args],
            cwd=cwd,
            creationflags=subprocess.CREATE_NEW_CONSOLE,
            startupinfo=startup,
        )
        time.sleep(STARTUP)
        if self.process.poll() is not None:
            raise RuntimeError(
                f"the app exited at once with code {self.process.returncode}"
                " (2 — no terminal, or another instance is running)"
            )
        KERNEL32.FreeConsole()
        _check(KERNEL32.AttachConsole(self.process.pid), "AttachConsole")

    def close(self) -> int | None:
        try:
            press("esc")
            press("ctrl+q", 1.2)
        except OSError:
            pass
        KERNEL32.FreeConsole()
        try:
            return self.process.wait(timeout=6)
        except subprocess.TimeoutExpired:
            self.process.kill()
            return None


class Report:
    """Collects the checks of a scenario, printing each as it is made."""

    def __init__(self) -> None:
        self.failed = 0

    def check(self, ok: bool, what: str) -> None:
        print(f"  {'ok  ' if ok else 'FAIL'}  {what}")
        self.failed += 0 if ok else 1

    def screen(self, title: str, rows: list[list[tuple[str, int]]]) -> None:
        cells = [cell for row in rows for cell in row]
        counts = Counter(background(a) for _, a in cells)
        print(f"- {title}: {len(rows[0])}x{len(rows)}, backgrounds {dict(sorted(counts.items()))}")

    def painted(self, title: str, rows: list[list[tuple[str, int]]]) -> None:
        """Every cell carries the (light) canvas; none the console's own."""
        self.screen(title, rows)
        bare = [
            (x, y)
            for y, row in enumerate(rows)
            for x, (_, attributes) in enumerate(row)
            if background(attributes) != BRIGHT_WHITE
        ]
        self.check(not bare, f"{title}: every cell is painted ({len(bare)} are not: {bare[:6]})")

    def wide_glyphs(self, title: str, rows: list[list[tuple[str, int]]], at_least: int) -> None:
        """The cell behind each wide glyph has the glyph's own background."""
        trailing = [
            (x, y, background(row[x - 1][1]), background(attributes))
            for y, row in enumerate(rows)
            for x, (_, attributes) in enumerate(row)
            if attributes & TRAILING_CELL and x > 0
        ]
        self.check(
            len(trailing) >= at_least,
            f"{title}: {len(trailing)} wide glyphs on screen (at least {at_least} expected)",
        )
        odd = [(x, y) for x, y, glyph, behind in trailing if glyph != behind]
        self.check(
            not odd,
            f"{title}: the cell behind each wide glyph has its background"
            f" ({len(odd)} differ: {odd[:6]})",
        )


def to_interface_fields() -> None:
    press("ctrl+p", SETTLE_REPAINT)
    for _ in range(TABS_TO_INTERFACE):
        press("tab")
    press("enter")


def open_emoji_picker() -> None:
    press("ctrl+b", SETTLE_REPAINT)


def scenario_full_mode(exe: Path, report: Report) -> None:
    session = Session(exe, ["demo"])
    try:
        rows = read_screen()
        report.screen("as launched (system mode)", rows)
        report.check(
            all(background(a) == BLACK for row in rows for _, a in row),
            "the system mode paints no background",
        )

        to_interface_fields()  # the first field is "Colour mode"
        press("right", SETTLE_REPAINT)  # system -> full
        press("down")
        press("right", SETTLE_REPAINT)  # theme: dark -> light
        report.painted("settings, full / light", read_screen())

        press("esc")
        press("esc", SETTLE_REPAINT)
        report.painted("chat", read_screen())

        open_emoji_picker()
        rows = read_screen()
        report.painted("emoji picker over the chat", rows)
        report.wide_glyphs("emoji picker", rows, at_least=40)
        for _ in range(3):
            press("right")
        press("down")
        rows = read_screen()
        report.painted("emoji picker, selection moved", rows)
        report.wide_glyphs("emoji picker, selection moved", rows, at_least=40)
        press("esc", SETTLE_REPAINT)
        report.painted("emoji picker closed", read_screen())

        press("f1", SETTLE_REPAINT)
        report.painted("help dialog over the chat", read_screen())
        press("esc", SETTLE_REPAINT)

        to_interface_fields()
        press("left", SETTLE_REPAINT)  # full -> system
        rows = read_screen()
        report.screen("back in the system mode", rows)
        report.check(
            all(background(a) == BLACK for row in rows for _, a in row),
            "the console's own background is back",
        )
        press("esc")
    finally:
        code = session.close()
    report.check(code == 0, f"the app exited with code {code}")


def scenario_first_frame(exe: Path, report: Report) -> None:
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        (root / "data").mkdir()
        settings = {
            "schema_version": 3,
            "interface": {"language": "en", "theme_mode": "full", "full_theme": "light"},
        }
        (root / "data" / "settings.json").write_text(json.dumps(settings), encoding="utf-8")

        print("first run: type the draft")
        session = Session(copy, [], cwd=root)
        try:
            report.painted("started in full / light", read_screen())
            type_text("中文字 wide ")
            for moves in (0, 3):  # two emoji, through the app's own picker
                open_emoji_picker()
                for _ in range(moves):
                    press("right", 0.2)
                press("enter", SETTLE_REPAINT)
            time.sleep(1.5)  # the draft is saved with a debounce
            rows = read_screen()
            report.wide_glyphs("the input box", rows, at_least=5)
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")

        print("second run: the draft is in the very first frame")
        session = Session(copy, [], cwd=root)
        try:
            rows = read_screen()
            report.painted("first frame", rows)
            report.wide_glyphs("first frame", rows, at_least=5)
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")


SCENARIOS = {"full-mode": scenario_full_mode, "first-frame": scenario_first_frame}


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Drive the TUI in a hidden Windows console and read the screen back."
    )
    parser.add_argument("--exe", default="target/debug/mindfork.exe", help="the binary to run")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), default="full-mode")
    args = parser.parse_args()
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    exe = Path(args.exe).resolve()
    if not exe.is_file():
        print(f"console_probe: {exe} does not exist — `cargo build` first")
        return 2

    report = Report()
    print(f"console_probe: {args.scenario} — {exe}")
    try:
        SCENARIOS[args.scenario](exe, report)
    except (OSError, RuntimeError) as error:
        print(f"console_probe: cannot run here — {error}")
        return 2
    print(f"console_probe: {'all checks passed' if not report.failed else f'{report.failed} check(s) FAILED'}")
    return 1 if report.failed else 0


if __name__ == "__main__":
    sys.exit(main())
