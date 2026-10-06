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
* A hidden console does not resize — `SetConsoleWindowInfo` succeeds and
  changes nothing — so a resize cannot be exercised here. A console can be
  **started** at a size, though: `mode con` runs in it first
  (`Session(size=...)`), which is how the `small-window` scenario gets a
  57×5 one.
* Attributes other than colour come back as the console keeps them: reverse
  video is a flag of the cell (`COMMON_LVB_REVERSE_VIDEO`), underline another;
  bold reads as the bright half of the colour, dim and italic are not kept.
  So "no styling" is measurable as *every cell carries the console's default
  attributes*, which is what the monochrome scenarios check.

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
* `mono` — `mindfork demo`: the launch frame as the control (the system mode
  is coloured), then Settings → Interface → Colour mode → monochrome; the
  settings screen, the chat, the help dialog and the emoji picker must carry
  the console's default attributes in every cell, and a text selected in the
  input box must be the only reverse video on the screen.
* `no-color` — `mindfork demo` three times: with `NO_COLOR=1`, where the very
  first frame must already be bare; with `NO_COLOR=` (empty), which is an
  unset one; and without it.
* `gateway` — a copy of the binary in a scratch directory whose settings name
  the `openrouter` mode, against the **real gateway** (it needs
  `OPENROUTER_API_KEY`, which the settings read by name, and spends a fraction
  of a cent): a question typed into the chat is answered, both of the mode's
  slots are ready in the status line, a file is indexed through the gateway's
  embedder, the settings show the provider's rows with the attribution switch,
  and `Enter` on the model row lists the gateway's catalogue with the window
  and the price. Speech goes the same way: `/tts` reads the reply aloud
  through the gateway — **audibly**, it needs a sound card — the Speech tab
  shows the provider's rows and none for instructions, its model row lists the
  gateway's speech models with their voices counted and no price, and its
  voice row lists the voices of the model named. And video: the settings'
  search finds the video group, which names its provider and has no row for
  the resolution, and its model row lists the models that take video, the
  Gemini family first. Takes the single-instance lock, like `first-frame`.
* `user-theme` — a copy of the binary in a scratch directory whose
  `data/themes/` holds a theme of one colour (a light canvas) next to a file
  that is not a theme, with the settings naming that theme. The first frame
  must be painted with it, the settings row must show its name, and the log
  must say what the other file was. The control is the same run with the
  theme taken away: the name then answers to nothing and the canvas is the
  dark one. Takes the single-instance lock, like `first-frame`.

* `altgr` — `mindfork demo`: text typed with AltGr reaches the input box. Windows
  reports AltGr as Ctrl+Alt, so the probe injects AltGr+8 as the console gives
  it — the ruble sign with `LEFT_CTRL | RIGHT_ALT` — and needs the Russian
  layout installed, where that is the ruble's key. The control arm is Ctrl+Alt+A,
  which no installed layout maps: it must type nothing, and `Ctrl+K` must still
  clear the box. Then the chat list's search line, which takes text of its own.

* `ollama` — a copy of the binary in a scratch directory, set up by the
  README's Ollama recipe (`mindfork setup --set …`, word for word), against a
  **running Ollama** with the model pulled and `OLLAMA_CONTEXT_LENGTH` set
  (`MINDFORK_OLLAMA_URL` and `MINDFORK_OLLAMA_MODEL` override the recipe's
  values, and `MINDFORK_OLLAMA_CONTEXT` types a window as 0.15.0's recipe did): a question is
  answered, no slow-prefill note names llama-server's launch flags — Ollama
  sends llama.cpp's timings, its first with the model's load in them — a note
  is saved through the agentic loop, and `Enter` on the model row lists what
  the server serves. With no window typed the app must have read Ollama's own
  from `/api/ps` after the first turn (its log says so); a server whose window
  is 4096 or less must have been told it is too small, and no round may be
  told as cut (the control of `ollama-cut`). Takes the single-instance lock,
  like `first-frame`.
* `ollama-cut` — the same recipe against an Ollama started with
  `OLLAMA_CONTEXT_LENGTH=4096`: a short question fits; a page pasted after it
  does not fit beside the instructions, so Ollama cuts the prompt to half its
  window from the front and answers. The feed must carry the cut-prompt note
  and the app's log the cut round (docs/research/prompt-cut-detection.md).
* `lmstudio` — LM Studio's recipe against its **running server**, the model
  loaded first at a small window (`lms load <model> -c 4096`;
  `MINDFORK_LMSTUDIO_URL` and `MINDFORK_LMSTUDIO_MODEL` override the URL and
  the model's key): a question is answered with no launch line, the app has
  read LM Studio's window from `/api/v1/models` and knows the server (its log
  says so), the window-too-small note names LM Studio's setting and not
  Ollama's, a note is saved, and no round is told as cut
  (docs/research/local-servers.md).
* `lmstudio-cut` — the same, the model loaded at 8192: a question and a page
  fit; a second page does not, and LM Studio drops the middle of the
  conversation in silence. The feed must carry the cut-prompt note in LM
  Studio's words and the app's log the cut round.
* `first-run` — a fresh copy with no engine, Ollama and LM Studio both running
  (docs/research/local-servers.md, stage 2): the *Local servers* list opens by
  itself and names both; `Enter` on Ollama's row answers a question; `/local`
  lists again, and LM Studio's row brings its embedder, which a saved note
  then uses — `settings.json` holds each pick. `first-run-none` — the same with
  neither running: the empty feed names `/local`, no list opens, and `/local`
  says nothing answered.

* `small-window` — `mindfork demo` in consoles started small (spec §11.1.1):
  at 57×5, the window of the report, the frame is the "window too small"
  notice and no key but quit does anything; at 57×9 it is the chat, whole;
  and at 45×12 — a window the chat fits — opening the settings (46 columns)
  or the emoji picker puts the notice up with no resize, `Esc` takes it
  down, and the chat comes back cell for cell.

It is a **spike tool**, not part of the app and not run in CI: run it by hand
after `cargo build`, and paste its output into the journal entry.

Usage:

    python tools/console_probe.py
    python tools/console_probe.py --scenario first-frame
    python tools/console_probe.py --scenario mono
    python tools/console_probe.py --scenario no-color
    python tools/console_probe.py --scenario user-theme
    python tools/console_probe.py --scenario gateway
    python tools/console_probe.py --scenario small-window
    python tools/console_probe.py --scenario altgr
    python tools/console_probe.py --scenario ollama
    python tools/console_probe.py --exe target/release/mindfork.exe

Exit code: 0 — every check passed, 1 — a check failed, 2 — cannot run here.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
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
RIGHT_ALT_PRESSED = 0x0001
LEFT_ALT_PRESSED = 0x0002
LEFT_CTRL_PRESSED = 0x0008
# What Windows reports for AltGr: the right Alt, and a left Ctrl it synthesizes.
ALTGR = LEFT_CTRL_PRESSED | RIGHT_ALT_PRESSED
SHIFT_PRESSED = 0x0010
ENHANCED_KEY = 0x0100
# A wide glyph takes two cells, and the console marks both: the first as the
# leading one, the second as the trailing one.
LEADING_CELL = 0x0100
TRAILING_CELL = 0x0200
# Reverse video, as the console keeps it: a flag of the cell.
REVERSE_VIDEO = 0x4000
# The part of a cell's attributes that is its colours.
COLOURS = 0x00FF
# What in a cell's attributes is not styling: which half of a wide glyph it is.
NOT_STYLING = LEADING_CELL | TRAILING_CELL
# Legacy colour indexes, as `ReadConsoleOutputW` reports a background.
BLACK = 0
BRIGHT_WHITE = 15

# What opens the settings screen and the emoji picker, and the file the settings
# are kept in — each said once, for the scenarios that press the one and write
# the other.
SETTINGS_KEY = "ctrl+p"
EMOJI_KEY = "ctrl+b"
SETTINGS_FILE = "settings.json"
# What clears the input box, and the AltGr chord that types the ruble sign —
# the `altgr` scenario presses each more than once.
CLEAR_KEY = "ctrl+k"
RUBLE_KEY = "altgr+8"
# The schema the scenarios write their settings at: the app's own
# `SETTINGS_SCHEMA`. A file below it is migrated at the start — with a
# pre-migration backup no scenario asked for — and one above it is refused.
SETTINGS_SCHEMA = 5

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
    "up": (0x26, "\0", ENHANCED_KEY),
    "right": (0x27, "\0", ENHANCED_KEY),
    "down": (0x28, "\0", ENHANCED_KEY),
    "f1": (0x70, "\0", 0),
    "shift+left": (0x25, "\0", ENHANCED_KEY | SHIFT_PRESSED),
    EMOJI_KEY: (0x42, "\x02", LEFT_CTRL_PRESSED),
    SETTINGS_KEY: (0x50, "\x10", LEFT_CTRL_PRESSED),
    "ctrl+q": (0x51, "\x11", LEFT_CTRL_PRESSED),
    CLEAR_KEY: (0x4B, "\x0b", LEFT_CTRL_PRESSED),
    # AltGr+8 on the Russian layout (Windows 8.1 on): the ruble sign. The
    # `altgr` scenario needs that layout installed, and says so if it is not.
    RUBLE_KEY: (0x38, "₽", ALTGR),
    # The left Ctrl and Alt on a key no installed layout maps under AltGr: the
    # console gives no character, and crossterm names the key's own.
    "ctrl+alt+a": (0x41, "\0", LEFT_CTRL_PRESSED | LEFT_ALT_PRESSED),
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


def paste_text(text: str) -> None:
    """Characters in one burst, which the app reads as a paste — a page put
    into the input at once. The Basic Multilingual Plane, no line breaks."""
    records = (InputRecord * (2 * len(text)))()
    for i, char in enumerate(text):
        for k, down in enumerate((True, False)):
            record = records[2 * i + k]
            record.EventType = KEY_EVENT
            key = record.Event.KeyEvent
            key.bKeyDown = down
            key.wRepeatCount = 1
            key.uChar = char
    handle = _open("CONIN$")
    try:
        written = wt.DWORD(0)
        _check(
            KERNEL32.WriteConsoleInputW(handle, records, len(records), ctypes.byref(written)),
            "WriteConsoleInputW",
        )
    finally:
        KERNEL32.CloseHandle(handle)
    time.sleep(SETTLE_REPAINT)


def read_cursor() -> tuple[int, int]:
    """Where the console's cursor is, relative to the visible window.

    Not whether it shows: `GetConsoleCursorInfo` does not see a hide sent as
    `ESC[?25l` — measured, it reports the cursor visible under the "Window too
    small" notice, which has none. A hidden cursor is told by its place
    instead: ratatui moves a cursor it shows into the box that placed it, and
    leaves one it hides at the last cell its diff wrote."""
    handle = _open("CONOUT$")
    try:
        info = ScreenBufferInfo()
        _check(
            KERNEL32.GetConsoleScreenBufferInfo(handle, ctypes.byref(info)),
            "GetConsoleScreenBufferInfo",
        )
        at = info.dwCursorPosition
        return (at.X - info.srWindow.Left, at.Y - info.srWindow.Top)
    finally:
        KERNEL32.CloseHandle(handle)


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

    def __init__(
        self,
        exe: Path,
        args: list[str],
        cwd: Path | None = None,
        no_color: str | None = None,
        size: tuple[int, int] | None = None,
    ):
        """`size` — columns and rows the console is started at. The console
        cannot be resized once the app runs in it (see the module's notes), so
        `mode con` shrinks it first and the app is its second command; the
        exit code is then the app's, handed on by `cmd`."""
        command = [str(exe), *args]
        if size is not None:
            cols, rows = size
            # An argument list, like the plain launch: `cmd /c` runs the rest of
            # its line, and `&&` is one of the words on it.
            resize = ["mode", "con:", f"cols={int(cols)}", f"lines={int(rows)}"]
            command = ["cmd.exe", "/d", "/c", *resize, "&&", *command]
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0  # SW_HIDE
        # `NO_COLOR` is the scenario's to decide, not the shell's the probe
        # happens to be run from.
        env = {k: v for k, v in os.environ.items() if k.upper() != "NO_COLOR"}
        if no_color is not None:
            env["NO_COLOR"] = no_color
        self.process = subprocess.Popen(
            command,
            cwd=cwd,
            env=env,
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

    def close(self, esc_first: bool = True) -> int | None:
        """Quits the app and returns its exit code. `Esc` goes first to close
        whatever a scenario left open — unless the scenario is about `Ctrl+Q`
        ending the session on its own."""
        try:
            if esc_first:
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

    def covered(
        self, title: str, rows: list[list[tuple[str, int]]], shows: tuple[str, ...] = ()
    ) -> None:
        """No cell is left on the console's own background. What `painted` is
        for a theme whose backdrop does not read as its canvas: a sepia
        canvas comes back as white and its selection backdrop as yellow, and
        both are the theme's."""
        self.screen(title, rows)
        bare = [
            (x, y)
            for y, row in enumerate(rows)
            for x, (_, attributes) in enumerate(row)
            if background(attributes) == BLACK
        ]
        self.check(not bare, f"{title}: no cell is the console's own ({len(bare)} are: {bare[:6]})")
        self.says(title, rows, shows)

    def says(self, title: str, rows: list[list[tuple[str, int]]], shows: tuple[str, ...]) -> None:
        drawn = "\n".join("".join(char for char, _ in row) for row in rows)
        for text in shows:
            self.check(text in drawn, f"{title}: {text!r} is on the screen")

    def bare(
        self,
        title: str,
        rows: list[list[tuple[str, int]]],
        reversed_text: str = "",
        shows: tuple[str, ...] = (),
    ) -> None:
        """No cell is styled: each carries the attributes of the one cell the
        app never draws anything but a space into — the console's default.
        `reversed_text` is what must be in reverse video, and nothing else;
        `shows` is what must be on the screen in text — the glyphs that say
        what the styling said."""
        plain = Counter(a & ~NOT_STYLING & ~REVERSE_VIDEO for row in rows for _, a in row)
        default = plain.most_common(1)[0][0]
        print(
            f"- {title}: {len(rows[0])}x{len(rows)}, attributes "
            f"{ {hex(a): n for a, n in sorted(plain.items())} }"
        )
        styled = [
            (x, y, hex(attributes))
            for y, row in enumerate(rows)
            for x, (_, attributes) in enumerate(row)
            if attributes & ~NOT_STYLING & ~REVERSE_VIDEO != default
        ]
        self.check(not styled, f"{title}: no cell is styled ({len(styled)} are: {styled[:6]})")
        reverse = "".join(
            char for row in rows for char, attributes in row if attributes & REVERSE_VIDEO
        )
        self.check(
            reverse == reversed_text,
            f"{title}: in reverse video — {reverse!r} (expected {reversed_text!r})",
        )
        self.says(title, rows, shows)

    def coloured(self, title: str, rows: list[list[tuple[str, int]]]) -> None:
        """The control of `bare`: the same kind of frame, styled."""
        colours = Counter(a & COLOURS for row in rows for _, a in row)
        print(f"- {title}: colours { {hex(a): n for a, n in sorted(colours.items())} }")
        self.check(len(colours) > 2, f"{title}: the frame is coloured ({len(colours)} colour pairs)")

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
    press(SETTINGS_KEY, SETTLE_REPAINT)
    for _ in range(TABS_TO_INTERFACE):
        press("tab")
    press("enter")


def open_emoji_picker() -> None:
    press(EMOJI_KEY, SETTLE_REPAINT)


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
            "schema_version": SETTINGS_SCHEMA,
            "interface": {"language": "en", "theme_mode": "full", "full_theme": "light"},
        }
        (root / "data" / SETTINGS_FILE).write_text(json.dumps(settings), encoding="utf-8")

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


def scenario_mono(exe: Path, report: Report) -> None:
    session = Session(exe, ["demo"])
    try:
        report.coloured("as launched (system mode)", read_screen())

        to_interface_fields()  # the first field is "Colour mode"
        press("left", SETTLE_REPAINT)  # system -> monochrome, the long way round
        report.bare("settings, monochrome", read_screen(), shows=("monochrome", "[Esc]"))

        press("esc")
        press("esc", SETTLE_REPAINT)
        report.bare("chat", read_screen(), shows=("║ ",))

        type_text("select me")
        for _ in range(2):
            press("shift+left")
        time.sleep(SETTLE)
        report.bare("chat, two characters selected", read_screen(), reversed_text="me")
        press("right")
        report.bare("chat, the selection dropped", read_screen())

        press("f1", SETTLE_REPAINT)
        report.bare("help dialog over the chat", read_screen(), shows=("[",))
        press("esc", SETTLE_REPAINT)

        open_emoji_picker()
        rows = read_screen()
        report.bare("emoji picker over the chat", rows, shows=("[",))
        report.wide_glyphs("emoji picker", rows, at_least=40)
        press("esc", SETTLE_REPAINT)

        to_interface_fields()
        press("right", SETTLE_REPAINT)  # monochrome -> system
        report.coloured("back in the system mode", read_screen())
        press("esc")
    finally:
        code = session.close()
    report.check(code == 0, f"the app exited with code {code}")


def scenario_no_color(exe: Path, report: Report) -> None:
    for no_color, title, bare in (
        ("1", "NO_COLOR=1", True),
        ("", "NO_COLOR= (empty)", False),
        (None, "NO_COLOR unset", False),
    ):
        session = Session(exe, ["demo"], no_color=no_color)
        try:
            rows = read_screen()
            if bare:
                report.bare(f"{title}, first frame", rows, shows=("[F1]",))
                # The mode is the environment's, not a stored choice: the row
                # says it, and choosing `system` there wins over the variable.
                to_interface_fields()
                report.bare(f"{title}, settings", read_screen(), shows=("monochrome",))
                press("right", SETTLE_REPAINT)  # monochrome -> system
                report.coloured(f"{title}, system chosen in settings", read_screen())
                press("esc")
            else:
                report.coloured(f"{title}, first frame", rows)
        finally:
            code = session.close()
        report.check(code == 0, f"{title}: the app exited with code {code}")


def scenario_user_theme(exe: Path, report: Report) -> None:
    theme = "probe"
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        themes = root / "data" / "themes"
        themes.mkdir(parents=True)
        # A theme of its canvas alone: everything else is fitted to it.
        theme_file = themes / f"{theme}.json"
        theme_file.write_text(json.dumps({"canvas": "#f4ecd8"}), encoding="utf-8")
        (themes / "broken.json").write_text("{", encoding="utf-8")
        settings = {
            "schema_version": SETTINGS_SCHEMA,
            "interface": {"language": "en", "theme_mode": "full", "full_theme": theme},
        }
        (root / "data" / SETTINGS_FILE).write_text(json.dumps(settings), encoding="utf-8")

        print("first run: the theme is in data/themes")
        session = Session(copy, [], cwd=root)
        try:
            report.covered("started in a theme of the user's", read_screen())
            to_interface_fields()
            report.covered("settings, the theme by its name", read_screen(), shows=(theme,))
            press("esc")
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")
        log = "\n".join(
            path.read_text(encoding="utf-8", errors="replace")
            for path in (root / "data" / "logs").glob("*")
            if path.is_file()
        )
        said = [line for line in log.splitlines() if "broken.json" in line]
        report.check(
            len(said) == 1 and "skipping" in said[0],
            f"the log says what the other file was ({len(said)} line(s))",
        )
        report.check(theme_file.name not in log, "and nothing about a theme with nothing to say")

        print("second run (the control): the theme is gone, its name is still in the settings")
        theme_file.unlink()
        session = Session(copy, [], cwd=root)
        try:
            rows = read_screen()
            report.screen("started with a name nothing answers to", rows)
            report.check(
                all(background(a) == BLACK for row in rows for _, a in row),
                "the canvas is the dark one",
            )
            to_interface_fields()
            report.says("settings, the lost theme", read_screen(), shows=(theme,))
            press("esc")
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code} the second time")


# What the status line says while a message is read aloud, the picker's first
# row, the row of the gateway's key, the unit a listed price is in, the status
# line's chip for a ready chat engine and the input box's title once no turn
# runs — each read off the screen more than once.
SPEAKING = "speaking"
BY_HAND = "Type a name by hand"
KEY_ROW = "OpenRouter API key"
PRICED = "per 1M tokens"
CHAT_READY = "● chat"
TURN_OVER = "Enter send"
# The chat scenarios' first question; its one-word answer is read off the screen.
CAPITAL_QUESTION = "Answer with one word: what is the capital of France?"
# A turn through the agentic loop: the model saves a note, which the embedder
# indexes when there is one.
SAVE_A_NOTE = "Save a note with the note_save tool: my GPU has 24 GB. Then say done."


def text_of(rows: list[list[tuple[str, int]]]) -> str:
    return "\n".join("".join(char for char, _ in row).rstrip() for row in rows)


def wait_for_text(text: str, seconds: float) -> list[list[tuple[str, int]]]:
    """The screen once `text` is on it, or as it is when the time is up — the
    caller's check then says what was there instead."""
    deadline = time.monotonic() + seconds
    rows = read_screen()
    while text not in text_of(rows) and time.monotonic() < deadline:
        time.sleep(0.5)
        rows = read_screen()
    return rows


def wait_until_gone(text: str, seconds: float) -> list[list[tuple[str, int]]]:
    """Reads the screen until `text` has left it, or the time is up."""
    deadline = time.monotonic() + seconds
    rows = read_screen()
    while text in text_of(rows) and time.monotonic() < deadline:
        time.sleep(0.5)
        rows = read_screen()
    return rows


def scenario_gateway(exe: Path, report: Report) -> None:
    if not os.environ.get("OPENROUTER_API_KEY", "").strip():
        raise RuntimeError("OPENROUTER_API_KEY is not set, and this scenario is the real gateway")
    model = os.environ.get("MINDFORK_OPENROUTER_MODEL", "anthropic/claude-haiku-4.5")
    embedder = os.environ.get("MINDFORK_OPENROUTER_EMBED_MODEL", "baai/bge-m3")
    speaker = os.environ.get("MINDFORK_OPENROUTER_TTS_MODEL", "x-ai/grok-voice-tts-1.0")
    voice = os.environ.get("MINDFORK_OPENROUTER_TTS_VOICE", "eve")
    watcher = os.environ.get("MINDFORK_OPENROUTER_VIDEO_MODEL", "google/gemini-3.5-flash-lite")
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        (root / "data").mkdir()
        settings = {
            "schema_version": SETTINGS_SCHEMA,
            "interface": {"language": "en"},
            "engine": {
                "mode": "openrouter",
                # The key is named, not written: the settings hold no secret.
                "openrouter": {"model_name": model, "api_key_env": "OPENROUTER_API_KEY"},
            },
            "embed": {
                "mode": "openrouter",
                "openrouter": {"model_name": embedder, "api_key_env": "OPENROUTER_API_KEY"},
            },
            "tts": {
                "mode": "openrouter",
                "openrouter": {
                    "model_name": speaker,
                    "voice": voice,
                    "api_key_env": "OPENROUTER_API_KEY",
                },
            },
            "video": {
                "provider": "openrouter",
                "openrouter": {"model_name": watcher, "api_key_env": "OPENROUTER_API_KEY"},
            },
        }
        (root / "data" / SETTINGS_FILE).write_text(json.dumps(settings), encoding="utf-8")
        (root / "notes.txt").write_text(
            "The capital of France is Paris.\n\nThe moon's gravity is a sixth of the earth's.\n",
            encoding="utf-8",
        )

        session = Session(copy, [], cwd=root)
        try:
            print("the chat: a question through the gateway")
            type_text(CAPITAL_QUESTION)
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text("Paris", 60)
            print(text_of(rows))
            report.says("the reply", rows, ("Paris",))
            # Both keys were judged by now: the chips are the ready ones.
            report.says("the status line", rows, (CHAT_READY, "● emb"))

            print("the embedder: a file indexed through the gateway")
            type_text("/rag add notes.txt")
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text("indexing finished", 60)
            print(text_of(rows))
            report.says("the knowledge base", rows, ("indexing finished", "files: 1"))

            print("speech: the reply read aloud through the gateway (audible)")
            type_text("/tts")
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text(SPEAKING, 30)
            report.says("the status line while it speaks", rows, (SPEAKING,))
            rows = wait_until_gone(SPEAKING, 60)
            print(text_of(rows))
            said = text_of(rows)
            report.check(SPEAKING not in said, "the speech ended")
            report.check(
                "speech synthesis failed" not in said and "audio is unavailable" not in said,
                "and nothing failed on the way",
            )

            print("the settings: the provider's rows")
            press(SETTINGS_KEY, SETTLE_REPAINT)  # opens on "Model/server"
            press("enter", SETTLE_REPAINT)  # into its fields: the row of tabs
            rows = read_screen()
            print(text_of(rows))
            report.says(
                "Model/server",
                rows,
                ("openrouter", model, KEY_ROW, "Name the app to OpenRouter"),
            )

            print("the picker: the gateway's catalogue behind the model row")
            press("down")  # the tabs -> "Mode"
            press("down")  # -> "Model"
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text(PRICED, 30)
            print(text_of(rows))
            report.says("the catalogue", rows, ("context", PRICED, BY_HAND))
            press("esc")

            print("the settings: the speech slot's rows")
            press("up")  # "Model" -> "Mode"
            press("up")  # -> the tabs
            press("left", SETTLE_REPAINT)  # the first tab wraps onto the last: "Speech"
            rows = read_screen()
            print(text_of(rows))
            report.says(
                "Speech",
                rows,
                ("openrouter", speaker, voice, KEY_ROW, "Name the app to OpenRouter"),
            )
            report.check("Instructions" not in text_of(rows), "no row for instructions")

            print("the picker: the gateway's speech models behind the model row")
            press("down")  # the tabs -> "Mode"
            press("down")  # -> "Model"
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text("voices:", 30)
            print(text_of(rows))
            report.says("the speech models", rows, ("voices:", BY_HAND))
            report.check("per 1M" not in text_of(rows), "a speech model's row names no price")
            press("esc")

            print("the picker: the model's voices behind the voice row")
            press("down")  # "Model" -> "Voice"
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text("the model's voices", 30)
            print(text_of(rows))
            report.says("the voices", rows, (voice, "the model's voices", BY_HAND))
            press("esc")

            print("the settings: the video group, found by the search")
            type_text("/")
            type_text("Max video length")
            press("enter", SETTLE_REPAINT)  # lands on the row, in "Tools"
            rows = read_screen()
            print(text_of(rows))
            report.says(
                "Video (YouTube)",
                rows,
                ("Provider", "openrouter", watcher, KEY_ROW, "Max video length"),
            )
            report.check(
                "Input resolution" not in text_of(rows),
                "no row for the resolution: the gateway carries none",
            )

            print("the picker: the models that take video behind the model row")
            press("up")  # "Max video length" -> "Model"
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text(PRICED, 30)
            print(text_of(rows))
            report.says("the video models", rows, ("google/gemini", "context", BY_HAND))
            listed = [line for line in text_of(rows).splitlines() if " context " in line]
            report.check(
                bool(listed) and all("google/gemini" in line for line in listed[:5]),
                "the Gemini family opens the list",
            )
            report.check(
                "no tools" not in text_of(rows), "a model that watches is not marked for tools"
            )
            press("esc")
            press("esc")
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")
        log = "\n".join(
            path.read_text(encoding="utf-8", errors="replace")
            for path in (root / "data" / "logs").glob("*")
            if path.is_file()
        )
        key = os.environ["OPENROUTER_API_KEY"].strip()
        report.check(key not in log, "the key is not in the log")
        saved = (root / "data" / SETTINGS_FILE).read_text(encoding="utf-8")
        report.check(key not in saved, "nor in the settings")


TOO_SMALL = "Window too small"
QUIT_HINT = "Ctrl+Q"


def too_small_for_the_chat(exe: Path, report: Report) -> None:
    """57x2: below the chat's three rows, the notice and one key."""
    session = Session(exe, ["demo"], size=(57, 2))
    try:
        rows = read_screen()
        shown = text_of(rows)
        report.screen("57x2, as launched", rows)
        report.says("57x2", rows, (TOO_SMALL, "57×2", "20×3"))
        report.check(
            not any(glyph in shown for glyph in "╭│❯●"),
            "57x2: nothing of the chat is on the screen",
        )
        report.check("Esc" not in shown, "57x2: the way back is not named")
        press("enter")
        type_text("h")
        press("esc")
        report.check(text_of(read_screen()) == shown, "57x2: Enter, a letter and Esc change nothing")
    finally:
        code = session.close(esc_first=False)
    report.check(code == 0, f"57x2: Ctrl+Q alone ended the session (exit code {code})")


def row_starting(lines: list[str], prefix: str, after: int = -1) -> int | None:
    """The first row past `after` that starts with `prefix`."""
    return next((y for y, line in enumerate(lines) if y > after and line.startswith(prefix)), None)


def the_reported_window(exe: Path, report: Report) -> None:
    """57x5, the window of the report: four rows of the conversation over the
    `❯` row, where the prompt used to stand on the status bar."""
    session = Session(exe, ["demo"], size=(57, 5))
    try:
        rows = read_screen()
        shown = text_of(rows)
        lines = shown.split("\n")
        report.screen("57x5, as launched", rows)
        report.check(TOO_SMALL not in shown, "57x5: the chat, not the notice")
        report.check(lines[4].startswith("❯"), "57x5: the prompt is on the last row")
        report.check(
            not any(line.startswith(("╭", "│", "●")) for line in lines[:4]),
            "57x5: four bare rows of the conversation above it",
        )
        report.check(lines[3].strip() != "", "57x5: the row over the prompt is text, not padding")
        type_text("hey")
        typed = text_of(read_screen()).split("\n")[4]
        report.check(
            typed.startswith("❯") and "hey" in typed,
            "57x5: a typed word lands on the prompt's row",
        )
    finally:
        code = session.close()
    report.check(code == 0, f"57x5: the app exited with code {code}")


def the_ladder(exe: Path, report: Report) -> None:
    """From ten rows down, one piece of chrome a step (spec §11.1.1): the
    status bar's second row, the input's border, the feed's border, the
    status row — each window started at its size."""
    for height, feed_border, prompt, status in (
        (10, True, "│❯", True),
        (9, True, "❯", True),
        (8, True, "❯", True),
        (7, False, "❯", True),
        (6, False, "❯", True),
    ):
        at = f"57x{height}"
        session = Session(exe, ["demo"], size=(57, height))
        try:
            rows = read_screen()
            lines = text_of(rows).split("\n")
            report.screen(f"{at}, as launched", rows)
            report.check(TOO_SMALL not in text_of(rows), f"{at}: the chat, not the notice")
            report.check(
                lines[0].startswith("╭") == feed_border,
                f"{at}: the feed {'has' if feed_border else 'has no'} border",
            )
            input_row = row_starting(lines, prompt)
            report.check(input_row is not None, f"{at}: the input's row starts with {prompt!r}")
            bar = row_starting(lines, "●")
            report.check(
                (bar is not None) == status and (bar is None or bar == height - 1),
                f"{at}: the status bar {'is the last row' if status else 'is gone'}",
            )
            report.check(lines[-1].strip() != "", f"{at}: the frame is used to its last row")
        finally:
            code = session.close()
        report.check(code == 0, f"{at}: the app exited with code {code}")


def a_layer_that_does_not_fit(exe: Path, report: Report) -> None:
    """45x12, a window the chat fits: what is opened over it may not. No resize
    here — the notice comes with the key that opens and goes with `Esc`."""
    session = Session(exe, ["demo"], size=(45, 12))
    try:
        chat = text_of(read_screen())
        report.check(TOO_SMALL not in chat and "│❯" in chat, "45x12: the chat fits")
        for title, key, need in (
            ("the settings", SETTINGS_KEY, "46×12"),
            ("the emoji picker", EMOJI_KEY, "46×6"),
        ):
            press(key, SETTLE_REPAINT)
            rows = read_screen()
            report.says(f"45x12, {title}", rows, (TOO_SMALL, "45×12", need, "Esc"))
            press("enter")
            report.check(
                text_of(read_screen()) == text_of(rows),
                f"45x12, {title}: Enter does nothing under the notice",
            )
            press("esc", SETTLE_REPAINT)
            report.check(
                text_of(read_screen()) == chat,
                f"45x12, {title}: Esc brings the chat back, cell for cell",
            )
    finally:
        code = session.close()
    report.check(code == 0, f"45x12: the app exited with code {code}")


def under_the_panel(lines: list[str]) -> list[str]:
    """The rows below a full-screen panel's bottom border: its footer."""
    bottom = max((y for y, line in enumerate(lines) if line.startswith("╰")), default=len(lines))
    return lines[bottom + 1 :]


def the_footers(exe: Path, report: Report) -> None:
    """A screen's footer takes a third of the window at most, and `F1`
    stays in it (spec §11.1.1): the chat list at 57x14, where the legend used
    to wrap to seven rows in `en` and thirteen in `ru`, and the settings at
    their minimum, 46x12, where the footer is the one row the minimum holds."""
    session = Session(exe, ["demo"], size=(57, 14))
    try:
        press("esc", SETTLE_REPAINT)
        rows = read_screen()
        lines = text_of(rows).split("\n")
        report.screen("57x14, the chat list", rows)
        footer = under_the_panel(lines)
        report.check(0 < len(footer) <= 4, f"57x14, the chat list: {len(footer)} rows of footer, 4 allowed")
        report.check(any("F1" in row for row in footer), "57x14, the chat list: F1 is in the footer")
        report.check(any("Esc" in row for row in footer), "57x14, the chat list: Esc is in the footer")
        chats = sum(" ● " in line for line in lines)
        report.check(chats >= 5, f"57x14, the chat list: {chats} chats on screen")
    finally:
        code = session.close()
    report.check(code == 0, f"57x14: the app exited with code {code}")

    session = Session(exe, ["demo"], size=(46, 12))
    try:
        press(SETTINGS_KEY, SETTLE_REPAINT)
        rows = read_screen()
        shown = text_of(rows)
        report.screen("46x12, the settings", rows)
        report.check(TOO_SMALL not in shown, "46x12: the settings, not the notice")
        footer = under_the_panel(shown.split("\n"))
        report.check(len(footer) == 1, f"46x12, the settings: {len(footer)} rows of footer, 1 allowed")
        report.check(bool(footer) and "F1" in footer[0], "46x12, the settings: F1 is in the footer")
        report.check("Interface" in shown, "46x12, the settings: the menu to its last section")
    finally:
        code = session.close()
    report.check(code == 0, f"46x12: the app exited with code {code}")


def the_titles(exe: Path, report: Report) -> None:
    """41x12: a border title keeps whole parts, and the help's tab strip
    keeps the active tab whole (spec §11.1.1) — both used to be cut at the
    edge with no mark."""
    session = Session(exe, ["demo"], size=(41, 12))
    try:
        lines = text_of(read_screen()).split("\n")
        title = next((line for line in lines if line.startswith("╭ input")), "")
        report.check(
            title.startswith("╭ input · Enter send ─"),
            f"41x12: the input box's title keeps whole parts ({title.strip()!r})",
        )
        press("f1", SETTLE_REPAINT)
        tabs = []
        for _ in range(6):
            rows = read_screen()
            strip = next((line for line in text_of(rows).split("\n")[1:3] if "│" in line[2:]), "")
            tabs.append(strip)
            press("tab", SETTLE_REPAINT)
        report.screen("41x12, the help", rows)
        # Six presses of `Tab` walk all six tabs, whichever the help opened on,
        # so each one has to be whole on the strip somewhere in the walk.
        for label in ("About", "Shortcuts", "Commands", "License", "Legal", "Components"):
            report.check(any(label in strip for strip in tabs), f"41x12, the help: {label} is whole on the strip")
        report.check(all("…" in strip for strip in tabs), "41x12, the help: the hidden tabs are marked")
        press("esc", SETTLE_REPAINT)
    finally:
        code = session.close()
    report.check(code == 0, f"41x12: the app exited with code {code}")


def no_cursor_under_the_help(exe: Path, report: Report) -> None:
    """80x24: the box in front has the cursor; the help dialog over it
    takes it away — over the chat's input and over the settings' search field
    — and `Esc` gives it back where it was. Read by place ([`read_cursor`]):
    under the dialog the cursor is no longer in the box, and on the dialog's
    own last row, where the diff stopped writing."""
    session = Session(exe, ["demo"], size=(80, 24))
    try:
        for where, opening in (("the chat", ()), ("the settings' search", (SETTINGS_KEY, "/"))):
            for key in opening:
                if key in KEYS:
                    press(key, SETTLE_REPAINT)
                else:
                    type_text(key)
            time.sleep(SETTLE)
            rows = text_of(read_screen()).split("\n")
            before = read_cursor()
            report.check("❯" in rows[before[1]], f"80x24, {where}: the cursor is in the box {before}")
            press("f1", SETTLE_REPAINT)
            under = read_cursor()
            report.check(under != before, f"80x24, {where}: the help takes the cursor out of the box {under}")
            press("esc", SETTLE_REPAINT)
            after = read_cursor()
            report.check(after == before, f"80x24, {where}: the cursor is back {after}")
    finally:
        code = session.close()
    report.check(code == 0, f"80x24: the app exited with code {code}")


def input_row(rows: list[list[tuple[str, int]]]) -> str:
    """The input box's row: the one the prompt mark `❯` starts."""
    for line in text_of(rows).split("\n"):
        if "❯" in line:
            return line.split("❯", 1)[1].rstrip(" │")
    return ""


def scenario_altgr(exe: Path, report: Report) -> None:
    session = Session(exe, ["demo"])
    try:
        press(CLEAR_KEY)
        report.check(input_row(read_screen()).strip() == "", "Ctrl+K cleared the draft")
        type_text("price ")
        press(RUBLE_KEY)
        press("ctrl+alt+a")
        row = input_row(read_screen())
        print(f"    the input box: {row.strip()!r}")
        report.check("price ₽" in row, "AltGr+8 typed the ruble sign")
        report.check(
            # `a`, or the Russian layout's letter on that key.
            not any(c in row.replace("price", "") for c in "a\u0444"),
            "control arm: Ctrl+Alt+A typed nothing",
        )
        press(CLEAR_KEY)
        report.check(input_row(read_screen()).strip() == "", "Ctrl+K still clears the box")
        # The chat list's search line takes typed text of its own.
        press("esc", SETTLE_REPAINT)
        press(RUBLE_KEY)
        search = next((line for line in text_of(read_screen()).split("\n") if "\u2315" in line), "")
        print(f"    the chat list's search: {search.strip()!r}")
        report.check("\u20bd" in search, "AltGr+8 typed into the chat list's search")
    finally:
        code = session.close()
    report.check(code == 0, f"the app exited with code {code}")


def scenario_small_window(exe: Path, report: Report) -> None:
    too_small_for_the_chat(exe, report)
    the_reported_window(exe, report)
    the_ladder(exe, report)
    a_layer_that_does_not_fit(exe, report)
    the_footers(exe, report)
    the_titles(exe, report)
    no_cursor_under_the_help(exe, report)


# The Ollama recipe as the README prints it (install.md §3): the window is
# Ollama's own setting, which the app reads from `/api/ps`; a window typed into
# the app (`MINDFORK_OLLAMA_CONTEXT=16384`) is 0.15.0's recipe.
OLLAMA_URL = "http://localhost:11434/v1"
OLLAMA_MODEL = "gemma4:e4b"
OLLAMA_WINDOW = ""
# The slow-prefill note's launch line — llama-server's flags, which Ollama does
# not take.
LAUNCH_LINE = "-b 256 -ub 256"
# What the window-too-small note names, and what the app logs when the engine
# says what its window is (docs/research/ollama-window.md).
WINDOW_NOTE = "OLLAMA_CONTEXT_LENGTH"
WINDOW_LOGGED = "engine reported its context window"
# A page the window cannot hold beside the instructions, pasted at once, its
# question at the end where Ollama's cut keeps it; and what the app says and
# logs of the cut that follows (docs/research/prompt-cut-detection.md).
_CUT_WORDS = ("river", "lantern", "copper", "meadow", "harbor", "violet",
              "engine", "marble", "orchard", "glacier", "compass", "willow")
CUT_PAGE = "Here are my notes. " + " ".join(
    f"Note {i}: the {_CUT_WORDS[i % 12]} by the {_CUT_WORDS[(i * 5 + 3) % 12]}"
    f" keeps its {_CUT_WORDS[(i * 7 + 1) % 12]} until spring."
    for i in range(110)
) + " Now answer with one word: what is the capital of France?"
CUT_NOTE = "/regen"
CUT_LOGGED = "the server processed less of the prompt than it held"


def ollama_window(url: str, model: str) -> int | None:
    """The window Ollama loaded `model` with, from its `/api/ps` — what the app
    is expected to have read."""
    root = url.rstrip("/").removesuffix("/v1")
    with urllib.request.urlopen(f"{root}/api/ps", timeout=10) as resp:
        loaded = json.load(resp).get("models", [])
    return next((m.get("context_length") for m in loaded if m.get("name") == model), None)


def chat_is_up(report: Report) -> None:
    """The chat's status chip is on the screen: the app is up and its engine
    applied."""
    report.says("the start", wait_for_text(CHAT_READY, 60), (CHAT_READY,))


def set_language(copy: Path, root: Path) -> None:
    """The interface's language: English, so the screen reads the same on any
    machine."""
    language = [str(copy), "setup", "--set", "interface.language=en"]
    subprocess.run(language, cwd=root, check=True, capture_output=True)


def set_up_external(
    copy: Path, root: Path, url: str, model: str, report: Report, extra: list[str] | None = None
) -> None:
    """The interface's language first, so the screen reads the same on any
    machine; then the recipe's own line for an external server, word for word
    (with `extra` keys after it)."""
    set_language(copy, root)
    recipe = [
        str(copy), "setup",
        "--set", "engine.mode=external",
        "--set", f"engine.external.url={url}",
        "--set", f"engine.external.model_name={model}",
    ] + (extra or [])
    done = subprocess.run(recipe, cwd=root, capture_output=True, text=True)
    print(done.stdout)
    report.check(done.returncode == 0, f"the recipe's setup line exited with {done.returncode}")


def log_lines(root: Path, needle: str) -> list[str]:
    """The lines of the app's logs, in the scratch copy's data, that hold
    `needle`."""
    return [
        line
        for log in sorted((root / "data" / "logs").glob("*"))
        for line in log.read_text(encoding="utf-8", errors="replace").splitlines()
        if needle in line
    ]


def scenario_ollama(exe: Path, report: Report) -> None:
    url = os.environ.get("MINDFORK_OLLAMA_URL", OLLAMA_URL)
    model = os.environ.get("MINDFORK_OLLAMA_MODEL", OLLAMA_MODEL)
    window = os.environ.get("MINDFORK_OLLAMA_CONTEXT", OLLAMA_WINDOW)
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        # The recipe types no window: the app reads Ollama's. A typed one is
        # 0.15.0's recipe, kept as an arm.
        extra = ["--set", f"compaction.context_tokens={window}"] if window else []
        set_up_external(copy, root, url, model, report, extra)

        session = Session(copy, [], cwd=root)
        try:
            print("the chat: a question through Ollama (the first loads the model)")
            chat_is_up(report)
            type_text(CAPITAL_QUESTION)
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text("Paris", 180)
            # The turn's notes land at its end, after the reply's last token.
            wait_for_text(TURN_OVER, 30)
            time.sleep(2)
            rows = read_screen()
            print(text_of(rows))
            report.says("the reply", rows, ("Paris", model))
            report.check(
                LAUNCH_LINE not in text_of(rows),
                "no llama-server launch line for a server that is not one",
            )

            print("a tool: a note saved through the agentic loop")
            type_text(SAVE_A_NOTE)
            press("enter", SETTLE_REPAINT)
            wait_for_text("note_save(", 180)
            wait_for_text(TURN_OVER, 120)
            time.sleep(2)
            rows = read_screen()
            print(text_of(rows))
            report.says("the tool's card", rows, ("note_save(",))
            report.check(LAUNCH_LINE not in text_of(rows), "nor after the second turn")
            served = ollama_window(url, model)
            print(f"Ollama runs {model} with a window of {served}")
            if served is not None and served <= 4096:
                report.says("the window-too-small note", rows, (WINDOW_NOTE,))
            else:
                report.check(WINDOW_NOTE not in text_of(rows), "no window-too-small note")

            print("the settings: the model row lists what Ollama serves")
            press(SETTINGS_KEY, SETTLE_REPAINT)  # opens on "Model/server"
            press("enter", SETTLE_REPAINT)  # into its fields: the row of tabs
            press("down")  # -> "Mode"
            press("down")  # -> "URL (external)"
            press("down")  # -> "Model (opt.)"
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text(BY_HAND, 30)
            print(text_of(rows))
            report.says("the server's models", rows, (BY_HAND, model))
            press("esc")
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")
        cuts = log_lines(root, CUT_LOGGED)
        report.check(not cuts, f"no round told as cut: {cuts}")
        if not window:
            logged = log_lines(root, WINDOW_LOGGED)
            for line in logged:
                print(line)
            report.check(
                any(f"context_budget={served}" in line for line in logged),
                f"the app read Ollama's window, {served}, with none typed",
            )


def scenario_ollama_cut(exe: Path, report: Report) -> None:
    """A prompt Ollama cut in silence, told (docs/research/prompt-cut-detection.md).

    Needs Ollama started with a small window — `OLLAMA_CONTEXT_LENGTH=4096`,
    under which mindfork's first turn of about 3500 tokens fits and a pasted
    page after it does not: the system message and the paste alone are over the
    window, so Ollama cuts the prompt to half of it from the front. The app
    knows the first turn's exact size, and the second cannot hold less."""
    url = os.environ.get("MINDFORK_OLLAMA_URL", OLLAMA_URL)
    model = os.environ.get("MINDFORK_OLLAMA_MODEL", OLLAMA_MODEL)
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        set_up_external(copy, root, url, model, report)

        session = Session(copy, [], cwd=root)
        try:
            print("the first turn: a short question, which fits")
            chat_is_up(report)
            type_text(CAPITAL_QUESTION)
            press("enter", SETTLE_REPAINT)
            wait_for_text("Paris", 180)
            wait_for_text(TURN_OVER, 30)
            served = ollama_window(url, model)
            print(f"Ollama runs {model} with a window of {served}")
            report.check(
                served is not None and served <= 4096,
                f"Ollama's window is {served}: start it with OLLAMA_CONTEXT_LENGTH=4096",
            )
            report.check(CUT_NOTE not in text_of(read_screen()), "no cut after a turn that fit")

            print("the second turn: a pasted page the window cannot hold beside the instructions")
            paste_text(CUT_PAGE)
            # The burst is drained as one paste: an Enter in the same drain would
            # be a line break in it, so it waits for the page to be in the box.
            wait_for_text("capital of France?", 30)
            time.sleep(2)
            press("enter", SETTLE_REPAINT)
            wait_for_text(TURN_OVER, 300)
            time.sleep(2)
            rows = read_screen()
            print(text_of(rows))
            report.says("the cut-prompt note", rows, (CUT_NOTE,))
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")
        logged = log_lines(root, CUT_LOGGED)
        for line in logged:
            print(line)
        report.check(bool(logged), "the cut round told in the log")


# LM Studio's recipe (docs/research/local-servers.md): its own server, the
# model's key; the window is LM Studio's load setting, which the app reads from
# `/api/v1/models`. Its notes name LM Studio's setting, never Ollama's.
LMSTUDIO_URL = "http://localhost:1234/v1"
LMSTUDIO_MODEL = "google_gemma-4-e4b-it"
LMSTUDIO_NOTE = ("Context", "Length", "unload")


def lmstudio_window(url: str, model: str) -> int | None:
    """The window of the LM Studio instance a request naming `model` runs in,
    from `/api/v1/models` — what the app is expected to have read."""
    root = url.rstrip("/").removesuffix("/v1")
    with urllib.request.urlopen(f"{root}/api/v1/models", timeout=10) as resp:
        models = json.load(resp).get("models", [])
    for entry in models:
        for instance in entry.get("loaded_instances", []):
            if instance.get("id") == model:
                return instance.get("config", {}).get("context_length")
    return None


def scenario_lmstudio(exe: Path, report: Report) -> None:
    """LM Studio's recipe against its running server, the model loaded at a
    small window first — `lms load <model> -c 4096` — which the app must read
    and say is too small in LM Studio's own words."""
    url = os.environ.get("MINDFORK_LMSTUDIO_URL", LMSTUDIO_URL)
    model = os.environ.get("MINDFORK_LMSTUDIO_MODEL", LMSTUDIO_MODEL)
    served = lmstudio_window(url, model)
    print(f"LM Studio runs {model} with a window of {served}")
    report.check(served is not None, f"LM Studio has {model} loaded: lms load {model} -c 4096")
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        set_up_external(copy, root, url, model, report)
        session = Session(copy, [], cwd=root)
        try:
            print("the chat: a question through LM Studio")
            chat_is_up(report)
            type_text(CAPITAL_QUESTION)
            press("enter", SETTLE_REPAINT)
            wait_for_text("Paris", 180)
            wait_for_text(TURN_OVER, 30)
            time.sleep(2)
            rows = read_screen()
            print(text_of(rows))
            report.says("LM Studio's reply", rows, ("Paris", model))
            report.check(LAUNCH_LINE not in text_of(rows), "no llama-server launch line")
            if served is not None and served <= 4096:
                report.says("the window-too-small note, LM Studio's", rows, LMSTUDIO_NOTE)
                report.check("OLLAMA" not in text_of(rows), "no Ollama setting named")
            else:
                report.check(LMSTUDIO_NOTE[0] not in text_of(rows), "no window-too-small note")

            print("a tool: a note saved through the agentic loop")
            type_text(SAVE_A_NOTE)
            press("enter", SETTLE_REPAINT)
            wait_for_text("note_save(", 180)
            wait_for_text(TURN_OVER, 120)
            time.sleep(2)
            rows = read_screen()
            print(text_of(rows))
            report.says("the tool's card", rows, ("note_save(",))
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")
        cuts = log_lines(root, CUT_LOGGED)
        report.check(not cuts, f"no round told as cut: {cuts}")
        logged = log_lines(root, WINDOW_LOGGED)
        for line in logged:
            print(line)
        report.check(
            any(f"context_budget={served}" in line and "LmStudio" in line for line in logged),
            f"the app read LM Studio's window, {served}, and knew the server",
        )


def notes_page(count: int, question: str) -> str:
    """A page of `count` numbered notes, its question at the end — about
    sixteen tokens a note."""
    return "Here are my notes. " + " ".join(
        f"Note {i}: the {_CUT_WORDS[i % 12]} by the {_CUT_WORDS[(i * 5 + 3) % 12]}"
        f" keeps its {_CUT_WORDS[(i * 7 + 1) % 12]} until spring."
        for i in range(count)
    ) + f" Now answer with one word: {question}"


def scenario_lmstudio_cut(exe: Path, report: Report) -> None:
    """LM Studio's silent cut of the middle of a conversation, told
    (docs/research/local-servers.md §2). Needs the model loaded at 8192 —
    `lms unload <model>`, then `lms load <model> -c 8192`: at 4096 mindfork's
    instructions are so much of the window that LM Studio cannot cut a
    conversation down to fit and refuses it instead. The first turn and a page
    fit; a second page puts the request over the window while the instructions,
    the first message and the last still fit, so LM Studio drops the first page
    and answers."""
    url = os.environ.get("MINDFORK_LMSTUDIO_URL", LMSTUDIO_URL)
    model = os.environ.get("MINDFORK_LMSTUDIO_MODEL", LMSTUDIO_MODEL)
    served = lmstudio_window(url, model)
    print(f"LM Studio runs {model} with a window of {served}")
    report.check(
        served == 8192,
        f"LM Studio's window is {served}: lms unload {model}, then lms load {model} -c 8192",
    )
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        set_up_external(copy, root, url, model, report)
        session = Session(copy, [], cwd=root)
        rows = []
        try:
            print("the first turn: a short question")
            chat_is_up(report)
            type_text(CAPITAL_QUESTION)
            press("enter", SETTLE_REPAINT)
            wait_for_text("Paris", 180)
            wait_for_text(TURN_OVER, 30)
            pages = [(150, "what is the capital of Italy?"), (175, "what is the capital of Spain?")]
            for page, (count, question) in enumerate(pages):
                print(f"page {page + 1}: {count} notes")
                paste_text(notes_page(count, question))
                wait_for_text(question, 30)
                time.sleep(2)
                press("enter", SETTLE_REPAINT)
                wait_for_text(TURN_OVER, 300)
                time.sleep(2)
                rows = read_screen()
                if page == 0:
                    report.check(CUT_NOTE not in text_of(rows), "no cut while the pages fit")
            print(text_of(rows))
            report.says("the cut-prompt note, LM Studio's", rows, (CUT_NOTE,) + LMSTUDIO_NOTE)
            report.check("OLLAMA" not in text_of(rows), "no Ollama setting named")
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")
        logged = log_lines(root, CUT_LOGGED)
        for line in logged:
            print(line)
        report.check(bool(logged), "the cut round told in the log")


# The engineless chat offering what it finds (docs/research/local-servers.md,
# stage 2): the list's title, and what a pick says.
LOCAL_TITLE = "Local servers"
OLLAMA_ROW = "Ollama · "
LMSTUDIO_ROW = "LM Studio · "
NOTHING_ANSWERED = "Neither Ollama"


def settings_of(root: Path) -> dict:
    """The scratch copy's `settings.json`, as the app wrote it."""
    with open(root / "data" / "settings.json", encoding="utf-8") as f:
        return json.load(f)


def scenario_first_run(exe: Path, report: Report) -> None:
    """A fresh copy with no engine, Ollama and LM Studio both running: the list
    opens by itself and names both; a pick of Ollama answers; `/local` lists
    again and a pick of LM Studio brings its embedder too, which a saved note
    then uses. Needs Ollama with a chat model pulled and no embedder, and LM
    Studio's server with a chat model and the embedder it ships."""
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        set_language(copy, root)
        session = Session(copy, [], cwd=root)
        try:
            print("the start: no engine, and the list opens by itself")
            rows = wait_for_text(LOCAL_TITLE, 60)
            print(text_of(rows))
            report.says("the list", rows, (LOCAL_TITLE, OLLAMA_ROW, LMSTUDIO_ROW, "embeddings:"))

            print("Enter on the first row: Ollama")
            press("enter", SETTLE_REPAINT)
            wait_for_text("now runs on Ollama", 30)
            chat_is_up(report)
            type_text(CAPITAL_QUESTION)
            press("enter", SETTLE_REPAINT)
            wait_for_text("Paris", 180)
            wait_for_text(TURN_OVER, 30)
            time.sleep(2)
            report.says("Ollama's reply", read_screen(), ("Paris",))
            written = settings_of(root)
            report.check(
                written["engine"]["mode"] == "external"
                and written["engine"]["external"]["url"].endswith(":11434/v1"),
                f"the pick is in settings.json: {written['engine']['external']}",
            )
            report.check(written["embed"]["mode"] != "external", "no embedder from a server with none")

            print("/local: the list again, and LM Studio with its embedder")
            type_text("/local")
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text(LOCAL_TITLE, 30)
            print(text_of(rows))
            lines = text_of(rows).splitlines()
            first = next(i for i, line in enumerate(lines) if OLLAMA_ROW in line or LMSTUDIO_ROW in line)
            target = next(i for i, line in enumerate(lines) if LMSTUDIO_ROW in line)
            for _ in range(target - first):
                press("down")
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text("now runs on LM Studio", 30)
            print(text_of(rows))
            report.says("the pick's note", rows, ("now runs on LM Studio", "nomic"))
            time.sleep(3)
            type_text("Answer with one word: what is the capital of Italy?")
            press("enter", SETTLE_REPAINT)
            wait_for_text("Rome", 180)
            wait_for_text(TURN_OVER, 30)
            print("a note saved, which the embedder indexes")
            type_text(SAVE_A_NOTE)
            press("enter", SETTLE_REPAINT)
            wait_for_text("note_save(", 180)
            wait_for_text(TURN_OVER, 120)
            time.sleep(3)
            rows = read_screen()
            print(text_of(rows))
            report.says("LM Studio's turns", rows, ("Rome", "note_save("))
            written = settings_of(root)
            report.check(
                written["engine"]["external"]["url"].endswith(":1234/v1")
                and written["embed"]["mode"] == "external"
                and "nomic" in (written["embed"]["external"].get("model_name") or ""),
                f"LM Studio and its embedder in settings.json: {written['engine']['external']}, {written['embed']['external']}",
            )
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")
        errors = [line for line in log_lines(root, " ERROR ") if "embed" in line.lower()]
        report.check(not errors, f"no embedder errors in the log: {errors}")
        for line in log_lines(root, "looked for local servers") + log_lines(root, "a local server was picked"):
            print(line)


def scenario_first_run_none(exe: Path, report: Report) -> None:
    """A fresh copy with no engine and **neither server running**: the start
    stays silent — the empty feed names the four routes, `/local` among them —
    and `/local` says what it asked and that nothing answered."""
    with tempfile.TemporaryDirectory(prefix="mindfork-probe-") as scratch:
        root = Path(scratch)
        copy = root / exe.name
        shutil.copy(exe, copy)
        set_language(copy, root)
        session = Session(copy, [], cwd=root)
        try:
            rows = wait_for_text("Four ways to start", 60)
            time.sleep(5)
            rows = read_screen()
            print(text_of(rows))
            report.says("the empty feed", rows, ("Four ways to start", "/local"))
            report.check(LOCAL_TITLE not in text_of(rows), "no list when nothing answered")
            type_text("/local")
            press("enter", SETTLE_REPAINT)
            rows = wait_for_text(NOTHING_ANSWERED, 30)
            print(text_of(rows))
            report.says("the command's answer", rows, (NOTHING_ANSWERED, "11434", "1234"))
        finally:
            code = session.close()
        report.check(code == 0, f"the app exited with code {code}")


SCENARIOS = {
    "altgr": scenario_altgr,
    "ollama": scenario_ollama,
    "ollama-cut": scenario_ollama_cut,
    "lmstudio": scenario_lmstudio,
    "lmstudio-cut": scenario_lmstudio_cut,
    "first-run": scenario_first_run,
    "first-run-none": scenario_first_run_none,
    "small-window": scenario_small_window,
    "full-mode": scenario_full_mode,
    "gateway": scenario_gateway,
    "first-frame": scenario_first_frame,
    "mono": scenario_mono,
    "no-color": scenario_no_color,
    "user-theme": scenario_user_theme,
}


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
