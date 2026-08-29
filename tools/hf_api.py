#!/usr/bin/env python3
"""Hugging Face Inference Endpoints: the client both e2e tools share.

Extracted from `tools/hf_probe.py` (stage 0) when `tools/e2e_hf.py` (stage 2)
needed the same lifecycle. Plan: docs/history/remote-e2e-hf.md §6.

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

# The chat models the gate can run, each as **one** decision: a name resolves to
# the repository, the weights and the projector together. Three independent flags
# would let a caller compose a repo/file pair that does not exist and find out
# twenty minutes into a deploy (docs/history/e2e-second-chat-model.md, fork F1).
#
# `tag` is what goes into the endpoint name, so it has to stay short: the API caps
# a name at 32 characters and the CI run id already spends ~13 of them.
#
# `mmproj` is not optional decoration. Three `#[ignore]` smokes require a
# projector and **fail loudly rather than skip** against a text-only server, by
# design (docs/lessons.md §9) — so a gate deployed without one is a gate that is
# red for a reason unrelated to the code under test (fork F4).
CHAT_MODELS = {
    "gemma-4-31b": {
        "tag": "gemma",
        "repo": "google/gemma-4-31B-it-qat-q4_0-gguf",
        "gguf": "gemma-4-31B_q4_0-it.gguf",  # 17.65 GB
        "mmproj": "gemma-4-31B-it-mmproj.gguf",  # 1.20 GB
        "instance": "nvidia-l40s",  # 48 GB
        "region": "us-east-1",
    },
    "qwen-3.6-27b": {
        "tag": "qwen",
        "repo": "ggml-org/Qwen3.6-27B-GGUF",
        "gguf": "Qwen3.6-27B-Q4_K_M.gguf",  # 19.10 GB
        "mmproj": "mmproj-Qwen3.6-27B-Q8_0.gguf",  # 0.63 GB
        "instance": "nvidia-l40s",
        "region": "us-east-1",
    },
    # The third model is not a third flavour of the first two: it is split
    # across two files, it is an OpenAI open-weights model (the harmony
    # template), and it is 63 GB. Each of those is a dimension the gate has
    # never had (docs/research/e2e-gpt-oss-120b.md §1).
    "gpt-oss-120b": {
        "tag": "oss",
        "repo": "unsloth/gpt-oss-120b-GGUF",
        # Part **one** of two: llama.cpp is handed the first part and finds the
        # rest by name in the same directory (src/shared/gguf.rs).
        "gguf": "Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf",  # 49.61 + 13.78 GB
        # Text-only, and no projector for it exists — which is a different thing
        # from `--no-mmproj` and is treated differently (see `text_only`).
        "mmproj": None,
        # **Load-bearing.** That repository holds 13 quantizations, 1010 GB in
        # total, and `variant` is what keeps the endpoint from pulling all of
        # them: measured against the API, a file that does not match is simply
        # not on disk ("No such file or directory" at load). Undocumented on
        # HF's docs page; it is in the endpoints OpenAPI schema.
        "variant": "Q8_0/*",
        # 141 GB, aws us-west-2, $5.00/hr. The H100 is *not* the cheap option
        # here: it exists only on gcp at $10.00/hr and this account's quota for
        # it is 0 (research §3).
        "instance": "nvidia-h200",
        "region": "us-west-2",
        # Pinned rather than left to `--fit`, which the image runs by default:
        # on a 63 GB model an automatic partial offload does not fail, it just
        # runs part of the model on the CPU (research §4, U6). The two smaller
        # models keep the default they were measured on.
        "gpu_layers": 9999,
    },
}
# Gemma stays the default: the memory gates' similarity thresholds are calibrated
# against that stack, and it is the model every earlier live run was measured on
# (fork F3). A second family is a dimension, not a new baseline.
DEFAULT_CHAT_MODEL = "gemma-4-31b"

# Where an endpoint goes when neither the caller nor the model says otherwise.
DEFAULT_REGION = "us-east-1"

# Where a GPU that is *not* in the default region actually exists (GET
# /v2/provider, 2026-08-29). An instance implies its region: asking for an H200
# in us-east-1 is not a choice, it is a failed deploy, and the catalogue is the
# only thing that knows which is which. Listed here so that `--chat-instance`
# alone stays a safe thing to pass.
INSTANCE_REGIONS = {
    "nvidia-h200": "us-west-2",
    "nvidia-rtx-pro-6000": "us-east-2",
}

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
# lost (docs/history/remote-e2e-hf.md §6).
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
def chat_model(args):
    """The selected model's record: repository, weights, projector, and the
    hardware and download filter that go with them — one decision, not six."""
    return CHAT_MODELS[getattr(args, "chat_model", DEFAULT_CHAT_MODEL)]


def chat_instance(args):
    """The GPU for the chat endpoint: the caller's, else the model's own.

    A model that does not fit its default instance is not a choice anyone should
    have to remember to make — a 63 GB model dispatched onto a 48 GB L40S fails
    after the deploy, not before it.
    """
    return args.chat_instance or chat_model(args)["instance"]


def chat_region(args):
    """The region for the chat endpoint: the caller's, else the one implied by an
    explicitly named instance, else the model's own, else the default.

    The middle case is the one that matters: a caller who overrides the instance
    is not thereby asking for the model's region, and an H200 does not exist in
    us-east-1.
    """
    if args.region:
        return args.region
    if args.chat_instance:
        return INSTANCE_REGIONS.get(args.chat_instance, DEFAULT_REGION)
    return chat_model(args).get("region") or DEFAULT_REGION


def text_only(args):
    """The selected model has **no projector in existence** — as opposed to a
    projector this run chose not to deploy.

    The difference is the whole of fork F3 (docs/research/e2e-gpt-oss-120b.md).
    A model with no projector (`gpt-oss-120b`) makes the three vision smokes
    meaningless, so the run declares `MINDFORK_LIVE_TEXT_ONLY=1` and they skip,
    in writing. `--no-mmproj` deliberately does **not**: that flag exists to
    deploy a *sighted* model blind, and those three failing is precisely its
    point (docs/lessons.md §9).
    """
    return chat_model(args)["mmproj"] is None


def split_model(args):
    """The weights being deployed are **part one of several**.

    Read off the file name, because that is where the fact lives: `gguf-split`
    writes `<name>-00001-of-00002.gguf`, and llama.cpp is handed part one and
    finds the rest. Declared to the suite as `MINDFORK_LIVE_SPLIT_MODEL=1`, which
    turns on the smoke that checks a part number never reaches a chat header
    (docs/research/e2e-gpt-oss-120b.md §6, T2).

    The shape is duplicated from `src/shared/gguf.rs` rather than shared, because
    there is no way to share it across the language boundary; that module remains
    the one place the *meaning* of the tail is decided, and this is only asking
    which file was deployed.
    """
    gguf = args.gguf or chat_model(args)["gguf"]
    return bool(re.search(r"-\d{5}-of-\d{5}\.gguf$", gguf))


def chat_payload(name, args):
    """The v2 create payload for the managed llama.cpp engine (chat).

    Every field here was settled against the live API in stage 0, mostly by
    successive 422s (docs/history/remote-e2e-hf.md §3): the file is chosen with
    `modelPath` — not any name one would guess — and `ctxSize` is explicit,
    so context is set directly rather than falling out of the Max Tokens x Max
    Concurrent Requests settings the docs describe. `nParallel` splits ctxSize
    between llama.cpp slots and the suite runs --test-threads=1, so 1 keeps the
    whole context.

    `mmprojModelPath` **is** sent, which reverses stage 0's "the repo's second
    file is a vision projector we do not want". That was written before the app
    could see an image at all; since then three smokes require one and fail loudly
    on a blind model rather than skipping, so omitting it does not save a check —
    it costs three (docs/history/e2e-second-chat-model.md §3).
    """
    model = chat_model(args)
    mmproj = None if args.no_mmproj else (args.mmproj or model["mmproj"])
    payload = {
        "name": name,
        "type": args.endpoint_type,
        # The model's own region, unless the caller named one. A 63 GB model
        # lives where the card that holds it lives (H200: us-west-2), and the
        # embedders stay wherever `--region` puts them — three endpoints in two
        # regions is latency, not correctness.
        "provider": {"vendor": args.vendor, "region": chat_region(args)},
        "compute": {
            "accelerator": "gpu",
            "instanceType": chat_instance(args),
            "instanceSize": args.instance_size,
            "scaling": {
                "minReplica": 0,
                "maxReplica": 1,
                # Minutes. This is the leak ceiling: a run that dies without
                # deleting wastes at most this much idle GPU, whatever else
                # fails (docs/history/remote-e2e-hf.md §8).
                "scaleToZeroTimeout": args.scale_to_zero,
            },
        },
        "model": {
            "repository": model["repo"],
            "framework": "llamacpp",
            "image": {
                "llamacpp": {
                    "modelPath": args.gguf or model["gguf"],
                    "ctxSize": args.ctx,
                    "nParallel": args.parallel,
                    "threadsHttp": args.threads_http,
                    "url": args.image,
                }
            },
            "env": {"LLAMA_ARG_JINJA": "1"},
        },
    }
    # Added rather than set to null: the field is optional, and an explicit null
    # is a different thing to send than an absent key. Same for the two below.
    if mmproj:
        payload["model"]["image"]["llamacpp"]["mmprojModelPath"] = mmproj
    variant = args.variant or model.get("variant")
    if variant:
        payload["model"]["image"]["llamacpp"]["variant"] = variant
    gpu_layers = args.gpu_layers or model.get("gpu_layers")
    if gpu_layers:
        # Left unset, the image runs llama.cpp's `--fit`, which sizes what it
        # offloads to the memory it finds. On a 63 GB model that does not fail —
        # it quietly runs part of the model on the CPU and turns a 20-minute
        # suite into a timeout. Pin it (research §4, U6).
        payload["model"]["image"]["llamacpp"]["nGpuLayers"] = gpu_layers
    return payload


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
        "provider": {"vendor": args.vendor, "region": args.region or DEFAULT_REGION},
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
    parser.add_argument("--region", default="", help=f"default: the chat model's own, else {DEFAULT_REGION}")
    parser.add_argument("--chat-instance", default="", help="default: the model's own; `hf_probe.py hardware` lists them")
    parser.add_argument("--embed-instance", default="nvidia-t4")
    parser.add_argument("--instance-size", default="x1")
    parser.add_argument(
        "--chat-model",
        default=DEFAULT_CHAT_MODEL,
        choices=sorted(CHAT_MODELS),
        help="which chat model to deploy (repository + weights + projector as one choice)",
    )
    parser.add_argument("--gguf", default="", help="override the model's weights file")
    parser.add_argument("--mmproj", default="", help="override the model's vision projector")
    parser.add_argument(
        "--no-mmproj",
        action="store_true",
        help="deploy without a projector (the 3 vision smokes then FAIL, not skip)",
    )
    parser.add_argument(
        "--variant",
        default="",
        help="override the model's glob for which .gguf files the endpoint pulls",
    )
    parser.add_argument(
        "--gpu-layers",
        type=int,
        default=0,
        help="layers on the GPU; 0 = unset, i.e. llama.cpp's own --fit (research §4, U6)",
    )
    parser.add_argument("--ctx", type=int, default=16384, help="llama.cpp context (matches the local runs)")
    parser.add_argument("--parallel", type=int, default=1, help="llama.cpp slots; ctx is split between them")
    parser.add_argument("--threads-http", type=int, default=8)
    # `url` is required by BaseContainer and has no catalog default, so the
    # llama.cpp build is ours to pick -- and can be pinned to a tag, which
    # removes the "unpinned master" caveat from docs/history/remote-e2e-hf.md §10.
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
    (docs/history/remote-e2e-hf.md §3)."""
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


def verify_gone(name, attempts=6, delay=2.0):
    """Re-read until the endpoint is a 404. Reporting "deleted" without checking
    is how leaks start — but so is checking too eagerly.

    Deletion is not instantaneous: HF acknowledges the DELETE with a 200 and the
    endpoint disappears a moment later. CI run 30396557877 verified ~400 ms after
    its DELETE, saw a 200, and cried CLEANUP FAILED (exit 3) over an endpoint
    that was in fact deleted — which also masked four genuine test failures
    behind the wrong exit code. Only the *last* endpoint hits it, because
    `cleanup` sends every DELETE before verifying any, so the earlier ones get
    incidental delay for free.

    Retrying is safe on every path: the DELETE has already been sent, so the
    meter is stopped whatever happens next — this waits on the *proof*, not on
    the action. Returns (gone, last_status).
    """
    check = 0
    for attempt in range(attempts):
        check, _ = http("GET", f"{API}/{NAMESPACE}/{name}")
        if check == 404:
            return True, check
        if attempt + 1 < attempts:
            time.sleep(delay)
    return False, check


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


def _keep_endpoints():
    """--keep: hand the names to the user instead of deleting them."""
    global CREATED
    names, CREATED = CREATED, []
    say("\n=== --keep: NOT deleting ===")
    for name in names:
        say(f"  python tools/e2e_hf.py delete {name}")


def _verify_deletions(sent):
    """Verify each sent DELETE; a proven-gone name leaves CREATED. Returns the
    names that are still there."""
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
    return failed


def cleanup():
    if not CREATED:
        return
    if KEEP:
        _keep_endpoints()
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
    failed = _verify_deletions(sent)
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
