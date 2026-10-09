#!/usr/bin/env python3
"""Run the live e2e smokes against ephemeral HF Inference Endpoints.

Plan: docs/history/remote-e2e-hf.md §6. Research: docs/research/remote-e2e-gpu.md.
The client (HTTP, payloads, lifecycle, cleanup) is shared with the stage-0
probe: tools/hf_api.py.

One entry point, identical locally and in CI (the precedent is
`packaging/linux/build-packages.sh`). It creates three endpoints — chat
(llama.cpp, `--chat-model`, on the GPU that model's record names), embeddings
(the same engine in
`embeddings` mode, bge-m3 Q8_0, T4) and a *second, different* embedding model
(multilingual-e5-large-instruct q8_0, T4) for the smokes that guard the
embedding-model-change track — waits for all of them, runs the `#[ignore]`
suite against them, and deletes them, verifying the deletion.

The chat model is **one dispatch, one model** (`--chat-model`, default
gemma-4-31b; docs/history/e2e-second-chat-model.md fork F2). Running both
families in one go would run the suite twice — and one family's suite is already
~70 minutes, which is why the workflow deals it across shards (`--shard`), each a
job under a ceiling that must stay below the sweeper's 90-minute threshold.

The alternate embedder is on by default and costs ~$0.13 of the ~$1 run. That
is the point: without it four memory-critical smokes skip *while reporting ok*,
which is the failure mode a gate exists to prevent. `--no-alt-embed` opts out.

    set HF_TOKEN=hf_...
    python tools/e2e_hf.py run                  # the full remote gate
    python tools/e2e_hf.py run --chat-model qwen-3.6-27b   # the other model family
    python tools/e2e_hf.py run --chat-model gpt-oss-120b   # split weights, H200, text-only
    python tools/e2e_hf.py run --dry-run        # payloads only, spends nothing
    python tools/e2e_hf.py run --filter e2e_live --no-prebuild
    python tools/e2e_hf.py run --shard 2/3      # every third smoke, endpoints of its own
    python tools/e2e_hf.py run --reuse-chat e2e-chat-0728-2010   # iterate, no deploy
    python tools/e2e_hf.py sweep --dry-run      # what the sweeper would delete
    python tools/e2e_hf.py delete-run --run-id 0728-2010   # a run's endpoints, by name
    python tools/e2e_hf.py list
    python tools/e2e_hf.py --self-test          # the offline arms, no token needed

Three properties are load-bearing, in this order:

1. **The endpoints are deleted, and the deletion is verified.** Cleanup runs
   from `finally`, `atexit` and SIGINT/SIGTERM, and a failed delete exits 3 —
   *even when the tests passed*. A leaked endpoint is a failure; a red suite is
   just a result. In CI the runner is `exec`ed, so the cancellation's signal
   reaches it, and a step of its own then runs `delete-run` whatever happened:
   the handlers only help a process that is alive to run them.
2. **The names are deterministic and printed before the create call**, so an
   orphan is identifiable from the log even if the response is lost.
3. **The tests are compiled before the GPU is created** (`cargo test --no-run`),
   so a cold CI runner does not spend several minutes of billed L40S time
   linking. `--no-prebuild` turns it off.

Exit codes: 0 ok · 1 usage/config · 2 the suite or a readiness check failed ·
3 CLEANUP FAILED (outranks everything: it is the one that costs money).
"""

from __future__ import annotations

import argparse
import datetime as dt
import os
import re
import shlex
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import hf_api as hf  # noqa: E402

# The sweeper deletes `e2e-*` endpoints older than this. It MUST stay above the
# workflow's `timeout-minutes` (60), or the sweeper would delete the endpoints of
# a run that is still using them — turning a backstop into a saboteur. 90 leaves
# 30 minutes of headroom over a job that cannot outlive its own timeout
# (`--self-test` checks the order).
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


def cargo_command(args, names=None):
    """The suite's command: a string for the shell, or — for a shard — an argv
    list that names its tests exactly. A list, because a shard's names run to
    kilobytes and `cmd.exe`, which `shell=True` means on Windows, stops at 8191
    characters."""
    if args.command:
        return args.command
    if names is not None:
        return ["cargo", "test", "--", *shlex.split(args.test_args), "--exact", *names]
    filt = f" {args.filter}" if args.filter else ""
    return f"cargo test{filt} -- {args.test_args}"


def show_command(command):
    """A command as the log shows it: a shard's names are listed on their own."""
    if isinstance(command, str):
        return command
    head = command[: command.index("--exact") + 1]
    return f"{' '.join(head)} <{len(command) - len(head)} names, listed above>"


# A shard's number goes into the endpoint names as `-s<i>`, and one digit is
# what keeps a CI name — `e2e-chat-gemma-<11-digit run>-<attempt>-s<i>` — within
# the API's 32 characters (`--self-test` checks it).
SHARD_MAX = 9


def parse_shard(text):
    """`i/N` -> (i, N), 1 <= i <= N <= SHARD_MAX. An argparse `type`."""
    m = re.fullmatch(r"(\d+)/(\d+)", text.strip())
    if not m:
        raise argparse.ArgumentTypeError(f"{text!r} is not i/N, e.g. 2/3")
    i, n = int(m.group(1)), int(m.group(2))
    if not 1 <= i <= n <= SHARD_MAX:
        raise argparse.ArgumentTypeError(f"{text!r}: need 1 <= i <= N <= {SHARD_MAX}")
    return i, n


def sharded(shard):
    """`1/1` is the whole suite, exactly as no `--shard` at all — so the workflow
    can always pass one, and a single-job dispatch keeps the names it had."""
    return shard is not None and shard[1] > 1


def run_ident(run, shard):
    """The identity a run's endpoint names carry. A shard is a run of its own:
    two shards of one dispatch never share an endpoint, and neither's cleanup —
    nor its backstop step — can delete the other's."""
    return f"{run}-s{shard[0]}" if sharded(shard) else run


def parse_test_list(text):
    """The names in `cargo test -- --ignored --list` output (`<name>: test`)."""
    return [line[: -len(": test")] for line in text.splitlines() if line.endswith(": test")]


def shard_of(names, shard):
    """Every N-th smoke, starting at the i-th, in the listed (libtest) order.

    Round-robin rather than balanced by measured durations: it needs no timing
    file to keep current, and the order is alphabetical by module path, so the
    heavy orchestrator smokes — one contiguous block — are dealt evenly across
    the shards. Each shard keeps libtest's relative order.
    """
    i, n = shard
    return names[i - 1 :: n]


def list_tests(args):
    """The `#[ignore]` smokes `--filter` selects, or None if the listing failed.
    Runs before any endpoint exists, so a shard learns what it holds for free."""
    filt = [args.filter] if args.filter else []
    command = ["cargo", "test", *filt, "--", "--ignored", "--list"]
    print(f"\n=== list ===\n  $ {' '.join(command)}", flush=True)
    out = subprocess.run(command, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace")
    if out.returncode != 0:
        print(f"  list exit={out.returncode}", flush=True)
        return None
    return parse_test_list(out.stdout)


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


def run_suite(
    command, chat_url, embed_url, alt_embed_url, keepalive_every, text_only=False, split_model=False
):
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
    # A model with no projector *in existence* declares itself text-only, and the
    # three vision smokes skip instead of failing for a reason that is not about
    # the code. Derived from the deployed model rather than taken as a flag, so it
    # can never disagree with what is actually running; `--no-mmproj` deliberately
    # does not set it (hf_api.text_only, docs/research/e2e-gpt-oss-120b.md fork F3).
    if text_only:
        env["MINDFORK_LIVE_TEXT_ONLY"] = "1"
    else:
        env.pop("MINDFORK_LIVE_TEXT_ONLY", None)
    # ...and, the other way round, a model that *is* split turns on the smoke
    # that would otherwise never meet one. Same principle: derived from what was
    # deployed, so the declaration cannot drift from the stack.
    if split_model:
        env["MINDFORK_LIVE_SPLIT_MODEL"] = "1"
    else:
        env.pop("MINDFORK_LIVE_SPLIT_MODEL", None)
    print("\n=== suite ===", flush=True)
    print(f"  MINDFORK_ENGINE_URL={env['MINDFORK_ENGINE_URL']}")
    print(f"  MINDFORK_EMBED_URL={env.get('MINDFORK_EMBED_URL', '(unset — memory smokes will skip)')}")
    print(f"  MINDFORK_EMBED_URL_ALT={env.get('MINDFORK_EMBED_URL_ALT', '(unset — model-change smokes will skip)')}")
    if text_only:
        print("  MINDFORK_LIVE_TEXT_ONLY=1 (no projector exists for this model: the 3 vision smokes will SKIP)")
    if split_model:
        print("  MINDFORK_LIVE_SPLIT_MODEL=1 (the weights are split across files)")
    print(f"  $ {show_command(command)}\n", flush=True)
    started = time.time()
    with KeepAlive(chat_url, [embed_url, alt_embed_url], every=keepalive_every):
        code = subprocess.run(command, shell=isinstance(command, str), env=env).returncode
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


# Seconds between deleting an endpoint HF could not start and creating it
# again. The deletion is verified first; this is only manners towards the API.
RETRY_PAUSE_S = 20

# The states after which an endpoint is created again: HF could not start it.
# A long schedule is not one of them — `initializing` for ten minutes and more
# was followed by `running` several times on 2026-10-09 — and neither is a
# server that came up and never answered `/health`, which no card would cure.
FAILED_TO_START = ("failed", "updateFailed")


class Rung:
    """One attempt at an endpoint: the card it asks for, where that card lives,
    and its payload."""

    def __init__(self, card, place, make):
        self.card = card
        self.place = place  # (vendor, region)
        self.make = make  # name -> create payload

    def label(self):
        return f"{self.card} @ {'/'.join(self.place)}"


def chat_rungs(args):
    """The cards the chat endpoint is tried on, in order.

    The model's own card, then its record's fallback cards, then its own card
    once more — HF's refusals come in waves, and the wave may have passed. A card
    named by hand (`--chat-instance`) is a measurement of that card: it is tried
    twice and never swapped. A model with no fallback is tried twice on its own.
    """

    def rung_on(card_args):
        def make(name):
            return hf.apply_overrides(hf.chat_payload(name, card_args), args.set)

        return Rung(hf.chat_instance(card_args), hf.chat_place(card_args), make)

    first = rung_on(args)
    fallbacks = [] if args.chat_instance else hf.chat_model(args).get("fallback") or []
    rungs = [first]
    for card in fallbacks:
        moved = argparse.Namespace(**vars(args))
        moved.chat_instance = card
        rungs.append(rung_on(moved))
    return rungs + [first]


def embed_rungs(args, payload):
    """An embedder's attempts: its own card, twice — a T4 has no fallback."""
    place = hf.card_place(args, args.embed_instance)
    first = Rung(args.embed_instance, place, lambda name: hf.apply_overrides(payload(name, args), args.set))
    return [first, first]


def catalogue_warnings(catalogue, rungs, size):
    """What HF's provider catalogue says against the cards a run may ask for:
    one line per card that is not plainly `available` there.

    The H200 this gate ran `gpt-oss-120b` on was already `deprecated` there on
    2026-10-09; the next day its region was `not_available` and a create a 400
    (research e2e-gpt-oss-120b.md §11). The ladder survives one dead card, but
    only a warning says the ladder has started to shrink.
    """
    places = {}
    for vendor in (catalogue or {}).get("vendors") or []:
        for region in vendor.get("regions") or []:
            for compute in region.get("computes") or []:
                key = (vendor.get("name"), region.get("name"), compute.get("instanceType"), compute.get("instanceSize"))
                places[key] = (region.get("status"), compute.get("status"))
    warnings = []
    for label, (vendor, region), card in dict.fromkeys((r.label(), r.place, r.card) for r in rungs):
        found = places.get((vendor, region, card, size))
        if found is None:
            warnings.append(f"{label} {size} is not in HF's catalogue")
        elif found != ("available", "available"):
            warnings.append(f"{label} {size}: region {found[0]}, card {found[1]} in HF's catalogue")
    return warnings


def warn_catalogue(rungs, size):
    """Print `catalogue_warnings` — in CI as annotations, which the run's page
    shows; a catalogue that cannot be read is said, never fatal."""
    status, catalogue = hf.http("GET", hf.PROVIDER_URLS[0])
    if not hf.ok(status) or not isinstance(catalogue, dict):
        print(f"  (HF's catalogue unreadable: HTTP {status}; the cards are not checked)")
        return
    prefix = "::warning::" if os.environ.get("GITHUB_ACTIONS") == "true" else "  WARNING: "
    for line in catalogue_warnings(catalogue, rungs, size):
        print(f"{prefix}{line}", flush=True)


def bring_up(kind, name, args, rungs=None, created=True, pause=RETRY_PAUSE_S):
    """Wait for one endpoint and return `(url, card)` once it answers, or None.

    An endpoint of ours that HF fails to start, or refuses to create (a quota,
    a full region), is deleted, proven gone, and created again under the same
    name on the next rung — the same name, so the runner's cleanup, the
    workflow's `delete-run` and the sweeper all still know it. `created` says
    whether the first rung's create (made by `create_endpoints`, before any
    wait, so the three overlap) went through. A reused endpoint has no rungs:
    it is waited for and nothing more.
    """
    rungs = rungs or [None]
    for i, rung in enumerate(rungs):
        if i > 0:
            print(f"\n  -> {kind}: creating it again on {rung.label()} (attempt {i + 1} of {len(rungs)})", flush=True)
            if not hf.delete_one(name):
                return None
            time.sleep(pause)
            created = hf.create(rung.make(name)) is not None
        if not created:
            continue
        url = hf.wait_running(name, args.timeout)
        if url:
            if not hf.wait_healthy(url, timeout=args.health_timeout):
                print(f"  -> {kind} endpoint never became healthy")
                return None
            return url, rung.card if rung else None
        state = hf.endpoint_state(name)[0]
        if state not in FAILED_TO_START:
            return None
    print(f"  -> {kind}: no card started it in {len(rungs)} attempt(s)")
    return None


def run_names(ident, tag):
    """(chat, embed, alt): the endpoint names a run with this identity creates.

    One function for the create and for every delete after it — the runner's own
    and the workflow's backstop step — so the two cannot drift apart. The model's
    tag goes into the chat name, so a listing, the sweeper's log and an orphan hunt
    all say *which* model the endpoint is holding. Kept short because `safe_name`
    truncates at 32 and the CI run id spends ~14.
    """
    return (
        hf.safe_name(f"e2e-chat-{tag}-{ident}"),
        hf.safe_name(f"e2e-embed-{ident}"),
        hf.safe_name(f"e2e-alt-{ident}"),
    )


def run_names_any_model(ident):
    """Every name the run `ident` could have created, whichever model it rented:
    the backstop is told the run, not the model, and must not miss one."""
    names = []
    for model in hf.CHAT_MODELS.values():
        for name in run_names(ident, model["tag"]):
            if name not in names:
                names.append(name)
    return names


def run_endpoints(ident, listed):
    """Which of the `listed` endpoint names belong to the run `ident`.

    Exact names only, never a prefix or a pattern: a backstop that deletes by
    pattern is one typo away from deleting a GPU another run is using, and run
    ids nest: `37947267239-1` is a prefix of `37947267239-11` and a suffix of
    `137947267239-1`.
    """
    ours = set(run_names_any_model(ident))
    return [name for name in listed if name in ours]


class Plan:
    """The run's endpoint names and which of them are wanted."""

    def __init__(self, args):
        self.shard = getattr(args, "shard", None)
        self.ident = run_ident(args.run_id or run_id(), self.shard)
        self.model = hf.chat_model(args)
        chat, embed, alt = run_names(self.ident, self.model["tag"])
        self.chat_name = args.reuse_chat or chat
        self.embed_name = args.reuse_embed or embed
        self.alt_name = args.reuse_alt_embed or alt
        self.want_embed = not args.no_embed
        # The alternate embedder is a *second* model, so it needs the first one
        # to compare against — every smoke that reads MINDFORK_EMBED_URL_ALT
        # also reads MINDFORK_EMBED_URL.
        self.want_alt = self.want_embed and not args.no_alt_embed


def print_plan(plan, args):
    """Printed before anything is created: the trail an orphan is found by."""
    print(f"\nrun id: {plan.ident}")
    print(f"  chat  model:    {args.chat_model}  ({plan.model['repo']})")
    print(f"                  weights {args.gguf or plan.model['gguf']}")
    # ASCII on purpose: a piped stdout on Windows is cp1252, and `init` only
    # keeps a stray character from killing a run that is holding a GPU -- it does
    # not make the log readable.
    if args.no_mmproj:
        projector = "(none - asked for; the 3 vision smokes will FAIL)"
    elif hf.text_only(args):
        projector = "(none exists for this model; the 3 vision smokes will SKIP)"
    else:
        projector = args.mmproj or plan.model["mmproj"]
    print(f"                  mmproj  {projector}")
    variant = args.variant or plan.model.get("variant")
    if variant:
        # Which .gguf files the endpoint pulls at all. On a repository that holds
        # every quantization it is the difference between 63 GB and 1010 GB.
        print(f"                  variant {variant}")
    print(f"  chat  hardware: {hf.chat_instance(args)} {args.instance_size} @ {'/'.join(hf.chat_place(args))}")
    if not args.reuse_chat:
        # The trail an orphan hunt needs: which cards the name may be found on.
        print(f"  chat  attempts: {' -> '.join(r.label() for r in chat_rungs(args))}  (when one fails to start)")
    print(f"  chat  endpoint: {plan.chat_name}{'  (reused)' if args.reuse_chat else ''}")
    if plan.want_embed:
        print(f"  embed endpoint: {plan.embed_name}{'  (reused)' if args.reuse_embed else ''}")
    if plan.want_alt:
        print(f"  alt   endpoint: {plan.alt_name}{'  (reused)' if args.reuse_alt_embed else ''}")
    if sharded(plan.shard):
        print(f"  shard:          {plan.shard[0]}/{plan.shard[1]} of the smokes `--filter` selects")
    print(f"  command: {show_plan_command(plan, args)}", flush=True)


def show_plan_command(plan, args):
    if sharded(plan.shard) and not args.command:
        return f"cargo test -- {args.test_args} --exact <this shard's names, listed after the build>"
    return cargo_command(args)


def dry_run(plan, args):
    hf.create(hf.apply_overrides(hf.chat_payload(plan.chat_name, args), args.set), True)
    if plan.want_embed:
        hf.create(hf.apply_overrides(hf.embed_payload(plan.embed_name, args), args.set), True)
    if plan.want_alt:
        hf.create(hf.apply_overrides(alt_payload(plan.alt_name, args), args.set), True)
    print(f"\n--dry-run: nothing created. Would run:\n  $ {show_plan_command(plan, args)}")
    return hf.EXIT_OK


def endpoint_specs(plan, args):
    """(kind, name, rungs) for each endpoint the run wants; `rungs` is None for a
    reused one, which is neither created nor created again."""
    specs = [("chat", plan.chat_name, None if args.reuse_chat else chat_rungs(args))]
    if plan.want_embed:
        rungs = None if args.reuse_embed else embed_rungs(args, hf.embed_payload)
        specs.append(("embed", plan.embed_name, rungs))
    if plan.want_alt:
        rungs = None if args.reuse_alt_embed else embed_rungs(args, alt_payload)
        specs.append(("alt embed", plan.alt_name, rungs))
    return specs


def create_endpoints(specs):
    """Create every endpoint's first rung before waiting for any: deploy is ~21 s
    (chat) and ~84 s (embed), and they overlap. Returns, per name, whether the
    create went through — a refused one is not the end of the run, it is the
    first rung of its ladder spent (see `bring_up`)."""
    return {
        name: rungs is None or hf.create(rungs[0].make(name)) is not None
        for _kind, name, rungs in specs
    }


def bring_up_all(specs, created, args):
    """{kind: (url, card)} for every endpoint, or None when one could not be had."""
    up = {}
    for kind, name, rungs in specs:
        got = bring_up(kind, name, args, rungs, created[name])
        if got is None:
            return None
        up[kind] = got
    return up


def choose_shard(args, shard):
    """This shard's smokes, printed — the log is the only place that says which
    shard ran what — or None when the listing failed."""
    listed = list_tests(args)
    if listed is None:
        return None
    names = shard_of(listed, shard)
    print(f"  {len(listed)} smoke(s) listed; shard {shard[0]}/{shard[1]} runs {len(names)}:", flush=True)
    for name in names:
        print(f"    {name}")
    return names


def cmd_run(args):
    started = time.time()
    plan = Plan(args)
    print_plan(plan, args)
    specs = endpoint_specs(plan, args)
    warn_catalogue([r for _kind, _name, rungs in specs for r in rungs or []], args.instance_size)

    if args.dry_run:
        return dry_run(plan, args)

    if not args.no_prebuild and prebuild(args) != 0:
        print("\nprebuild failed — no endpoint was created, nothing was billed.")
        return hf.EXIT_FAILED

    names = None
    if sharded(plan.shard) and not args.command:
        names = choose_shard(args, plan.shard)
        if names is None:
            print("\nthe test list failed — no endpoint was created, nothing was billed.")
            return hf.EXIT_FAILED
        if not names:
            print(f"\nshard {plan.shard[0]}/{plan.shard[1]} holds no smokes — nothing to rent.")
            return hf.EXIT_OK

    up = bring_up_all(specs, create_endpoints(specs), args)
    if up is None:
        return hf.EXIT_FAILED
    chat_url, chat_card = up["chat"]
    embed_url = up.get("embed", (None, None))[0]
    alt_embed_url = up.get("alt embed", (None, None))[0]
    ready = time.time()
    print(f"\n  endpoints ready after {ready - started:.0f}s", flush=True)

    code, suite_time = run_suite(
        cargo_command(args, names),
        chat_url,
        embed_url,
        alt_embed_url,
        args.keepalive_seconds,
        text_only=hf.text_only(args),
        split_model=hf.split_model(args),
    )

    print("\n=== summary ===")
    on = f"  on {chat_card} @ {'/'.join(hf.card_place(args, chat_card))}" if chat_card else ""
    moved = "  (a fallback: the model's own card did not start)" if chat_card and chat_card != hf.chat_instance(args) else ""
    print(f"  chat  {plan.chat_name}  {chat_url}{on}{moved}")
    print(f"  embed {plan.embed_name}  {embed_url or '(none)'}")
    print(f"  alt   {plan.alt_name}  {alt_embed_url or '(none)'}")
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


def sweep_verdict(item, prefix, now, limit):
    """(verdict, name, state, detail) for one endpoint.

    Verdicts: "skip" (not ours), "unknown" (unreadable createdAt — deliberately
    fail-safe towards *keeping* it: deleting an endpoint whose age we cannot
    establish could kill a run that is still using it, destroying real work for
    a false red, whereas a leak is already money-bounded by scale-to-zero),
    "keep" (young), "delete" (past the limit). `detail` is the raw createdAt
    for "unknown" and the age in minutes for "keep"/"delete".
    """
    name = item.get("name", "")
    st = item.get("status") or {}
    state = st.get("state", "?")
    if not name.startswith(prefix):
        return "skip", name, state, None
    created = parse_time(st.get("createdAt"))
    if created is None:
        return "unknown", name, state, st.get("createdAt")
    age = now - created
    mins = age.total_seconds() / 60
    return ("keep" if age < limit else "delete"), name, state, mins


def cmd_sweep(args):
    """Delete orphaned `e2e-*` endpoints older than the age limit.

    This catches what neither the runner's own cleanup nor the workflow's
    `delete-run` step could: an endpoint left while the API was unreachable, a
    runner machine lost with its job, a local run killed outright. It also
    reclaims endpoint *quota*, which scale-to-zero does not
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
        verdict, name, state, detail = sweep_verdict(item, args.prefix, now, limit)
        if verdict == "skip":
            print(f"  skip  {name:<34} {state:<14} (not {args.prefix}*)")
        elif verdict == "unknown":
            unknown.append(name)
            print(f"  KEEP  {name:<34} {state:<14} !! unreadable createdAt {detail!r}")
        elif verdict == "keep":
            print(f"  keep  {name:<34} {state:<14} {detail:.0f} min old")
        else:
            print(f"  DELETE {name:<33} {state:<14} {detail:.0f} min old", flush=True)
            doomed.append(name)
            if not args.dry_run and not hf.delete_one(name):
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


def cmd_delete_run(args):
    """Delete what the run `--run-id` created, by name — the workflow's backstop.

    The runner deletes its own endpoints from `finally`, `atexit` and its signal
    handler, but only while it is alive to. CI run 37947267239 hit the job's
    timeout, and the runner never heard of it: GitHub signals the step's shell,
    not the shell's children, so the Python process was killed as an orphan at
    the end of the job with all three endpoints still running. This runs as a
    step of its own, `if: always()`, and needs nothing from the step before it
    but the run id: the names are derived from it exactly as the create derived
    them (`run_names`), and only those names are touched.

    The deletion is the runner's own: the names are adopted into `hf.CREATED`
    and `hf.cleanup` sends every DELETE before verifying any, and exits 3 if one
    is not proven gone. Then it lists what is left in the namespace — the
    evidence the step used to be limited to.
    """
    if not args.run_id.strip():
        hf.die(hf.EXIT_USAGE, "delete-run needs a non-empty --run-id")
    # A shard's backstop deletes that shard's endpoints and no other's: its
    # siblings may still be running their suites.
    ident = run_ident(args.run_id.strip(), args.shard)
    items = hf.list_endpoints()
    if items is None:
        # Blind, then: ask for every name the run could have used. A DELETE of a
        # name that does not exist is a 404, and a 404 is the proof we want.
        print("  cannot list the endpoints — deleting every name this run could have used")
        doomed = run_names_any_model(ident)
    else:
        doomed = run_endpoints(ident, [item.get("name", "") for item in items])
    print(f"delete-run {ident}: {len(doomed)} endpoint(s) of this run still exist", flush=True)
    hf.CREATED.extend(doomed)
    hf.cleanup()
    print("\nleft in the namespace:")
    return hf.cmd_list(args)


# --------------------------------------------------------------------------
# Self-test: the arms that decide without the network
# --------------------------------------------------------------------------
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIVE_WORKFLOW = os.path.join(ROOT, ".github", "workflows", "e2e-live.yml")
SWEEPER_WORKFLOW = os.path.join(ROOT, ".github", "workflows", "e2e-sweeper.yml")


def _name_failures(name, ident):
    """What is wrong with one endpoint name made for the run `ident`."""
    out = []
    # Truncation would cut the run id off the end, and the id is what an orphan
    # is identified by — in the log and by `delete-run`.
    if not name.endswith(ident) or len(name) > hf.NAME_MAX:
        out.append(f"names: {name!r} does not end in {ident!r} within {hf.NAME_MAX}")
    if hf.safe_name(name) != name or not name.startswith(SWEEP_PREFIX):
        out.append(f"names: {name!r} is not a stable {SWEEP_PREFIX}* name")
    return out


def _check_names(failures):
    # A CI id with a two-digit attempt and the local timestamp form, each also as
    # the highest shard: the longest names the gate can make.
    top = (SHARD_MAX, SHARD_MAX)
    for ident in ("99999999999-12", "1009-143015", run_ident("99999999999-12", top), run_ident("1009-143015", top)):
        for key, model in hf.CHAT_MODELS.items():
            names = run_names(ident, model["tag"])
            if len(set(names)) != 3:
                failures.append(f"names: {key} {ident} gives duplicates {names}")
            for name in names:
                failures.extend(_name_failures(name, ident))


def _check_run_endpoints(failures):
    ident = "37947267239-1"
    listed = [
        "e2e-chat-gemma-37947267239-1",
        "e2e-embed-37947267239-1",
        "e2e-alt-37947267239-1",
        "e2e-chat-oss-37947267239-1",  # the same run under another model is still ours
        "e2e-embed-37947267239-11",  # a later attempt
        "e2e-embed-137947267239-1",  # another run whose id ends in ours
        "e2e-chat-qwen-37947267239-2",
        "e2e-alt-37947267239-1-old",
        "my-own-endpoint",
    ]
    got = run_endpoints(ident, listed)
    want = listed[:4]
    if got != want:
        failures.append(f"delete-run: chose {got}, want {want}")
    if run_endpoints(ident, ["e2e-embed-37947267239", "e2e-chat-gemma-7947267239-1"]):
        failures.append("delete-run: chose a name that only resembles the run's")
    # A shard's backstop runs while its siblings may still be using theirs.
    family = [run_names(run_ident(ident, s), "gemma") for s in (None, (1, 3), (2, 3), (3, 3))]
    listed = [name for names in family for name in names]
    got = run_endpoints(run_ident(ident, (2, 3)), listed)
    if got != list(family[2]):
        failures.append(f"delete-run --shard 2/3: chose {got}, want {list(family[2])}")


def _check_shard_parsing(failures):
    for text, want in (("2/3", (2, 3)), (" 1/1 ", (1, 1)), (f"{SHARD_MAX}/{SHARD_MAX}", (SHARD_MAX, SHARD_MAX))):
        try:
            if parse_shard(text) != want:
                failures.append(f"shard: {text!r} -> {parse_shard(text)}, want {want}")
        except argparse.ArgumentTypeError as e:
            failures.append(f"shard: {text!r} refused: {e}")
    for text in ("0/3", "4/3", "3", f"1/{SHARD_MAX + 1}", "a/b", "2/3/4", "-1/3"):
        try:
            parse_shard(text)
            failures.append(f"shard: {text!r} accepted")
        except argparse.ArgumentTypeError:
            pass
    if run_ident("r", None) != "r" or run_ident("r", (1, 1)) != "r" or run_ident("r", (2, 3)) != "r-s2":
        failures.append("shard: 1/1 must keep the unsharded names, 2/3 must add -s2")


def _check_shard_deal(failures):
    # Every smoke in exactly one shard, the shards within one of each other in
    # size, each in libtest's order — for every N, over a list that does not
    # divide evenly.
    names = [f"m{i // 70}::t{i:03}" for i in range(250)]
    for n in range(1, SHARD_MAX + 1):
        parts = [shard_of(names, (i, n)) for i in range(1, n + 1)]
        flat = sorted(x for part in parts for x in part)
        sizes = [len(part) for part in parts]
        if flat != names or max(sizes) - min(sizes) > 1 or any(part != sorted(part) for part in parts):
            failures.append(f"shard: N={n} does not deal {len(names)} smokes once each, evenly, in order")
    listing = "mod::smoke_one: test\nall_the_rest: test\nmod::bench_x: benchmark\n\n2 tests, 1 benchmark\n"
    if parse_test_list(listing) != ["mod::smoke_one", "all_the_rest"]:
        failures.append(f"shard: the --list parse gave {parse_test_list(listing)}")
    command = cargo_command(argparse.Namespace(command="", test_args=DEFAULT_TEST_ARGS, filter="x"), ["mod::one", "two"])
    want = ["cargo", "test", "--", "--ignored", "--nocapture", "--test-threads=1", "--exact", "mod::one", "two"]
    if command != want:
        failures.append(f"shard: the suite's argv is {command}, want {want}")


def _check_embed_batch(failures):
    """Both embedders take a chunk as long as their context in one batch: the
    image's default of 512 left spec.md unindexed on every full run."""
    parser = argparse.ArgumentParser()
    hf.add_endpoint_args(parser)
    args = parser.parse_args([])
    for payload in (hf.embed_payload("e2e-embed-x", args), alt_payload("e2e-alt-x", args)):
        ctx = str(payload["model"]["image"]["llamacpp"]["ctxSize"])
        env = payload["model"].get("env") or {}
        if env.get("LLAMA_ARG_UBATCH") != ctx or env.get("LLAMA_ARG_BATCH") != ctx:
            failures.append(f"embed: {payload['model']['repository']} batches {env}, want the context {ctx}")


def _run_args(*argv):
    return build_parser().parse_args(["run", *argv])


def _check_rungs(failures):
    """Which cards each endpoint is tried on, and that every rung's payload
    really asks for its card — in the place that card lives in."""
    rtx, a100, h200, l40s = "nvidia-rtx-pro-6000", "nvidia-a100", "nvidia-h200", "nvidia-l40s"
    east1, east2, gcp = ("aws", "us-east-1"), ("aws", "us-east-2"), ("gcp", "us-south1")
    cases = [
        ((), [(l40s, east1), (a100, east1), (l40s, east1)]),
        (("--chat-model", "qwen-3.6-27b"), [(l40s, east1), (a100, east1), (l40s, east1)]),
        (("--chat-model", "gpt-oss-120b"), [(rtx, east2), (a100, east1), (h200, gcp), (rtx, east2)]),
        (("--chat-instance", a100), [(a100, east1), (a100, east1)]),
        (("--chat-model", "gpt-oss-120b", "--chat-instance", h200), [(h200, gcp), (h200, gcp)]),
        # A place named by hand is a measurement of that place: every rung keeps it.
        (("--region", "us-east-1"), [(l40s, east1), (a100, east1), (l40s, east1)]),
        (("--chat-model", "gpt-oss-120b", "--vendor", "aws", "--region", "us-east-1"), [(rtx, east1), (a100, east1), (h200, east1), (rtx, east1)]),
    ]
    for argv, want in cases:
        args = _run_args(*argv)
        rungs = chat_rungs(args)
        if [(r.card, r.place) for r in rungs] != want:
            failures.append(f"rungs: {argv or 'default'} -> {[r.label() for r in rungs]}, want {want}")
            continue
        for rung in rungs:
            payload = rung.make("e2e-chat-x")
            asked = (payload["compute"]["instanceType"], (payload["provider"]["vendor"], payload["provider"]["region"]))
            if asked != (rung.card, rung.place):
                failures.append(f"rungs: {argv or 'default'} rung {rung.label()} asks for {asked}")
    # The embedders stay on their T4 in aws us-east-1 whichever model the chat is.
    for argv in ((), ("--chat-model", "gpt-oss-120b")):
        args = _run_args(*argv)
        embed = [(r.card, r.place) for r in embed_rungs(args, hf.embed_payload)]
        payload = hf.embed_payload("e2e-embed-x", args)
        asked = (payload["compute"]["instanceType"], (payload["provider"]["vendor"], payload["provider"]["region"]))
        if embed != [("nvidia-t4", east1)] * 2 or asked != ("nvidia-t4", east1):
            failures.append(f"rungs: {argv or 'default'} embedder tried on {embed}, its payload asks for {asked}")


def _catalogue(*computes):
    """A provider catalogue in HF's shape: (vendor, region, region status,
    instanceType, compute status) per x1 compute."""
    vendors = {}
    for vendor, region, region_status, card, status in computes:
        regions = vendors.setdefault(vendor, {})
        entry = regions.setdefault(region, {"name": region, "status": region_status, "computes": []})
        entry["computes"].append({"instanceType": card, "instanceSize": "x1", "status": status})
    return {"vendors": [{"name": v, "regions": list(r.values())} for v, r in vendors.items()]}


def _check_catalogue(failures):
    """A card the ladder may ask for is warned about unless the catalogue lists
    it, in its place, plainly available — once per card, not once per rung."""
    rungs = chat_rungs(_run_args("--chat-model", "gpt-oss-120b"))
    healthy = _catalogue(
        ("aws", "us-east-2", "available", "nvidia-rtx-pro-6000", "available"),
        ("aws", "us-east-1", "available", "nvidia-a100", "available"),
        ("gcp", "us-south1", "available", "nvidia-h200", "available"),
    )
    if catalogue_warnings(healthy, rungs, "x1"):
        failures.append(f"catalogue: warned on a healthy one: {catalogue_warnings(healthy, rungs, 'x1')}")
    if len(catalogue_warnings(healthy, rungs, "x2")) != 3:
        failures.append("catalogue: a size the catalogue does not list is not warned about")
    sick = _catalogue(
        ("aws", "us-east-2", "available", "nvidia-rtx-pro-6000", "deprecated"),
        ("aws", "us-east-1", "not_available", "nvidia-a100", "available"),
        # The H200 in the place it *used* to be: not where the ladder asks for it.
        ("aws", "us-west-2", "available", "nvidia-h200", "available"),
    )
    got = catalogue_warnings(sick, rungs, "x1")
    want = ["nvidia-rtx-pro-6000 @ aws/us-east-2", "nvidia-a100 @ aws/us-east-1", "nvidia-h200 @ gcp/us-south1"]
    if [w.split(" x1")[0] for w in got] != want:
        failures.append(f"catalogue: {got}, want one warning each for {want}")


class _FakeHf:
    """The calls `bring_up` makes, scripted: `states` is what each wait sees
    (a URL, or the state the endpoint is left in), `healthy` what /health says."""

    def __init__(self, states, healthy=True, creates=True, deletes=True):
        self.states = list(states)
        self.healthy = healthy
        self.creates = creates
        self.deletes = deletes
        self.calls = []
        self.left = None

    def create(self, payload, dry_run=False):
        self.calls.append(("create", payload["compute"]["instanceType"]))
        return {} if self.creates else None

    def delete_one(self, name):
        self.calls.append(("delete", name))
        return self.deletes

    def wait_running(self, name, timeout, poll=10):
        # Past the script, still starting: a wait nobody planned for shows up as
        # a wrong result, not as a crash of the self-test.
        state = self.states.pop(0) if self.states else "initializing"
        if state.startswith("https://"):
            return state
        self.left = state
        return None

    def endpoint_state(self, name):
        return self.left, None, ""

    def wait_healthy(self, url, timeout=900, poll=5):
        return self.healthy


def _bring_up_with(fake, created=True):
    """`bring_up` of the default chat ladder against `fake`, quietly."""
    names = ("create", "delete_one", "wait_running", "endpoint_state", "wait_healthy")
    saved = {n: getattr(hf, n) for n in names}
    stdout, sys.stdout = sys.stdout, open(os.devnull, "w", encoding="utf-8")
    try:
        for n in names:
            setattr(hf, n, getattr(fake, n))
        args = _run_args()
        return bring_up("chat", "e2e-chat-x", args, chat_rungs(args), created, pause=0)
    finally:
        sys.stdout.close()
        sys.stdout = stdout
        for n, f in saved.items():
            setattr(hf, n, f)


def _check_bring_up(failures):
    """The ladder walked: what is retried, on which card, and what is not."""
    url = "https://e2e.example"
    cases = [
        # (what, fake, created, want result, want calls)
        ("started at once", _FakeHf([url]), True, (url, "nvidia-l40s"), []),
        (
            "failed to start -> the fallback card",
            _FakeHf(["failed", url]),
            True,
            (url, "nvidia-a100"),
            [("delete", "e2e-chat-x"), ("create", "nvidia-a100")],
        ),
        (
            "refused at create -> the fallback card",
            _FakeHf([url]),
            False,
            (url, "nvidia-a100"),
            [("delete", "e2e-chat-x"), ("create", "nvidia-a100")],
        ),
        ("a long schedule is not retried", _FakeHf(["initializing"]), True, None, []),
        ("a server that never answers is not retried", _FakeHf([url], healthy=False), True, None, []),
        (
            "every rung failed",
            _FakeHf(["failed", "updateFailed", "failed"]),
            True,
            None,
            [("delete", "e2e-chat-x"), ("create", "nvidia-a100"), ("delete", "e2e-chat-x"), ("create", "nvidia-l40s")],
        ),
        (
            "a name not proven gone is not created again",
            _FakeHf(["failed"], deletes=False),
            True,
            None,
            [("delete", "e2e-chat-x")],
        ),
    ]
    for what, fake, created, want, calls in cases:
        got = _bring_up_with(fake, created)
        if got != want or fake.calls != calls:
            failures.append(f"bring_up: {what}: got {got} after {fake.calls}, want {want} after {calls}")


def _check_sweep(failures):
    now = dt.datetime(2026, 10, 9, 18, 17, tzinfo=dt.timezone.utc)
    limit = dt.timedelta(minutes=SWEEP_MAX_AGE_MIN)
    cases = [
        ({"name": "my-own", "status": {"createdAt": "2026-10-01T00:00:00Z"}}, "skip"),
        ({"name": "e2e-x", "status": {"createdAt": "yesterday"}}, "unknown"),
        ({"name": "e2e-x", "status": {}}, "unknown"),
        ({"name": "e2e-x", "status": {"createdAt": "2026-10-09T17:47:00.123Z"}}, "keep"),
        # Nine fractional digits and an explicit offset: the parser once ate the
        # offset's digits along with the fraction (docs/lessons.md).
        ({"name": "e2e-x", "status": {"createdAt": "2026-10-09T14:54:24.682123456+00:00"}}, "delete"),
        ({"name": "e2e-x", "status": {"createdAt": "2026-10-09T16:30:00Z"}}, "delete"),
    ]
    for item, want in cases:
        got = sweep_verdict(item, SWEEP_PREFIX, now, limit)[0]
        if got != want:
            failures.append(f"sweep: {item} -> {got}, want {want}")


def _read(path, failures):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        failures.append(f"workflow: cannot read {path}: {e}")
        return ""


def _check_workflows(failures):
    """The two facts the gate's safety rests on live in YAML, out of reach of
    any test: they are checked here, as text."""
    live = _read(LIVE_WORKFLOW, failures)
    sweeper = _read(SWEEPER_WORKFLOW, failures)
    # The job's ceiling must stay under the sweeper's threshold, or the sweeper
    # could delete the endpoints of a run that is still using them.
    ceilings = [int(m) for m in re.findall(r"^ {4}timeout-minutes: (\d+)", live, re.M)]
    if not ceilings or max(ceilings) >= SWEEP_MAX_AGE_MIN:
        failures.append(f"workflow: job timeout {ceilings} is not below the sweep age {SWEEP_MAX_AGE_MIN}")
    if f'default: "{SWEEP_MAX_AGE_MIN}"' not in sweeper or f"${{MAX_AGE:-{SWEEP_MAX_AGE_MIN}}}" not in sweeper:
        failures.append(f"workflow: the sweeper's default age is not {SWEEP_MAX_AGE_MIN}")
    # `exec`, or the cancellation signals the shell and the runner never hears
    # of it (CI run 37947267239).
    if not re.search(r'^\s*exec python3 tools/e2e_hf\.py "\$\{args\[@\]\}"\s*$', live, re.M):
        failures.append("workflow: the runner is not exec'd, so a cancellation never reaches it")
    steps = re.split(r"^ {6}- ", live, flags=re.M)
    backstop = [s for s in steps if "tools/e2e_hf.py delete-run" in s]
    if len(backstop) != 1 or "if: always()" not in backstop[0]:
        failures.append("workflow: no `if: always()` step runs `delete-run`")
    # The shards: the run and its backstop must name the same shard, or the
    # backstop derives the unsharded names and deletes nothing — in silence.
    shard_arg = '--shard "${SHARD}/${SHARDS}"'
    runner = [s for s in steps if "exec python3 tools/e2e_hf.py" in s]
    if not runner or shard_arg not in runner[0] or not backstop or shard_arg not in backstop[0]:
        failures.append(f"workflow: the run and its backstop do not both pass {shard_arg}")
    # One shard's red must not cancel the others: they hold endpoints of their own.
    if "fail-fast: false" not in live:
        failures.append("workflow: the shard matrix does not set fail-fast: false")
    # Every card a model may run on can be named by hand, to measure it alone.
    offered = re.search(r"^ {6}chat_instance:.*?^ {8}options: \[([^\]]*)\]", live, re.M | re.S)
    offered = {c.strip() for c in offered.group(1).split(",")} if offered else set()
    cards = {c for m in hf.CHAT_MODELS.values() for c in [m["instance"], *(m.get("fallback") or [])]}
    if not cards <= offered:
        failures.append(f"workflow: chat_instance does not offer {sorted(cards - offered)}")


def self_test():
    """Drive the arms that decide offline, so a dispatch never discovers one with
    a GPU billing."""
    failures = []
    _check_names(failures)
    _check_run_endpoints(failures)
    _check_shard_parsing(failures)
    _check_shard_deal(failures)
    _check_embed_batch(failures)
    _check_rungs(failures)
    _check_catalogue(failures)
    _check_bring_up(failures)
    _check_sweep(failures)
    _check_workflows(failures)
    for line in failures:
        print(f"self-test: {line}", file=sys.stderr)
    print(f"e2e_hf --self-test: {len(failures)} failure(s)")
    return 1 if failures else 0


# --------------------------------------------------------------------------
def main():
    # Before the parser and `hf.init`: the self-test needs neither a token nor
    # the network, which is what lets CI's lint job run it.
    if sys.argv[1:] == ["--self-test"]:
        return self_test()
    args = build_parser().parse_args()
    hf.init(args.namespace, keep=getattr(args, "keep", False))
    try:
        return args.fn(args)
    finally:
        hf.cleanup()


def build_parser():
    """The command line — one place, so the self-test parses what a run parses."""
    parser = hf.run_parser(__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("run", help="create the endpoints, run the suite, delete them")
    hf.add_endpoint_args(p)
    p.add_argument("--run-id", default="", help="identity in the endpoint names (default: CI run id or a timestamp)")
    p.add_argument("--filter", default="", help="cargo test name filter, e.g. e2e_live")
    p.add_argument("--test-args", default=DEFAULT_TEST_ARGS, help="args after `--` (single-threaded is required)")
    p.add_argument("--command", default="", help="run this instead of the composed cargo command")
    p.add_argument("--no-prebuild", action="store_true", help="do not `cargo test --no-run` before deploying")
    p.add_argument(
        "--shard",
        type=parse_shard,
        default=None,
        metavar="I/N",
        help="run every N-th smoke from the I-th, on endpoints of this shard's own (1/1 = all)",
    )
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

    p = sub.add_parser("delete-run", help="delete the endpoints one run created, by their names")
    p.add_argument("--run-id", required=True, help="the run's identity, as `run --run-id` was given it")
    p.add_argument("--shard", type=parse_shard, default=None, metavar="I/N", help="as `run --shard` was given it")
    p.set_defaults(fn=cmd_delete_run)

    hf.add_shared_commands(sub)
    return parser


if __name__ == "__main__":
    sys.exit(main())
