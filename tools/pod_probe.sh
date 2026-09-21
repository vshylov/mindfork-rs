#!/usr/bin/env bash
# The probe of docs/research/cloud-provisioning.md §7, for a rented GPU pod
# (written against RunPod's `*-ubuntu2404` images; nothing in it is RunPod's).
#
#   bash pod_probe.sh [DIR]            # DIR defaults to /workspace/mindfork-probe
#   SKIP_CUDART=1 bash pod_probe.sh    # skip the 594 MB CUDA runtime half of P3
#   SKIP_SANDBOX=1 bash pod_probe.sh   # skip P5 (~206 MB down, ~1.7 GB on disk)
#
# It uses only what is released today, keeps everything under DIR, and writes
# one report — DIR/probe-report.txt — to paste back. Three properties matter:
#
# 1. **No step stops the next.** There is deliberately no `set -e`: a probe that
#    dies at P1 answers one question out of eight, on a meter.
# 2. **Nothing secret reaches the report.** The environment is never dumped (a
#    pod carries an API key in it), and `/etc/machine-id` — key material for
#    `shared::secrets` — is reported as a size and a hash prefix, never as itself.
# 3. **Nothing outside DIR is changed** except the one package P1 exists to
#    measure (`libasound2t64`), on a container disk the next stop clears anyway.
#
# P6 (the TUI in each of the pod's terminals) is a person looking at
# `mindfork demo`; the closing lines say how.

set -u

DIR="${1:-/workspace/mindfork-probe}"
APP="$DIR/mindfork"
REPORT="$DIR/probe-report.txt"
REPO="vshylov/mindfork-rs"
EMBED_URL="https://huggingface.co/ggml-org/bge-m3-Q8_0-GGUF/resolve/main/bge-m3-q8_0.gguf"
MODEL="$DIR/bge-m3-q8_0.gguf"
PORT=8011

mkdir -p "$APP" || { echo "cannot create $APP"; exit 1; }
: > "$REPORT"

say() { printf '%s\n' "$*" | tee -a "$REPORT"; }
section() { say ""; say "== $*"; }
# Runs a command, its output and its exit code both into the report.
run() {
    say "\$ $*"
    "$@" 2>&1 | tee -a "$REPORT"
    local code="${PIPESTATUS[0]}"
    say "   -> exit $code"
    return "$code"
}

# ---------------------------------------------------------------- P0 the host
section "P0 host"
say "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
say "os: $(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-unknown}")"
say "glibc: $(ldd --version 2>&1 | head -1)"
say "user: $(id -un) (uid $(id -u))"
say "cpu: $(nproc) cores, ram: $(free -g 2>/dev/null | awk '/^Mem:/{print $2" GiB"}')"
if command -v nvidia-smi >/dev/null; then
    run nvidia-smi --query-gpu=name,driver_version,compute_cap,memory.total --format=csv
else
    say "nvidia-smi: not found"
fi
say "nvcc: $(nvcc --version 2>/dev/null | tail -1 || echo none)"
say "volume: $(df -hT "$DIR" | tail -1)"
say "root:   $(df -hT / | tail -1)"
say "tools: $(for t in curl tar tmux unzip sha256sum apt-get python3; do command -v "$t" >/dev/null && printf '%s ' "$t"; done)"

# ---------------------------------------------------------------- P8 machine-id
section "P8 /etc/machine-id (compare the hash prefix across a stop/start)"
if [ -e /etc/machine-id ]; then
    say "size: $(wc -c < /etc/machine-id) bytes, sha256 prefix: $(sha256sum < /etc/machine-id | cut -c1-12)"
else
    say "absent"
fi

# Everything below downloads. The two sections above are worth having even from
# an image without curl, which is why the check sits here and not at the top.
if ! command -v curl >/dev/null; then
    say ""
    say "curl is not installed: nothing below can run. Report: $REPORT"
    exit 1
fi

# ---------------------------------------------------------------- P1 the archive
section "P1 the released archive"
TAG="$(curl -fsSI "https://github.com/$REPO/releases/latest" | tr -d '\r' \
    | awk 'tolower($1)=="location:"{print $2}' | sed 's#.*/tag/##')"
say "latest release (from the redirect, no API call): ${TAG:-UNRESOLVED}"
ARCHIVE="mindfork-rs-${TAG}-x86_64-linux.tar.gz"
BASE="https://github.com/$REPO/releases/download/$TAG"
if [ -n "$TAG" ] && [ ! -x "$APP/mindfork" ]; then
    run curl -fL --retry 5 --no-progress-meter -o "$DIR/$ARCHIVE" "$BASE/$ARCHIVE"
    run curl -fL --retry 5 --no-progress-meter -o "$DIR/sha256sums.txt" "$BASE/sha256sums.txt"
    ( cd "$DIR" && grep -F "$ARCHIVE" sha256sums.txt | sha256sum -c - ) 2>&1 | tee -a "$REPORT"
    run tar -C "$APP" -xzf "$DIR/$ARCHIVE"
fi
say "-- before any package is installed:"
run "$APP/mindfork" --version
if ! ldconfig -p 2>/dev/null | grep -q 'libasound\.so\.2'; then
    say "-- libasound.so.2 is absent; installing it (this is the measurement)"
    if [ "$(id -u)" = 0 ] && command -v apt-get >/dev/null; then
        SECONDS=0
        apt-get update -qq >/dev/null 2>&1
        if apt-get install -y -qq --no-install-recommends libasound2t64 >/dev/null 2>&1; then
            say "installed libasound2t64 in ${SECONDS}s"
        elif apt-get install -y -qq --no-install-recommends libasound2 >/dev/null 2>&1; then
            say "installed libasound2 (no t64 on this release) in ${SECONDS}s"
        else
            say "apt-get could not install either package"
        fi
    else
        say "not root, or no apt-get: install libasound2t64 by hand and re-run"
    fi
    run "$APP/mindfork" --version
else
    say "libasound.so.2 was already present in this image"
fi
# P2, P3 and P5 are the app's own commands: without a binary that starts they
# would print the same "not found" three times over.
if ! "$APP/mindfork" --version >/dev/null 2>&1; then
    say ""
    say "mindfork does not start here, so P2-P5 cannot run. Report: $REPORT"
    exit 1
fi

# ---------------------------------------------------------------- P2 the API
section "P2 GitHub's API from this address (rate_limit itself costs nothing)"
curl -fsS https://api.github.com/rate_limit 2>&1 \
    | tr -d ' \n' | grep -o '"core":{[^}]*}' | tee -a "$REPORT"
say ""
run "$APP/mindfork" llama backends

# ---------------------------------------------------------------- P3 CUDA
section "P3 the official CUDA build"
BACKEND="$("$APP/mindfork" llama backends 2>/dev/null | awk '$1 ~ /^cuda-12/{print $1; exit}')"
say "backend picked from the list: ${BACKEND:-NONE}"
if [ -n "$BACKEND" ]; then
    say "-- without --no-cudart (expected to be REFUSED until stage 0 lands):"
    run "$APP/mindfork" llama setup --backend "$BACKEND"
    say "-- with --no-cudart (the host's own CUDA libraries):"
    run "$APP/mindfork" llama setup --backend "$BACKEND" --no-cudart
fi
LLAMA="$(find "$APP/data/llama" -maxdepth 2 -name llama-server -type f 2>/dev/null | head -1)"
LLAMA_DIR="$(dirname "${LLAMA:-/nonexistent/x}")"
say "installed: ${LLAMA:-NOTHING}"
cuda_links() { # which libcudart/libcublas the CUDA backend resolves to
    local lib
    lib="$(find "$1" -maxdepth 1 -name 'libggml-cuda*.so*' | head -1)"
    [ -n "$lib" ] && ldd "$lib" 2>&1 | grep -E 'cudart|cublas|not found' | tee -a "$REPORT"
}
if [ -n "$LLAMA" ]; then
    run "$LLAMA" --list-devices
    cuda_links "$LLAMA_DIR"
    if [ -z "${SKIP_CUDART:-}" ]; then
        ID="$(basename "$LLAMA_DIR")"; LTAG="${ID##*-}"; LBACK="${ID%-*}"
        RT="cudart-llama-${LTAG}-bin-ubuntu-${LBACK}-x64.tar.gz"
        WITH="$DIR/llama-with-cudart"
        say "-- the same build with upstream's runtime archive beside it ($RT):"
        if [ ! -d "$WITH" ]; then
            cp -a "$LLAMA_DIR" "$WITH"
            run curl -fL --retry 5 -C - --no-progress-meter -o "$DIR/$RT" \
                "https://github.com/ggml-org/llama.cpp/releases/download/$LTAG/$RT"
            say "sha256: $(sha256sum "$DIR/$RT" | cut -d' ' -f1)"
            say "layout: $(tar -tzf "$DIR/$RT" | head -5 | tr '\n' ' ')"
            mkdir -p "$DIR/rt" && tar -C "$DIR/rt" -xzf "$DIR/$RT"
            find "$DIR/rt" \( -type f -o -type l \) -name '*.so*' -exec cp -a {} "$WITH/" \;
        fi
        run "$WITH/llama-server" --list-devices
        cuda_links "$WITH"
    fi
fi

# ---------------------------------------------------------------- P4 + P7
section "P4 time to ready, cold then warm; P7 the volume"
if [ -n "$LLAMA" ]; then
    [ -f "$MODEL" ] || run curl -fL --retry 10 --retry-all-errors --http1.1 -C - \
        --no-progress-meter -o "$MODEL" "$EMBED_URL"
    say "model: $(wc -c < "$MODEL" 2>/dev/null) bytes"
    say "-- P7 sequential read, bypassing the page cache:"
    dd if="$MODEL" of=/dev/null bs=4M iflag=direct 2>&1 | tail -1 | tee -a "$REPORT"
    export CUDA_CACHE_PATH="$DIR/cuda-cache"
    ready() { # $1 — label. Whole seconds: the question is a JIT's minutes.
        local code="" i pid
        SECONDS=0
        "$LLAMA" -m "$MODEL" --embeddings -ngl 99 -c 8192 -ub 8192 -b 8192 \
            --host 127.0.0.1 --port "$PORT" > "$DIR/server-$1.log" 2>&1 &
        pid=$!
        for i in $(seq 1 1200); do
            code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/health")"
            [ "$code" = 200 ] && break
            kill -0 "$pid" 2>/dev/null || { code="exited"; break; }
            sleep 0.5
        done
        say "ready[$1]: $code after ${SECONDS}s"
        grep -iE 'found [0-9]+ CUDA|offloaded|error|failed' "$DIR/server-$1.log" | head -6 | tee -a "$REPORT"
        kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
    }
    ready cold
    ready warm
    say "JIT cache on the volume: $(du -sh "$CUDA_CACHE_PATH" 2>/dev/null | cut -f1 || echo none) (empty = this card needed no JIT)"
else
    say "skipped: no llama-server was installed"
fi

# ---------------------------------------------------------------- P5 sandbox
section "P5 the Python sandbox in this container"
if [ -z "${SKIP_SANDBOX:-}" ]; then
    SECONDS=0
    run "$APP/mindfork" sandbox setup --enable-python
    say "wall time: ${SECONDS}s; on disk: $(du -sh "$APP/data/sandbox" 2>/dev/null | cut -f1)"
else
    say "skipped (SKIP_SANDBOX)"
fi

# ---------------------------------------------------------------- P6 by hand
section "P6 is yours"
say "Run   $APP/mindfork demo   in each terminal the pod offers — the web"
say "terminal, basic SSH (ssh.runpod.io), full SSH — bare, then inside"
say "'tmux new -A -s probe'. Note for each: does it draw, do the arrows, Ctrl+P,"
say "F1 and Esc arrive, does a resize redraw, does the mouse wheel scroll."
say ""
say "Report: $REPORT   (everything this probe wrote is under $DIR)"
