#!/usr/bin/env python3
"""Hugging Face Inference Endpoints: the client both e2e tools share.

Extracted from `tools/hf_probe.py` (stage 0) when `tools/e2e_hf.py` (stage 2)
needed the same lifecycle. Plan: docs/remote-e2e-hf.md §6.

Sharing it is not tidiness. Two copies of the create payload would drift the
moment the API schema moves, and — worse — two copies of the cleanup would mean
two places where a bug leaks a billing GPU. There is one implementation of
"create, wait, delete, **verify** deleted", and both scripts use it.

Stdlib only, on purpose: this has to run on a bare CI runner with no
`pip install` step, and the raw REST API prints the server's own 4xx bodies
verbatim, so a rejected payload teaches us the schema (`--set` iterates on it).

The token needs two boxes ticked in the fine-grained token editor, under
User Permissions -> Inference: **Manage Inference Endpoints** (create/list/
delete) and **Make calls to Inference Endpoints** (the inference requests
themselves, which an `authenticated` endpoint checks).
`python tools/hf_probe.py doctor` reports which one is missing.
"""

from __future__ import annotations

import argparse
import atexit
import json
import os
import re
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

# The *second* embedding model, for the four smokes that guard the
# embedding-model-change track (`MINDFORK_EMBED_URL_ALT`). It has to be a
# genuinely different vector space **at the same dimensionality**: bge-m3 and
# multilingual-e5-large-instruct are both 1024-d, yet the same text embedded by
# both scores ~0.37 — which is exactly why no dimensionality check can catch a
# swap, and why those smokes exist (docs/research/
# embedding-model-change-reindex.md §1). q8_0 deliberately matches the local
# stand, because the calibration figures were measured against that file.
ALT_EMBED_REPO = "Ralriki/multilingual-e5-large-instruct-GGUF"
ALT_EMBED_GGUF = "multilingual-e5-large-instruct-q8_0.gguf"

# "Must only contain lowercase alphanumeric characters or '-' and have a length
# of 32 characters maximum" (the API's own description of Endpoint.name).
NAME_MAX = 32

# Endpoints created by this process, deleted on any exit path. Names are printed
# before the create call, so an orphan is identifiable even if the response is
# lost (docs/remote-e2e-hf.md §6).
CREATED: list[str] = []
KEEP = False
NAMESPACE = ""
TOKEN = ""

EXIT_OK = 0
EXIT_USAGE = 1
EXIT_FAILED = 2
EXIT_CLEANUP = 3  # louder than a failed test: this one costs money


# --------------------------------------------------------------------------
# HTTP (stdlib; never raises on 4xx/5xx — we want to read those bodies)
# --------------------------------------------------------------------------
def http(method, url, body=None, timeout=60, raw=False, auth=True):
    """Returns (status, parsed_json_or_bytes). A transport failure returns (0, str)."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if auth:
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


def die(code, message):
    print(f"error: {message}", file=sys.stderr)
    sys.exit(code)


# --------------------------------------------------------------------------
# Setup
# --------------------------------------------------------------------------
def resolve_namespace(explicit):
    if explicit:
        return explicit
    status, body = http("GET", WHOAMI)
    if not ok(status) or not isinstance(body, dict):
        die(
            EXIT_USAGE,
            f"cannot resolve the namespace from whoami (HTTP {status}).\n"
            "       Run `python tools/hf_probe.py doctor` to see what the token is missing.",
        )
    return body.get("name") or die(EXIT_USAGE, "whoami returned no user name; pass --namespace")


def read_token():
    token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN") or ""
    if not token:
        die(EXIT_USAGE, "set HF_TOKEN (a fine-grained token with Inference Endpoints access)")
    return token


def init(namespace_arg, keep=False, resolve=True):
    """Read the token, resolve the namespace, and arm the cleanup handlers."""
    global TOKEN, NAMESPACE, KEEP
    # This tool prints JSON straight from a remote API, and on Windows a piped
    # stdout defaults to cp1252 — one unexpected character would otherwise abort
    # a run that is holding a GPU.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")
        except (AttributeError, ValueError):
            pass
    TOKEN = read_token()
    KEEP = keep
    NAMESPACE = resolve_namespace(namespace_arg) if resolve else namespace_arg
    if resolve:
        print(f"namespace: {NAMESPACE}")
    install_handlers()


def install_handlers():
    # SIGINT/SIGTERM: a cancelled CI job (GitHub sends SIGINT, then SIGKILL
    # ~7.5 s later). SIGBREAK: Ctrl+Break on Windows, which is also the only way
    # to signal this process group from another one — i.e. how the cancellation
    # drill is run on a dev box.
    for name in ("SIGINT", "SIGTERM", "SIGBREAK"):
        sig = getattr(signal, name, None)
        if sig is None:
            continue
        try:
            signal.signal(sig, _on_signal)
        except (ValueError, AttributeError, OSError):
            pass  # not settable on every platform; never fail because of it
    atexit.register(cleanup)


def _on_signal(signum, _frame):
    # The path that matters most: a cancelled CI job signals the process, and
    # this is the only chance to hand the GPU back.
    say(f"\n[signal {signum}] cleaning up before exit")
    cleanup()
    os._exit(130)


def safe_name(name):
    """Coerce to what the API accepts, rather than letting it 422 after a wait."""
    clean = re.sub(r"[^a-z0-9-]+", "-", name.lower()).strip("-")
    return clean[:NAME_MAX].rstrip("-")


# --------------------------------------------------------------------------
# Create payloads
# --------------------------------------------------------------------------
def chat_payload(name, args):
    """The v2 create payload for the managed llama.cpp engine (chat).

    Every field here was settled against the live API in stage 0, mostly by
    successive 422s (docs/remote-e2e-hf.md §3): the file is chosen with
    `modelPath` — not any name one would guess — and `ctxSize` is explicit,
    so context is set directly rather than falling out of the Max Tokens x Max
    Concurrent Requests settings the docs describe. `nParallel` splits ctxSize
    between llama.cpp slots and the suite runs --test-threads=1, so 1 keeps the
    whole context. `mmprojModelPath` is deliberately omitted: the repo's second
    file is a vision projector we do not want.
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
                # Minutes. This is the leak ceiling: a run that dies without
                # deleting wastes at most this much idle GPU, whatever else
                # fails (docs/remote-e2e-hf.md §8).
                "scaleToZeroTimeout": args.scale_to_zero,
            },
        },
        "model": {
            "repository": CHAT_REPO,
            "framework": "llamacpp",
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


def embed_payload(name, args, repo=None, gguf=None):
    """The same engine in embedding mode.

    `LlamacppMode` has an `embeddings` value, so bge-m3 is served by llama.cpp
    itself rather than TEI — the *same GGUF and quantization as the local
    stack*, which matters because the memory gates' similarity thresholds were
    calibrated against exactly that model (docs/research/
    embedding-model-change-reindex.md §8.2). `pooling` is left unset on purpose,
    so llama.cpp reads it from the GGUF metadata exactly as it does locally,
    where the user passes no --pooling either.

    `repo`/`gguf` override the model, which is how the *alternate* embedder
    (ALT_EMBED_*) is deployed — same engine, same flags, different weights.
    `ctxSize` is shared with the primary embedder on purpose: e5-large's
    `n_ctx_train` is only 514, but llama.cpp accepts a larger context (the local
    stand runs it on the default 4096), and what actually breaks embeddings is
    too *small* a physical batch, not too large a context.
    """
    # Falling back to the flag, not to the constant: `--embed-gguf` must keep
    # working for the primary embedder.
    repo = repo or EMBED_REPO
    gguf = gguf or args.embed_gguf
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
        "model": {
            "repository": repo,
            "framework": "llamacpp",
            "image": {
                "llamacpp": {
                    "modelPath": gguf,
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


def add_endpoint_args(parser):
    """The flags that describe *what to deploy* — shared so the two scripts
    cannot deploy subtly different endpoints."""
    parser.add_argument("--vendor", default="aws")
    parser.add_argument("--region", default="us-east-1")
    parser.add_argument("--chat-instance", default="nvidia-l40s", help="48 GB; `hf_probe.py hardware` lists them")
    parser.add_argument("--embed-instance", default="nvidia-t4")
    parser.add_argument("--instance-size", default="x1")
    parser.add_argument("--gguf", default=CHAT_GGUF)
    parser.add_argument("--ctx", type=int, default=16384, help="llama.cpp context (matches the local runs)")
    parser.add_argument("--parallel", type=int, default=1, help="llama.cpp slots; ctx is split between them")
    parser.add_argument("--threads-http", type=int, default=8)
    # `url` is required by BaseContainer and has no catalog default, so the
    # llama.cpp build is ours to pick -- and can be pinned to a tag, which
    # removes the "unpinned master" caveat from docs/remote-e2e-hf.md §10.
    parser.add_argument("--image", default="ghcr.io/ggml-org/llama.cpp:server-cuda")
    parser.add_argument(
        "--endpoint-type",
        default="authenticated",
        choices=["public", "authenticated", "private"],
        help="`private` is PrivateLink-only and unreachable from CI",
    )
    parser.add_argument("--embed-gguf", default=EMBED_GGUF)
    parser.add_argument("--embed-ctx", type=int, default=8192, help="bge-m3 tops out at 8192")
    parser.add_argument("--scale-to-zero", type=int, default=15, help="idle minutes (the leak ceiling)")
    parser.add_argument("--timeout", type=int, default=1500, help="seconds to wait for 'running'")
    parser.add_argument("--keep", action="store_true", help="do not delete (debugging; costs money)")
    parser.add_argument("--dry-run", action="store_true", help="print payloads, send nothing")
    parser.add_argument("--set", action="append", metavar="a.b.c=json", help="override a payload field")


# --------------------------------------------------------------------------
# Lifecycle
# --------------------------------------------------------------------------
def create(payload, dry_run=False):
    name = payload["name"]
    print(f"\n=== create {name} ===", flush=True)
    print(json.dumps(payload, indent=2))
    if dry_run:
        print("  --dry-run: not sent")
        return None
    # Record BEFORE the call: a lost response must still leave a trail.
    CREATED.append(name)
    status, body = http("POST", f"{API}/{NAMESPACE}", payload)
    show(status, body)
    if not ok(status):
        # Creation failed, so there is probably nothing to delete — but keep the
        # name registered anyway; a verified 404 at cleanup costs nothing and a
        # half-created endpoint would otherwise be missed.
        print("  -> create FAILED; the body above is the schema hint.")
        return None
    if isinstance(body, dict):
        # The API coerces an unknown `type` instead of rejecting it: sending the
        # older "protected" silently produced a `private` (PrivateLink-only)
        # endpoint that no CI runner could reach, with a 200 and a healthy-looking
        # response. Never trust the echo.
        got, want = body.get("type"), payload.get("type")
        if got != want:
            print(f"  -> REFUSING: asked for type={want!r}, got {got!r}. It will be deleted.")
            return None
    return body


def endpoint_state(name):
    """(state, url, message) — state is '?' when the endpoint cannot be read."""
    status, body = http("GET", f"{API}/{NAMESPACE}/{name}")
    if not ok(status) or not isinstance(body, dict):
        return "?", None, f"HTTP {status}"
    st = body.get("status") or {}
    return st.get("state", "?"), st.get("url") or body.get("url"), st.get("message") or ""


def wait_running(name, timeout, poll=10):
    """Wait for the container. NOT for the model — see wait_healthy."""
    print(f"\n=== wait for {name} (timeout {timeout}s) ===", flush=True)
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        state, url, message = endpoint_state(name)
        if state != last:
            print(f"  [{int(time.time() - deadline + timeout):>4}s] {state}", flush=True)
            last = state
        # `scaledToZero` matters for --reuse: the endpoint is configured and
        # billing nothing, and the first request wakes it, which wait_healthy's
        # polling then does.
        if state in ("running", "scaledToZero"):
            print(f"  -> {state}: {url}")
            return url
        if state in ("failed", "updateFailed"):
            print(f"  -> FAILED: {message}")
            return None
        if state == "paused":
            print("  -> paused; resume it in the UI or delete it (this script does not resume)")
            return None
        time.sleep(poll)
    print("  -> timed out")
    return None


def wait_healthy(url, timeout=900, poll=5):
    """HF's `running` state only means the container is up; llama.cpp still
    answers `503 Loading model` until the weights are in. This is the same
    distinction OpenAiClient::probe() draws, and the runner needs it too --
    starting the suite on `running` alone fails the first request
    (docs/remote-e2e-hf.md §3)."""
    print(f"  waiting for /health at {url} (timeout {timeout}s)", flush=True)
    deadline = time.time() + timeout
    status = 0
    while time.time() < deadline:
        status, _ = http("GET", f"{url}/health", timeout=30)
        if status == 200:
            print("  -> healthy", flush=True)
            return True
        time.sleep(poll)
    print(f"  -> not healthy within {timeout}s (last HTTP {status})")
    return False


def request_delete(name):
    """Send the DELETE — the call that actually stops the meter."""
    return http("DELETE", f"{API}/{NAMESPACE}/{name}")


def verify_gone(name):
    """Re-read and require a 404. Reporting "deleted" without checking is how
    leaks start. Returns (gone, status)."""
    check, _ = http("GET", f"{API}/{NAMESPACE}/{name}")
    return check == 404, check


def delete_one(name):
    status, body = request_delete(name)
    gone, check = verify_gone(name)
    print(f"  delete {name}: HTTP {status}, {'gone' if gone else f'STILL THERE (GET -> {check})'}", flush=True)
    if not gone and not ok(status):
        show(status, body, limit=800)
    return gone


def say(*args):
    """Print, but never let a dead stdout stop a deletion.

    Found the hard way, 2026-07-28: when the parent process died the runner's
    stdout pipe broke, the first `print` in cleanup raised BrokenPipeError, and
    both endpoints leaked — while the log, of course, could not say so. Cleanup
    must not depend on being able to talk.
    """
    try:
        print(*args, flush=True)
    except Exception:
        pass


def cleanup():
    global CREATED
    if not CREATED:
        return
    if KEEP:
        names, CREATED = CREATED, []
        say("\n=== --keep: NOT deleting ===")
        for name in names:
            say(f"  python tools/e2e_hf.py delete {name}")
        return
    # A name leaves CREATED only once its endpoint is *proven* gone. Clearing
    # the list up front means that anything raising in between — a broken pipe,
    # a killed interpreter — loses the names, and then the atexit re-entry finds
    # nothing to do and the endpoints bill on. Keeping them makes cleanup
    # retryable instead: `finally` and `atexit` both get a real chance.
    names = list(CREATED)
    say("\n=== cleanup ===")
    # Every DELETE first, then every verification. A cancelled CI job gives this
    # handler roughly 7.5 s before SIGKILL, so the calls that free the GPU must
    # not queue behind a confirmation round-trip for the previous endpoint.
    sent = [(name, request_delete(name)) for name in names]
    failed = []
    for name, (status, body) in sent:
        gone, check = verify_gone(name)
        say(f"  delete {name}: HTTP {status}, {'gone' if gone else f'STILL THERE (GET -> {check})'}")
        if gone:
            CREATED.remove(name)
        else:
            failed.append(name)
            if not ok(status):
                say(json.dumps(body, ensure_ascii=False)[:800] if not isinstance(body, str) else body[:800])
    if failed:
        say("\n!!! CLEANUP FAILED — these endpoints may still be billing:")
        for name in failed:
            say(f"  !!! {name}  ->  python tools/e2e_hf.py delete {name}")
        say("!!! Check https://endpoints.huggingface.co/ now.")
        # Bypass any further handlers; a leaked GPU must be the last word, and
        # must outrank a failed test in the exit code.
        os._exit(EXIT_CLEANUP)


def list_endpoints():
    status, body = http("GET", f"{API}/{NAMESPACE}")
    if not ok(status):
        show(status, body)
        return None
    items = body.get("items", body) if isinstance(body, dict) else body
    return items or []


# --------------------------------------------------------------------------
# Shared commands
# --------------------------------------------------------------------------
def cmd_list(_args):
    items = list_endpoints()
    if items is None:
        return EXIT_FAILED
    if not items:
        print("  (no endpoints)")
    for item in items:
        st = item.get("status") or {}
        print(
            f"  {item.get('name', '?'):<34} {st.get('state', '?'):<14} "
            f"{st.get('createdAt', '?'):<26} {(item.get('model') or {}).get('repository', '')}"
        )
    return EXIT_OK


def cmd_delete(args):
    return EXIT_OK if delete_one(args.name) else EXIT_CLEANUP


def add_shared_commands(sub):
    sub.add_parser("list", help="list your endpoints (name, state, created, repo)").set_defaults(fn=cmd_list)
    p = sub.add_parser("delete", help="delete one endpoint and verify it is gone")
    p.add_argument("name")
    p.set_defaults(fn=cmd_delete)


def run_parser(description):
    parser = argparse.ArgumentParser(description=description, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--namespace", default="", help="HF user or org (default: whoami)")
    return parser
