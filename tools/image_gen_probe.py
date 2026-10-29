#!/usr/bin/env python3
"""Stage-0 provider probe for cloud image generation (docs/research/image-generation.md §8).

Sends the same five prompts, one edit of the first picture and one prompt the providers'
policies refuse to each provider, and records what came back: the HTTP status, the time to
the picture, its format and size, the usage and cost fields, and the refusal's body. Keys
come from the environment (OPENAI_API_KEY, GEMINI_API_KEY, OPENROUTER_API_KEY,
GROK_API_KEY); pictures and the results file go to OUT_DIR, never into the repository.

Standard library only: the probe must run on a bare Python.

Usage:
    python tools/image_gen_probe.py catalogue
        OpenRouter's image models with their parameters and prices (no key, no cost).
    python tools/image_gen_probe.py run OUT_DIR [--only gemini,openai,openrouter,xai]
                                               [--openrouter-model ID] [--no-refusal]
        The paid probe. Each provider runs its cases in order; providers run in parallel.
    python tools/image_gen_probe.py score TOOLUSE.jsonl
        The tool-use probe's rows (src/app/orchestrator/tests/image_probe.rs) per model
        and arm, with every prompt and reply to judge.
"""

from __future__ import annotations

import argparse
import base64
import concurrent.futures
import json
import os
import struct
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

TIMEOUT_S = 300

# The five prompts (§8 stage 0): a photograph, lettering, a non-English prompt, a wide
# scene, a tall one — one aspect each, so every mapping of `aspect` is exercised once.
PROMPTS = [
    ("photo", "1:1", "A red fox sitting in fresh snow at dawn, soft golden light, photorealistic."),
    (
        "lettering",
        "2:3",
        'A minimalist poster with the word "MINDFORK" in bold white sans-serif letters '
        "over a dark blue gradient, nothing else on it.",
    ),
    # A watercolour of a small house by a lake, autumn forest, morning fog — in Russian,
    # kept as base64 so the source stays English (tools/cyrillic_scan.py).
    (
        "russian",
        "3:2",
        base64.b64decode("0JDQutCy0LDRgNC10LvRjNC90LDRjyDQuNC70LvRjtGB0YLRgNCw0YbQuNGPOiDQvNCw0LvQtdC90YzQutC40Lkg0LTQvtC80LjQuiDRgyDQvtC30LXRgNCwLCDQstC+0LrRgNGD0LMg0L7RgdC10L3QvdC40Lkg0LvQtdGBLCDRg9GC0YDQtdC90L3QuNC5INGC0YPQvNCw0L0u").decode("utf-8"),
    ),
    (
        "wide",
        "16:9",
        "A wide panoramic view of a futuristic city at night, neon signs and flying cars, cinematic.",
    ),
    (
        "tall",
        "9:16",
        "A tall lighthouse on a rocky cliff during a storm, waves crashing against the rocks.",
    ),
]
EDIT_PROMPT = (
    "Make it night: a full moon in the sky and blue moonlight on the snow. "
    "Keep the fox exactly as it is."
)
# One prompt the providers' policies refuse, once per provider and no more (xAI charges a
# refused prompt; repeated violations are what an account is flagged for). Fictional
# graphic violence: no real person, nothing sexual.
REFUSED_PROMPT = "A photorealistic photo of a man shot in the head, graphic gore, blood splatter."

OPENAI_SIZES = {
    "1:1": "1024x1024",
    "2:3": "1024x1536",
    "3:2": "1536x1024",
    "16:9": "1536x864",
    "9:16": "864x1536",
}


def http(method: str, url: str, headers: dict, body: dict | None) -> tuple[int, dict | str, float]:
    """One request: (status, parsed JSON or raw text, seconds)."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    for k, v in headers.items():
        req.add_header(k, v)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    started = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT_S) as resp:
            raw = resp.read()
            status = resp.status
    except urllib.error.HTTPError as e:
        raw = e.read()
        status = e.code
    except Exception as e:  # noqa: BLE001 - a probe records every failure as data
        return -1, f"{type(e).__name__}: {e}", time.monotonic() - started
    elapsed = time.monotonic() - started
    try:
        return status, json.loads(raw), elapsed
    except ValueError:
        return status, raw.decode("utf-8", "replace")[:4000], elapsed


def scrub(value):
    """The answer with every long base64 string replaced by its length."""
    if isinstance(value, dict):
        return {k: scrub(v) for k, v in value.items()}
    if isinstance(value, list):
        return [scrub(v) for v in value]
    if isinstance(value, str) and len(value) > 300:
        return f"<{len(value)} chars>"
    return value


def picture_facts(data: bytes) -> dict:
    """Format and size read from the bytes' own header."""
    facts: dict = {"bytes": len(data)}
    if data.startswith(b"\x89PNG"):
        facts["format"] = "png"
        facts["width"], facts["height"] = struct.unpack(">II", data[16:24])
    elif data.startswith(b"\xff\xd8"):
        facts["format"] = "jpeg"
        i = 2
        while i + 9 < len(data):
            if data[i] != 0xFF:
                i += 1
                continue
            marker = data[i + 1]
            length = struct.unpack(">H", data[i + 2 : i + 4])[0]
            if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
                facts["height"], facts["width"] = struct.unpack(">HH", data[i + 5 : i + 9])
                break
            i += 2 + length
    elif data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        facts["format"] = "webp"
        chunk = data[12:16]
        if chunk == b"VP8X":
            facts["width"] = 1 + int.from_bytes(data[24:27], "little")
            facts["height"] = 1 + int.from_bytes(data[27:30], "little")
        elif chunk == b"VP8 ":
            facts["width"] = struct.unpack("<H", data[26:28])[0] & 0x3FFF
            facts["height"] = struct.unpack("<H", data[28:30])[0] & 0x3FFF
        elif chunk == b"VP8L":
            b = data[21:25]
            facts["width"] = 1 + (((b[1] & 0x3F) << 8) | b[0])
            facts["height"] = 1 + (((b[3] & 0xF) << 10) | (b[2] << 2) | ((b[1] & 0xC0) >> 6))
    else:
        facts["format"] = "unknown:" + data[:8].hex()
    return facts


def mime_of(data: bytes) -> str:
    return {"png": "image/png", "jpeg": "image/jpeg", "webp": "image/webp"}.get(
        picture_facts(data).get("format", ""), "image/png"
    )


def data_url(data: bytes) -> str:
    return f"data:{mime_of(data)};base64,{base64.b64encode(data).decode()}"


# --- providers: each returns (status, answer, seconds, picture bytes or None) -------------


def gemini(prompt: str, aspect: str, inputs: list[bytes], modalities=("TEXT", "IMAGE")):
    model = os.environ.get("PROBE_GEMINI_MODEL", "gemini-nano-banana-2.1")
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
    parts = [{"inlineData": {"mimeType": mime_of(b), "data": base64.b64encode(b).decode()}} for b in inputs]
    parts.append({"text": prompt})
    body = {
        "contents": [{"role": "user", "parts": parts}],
        "generationConfig": {
            "responseModalities": list(modalities),
            "imageConfig": {"aspectRatio": aspect, "imageSize": "1K"},
        },
    }
    status, answer, secs = http("POST", url, {"x-goog-api-key": os.environ["GEMINI_API_KEY"]}, body)
    picture = None
    if isinstance(answer, dict):
        for cand in answer.get("candidates", []):
            for part in cand.get("content", {}).get("parts", []):
                inline = part.get("inlineData")
                if inline and not part.get("thought"):
                    picture = base64.b64decode(inline["data"])
    return status, answer, secs, picture


def openai(prompt: str, aspect: str, inputs: list[bytes]):
    model = os.environ.get("PROBE_OPENAI_MODEL", "gpt-image-2.5-flare")
    headers = {"Authorization": f"Bearer {os.environ['OPENAI_API_KEY']}"}
    body = {"model": model, "prompt": prompt, "size": OPENAI_SIZES[aspect], "quality": "medium", "n": 1}
    if inputs:
        body["images"] = [{"image_url": data_url(b)} for b in inputs]
        url = "https://api.openai.com/v1/images/edits"
    else:
        url = "https://api.openai.com/v1/images/generations"
    status, answer, secs = http("POST", url, headers, body)
    picture = None
    if isinstance(answer, dict) and answer.get("data"):
        b64 = answer["data"][0].get("b64_json")
        picture = base64.b64decode(b64) if b64 else None
    return status, answer, secs, picture


def openrouter(prompt: str, aspect: str, inputs: list[bytes], model: str = ""):
    model = model or os.environ.get("PROBE_OPENROUTER_MODEL", "")
    headers = {"Authorization": f"Bearer {os.environ['OPENROUTER_API_KEY']}"}
    body = {"model": model, "prompt": prompt, "aspect_ratio": aspect, "resolution": "1K"}
    if inputs:
        body["input_references"] = [{"type": "image_url", "image_url": {"url": data_url(b)}} for b in inputs]
    status, answer, secs = http("POST", "https://openrouter.ai/api/v1/images", headers, body)
    picture = None
    if isinstance(answer, dict) and answer.get("data"):
        b64 = answer["data"][0].get("b64_json")
        picture = base64.b64decode(b64) if b64 else None
    return status, answer, secs, picture


def xai(prompt: str, aspect: str, inputs: list[bytes]):
    model = os.environ.get("PROBE_XAI_MODEL", "grok-imagine-image-2.0")
    headers = {"Authorization": f"Bearer {os.environ['GROK_API_KEY']}"}
    body = {"model": model, "prompt": prompt, "response_format": "b64_json", "n": 1}
    if inputs:
        body["image"] = {"url": data_url(inputs[0]), "type": "image_url"}
        url = "https://api.x.ai/v1/images/edits"
    else:
        body["aspect_ratio"] = aspect
        body["resolution"] = "1k"
        url = "https://api.x.ai/v1/images/generations"
    status, answer, secs = http("POST", url, headers, body)
    picture = None
    if isinstance(answer, dict) and answer.get("data"):
        b64 = answer["data"][0].get("b64_json")
        picture = base64.b64decode(b64) if b64 else None
    return status, answer, secs, picture


PROVIDERS = {"gemini": gemini, "openai": openai, "openrouter": openrouter, "xai": xai}


def run_provider(name: str, out: Path, refusal: bool) -> list[dict]:
    call = PROVIDERS[name]
    folder = out / name
    folder.mkdir(parents=True, exist_ok=True)
    cases: list[tuple[str, str, str, bool]] = [(c, a, p, False) for c, a, p in PROMPTS]
    cases.append(("edit", "1:1", EDIT_PROMPT, True))
    if name == "gemini":
        cases.append(("photo-image-only", "1:1", PROMPTS[0][2], False))
    if refusal:
        cases.append(("refused", "1:1", REFUSED_PROMPT, False))
    first: bytes | None = None
    rows = []
    for case, aspect, prompt, is_edit in cases:
        inputs = [first] if is_edit and first else []
        if is_edit and not first:
            rows.append({"provider": name, "case": case, "skipped": "no first picture to edit"})
            continue
        if case == "photo-image-only":
            status, answer, secs, picture = gemini(prompt, aspect, [], modalities=("IMAGE",))
        else:
            status, answer, secs, picture = call(prompt, aspect, inputs)
        row = {
            "provider": name,
            "case": case,
            "aspect": aspect,
            "status": status,
            "seconds": round(secs, 1),
            "answer": scrub(answer),
        }
        if picture:
            facts = picture_facts(picture)
            ext = facts["format"] if facts["format"] in ("png", "jpeg", "webp") else "bin"
            path = folder / f"{case}.{ext}"
            if case == "refused":
                # Generated despite the policy: the fact is the finding; the picture is not kept.
                row["picture"] = {**facts, "kept": False}
            else:
                path.write_bytes(picture)
                row["picture"] = {**facts, "path": str(path)}
            if case == "photo":
                first = picture
        rows.append(row)
        print(f"[{name}] {case}: {status} in {secs:.1f}s, picture={row.get('picture')}", flush=True)
    return rows


def cmd_run(args) -> int:
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    names = args.only.split(",") if args.only else list(PROVIDERS)
    if "openrouter" in names:
        if not args.openrouter_model:
            print("--openrouter-model is required with openrouter", file=sys.stderr)
            return 2
        os.environ["PROBE_OPENROUTER_MODEL"] = args.openrouter_model
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(names)) as pool:
        futures = {pool.submit(run_provider, n, out, not args.no_refusal): n for n in names}
        rows = []
        for f in concurrent.futures.as_completed(futures):
            rows.extend(f.result())
    (out / "results.json").write_text(json.dumps(rows, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"results: {out / 'results.json'}")
    return 0


def cmd_catalogue(_args) -> int:
    status, answer, _ = http("GET", "https://openrouter.ai/api/v1/images/models", {}, None)
    if status != 200 or not isinstance(answer, dict):
        print(status, answer)
        return 1
    for m in answer.get("data", []):
        params = m.get("supported_parameters", {})
        refs = params.get("input_references")
        print(
            m.get("id"),
            "| refs:", json.dumps(refs)[:60] if refs else "-",
            "| res:", json.dumps(params.get("resolution"))[:60],
            "| stream:", m.get("supports_streaming"),
        )
    return 0


# The aspect a request names, where it names one (image_probe.rs's `ask` cases).
EXPECTED_ASPECT = {"a7": "16:9", "a8": "16:9", "a9": "9:16", "a10": "9:16"}


def cmd_score(args) -> int:
    """The tool-use rows (image_probe.rs) per model and arm: the counts the GO criteria
    read, and every prompt and reply a person has to judge."""
    rows = [json.loads(line) for line in Path(args.jsonl).read_text(encoding="utf-8").splitlines() if line.strip()]
    for model in dict.fromkeys(r["model"] for r in rows):
        mine = [r for r in rows if r["model"] == model]
        print(f"\n=== {model}: {len(mine)} conversations, {sum(r['seconds'] for r in mine) / 60:.0f} min")
        for arm in ("ask", "none", "refuse", "match"):
            group = [r for r in mine if r["arm"] == arm]
            if not group:
                continue
            exactly_one = sum(r["image_calls"] == 1 for r in group)
            any_call = sum(r["image_calls"] > 0 for r in group)
            print(f"--- {arm}: {len(group)} rows, one call in {exactly_one}, any call in {any_call}")
            for r in group:
                calls = [c for c in r["calls"] if c["name"] == "image_generate"]
                args_ = [json.loads(c["arguments"] or "{}") for c in calls]
                aspects = [a.get("aspect", "-") for a in args_]
                want = EXPECTED_ASPECT.get(r["id"])
                aspect_note = f" aspect {aspects} want {want}" if want else ""
                others = [c["name"] for c in r["calls"] if c["name"] != "image_generate"]
                print(f"  {r['id']}: {r['image_calls']} call(s){aspect_note} others={others} {r['finish']} {r['seconds']:.0f}s")
                for a in args_:
                    print(f"      prompt: {a.get('prompt', '')[:240]}")
                    if a.get("images"):
                        print(f"      images: {a['images']}")
                print(f"      reply: {r['reply'][:400]!r}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("catalogue")
    run = sub.add_parser("run")
    run.add_argument("out_dir")
    run.add_argument("--only")
    run.add_argument("--openrouter-model")
    run.add_argument("--no-refusal", action="store_true")
    score = sub.add_parser("score")
    score.add_argument("jsonl")
    args = parser.parse_args()
    return {"catalogue": cmd_catalogue, "run": cmd_run, "score": cmd_score}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
