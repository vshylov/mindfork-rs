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


def one(tag, stream, results, slot=None):
    t0 = time.time()
    b = body(tag, stream)
    if b["stream_options"] is None:
        del b["stream_options"]
    if slot is not None:
        b["id_slot"] = slot
    rec = {"tag": tag, "stream": stream}
    try:
        r = requests.post(f"{BASE}/v1/chat/completions", json=b, stream=stream, timeout=600)
        rec["status"] = r.status_code
        if not stream:
            j = r.json()
            rec["error"] = j.get("error")
            ch = (j.get("choices") or [{}])[0]
            rec["finish"] = ch.get("finish_reason")
            rec["content_len"] = len((ch.get("message") or {}).get("content") or "")
            rec["usage"] = j.get("usage")
            rec["timings"] = {k: j.get("timings", {}).get(k) for k in ("prompt_n", "cache_n", "predicted_n")}
        else:
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
                for ch in j.get("choices") or []:
                    if ch.get("finish_reason"):
                        rec["finish"] = ch["finish_reason"]
                if j.get("usage"):
                    rec["usage"] = j["usage"]
                if j.get("timings"):
                    rec["timings"] = {k: j["timings"].get(k) for k in ("prompt_n", "cache_n", "predicted_n")}
            rec["chunks"] = n
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
        pair = run_pair(False) if ARM == "parkpar" else [one("A", False, [])]
        print("pair:", json.dumps(pair))
        res = []
        one("P", False, res)
        print("parent again:", json.dumps(res))
        return
    for stream in (False, True):
        print(("stream" if stream else "plain") + ":", json.dumps(run_pair(stream)))
        time.sleep(1)


main()
