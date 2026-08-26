#!/bin/sh
# Fills the models volume from Hugging Face. Runs as the one-shot `models` service
# in docker/compose.yaml; both llama-server services wait for it to exit 0.
#
# Three properties are load-bearing:
#
# 1. **Idempotent.** A file whose size already matches the server's is skipped, so
#    `docker compose up` after the first one costs three HEAD requests.
# 2. **A partial download never looks finished.** Bytes land in `<name>.part` and
#    are renamed only after the size check passes — an interrupted `up` resumes
#    (`curl -C -`) instead of leaving a truncated GGUF that llama-server would
#    accept and then die loading.
# 3. **The expected size comes from the server, not from a constant here.** Change
#    the quant in `.env` and the check follows; a hardcoded number would have to
#    be edited in lockstep and would silently pass when it was not.
#
# POSIX sh (busybox ash in curlimages/curl). `set -e` plus `a && b` at statement
# level is the classic accidental-exit pattern, so every branch below is an
# explicit `if`.

set -eu

DEST="${DEST:-/models}"
mkdir -p "$DEST"

# The token goes into a file curl reads the header from, never onto the command
# line: argv is world-readable through `ps` and shows up in anything that dumps
# the process table, whereas the file is 600 and unlinked on exit.
AUTH_FILE=""
if [ -n "${HF_TOKEN:-}" ]; then
    AUTH_FILE="$(mktemp)"
    chmod 600 "$AUTH_FILE"
    printf 'Authorization: Bearer %s\n' "$HF_TOKEN" > "$AUTH_FILE"
    trap 'rm -f "$AUTH_FILE"' EXIT INT TERM
fi

# curl with the token header only when there is one — an empty `-H` is not the
# same as no `-H`.
hf_curl() {
    if [ -n "$AUTH_FILE" ]; then
        curl -H "@${AUTH_FILE}" "$@"
    else
        curl "$@"
    fi
}

# "<status> <size>" for a file, from one HEAD that follows the redirects to the
# CDN — the *last* status line and the *last* Content-Length in the chain are the
# ones that describe the actual object. The status matters: a 404 page also has a
# Content-Length, and taking it for the file's size turns "this name is wrong"
# into a confusing size mismatch three minutes later.
remote_probe() {
    hf_curl -sIL --max-time 60 "$1" \
        | tr -d '\r' \
        | awk '
            $1 ~ /^[Hh][Tt][Tt][Pp]\// { code = $2 }
            tolower($1) == "content-length:" { n = $2 }
            END { print (code == "" ? "000" : code), (n == "" ? 0 : n) }'
}

size_of() {
    if [ -f "$1" ]; then
        wc -c < "$1" | tr -d ' '
    else
        echo 0
    fi
}

# curl's own progress meter is carriage-return based and turns `docker compose
# logs` into a wall of half-drawn tables (there is no TTY here). Silence it and
# emit one honest line a minute instead — over a 45-minute download the
# difference between "silent" and "readable" matters more than the resolution.
progress_ticker() {
    while sleep 60; do
        cur="$(size_of "$1")"
        if [ "$2" -gt 0 ]; then
            echo "   ${cur} / ${2} B ($(( cur * 100 / $2 ))%)"
        else
            echo "   ${cur} B"
        fi
    done
}

fetch() {
    repo="$1"
    file="$2"
    url="https://huggingface.co/${repo}/resolve/main/${file}"
    dest="${DEST}/${file}"
    part="${dest}.part"

    probe="$(remote_probe "$url")"
    code="${probe% *}"
    want="${probe#* }"
    have="$(size_of "$dest")"

    if [ "$code" != "200" ]; then
        echo "!! ${file}: HTTP ${code} from ${repo}"
        echo "   check the repository and file name in .env"
        if [ "$code" = "401" ] || [ "$code" = "403" ]; then
            echo "   a gated repository needs HF_TOKEN"
        fi
        exit 1
    fi

    if [ "$want" -eq 0 ]; then
        echo "!! ${file}: the server did not report a size; the check is off"
        if [ "$have" -gt 0 ]; then
            echo "   keeping the existing ${have} B copy"
            return 0
        fi
    elif [ "$have" = "$want" ]; then
        echo "ok ${file} (${have} B, already complete)"
        return 0
    elif [ "$have" -gt 0 ]; then
        echo ".. ${file}: have ${have} B, expected ${want} B — refetching"
    else
        echo ".. ${file}: ${want} B from ${repo}"
    fi

    # A complete `.part` means a previous run downloaded everything and died
    # before the rename; resuming it would ask for a range past EOF and get a 416.
    partial="$(size_of "$part")"
    if [ "$want" -gt 0 ] && [ "$partial" = "$want" ]; then
        echo "   a complete .part was left behind — installing it"
    else
        # Without this line an interrupted `up` looks like it started over: the
        # bytes are in `<name>.part`, and the size printed above is the target's.
        if [ "$partial" -gt 0 ]; then
            echo "   resuming from ${partial} B"
        fi
        progress_ticker "$part" "$want" &
        ticker=$!
        # `set -e` must not skip the kill, and a failed download must still fail
        # the service — hence the explicit status capture rather than `||`.
        status=0
        # `--http1.1` and `--retry-all-errors` are both measured, not defensive.
        # A 4.6 GiB transfer from the HF CDN died at 4.34 GiB with curl exit 92
        # ("stream error in the HTTP/2 framing layer") — HTTP/2 multiplexing is
        # not worth its failure rate for one enormous sequential body. And curl's
        # plain `--retry` covers transient *HTTP* statuses and connection
        # problems, not a mid-stream protocol error, so without
        # `--retry-all-errors` the very failure that actually happens is the one
        # not retried. With `-C -` a retry resumes rather than restarting.
        hf_curl -fL --no-progress-meter --http1.1 \
            --retry 10 --retry-delay 5 --retry-all-errors --retry-connrefused \
            -C - -o "$part" "$url" || status=$?
        kill "$ticker" 2>/dev/null || true
        wait "$ticker" 2>/dev/null || true
        if [ "$status" -ne 0 ]; then
            echo "!! ${file}: curl exited ${status}"
            exit "$status"
        fi
    fi

    got="$(size_of "$part")"
    if [ "$want" -gt 0 ] && [ "$got" != "$want" ]; then
        echo "!! ${file}: downloaded ${got} B, expected ${want} B — not installing"
        exit 1
    fi
    mv "$part" "$dest"
    chmod 0644 "$dest"
    echo "ok ${file} (${got} B)"
}

echo "== models -> ${DEST}"
fetch "$CHAT_REPO" "$CHAT_GGUF"
fetch "$EMBED_REPO" "$EMBED_GGUF"

# The vision projector is optional: blanking MMPROJ_GGUF in `.env` (together with
# the --mmproj flag in CHAT_EXTRA_ARGS) runs the chat model text-only.
if [ -n "${MMPROJ_GGUF:-}" ]; then
    fetch "$CHAT_REPO" "$MMPROJ_GGUF"
fi

echo "== done"
ls -l "$DEST"
