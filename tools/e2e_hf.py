#!/usr/bin/env python3
"""Run the live e2e smokes against ephemeral HF Inference Endpoints.

Plan: docs/history/remote-e2e-hf.md §6. Research: docs/research/remote-e2e-gpu.md.
The client (HTTP, payloads, lifecycle, cleanup) is shared with the stage-0
probe: tools/hf_api.py.

One entry point, identical locally and in CI (the precedent is
`packaging/linux/build-packages.sh`). It creates three endpoints — chat
(llama.cpp, gemma-4-31B q4_0, L40S), embeddings (the same engine in
`embeddings` mode, bge-m3 Q8_0, T4) and a *second, different* embedding model
(multilingual-e5-large-instruct q8_0, T4) for the smokes that guard the
embedding-model-change track — waits for all of them, runs the `#[ignore]`
suite against them, and deletes them, verifying the deletion.

The alternate embedder is on by default and costs ~$0.13 of the ~$1 run. That
is the point: without it four memory-critical smokes skip *while reporting ok*,
which is the failure mode a gate exists to prevent. `--no-alt-embed` opts out.

    set HF_TOKEN=hf_...
    python tools/e2e_hf.py run                  # the full remote gate
    python tools/e2e_hf.py run --dry-run        # payloads only, spends nothing
    python tools/e2e_hf.py run --filter e2e_live --no-prebuild
    python tools/e2e_hf.py run --reuse-chat e2e-chat-0728-2010   # iterate, no deploy
    python tools/e2e_hf.py sweep --dry-run      # what the sweeper would delete
    python tools/e2e_hf.py list

Three properties are load-bearing, in this order:

1. **The endpoints are deleted, and the deletion is verified.** Cleanup runs
   from `finally`, `atexit` and SIGINT/SIGTERM, and a failed delete exits 3 —
   *even when the tests passed*. A leaked endpoint is a failure; a red suite is
   just a result.
2. **The names are deterministic and printed before the create call**, so an
   orphan is identifiable from the log even if the response is lost.
3. **The tests are compiled before the GPU is created** (`cargo test --no-run`),
   so a cold CI runner does not spend several minutes of billed L40S time
   linking. `--no-prebuild` turns it off.

Exit codes: 0 ok · 1 usage/config · 2 the suite or a readiness check failed ·
3 CLEANUP FAILED (outranks everything: it is the one that costs money).
"""

from __future__ import annotations

import datetime as dt
import os
import re
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import hf_api as hf  # noqa: E402

# The sweeper deletes `e2e-*` endpoints older than this. It MUST stay above the
# workflow's `timeout-minutes` (45), or the sweeper would delete the endpoints of
# a run that is still using them — turning a backstop into a saboteur. 90 leaves
# 45 minutes of headroom over a job that cannot outlive its own timeout.
SWEEP_MAX_AGE_MIN = 90
SWEEP_PREFIX = "e2e-"

DEFAULT_TEST_ARGS = "--ignored --nocapture --test-threads=1"


def run_id():
    """A stable, short identity for the run: the CI run + attempt, or a local
    timestamp. Part of the endpoint name, so it must survive `safe_name`."""
    gh = os.environ.get("GITHUB_RUN_ID")
    if gh:
        return f"{gh}-{os.environ.get('GITHUB_RUN_ATTEMPT', '1')}"
    return time.strftime("%m%d-%H%M%S")


def cargo_command(args):
    if args.command:
        return args.command
    filt = f" {args.filter}" if args.filter else ""
    return f"cargo test{filt} -- {args.test_args}"


def prebuild(args):
    """Compile the tests before anything is billing.

    `cargo test --no-run` produces exactly the artifacts the real run then
    reuses, so this is pure win: on a cold runner it moves several minutes of
    compilation out of the GPU window. A build failure is caught here too,
    before a single cent is spent.
    """
    filt = f" {args.filter}" if args.filter else ""
    command = f"cargo test{filt} --no-run"
    print(f"\n=== prebuild ===\n  $ {command}\n", flush=True)
    started = time.time()
    code = subprocess.run(command, shell=True).returncode
    print(f"\n  prebuild exit={code} after {time.time() - started:.0f}s", flush=True)
    return code


def run_suite(command, chat_url, embed_url, alt_embed_url, keepalive_every):
    """Run the suite with the endpoint env set.

    The lifecycle stays inside this process on purpose: creating the endpoints
    in one command and running cargo in another would leak them if anything
    died in between (docs/history/remote-e2e-hf.md §6).
    """
    env = dict(os.environ)
    env["MINDFORK_ENGINE_URL"] = f"{chat_url}/v1"
    env["MINDFORK_ENGINE_KEY"] = hf.TOKEN
    for url, url_var, key_var in (
        (embed_url, "MINDFORK_EMBED_URL", "MINDFORK_EMBED_KEY"),
        (alt_embed_url, "MINDFORK_EMBED_URL_ALT", "MINDFORK_EMBED_KEY_ALT"),
    ):
        if url:
            env[url_var] = f"{url}/v1"
            env[key_var] = hf.TOKEN
        else:
            # A stale value from the shell would point the memory smokes at a
            # server this run does not control.
            env.pop(url_var, None)
            env.pop(key_var, None)
    print("\n=== suite ===", flush=True)
    print(f"  MINDFORK_ENGINE_URL={env['MINDFORK_ENGINE_URL']}")
    print(f"  MINDFORK_EMBED_URL={env.get('MINDFORK_EMBED_URL', '(unset — memory smokes will skip)')}")
    print(f"  MINDFORK_EMBED_URL_ALT={env.get('MINDFORK_EMBED_URL_ALT', '(unset — model-change smokes will skip)')}")
    print(f"  $ {command}\n", flush=True)
    started = time.time()
    with KeepAlive(chat_url, [embed_url, alt_embed_url], every=keepalive_every):
        code = subprocess.run(command, shell=True, env=env).returncode
    elapsed = time.time() - started
    print(f"  suite exit={code} after {elapsed:.0f}s", flush=True)
    return code, elapsed


class KeepAlive:
    """Keep the endpoints out of scale-to-zero for as long as the suite runs.

    HF scales an endpoint to zero once it is idle for `--scale-to-zero` minutes,
    and the suite legitimately leaves one untouched for longer than that: the
    alternate embedder is used by `embed_guard` early and by `embed_prefix` some
    twenty minutes later. Waking is not free to the caller — the request that
    wakes it gets a `503`, which is exactly what turned CI run 30396557877 red.

    The alternative was to widen the idle window, but that window *is* the leak
    ceiling: it is what bounds the cost of a run that dies without cleaning up,
    and it is the reason this platform was chosen over a rented pod. A real
    inference request every few minutes keeps `lastUsedAt` fresh and leaves that
    guarantee untouched (user's decision, 2026-07-29).

    Deliberately a daemon thread: it must never keep the process alive, and it
    must never be able to fail the run — every ping is best-effort.
    """

    def __init__(self, chat_url, embed_urls, every=300):
        self.chat_url = chat_url
        self.embed_urls = [u for u in embed_urls if u]
        self.every = every
        self.stop = threading.Event()
        self.thread = None
        self.pings = 0

    def _ping_once(self):
        for url in self.embed_urls:
            hf.http("POST", f"{url}/v1/embeddings", {"input": ["keepalive"]}, timeout=120)
        if self.chat_url:
            hf.http(
                "POST",
                f"{self.chat_url}/v1/chat/completions",
                {"messages": [{"role": "user", "content": "ping"}], "max_tokens": 1},
                timeout=120,
            )

    def _loop(self):
        # `wait` rather than `sleep`, so stopping is immediate at the end of the
        # suite instead of up to `every` seconds later.
        while not self.stop.wait(self.every):
            try:
                self._ping_once()
                self.pings += 1
            except Exception as e:  # a ping must never fail the run
                print(f"  keepalive: ping failed ({type(e).__name__}: {e})", flush=True)

    def __enter__(self):
        self.thread = threading.Thread(target=self._loop, daemon=True, name="keepalive")
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.stop.set()
        if self.thread:
            self.thread.join(timeout=10)
        print(f"  keepalive: {self.pings} round(s) of pings", flush=True)
        return False


def alt_payload(name, args):
    """The alternate embedding endpoint: same engine and flags, other weights."""
    return hf.embed_payload(name, args, repo=hf.ALT_EMBED_REPO, gguf=hf.ALT_EMBED_GGUF)


def bring_up(kind, name, args):
    """Wait for one endpoint (created or reused) and return its URL once it answers."""
    url = hf.wait_running(name, args.timeout)
    if not url:
        return None
    if not hf.wait_healthy(url, timeout=args.health_timeout):
        print(f"  -> {kind} endpoint never became healthy")
        return None
    return url


def cmd_run(args):
    started = time.time()
    ident = args.run_id or run_id()
    chat_name = args.reuse_chat or hf.safe_name(f"e2e-chat-{ident}")
    embed_name = args.reuse_embed or hf.safe_name(f"e2e-embed-{ident}")
    alt_name = args.reuse_alt_embed or hf.safe_name(f"e2e-alt-{ident}")
    want_embed = not args.no_embed
    # The alternate embedder is a *second* model, so it needs the first one to
    # compare against — every smoke that reads MINDFORK_EMBED_URL_ALT also reads
    # MINDFORK_EMBED_URL.
    want_alt = want_embed and not args.no_alt_embed

    # Printed before anything is created: this is the trail an orphan is found by.
    print(f"\nrun id: {ident}")
    print(f"  chat  endpoint: {chat_name}{'  (reused)' if args.reuse_chat else ''}")
    if want_embed:
        print(f"  embed endpoint: {embed_name}{'  (reused)' if args.reuse_embed else ''}")
    if want_alt:
        print(f"  alt   endpoint: {alt_name}{'  (reused)' if args.reuse_alt_embed else ''}")
    print(f"  command: {cargo_command(args)}", flush=True)

    if args.dry_run:
        hf.create(hf.apply_overrides(hf.chat_payload(chat_name, args), args.set), True)
        if want_embed:
            hf.create(hf.apply_overrides(hf.embed_payload(embed_name, args), args.set), True)
        if want_alt:
            hf.create(hf.apply_overrides(alt_payload(alt_name, args), args.set), True)
        print(f"\n--dry-run: nothing created. Would run:\n  $ {cargo_command(args)}")
        return hf.EXIT_OK

    if not args.no_prebuild and prebuild(args) != 0:
        print("\nprebuild failed — no endpoint was created, nothing was billed.")
        return hf.EXIT_FAILED

    # Create both before waiting for either: deploy is ~21 s (chat) and ~84 s
    # (embed), and they overlap.
    if not args.reuse_chat:
        payload = hf.apply_overrides(hf.chat_payload(chat_name, args), args.set)
        if hf.create(payload) is None:
            return hf.EXIT_FAILED
    if want_embed and not args.reuse_embed:
        payload = hf.apply_overrides(hf.embed_payload(embed_name, args), args.set)
        if hf.create(payload) is None:
            return hf.EXIT_FAILED
    if want_alt and not args.reuse_alt_embed:
        if hf.create(hf.apply_overrides(alt_payload(alt_name, args), args.set)) is None:
            return hf.EXIT_FAILED

    chat_url = bring_up("chat", chat_name, args)
    if not chat_url:
        return hf.EXIT_FAILED
    embed_url = None
    if want_embed:
        embed_url = bring_up("embed", embed_name, args)
        if not embed_url:
            return hf.EXIT_FAILED
    alt_embed_url = None
    if want_alt:
        alt_embed_url = bring_up("alt embed", alt_name, args)
        if not alt_embed_url:
            return hf.EXIT_FAILED
    ready = time.time()
    print(f"\n  endpoints ready after {ready - started:.0f}s", flush=True)

    code, suite_time = run_suite(
        cargo_command(args), chat_url, embed_url, alt_embed_url, args.keepalive_seconds
    )

    print("\n=== summary ===")
    print(f"  chat  {chat_name}  {chat_url}")
    print(f"  embed {embed_name}  {embed_url or '(none)'}")
    print(f"  alt   {alt_name}  {alt_embed_url or '(none)'}")
    print(f"  ready in {ready - started:.0f}s, suite {suite_time:.0f}s, total {time.time() - started:.0f}s")
    print(f"  suite exit={code}")
    if args.keep:
        print("  --keep: the endpoints are STILL RUNNING and billing.")
    return hf.EXIT_OK if code == 0 else hf.EXIT_FAILED


# --------------------------------------------------------------------------
# Sweeper
# --------------------------------------------------------------------------
def parse_time(value):
    """ISO-8601 from the API, tolerant of the `Z` suffix and long fractions."""
    if not isinstance(value, str) or not value:
        return None
    text = value.strip().replace("Z", "+00:00")
    # fromisoformat wants at most 6 fractional digits. Truncate the *leading*
    # digit run only — the offset that follows (`+00:00`) has digits of its own,
    # and eating them turns a valid timestamp into an unparseable one.
    text = re.sub(r"\.(\d+)", lambda m: "." + m.group(1)[:6], text, count=1)
    try:
        parsed = dt.datetime.fromisoformat(text)
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=dt.timezone.utc)


def cmd_sweep(args):
    """Delete orphaned `e2e-*` endpoints older than the age limit.

    This catches the one leak the runner's own `finally` cannot: a create that
    succeeded while its response was lost, so nothing ever knew the name. It
    also reclaims endpoint *quota*, which scale-to-zero does not
    (docs/history/remote-e2e-hf.md §6).
    """
    items = hf.list_endpoints()
    if items is None:
        return hf.EXIT_FAILED
    now = dt.datetime.now(dt.timezone.utc)
    limit = dt.timedelta(minutes=args.max_age_minutes)
    doomed, failed, unknown = [], [], []

    print(f"sweep: prefix {args.prefix!r}, older than {args.max_age_minutes} min, {len(items)} endpoint(s)")
    for item in items:
        name = item.get("name", "")
        st = item.get("status") or {}
        state = st.get("state", "?")
        if not name.startswith(args.prefix):
            print(f"  skip  {name:<34} {state:<14} (not {args.prefix}*)")
            continue
        created = parse_time(st.get("createdAt"))
        if created is None:
            # Deliberately fail-safe towards *keeping* it. Deleting an endpoint
            # whose age we cannot establish could kill a run that is still using
            # it — destroying real work for a false red — whereas a leak is
            # already money-bounded by scale-to-zero. Loud, not silent.
            unknown.append(name)
            print(f"  KEEP  {name:<34} {state:<14} !! unreadable createdAt {st.get('createdAt')!r}")
            continue
        age = now - created
        mins = age.total_seconds() / 60
        if age < limit:
            print(f"  keep  {name:<34} {state:<14} {mins:.0f} min old")
            continue
        print(f"  DELETE {name:<33} {state:<14} {mins:.0f} min old", flush=True)
        doomed.append(name)
        if args.dry_run:
            continue
        if not hf.delete_one(name):
            failed.append(name)

    if args.dry_run:
        print(f"\n--dry-run: would delete {len(doomed)}")
        return hf.EXIT_OK
    print(f"\nswept {len(doomed) - len(failed)} of {len(doomed)}")
    if unknown:
        print(f"!! {len(unknown)} endpoint(s) kept with an unreadable creation time: {', '.join(unknown)}")
        print("!! Check them by hand at https://endpoints.huggingface.co/")
    if failed:
        print(f"!!! delete FAILED for: {', '.join(failed)}")
        return hf.EXIT_CLEANUP
    return hf.EXIT_OK


# --------------------------------------------------------------------------
def main():
    parser = hf.run_parser(__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("run", help="create the endpoints, run the suite, delete them")
    hf.add_endpoint_args(p)
    p.add_argument("--run-id", default="", help="identity in the endpoint names (default: CI run id or a timestamp)")
    p.add_argument("--filter", default="", help="cargo test name filter, e.g. e2e_live")
    p.add_argument("--test-args", default=DEFAULT_TEST_ARGS, help="args after `--` (single-threaded is required)")
    p.add_argument("--command", default="", help="run this instead of the composed cargo command")
    p.add_argument("--no-prebuild", action="store_true", help="do not `cargo test --no-run` before deploying")
    p.add_argument("--no-embed", action="store_true", help="chat endpoint only (memory smokes then skip)")
    p.add_argument(
        "--no-alt-embed",
        action="store_true",
        help="skip the second embedding model (the 4 model-change smokes then skip)",
    )
    p.add_argument("--reuse-chat", default="", help="attach to an existing chat endpoint (not created, not deleted)")
    p.add_argument("--reuse-embed", default="", help="attach to an existing embedding endpoint")
    p.add_argument("--reuse-alt-embed", default="", help="attach to an existing alternate embedding endpoint")
    p.add_argument("--health-timeout", type=int, default=900, help="seconds to wait for /health after `running`")
    p.add_argument(
        "--keepalive-seconds",
        type=int,
        default=300,
        help="ping the endpoints this often during the suite, so none scales to zero mid-run",
    )
    p.set_defaults(fn=cmd_run)

    p = sub.add_parser("sweep", help="delete orphaned e2e-* endpoints older than the limit")
    p.add_argument("--max-age-minutes", type=int, default=SWEEP_MAX_AGE_MIN)
    p.add_argument("--prefix", default=SWEEP_PREFIX)
    p.add_argument("--dry-run", action="store_true")
    p.set_defaults(fn=cmd_sweep)

    hf.add_shared_commands(sub)

    args = parser.parse_args()
    hf.init(args.namespace, keep=getattr(args, "keep", False))
    try:
        return args.fn(args)
    finally:
        hf.cleanup()


if __name__ == "__main__":
    sys.exit(main())
