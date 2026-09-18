#!/usr/bin/env python3
"""The gate a pushed `v*` tag passes before anything is built or published.

A release is the one workflow that runs once per version and nowhere else, and
until this script existed it trusted the tag completely: `release.yml` derived
the version from `GITHUB_REF_NAME`, built the artifacts, pulled the notes out of
`CHANGELOG.md` with an `awk` that silently fell back to the line "Release vX.Y.Z."
when it found nothing, and published the result live with `gh release create`.
So a tag one digit off `Cargo.toml` shipped binaries whose `--version` disagreed
with their own release page, and a forgotten CHANGELOG rename shipped a release
page with no notes — both discovered by whoever downloaded it.

What it checks, in order, before the first build starts:

1. **The tag's shape.** `vX.Y.Z`, optionally with a prerelease suffix
   (`v0.9.9-rc1`).
2. **The tag against `Cargo.toml`.** `X.Y.Z` must equal the package version —
   the source of truth for `env!("CARGO_PKG_VERSION")`, and therefore for what
   the shipped binary answers (AGENTS.md §6).
3. **The notes.** `CHANGELOG.md` must carry a `## [X.Y.Z]` section with
   something in it. A **prerelease** tag may fall back to `## [Unreleased]`,
   which is what makes a rehearsal of the whole pipeline possible against an
   unreleased version (docs/research/release-pipeline.md §6); a final tag may
   not, because an empty release page is exactly the failure this gate exists
   to stop.

It then writes the notes file and reports `version` and `prerelease` on
`$GITHUB_OUTPUT` for the steps that follow.

`--self-test` runs the arms above against in-memory fixtures. A workflow's error
paths are otherwise only ever exercised by a release that fails, which is the
one place where finding out costs the most (docs/lessons.md §10 — validate a
`run:` block by executing it against stubs, not by reading it).

It writes `notes.md` in the working directory — the file
`gh release create --notes-file` is given.

Usage:
    python tools/release_guard.py --tag v0.9.9
    python tools/release_guard.py --self-test
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

TAG_RE = re.compile(r"^v(?P<core>\d+\.\d+\.\d+)(?:-(?P<suffix>[0-9A-Za-z.\-]+))?$")
# `version = "0.9.9"` in the `[package]` table — the first such line in the file,
# which is where Cargo's own manifest format puts it.
VERSION_RE = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)
UNRELEASED = "Unreleased"


class GuardError(Exception):
    """A refusal with a message meant for the workflow log."""


def parse_tag(tag: str) -> tuple[str, str | None]:
    """`v0.9.9-rc1` -> `("0.9.9", "rc1")`. Raises on anything else."""
    match = TAG_RE.match(tag)
    if not match:
        raise GuardError(
            f"the tag {tag!r} is not `vX.Y.Z` with an optional prerelease suffix "
            "(for example v0.9.9 or v0.9.9-rc1)"
        )
    return match.group("core"), match.group("suffix")


def cargo_version(manifest: str) -> str:
    """The `[package] version` of a `Cargo.toml` given as text."""
    match = VERSION_RE.search(manifest)
    if not match:
        raise GuardError("Cargo.toml carries no `version = \"...\"` line")
    return match.group(1)


def changelog_section(changelog: str, heading: str) -> str | None:
    """The body of `## [heading]`, or `None` when there is no such section.

    The body runs to the next `## [` heading, matching how `CHANGELOG.md` is
    structured (Keep a Changelog): version sections at level two, categories
    beneath them.
    """
    lines = changelog.splitlines()
    start = None
    for index, line in enumerate(lines):
        if line.startswith(f"## [{heading}]"):
            start = index + 1
            break
    if start is None:
        return None
    end = len(lines)
    for index in range(start, len(lines)):
        if lines[index].startswith("## ["):
            end = index
            break
    return "\n".join(lines[start:end]).strip("\n")


def build_notes(changelog: str, version: str, prerelease: bool) -> tuple[str, str]:
    """The release notes for `version`, and the source they came from.

    A prerelease falls back to `[Unreleased]` and says so in the notes: the page
    is a rehearsal's, and a reader should not take it for the released set.
    """
    body = changelog_section(changelog, version)
    if body and body.strip():
        return body, version
    if not prerelease:
        raise GuardError(
            f"CHANGELOG.md has no non-empty `## [{version}]` section. "
            "The release checklist (AGENTS.md §6) renames `[Unreleased]` to the "
            "version being tagged; a release page with no notes is the symptom "
            "of that step being skipped."
        )
    body = changelog_section(changelog, UNRELEASED)
    if not body or not body.strip():
        raise GuardError(
            "CHANGELOG.md has neither a `## [%s]` section nor a non-empty "
            "`## [Unreleased]` one to fall back on" % version
        )
    note = (
        f"> A prerelease of {version}. These are the current `[Unreleased]` "
        "notes, not a released set.\n"
    )
    return note + "\n" + body, UNRELEASED


def footer(server_url: str, repository: str, tag: str) -> str:
    """The line every release page ends with: the site, and the pinned guide.

    Pinned to this tag on purpose — the install instructions travel with the
    release they describe, unlike a link to `main`.
    """
    return (
        f"\n---\n\n[mindfork.io](https://mindfork.io) · "
        f"[Install guide]({server_url}/{repository}/blob/{tag}/docs/install.md)\n"
    )


def emit_outputs(**values: str) -> None:
    """Write `name=value` pairs to `$GITHUB_OUTPUT`, and echo them either way."""
    path = os.environ.get("GITHUB_OUTPUT")
    for name, value in values.items():
        print(f"{name}={value}")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            for name, value in values.items():
                handle.write(f"{name}={value}\n")


# The notes file `gh release create --notes-file` is given. A fixed name in the
# working directory rather than an argument: nothing a caller passes should be
# able to decide where this writes, and the workflow only ever wanted `notes.md`
# beside the checkout (SonarQube `pythonsecurity:S2083`).
NOTES_FILE = "notes.md"


def run(tag: str, root: Path) -> int:
    version, suffix = parse_tag(tag)
    manifest = (root / "Cargo.toml").read_text(encoding="utf-8")
    declared = cargo_version(manifest)
    if version != declared:
        raise GuardError(
            f"the tag says {version} and Cargo.toml says {declared}. The manifest "
            "is the source of truth for the version the binary reports, so a "
            "release built from this tag would contradict its own page. Either "
            "the release PR's version bump is missing, or the tag is wrong."
        )
    prerelease = suffix is not None
    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    body, source = build_notes(changelog, version, prerelease)
    server_url = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
    repository = os.environ.get("GITHUB_REPOSITORY", "vshylov/mindfork-rs")
    notes_path = Path.cwd() / NOTES_FILE
    notes_path.write_text(body + "\n" + footer(server_url, repository, tag), encoding="utf-8")

    print(f"tag {tag} matches Cargo.toml {declared}")
    print(f"notes: `[{source}]` -> {notes_path} ({len(body.splitlines())} lines)")
    emit_outputs(version=version, prerelease="true" if prerelease else "false")
    return 0


MANIFEST = '[package]\nname = "mindfork"\nversion = "0.9.9"\nedition = "2024"\n'
CHANGELOG = (
    "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- something new\n\n"
    "## [0.9.9] — 2026-09-01\n\n### Fixed\n\n- the released fix\n\n"
    "## [0.9.8] — 2026-08-01\n\n- older\n"
)


def self_test() -> int:
    """Exercise every arm of the gate against fixtures. Returns an exit code."""
    failures: list[str] = []

    def check(label: str, condition: bool) -> None:
        print(f"{'ok  ' if condition else 'FAIL'} {label}")
        if not condition:
            failures.append(label)

    def refuses(label: str, thunk) -> None:
        try:
            thunk()
        except GuardError as error:
            print(f"ok   {label}: {error}")
            return
        check(label, False)

    check("a plain tag parses", parse_tag("v0.9.9") == ("0.9.9", None))
    check("a suffixed tag parses", parse_tag("v0.9.9-rc1") == ("0.9.9", "rc1"))
    refuses("a two-part tag is refused", lambda: parse_tag("v0.9"))
    refuses("a tag without `v` is refused", lambda: parse_tag("0.9.9"))
    refuses("a tag with a trailing word is refused", lambda: parse_tag("v0.9.9 rc"))
    check("the manifest version is read", cargo_version(MANIFEST) == "0.9.9")
    refuses("a manifest without a version is refused", lambda: cargo_version("[package]\n"))

    body, source = build_notes(CHANGELOG, "0.9.9", prerelease=False)
    check("a released section is found", source == "0.9.9" and "the released fix" in body)
    check("the section stops at the next one", "older" not in body)

    body, source = build_notes(CHANGELOG, "0.9.10", prerelease=True)
    check(
        "a prerelease falls back to [Unreleased]",
        source == UNRELEASED and "something new" in body and "prerelease of 0.9.10" in body,
    )
    refuses(
        "a final tag with no section is refused",
        lambda: build_notes(CHANGELOG, "0.9.10", prerelease=False),
    )
    refuses(
        "an empty section is refused",
        lambda: build_notes("# Changelog\n\n## [0.9.9]\n\n## [0.9.8]\n", "0.9.9", False),
    )
    refuses(
        "a prerelease with nothing to fall back on is refused",
        lambda: build_notes("# Changelog\n\n## [Unreleased]\n", "0.9.10", True),
    )
    check(
        "the footer pins the guide to the tag",
        "/blob/v0.9.9-rc1/docs/install.md"
        in footer("https://github.com", "vshylov/mindfork-rs", "v0.9.9-rc1"),
    )

    if failures:
        print(f"\n{len(failures)} self-test failure(s)")
        return 1
    print("\nrelease_guard self-test: all arms behave")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", default=os.environ.get("GITHUB_REF_NAME"))
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if not args.tag:
        print("error: --tag (or GITHUB_REF_NAME) is required", file=sys.stderr)
        return 2
    try:
        return run(args.tag, Path(__file__).resolve().parent.parent)
    except GuardError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    # The notes carry em-dashes and the odd arrow; a Windows console defaults to
    # cp1252, where printing them raises UnicodeEncodeError exactly when the
    # report is wanted (the trap `cyrillic_scan.py` hit first).
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
