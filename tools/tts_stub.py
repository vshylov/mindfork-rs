#!/usr/bin/env python3
"""A speech server that needs no model and no key — for checking `/tts`.

The `external` speech mode (spec §11.9) speaks to any OpenAI-compatible TTS
server. This one stands in for such a server. It answers `POST
/v1/audio/speech` with a two-note tone instead of a voice:

- `wav`, which is what the `external` mode asks for;
- or raw `pcm`, 24 kHz s16le mono, as the OpenAI cloud sends it.

Any other `response_format` is refused with a 400 that names the parameter, as
a real server's refusal would. `GET /v1/models` lists one model. Every request
is printed: the text's length, the voice and the format. So a session can see
what the app asked for, even where the sound itself cannot be heard — a
rented Mac over VNC (docs/research/macos.md §13.4), a container, a CI box.

The tone lasts about as long as the text would take to read: about 15
characters a second, between 0.4 and 6 seconds. The playback queue therefore
gets fragments of different lengths, as it does from a real voice.

It listens on 127.0.0.1 only.

Usage:
    python3 tools/tts_stub.py                 # http://127.0.0.1:8880/v1
    python3 tools/tts_stub.py --port 9000
    python3 tools/tts_stub.py --self-test

Point the app at it: Ctrl+P, then Model, then the Speech tab — mode `external`,
URL `http://127.0.0.1:8880/v1`. Or:

    mindfork setup --set tts.mode=external --set tts.external.url=http://127.0.0.1:8880/v1

The live smoke of the external mode runs against it too:

    MINDFORK_TTS_URL=http://127.0.0.1:8880/v1 cargo test external_server_synthesizes_live -- --ignored
"""

from __future__ import annotations

import argparse
import io
import json
import math
import struct
import sys
import threading
import urllib.error
import urllib.request
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
DEFAULT_PORT = 8880
SPEECH_PATH = "/v1/audio/speech"
MODELS_PATH = "/v1/models"
MODEL_NAME = "tone"
FORMAT_FIELD = "response_format"
CONTENT_TYPE = "Content-Type"
JSON_TYPE = "application/json"

# OpenAI's raw PCM: 24 kHz, signed 16-bit little-endian, mono — what the app
# assumes for `pcm` (shared/tts/openai.rs, OPENAI_PCM_RATE).
RATE = 24_000
CHARS_PER_SECOND = 15
SHORTEST, LONGEST = 0.4, 6.0
# Two notes, A4 then E5, with a short fade at each end so playback does not click.
NOTES = (440.0, 659.25)
FADE = 0.01
LEVEL = 0.2


def seconds_for(text: str) -> float:
    """How long the tone for `text` lasts."""
    return min(LONGEST, max(SHORTEST, len(text) / CHARS_PER_SECOND))


def tone(seconds: float) -> bytes:
    """Raw samples: s16le mono at `RATE`, half the time on each note."""
    total = int(seconds * RATE)
    fade = max(1, int(FADE * RATE))
    half = total // 2
    out = bytearray()
    for i in range(total):
        note = NOTES[0] if i < half else NOTES[1]
        # Each note fades in and out on its own, so the step between them is quiet.
        start = 0 if i < half else half
        end = half if i < half else total
        envelope = min(1.0, (i - start) / fade, (end - i) / fade)
        sample = LEVEL * envelope * math.sin(2 * math.pi * note * (i - start) / RATE)
        out += struct.pack("<h", int(sample * 32767))
    return bytes(out)


def as_wav(samples: bytes) -> bytes:
    """The samples in a WAV container."""
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(samples)
    return buffer.getvalue()


def speech(body: dict) -> tuple[int, str, bytes]:
    """The answer to one speech request: status, content type, body."""
    text = body.get("input")
    if not isinstance(text, str) or not text:
        return 400, JSON_TYPE, refusal("`input` must be a non-empty string")
    fmt = body.get(FORMAT_FIELD, "mp3")
    samples = tone(seconds_for(text))
    if fmt == "wav":
        return 200, "audio/wav", as_wav(samples)
    if fmt == "pcm":
        return 200, f"audio/pcm;rate={RATE};channels=1", samples
    return 400, JSON_TYPE, refusal(f"`{FORMAT_FIELD}` must be wav or pcm, not {fmt!r}")


def refusal(message: str) -> bytes:
    """An error body in the OpenAI shape."""
    return json.dumps({"error": {"message": message, "type": "invalid_request_error"}}).encode()


class Handler(BaseHTTPRequestHandler):
    server_version = "tts_stub"

    def do_GET(self) -> None:  # noqa: N802 - the name http.server dispatches to
        if self.path.rstrip("/") == MODELS_PATH:
            listing = {"object": "list", "data": [{"id": MODEL_NAME, "object": "model"}]}
            self.answer(200, JSON_TYPE, json.dumps(listing).encode())
        else:
            self.answer(404, JSON_TYPE, refusal(f"no route {self.path}"))

    def do_POST(self) -> None:  # noqa: N802 - the name http.server dispatches to
        if self.path.rstrip("/") != SPEECH_PATH:
            self.answer(404, JSON_TYPE, refusal(f"no route {self.path}"))
            return
        length = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(length) or b"{}")
        except json.JSONDecodeError:
            self.answer(400, JSON_TYPE, refusal("the body is not JSON"))
            return
        if not isinstance(body, dict):
            self.answer(400, JSON_TYPE, refusal("the body is not a JSON object"))
            return
        status, kind, payload = speech(body)
        text = body.get("input") if isinstance(body.get("input"), str) else ""
        print(
            f"speech: {len(text)} chars, voice={body.get('voice')!r}, "
            f"model={body.get('model')!r}, {FORMAT_FIELD}={body.get(FORMAT_FIELD)!r} "
            f"-> {status}, {len(payload)} bytes",
            flush=True,
        )
        self.answer(status, kind, payload)

    def answer(self, status: int, kind: str, payload: bytes) -> None:
        self.send_response(status)
        self.send_header(CONTENT_TYPE, kind)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002 - the base signature
        """The per-request line above says more; the access log is dropped."""


def serve(port: int) -> ThreadingHTTPServer:
    return ThreadingHTTPServer((HOST, port), Handler)


# ---------------------------------------------------------------- self-test


def self_test() -> int:
    """Every route against a server on a free port, in this process."""
    server = serve(0)
    base = f"http://{HOST}:{server.server_address[1]}"
    threading.Thread(target=server.serve_forever, daemon=True).start()
    failures = []

    def check(ok: bool, what: str) -> None:
        print(f"  {'ok  ' if ok else 'FAIL'}  {what}")
        if not ok:
            failures.append(what)

    def call(method: str, path: str, body: bytes | None = None) -> tuple[int, str, bytes]:
        request = urllib.request.Request(base + path, data=body, method=method)
        request.add_header(CONTENT_TYPE, JSON_TYPE)
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return response.status, response.headers.get(CONTENT_TYPE, ""), response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.headers.get(CONTENT_TYPE, ""), error.read()

    def ask(fmt: str, text: str = "Speech check, one two three.") -> tuple[int, str, bytes]:
        payload = {"model": "tts-1", "voice": "bella", "input": text, FORMAT_FIELD: fmt}
        return call("POST", SPEECH_PATH, json.dumps(payload).encode())

    try:
        status, kind, body = ask("wav")
        check(status == 200 and kind == "audio/wav", "wav is answered as audio/wav")
        with wave.open(io.BytesIO(body)) as w:
            check(
                (w.getnchannels(), w.getsampwidth(), w.getframerate()) == (1, 2, RATE),
                "the wav is mono, 16-bit, 24 kHz",
            )
            check(
                abs(w.getnframes() / RATE - seconds_for("Speech check, one two three.")) < 0.01,
                "the tone lasts as long as the text takes to read",
            )
        status, kind, body = ask("pcm")
        check(status == 200 and "rate=24000" in kind, "pcm is labelled with its rate")
        check(len(body) % 2 == 0 and len(body) > 1000, "pcm is whole 16-bit samples")
        peak = max(abs(s) for (s,) in struct.iter_unpack("<h", body))
        check(0 < peak <= int(LEVEL * 32767) + 1, "the tone is audible and not clipped")
        check(
            abs(struct.unpack("<h", body[:2])[0]) < 100,
            "the tone starts from silence (no click)",
        )
        status, _, body = ask("mp3")
        check(status == 400 and FORMAT_FIELD.encode() in body, "mp3 is refused, naming the field")
        status, _, _ = ask("wav", text="")
        check(status == 400, "an empty input is refused")
        status, _, _ = call("POST", SPEECH_PATH, b"not json")
        check(status == 400, "a body that is not JSON is refused")
        status, _, _ = call("POST", SPEECH_PATH, b"[1, 2]")
        check(status == 400, "a body that is not an object is refused")
        status, _, body = call("GET", MODELS_PATH)
        check(
            status == 200 and json.loads(body)["data"][0]["id"] == MODEL_NAME,
            "the model list names the tone",
        )
        status, _, _ = call("GET", "/v1/voices")
        check(status == 404, "an unknown route is a 404")
        check(seconds_for("x" * 1000) == LONGEST, "a long text is capped")
        check(seconds_for("x") == SHORTEST, "a short text still sounds")
    finally:
        server.shutdown()
    print(f"tts_stub self-test: {'all checks passed' if not failures else f'{len(failures)} FAILED'}")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description="A tone-speaking OpenAI-compatible TTS server.")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT, help="port on 127.0.0.1")
    parser.add_argument("--self-test", action="store_true", help="run the checks and exit")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    server = serve(args.port)
    print(f"tts_stub: http://{HOST}:{args.port}/v1 — Ctrl+C stops it", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
