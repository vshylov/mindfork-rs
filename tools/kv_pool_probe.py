"""Unified-KV-pool overflow probe (docs/research/admission-by-budget.md §3).

Against a llama-server launched `-c POOL -np 2 --kv-unified`: two conversations
whose prompts together outgrow the pool, run one after another (control) and
at once (the hazard). Records, per request: HTTP status, finish reason,
`usage`, llama.cpp's `timings` (prompt_n / cache_n) and any error envelope,
in both streaming and non-streaming shapes.

Usage: python pool_probe.py BASE_URL ARM [PROMPT_TOKENS] [MAX_TOKENS]
  ARM: seq | par | park | parkpar
"""
import json
import sys
import threading
import time

import requests

BASE = sys.argv[1].rstrip("/")
ARM = sys.argv[2]
PROMPT_WORDS = int(sys.argv[3]) if len(sys.argv) > 3 else 900
MAX_TOKENS = int(sys.argv[4]) if len(sys.argv) > 4 else 400

WORDS = ("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi "
         "omicron pi rho sigma tau upsilon phi chi psi omega").split()

# The three `timings` fields the probe reads: how much of the prompt the server
# actually processed, how much of it came from the cache, and how far the reply
# got before the pool ended it.
TIMING_KEYS = ("prompt_n", "cache_n", "predicted_n")


def filler(tag, n_words):
    # Distinct per conversation (the tag is in every line) so no prefix is
    # shared between A and B and the cache cannot help either.
    out = []
    i = 0
    while len(out) < n_words:
        out.append(f"{tag}{i}-{WORDS[i % len(WORDS)]}")
        i += 1
    return " ".join(out)


def body(tag, stream):
    return {
        "model": "x",
        "messages": [
            {"role": "system", "content": f"You are conversation {tag}. Reply at length."},
            {"role": "user", "content": filler(tag, PROMPT_WORDS)
             + "\n\nWrite a long story about a lighthouse keeper; do not stop early."},
        ],
        "max_tokens": MAX_TOKENS,
        "temperature": 0.0,
        "stream": stream,
        "stream_options": {"include_usage": True} if stream else None,
    }


def request_body(tag, stream, slot):
    """`body`, minus the field a non-streaming request must not carry, plus the
    slot pin when the arm asks for one."""
    b = body(tag, stream)
    if b["stream_options"] is None:
        del b["stream_options"]
    if slot is not None:
        b["id_slot"] = slot
    return b


def timings(src):
    """llama.cpp's `timings`, narrowed to the fields recorded."""
    return {k: src.get(k) for k in TIMING_KEYS}


def read_plain(rec, r):
    """The non-streaming answer: one JSON document, error envelope included."""
    j = r.json()
    rec["error"] = j.get("error")
    ch = (j.get("choices") or [{}])[0]
    rec["finish"] = ch.get("finish_reason")
    rec["content_len"] = len((ch.get("message") or {}).get("content") or "")
    rec["usage"] = j.get("usage")
    rec["timings"] = timings(j.get("timings", {}))


def merge_chunk(rec, j):
    """What one `data:` chunk contributes: the finish reason of any choice
    carrying one, and the last `usage`/`timings` the stream reported."""
    for ch in j.get("choices") or []:
        if ch.get("finish_reason"):
            rec["finish"] = ch["finish_reason"]
    if j.get("usage"):
        rec["usage"] = j["usage"]
    if j.get("timings"):
        rec["timings"] = timings(j["timings"])


def read_stream(rec, r):
    """The SSE answer: `data:` chunks until `[DONE]`. An overflow arrives as an
    error envelope *in band* — a chunk of its own, counted so the record says
    how far the reply got first — and anything that is not a `data:` line is
    kept verbatim rather than discarded."""
    n = 0
    for line in r.iter_lines():
        if not line:
            continue
        s = line.decode()
        if not s.startswith("data: "):
            rec.setdefault("other_lines", []).append(s[:200])
            continue
        d = s[6:]
        if d == "[DONE]":
            break
        j = json.loads(d)
        if "error" in j:
            rec["error"] = j["error"]
            rec["error_after_chunks"] = n
            continue
        n += 1
        merge_chunk(rec, j)
    rec["chunks"] = n


def one(tag, stream, results, slot=None):
    t0 = time.time()
    rec = {"tag": tag, "stream": stream}
    try:
        r = requests.post(f"{BASE}/v1/chat/completions",
                          json=request_body(tag, stream, slot), stream=stream, timeout=600)
        rec["status"] = r.status_code
        if stream:
            read_stream(rec, r)
        else:
            read_plain(rec, r)
    except Exception as e:  # noqa: BLE001
        rec["exception"] = repr(e)[:300]
    rec["wall_s"] = round(time.time() - t0, 2)
    results.append(rec)


def run_pair(stream):
    res = []
    if ARM in ("seq",):
        one("A", stream, res)
        one("B", stream, res)
    else:
        ts = [threading.Thread(target=one, args=(t, stream, res)) for t in ("A", "B")]
        for t in ts:
            t.start()
        for t in ts:
            t.join()
    return res


def main():
    print(json.dumps({"arm": ARM, "prompt_words": PROMPT_WORDS, "max_tokens": MAX_TOKENS,
                      "props": {k: v for k, v in requests.get(f"{BASE}/props").json().items()
                                if k in ("total_slots", "default_generation_settings", "build_info")}}))
    if ARM in ("park", "parkpar"):
        # A "parent" conversation P that runs first, then idles (parked into
        # the RAM prompt cache under the unified pool), then returns after the
        # hazard: does its cache survive the siblings' failure?
        res = []
        one("P", False, res)
        print("parent first:", json.dumps(res))
        if ARM == "parkpar":
            pair = run_pair(False)
        else:
            # `park`: one sibling rather than a colliding pair. `one` records
            # into the list it is given and returns nothing, so the list is
            # what gets printed -- echoing the call itself printed `[null]`.
            pair = []
            one("A", False, pair)
        print("pair:", json.dumps(pair))
        res = []
        one("P", False, res)
        print("parent again:", json.dumps(res))
        return
    for stream in (False, True):
        print(("stream" if stream else "plain") + ":", json.dumps(run_pair(stream)))
        time.sleep(1)


main()
