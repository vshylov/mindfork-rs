#!/usr/bin/env python3
"""OSC 11 probe: does this terminal report its background colour, and how fast?

Background — `Theme::Auto` (`src/shared/config.rs`) is documented as "follow the
system setting", but `Palette::auto()` (`src/shared/theme.rs`) hard-codes
`dark: true`. Making `Auto` actually adapt would mean querying the terminal with
OSC 11 (`ESC ] 11 ; ? BEL`) and reading the reply. Whether that is worth doing
depends on a fact this project has not measured: which of the terminals our
users actually run will answer.

The precedent is OSC 52 (docs/history/osc52-clipboard.md): JupyterLab turned out
to drop it entirely, and that measurement — not the specification — is what
shaped the feature. So this probe measures rather than assumes.

It is a **spike tool**, not part of the app: it is run by hand in each terminal
and its output is pasted into the journal's host matrix.

Usage (run it inside the terminal you want to measure):

    python tools/osc11_probe.py
    python tools/osc11_probe.py --timeout 500 --repeat 3

It needs a real terminal on both stdin and stdout; under a pipe or in CI it
exits 2 rather than hanging. It is read-only with respect to the terminal: it
writes a query, reads the answer, and restores the console mode it found.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import time

# `ESC ] 11 ; ? BEL` — "report the background colour". BEL rather than ST as the
# terminator: both are accepted as the *query* terminator far more widely than
# ST alone, and the reply's terminator is read back either way (`read_osc`).
ESC = "\x1b"
BEL = "\x07"
QUERY_BG = f"{ESC}]11;?{BEL}"
QUERY_FG = f"{ESC}]10;?{BEL}"

# A multiplexer swallows the query unless it is told to pass it through to the
# terminal it is itself running in. tmux: `ESC P tmux; <ESC-doubled body> ESC \`.
# GNU screen: `ESC P <body> ESC \`. Both are why "does tmux need wrapping" is a
# separate row in the matrix and not an implementation detail.
def wrap_passthrough(seq: str, mode: str = "auto") -> tuple[str, str]:
    """Returns (sequence to write, name of the wrapping applied).

    `mode` forces the choice; `auto` picks by environment. Forcing matters
    because passthrough is **output-only**: it carries the query out to the
    outer terminal, but the reply comes back on the multiplexer's own input and
    is consumed there rather than delivered to the pane. A multiplexer that
    implements OSC 11 itself will answer the *unwrapped* query, so "wrapped
    gets no answer" is not the same finding as "the multiplexer is silent".
    """
    if mode == "none":
        return seq, "none (forced)"
    if mode == "tmux":
        return f"{ESC}Ptmux;{seq.replace(ESC, ESC + ESC)}{ESC}\\", "tmux (forced)"
    if mode == "screen":
        return f"{ESC}P{seq}{ESC}\\", "screen (forced)"
    if os.environ.get("TMUX"):
        return f"{ESC}Ptmux;{seq.replace(ESC, ESC + ESC)}{ESC}\\", "tmux"
    if os.environ.get("STY"):
        return f"{ESC}P{seq}{ESC}\\", "screen"
    return seq, "none"


# ---------------------------------------------------------------------------
# Terminal plumbing. Two implementations of the same three operations (put the
# console into a raw-ish mode, read single bytes with a deadline, restore) —
# POSIX termios and the Win32 console API.
# ---------------------------------------------------------------------------

IS_WINDOWS = os.name == "nt"

# Win32 console mode flags (see the Console API docs).
ENABLE_PROCESSED_INPUT = 0x0001
ENABLE_LINE_INPUT = 0x0002
ENABLE_ECHO_INPUT = 0x0004
ENABLE_VIRTUAL_TERMINAL_INPUT = 0x0200
ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004


class PosixTerminal:
    """Raw mode via termios; byte reads gated by `select`."""

    name = "posix"

    def __init__(self) -> None:
        import termios

        self._termios = termios
        self._fd = sys.stdin.fileno()
        self._saved = None
        self.vt_input = True  # always true on a POSIX tty

    def __enter__(self):
        import tty

        self._saved = self._termios.tcgetattr(self._fd)
        # cbreak, not full raw: signals stay live, so Ctrl+C still kills a probe
        # against a terminal that answers with something we did not expect.
        tty.setcbreak(self._fd, self._termios.TCSANOW)
        # Echo off, or a terminal that *does* answer paints its own reply.
        mode = self._termios.tcgetattr(self._fd)
        mode[3] &= ~self._termios.ECHO
        self._termios.tcsetattr(self._fd, self._termios.TCSANOW, mode)
        return self

    def __exit__(self, *exc):
        if self._saved is not None:
            self._termios.tcsetattr(self._fd, self._termios.TCSADRAIN, self._saved)
        return False

    def write(self, text: str) -> None:
        sys.stdout.write(text)
        sys.stdout.flush()

    def read_byte(self, deadline: float):
        """One byte, or None once `deadline` (a `time.monotonic` value) passes."""
        import select

        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        ready, _, _ = select.select([self._fd], [], [], remaining)
        if not ready:
            return None
        data = os.read(self._fd, 1)
        return data[0] if data else None


class WindowsTerminal:
    """Raw-ish mode via SetConsoleMode; byte reads polled through msvcrt.

    The reply only reaches stdin as raw VT bytes when the input handle has
    `ENABLE_VIRTUAL_TERMINAL_INPUT`. Legacy conhost refuses that flag — which is
    itself a result worth recording, so a failure to set it is reported rather
    than raised.
    """

    name = "windows"

    def __init__(self) -> None:
        import ctypes

        self._ctypes = ctypes
        self._k32 = ctypes.windll.kernel32
        self._h_in = self._k32.GetStdHandle(-10)
        self._h_out = self._k32.GetStdHandle(-11)
        self._saved_in = None
        self._saved_out = None
        self.vt_input = False

    def _get_mode(self, handle):
        mode = self._ctypes.c_uint32()
        if not self._k32.GetConsoleMode(handle, self._ctypes.byref(mode)):
            return None
        return mode.value

    def __enter__(self):
        self._saved_in = self._get_mode(self._h_in)
        self._saved_out = self._get_mode(self._h_out)

        if self._saved_out is not None:
            self._k32.SetConsoleMode(
                self._h_out, self._saved_out | ENABLE_VIRTUAL_TERMINAL_PROCESSING
            )
        if self._saved_in is not None:
            wanted = self._saved_in
            wanted &= ~(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT)
            wanted |= ENABLE_VIRTUAL_TERMINAL_INPUT
            self._k32.SetConsoleMode(self._h_in, wanted)
            # Read the mode back: conhost silently drops the VT-input bit, and
            # "the terminal never got the chance to answer" is a different
            # finding from "the terminal chose not to".
            actual = self._get_mode(self._h_in)
            self.vt_input = bool(actual is not None and actual & ENABLE_VIRTUAL_TERMINAL_INPUT)
        return self

    def __exit__(self, *exc):
        if self._saved_in is not None:
            self._k32.SetConsoleMode(self._h_in, self._saved_in)
        if self._saved_out is not None:
            self._k32.SetConsoleMode(self._h_out, self._saved_out)
        return False

    def write(self, text: str) -> None:
        sys.stdout.write(text)
        sys.stdout.flush()

    def read_byte(self, deadline: float):
        import msvcrt

        while True:
            if msvcrt.kbhit():
                return msvcrt.getch()[0]
            if time.monotonic() >= deadline:
                return None
            time.sleep(0.001)


def open_terminal():
    return WindowsTerminal() if IS_WINDOWS else PosixTerminal()


# ---------------------------------------------------------------------------
# The exchange itself.
# ---------------------------------------------------------------------------


class Reply:
    def __init__(self):
        self.raw = b""
        self.elapsed_ms = 0.0
        self.status = "no-answer"  # no-answer | ok | foreign-input | truncated
        self.note = ""


def read_osc(term, deadline: float) -> Reply:
    """Reads one OSC reply, consuming nothing that is not plainly ours.

    The safety property matters more than the parsing: a terminal that does not
    answer leaves whatever the user typed sitting in the input buffer, and a
    probe that swallowed it would be a worse bug than the one it is measuring.
    So the first byte must be ESC and the second `]`; anything else stops the
    read immediately and is reported as foreign input.
    """
    reply = Reply()
    started = time.monotonic()

    first = term.read_byte(deadline)
    if first is None:
        reply.elapsed_ms = (time.monotonic() - started) * 1000
        return reply

    reply.raw += bytes([first])
    if first != 0x1B:
        reply.status = "foreign-input"
        reply.note = "first byte was not ESC - the terminal did not answer and something else was waiting on stdin"
        reply.elapsed_ms = (time.monotonic() - started) * 1000
        return reply

    second = term.read_byte(deadline)
    if second is None:
        reply.status = "truncated"
        reply.note = "a bare ESC arrived and nothing followed"
        reply.elapsed_ms = (time.monotonic() - started) * 1000
        return reply

    reply.raw += bytes([second])
    if second != 0x5D:  # ']'
        reply.status = "foreign-input"
        reply.note = "ESC was not followed by ']' - not an OSC reply"
        reply.elapsed_ms = (time.monotonic() - started) * 1000
        return reply

    # Body, to BEL or to ST (`ESC \`).
    while True:
        b = term.read_byte(deadline)
        if b is None:
            reply.status = "truncated"
            reply.note = "the reply never terminated"
            break
        reply.raw += bytes([b])
        if b == 0x07:  # BEL
            reply.status = "ok"
            break
        if b == 0x1B:  # possible ST
            nxt = term.read_byte(deadline)
            if nxt is not None:
                reply.raw += bytes([nxt])
            reply.status = "ok"
            break
        if len(reply.raw) > 128:
            reply.status = "truncated"
            reply.note = "the reply ran past 128 bytes"
            break

    reply.elapsed_ms = (time.monotonic() - started) * 1000
    return reply


# `rgb:RRRR/GGGG/BBBB`, and the 1-, 2- and 3-digit widths X colour names allow.
RGB_RE = re.compile(rb"rgb:([0-9a-fA-F]{1,4})/([0-9a-fA-F]{1,4})/([0-9a-fA-F]{1,4})")
HASH_RE = re.compile(rb"#([0-9a-fA-F]{6})")


def parse_rgb(raw: bytes):
    """Extracts 8-bit RGB from an OSC 10/11 reply body, or None."""
    m = RGB_RE.search(raw)
    if m:
        out = []
        for part in m.groups():
            # Components are scaled to their own width: `ffff`, `ff` and `f` are
            # all "full". Shifting by width is what X11 does.
            value = int(part, 16)
            width = len(part) * 4
            out.append(round(value * 255 / ((1 << width) - 1)))
        return tuple(out)
    m = HASH_RE.search(raw)
    if m:
        v = int(m.group(1), 16)
        return ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
    return None


def relative_luminance(rgb) -> float:
    """WCAG relative luminance (sRGB linearised). 0 = black, 1 = white."""

    def channel(c: float) -> float:
        c /= 255.0
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = rgb
    return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)


def simple_luma(rgb) -> float:
    """Gamma-space luma (0.2126/0.7152/0.0722), the cheap test most tools use."""
    r, g, b = rgb
    return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0


def escape(raw: bytes) -> str:
    """Renders the reply printably, so it can go into a journal table as-is."""
    out = []
    for b in raw:
        if b == 0x1B:
            out.append("ESC")
        elif b == 0x07:
            out.append("BEL")
        elif 0x20 <= b < 0x7F:
            out.append(chr(b))
        else:
            out.append(f"\\x{b:02x}")
    return "".join(out)


# Environment keys worth carrying into the matrix: each identifies a host we
# would otherwise have to guess at from the reply alone.
FINGERPRINT_KEYS = (
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "COLORTERM",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TMUX",
    "STY",
    "SSH_CONNECTION",
    "SSH_TTY",
    "JPY_PARENT_PID",
    "JUPYTER_SERVER_ROOT",
    "VSCODE_INJECTION",
)


def fingerprint() -> list[str]:
    lines = []
    for key in FINGERPRINT_KEYS:
        value = os.environ.get(key)
        if value:
            # WT_SESSION and TMUX are just presence markers; their values are
            # noise (and a socket path is arguably private).
            if key in ("WT_SESSION", "TMUX", "SSH_CONNECTION", "SSH_TTY", "JPY_PARENT_PID"):
                value = "<set>"
            lines.append(f"  {key}={value}")
    return lines or ["  (none of the usual markers are set)"]


def probe_once(term, query: str, label: str, timeout_ms: int, mode: str = "auto") -> Reply:
    payload, wrapping = wrap_passthrough(query, mode)
    term.write(payload)
    deadline = time.monotonic() + timeout_ms / 1000.0
    reply = read_osc(term, deadline)
    reply.label = label
    reply.wrapping = wrapping
    return reply


def report(reply: Reply, timeout_ms: int) -> list[str]:
    """Renders one probe result as lines (returned, so `--out` can keep them)."""
    lines = [
        f"{reply.label}:",
        f"  passthrough wrapping : {reply.wrapping}",
        f"  status               : {reply.status}",
        f"  elapsed              : {reply.elapsed_ms:.1f} ms (timeout {timeout_ms} ms)",
    ]
    if reply.raw:
        lines.append(f"  raw reply            : {escape(reply.raw)}")
    if reply.note:
        lines.append(f"  note                 : {reply.note}")

    rgb = parse_rgb(reply.raw) if reply.status == "ok" else None
    if rgb:
        lum = relative_luminance(rgb)
        luma = simple_luma(rgb)
        verdict = "DARK" if lum < 0.5 else "LIGHT"
        lines.append(f"  parsed rgb           : #{rgb[0]:02x}{rgb[1]:02x}{rgb[2]:02x}  {rgb}")
        lines.append(f"  relative luminance   : {lum:.4f}   (WCAG, linearised)")
        lines.append(f"  gamma-space luma     : {luma:.4f}")
        lines.append(f"  verdict              : {verdict}  (threshold 0.5 on relative luminance)")
    elif reply.status == "ok":
        lines.append("  parsed rgb           : (could not parse - record the raw reply)")
    return lines


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Measure whether this terminal answers an OSC 11 background-colour query."
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=200,
        help="milliseconds to wait for the reply (default: 200)",
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=1,
        help="repeat the query N times, to see whether the timing is stable (default: 1)",
    )
    parser.add_argument(
        "--foreground",
        action="store_true",
        help="also query OSC 10 (foreground colour)",
    )
    parser.add_argument(
        "--wrapping",
        choices=("auto", "none", "tmux", "screen"),
        default="auto",
        help="multiplexer passthrough to apply (default: auto, by environment). "
        "Use 'none' inside tmux/screen to ask the multiplexer itself rather than "
        "the terminal behind it - passthrough carries the query out but not the "
        "reply back.",
    )
    parser.add_argument(
        "--delay",
        type=int,
        default=0,
        help="milliseconds to settle before the first query (default: 0). Raise it when "
        "the probe runs at shell start-up, before the emulator has attached.",
    )
    parser.add_argument(
        "--out",
        metavar="PATH",
        help="also write the report to PATH, so a matrix row can be copied rather "
        "than transcribed off the screen",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the parser over recorded replies and exit; needs no terminal",
    )
    args = parser.parse_args()

    if args.self_test:
        return self_test()

    if not sys.stdin.isatty() or not sys.stdout.isatty():
        print(
            "osc11_probe: needs a real terminal on both stdin and stdout.\n"
            "It is meant to be run by hand inside the terminal being measured, not\n"
            "through a pipe, a task runner or CI.",
            file=sys.stderr,
        )
        return 2

    out: list[str] = ["OSC 11 probe - " + ("Windows console" if IS_WINDOWS else "POSIX tty")]
    out.append("")
    out.append("Environment fingerprint:")
    out.extend(fingerprint())

    with open_terminal() as term:
        if IS_WINDOWS and not term.vt_input:
            out.append("")
            out.append("  WARNING: ENABLE_VIRTUAL_TERMINAL_INPUT could not be set on stdin.")
            out.append("  This is legacy conhost behaviour: the reply, if any, will not reach")
            out.append("  us as VT bytes. Record this row as 'no VT input', not 'no answer'.")

        if args.delay > 0:
            time.sleep(args.delay / 1000.0)

        replies = []
        for i in range(max(1, args.repeat)):
            label = "background (OSC 11)" + (f" - attempt {i + 1}" if args.repeat > 1 else "")
            replies.append(probe_once(term, QUERY_BG, label, args.timeout, args.wrapping))
        if args.foreground:
            replies.append(probe_once(term, QUERY_FG, "foreground (OSC 10)", args.timeout, args.wrapping))

    for reply in replies:
        out.append("")
        out.extend(report(reply, args.timeout))

    # One pasteable line per run, so a matrix row does not have to be
    # transcribed by hand out of the block above.
    bg = replies[0]
    rgb = parse_rgb(bg.raw) if bg.status == "ok" else None
    summary = (
        f"RESULT host={os.environ.get('TERM_PROGRAM') or os.environ.get('TERM') or 'unknown'}"
        f" wrapping={bg.wrapping}"
        f" status={bg.status}"
        f" ms={bg.elapsed_ms:.1f}"
    )
    if rgb:
        lum = relative_luminance(rgb)
        summary += f" bg=#{rgb[0]:02x}{rgb[1]:02x}{rgb[2]:02x} lum={lum:.4f} verdict={'dark' if lum < 0.5 else 'light'}"
    out.append("")
    out.append(summary)

    # Raw mode is already restored, but a terminal that was in it needs CR as
    # well as LF for the block to come out left-aligned.
    sys.stdout.write("\r\n".join(out) + "\r\n")
    sys.stdout.flush()

    if args.out:
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write("\n".join(out) + "\n")

    return 0


# ---------------------------------------------------------------------------
# Self-test: the parsing and luminance halves, over replies recorded from real
# terminals plus the shapes the specification allows. Runs without a tty, so it
# is the part CI could keep if this spike graduates.
# ---------------------------------------------------------------------------

def self_test() -> int:
    cases = [
        # (raw reply, expected 8-bit rgb, expected verdict)
        (b"\x1b]11;rgb:0000/0000/0000\x07", (0, 0, 0), "dark"),
        (b"\x1b]11;rgb:ffff/ffff/ffff\x07", (255, 255, 255), "light"),
        # xterm's usual 4-digit form for a typical dark terminal (#1e1e1e).
        (b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07", (30, 30, 30), "dark"),
        # 2-digit components, ST-terminated.
        (b"\x1b]11;rgb:fd/f6/e3\x1b\\", (253, 246, 227), "light"),
        # 1-digit components: `f` is full-scale, not 15/255.
        (b"\x1b]11;rgb:f/f/f\x07", (255, 255, 255), "light"),
        # The `#rrggbb` form some terminals answer with.
        (b"\x1b]11;#282c34\x07", (40, 44, 52), "dark"),
    ]
    failures = 0
    for raw, expected_rgb, expected_verdict in cases:
        rgb = parse_rgb(raw)
        if rgb != expected_rgb:
            print(f"FAIL parse {escape(raw)}: got {rgb}, expected {expected_rgb}")
            failures += 1
            continue
        verdict = "dark" if relative_luminance(rgb) < 0.5 else "light"
        if verdict != expected_verdict:
            print(f"FAIL verdict {escape(raw)}: got {verdict}, expected {expected_verdict}")
            failures += 1
            continue
        print(f"ok  {escape(raw)} -> #{rgb[0]:02x}{rgb[1]:02x}{rgb[2]:02x} {verdict}")

    # Replies that must not parse into a colour.
    for raw in (b"\x1b]11;\x07", b"\x1b]11;rgb:zz/zz/zz\x07", b""):
        if parse_rgb(raw) is not None:
            print(f"FAIL non-colour reply parsed: {escape(raw)}")
            failures += 1
        else:
            print(f"ok  {escape(raw) or '(empty)'} -> no colour")

    failures += read_osc_self_test()

    print(f"\n{'FAILED' if failures else 'PASSED'}: {failures} failure(s)")
    return 1 if failures else 0


class FakeTerminal:
    """Feeds `read_osc` a scripted byte stream; counts what it consumed."""

    def __init__(self, data: bytes):
        self._data = data
        self.consumed = 0

    def read_byte(self, deadline: float):
        if self.consumed >= len(self._data):
            return None  # stands in for "the deadline passed with nothing there"
        b = self._data[self.consumed]
        self.consumed += 1
        return b


def read_osc_self_test() -> int:
    """Exercises the reply state machine, including the safety property.

    The last two cases are the ones that matter: a terminal that stays silent
    must leave the user's type-ahead untouched, and a probe that consumed it
    would be a worse defect than the one being measured.
    """
    cases = [
        # (name, stream, expected status, expected bytes consumed)
        ("BEL-terminated reply", b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07", "ok", 24),
        ("ST-terminated reply", b"\x1b]11;rgb:fd/f6/e3\x1b\\", "ok", 19),
        ("silence", b"", "no-answer", 0),
        ("bare ESC", b"\x1b", "truncated", 1),
        ("unterminated body", b"\x1b]11;rgb:11", "truncated", 11),
        # Type-ahead: a keystroke waiting on stdin must cost exactly one byte.
        ("keystroke, not a reply", b"q", "foreign-input", 1),
        ("ESC then a key", b"\x1bA", "foreign-input", 2),
    ]
    failures = 0
    deadline = time.monotonic() + 60
    for name, stream, expected_status, expected_consumed in cases:
        term = FakeTerminal(stream)
        reply = read_osc(term, deadline)
        problems = []
        if reply.status != expected_status:
            problems.append(f"status {reply.status!r} != {expected_status!r}")
        if term.consumed != expected_consumed:
            problems.append(f"consumed {term.consumed} != {expected_consumed}")
        if problems:
            print(f"FAIL read_osc {name}: {'; '.join(problems)}")
            failures += 1
        else:
            print(f"ok  read_osc {name} -> {reply.status}, {term.consumed} byte(s) consumed")
    return failures


if __name__ == "__main__":
    sys.exit(main())
