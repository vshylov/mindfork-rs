#!/usr/bin/env python3
"""Probe HF Inference Endpoints as a host for the live e2e smokes (stage 0).

Plan: docs/history/remote-e2e-hf.md §3. Research: docs/research/remote-e2e-gpu.md.
The client (HTTP, payloads, lifecycle, cleanup) lives in tools/hf_api.py, shared
with the stage-2 runner tools/e2e_hf.py — one implementation of "create, wait,
delete, verify deleted", not two.

This script answered, against a real throwaway endpoint, the questions that are
not documented anywhere reachable — above all **U1: how to select which `.gguf`
file the llama.cpp engine serves**, because the target repository holds two
files (the 17.65 GB model and an unwanted 1.2 GB mmproj). All of U1–U7 are
answered (docs/history/remote-e2e-hf.md §3); it is kept because the answers can change
and because `doctor` and `hardware` remain the fastest way to diagnose a 403 or
find an instance-type string.

**For actually running the suite, use `tools/e2e_hf.py`.**

Usage:
    set HF_TOKEN=hf_...                      # fine-grained, Inference Endpoints
    python tools/hf_probe.py doctor          # diagnose a 401/403
    python tools/hf_probe.py hardware        # U7: vendors/regions/instance types
    python tools/hf_probe.py run             # the full probe (creates + deletes)
    python tools/hf_probe.py run --dry-run   # print payloads, send nothing
    python tools/hf_probe.py inspect <name>  # read what the UI built
    python tools/hf_probe.py list
    python tools/hf_probe.py delete <name>

Exit codes: 0 ok · 1 usage/config · 2 a probe check failed · 3 CLEANUP FAILED.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import hf_api as hf  # noqa: E402

# The two checkboxes under User Permissions -> Inference in the fine-grained
# token editor (https://huggingface.co/settings/tokens). "Manage" covers
# create/list/delete; "Make calls" is what an `authenticated` endpoint checks on
# every inference request — the probe needs both, and so does the CI job.
# Wire names observed on a working token (2026-07-28); the UI label is what the
# user has to tick. Both are required: `write` alone creates an endpoint that
# then 403s on every request — i.e. it fails *after* the meter has started.
NEEDED_PERMISSIONS = [
    # (exact wire name, distinctive fragment, UI label, what it buys)
    # The fragments must not cross-match: "endpoints.write" is absent from
    # "inference.endpoints.infer.write", so holding only `infer` cannot be
    # mistaken for holding `manage`.
    (
        "inference.endpoints.write",
        "endpoints.write",
        "Manage Inference Endpoints",
        "create / list / delete via api.endpoints.huggingface.cloud",
    ),
    (
        "inference.endpoints.infer.write",
        "endpoints.infer",
        "Make calls to Inference Endpoints",
        "POST /v1/chat/completions against an authenticated endpoint",
    ),
]


def held_permissions(granted):
    """Which of NEEDED_PERMISSIONS the token has. Returns [(label, what, held, wire)]."""
    out = []
    for wire, frag, label, what in NEEDED_PERMISSIONS:
        held = wire in granted or any(frag in p for p in granted)
        out.append((label, what, held, wire))
    return out


def _granted_permissions(body):
    """(role, granted permission names) from a whoami response."""
    auth = (body.get("auth") or {}).get("accessToken") or {}
    fine = auth.get("fineGrained") or {}
    granted = list(fine.get("global") or [])
    for scope in fine.get("scoped") or []:
        granted += list(scope.get("permissions") or [])
    return auth.get("role", "?"), granted


def _report_fine_grained(granted):
    """Print the held/missing table; returns the missing [(label, what)]."""
    missing = []
    print()
    for label, what, held, wire in held_permissions(granted):
        print(f"  [{'ok     ' if held else 'MISSING'}] {label:<34} ({wire})")
        if not held:
            missing.append((label, what))
    if missing:
        print("\n  -> tick these in https://huggingface.co/settings/tokens -> your token")
        print("     -> Edit -> User Permissions -> Inference (or use the `Inference` preset):")
        for label, what in missing:
            print(f"       [x] {label:<34} ({what})")
    return missing


def _print_403_advice(ns):
    print("\n  -> 403 from the management API. In order of likelihood:")
    print("     1. the token lacks 'Manage Inference Endpoints' (see above);")
    print("     2. no payment method on the account that owns the namespace —")
    print("        Inference Endpoints requires a card on file, and Hub credits")
    print("        do not substitute for one. Check https://huggingface.co/settings/billing;")
    print(f"     3. `{ns}` is an org and the token is not approved for it")
    print("        (org policies can hold a fine-grained token in Pending).")


def cmd_doctor(args):
    """Pinpoint a 403: token role, granted permissions, and the management API."""
    print("\n=== 1. whoami ===")
    status, body = hf.http("GET", hf.WHOAMI)
    hf.show(status, body if not isinstance(body, dict) else {k: v for k, v in body.items() if k != "orgs"}, limit=3000)
    if status in (401, 403):
        print("  -> the token itself is rejected: wrong value, deleted, or revoked.")
        return hf.EXIT_FAILED
    if not hf.ok(status):
        return hf.EXIT_FAILED

    role, granted = _granted_permissions(body)
    print(f"\n  user: {body.get('name')} ({body.get('type')})   token role: {role}")
    print(f"  granted permissions: {granted or '(none reported)'}")

    missing = []
    if role == "fineGrained":
        missing = _report_fine_grained(granted)
    elif role in ("read",):
        print("  -> a `read` token cannot manage endpoints. Use fine-grained (preferred) or `write`.")

    print("\n=== 2. management API ===")
    ns = args.namespace or body.get("name")
    status, listing = hf.http("GET", f"{hf.API}/{ns}")
    hf.show(status, listing, limit=1500)
    if status == 403:
        _print_403_advice(ns)
        return hf.EXIT_FAILED
    if status == 401:
        print("  -> 401: the token is not being accepted at all by this API.")
        return hf.EXIT_FAILED
    if hf.ok(status):
        if missing:
            print("  -> management API OK, but a permission above is missing: `run` will")
            print("     create the endpoint and then 403 on the inference calls.")
        else:
            print("  -> management API OK and both permissions present. Ready for `run`.")
    return hf.EXIT_OK if hf.ok(status) and not missing else hf.EXIT_FAILED


# --------------------------------------------------------------------------
# Checks (U1-U6)
# --------------------------------------------------------------------------
class Checks:
    def __init__(self):
        self.results = []

    def record(self, uid, name, passed, detail=""):
        self.results.append((uid, name, passed, detail))
        print(f"  [{'PASS' if passed else 'FAIL'}] {uid} {name}{': ' + detail if detail else ''}")

    def failed(self):
        return [r for r in self.results if not r[2]]

    def summary(self):
        print("\n=== summary ===")
        for uid, name, passed, detail in self.results:
            print(f"  {'PASS' if passed else 'FAIL'}  {uid:<3} {name}{': ' + detail if detail else ''}")


def run_suite(command, chat_url, embed_url):
    """Run a command with the endpoint env set, then let cleanup happen."""
    env = dict(os.environ)
    env["MINDFORK_ENGINE_URL"] = f"{chat_url}/v1"
    env["MINDFORK_ENGINE_KEY"] = hf.TOKEN
    if embed_url:
        env["MINDFORK_EMBED_URL"] = f"{embed_url}/v1"
        env["MINDFORK_EMBED_KEY"] = hf.TOKEN
    print("\n=== suite ===")
    print(f"  MINDFORK_ENGINE_URL={env['MINDFORK_ENGINE_URL']}")
    if embed_url:
        print(f"  MINDFORK_EMBED_URL={env['MINDFORK_EMBED_URL']}")
    print(f"  $ {command}\n", flush=True)
    started = time.time()
    code = subprocess.run(command, shell=True, env=env).returncode
    print(f"\n  suite exit={code} after {time.time() - started:.0f}s", flush=True)
    return code


def _check_tool_calling(url, checks):
    """U3 — jinja/tool calling. Most of the 44 orchestrator smokes depend on it."""
    tools = [
        {
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather in a city.",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"],
                },
            },
        }
    ]
    status, body = hf.http(
        "POST",
        f"{url}/v1/chat/completions",
        {
            "messages": [{"role": "user", "content": "What is the weather in Kyiv? Use the tool."}],
            "tools": tools,
            "tool_choice": "auto",
            "max_tokens": 256,
        },
        timeout=300,
    )
    reason = ""
    called = False
    if hf.ok(status) and isinstance(body, dict):
        choice = (body.get("choices") or [{}])[0]
        reason = choice.get("finish_reason", "")
        called = bool((choice.get("message") or {}).get("tool_calls"))
    checks.record("U3", "tool calling (LLAMA_ARG_JINJA)", called, f"finish_reason={reason or f'HTTP {status}'}")


def _check_streaming(url, checks):
    """U4 — SSE through the router: time to first byte, and whether a longer
    generation survives. Streamed by hand so TTFB is real, not buffered."""
    print("  [ .. ] U4 streaming …")
    req = urllib.request.Request(
        f"{url}/v1/chat/completions",
        data=json.dumps(
            {
                "messages": [{"role": "user", "content": "Count from 1 to 120, one number per line."}],
                "stream": True,
                "max_tokens": 600,
            }
        ).encode(),
        method="POST",
    )
    req.add_header("Authorization", f"Bearer {hf.TOKEN}")
    req.add_header("Content-Type", "application/json")
    start = time.time()
    ttfb, events, err = None, 0, ""
    try:
        with urllib.request.urlopen(req, timeout=600) as resp:
            for line in resp:
                if ttfb is None:
                    ttfb = time.time() - start
                if line.startswith(b"data:"):
                    events += 1
    except Exception as e:
        err = f"{type(e).__name__}: {e}"
    total = time.time() - start
    detail = f"ttfb={ttfb:.1f}s, events={events}, total={total:.1f}s" if ttfb else f"no data ({err})"
    checks.record("U4", "SSE streaming", events > 5 and not err, detail + (f" err={err}" if err else ""))


def probe_chat(url, checks, expected_gguf):
    """U1 (which file got loaded), U2 (context), U3 (jinja), U4 (SSE), U6 (auth).

    `expected_gguf` is the weights file the payload asked for — the whole point of
    U1 is that `/props` reports the file llama.cpp *actually* loaded, so the name
    has to be threaded in from the model that was selected rather than read from a
    constant that no longer describes every run."""
    print(f"\n=== chat checks against {url} ===")
    hf.wait_healthy(url)

    # U6 — authenticated really means authenticated. Without the header this must
    # not be a 200; a public endpoint would silently pass every other check.
    status, _ = hf.http("GET", f"{url}/v1/models", timeout=30, auth=False)
    checks.record("U6", "unauthenticated request rejected", status in (401, 403), f"HTTP {status}")

    # U5-adjacent — does /health survive the router? Decides whether the two
    # supervisor smokes keep their meaning (docs/history/remote-e2e-hf.md §4).
    status, _ = hf.http("GET", f"{url}/health", timeout=30)
    checks.record("U5b", "/health reachable", hf.ok(status), f"HTTP {status}")

    # U1 + U2 — /props tells us which file llama.cpp actually loaded and the
    # context it ended up with. This is the real answer to U1, whatever the
    # create payload looked like.
    status, body = hf.http("GET", f"{url}/props", timeout=60)
    if hf.ok(status) and isinstance(body, dict):
        settings = body.get("default_generation_settings") or {}
        n_ctx = settings.get("n_ctx") or body.get("n_ctx")
        model = body.get("model_path") or settings.get("model") or "?"
        checks.record("U1", "loaded model file", expected_gguf in str(model), str(model))
        checks.record("U2", "effective context", bool(n_ctx), f"n_ctx={n_ctx}")
    else:
        checks.record("U1", "loaded model file", False, f"/props HTTP {status}")
        checks.record("U2", "effective context", False, "unknown (/props unavailable)")

    _check_tool_calling(url, checks)
    _check_streaming(url, checks)


def probe_embed(url, checks):
    """U5 — embedding endpoint: OpenAI shape and dimension."""
    print(f"\n=== embedding checks against {url} ===")
    if not hf.wait_healthy(url):
        checks.record("U5", "embedding endpoint healthy", False, "never returned 200 on /health")
        return
    status, body = hf.http(
        "POST",
        f"{url}/v1/embeddings",
        {"input": ["mindfork probe", "second passage"], "model": hf.EMBED_REPO},
        timeout=180,
    )
    dim, count = 0, 0
    if hf.ok(status) and isinstance(body, dict):
        data = body.get("data") or []
        count = len(data)
        if data:
            dim = len(data[0].get("embedding") or [])
    checks.record(
        "U5",
        "llama.cpp /v1/embeddings (OpenAI shape)",
        count == 2 and dim == hf.EMBED_DIM,
        f"vectors={count}, dim={dim}" if count else f"HTTP {status}",
    )


# --------------------------------------------------------------------------
# Commands
# --------------------------------------------------------------------------
def cmd_hardware(_args):
    """U7 — the real vendor/region/instanceType strings."""
    for url in hf.PROVIDER_URLS:
        print(f"\n=== GET {url} ===")
        status, body = hf.http("GET", url)
        hf.show(status, body, limit=20000)
        if hf.ok(status):
            return hf.EXIT_OK
    return hf.EXIT_FAILED


def cmd_inspect(args):
    """Create it in the UI, then read what the UI built."""
    status, body = hf.http("GET", f"{hf.API}/{hf.NAMESPACE}/{args.name}")
    hf.show(status, body, limit=100000)
    return hf.EXIT_OK if hf.ok(status) else hf.EXIT_FAILED


def _run_embed_only(args, stamp, checks):
    """--embed-only: create, probe and summarize just the embedding endpoint."""
    name = f"e2e-probe-embed-{stamp}"
    if hf.create(hf.apply_overrides(hf.embed_payload(name, args), args.set), args.dry_run) is None:
        return hf.EXIT_OK if args.dry_run else hf.EXIT_FAILED
    url = hf.wait_running(name, args.timeout)
    if url:
        probe_embed(url, checks)
    else:
        checks.record("U5", "embedding endpoint reached running", False, "see above")
    checks.summary()
    return hf.EXIT_FAILED if checks.failed() else hf.EXIT_OK


def _create_and_probe_embed(args, stamp, checks):
    """Create the embedding endpoint and probe it. Returns its URL, or None."""
    name = f"e2e-probe-embed-{stamp}"
    if hf.create(hf.apply_overrides(hf.embed_payload(name, args), args.set)) is None:
        return None
    url = hf.wait_running(name, args.timeout)
    if url:
        probe_embed(url, checks)
    else:
        checks.record("U5", "embedding endpoint reached running", False, "see above")
    return url


def cmd_run(args):
    stamp = time.strftime("%m%d-%H%M%S")
    checks = Checks()
    started = time.time()

    if args.embed_only:
        return _run_embed_only(args, stamp, checks)

    chat_name = f"e2e-probe-chat-{stamp}"
    payload = hf.apply_overrides(hf.chat_payload(chat_name, args), args.set)
    created = hf.create(payload, args.dry_run)
    if args.dry_run:
        if not args.chat_only:
            hf.create(hf.apply_overrides(hf.embed_payload(f"e2e-probe-embed-{stamp}", args), args.set), True)
        return hf.EXIT_OK
    if created is None:
        return hf.EXIT_FAILED

    chat_url = hf.wait_running(chat_name, args.timeout)
    if chat_url:
        probe_chat(chat_url, checks, args.gguf or hf.chat_model(args)["gguf"])
    else:
        checks.record("U1", "endpoint reached running", False, "see the state/message above")

    embed_url = None
    if not args.chat_only:
        embed_url = _create_and_probe_embed(args, stamp, checks)

    suite_code = 0
    if args.suite and chat_url:
        suite_code = run_suite(args.suite, chat_url, embed_url)

    checks.summary()
    print(f"\nelapsed {time.time() - started:.0f}s")
    if chat_url and not args.suite:
        print("\nTo run the suite against these endpoints, use tools/e2e_hf.py:")
        print(f"  python tools/e2e_hf.py run --reuse-chat {chat_name}")
    return hf.EXIT_FAILED if (checks.failed() or suite_code) else hf.EXIT_OK


def main():
    parser = hf.run_parser(__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("doctor", help="diagnose a 401/403: token role, permissions, API access").set_defaults(fn=cmd_doctor)
    sub.add_parser("hardware", help="U7: list vendors/regions/instance types").set_defaults(fn=cmd_hardware)
    p = sub.add_parser("inspect", help="dump one endpoint's raw JSON")
    p.add_argument("name")
    p.set_defaults(fn=cmd_inspect)
    hf.add_shared_commands(sub)

    p = sub.add_parser("run", help="create, probe, delete")
    hf.add_endpoint_args(p)
    p.add_argument("--chat-only", action="store_true")
    p.add_argument("--embed-only", action="store_true")
    p.add_argument(
        "--suite",
        nargs="?",
        const="cargo test -- --ignored --test-threads=1",
        help="run this command with the endpoint env set (prefer tools/e2e_hf.py)",
    )
    p.set_defaults(fn=cmd_run)

    args = parser.parse_args()
    # `doctor` diagnoses the very call that namespace resolution depends on, so
    # it must not die inside it.
    hf.init(args.namespace, keep=getattr(args, "keep", False), resolve=args.cmd != "doctor")

    try:
        return args.fn(args)
    finally:
        hf.cleanup()


if __name__ == "__main__":
    sys.exit(main())
