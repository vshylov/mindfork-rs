#!/usr/bin/env python3
"""The headless half of the rented Mac day (docs/research/macos.md §13.5).

It runs on the Mac, over SSH. The screen half (§13.6) is a person at the
keyboard. What this half answers can be measured, so it is measured here.

    python3 mac_probe.py [DIR]               # every step; DIR = ~/mindfork-probe
    python3 mac_probe.py DIR --only M4,M5    # some steps
    python3 mac_probe.py --self-test         # the parts that need no Mac, anywhere

It is written for the python3 of Apple's Command Line Tools (3.9) and uses the
standard library only. It writes one report, `DIR/mac-probe-report.txt`, which
each run appends to, and a log per command in `DIR/logs/`. It is shaped like
`tools/pod_probe.sh`:

1. **No step stops the next.** A failed step is written down and the next one
   runs: the host is paid for by the hour.
2. **Nothing secret reaches the report.** The environment is never dumped. The
   hardware UUID is the key material of the `platform-uuid-v1` secret scheme
   (`shared/secrets.rs`), so it is reported only as a hash prefix.
3. **Everything lands under DIR**, except what a step exists to measure:
   - the tap's formula and Ollama (Homebrew);
   - iTerm2 and Ghostty, for the screen half;
   - rustup's `~/.cargo`.

The steps:

- **M0 the host**: macOS, the chip, memory, the GPU and Metal, the disk, the
  tools present.
- **M1 the install routes**:
  - `install.sh` into DIR/portable, its link measured and then removed so
    that Homebrew's can take the name;
  - `brew install vshylov/tap/mindfork`;
  - `cargo install --locked mindfork`, after rustup;
  - the two terminals as casks.
- **M2 the managed engine on Metal**: the GGUFs; `setup --sandbox --llama metal
  … --verify` with the portable build; `llama-bench`.
- **M3 the live gate**: a checkout of `main`, `cargo build` (which gives
  `mindfork keys` for the screen half), `cargo test`, then the ignored smokes.
  They run against two llama-servers of M2's build and `tools/tts_stub.py`.
- **M4 Ollama**: started if nothing answers on :11434, the model pulled, a
  request made, and the window `/api/ps` reports read. It is left running for
  the screen half.
- **M5 LM Studio**: the app must be installed and opened once, which is the
  owner's to do over VNC. Then:
  - each model `/api/v1/models` lists — a GGUF and an MLX build;
  - each LLM loaded at a 4096 window;
  - a last message over that window, and a conversation over it.

  The answer is a refusal, a cut, or neither.

M4 and M5 speak only HTTP, so either can be run against any machine's servers.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = "vshylov/mindfork-rs"
INSTALL_SH = f"https://github.com/{REPO}/releases/latest/download/install.sh"
CLONE_URL = f"https://github.com/{REPO}.git"
FORMULA = "vshylov/tap/mindfork"
CASKS = ("iterm2", "ghostty")
HF = "https://huggingface.co"
GEMMA = f"{HF}/bartowski/google_gemma-4-E4B-it-GGUF/resolve/main"
DOWNLOADS = {
    # The chat model the Windows stand measures with, its projector, the embedder.
    "chat": f"{GEMMA}/google_gemma-4-E4B-it-Q4_1.gguf",
    "mmproj": f"{GEMMA}/mmproj-google_gemma-4-E4B-it-f16.gguf",
    "embed": f"{HF}/ggml-org/bge-m3-Q8_0-GGUF/resolve/main/bge-m3-q8_0.gguf",
}
OLLAMA = "http://127.0.0.1:11434"
OLLAMA_VERSION = f"{OLLAMA}/api/version"
OLLAMA_MODEL = "gemma4:e4b"
LM_STUDIO = "http://127.0.0.1:1234"
LM_MODELS = f"{LM_STUDIO}/api/v1/models"
CHAT_PORT, EMBED_PORT, TTS_PORT = 8000, 8001, 8880
LOCALHOST = "http://127.0.0.1"
# The window LM Studio's overflow is measured at, as on the Windows stand
# (docs/research/local-servers.md §2).
WINDOW = 4096
STEPS = ("M0", "M1", "M2", "M3", "M4", "M5")
REPORT_NAME = "mac-probe-report.txt"
CHAT_COMPLETIONS = "/v1/chat/completions"
JSON_TYPE = "application/json"
FILLER = "The lighthouse keeper counted the boats that came into the harbour. "
CODE_WORD = "LANTERN"
MISSING = 127


# ---------------------------------------------------------------- the report


class Probe:
    """The report, the logs and the commands, under one directory."""

    def __init__(self, root: Path):
        self.root = root
        self.logs = root / "logs"
        self.logs.mkdir(parents=True, exist_ok=True)
        self.report = root / REPORT_NAME

    def say(self, text: str = "") -> None:
        print(text, flush=True)
        with open(self.report, "a", encoding="utf-8") as out:
            out.write(text + "\n")

    def section(self, title: str) -> None:
        self.say()
        self.say(f"== {title}")

    def run(
        self,
        name: str,
        args: list[str] | str,
        *,
        env: dict | None = None,
        cwd: Path | None = None,
        timeout: float | None = None,
        tail: int = 12,
    ) -> int:
        """Runs a command into `logs/<name>.log`; its last lines and its exit
        code go into the report. A string is run by the shell (a pipe)."""
        log = self.logs / f"{name}.log"
        shown = args if isinstance(args, str) else " ".join(args)
        self.say(f"$ {shown}   (logs/{log.name})")
        started = time.monotonic()
        try:
            with open(log, "w", encoding="utf-8", errors="replace") as out:
                code = subprocess.run(
                    args,
                    shell=isinstance(args, str),
                    stdout=out,
                    stderr=subprocess.STDOUT,
                    env=env,
                    cwd=cwd,
                    timeout=timeout,
                ).returncode
        except FileNotFoundError:
            code = MISSING
            log.write_text("not found\n", encoding="utf-8")
        except subprocess.TimeoutExpired:
            code = -1
            with open(log, "a", encoding="utf-8") as out:
                out.write(f"\n[timed out after {timeout:.0f}s]\n")
        for line in tail_lines(log, tail):
            self.say(f"   {line}")
        self.say(f"   -> exit {code} after {time.monotonic() - started:.0f}s")
        return code


def tail_lines(path: Path, count: int) -> list[str]:
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    return [line.rstrip() for line in lines[-count:]] if count else []


def capture(args: list[str], timeout: float = 60) -> str:
    """A command's output, or "" when it is missing or fails."""
    try:
        done = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return done.stdout.strip() if done.returncode == 0 else ""


def http(method: str, url: str, body: object = None, timeout: float = 300) -> tuple[int, object]:
    """Status and the parsed JSON (or the text) of one request; status 0 when
    nothing answered."""
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("Content-Type", JSON_TYPE)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status, raw = response.status, response.read()
    except urllib.error.HTTPError as error:
        status, raw = error.code, error.read()
    except OSError:  # a URLError is one: nothing answered
        return 0, None
    text = raw.decode("utf-8", errors="replace")
    try:
        return status, json.loads(text)
    except ValueError:
        return status, text


def wait_for(url: str, seconds: float) -> bool:
    """Whether `url` answers 200 within `seconds`."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if http("GET", url, timeout=5)[0] == 200:
            return True
        time.sleep(1)
    return False


def hash_prefix(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:12]


def platform_uuid(ioreg: str) -> str | None:
    """`IOPlatformUUID` out of `ioreg -rd1 -c IOPlatformExpertDevice`."""
    found = re.search(r'"IOPlatformUUID"\s*=\s*"([^"]+)"', ioreg)
    return found.group(1) if found else None


BREW_BIN, LOCAL_BIN = "/opt/homebrew/bin", "/usr/local/bin"
HOMEBREW_BINS = (BREW_BIN, "/opt/homebrew/sbin", LOCAL_BIN)


def with_homebrew(path: str, exists=Path) -> str:
    """`PATH` with Homebrew's directories first. A command sent over SSH runs
    with the system's bare `PATH`, without them — `brew` and what it installs
    would be "not found" — since only a login shell's profile adds them."""
    present = path.split(os.pathsep) if path else []
    missing = [d for d in HOMEBREW_BINS if d not in present and exists(d).is_dir()]
    return os.pathsep.join(missing + present)


def cargo_env() -> dict:
    """The environment with rustup's cargo first on PATH."""
    env = dict(os.environ)
    env["PATH"] = f"{Path.home() / '.cargo' / 'bin'}{os.pathsep}{env.get('PATH', '')}"
    return env


def ensure_rust(probe: Probe) -> bool:
    """rustup's toolchain, installed if missing (no password needed)."""
    if shutil.which("cargo", path=cargo_env()["PATH"]):
        return True
    probe.run(
        "rustup",
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal",
        timeout=1200,
    )
    return shutil.which("cargo", path=cargo_env()["PATH"]) is not None


def download(probe: Probe, root: Path) -> dict[str, Path]:
    """The GGUFs under DIR/models, resumed if a run was cut short."""
    models = root / "models"
    models.mkdir(exist_ok=True)
    paths = {}
    for name, url in DOWNLOADS.items():
        path = models / url.rsplit("/", 1)[1]
        if not path.is_file():
            probe.run(
                f"download-{name}",
                ["curl", "-fL", "--retry", "10", "--retry-all-errors", "-C", "-",
                 "--no-progress-meter", "-o", str(path), url],
                tail=2,
            )
        size = path.stat().st_size if path.is_file() else 0
        probe.say(f"{name}: {path.name}, {size / 2**30:.2f} GiB")
        paths[name] = path
    return paths


def llama_tool(root: Path, name: str) -> Path | None:
    """A binary of the llama.cpp build `llama setup` put under the portable data root."""
    found = sorted((root / "portable" / "data" / "llama").glob(f"*/{name}"))
    return found[-1] if found else None


# ---------------------------------------------------------------- M0 the host


def step_host(probe: Probe, root: Path) -> None:
    probe.section("M0 the host")
    probe.say(f"date: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}")
    probe.say(f"macOS: {capture(['sw_vers', '-productVersion'])} ({capture(['sw_vers', '-buildVersion'])})")
    probe.say(f"arch: {platform.machine()}, chip: {capture(['sysctl', '-n', 'machdep.cpu.brand_string'])}")
    memory = capture(["sysctl", "-n", "hw.memsize"])
    probe.say(f"memory: {int(memory) / 2**30:.0f} GiB" if memory.isdigit() else "memory: unknown")
    display = capture(["system_profiler", "SPDisplaysDataType"], timeout=120)
    for line in display.splitlines():
        if re.search(r"Chipset Model|Total Number of Cores|Metal", line):
            probe.say(f"gpu: {line.strip()}")
    usage = shutil.disk_usage(root)
    probe.say(f"disk: {usage.free / 2**30:.0f} GiB free of {usage.total / 2**30:.0f}")
    tools = ["brew", "curl", "shasum", "python3", "git", "cargo", "ollama"]
    probe.say("tools: " + " ".join(t for t in tools if shutil.which(t)))
    uuid = platform_uuid(capture(["ioreg", "-rd1", "-c", "IOPlatformExpertDevice"]))
    probe.say(
        f"IOPlatformUUID: present, sha256 prefix {hash_prefix(uuid)}" if uuid else "IOPlatformUUID: not found"
    )
    terminals = [app for app in ("Terminal", "iTerm", "Ghostty") if Path(f"/Applications/{app}.app").exists()]
    terminals += ["Terminal"] if Path("/System/Applications/Utilities/Terminal.app").exists() else []
    probe.say(f"terminals: {', '.join(sorted(set(terminals))) or 'none found'}")


# ---------------------------------------------------------------- M1 installs


def step_install(probe: Probe, root: Path) -> None:
    probe.section("M1 the install routes")
    install_script(probe, root / "portable")
    install_tap(probe)
    install_crate(probe, root)


def install_script(probe: Probe, portable: Path) -> None:
    probe.say("-- M1a install.sh, as the README gives it, into DIR/portable")
    probe.run("install-sh", f"curl -fsSL {INSTALL_SH} | sh -s -- --dir '{portable}'", timeout=900)
    probe.run("portable-version", [str(portable / "mindfork"), "--version"], tail=1)
    for bin_dir in (BREW_BIN, LOCAL_BIN):
        link = Path(bin_dir) / "mindfork"
        if link.is_symlink():
            target = os.readlink(link)
            probe.say(f"link: {link} -> {target}")
            if target.startswith(str(portable)):
                link.unlink()
                probe.say("   removed, so that Homebrew's formula can take the name")


def install_tap(probe: Probe) -> None:
    probe.say("-- M1b the Homebrew tap")
    if not shutil.which("brew"):
        probe.say("brew: not found — the tap and the casks are skipped")
        return
    probe.run("brew-install", ["brew", "install", FORMULA], timeout=1800)
    prefix = Path(capture(["brew", "--prefix", FORMULA]) or "/nonexistent")
    defaults = prefix / "libexec" / "defaults.json"
    probe.say(f"defaults.json: {defaults.read_text().strip() if defaults.is_file() else 'MISSING'}")
    dictionaries = prefix / "libexec" / "data" / "dictionaries"
    count = len(list(dictionaries.glob("*.dic"))) if dictionaries.is_dir() else 0
    probe.say(f"dictionaries beside the binary: {count}")
    brew_app = Path(capture(["brew", "--prefix"]) or "/opt/homebrew") / "bin" / "mindfork"
    probe.run("brew-version", [str(brew_app), "--version"], tail=1)
    probe.run("brew-llama-installed", [str(brew_app), "llama", "installed"], tail=3)
    data = Path.home() / "Library" / "Application Support" / "mindfork-rs"
    probe.say(f"data root of the brew install: {data} ({'present' if data.is_dir() else 'ABSENT'})")
    probe.say("-- M1d the terminals the screen half needs")
    probe.run("casks", ["brew", "install", "--cask", *CASKS], timeout=1800, tail=4)


def install_crate(probe: Probe, root: Path) -> None:
    probe.say("-- M1c crates.io, built here")
    if ensure_rust(probe):
        probe.run(
            "cargo-install",
            ["cargo", "install", "--locked", "mindfork", "--root", str(root / "cargo-install")],
            env=cargo_env(),
            timeout=3600,
            tail=3,
        )
        probe.run("cargo-version", [str(root / "cargo-install" / "bin" / "mindfork"), "--version"], tail=1)
    else:
        probe.say("rustup did not install: skipped")


# ---------------------------------------------------------------- M2 Metal


def step_metal(probe: Probe, root: Path) -> None:
    probe.section("M2 the managed engine on Metal")
    app = root / "portable" / "mindfork"
    if not app.is_file():
        probe.say("no portable build (M1 first): skipped")
        return
    models = download(probe, root)
    probe.run(
        "setup-verify",
        [str(app), "setup", "--sandbox", "--llama", "metal",
         "--model", str(models["chat"]), "--mmproj", str(models["mmproj"]),
         "--embed-model", str(models["embed"]), "--ctx", "16384", "--verify"],
        timeout=3600,
        tail=25,
    )
    bench = llama_tool(root, "llama-bench")
    if bench:
        probe.say(f"build: {bench.parent.name}")
        probe.run("llama-bench", [str(bench), "-m", str(models["chat"]), "-ngl", "99",
                                  "-p", "512", "-n", "128"], timeout=1800, tail=8)
    else:
        probe.say("llama-bench: not in the installed build")


# ---------------------------------------------------------------- M3 the gate


def test_summary(output: str) -> tuple[dict[str, int], list[str]]:
    """The `test result:` lines summed, and the names of the failed tests."""
    totals = {"passed": 0, "failed": 0, "ignored": 0}
    for found in re.finditer(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", output):
        for key, value in zip(totals, found.groups()):
            totals[key] += int(value)
    failed = re.findall(r"^test (\S+) \.\.\. FAILED$", output, flags=re.MULTILINE)
    return totals, failed


def start(probe: Probe, name: str, args: list[str], *, detach: bool = False) -> subprocess.Popen | None:
    """A server in the background, its output in `logs/<name>.log`. A detached
    one is in a session of its own, so the end of the SSH session that started
    the probe does not take it down with it."""
    log = open(probe.logs / f"{name}.log", "w", encoding="utf-8", errors="replace")  # noqa: SIM115 - held by the child
    try:
        return subprocess.Popen(args, stdout=log, stderr=subprocess.STDOUT, start_new_session=detach)
    except OSError as error:
        probe.say(f"{name}: cannot start ({error})")
        return None


def stop(processes: list[subprocess.Popen | None]) -> None:
    for process in processes:
        if process and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()


def step_gate(probe: Probe, root: Path) -> None:
    probe.section("M3 the live gate on a Mac")
    server = llama_tool(root, "llama-server")
    models = {name: root / "models" / url.rsplit("/", 1)[1] for name, url in DOWNLOADS.items()}
    if not server or not all(path.is_file() for path in models.values()):
        probe.say("no llama-server or models (M2 first): skipped")
        return
    if not ensure_rust(probe):
        probe.say("rustup did not install: skipped")
        return
    src = root / "src"
    if (src / ".git").is_dir():
        probe.run("git-pull", ["git", "-C", str(src), "pull", "--ff-only"], tail=2)
    else:
        probe.run("git-clone", ["git", "clone", "--depth", "1", CLONE_URL, str(src)], tail=2)
    probe.say(f"commit: {capture(['git', '-C', str(src), 'log', '-1', '--format=%h %s'])}")
    env = cargo_env()
    probe.run("cargo-build", ["cargo", "build"], cwd=src, env=env, timeout=3600, tail=2)
    probe.run("cargo-test", ["cargo", "test"], cwd=src, env=env, timeout=3600, tail=3)
    totals, failed = test_summary((probe.logs / "cargo-test.log").read_text(errors="replace"))
    probe.say(f"unit tests: {totals}; failed: {', '.join(failed) or 'none'}")

    servers = [
        start(probe, "server-chat", [str(server), "-m", str(models["chat"]), "--mmproj", str(models["mmproj"]),
                                     "-ngl", "99", "-c", "32768", "--jinja",
                                     "--host", "127.0.0.1", "--port", str(CHAT_PORT)]),
        start(probe, "server-embed", [str(server), "-m", str(models["embed"]), "--embeddings", "-ngl", "99",
                                      "-c", "8192", "-ub", "8192", "-b", "8192",
                                      "--host", "127.0.0.1", "--port", str(EMBED_PORT)]),
        start(probe, "tts-stub", [sys.executable, str(src / "tools" / "tts_stub.py"), "--port", str(TTS_PORT)]),
    ]
    try:
        for port in (CHAT_PORT, EMBED_PORT):
            probe.say(f":{port} ready: {wait_for(f'{LOCALHOST}:{port}/health', 600)}")
        env["MINDFORK_ENGINE_URL"] = f"{LOCALHOST}:{CHAT_PORT}/v1"
        env["MINDFORK_EMBED_URL"] = f"{LOCALHOST}:{EMBED_PORT}/v1"
        env["MINDFORK_TTS_URL"] = f"{LOCALHOST}:{TTS_PORT}/v1"
        for stale in ("MINDFORK_EMBED_URL_ALT", "MINDFORK_ENGINE_KEY", "MINDFORK_LIVE_TEXT_ONLY"):
            env.pop(stale, None)
        probe.run("cargo-test-ignored", ["cargo", "test", "--", "--ignored", "--nocapture", "--test-threads=1"],
                  cwd=src, env=env, timeout=7200, tail=3)
        totals, failed = test_summary((probe.logs / "cargo-test-ignored.log").read_text(errors="replace"))
        probe.say(f"live smokes: {totals}; failed: {', '.join(failed) or 'none'}")
    finally:
        stop(servers)


# ---------------------------------------------------------------- M4 Ollama


def ollama_windows(ps: object) -> list[tuple[str, int | None]]:
    """(model, context_length) for each model `/api/ps` lists as loaded."""
    if not isinstance(ps, dict):
        return []
    return [(m.get("name", "?"), m.get("context_length")) for m in ps.get("models", []) if isinstance(m, dict)]


def step_ollama(probe: Probe, root: Path) -> None:
    probe.section("M4 Ollama")
    if http("GET", OLLAMA_VERSION, timeout=5)[0] != 200:
        if sys.platform != "darwin":
            probe.say("nothing answers on :11434, and this is not a Mac to install it on: skipped")
            return
        if not shutil.which("ollama"):
            probe.run("brew-ollama", ["brew", "install", "ollama"], timeout=1800, tail=3)
        served = start(probe, "ollama-serve", ["ollama", "serve"], detach=True)
        probe.say(f"ollama serve: pid {served.pid if served else '-'}, left running for the screen half")
        if not wait_for(OLLAMA_VERSION, 60):
            probe.say("ollama did not come up: skipped")
            return
    probe.say(f"version: {http('GET', OLLAMA_VERSION)[1]}")
    status, _ = http("POST", f"{OLLAMA}/api/pull", {"model": OLLAMA_MODEL, "stream": False}, timeout=7200)
    probe.say(f"pull {OLLAMA_MODEL}: {status}")
    status, answer = http("POST", f"{OLLAMA}/api/chat", {
        "model": OLLAMA_MODEL, "stream": False,
        "messages": [{"role": "user", "content": "Reply with one word: ready."}],
        "options": {"num_predict": 16},
    })
    if isinstance(answer, dict):
        load = answer.get("load_duration", 0) / 1e9
        probe.say(f"first request: {status}, the model loaded in {load:.1f}s")
    else:
        probe.say(f"first request: {status}")
    for name, window in ollama_windows(http("GET", f"{OLLAMA}/api/ps")[1]):
        probe.say(f"loaded: {name}, window {window}")


# ---------------------------------------------------------------- M5 LM Studio


def lm_llms(listing: object) -> list[dict]:
    """The LLM entries of LM Studio's `/api/v1/models`."""
    if not isinstance(listing, dict):
        return []
    return [m for m in listing.get("models", []) if isinstance(m, dict) and m.get("type") == "llm"]


def model_facts(entry: dict) -> str:
    """What tells one build from another — the engine's format above all, which
    is what an MLX build differs in — and the names of every other field, so a
    field LM Studio adds for MLX is seen even when it is not one of these."""
    known = ("format", "architecture", "quantization", "params_string", "max_context_length")
    facts = {k: entry[k] for k in known if k in entry}
    others = sorted(k for k in entry if k not in known and k not in ("key", "loaded_instances"))
    return f"{json.dumps(facts)}; other fields: {', '.join(others)}"


def loaded_windows(entry: dict) -> list[object]:
    return [i.get("config", {}).get("context_length") for i in entry.get("loaded_instances", []) if isinstance(i, dict)]


def calibrate(small: tuple[int, int], large: tuple[int, int]) -> tuple[float, float]:
    """Tokens per filler sentence and the fixed overhead, from two requests:
    (sentences, prompt_tokens) each."""
    per = (large[1] - small[1]) / (large[0] - small[0])
    return per, small[1] - small[0] * per


def conversation(pairs: int, sentences: int) -> list[dict]:
    """A fact in the first message, `pairs` exchanges of filler, a question."""
    messages = [
        {"role": "user", "content": f"The code word is {CODE_WORD}. Remember it."},
        {"role": "assistant", "content": "Noted."},
    ]
    for _ in range(pairs):
        messages += [
            {"role": "user", "content": FILLER * sentences},
            {"role": "assistant", "content": "Noted."},
        ]
    messages.append({"role": "user", "content": "Reply with one word: OK."})
    return messages


def verdict(status: int, answer: object, window: int, sent: float) -> str:
    """What the server did with a prompt over its window."""
    if status != 200:
        message = answer.get("error", answer) if isinstance(answer, dict) else answer
        return f"refused ({status}): {str(message)[:160]}"
    usage = answer.get("usage", {}) if isinstance(answer, dict) else {}
    prompt = usage.get("prompt_tokens")
    if not isinstance(prompt, int):
        return "answered, with no usage to tell a cut by"
    if prompt < min(window, sent * 0.9):
        return f"answered after a CUT: {prompt} prompt tokens of ~{sent:.0f} sent"
    return f"answered in full: {prompt} prompt tokens of ~{sent:.0f} sent"


def prompt_tokens(model: str, sentences: int) -> int | None:
    status, answer = http("POST", LM_STUDIO + CHAT_COMPLETIONS, {
        "model": model, "max_tokens": 1, "temperature": 0,
        "messages": [{"role": "user", "content": FILLER * sentences}],
    })
    if status == 200 and isinstance(answer, dict):
        tokens = answer.get("usage", {}).get("prompt_tokens")
        return tokens if isinstance(tokens, int) else None
    return None


def measure_overflow(probe: Probe, model: str) -> None:
    """The two overflows of local-servers.md §2, at `WINDOW`."""
    small, large = prompt_tokens(model, 20), prompt_tokens(model, 60)
    if small is None or large is None:
        probe.say("   calibration failed: no usage in the answers")
        return
    per, overhead = calibrate((20, small), (60, large))
    probe.say(f"   {per:.1f} tokens a sentence, {overhead:.0f} of overhead")
    target = WINDOW * 1.5
    sentences = math.ceil((target - overhead) / per)
    status, answer = http("POST", LM_STUDIO + CHAT_COMPLETIONS, {
        "model": model, "max_tokens": 8, "temperature": 0,
        "messages": [{"role": "user", "content": FILLER * sentences}],
    })
    probe.say(f"   one message over the window: {verdict(status, answer, WINDOW, overhead + sentences * per)}")
    per_pair = 25 * per + 12
    pairs = math.ceil((target - overhead) / per_pair)
    status, answer = http("POST", LM_STUDIO + CHAT_COMPLETIONS, {
        "model": model, "max_tokens": 8, "temperature": 0,
        "messages": conversation(pairs, 25),
    })
    probe.say(f"   a conversation over it: {verdict(status, answer, WINDOW, overhead + pairs * per_pair)}")


def step_lmstudio(probe: Probe, root: Path) -> None:
    probe.section("M5 LM Studio")
    lms = Path.home() / ".lmstudio" / "bin" / ("lms.exe" if sys.platform == "win32" else "lms")
    if not lms.is_file():
        probe.say("lms not found: install LM Studio, open it once, then run --only M5")
        return
    probe.run("lms-server", [str(lms), "server", "start"], timeout=120, tail=2)
    if not wait_for(LM_MODELS, 60):
        probe.say("the server did not answer: skipped")
        return
    probe.say(f"lms: {capture([str(lms), 'version', '--json'])}")
    llms = lm_llms(http("GET", LM_MODELS)[1])
    if not llms:
        probe.say("no LLM downloaded: get the GGUF and the MLX build of Gemma 4 E4B in the app, then --only M5")
    for entry in llms:
        key = entry.get("key", "?")
        probe.say(f"-- {key}: {model_facts(entry)}")
        probe.run(f"lms-unload-{hash_prefix(key)}", [str(lms), "unload", "--all"], timeout=120, tail=1)
        probe.run(f"lms-load-{hash_prefix(key)}", [str(lms), "load", key, "-c", str(WINDOW)], timeout=600, tail=2)
        listed = next((m for m in lm_llms(http("GET", LM_MODELS)[1]) if m.get("key") == key), {})
        probe.say(f"   the window it reports: {loaded_windows(listed)}")
        measure_overflow(probe, key)
    probe.run("lms-unload-all", [str(lms), "unload", "--all"], timeout=120, tail=1)
    probe.say("the server is left on; load a model in the app for the screen half")


# ---------------------------------------------------------------- the end


def closing(probe: Probe, root: Path) -> None:
    probe.section("The screen half (§13.6)")
    keys = root / "src" / "target" / "debug" / "mindfork"
    probe.say(f"the key echo:      {keys} keys -o {root}/keys-<terminal>.txt")
    probe.say(f"the speech stub:   python3 {root}/src/tools/tts_stub.py")
    probe.say(f"the managed chat:  {root}/portable/mindfork")
    probe.say("the Homebrew one:  mindfork")
    probe.say(f"report: {probe.report}")


STEP_FUNCTIONS = {
    "M0": step_host,
    "M1": step_install,
    "M2": step_metal,
    "M3": step_gate,
    "M4": step_ollama,
    "M5": step_lmstudio,
}


# ---------------------------------------------------------------- self-test


def self_test() -> int:
    failures = []

    def check(ok: bool, what: str) -> None:
        print(f"  {'ok  ' if ok else 'FAIL'}  {what}")
        if not ok:
            failures.append(what)

    with tempfile.TemporaryDirectory(prefix="mac-probe-") as scratch:
        probe = Probe(Path(scratch))
        probe.say("first line")
        code = probe.run("ok", [sys.executable, "-c", "print('one'); print('two')"], tail=1)
        check(code == 0, "a command's exit code comes back")
        report = probe.report.read_text(encoding="utf-8")
        check("first line" in report and "   two" in report and "one\n" not in report,
              "the report gets the command's last lines only")
        check((probe.logs / "ok.log").read_text().splitlines() == ["one", "two"], "the log keeps everything")
        check(probe.run("missing", ["no-such-command-anywhere"]) == MISSING, "a missing command is 127")
        check(probe.run("slow", [sys.executable, "-c", "import time; time.sleep(5)"], timeout=0.5) == -1,
              "a command over its time is stopped")
        check(probe.run("fails", [sys.executable, "-c", "raise SystemExit(3)"]) == 3, "a failure's code is kept")

    uuid = "4C4C4544-0042-3510-8051-B9C04F4B4E32"
    ioreg = f'+-o J414sAP  <class IOPlatformExpertDevice>\n    "IOPlatformUUID" = "{uuid}"\n'
    check(platform_uuid(ioreg) == uuid, "the UUID is found in ioreg's output")
    check(len(hash_prefix(uuid)) == 12 and uuid not in hash_prefix(uuid), "only a hash prefix is reported")
    check(platform_uuid("nothing here") is None, "no UUID is no UUID")

    cargo = (
        "test keys::echo_works ... ok\ntest keys::echo_fails ... FAILED\n"
        "test result: FAILED. 10 passed; 1 failed; 2 ignored; 0 measured\n"
        "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured\n"
    )
    totals, failed = test_summary(cargo)
    check(totals == {"passed": 13, "failed": 1, "ignored": 2}, "test results are summed")
    check(failed == ["keys::echo_fails"], "the failed tests are named")

    ps = {"models": [{"name": OLLAMA_MODEL, "context_length": 4096}, "junk"]}
    check(ollama_windows(ps) == [(OLLAMA_MODEL, 4096)], "Ollama's window is read from /api/ps")
    check(ollama_windows("not json") == [], "an /api/ps that is not JSON is nothing")

    listing = {"models": [
        {"type": "llm", "key": "gemma", "loaded_instances": [{"id": "gemma", "config": {"context_length": 4096}}]},
        {"type": "embedding", "key": "nomic"},
    ]}
    llms = lm_llms(listing)
    check([m["key"] for m in llms] == ["gemma"], "LM Studio's LLMs are told from its embedders")
    check(loaded_windows(llms[0]) == [4096], "a loaded instance's window is read")
    class Here:
        """A file system where only Homebrew's `bin` exists."""

        def __init__(self, path: str):
            self.path = path

        def is_dir(self) -> bool:
            return self.path == BREW_BIN

    joined = with_homebrew(os.pathsep.join(["/usr/bin", "/bin"]), Here)
    check(joined.split(os.pathsep) == [BREW_BIN, "/usr/bin", "/bin"],
          "an SSH session's PATH gets Homebrew's bin first")
    check(with_homebrew(joined, Here) == joined, "a PATH that has it is left alone")
    facts = model_facts({"key": "g", "format": "mlx", "type": "llm", "vision": True, "loaded_instances": []})
    check(facts == '{"format": "mlx"}; other fields: type, vision', "a model's format is shown, other fields named")

    per, overhead = calibrate((20, 300), (60, 860))
    check(per == 14 and overhead == 20, "the calibration solves for tokens a sentence and overhead")
    messages = conversation(3, 2)
    check(CODE_WORD in messages[0]["content"] and len(messages) == 9, "the conversation has the fact first")
    check([m["role"] for m in messages[:2]] == ["user", "assistant"] and messages[-1]["role"] == "user",
          "the conversation alternates and ends on the user")

    refused = verdict(400, {"error": "exceed_context_size_error"}, WINDOW, 6000)
    check(refused.startswith("refused (400)") and "exceed" in refused, "a 400 is a refusal")
    check("CUT" in verdict(200, {"usage": {"prompt_tokens": 2809}}, WINDOW, 6000), "fewer tokens than sent is a cut")
    check("in full" in verdict(200, {"usage": {"prompt_tokens": 6100}}, WINDOW, 6000), "all of it is no cut")
    check("no usage" in verdict(200, {}, WINDOW, 6000), "no usage says so")

    print(f"mac_probe self-test: {'all checks passed' if not failures else f'{len(failures)} FAILED'}")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description="The headless half of the rented Mac day.")
    parser.add_argument("dir", nargs="?", default=str(Path.home() / "mindfork-probe"),
                        help="where everything goes (default ~/mindfork-probe)")
    parser.add_argument("--only", help=f"comma-separated steps of {','.join(STEPS)}")
    parser.add_argument("--self-test", action="store_true", help="run the offline checks and exit")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    steps = [s.strip().upper() for s in args.only.split(",")] if args.only else list(STEPS)
    unknown = [s for s in steps if s not in STEP_FUNCTIONS]
    if unknown:
        parser.error(f"unknown step(s): {', '.join(unknown)}")
    root = Path(args.dir).expanduser().resolve()
    os.environ["PATH"] = with_homebrew(os.environ.get("PATH", ""))
    probe = Probe(root)
    probe.say(f"######## mac_probe {', '.join(steps)} — {time.strftime('%Y-%m-%d %H:%M:%S')}")
    for step in steps:
        try:
            STEP_FUNCTIONS[step](probe, root)
        except Exception as error:  # noqa: BLE001 - no step stops the next
            probe.say(f"!! {step} stopped: {type(error).__name__}: {error}")
    closing(probe, root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
