#!/usr/bin/env python3
"""The dark card of the code-ownership essay: 2000x1000, the site's palette.

The essay (``site/content/articles/what-replaces-code-ownership.md``) opens
with this card, and a social site shows it as the page's preview. Left: the
hook in big type and the repository's numbers. Right: the one picture the
essay has, the CLAUDE.md cliff, drawn as a bold filled shape so it survives a
feed thumbnail. Drawn at 2x and downsampled for antialiasing, in JetBrains
Mono, found the way ``tools/screenshots.py`` finds it.

Nothing on the card is typed in. The cliff's points come from
``tools/claude_md_chart.py`` — the same points the essay's own chart plots —
and its labels are read off them; the numbers come from
``tools/repo_metrics.py``; the count of open Sonar issues is read from
SonarCloud, or taken from ``--sonar-issues`` offline. The card ends on the day
of the newest commit, as the chart does, so the two cannot disagree.

    python tools/hero.py                     # redraw site/static/images/what-replaces-code-ownership.png
    python tools/hero.py --out card.png --sonar-issues 0 --line4 "..."

Needs Pillow (``tools/media-requirements.txt``).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.request
from datetime import date, timedelta
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

import claude_md_chart
import repo_metrics
from screenshots import find_variant

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "site" / "static" / "images" / "what-replaces-code-ownership.png"
ICON = ROOT / "assets" / "mindfork-icon-256.png"
SONAR_PROPERTIES = ROOT / "sonar-project.properties"
SONAR_ISSUES = "https://sonarcloud.io/api/issues/search?componentKeys={key}&resolved=false&ps=1"

BG = (13, 14, 17)
TEXT = (232, 230, 227)
MUTED = (150, 148, 144)
ACCENT = (194, 90, 39)
GREEN = (108, 182, 108)
GRID = (44, 46, 52)

W, H = 2000, 1000
S = 2  # supersampling
# The plot region on the right; the hook's last baseline sits on its floor.
PLOT = (1180 * S, 1900 * S, 300 * S, 760 * S)


def font(px: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    hit = find_variant(None, "JetBrainsMono-Bold.ttf" if bold else "JetBrainsMono-Regular.ttf")
    if hit is None:
        raise SystemExit("JetBrains Mono not found (see tools/screenshots.py for where it is looked for)")
    return ImageFont.truetype(str(hit), px * S)


def dashed(d: ImageDraw.ImageDraw, x0: int, y: int, x1: int, fill, width: int, dash: int, gap: int) -> None:
    x = x0
    while x < x1:
        d.line((x, y, min(x + dash, x1), y), fill=fill, width=width)
        x += dash + gap


def day_label(day: date) -> str:
    return f"{day:%b} {day.day}"


def sonar_open_issues() -> int:
    """How many issues SonarCloud holds open on the project: the badge's number."""
    match = re.search(r"^sonar\.projectKey=(.+)$", SONAR_PROPERTIES.read_text(encoding="utf-8"), re.M)
    if match is None:
        raise SystemExit(f"{SONAR_PROPERTIES.name}: no sonar.projectKey")
    with urllib.request.urlopen(SONAR_ISSUES.format(key=match.group(1).strip()), timeout=30) as response:
        return int(json.load(response)["total"])


def numbers_row(data: dict, sonar_issues: int) -> str:
    return (
        f"{data['days']} days  ·  {data['commits']:,} commits  ·  {data['pull_requests_merged']:,} PRs  ·  "
        f"{data['test_fn_attributes']:,} tests  ·  {sonar_issues} Sonar issues"
    )


def draw(lines: tuple[str, str, str, str], points: list[claude_md_chart.Point], first: date, numbers: str, out: Path) -> None:
    img = Image.new("RGBA", (W * S, H * S), BG + (255,))
    d = ImageDraw.Draw(img)

    # Brand row.
    icon = Image.open(ICON).convert("RGBA").resize((72 * S, 72 * S), Image.LANCZOS)
    img.alpha_composite(icon, (100 * S, 84 * S))
    d.text((190 * S, 96 * S), "mindfork", font=font(40, True), fill=TEXT)
    d.text((410 * S, 104 * S), "mindfork.io", font=font(30), fill=MUTED)

    x0, x1, y0, y1 = PLOT

    # The hook, set from the bottom up so the green line shares the chart's floor.
    b4 = y1
    b3 = b4 - 78 * S
    b2 = b3 - 112 * S
    b1 = b2 - 126 * S
    d.text((100 * S, b1), lines[0], font=font(108, True), fill=TEXT, anchor="ls")
    d.text((100 * S, b2), lines[1], font=font(108, True), fill=TEXT, anchor="ls")
    d.text((100 * S, b3), lines[2], font=font(58, True), fill=ACCENT, anchor="ls")
    d.text((100 * S, b4), lines[3], font=font(58, True), fill=GREEN, anchor="ls")

    # The numbers.
    d.text((100 * S, 876 * S), numbers, font=font(30), fill=MUTED)

    # The cliff: the day on the x axis, kilobytes on the y axis, up to a megabyte.
    last = points[-1].day
    span, top_kb = max(1, (last - first).days), 1000

    def x(day: date) -> float:
        return x0 + (day - first).days / span * (x1 - x0)

    def y(size: int) -> float:
        return y1 - size / 1000 / top_kb * (y1 - y0)

    pts = [(x(p.day), y(p.size)) for p in points]
    d.text((x0, 186 * S), "CLAUDE.md, the file every session loads, in KB", font=font(26), fill=MUTED)

    # Area under the line in a translucent accent, then the line itself.
    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    ImageDraw.Draw(layer).polygon([(x0, y1), *pts, (x1, y1)], fill=ACCENT + (56,))
    img.alpha_composite(layer)
    d = ImageDraw.Draw(img)
    d.line((x0, y1, x1, y1), fill=GRID, width=2 * S)
    d.line(pts, fill=ACCENT, width=7 * S, joint="curve")
    r = 7 * S
    for px, py in pts:
        d.ellipse((px - r, py - r, px + r, py + r), fill=ACCENT)

    # The cap, the peak, the day after it, and the ends of the axis.
    yc = y(claude_md_chart.CAP_KB * 1000)
    dashed(d, x0, int(yc), x1, MUTED, 3 * S, 14 * S, 12 * S)
    d.text((x1, yc - 12 * S), f"{claude_md_chart.CAP_KB} KB cap", font=font(24), fill=MUTED, anchor="rd")
    peak = max(points, key=lambda p: p.size)
    px, py = x(peak.day), y(peak.size)
    d.text((px - 16 * S, py + 6 * S), f"{claude_md_chart.kb(peak.size):,} KB on {day_label(peak.day)}", font=font(40, True), fill=TEXT, anchor="rd")
    after = next((p for p in points if p.day == peak.day + timedelta(days=1)), None)
    if after is not None:
        fx, fy = x(after.day), y(after.size)
        d.text((fx + 22 * S, fy - 104 * S), f"{claude_md_chart.kb(after.size)} KB the next day", font=font(30, True), fill=TEXT, anchor="la")
    d.text((x0, y1 + 16 * S), day_label(first), font=font(24), fill=MUTED, anchor="la")
    d.text((x1, y1 + 16 * S), day_label(last), font=font(24), fill=MUTED, anchor="ra")

    img = img.resize((W, H), Image.LANCZOS).convert("RGB")
    img.save(out, optimize=True)
    print(f"{out} — {out.stat().st_size / 1024:.0f} KiB, {W}x{H}, {day_label(first)} to {day_label(last)}: {numbers}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    ap.add_argument("--out", type=Path, default=OUT)
    ap.add_argument("--sonar-issues", type=int, default=None, help="open Sonar issues (default: asked of SonarCloud)")
    ap.add_argument("--line1", default="Nobody owned")
    ap.add_argument("--line2", default="this code.")
    ap.add_argument("--line3", default="It didn't fall apart.")
    ap.add_argument("--line4", default="And the metrics stayed green.")
    a = ap.parse_args()
    until = claude_md_chart.last_commit_day(ROOT)
    points = claude_md_chart.claude_md_points(ROOT, until)
    if not points:
        raise SystemExit(f"{claude_md_chart.FILE}: no commit to draw")
    sonar_issues = a.sonar_issues if a.sonar_issues is not None else sonar_open_issues()
    numbers = numbers_row(repo_metrics.collect(ROOT), sonar_issues)
    draw((a.line1, a.line2, a.line3, a.line4), points, claude_md_chart.first_commit_day(ROOT), numbers, a.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
