"""Runs one or more `#[ignore]` live smokes N times each and tallies them.

Model behaviour is a rate, not a yes/no: a single green run says little, which is
why the code-workspace track's go/no-go is "at least K of N per model family"
(docs/code-workspace.md section 6). This is the instrument for that.

    MINDFORK_ENGINE_URL=http://host:8000/v1 python tools/probe_runs.py
    MINDFORK_ENGINE_URL=... python tools/probe_runs.py code_workspace_gate_e2e_live
    PROBE_RUNS=10 python tools/probe_runs.py my_smoke_live

It is also what `tools/e2e_hf.py run --command` is handed when an arm has to run
against a rented endpoint, so one deployment covers every repetition instead of
one deploy per run.

Exits non-zero when a smoke falls below the bar, so a run's status is honest
rather than decorative.
"""

import os
import re
import subprocess
import sys
import time

# The smokes to repeat when none are named on the command line.
DEFAULT_TESTS = [
    "code_workspace_navigate_e2e_live",
    "code_workspace_gate_e2e_live",
]
RUNS = int(os.environ.get("PROBE_RUNS", "5"))
# The plan's bar is 3 of 5; a shortened run (smoke-testing this script) keeps the
# same spirit rather than reporting NO-GO for having run once.
THRESHOLD = 3 if RUNS >= 5 else (RUNS + 1) // 2


# A Rust test path is module segments and an identifier; nothing else can name a
# test, so nothing else is accepted from the command line.
TEST_NAME = re.compile(r"\A[A-Za-z0-9_:]{1,120}\Z")


def run_once(test_name):
    """One `cargo test` invocation.

    Returns `(passed, skipped, matched_nothing, evidence lines, seconds)`.
    """
    started = time.time()
    # Two things guard this call, and the order matters for both a reader and a
    # taint analyser:
    #
    #  * the name is validated **here**, immediately before use, and what reaches
    #    the command is the match object's own output rather than the argv string
    #    it was derived from. The check used to live in `main` alone, which reads
    #    as safe and is not: dataflow does not follow a guard across a function
    #    boundary, and SonarQube said so twice (pythonsecurity:S8701, then S8705
    #    on the surviving path) - the agentic-workflows family docs/lessons.md
    #    section 1 already records for CLI paths;
    #  * `shell=False` with an argv list, so no shell ever parses any of it.
    checked = TEST_NAME.fullmatch(test_name)
    if checked is None:
        raise ValueError("not a test name: %r" % (test_name,))
    name = checked.group(0)
    proc = subprocess.run(
        ["cargo", "test", name, "--", "--ignored", "--nocapture", "--test-threads=1"],
        shell=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    # The diagnostics a smoke prints (tool calls, replies) go to stderr via
    # eprintln!, so both streams are always needed - reading stdout alone
    # silently loses exactly the evidence this measurement exists to collect.
    out = (proc.stdout or "") + (proc.stderr or "")
    ran = re.search(r"test result: ok\. (\d+) passed", out)
    # A filter that matches nothing also reports `ok`, which is the "a skipped
    # smoke reporting ok is worse than a failing one" trap (docs/lessons.md
    # section 9). Zero tests run is never a pass.
    matched_nothing = bool(ran) and int(ran.group(1)) == 0
    passed = bool(ran) and int(ran.group(1)) > 0 and proc.returncode == 0
    skipped = "skip: MINDFORK_ENGINE_URL not set" in out
    evidence = [
        ln
        for ln in out.splitlines()
        if ln.startswith("PROBE:") or ln.startswith("→ ") or "tool calls:" in ln
    ]
    return passed, skipped, matched_nothing, evidence, time.time() - started


def _utf8_streams():
    """Forces UTF-8 on the two output streams.

    The evidence lines carry tool results verbatim, which are in the
    agent-scaffold language - on a Windows console that is cp1252, and the run
    would die on a print rather than on anything it measured.
    """
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except AttributeError:
            pass


def _measure(test):
    """Runs one smoke `RUNS` times and returns how many passed.

    `None` means *stop the whole measurement*: the filter matched no test, so
    nothing was measured and every later number would be meaningless.
    """
    wins = 0
    for i in range(1, RUNS + 1):
        passed, skipped, matched_nothing, evidence, secs = run_once(test)
        if matched_nothing:
            print("%s: no test matches this name - nothing was measured" % test, flush=True)
            return None
        if skipped:
            print("%s run %d: SKIPPED - no engine" % (test, i), flush=True)
            continue
        wins += 1 if passed else 0
        print(
            "%s run %d: %s (%.0fs)" % (test, i, "PASS" if passed else "FAIL", secs),
            flush=True,
        )
        for line in evidence:
            print("    %s" % line[:400], flush=True)
    return wins


def _tally(tests, verdicts):
    """Prints the per-smoke verdicts and answers whether all of them cleared the bar."""
    print("\n=== tally ===", flush=True)
    ok = True
    for test in tests:
        wins = verdicts.get(test, 0)
        good = wins >= THRESHOLD
        ok = ok and good
        print(
            "%s: %d/%d - %s (bar: %d)" % (test, wins, RUNS, "GO" if good else "NO-GO", THRESHOLD),
            flush=True,
        )
    return ok


def main():
    _utf8_streams()

    tests = sys.argv[1:] or DEFAULT_TESTS
    # A readable refusal for a typo. The guard that matters is in `run_once`,
    # next to the call it protects - this one only turns a raised ValueError
    # into a sentence.
    bad = [t for t in tests if not TEST_NAME.fullmatch(t)]
    if bad:
        print("not test names: %s" % ", ".join(bad))
        return 2
    if not os.environ.get("MINDFORK_ENGINE_URL"):
        print("MINDFORK_ENGINE_URL is not set - nothing to measure")
        return 2
    print("engine: %s" % os.environ["MINDFORK_ENGINE_URL"], flush=True)
    print("smokes: %s (%d runs each)" % (", ".join(tests), RUNS), flush=True)

    verdicts = {}
    for test in tests:
        wins = _measure(test)
        if wins is None:
            return 2
        verdicts[test] = wins
        print("%s: %d/%d" % (test, wins, RUNS), flush=True)

    return 0 if _tally(tests, verdicts) else 1


if __name__ == "__main__":
    sys.exit(main())
