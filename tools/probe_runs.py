"""Runs the stage-0 code-workspace probes N times per arm and tallies them.

Handed to `tools/e2e_hf.py run --command`, so one rented endpoint covers the
whole measurement instead of one deploy per run. Locally it is just:

    MINDFORK_ENGINE_URL=http://host:8000/v1 python tools/probe_runs.py

The go/no-go bar is per family (docs/code-workspace.md section 6): at least
THRESHOLD of RUNS reaching a correct, compiling edit. Exits non-zero below it,
so the run's status is honest rather than decorative.
"""

import os
import re
import subprocess
import sys
import time

ARMS = [
    ("A", "code_edit_probe_live"),
    ("B", "code_edit_probe_ambiguous_live"),
]
RUNS = int(os.environ.get("PROBE_RUNS", "5"))
# The plan's bar is 3 of 5; a shortened run (smoke-testing this script) keeps
# the same spirit rather than reporting NO-GO for having run once.
THRESHOLD = 3 if RUNS >= 5 else (RUNS + 1) // 2


def run_once(test_name):
    """One `cargo test` invocation. Returns (passed, probe_lines, edit_args)."""
    started = time.time()
    proc = subprocess.run(
        "cargo test %s -- --ignored --nocapture --test-threads=1" % test_name,
        shell=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    # The diagnostics the smoke prints (tool calls, PROBE lines) go to stderr
    # via eprintln!, so both streams are always needed - reading stdout alone
    # silently loses exactly the evidence this measurement exists to collect.
    out = (proc.stdout or "") + (proc.stderr or "")
    passed = "test result: ok." in out and proc.returncode == 0
    # A smoke that skips reports ok too, which is the failure mode a gate exists
    # to prevent (docs/lessons.md section 9) - treat it as neither pass nor fail.
    skipped = "skip: MINDFORK_ENGINE_URL not set" in out
    probe = [ln for ln in out.splitlines() if ln.startswith("PROBE:")]
    edits = re.findall(r"^→ code_edit\((.*)$", out, re.M)
    return passed, skipped, probe, edits, time.time() - started


def main():
    # The evidence lines carry the tool results verbatim, which are in the
    # agent-scaffold language - on a Windows console that is cp1252 and the
    # whole run dies on a print rather than on anything it measured.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except AttributeError:
            pass
    if not os.environ.get("MINDFORK_ENGINE_URL"):
        print("MINDFORK_ENGINE_URL is not set - nothing to measure")
        return 2
    print("engine: %s" % os.environ["MINDFORK_ENGINE_URL"], flush=True)
    verdicts = {}
    for label, test in ARMS:
        wins = 0
        for i in range(1, RUNS + 1):
            passed, skipped, probe, edits, secs = run_once(test)
            if skipped:
                print("arm %s run %d: SKIPPED - no engine" % (label, i), flush=True)
                continue
            wins += 1 if passed else 0
            print(
                "arm %s run %d: %s (%.0fs) %s"
                % (label, i, "PASS" if passed else "FAIL", secs, probe[0] if probe else ""),
                flush=True,
            )
            for e in edits:
                print("    edit: %s" % e[:400], flush=True)
        verdicts[label] = wins
        print("ARM %s: %d/%d" % (label, wins, RUNS), flush=True)

    print("\n=== stage 0 tally ===", flush=True)
    ok = True
    for label, _ in ARMS:
        wins = verdicts.get(label, 0)
        good = wins >= THRESHOLD
        ok = ok and good
        print(
            "arm %s: %d/%d - %s (bar: %d)"
            % (label, wins, RUNS, "GO" if good else "NO-GO", THRESHOLD),
            flush=True,
        )
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
