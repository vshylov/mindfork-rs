#!/usr/bin/env python3
"""Every third-party action is pinned to a commit, with its version in a comment.

A `uses: owner/repo@v5` is a *mutable* reference: the tag is a pointer the
action's owner can move, so what runs in this repository's workflows is whatever
that pointer names at the moment a job starts. Two of ours make the consequence
concrete — `SonarSource/sonarqube-scan-action` receives `SONAR_TOKEN`, and
`aws-actions/configure-aws-credentials` assumes the role that deploys
mindfork.io with `id-token: write`. A compromised or repointed tag on either is
a credential leak that no review of this repository would show, because nothing
here changed. The same tag on `actions/upload-artifact` decides what a release's
assets are.

A 40-character commit SHA cannot be repointed, which is why GitHub's own
hardening guide says to use one. It is also unreadable, so the rule here is a
pin **and** a trailing comment naming the version it is — that is the line a
human reads, and the line Dependabot rewrites when it bumps the pin
(`.github/dependabot.yml`, `github-actions`, weekly).

What is checked, for every `uses:` in `.github/workflows/*.yml`:

1. a third-party action is `owner/repo[/path]@<40 hex>`;
2. it carries a trailing `# <version>` comment;
3. a local action (`./...`) is left alone — it is this repository's own code, and
   it is already pinned by the commit being built.

Exit code is non-zero on any violation, so this doubles as a CI lint.

Usage:
    python tools/actions_pin_check.py [--list]
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

WORKFLOWS = ".github/workflows"
# `- uses: owner/repo@ref` in any indentation, with or without the dash. The
# trailing `# comment` is split off by hand rather than matched here: an optional
# lazy group before an anchored `\s*$` is the shape that backtracks
# super-linearly (SonarQube `python:S8786`), and one `partition` is clearer.
USES_RE = re.compile(r"^[-\s]*uses:\s*(?P<ref>\S+)")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
LOCAL_PREFIXES = ("./", "docker://")


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def parse_uses(line: str) -> tuple[str, str] | None:
    """`(ref, comment)` for a `uses:` line, or `None` when it is not one."""
    match = USES_RE.match(line)
    if not match:
        return None
    ref = match.group("ref").strip("\"'")
    _, _, comment = line.partition("#")
    return ref, comment.strip()


def judge(ref: str, comment: str) -> str | None:
    """The violation this reference is, or `None` when it is properly pinned."""
    if ref.startswith(LOCAL_PREFIXES):
        return None  # this repository's own code, pinned by the commit being built
    if "@" not in ref:
        return f"`{ref}` names no ref at all"
    action, _, version = ref.rpartition("@")
    if not SHA_RE.match(version):
        return f"`{ref}` is pinned to the movable ref `{version}`"
    if not comment:
        return f"`{action}` is pinned but no comment says which version"
    return None


def scan(root: Path) -> tuple[list[tuple[str, int, str, str]], list[tuple[str, str]]]:
    """Return (entries, violations). An entry is (file, line, ref, comment)."""
    entries: list[tuple[str, int, str, str]] = []
    violations: list[tuple[str, str]] = []
    directory = root / WORKFLOWS
    files = sorted(directory.glob("*.yml")) + sorted(directory.glob("*.yaml"))
    if not files:
        # A check that goes quiet when its subject disappears reports a
        # confidence it no longer has (the reasoning in doc_index_check.py).
        violations.append((WORKFLOWS, "no workflow files found"))
        return entries, violations

    for path in files:
        rel = path.relative_to(root).as_posix()
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            parsed = parse_uses(line)
            if parsed is None:
                continue
            ref, comment = parsed
            problem = judge(ref, comment)
            if problem:
                violations.append((f"{rel}:{number}", problem))
            elif not ref.startswith(LOCAL_PREFIXES):
                entries.append((rel, number, ref, comment))
    return entries, violations


def report(entries: list[tuple[str, int, str, str]], violations, list_mode: bool) -> int:
    if list_mode and entries:
        print(f"Pinned actions ({len(entries)}):")
        for rel, number, ref, comment in entries:
            action, _, sha = ref.rpartition("@")
            print(f"  {rel}:{number}  {action}@{sha[:12]}…  # {comment}")
        print()
    if not violations:
        print(f"actions_pin_check: {len(entries)} action reference(s), all pinned.")
        return 0
    print(f"actions_pin_check: {len(violations)} violation(s)\n")
    for where, message in violations:
        print(f"  {where}: {message}")
    print(
        "\nPin each one to a commit and name the version beside it:\n"
        "  uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09  # v5.1.0\n"
        "The SHA for a tag:\n"
        "  gh api repos/<owner>/<repo>/git/ref/tags/<tag> --jq .object.sha\n"
        "(when that prints an annotated tag object, resolve it once more through\n"
        " `git/tags/<sha>`). Dependabot keeps the pins current afterwards."
    )
    return 1


def main() -> int:
    entries, violations = scan(repo_root())
    return report(entries, violations, "--list" in sys.argv)


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
