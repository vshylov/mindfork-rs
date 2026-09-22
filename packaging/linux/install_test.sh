#!/bin/sh
# Scenarios for install.sh, run by packaging.yml in bare distribution containers
# (and by hand: `sh packaging/linux/install_test.sh` inside any Linux container).
#
#   BIN=/path/to/mindfork sh packaging/linux/install_test.sh
#
# Everything here installs with `--from`, so **no scenario needs the network** —
# the download path is exercised separately, against a real release, by the
# workflow. With `BIN` (a real Linux binary) the "bare image" scenario runs
# against the real loader; without it that one is skipped and the rest use a
# stand-in script, which is enough for everything that is about this script
# rather than about the app.
#
# Every scenario that expects a refusal is a control arm for the one beside it:
# an install script whose checks cannot fail is not checking.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
S="$HERE/install.sh"
WORK="$(mktemp -d)"
pass=0
fail=0

check() { # description, expected exit code, actual exit code
    if [ "$2" = "$3" ]; then
        pass=$((pass + 1))
        echo "  PASS  $1"
    else
        fail=$((fail + 1))
        echo "  FAIL  $1 (expected exit $2, got $3)"
    fi
}

# A release directory shaped like release.yml's: a **flat** archive, and a
# sha256sums.txt whose lines spell the name `./name`.
release_dir() { # directory, tag, binary to pack
    mkdir -p "$1/stage/data/dictionaries"
    cp "$3" "$1/stage/mindfork"
    chmod 0755 "$1/stage/mindfork"
    echo "readme" >"$1/stage/README.md"
    echo "dict" >"$1/stage/data/dictionaries/en_US.dic"
    tar --owner="$FOREIGN_UID" --group="$FOREIGN_UID" --numeric-owner         -C "$1/stage" -czf "$1/mindfork-rs-$2-x86_64-linux.tar.gz" .
    rm -rf "$1/stage"
    (cd "$1" && sha256sum ./*.tar.gz >sha256sums.txt)
}

# The archive records whoever packed it. release.yml packs as the CI runner
# (uid 1001), so a real release's entries are owned by a user the target
# machine does not have — and with a fixed foreign owner here every scenario
# below exercises that, whatever uid runs the test.
FOREIGN_UID=1001

# The stand-in: answers `--version`, and otherwise reports how many arguments
# it got and each one on its own line — which is what proves a quoted argument
# with a space in it arrives as one.
STANDIN="$WORK/standin"
cat >"$STANDIN" <<'EOF'
#!/bin/sh
[ "${1:-}" = "--version" ] && { echo "mindfork 9.9.9"; exit 0; }
echo "ARGC:$#"
for a in "$@"; do echo "ARG:$a"; done
EOF

echo "== syntax"
sh -n "$S"
check "sh -n install.sh" 0 $?
sh "$S" --help | grep -q -- "--from DIR"
check "--help works with no file to read back" 0 $?

if [ -n "${BIN:-}" ]; then
    echo "== the real binary on this image"
    release_dir "$WORK/real" v0.0.1 "$BIN"
    if "$BIN" --version >/dev/null 2>&1; then expected=0; else expected=3; fi
    out="$(sh "$S" --from "$WORK/real" --dir "$WORK/opt-real" --no-deps --no-link 2>&1)"
    code=$?
    echo "$out" | sed 's/^/     | /'
    check "--no-deps: exit $expected, as the loader decides" "$expected" "$code"
    if [ "$expected" = 3 ]; then
        echo "$out" | grep -q "libasound"
        check "the refusal names the library and the command" 0 $?
    fi
    if [ -x "$WORK/opt-real/mindfork" ] && [ -f "$WORK/opt-real/data/dictionaries/en_US.dic" ]; then ok=0; else ok=1; fi
    check "unpacked all the same: binary and bundled data" 0 $ok
else
    echo "== the real binary: skipped (BIN is not set)"
fi

# What the first real pod found (2026-09-22): a container that runs as root
# but without CAP_CHOWN. GNU tar as root restores the archive's recorded owner
# by default, and here that is "Operation not permitted" on every entry. Only
# root can be refused a chown, so the arm exists only when the test runs as
# root — and it proves itself: a plain `tar -x` must fail where the script
# must succeed, or the environment cannot show the difference.
echo "== root without CAP_CHOWN (the pod's shape)"
if [ "$(id -u)" = 0 ] && command -v capsh >/dev/null 2>&1; then
    release_dir "$WORK/relc" v9.9.9 "$STANDIN"
    capsh --drop=cap_chown -- -c "mkdir -p $WORK/plain && cd $WORK/plain && tar -xzf $WORK/relc/mindfork-rs-v9.9.9-x86_64-linux.tar.gz" >/dev/null 2>&1
    check "control arm: a plain tar -x is refused without CAP_CHOWN" 2 $?
    capsh --drop=cap_chown -- -c "sh $S --from $WORK/relc --dir $WORK/opt-c --no-link -- --version" >/dev/null 2>&1
    check "install.sh installs where a plain tar cannot" 0 $?
    if [ -x "$WORK/opt-c/mindfork" ] && [ -f "$WORK/opt-c/data/dictionaries/en_US.dic" ]; then ok=0; else ok=1; fi
    check "…and everything is there" 0 $ok
elif [ "$(id -u)" = 0 ]; then
    # Root without capsh cannot show the difference — and this is the one arm
    # that found a real defect on a real pod, so it does not get to skip.
    fail=$((fail + 1))
    echo "  FAIL  capsh is not installed (libcap2-bin / libcap) — the CAP_CHOWN arm cannot run as root"
else
    echo "  SKIP  not root — a non-root tar never chowns, so there is nothing to refuse"
fi

echo "== install, hand-over, link"
release_dir "$WORK/rel" v9.9.9 "$STANDIN"
out="$(sh "$S" --from "$WORK/rel" --dir "$WORK/opt" --no-link -- setup --ctx 8192 --set "a.b=c d" 2>&1)"
code=$?
echo "$out" | sed 's/^/     | /'
check "install + hand-over" 0 $code
echo "$out" | grep -q "^ARGC:5$"
check "five arguments after -- arrive as five" 0 $?
echo "$out" | grep -q "^ARG:a.b=c d$"
check "…and the one with a space in it arrives whole" 0 $?
if [ "$(cat "$WORK/opt/.mindfork-version")" = "v9.9.9" ] && [ ! -e "$WORK/opt/.staging" ]; then ok=0; else ok=1; fi
check "the marker is written and staging is gone" 0 $ok

echo "== again: nothing is unpacked twice, and user data is not the script's to touch"
echo '{"mine":true}' >"$WORK/opt/data/settings.json"
out="$(sh "$S" --from "$WORK/rel" --dir "$WORK/opt" --no-link 2>&1)"
code=$?
echo "$out" | grep -q "already installed"
check "recognised as installed" 0 $?
check "and exits clean" 0 $code
grep -q mine "$WORK/opt/data/settings.json"
check "settings.json untouched" 0 $?

echo "== an upgrade over it"
mkdir -p "$WORK/rel2" && release_dir "$WORK/rel2" v9.9.10 "$STANDIN"
sh "$S" --from "$WORK/rel2" --dir "$WORK/opt" --no-link >/dev/null 2>&1
check "upgrade" 0 $?
if [ "$(cat "$WORK/opt/.mindfork-version")" = "v9.9.10" ] && grep -q mine "$WORK/opt/data/settings.json"; then ok=0; else ok=1; fi
check "the marker moved, the data did not" 0 $ok

echo "== refusals"
mkdir -p "$WORK/bad" && cp "$WORK/rel"/* "$WORK/bad/"
printf 'x' >>"$WORK/bad/mindfork-rs-v9.9.9-x86_64-linux.tar.gz"
out="$(sh "$S" --from "$WORK/bad" --dir "$WORK/opt-bad" --no-link 2>&1)"
check "a tampered archive" 1 $?
echo "$out" | grep -q "checksum mismatch"
check "…is refused as a checksum mismatch" 0 $?
[ ! -e "$WORK/opt-bad/mindfork" ]
check "…and nothing of it is installed" 0 $?
mkdir -p "$WORK/unlisted" && cp "$WORK/rel"/*.tar.gz "$WORK/unlisted/"
echo "00  ./other.tar.gz" >"$WORK/unlisted/sha256sums.txt"
sh "$S" --from "$WORK/unlisted" --dir "$WORK/opt-unl" --no-link >/dev/null 2>&1
check "an archive sha256sums.txt does not list" 1 $?
rm -f "$WORK/unlisted/sha256sums.txt"
sh "$S" --from "$WORK/unlisted" --dir "$WORK/opt-unl" --no-link >/dev/null 2>&1
check "no sha256sums.txt at all" 1 $?
mkdir -p "$WORK/two" && cp "$WORK/rel"/*.tar.gz "$WORK/rel2"/*.tar.gz "$WORK/two/"
sh "$S" --from "$WORK/two" --dir "$WORK/opt-two" --no-link >/dev/null 2>&1
check "two archives and no --version to choose" 1 $?
sh "$S" --version 'v1.2.3;rm' --dir "$WORK/x" >/dev/null 2>&1
check "a tag with shell in it" 1 $?
sh "$S" --version latest --dir "$WORK/x" >/dev/null 2>&1
check "a tag that is not a tag" 1 $?
sh "$S" --bogus >/dev/null 2>&1
check "an unknown option" 1 $?
sh "$S" --from "$WORK/nonexistent" >/dev/null 2>&1
check "--from a missing directory" 1 $?
sh "$S" --dir >/dev/null 2>&1
check "--dir with no value" 1 $?

rm -rf "$WORK"
echo
echo "passed=$pass failed=$fail"
[ "$fail" = 0 ]
