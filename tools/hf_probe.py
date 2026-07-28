#!/usr/bin/env python3
"""Probe HF Inference Endpoints as a host for the live e2e smokes (stage 0).

Plan: docs/remote-e2e-hf.md §3. Research: docs/research/remote-e2e-gpu.md.

This script exists to answer, against a real throwaway endpoint, the questions
that are not documented anywhere reachable — above all **U1: how to select which
`.gguf` file the llama.cpp engine serves**, because the target repository holds
two files (the 17.65 GB model and an unwanted 1.2 GB mmproj).

It deliberately uses the raw REST API (stdlib only, no `pip install`) instead of
`huggingface_hub`, for two reasons: full control over the request payload, and —
more useful here — the server's own 4xx bodies are printed verbatim, so a
rejected payload *teaches us the schema*. Payload field names below are informed
guesses; `--set`, `--payload` and `--dry-run` are there to iterate on them.

Cost: an L40S is $1.80/hr, a T4 $0.50/hr, billed by the minute. A full `run`
takes a few minutes. Cleanup is wired to `finally`, `atexit` and SIGINT/SIGTERM,
verifies deletion, and a failure to delete is a **louder** error than a failed
check (exit code 3) — a leaked endpoint is the one outcome that costs money.

The token needs two boxes ticked in the fine-grained token editor, under
User Permissions -> Inference: **Manage Inference Endpoints** (create/list/
delete) and **Make calls to Inference Endpoints** (the inference requests
themselves, which a `protected` endpoint checks). `doctor` reports which of
those is missing, and distinguishes that from the other two causes of a 403 —
no payment method on the account, or an org token still pending approval.

Usage:
    set HF_TOKEN=hf_...                      # fine-grained, Inference Endpoints
    python tools/hf_probe.py doctor          # diagnose a 401/403
    python tools/hf_probe.py hardware        # U7: vendors/regions/instance types
    python tools/hf_probe.py run             # the full probe (creates + deletes)
    python tools/hf_probe.py run --chat-only
    python tools/hf_probe.py run --dry-run   # print payloads, send nothing

    # If `run` cannot create the chat endpoint (most likely U1), create one by
    # hand in the UI, then let the UI tell us the schema:
    python tools/hf_probe.py inspect <name>
    python tools/hf_probe.py list
    python tools/hf_probe.py delete <name>

Exit codes: 0 ok · 1 usage/config · 2 a probe check failed · 3 CLEANUP FAILED.
"""

from __future__ import annotations

import argparse
import atexit
import json
import os
import signal
import sys
import time
import urllib.error
import urllib.request

API = "https://api.endpoints.huggingface.cloud/v2/endpoint"
PROVIDER_URLS = [
    "https://api.endpoints.huggingface.cloud/v2/provider",
    "https://api.endpoints.huggingface.cloud/v2/provider/vendor",
]
WHOAMI = "https://huggingface.co/api/whoami-v2"

CHAT_REPO = "google/gemma-4-31B-it-qat-q4_0-gguf"
CHAT_GGUF = "gemma-4-31B_q4_0-it.gguf"  # the 17.65 GB one; the other is mmproj
EMBED_REPO = "ggml-org/bge-m3-Q8_0-GGUF"  # the exact model the gates were calibrated on
EMBED_GGUF = "bge-m3-q8_0.gguf"
EMBED_DIM = 1024

# Endpoints created by this process, deleted on any exit path. Names are printed
# before the create call, so an orphan is identifiable even if the response is
# lost (docs/remote-e2e-hf.md §6).
CREATED: list[str] = []
KEEP = False
NAMESPACE = ""
TOKEN = ""


# --------------------------------------------------------------------------
# HTTP (stdlib; never raises on 4xx/5xx — we want to read those bodies)
# --------------------------------------------------------------------------
def http(method, url, body=None, timeout=60, raw=False):
    """Returns (status, parsed_json_or_bytes). A transport failure returns (0, str)."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", f"Bearer {TOKEN}")
    req.add_header("Accept", "application/json")
    if data:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            payload = resp.read()
            status = resp.status
    except urllib.error.HTTPError as e:
        payload, status = e.read(), e.code
    except Exception as e:  # DNS, TLS, timeout, connection reset
        return 0, f"{type(e).__name__}: {e}"
    if raw:
        return status, payload
    try:
        return status, json.loads(payload)
    except Exception:
        return status, payload.decode("utf-8", "replace")


def show(status, payload, limit=4000):
    text = json.dumps(payload, indent=2, ensure_ascii=False) if not isinstance(payload, str) else payload
    if len(text) > limit:
        text = text[:limit] + f"\n… [{len(text) - limit} more chars]"
    print(f"  HTTP {status}\n{text}")


def ok(status):
    return 200 <= status < 300


# --------------------------------------------------------------------------
# Endpoint lifecycle
# --------------------------------------------------------------------------
def resolve_namespace(explicit):
    if explicit:
        return explicit
    status, body = http("GET", WHOAMI)
    if not ok(status) or not isinstance(body, dict):
        die(
            1,
            f"cannot resolve the namespace from whoami (HTTP {status}).\n"
            "       Run `python tools/hf_probe.py doctor` to see what the token is missing.",
        )
    return body.get("name") or die(1, "whoami returned no user name; pass --namespace")


# The two checkboxes under User Permissions -> Inference in the fine-grained
# token editor (https://huggingface.co/settings/tokens). "Manage" covers
# create/list/delete; "Make calls" is what a `protected` endpoint checks on every
# inference request — the probe needs both, and so will the CI job.
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
        "POST /v1/chat/completions against a protected endpoint",
    ),
]


def held_permissions(granted):
    """Which of NEEDED_PERMISSIONS the token has. Returns [(label, what, held)]."""
    out = []
    for wire, frag, label, what in NEEDED_PERMISSIONS:
        held = wire in granted or any(frag in p for p in granted)
        out.append((label, what, held, wire))
    return out


def cmd_doctor(args):
    """Pinpoint a 403: token role, granted permissions, and the management API."""
    print("\n=== 1. whoami ===")
    status, body = http("GET", WHOAMI)
    show(status, body if not isinstance(body, dict) else {k: v for k, v in body.items() if k != "orgs"}, limit=3000)
    if status in (401, 403):
        print("  -> the token itself is rejected: wrong value, deleted, or revoked.")
        return 2
    if not ok(status):
        return 2

    auth = (body.get("auth") or {}).get("accessToken") or {}
    role = auth.get("role", "?")
    fine = auth.get("fineGrained") or {}
    granted = list(fine.get("global") or [])
    for scope in fine.get("scoped") or []:
        granted += list(scope.get("permissions") or [])
    print(f"\n  user: {body.get('name')} ({body.get('type')})   token role: {role}")
    print(f"  granted permissions: {granted or '(none reported)'}")

    missing = []
    if role == "fineGrained":
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
    elif role in ("read",):
        print("  -> a `read` token cannot manage endpoints. Use fine-grained (preferred) or `write`.")

    print("\n=== 2. management API ===")
    ns = args.namespace or body.get("name")
    status, listing = http("GET", f"{API}/{ns}")
    show(status, listing, limit=1500)
    if status == 403:
        print("\n  -> 403 from the management API. In order of likelihood:")
        print("     1. the token lacks 'Manage Inference Endpoints' (see above);")
        print("     2. no payment method on the account that owns the namespace —")
        print("        Inference Endpoints requires a card on file, and Hub credits")
        print("        do not substitute for one. Check https://huggingface.co/settings/billing;")
        print(f"     3. `{ns}` is an org and the token is not approved for it")
        print("        (org policies can hold a fine-grained token in Pending).")
        return 2
    if status == 401:
        print("  -> 401: the token is not being accepted at all by this API.")
        return 2
    if ok(status):
        if missing:
            print("  -> management API OK, but a permission above is missing: `run` will")
            print("     create the endpoint and then 403 on the inference calls.")
        else:
            print("  -> management API OK and both permissions present. Ready for `run`.")
    return 0 if ok(status) and not missing else 2


def chat_payload(name, args):
    """Best guess at the v2 create payload for the managed llama.cpp engine.

    `model.image.llamacpp` and the gguf field name are exactly what U1 is about:
    if this is wrong the API should say so, and the error body is the answer.
    """
    return {
        "name": name,
        "type": args.endpoint_type,
        "provider": {"vendor": args.vendor, "region": args.region},
        "compute": {
            "accelerator": "gpu",
            "instanceType": args.chat_instance,
            "instanceSize": args.instance_size,
            "scaling": {
                "minReplica": 0,
                "maxReplica": 1,
                # Minutes. Lowering this shrinks the leak ceiling of a crashed
                # run from one hour to this (docs/remote-e2e-hf.md §8).
                "scaleToZeroTimeout": args.scale_to_zero,
            },
        },
        "model": {
            "repository": CHAT_REPO,
            "framework": "llamacpp",
            # U1/U2, answered by successive 422s from the API itself
            # (2026-07-28): the field is `modelPath` (not any name one would
            # guess), and `ctxSize` is set *here* — so the context is explicit
            # after all, not an indirect consequence of the Max Tokens x Max
            # Concurrent Requests settings the docs describe.
            # `nParallel` splits ctxSize between slots in llama.cpp, and the
            # suite runs --test-threads=1, so 1 keeps the whole context.
            # `mmprojModelPath` is deliberately omitted: the repo's second file
            # is a vision projector we do not want, and it is an optional field
            # rather than something the engine picks up on its own.
            "image": {
                "llamacpp": {
                    "modelPath": args.gguf,
                    "ctxSize": args.ctx,
                    "nParallel": args.parallel,
                    "threadsHttp": args.threads_http,
                    "url": args.image,
                }
            },
            "env": {"LLAMA_ARG_JINJA": "1"},
        },
    }


def embed_payload(name, args):
    return {
        "name": name,
        "type": args.endpoint_type,
        "provider": {"vendor": args.vendor, "region": args.region},
        "compute": {
            "accelerator": "gpu",
            "instanceType": args.embed_instance,
            "instanceSize": args.instance_size,
            "scaling": {
                "minReplica": 0,
                "maxReplica": 1,
                "scaleToZeroTimeout": args.scale_to_zero,
            },
        },
        # The llama.cpp engine serves embeddings too — `LlamacppMode` has an
        # `embeddings` value (found in the OpenAPI schema, 2026-07-28). That is
        # better than TEI here: it is the *same* GGUF and quantization as the
        # local runs, and the memory gates' similarity thresholds were
        # calibrated against exactly that (docs/research/
        # embedding-model-change-reindex.md). `pooling` is left unset on
        # purpose, so llama.cpp reads it from the GGUF metadata exactly as it
        # does locally, where the user passes no --pooling either.
        "model": {
            "repository": EMBED_REPO,
            "framework": "llamacpp",
            "image": {
                "llamacpp": {
                    "modelPath": args.embed_gguf,
                    "ctxSize": args.embed_ctx,
                    "nParallel": args.parallel,
                    "threadsHttp": args.threads_http,
                    "mode": "embeddings",
                    "url": args.image,
                }
            },
        },
    }


def apply_overrides(payload, sets):
    """`--set a.b.c=value` — value is parsed as JSON, falling back to a string."""
    for item in sets or []:
        path, _, value = item.partition("=")
        try:
            parsed = json.loads(value)
        except Exception:
            parsed = value
        node = payload
        keys = path.split(".")
        for key in keys[:-1]:
            node = node.setdefault(key, {})
        node[keys[-1]] = parsed
    return payload


def create(payload, dry_run):
    name = payload["name"]
    print(f"\n=== create {name} ===")
    print(json.dumps(payload, indent=2))
    if dry_run:
        print("  --dry-run: not sent")
        return None
    # Record BEFORE the call: a lost response must still leave a trail.
    CREATED.append(name)
    status, body = http("POST", f"{API}/{NAMESPACE}", payload)
    show(status, body)
    if ok(status) and isinstance(body, dict):
        # The API coerces an unknown `type` instead of rejecting it: sending the
        # older "protected" silently produced a `private` (PrivateLink-only)
        # endpoint that no CI runner could reach. Never trust the echo.
        got, want = body.get("type"), payload.get("type")
        if got != want:
            print(f"  -> REFUSING: asked for type={want!r}, got {got!r}. Deleting.")
            return None
    if not ok(status):
        # Creation failed, so there is probably nothing to delete — but keep the
        # name registered anyway; a verified 404 at cleanup costs nothing and a
        # half-created endpoint would otherwise be missed.
        print("  -> create FAILED. The body above is the schema hint (U1).")
        return None
    return body


def wait_running(name, timeout, poll=10):
    print(f"\n=== wait for {name} (timeout {timeout}s) ===")
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        status, body = http("GET", f"{API}/{NAMESPACE}/{name}")
        if not ok(status):
            print(f"  HTTP {status} while polling")
            time.sleep(poll)
            continue
        state = (body.get("status") or {}).get("state", "?")
        if state != last:
            print(f"  [{int(time.time() - deadline + timeout):>4}s] {state}")
            last = state
        if state in ("running",):
            url = (body.get("status") or {}).get("url") or body.get("url")
            print(f"  -> running: {url}")
            return url
        if state in ("failed",):
            message = (body.get("status") or {}).get("message")
            print(f"  -> FAILED: {message}")
            return None
        time.sleep(poll)
    print("  -> timed out")
    return None


def delete_one(name):
    status, body = http("DELETE", f"{API}/{NAMESPACE}/{name}")
    # Verify rather than trust: re-read and require a 404.
    check, _ = http("GET", f"{API}/{NAMESPACE}/{name}")
    gone = check == 404
    mark = "gone" if gone else f"STILL THERE (GET -> {check})"
    print(f"  delete {name}: HTTP {status}, {mark}")
    if not gone and not ok(status):
        show(status, body, limit=800)
    return gone


def cleanup():
    global CREATED
    if not CREATED:
        return
    names, CREATED = CREATED, []
    if KEEP:
        print("\n=== --keep: NOT deleting ===")
        for name in names:
            print(f"  python tools/hf_probe.py delete {name}")
        return
    print("\n=== cleanup ===")
    failed = [n for n in names if not delete_one(n)]
    if failed:
        print("\n!!! CLEANUP FAILED — these endpoints may still be billing:")
        for name in failed:
            print(f"  !!! {name}  ->  python tools/hf_probe.py delete {name}")
        print("!!! Check https://endpoints.huggingface.co/ now.")
        os._exit(3)  # bypass any further handlers; this must be the last word


def on_signal(signum, _frame):
    print(f"\n[signal {signum}] cleaning up before exit")
    cleanup()
    os._exit(130)


def die(code, message):
    print(f"error: {message}", file=sys.stderr)
    sys.exit(code)


# --------------------------------------------------------------------------
# Checks (U2-U6)
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


def wait_healthy(url, timeout=600, poll=5):
    """HF's `running` state only means the container is up; llama.cpp still
    answers `503 Loading model` until the weights are in. This is the same
    distinction OpenAiClient::probe() draws, and the runner needs it too --
    starting the suite on `running` alone would fail the first request."""
    print(f"  waiting for /health (timeout {timeout}s)")
    deadline = time.time() + timeout
    while time.time() < deadline:
        status, _ = http("GET", f"{url}/health", timeout=30)
        if status == 200:
            print("  -> healthy")
            return True
        time.sleep(poll)
    print(f"  -> not healthy within {timeout}s (last HTTP {status})")
    return False


def probe_chat(url, checks):
    """U1 (which file got loaded), U2 (context), U3 (jinja), U4 (SSE), U6 (auth)."""
    print(f"\n=== chat checks against {url} ===")
    wait_healthy(url)

    # U6 — protected really means protected. Without the header this must not be
    # a 200; a public endpoint would silently pass every other check.
    saved, globals()["TOKEN"] = TOKEN, ""
    status, _ = http("GET", f"{url}/v1/models", timeout=30)
    globals()["TOKEN"] = saved
    checks.record("U6", "unauthenticated request rejected", status in (401, 403), f"HTTP {status}")

    # U5-adjacent — does /health survive the router? Decides whether the two
    # supervisor smokes keep their meaning (docs/remote-e2e-hf.md §4).
    status, _ = http("GET", f"{url}/health", timeout=30)
    checks.record("U5b", "/health reachable", ok(status), f"HTTP {status}")

    # U1 + U2 — /props tells us which file llama.cpp actually loaded and the
    # context it ended up with. This is the real answer to U1, whatever the
    # create payload looked like.
    status, body = http("GET", f"{url}/props", timeout=60)
    if ok(status) and isinstance(body, dict):
        settings = body.get("default_generation_settings") or {}
        n_ctx = settings.get("n_ctx") or body.get("n_ctx")
        model = body.get("model_path") or settings.get("model") or "?"
        checks.record("U1", "loaded model file", CHAT_GGUF in str(model), str(model))
        checks.record("U2", "effective context", bool(n_ctx), f"n_ctx={n_ctx}")
    else:
        checks.record("U1", "loaded model file", False, f"/props HTTP {status}")
        checks.record("U2", "effective context", False, "unknown (/props unavailable)")

    # U3 — jinja/tool calling. Most of the 44 orchestrator smokes depend on it.
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
    status, body = http(
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
    if ok(status) and isinstance(body, dict):
        choice = (body.get("choices") or [{}])[0]
        reason = choice.get("finish_reason", "")
        called = bool((choice.get("message") or {}).get("tool_calls"))
    checks.record("U3", "tool calling (LLAMA_ARG_JINJA)", called, f"finish_reason={reason or f'HTTP {status}'}")

    # U4 — SSE through the router: time to first byte, and whether a longer
    # generation survives. Streamed by hand so TTFB is real, not buffered.
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
    req.add_header("Authorization", f"Bearer {TOKEN}")
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


def probe_embed(url, checks):
    """U5 — embedding endpoint: OpenAI shape and dimension."""
    print(f"\n=== embedding checks against {url} ===")
    if not wait_healthy(url):
        checks.record("U5", "embedding endpoint healthy", False, "never returned 200 on /health")
        return
    status, body = http(
        "POST",
        f"{url}/v1/embeddings",
        {"input": ["mindfork probe", "second passage"], "model": EMBED_REPO},
        timeout=180,
    )
    dim, count = 0, 0
    if ok(status) and isinstance(body, dict):
        data = body.get("data") or []
        count = len(data)
        if data:
            dim = len(data[0].get("embedding") or [])
    checks.record(
        "U5",
        "llama.cpp /v1/embeddings (OpenAI shape)",
        count == 2 and dim == EMBED_DIM,
        f"vectors={count}, dim={dim}" if count else f"HTTP {status}",
    )


# --------------------------------------------------------------------------
# Commands
# --------------------------------------------------------------------------
def cmd_hardware(args):
    """U7 — the real vendor/region/instanceType strings."""
    for url in PROVIDER_URLS:
        print(f"\n=== GET {url} ===")
        status, body = http("GET", url)
        show(status, body, limit=20000)
        if ok(status):
            return 0
    return 2


def cmd_list(args):
    status, body = http("GET", f"{API}/{NAMESPACE}")
    if not ok(status):
        show(status, body)
        return 2
    items = body.get("items", body) if isinstance(body, dict) else body
    for item in items or []:
        state = (item.get("status") or {}).get("state", "?")
        print(f"  {item.get('name'):<40} {state:<14} {item.get('model', {}).get('repository', '')}")
    return 0


def cmd_inspect(args):
    """The reliable answer to U1: create it in the UI, then read what the UI built."""
    status, body = http("GET", f"{API}/{NAMESPACE}/{args.name}")
    show(status, body, limit=100000)
    return 0 if ok(status) else 2


def cmd_delete(args):
    return 0 if delete_one(args.name) else 3


def cmd_run(args):
    stamp = time.strftime("%m%d-%H%M%S")
    checks = Checks()
    started = time.time()

    if args.embed_only:
        name = f"e2e-probe-embed-{stamp}"
        if create(apply_overrides(embed_payload(name, args), args.set), args.dry_run) is None:
            return 0 if args.dry_run else 2
        url = wait_running(name, args.timeout)
        if url:
            probe_embed(url, checks)
        else:
            checks.record("U5", "TEI endpoint reached running", False, "see above")
        checks.summary()
        return 2 if checks.failed() else 0

    chat_name = f"e2e-probe-chat-{stamp}"
    payload = apply_overrides(chat_payload(chat_name, args), args.set)
    created = create(payload, args.dry_run)
    if args.dry_run:
        if not args.chat_only:
            create(apply_overrides(embed_payload(f"e2e-probe-embed-{stamp}", args), args.set), True)
        return 0
    if created is None:
        print(
            "\nU1 unresolved. Next step: create one endpoint in the UI "
            f"(repo {CHAT_REPO}, file {CHAT_GGUF}), then run:\n"
            "  python tools/hf_probe.py inspect <name>\n"
            "and copy the `model.image` section back into chat_payload()."
        )
        return 2

    chat_url = wait_running(chat_name, args.timeout)
    if chat_url:
        probe_chat(chat_url, checks)
    else:
        checks.record("U1", "endpoint reached running", False, "see the state/message above")

    if not args.chat_only:
        embed_name = f"e2e-probe-embed-{stamp}"
        if create(apply_overrides(embed_payload(embed_name, args), args.set), False) is not None:
            embed_url = wait_running(embed_name, args.timeout)
            if embed_url:
                probe_embed(embed_url, checks)
            else:
                checks.record("U5", "TEI endpoint reached running", False, "see above")

    checks.summary()
    print(f"\nelapsed {time.time() - started:.0f}s")
    if chat_url:
        print("\nTo run the suite against this endpoint before it is deleted, use --keep and:")
        print(f'  MINDFORK_ENGINE_URL={chat_url}/v1  MINDFORK_ENGINE_KEY=$HF_TOKEN \\')
        print("  cargo test -- --ignored --test-threads=1")
    return 2 if checks.failed() else 0


def main():
    global TOKEN, NAMESPACE, KEEP

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--namespace", default="", help="HF user or org (default: whoami)")
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("doctor", help="diagnose a 401/403: token role, permissions, API access").set_defaults(
        fn=cmd_doctor
    )
    sub.add_parser("hardware", help="U7: list vendors/regions/instance types").set_defaults(fn=cmd_hardware)
    sub.add_parser("list", help="list your endpoints").set_defaults(fn=cmd_list)
    p = sub.add_parser("inspect", help="dump one endpoint's raw JSON (the U1 answer)")
    p.add_argument("name")
    p.set_defaults(fn=cmd_inspect)
    p = sub.add_parser("delete", help="delete one endpoint and verify it is gone")
    p.add_argument("name")
    p.set_defaults(fn=cmd_delete)

    p = sub.add_parser("run", help="create, probe, delete")
    p.add_argument("--vendor", default="aws")
    p.add_argument("--region", default="us-east-1")
    p.add_argument("--chat-instance", default="nvidia-l40s", help="U7: confirm with `hardware`")
    p.add_argument("--embed-instance", default="nvidia-t4")
    p.add_argument("--instance-size", default="x1")
    p.add_argument("--gguf", default=CHAT_GGUF)
    p.add_argument("--ctx", type=int, default=16384, help="llama.cpp context (matches the local runs)")
    p.add_argument("--parallel", type=int, default=1, help="llama.cpp slots; ctx is split between them")
    p.add_argument("--threads-http", type=int, default=8)
    # `url` is required by BaseContainer and has no catalog default, so the
    # llama.cpp build is ours to pick -- and can be pinned to a tag, which
    # removes the "unpinned master" caveat from docs/remote-e2e-hf.md.
    p.add_argument("--image", default="ghcr.io/ggml-org/llama.cpp:server-cuda")
    p.add_argument("--endpoint-type", default="authenticated", choices=["public", "authenticated", "private"])
    p.add_argument("--embed-gguf", default=EMBED_GGUF)
    p.add_argument("--embed-ctx", type=int, default=8192, help="bge-m3 tops out at 8192")
    p.add_argument("--scale-to-zero", type=int, default=15, help="idle minutes (the leak ceiling)")
    p.add_argument("--timeout", type=int, default=1500, help="seconds to wait for 'running'")
    p.add_argument("--chat-only", action="store_true")
    p.add_argument("--embed-only", action="store_true")
    p.add_argument("--keep", action="store_true", help="do not delete (debugging; costs money)")
    p.add_argument("--dry-run", action="store_true", help="print payloads, send nothing")
    p.add_argument("--set", action="append", metavar="a.b.c=json", help="override a payload field")
    p.set_defaults(fn=cmd_run)

    args = parser.parse_args()

    TOKEN = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN") or ""
    if not TOKEN:
        die(1, "set HF_TOKEN (a fine-grained token with Inference Endpoints access)")
    KEEP = getattr(args, "keep", False)
    if args.cmd == "doctor":
        # `doctor` diagnoses the very call that resolution depends on, so it must
        # not die inside it.
        NAMESPACE = args.namespace
    else:
        NAMESPACE = resolve_namespace(args.namespace)
        print(f"namespace: {NAMESPACE}")

    signal.signal(signal.SIGINT, on_signal)
    with contextlib_suppress():
        signal.signal(signal.SIGTERM, on_signal)
    atexit.register(cleanup)

    try:
        return args.fn(args)
    finally:
        cleanup()


class contextlib_suppress:
    """SIGTERM is not settable on every platform; never fail because of it."""

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return True


if __name__ == "__main__":
    sys.exit(main())
