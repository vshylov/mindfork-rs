#!/usr/bin/env python3
"""Repository metrics: the numbers the project quotes about itself.

The article about the project, the release posts and the journal quote figures
about this repository — days, commits, pull requests, test functions, the share
of test code, the documentation's size against the code's, the journal's census
of measured and rejected work — and each used to be an ad-hoc shell pipeline,
written again every time and never quite the same way twice. This script is the
one definition of each number. Run it from anywhere inside the repository:

    python tools/repo_metrics.py              # a Markdown table
    python tools/repo_metrics.py --json       # the same numbers as JSON
    python tools/repo_metrics.py --self-test  # the definitions, against a fixture repo

What each number means, where it is not obvious:

* ``pull_requests_merged`` counts merge commits whose subject is GitHub's
  "Merge pull request #N …"; ``merge_commits`` counts every merge, the local
  ones of the first days included.
* ``docs_only_share_pct`` is the share of non-merge commits that touch the
  documentation (``docs/``, ``spec.md``, ``CLAUDE.md``, ``AGENTS.md``,
  ``README.md``, ``CHANGELOG.md``) and nothing under ``src/``.
* ``test_share_pct`` estimates test code as whole files under a ``tests/``
  directory or named ``*tests.rs``, plus everything from the first
  ``#[cfg(test)]`` that opens a ``mod`` to the end of the file. An estimate: a
  ``#[cfg(test)]`` on a lone ``use`` does not start the test region.
* ``cargo_unit_tests_green`` and ``cargo_ignored_tests`` are read from
  CLAUDE.md's Status line, because only ``cargo test`` knows which test
  functions compile on a given platform; ``test_fn_attributes`` is the raw count
  of ``#[test]`` and ``#[tokio::test]`` in the tree, which is larger.
* ``control_arm_tests`` counts test functions whose doc comment names the
  control arm — the same code exercised with the defect switched off.
* The journal census counts ENTRIES (the block under one ``### `` heading) that
  match a pattern, never lines, so an entry that says "measured" five times
  counts once.
* ``entries_bare`` counts the journal entries that carry no evidence of any
  kind: no test count (the word "test" with a number near it), no GO, NO-GO or
  smoke line, and no measurement, probe or live run. ``entries_bare_first_half``
  says how many of them sit in the first half of their file — the early ones.

Sonar's figures are not here; they live on SonarCloud. Nothing is written. The
self-test builds a small git repository in a temporary directory — known
commits with model trailers, a pull-request merge, a tag, and a tree of the
shapes the counters look for — and checks every number against what was
planted. It needs ``git`` on PATH and nothing else.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
from collections import Counter
from datetime import date
from pathlib import Path

CLAUDE_MD = "CLAUDE.md"
SPEC_MD = "spec.md"
DOC_ROOTS = ("docs/", SPEC_MD, CLAUDE_MD, "AGENTS.md", "README.md", "CHANGELOG.md")
UTF8 = "utf-8"
KEY_SRC_BYTES = "src_bytes"
KEY_DOCS_BYTES = "docs_markdown_bytes"
CITATION = re.compile(r"spec §|architecture §|§\d")
TEST_FN = re.compile(r"#\[(?:tokio::)?test[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
IGNORE_ATTR = re.compile(r"#\[ignore")
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]\s*$")
MOD_LINE = re.compile(r"\s*(pub(\([^)]*\))?\s+)?mod\s+\w+")
STATUS_LINE = re.compile(r"\*\*(\d+) unit tests\s+green, (\d+) `#\[ignore\]`")
ENTRIES_INDEX = re.compile(r"^## Entries \((\d+)\)", re.M)
PR_SUBJECT = re.compile(r"Merge pull request #(\d+)")
VERSION_TAG = re.compile(r"v\d+\.\d+\.\d+")
RECURRENCE = re.compile(r"Recorded (twice|three times|four times|five times|six times)")
NAME_WORDS = ("refuse", "does_not", "says_", "never", "stays")
TEST_ATTR_LINE = re.compile(r"^\s*#\[(?:tokio::)?test")
ATTR_LINE = re.compile(r"^\s*#\[")
DOC_LINE = re.compile(r"^\s*///")
CONTROL_ARM = re.compile(r"control arm", re.I)
EVIDENCE = (
    re.compile(r"tests?[^.\n]{0,40}\d", re.I),  # a test count
    re.compile(r"\bGO\b|NO-GO|[Ss]moke"),  # a live run's verdict
    re.compile(r"measur|probe|live run", re.I),  # a measurement
)
CENSUS = {
    "entries_measured": re.compile(r"measured", re.I),
    "entries_rejected": re.compile(r"rejected", re.I),
    "entries_with_go": re.compile(r"\bGO\b"),
    "entries_no_live_run": re.compile(r"no live run", re.I),
    "entries_user_decision": re.compile(
        r"user's decision|owner's decision|the owner chose|the user chose|confirmed by the user|"
        r"the user confirmed|user decided|owner decided|decided by the user",
        re.I,
    ),
    "entries_mutation": re.compile(r"mutation|mutant", re.I),
    "entries_control_arm": re.compile(r"control arm", re.I),
    "entries_with_test_count": re.compile(r"tests?\s+green", re.I),
}
TOP_LEVEL_DOCS = (SPEC_MD, CLAUDE_MD, "AGENTS.md", "README.md", "CHANGELOG.md")


def git(root: Path | None, *args: str) -> str:
    """Run git in ``root`` (the current directory when ``None``) and return its stdout."""
    done = subprocess.run(
        ["git", *args],
        cwd=root,
        capture_output=True,
        text=True,
        encoding=UTF8,
        errors="replace",
        check=True,
    )
    return done.stdout


def read(path: Path) -> str:
    return path.read_text(encoding=UTF8, errors="replace")


def is_doc_path(path: str) -> bool:
    return any(path == r or path.startswith(r) for r in DOC_ROOTS)


def commit_touches(root: Path) -> tuple[int, int, int, int]:
    """(non-merge commits, touching src, touching docs, docs-only), one pass over the log."""
    non_merge = touches_src = touches_docs = docs_only = 0
    for block in git(root, "log", "--no-merges", "--name-only", "--format=%x00%H").split("\x00"):
        lines = [ln for ln in block.splitlines() if ln.strip()]
        if not lines:
            continue
        non_merge += 1
        files = lines[1:]
        src = any(f.startswith("src/") for f in files)
        docs = any(is_doc_path(f) for f in files)
        touches_src += src
        touches_docs += docs
        docs_only += docs and not src
    return non_merge, touches_src, touches_docs, docs_only


def model_trailers(root: Path) -> dict[str, int]:
    trailers: Counter[str] = Counter()
    for line in git(root, "log", "--no-merges", "--format=%(trailers:key=Co-Authored-By,valueonly)").splitlines():
        name = line.strip().split(" <")[0]
        if name:
            trailers[name] += 1
    return dict(trailers.most_common())


def history(root: Path) -> dict:
    dates = git(root, "log", "--format=%ad", "--date=short").splitlines()
    first, last = dates[-1], dates[0]
    days = (date.fromisoformat(last) - date.fromisoformat(first)).days
    pr_numbers = [int(m) for s in git(root, "log", "--merges", "--format=%s").splitlines() for m in PR_SUBJECT.findall(s)]
    tags = [t for t in git(root, "tag").splitlines() if VERSION_TAG.fullmatch(t)]
    non_merge, touches_src, touches_docs, docs_only = commit_touches(root)
    return {
        "first_commit": first,
        "last_commit": last,
        "days": days,
        "commits": int(git(root, "rev-list", "--count", "HEAD")),
        "merge_commits": int(git(root, "rev-list", "--count", "--merges", "HEAD")),
        "pull_requests_merged": len(pr_numbers),
        "highest_pr_number": max(pr_numbers, default=0),
        "prs_per_day": round(len(pr_numbers) / max(1, days), 2),
        "version_tags": len(tags),
        "latest_tag": max(tags, key=lambda t: tuple(int(x) for x in t[1:].split(".")), default=""),
        "non_merge_commits": non_merge,
        "commits_touching_src": touches_src,
        "commits_touching_docs": touches_docs,
        "docs_only_commits": docs_only,
        "docs_only_share_pct": round(100 * docs_only / non_merge, 1) if non_merge else 0,
        "model_trailers": model_trailers(root),
    }


def test_lines_of(path: Path, lines: list[str]) -> int:
    """Lines of a file that are test code: a whole file under ``tests/`` or named
    ``*tests.rs``, otherwise everything from the first ``#[cfg(test)]`` that opens a
    ``mod`` to the end."""
    if "tests" in path.parts[:-1] or path.name.endswith("tests.rs"):
        return len(lines)
    for i, line in enumerate(lines):
        if not CFG_TEST.match(line):
            continue
        following = next((ln for ln in lines[i + 1 : i + 4] if ln.strip()), "")
        if MOD_LINE.match(following):
            return len(lines) - i
    return 0


def control_arm_tests_of(lines: list[str]) -> int:
    """Test functions whose doc comment — above the attributes — names the control arm."""
    count = 0
    for i, line in enumerate(lines):
        if not TEST_ATTR_LINE.match(line):
            continue
        j = i - 1
        while j >= 0 and ATTR_LINE.match(lines[j]):
            j -= 1
        while j >= 0 and DOC_LINE.match(lines[j]):
            if CONTROL_ARM.search(lines[j]):
                count += 1
                break
            j -= 1
    return count


def source_tree(root: Path) -> dict:
    files = sorted((root / "src").rglob("*.rs"))
    total = test_lines = ignored = citations = cited_files = control_arms = 0
    names: list[str] = []
    for path in files:
        text = read(path)
        lines = text.splitlines()
        total += len(lines)
        test_lines += test_lines_of(path, lines)
        names.extend(TEST_FN.findall(text))
        ignored += len(IGNORE_ATTR.findall(text))
        control_arms += control_arm_tests_of(lines)
        hits = sum(1 for line in lines if CITATION.search(line))
        citations += hits
        cited_files += bool(hits)
    status = STATUS_LINE.search(read(root / CLAUDE_MD))
    return {
        "rs_files": len(files),
        "src_lines": total,
        "test_lines_estimate": test_lines,
        "test_share_pct": round(100 * test_lines / total, 1) if total else 0,
        "test_fn_attributes": len(names),
        "ignore_attrs": ignored,
        "cargo_unit_tests_green": int(status.group(1)) if status else None,
        "cargo_ignored_tests": int(status.group(2)) if status else None,
        "test_names_by_word": {w: sum(1 for n in names if w in n) for w in NAME_WORDS},
        "control_arm_tests": control_arms,
        "doc_citations_in_code": citations,
        "files_with_citations": cited_files,
        KEY_SRC_BYTES: sum(p.stat().st_size for p in files),
    }


def journal_census(journal: list[Path]) -> dict:
    headings = index_sum = bare = bare_first_half = 0
    census = dict.fromkeys(CENSUS, 0)
    for path in journal:
        text = read(path)
        index_sum += sum(int(n) for n in ENTRIES_INDEX.findall(text))
        entries = re.split(r"^### ", text, flags=re.M)[1:]
        headings += len(entries)
        for position, entry in enumerate(entries):
            for key, pattern in CENSUS.items():
                census[key] += bool(pattern.search(entry))
            if not any(pattern.search(entry) for pattern in EVIDENCE):
                bare += 1
                bare_first_half += position < len(entries) / 2
    return {
        "journal_files": len(journal),
        "journal_headings": headings,
        "journal_index_sum": index_sum,
        **census,
        "entries_bare": bare,
        "entries_bare_first_half": bare_first_half,
    }


def documents(root: Path) -> dict:
    md = list((root / "docs").rglob("*.md")) + [root / n for n in TOP_LEVEL_DOCS if (root / n).exists()]
    lessons = root / "docs" / "lessons.md"
    lessons_text = read(lessons)
    return {
        KEY_DOCS_BYTES: sum(p.stat().st_size for p in md),
        **journal_census(sorted((root / "docs" / "journal").glob("*.md"))),
        "lessons_rules": sum(1 for line in lessons_text.splitlines() if line.startswith("**")),
        "lessons_bytes": lessons.stat().st_size,
        "lessons_recurrences": dict(Counter(RECURRENCE.findall(lessons_text))),
        "research_docs": len(list((root / "docs" / "research").glob("*.md"))),
        "history_docs": len(list((root / "docs" / "history").glob("*.md"))),
        "adrs": len(list((root / "docs" / "decisions").glob("*.md"))),
        "claude_md_bytes": (root / CLAUDE_MD).stat().st_size,
        "spec_bytes": (root / SPEC_MD).stat().st_size,
        "architecture_bytes": (root / "docs" / "architecture.md").stat().st_size,
    }


def collect(root: Path) -> dict:
    data = {**history(root), **source_tree(root), **documents(root)}
    data["docs_to_src_bytes_ratio"] = round(data[KEY_DOCS_BYTES] / data[KEY_SRC_BYTES], 2) if data[KEY_SRC_BYTES] else 0
    return data


def render_table(data: dict) -> str:
    rows = ["| Metric | Value |", "| --- | --- |"]
    for key, value in data.items():
        if isinstance(value, dict):
            value = ", ".join(f"{k}: {v}" for k, v in value.items())
        rows.append(f"| {key} | {value} |")
    return "\n".join(rows)


# --- self-test: a fixture repository with known numbers ---------------------------

FIXTURE_MAIN_RS = """//! The entry point (spec §1).
fn main() {}

fn helper() -> u32 { 1 }

#[cfg(test)]
mod tests {
    #[test]
    fn refuses_a_bad_value() {}

    #[tokio::test]
    async fn says_what_it_did() {}

    /// The control arm: the same path with the defect switched off.
    #[ignore]
    #[test]
    fn never_runs_without_a_server() {}
}
"""
FIXTURE_TESTS_RS = """use super::*;

#[test]
fn stays_in_place() {}
"""
FIXTURE_CLAUDE_MD = "# Guide\n\nStatus: **12 unit tests green, 3 `#[ignore]`** today.\n"
FIXTURE_LESSONS_MD = "# Lessons\n\n**Rule one.** Recorded twice.\n\n**Rule two.** Text.\n"
FIXTURE_JOURNAL_MD = """# Alpha

## Entries (4)

- Post-M9: first (done)
- Post-M9: second (done)
- Post-M9: third (done)
- Post-M9: fourth (done)

### Post-M9: first (done)

Measured on a stand, then measured again the next day; Smoke — GO. 12 tests
green. The user's decision: (a).

### Post-M9: second (done)

Rejected after a mutation check with a control arm; no live run needed.

### Post-M9: third (done)

A note that asserts and shows nothing — the third of four, so it sits on the
half-way line and not before it.

### Post-M9: fourth (done)

Another note that shows nothing.
"""
FIXTURE_JOURNAL_ADDENDUM = "\nMore words, in a docs-only commit.\n"
FIXTURE_SMALL_MD = "# A heading\n\nA line of text.\n"
FIXTURE_DOCS = {
    CLAUDE_MD: FIXTURE_CLAUDE_MD,
    SPEC_MD: FIXTURE_SMALL_MD,
    "docs/architecture.md": FIXTURE_SMALL_MD,
    "docs/lessons.md": FIXTURE_LESSONS_MD,
    "docs/journal/alpha.md": FIXTURE_JOURNAL_MD,
    "docs/research/probe.md": FIXTURE_SMALL_MD,
    "docs/history/plan.md": FIXTURE_SMALL_MD,
    "docs/decisions/0001-choice.md": FIXTURE_SMALL_MD,
}
FIXTURE_IDENTITY = ("-c", "user.name=fixture", "-c", "user.email=fixture@example.com", "-c", "commit.gpgsign=false")


def write_fixture_file(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text.encode(UTF8))


def fixture_git(root: Path, *args: str) -> str:
    return git(root, *FIXTURE_IDENTITY, *args)


def fixture_commit(root: Path, message: str) -> None:
    fixture_git(root, "add", "-A")
    fixture_git(root, "commit", "-q", "-m", message)


def fixture_switch(root: Path, branch: str, new: bool = False) -> None:
    fixture_git(root, "checkout", "-q", *(("-b",) if new else ()), branch)


def build_fixture(root: Path) -> None:
    """Six commits on three branches: a skeleton, a docs-only edit, a PR branch with a
    file that is neither code nor docs and its merge in GitHub's words, a tag, and a
    local branch merged in git's own words — a merge that is not a pull request."""
    git(root, "init", "-q")
    git(root, "symbolic-ref", "HEAD", "refs/heads/main")
    write_fixture_file(root, "src/main.rs", FIXTURE_MAIN_RS)
    write_fixture_file(root, "src/app/tests.rs", FIXTURE_TESTS_RS)
    for rel, text in FIXTURE_DOCS.items():
        write_fixture_file(root, rel, text)
    fixture_commit(root, "feat: skeleton\n\nCo-Authored-By: Model A <a@example.com>")
    write_fixture_file(root, "docs/journal/alpha.md", FIXTURE_JOURNAL_MD + FIXTURE_JOURNAL_ADDENDUM)
    fixture_commit(root, "docs: journal\n\nCo-Authored-By: Model B <b@example.com>")
    fixture_switch(root, "feat/x", new=True)
    write_fixture_file(root, "Cargo.toml", "[package]\nname = \"fixture\"\n")
    fixture_commit(root, "chore: manifest\n\nCo-Authored-By: Model A <a@example.com>")
    fixture_switch(root, "main")
    fixture_git(root, "merge", "-q", "--no-ff", "-m", "Merge pull request #7 from owner/feat/x", "feat/x")
    fixture_git(root, "tag", "v0.1.0")
    fixture_switch(root, "local", new=True)
    write_fixture_file(root, "notes.txt", "a local branch\n")
    fixture_commit(root, "chore: notes\n\nCo-Authored-By: Model B <b@example.com>")
    fixture_switch(root, "main")
    # The merge carries a trailer of its own so that the trailer census, which
    # counts non-merge commits only, is seen to leave merges out.
    fixture_git(root, "merge", "-q", "--no-ff", "-m", "Merge branch 'local' into main\n\nCo-Authored-By: Model C <c@example.com>", "local")


def expected_fixture_numbers() -> dict:
    today = date.today().isoformat()
    docs_bytes = sum(len(t.encode(UTF8)) for t in FIXTURE_DOCS.values()) + len(FIXTURE_JOURNAL_ADDENDUM.encode(UTF8))
    src_bytes = len(FIXTURE_MAIN_RS.encode(UTF8)) + len(FIXTURE_TESTS_RS.encode(UTF8))
    return {
        "first_commit": today,
        "last_commit": today,
        "days": 0,
        "commits": 6,
        "merge_commits": 2,
        "pull_requests_merged": 1,
        "highest_pr_number": 7,
        "version_tags": 1,
        "latest_tag": "v0.1.0",
        "non_merge_commits": 4,
        "commits_touching_src": 1,
        "commits_touching_docs": 2,
        "docs_only_commits": 1,
        "docs_only_share_pct": 25.0,
        "model_trailers": {"Model A": 2, "Model B": 2},
        "rs_files": 2,
        "src_lines": 22,
        "test_lines_estimate": 17,
        "test_share_pct": 77.3,
        "test_fn_attributes": 4,
        "ignore_attrs": 1,
        "cargo_unit_tests_green": 12,
        "cargo_ignored_tests": 3,
        "test_names_by_word": {"refuse": 1, "does_not": 0, "says_": 1, "never": 1, "stays": 1},
        "control_arm_tests": 1,
        "doc_citations_in_code": 1,
        "files_with_citations": 1,
        KEY_SRC_BYTES: src_bytes,
        KEY_DOCS_BYTES: docs_bytes,
        "journal_files": 1,
        "journal_headings": 4,
        "journal_index_sum": 4,
        "entries_measured": 1,
        "entries_rejected": 1,
        "entries_with_go": 1,
        "entries_no_live_run": 1,
        "entries_user_decision": 1,
        "entries_mutation": 1,
        "entries_control_arm": 1,
        "entries_with_test_count": 1,
        "entries_bare": 2,
        "entries_bare_first_half": 0,
        "lessons_rules": 2,
        "lessons_recurrences": {"twice": 1},
        "research_docs": 1,
        "history_docs": 1,
        "adrs": 1,
        "claude_md_bytes": len(FIXTURE_CLAUDE_MD.encode(UTF8)),
        "docs_to_src_bytes_ratio": round(docs_bytes / src_bytes, 2),
    }


def self_test() -> int:
    with tempfile.TemporaryDirectory(prefix="repo-metrics-") as tmp:
        root = Path(tmp)
        build_fixture(root)
        data = collect(root)
    failures = [
        f"{key}: expected {want!r}, got {data.get(key)!r}" for key, want in expected_fixture_numbers().items() if data.get(key) != want
    ]
    for failure in failures:
        print(f"self-test FAILED: {failure}")
    if failures:
        return 1
    print(f"self-test: {len(expected_fixture_numbers())} numbers match the fixture")
    return 0


def main(argv: list[str]) -> int:
    flags = set(argv)
    if flags - {"--json", "--self-test"}:
        print(__doc__.split("\n\n", 1)[0])
        print("usage: repo_metrics.py [--json | --self-test]")
        return 2
    if "--self-test" in flags:
        return self_test()
    root = Path(git(None, "rev-parse", "--show-toplevel").strip())
    data = collect(root)
    print(json.dumps(data, indent=2, ensure_ascii=False) if "--json" in flags else render_table(data))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
