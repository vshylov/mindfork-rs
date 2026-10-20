#!/usr/bin/env python3
"""The CLAUDE.md size chart in the code-ownership essay, drawn from git.

The essay at ``site/content/articles/what-replaces-code-ownership.md`` has one
figure: the size of CLAUDE.md, the file every session loads, over the project's
life — it grew to a megabyte and became a 30 KB router. The figure is inline
SVG, so it follows the site's theme toggle, and its points are read from the
repository's history. The dark card the page shows as its cover (drawn by
``tools/hero.py``) plots the same points. This script is the one definition of
those points and the one renderer of the figure, so the chart, the table under
it and the card cannot disagree about a byte.

    python tools/claude_md_chart.py                      # the points, as a Markdown table
    python tools/claude_md_chart.py --until 2026-10-19   # ...as of that day
    python tools/claude_md_chart.py --write              # redraw the figure in the essay
    python tools/claude_md_chart.py --check              # the essay's figure matches git
    python tools/claude_md_chart.py --self-test          # the rules, against a fixture repo

The points, in words: the size of CLAUDE.md at the last commit that changed it
on or before the end of each week — Sundays, counted from the day of the
repository's first commit, each commit on its own local date — plus the commit
where the file peaked and the last commit of the day after, plus the last day
itself when it is not a Sunday. The last day is the date of the newest commit,
not the day the script runs: a chart drawn on a quiet day ends where the
history ends. ``--check`` reads that day off the essay's own table and compares
the figure with the one git draws for it, so the figure stays checkable after
the history has moved on.

The self-test plants dated commits in a fixture repository — a mid-week edit, a
Sunday that touches another file, two commits of the peak size, two edits on
the day after, a commit after midnight in the committer's own zone — and checks
which commit each point reads and what the figure says about them.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ARTICLE = ROOT / "site" / "content" / "articles" / "what-replaces-code-ownership.md"
FILE = "CLAUDE.md"
COMMIT_URL = "https://github.com/vshylov/mindfork-rs/commit/"
CAP_KB = 30
UTF8 = "utf-8"

# The drawing, in the SVG's own units (viewBox 720x360): the plot area, the
# scale and the spacing of the ticks.
X0, X1, Y0, Y1 = 72, 696, 40, 324
GRID_KB = 250
TICK_DAYS = 21
TICK_CLEARANCE_DAYS = 14

FIGURE_START = '<figure class="chart" aria-labelledby="claude-md-size-title">'
FIGURE_END = "</details>\n"
TABLE_ROW = re.compile(r'<tr><td>(\d{4}-\d{2}-\d{2})</td><td class="num">')


@dataclass(frozen=True)
class Point:
    day: date
    size: int
    sha: str
    key: bool = False  # the peak and the day after are drawn larger


@dataclass(frozen=True)
class Commit:
    sha: str
    stamp: int
    day: date  # the committer's own local date


def git(root: Path, *args: str, env: dict | None = None) -> str:
    done = subprocess.run(
        ["git", *args], cwd=root, capture_output=True, text=True, encoding=UTF8, errors="replace", check=True, env=env
    )
    return done.stdout


def commits_touching(root: Path, path: str = FILE) -> list[Commit]:
    """Every commit on HEAD's history that changed ``path``, newest first."""
    out = []
    for line in git(root, "log", "--format=%H %ct %ci", "HEAD", "--", path).splitlines():
        sha, stamp, day = line.split()[:3]
        out.append(Commit(sha, int(stamp), date.fromisoformat(day)))
    return out


def sizes_at(root: Path, commits: list[Commit], path: str = FILE) -> dict[str, int]:
    """The size of ``path`` at each commit, one ``cat-file`` for all of them."""
    query = "".join(f"{c.sha}:{path}\n" for c in commits)
    done = subprocess.run(
        ["git", "cat-file", "--batch-check"], cwd=root, input=query, capture_output=True, text=True, encoding=UTF8, check=True
    )
    sizes = {}
    for commit, line in zip(commits, done.stdout.splitlines()):
        parts = line.split()
        if len(parts) == 3 and parts[1] == "blob":
            sizes[commit.sha] = int(parts[2])
    return sizes


def first_commit_day(root: Path) -> date:
    line = git(root, "log", "--reverse", "--format=%ci", "HEAD").splitlines()[0]
    return date.fromisoformat(line.split()[0])


def last_commit_day(root: Path) -> date:
    return date.fromisoformat(git(root, "log", "-1", "--format=%ci", "HEAD").split()[0])


def last_change_on_or_before(commits: list[Commit], day: date) -> Commit | None:
    before = [c for c in commits if c.day <= day]
    return max(before, key=lambda c: c.stamp) if before else None


def claude_md_points(root: Path, until: date) -> list[Point]:
    commits = [c for c in commits_touching(root) if c.day <= until]
    sizes = sizes_at(root, commits)
    commits = [c for c in commits if c.sha in sizes]
    if not commits:
        return []
    points: dict[date, Point] = {}

    def sample(day: date, key: bool = False) -> None:
        commit = last_change_on_or_before(commits, day)
        if commit is not None and (key or day not in points):
            points[day] = Point(day, sizes[commit.sha], commit.sha, key)

    day = first_commit_day(root)
    while day <= until:
        sample(day)
        day += timedelta(days=7)
    sample(until)
    # The peak: the first commit that reached the largest size, then the day after it.
    peak = min((c for c in commits if sizes[c.sha] == max(sizes.values())), key=lambda c: c.stamp)
    points[peak.day] = Point(peak.day, sizes[peak.sha], peak.sha, True)
    if peak.day + timedelta(days=1) <= until:
        sample(peak.day + timedelta(days=1), key=True)
    return [points[d] for d in sorted(points)]


# --- the figure -----------------------------------------------------------------


def num(value: float) -> str:
    """One decimal, the way the figure was first drawn; ``318.0`` prints as ``318``."""
    text = f"{value:.1f}"
    return text[:-2] if text.endswith(".0") else text


def kb(size: int) -> int:
    return round(size / 1000)


def short_day(day: date) -> str:
    return f"{day:%b} {day.day}"


def long_day(day: date) -> str:
    return f"{day:%B} {day.day}"


def render_figure(points: list[Point], first: date, until: date) -> str:
    span = max(1, (until - first).days)
    peak = max(points, key=lambda p: p.size)
    top_kb = max(1000, -(-kb(peak.size) // GRID_KB) * GRID_KB)

    def x(day: date) -> float:
        return X0 + (day - first).days / span * (X1 - X0)

    def y(size: int) -> float:
        return Y1 - size / 1000 / top_kb * (Y1 - Y0)

    after = next((p for p in points if p.key and p.day == peak.day + timedelta(days=1)), None)
    last = points[-1]
    desc = (
        f"CLAUDE.md grew from {kb(points[0].size)} KB on {long_day(points[0].day)} to {kb(peak.size):,} KB on "
        f"{long_day(peak.day)}, fell to {kb(after.size) if after else kb(last.size)} KB the next day, and has stayed under "
        f"the {CAP_KB} KB cap that CI enforces ever since; it was {last.size:,} bytes on {long_day(last.day)}."
    )
    out = [
        FIGURE_START,
        '<p class="chart-title" id="claude-md-size-title">The file every session loads grew to 1&nbsp;MB, '
        "then became a 30&nbsp;KB router</p>",
        f'<p class="chart-sub">CLAUDE.md size in KB, one point per week, {first:%B} to {until:%B} {until:%Y}</p>',
        '<svg viewBox="0 0 720 360" role="img" aria-labelledby="claude-md-size-desc">',
        f'<desc id="claude-md-size-desc">{desc}</desc>',
    ]
    grid = list(range(GRID_KB, top_kb + 1, GRID_KB))
    for level in grid:
        out.append(f'<line class="grid" x1="{X0}" x2="{X1}" y1="{num(y(level * 1000))}" y2="{num(y(level * 1000))}"/>')
    for level in [0, *grid]:
        out.append(f'<text class="tick" x="{X0 - 10}" y="{num(y(level * 1000) + 4.5)}" text-anchor="end">{level:,}</text>')
    out.append(f'<line class="base" x1="{X0}" x2="{X1}" y1="{Y1}" y2="{Y1}"/>')
    tick = 0
    while first + timedelta(days=tick) <= until - timedelta(days=TICK_CLEARANCE_DAYS):
        day = first + timedelta(days=tick)
        cls = "tick" if (tick // TICK_DAYS) % 2 == 0 else "tick minor"
        anchor = "start" if tick == 0 else "middle"
        out.append(f'<text class="{cls}" x="{num(x(day))}" y="{Y1 + 28}" text-anchor="{anchor}">{short_day(day)}</text>')
        tick += TICK_DAYS
    out.append(f'<text class="tick" x="{X1}" y="{Y1 + 28}" text-anchor="end">{short_day(until)}</text>')
    cap_y = y(CAP_KB * 1000)
    out.append(f'<line class="cap" x1="{X0}" x2="{X1}" y1="{num(cap_y)}" y2="{num(cap_y)}"/>')
    out.append(f'<text class="note" x="{X1}" y="{num(cap_y - 8)}" text-anchor="end">{CAP_KB} KB cap in CI</text>')
    path = " ".join(f"{'M' if i == 0 else 'L'}{num(x(p.day))} {num(y(p.size))}" for i, p in enumerate(points))
    out.append(f'<path class="series" d="{path}"/>')
    for p in points:
        cx, cy = num(x(p.day)), num(y(p.size))
        dot = f'<circle class="key" cx="{cx}" cy="{cy}" r="5.5"/>' if p.key else f'<circle class="dot" cx="{cx}" cy="{cy}" r="3"/>'
        out.append(
            f'<g class="pt">{dot}<circle class="hit" cx="{cx}" cy="{cy}" r="12">'
            f"<title>{short_day(p.day)}: {p.size:,} bytes</title></circle></g>"
        )
    out.append(
        f'<text class="label" x="{num(x(peak.day) - 12)}" y="{num(y(peak.size) + 5)}" text-anchor="end">'
        f"{kb(peak.size):,} KB on {short_day(peak.day)}</text>"
    )
    if after is not None:
        out.append(
            f'<text class="label lift" x="{num(x(after.day) + 12)}" y="{num(cap_y - 24)}">{kb(after.size)} KB the next day</text>'
        )
    out += [
        "</svg>",
        "<figcaption>Read from the repository's git history: the size of CLAUDE.md at the last commit of each week, "
        "plus the day it peaked and the day after. Hover a point, or open the table below, for the exact bytes.</figcaption>",
        "</figure>",
        '<details class="chart-data">',
        "<summary>The numbers behind the chart</summary>",
        '<table><thead><tr><th>Date</th><th class="num">Bytes</th><th>Commit</th></tr></thead><tbody>',
    ]
    for p in points:
        out.append(
            f'<tr><td>{p.day.isoformat()}</td><td class="num">{p.size:,}</td>'
            f'<td><a href="{COMMIT_URL}{p.sha}"><code>{p.sha[:8]}</code></a></td></tr>'
        )
    out += ["</tbody></table>", "</details>"]
    return "\n".join(out) + "\n"


def render_table(points: list[Point]) -> str:
    rows = ["| Date | Bytes | Commit |", "| --- | ---: | --- |"]
    rows += [f"| {p.day.isoformat()} | {p.size:,} | `{p.sha[:8]}`{' (key)' if p.key else ''} |" for p in points]
    return "\n".join(rows)


def figure_for(root: Path, until: date) -> str:
    points = claude_md_points(root, until)
    if not points:
        raise SystemExit(f"{FILE}: no commit on or before {until}")
    return render_figure(points, first_commit_day(root), until)


def split_article(text: str) -> tuple[str, str, str]:
    """The essay as (before, figure, after); a refusal when the figure is not where it was."""
    start = text.find(FIGURE_START)
    end = text.find(FIGURE_END, start)
    if start < 0 or end < 0:
        raise SystemExit(f"{ARTICLE.name}: the figure's markers are not there")
    end += len(FIGURE_END)
    return text[:start], text[start:end], text[end:]


def article_until(figure: str) -> date:
    """The last day the essay's table shows: the day the figure was drawn for."""
    days = TABLE_ROW.findall(figure)
    if not days:
        raise SystemExit(f"{ARTICLE.name}: the figure has no table rows")
    return date.fromisoformat(days[-1])


def write_article(root: Path, until: date) -> None:
    text = ARTICLE.read_bytes().decode(UTF8)
    before, figure, after = split_article(text)
    wanted = figure_for(root, until)
    if wanted == figure:
        print(f"unchanged: {ARTICLE.relative_to(ROOT)} (as of {until})")
        return
    # Written through a file object, bytes out as bytes came in (no newline
    # translation on Windows, docs/lessons.md on `write_text`).
    with open(ARTICLE, "wb") as out:
        out.write((before + wanted + after).encode(UTF8))
    print(f"written: {ARTICLE.relative_to(ROOT)} — the figure as of {until}")


def check_article(root: Path) -> int:
    _, figure, _ = split_article(ARTICLE.read_bytes().decode(UTF8))
    until = article_until(figure)
    wanted = figure_for(root, until)
    if wanted == figure:
        print(f"OK: the essay's figure matches git as of {until}")
        return 0
    have, want = figure.splitlines(), wanted.splitlines()
    first_diff = next((i for i, (a, b) in enumerate(zip(have, want)) if a != b), min(len(have), len(want)))
    print(f"stale: the essay's figure differs from git as of {until} at line {first_diff + 1} of the figure", file=sys.stderr)
    print(f"  have: {have[first_diff] if first_diff < len(have) else '<end>'}", file=sys.stderr)
    print(f"  want: {want[first_diff] if first_diff < len(want) else '<end>'}", file=sys.stderr)
    print("regenerate: python tools/claude_md_chart.py --write", file=sys.stderr)
    return 1


# --- self-test: a fixture repository with dated commits ----------------------------

FIXTURE_IDENTITY = ("-c", "user.name=fixture", "-c", "user.email=fixture@example.com", "-c", "commit.gpgsign=false")
FIXTURE_OTHER = "README.md"  # the file a commit touches when it leaves CLAUDE.md alone


def fixture_commit(root: Path, when: str, message: str, files: dict[str, int]) -> str:
    """Commit ``files`` (name -> size in bytes, filled with the message's first letter) at ``when``."""
    for name, size in files.items():
        (root / name).write_text(message[0] * size, encoding=UTF8)
    env = {**os.environ, "GIT_AUTHOR_DATE": when, "GIT_COMMITTER_DATE": when}
    git(root, *FIXTURE_IDENTITY, "add", "-A", env=env)
    git(root, *FIXTURE_IDENTITY, "commit", "-q", "-m", message, env=env)
    return git(root, "rev-parse", "HEAD").strip()


def build_fixture(root: Path) -> dict[str, str]:
    git(root, "init", "-q", "-b", "main")
    shas = {}
    shas["a"] = fixture_commit(root, "2026-06-14T12:00:00+03:00", "a: the first commit, a Sunday", {FILE: 10_000, FIXTURE_OTHER: 5})
    shas["b"] = fixture_commit(root, "2026-06-17T12:00:00+03:00", "b: a mid-week edit", {FILE: 100_000})
    shas["c"] = fixture_commit(root, "2026-06-21T09:00:00+03:00", "c: a Sunday that touches another file", {FIXTURE_OTHER: 6})
    shas["d"] = fixture_commit(root, "2026-06-22T00:30:00+03:00", "d: after midnight here, Sunday in UTC", {FILE: 70_000})
    shas["e"] = fixture_commit(root, "2026-06-23T10:00:00+03:00", "e: the peak", {FILE: 1_000_000})
    shas["f"] = fixture_commit(root, "2026-06-23T11:00:00+03:00", "f: the peak's size again", {FILE: 1_000_000})
    shas["g"] = fixture_commit(root, "2026-06-24T09:00:00+03:00", "g: the morning after", {FILE: 40_000})
    shas["h"] = fixture_commit(root, "2026-06-24T18:00:00+03:00", "h: the evening after", {FILE: 50_000})
    shas["i"] = fixture_commit(root, "2026-06-28T12:00:00+03:00", "i: a Sunday", {FILE: 60_000})
    shas["j"] = fixture_commit(root, "2026-07-03T12:00:00+03:00", "j: a Friday, another file", {FIXTURE_OTHER: 7})
    return shas


def self_test() -> int:
    failures: list[str] = []

    def check(name: str, got: object, want: object) -> None:
        if got != want:
            failures.append(f"{name}: expected {want!r}, got {got!r}")

    with tempfile.TemporaryDirectory(prefix="claude-md-chart-") as tmp:
        root = Path(tmp)
        shas = build_fixture(root)
        check("last commit day", last_commit_day(root), date(2026, 7, 3))
        check("first commit day", first_commit_day(root), date(2026, 6, 14))
        points = claude_md_points(root, date(2026, 7, 3))
        want = [
            ("2026-06-14", 10_000, "a", False),
            ("2026-06-21", 100_000, "b", False),  # the Sunday's own commit touched another file
            ("2026-06-23", 1_000_000, "e", True),  # the first commit of the peak size, not the second
            ("2026-06-24", 50_000, "h", True),  # the last edit of the day after
            ("2026-06-28", 60_000, "i", False),
            ("2026-07-03", 60_000, "i", False),  # the last day, read from the newest change before it
        ]
        got = [(p.day.isoformat(), p.size, next(k for k, s in shas.items() if s == p.sha), p.key) for p in points]
        check("points", got, want)
        check("points to a Sunday", [p.day.isoformat() for p in claude_md_points(root, date(2026, 6, 28))], [d for d, *_ in want[:5]])
        figure = render_figure(points, date(2026, 6, 14), date(2026, 7, 3))
        for needle in (
            "1,000 KB on Jun 23",
            "50 KB the next day",
            "it was 60,000 bytes on July 3",
            f'<a href="{COMMIT_URL}{shas["e"]}"><code>{shas["e"][:8]}</code></a>',
            '<circle class="key" cx="',
            'text-anchor="end">Jul 3</text>',
            "June to July 2026",
        ):
            check(f"figure says {needle!r}", needle in figure, True)
        check("figure ticks", figure.count('class="tick minor"'), 0)  # Jul 5 would be past the end
        check("figure rows", figure.count("<tr><td>"), 6)
        before, fig, after = split_article("x\n" + figure + "y\n")
        check("split", (before, after), ("x\n", "y\n"))
        check("until from the table", article_until(fig), date(2026, 7, 3))
    for failure in failures:
        print(f"self-test FAILED: {failure}")
    if failures:
        return 1
    print("self-test: the points and the figure match the fixture")
    return 0


def main(argv: list[str]) -> int:
    flags = [a for a in argv if a.startswith("--") and a != "--until"]
    until = None
    if "--until" in argv:
        at = argv.index("--until")
        if at + 1 >= len(argv):
            print("usage: --until YYYY-MM-DD", file=sys.stderr)
            return 2
        until = date.fromisoformat(argv[at + 1])
    if set(flags) - {"--write", "--check", "--self-test"} or len(flags) > 1:
        print(__doc__.split("\n\n", 1)[0])
        print("usage: claude_md_chart.py [--until YYYY-MM-DD] [--write | --check | --self-test]")
        return 2
    if "--self-test" in flags:
        return self_test()
    if "--check" in flags:
        return check_article(ROOT)
    until = until or last_commit_day(ROOT)
    if "--write" in flags:
        write_article(ROOT, until)
        return 0
    print(render_table(claude_md_points(ROOT, until)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
