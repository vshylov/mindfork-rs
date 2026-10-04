#!/bin/sh
# Scenarios for install.sh, run by packaging.yml in bare distribution containers
# and on a macOS runner (and by hand: `sh packaging/linux/install_test.sh` inside
# any Linux container, or on a Mac).
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

# The build this machine installs, as install.sh names its archive.
case "$(uname -s)" in
Darwin) OS=macos SUFFIX=aarch64-macos ;;
*) OS=linux SUFFIX=x86_64-linux ;;
esac

sha256() { # files… — `sha256sum`'s lines, from whichever tool the system has
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

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
    if [ "$OS" = macos ]; then
        # bsdtar spells the recorded owner its own way.
        tar --uid "$FOREIGN_UID" --gid "$FOREIGN_UID" --numeric-owner \
            -C "$1/stage" -czf "$1/mindfork-rs-$2-$SUFFIX.tar.gz" .
    else
        tar --owner="$FOREIGN_UID" --group="$FOREIGN_UID" --numeric-owner \
            -C "$1/stage" -czf "$1/mindfork-rs-$2-$SUFFIX.tar.gz" .
    fi
    rm -rf "$1/stage"
    (cd "$1" && sha256 ./*.tar.gz >sha256sums.txt)
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
    capsh --drop=cap_chown -- -c "mkdir -p $WORK/plain && cd $WORK/plain && tar -xzf $WORK/relc/mindfork-rs-v9.9.9-$SUFFIX.tar.gz" >/dev/null 2>&1
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

# OpenMP's runtime, which llama.cpp's Linux builds need and a bare image lacks
# (`ubuntu:24.04`, measured 2026-10-02). The system is stubbed — `ldconfig`
# answers from a state file, `apt-get` records what it was asked and "installs"
# by writing that file, `sudo` runs what it is given — so every arm runs the
# same as root or not, on any image, with no network. On a Mac only the control
# arm runs: its llama.cpp has no OpenMP, so nothing is ever asked for.
echo "== OpenMP for llama.cpp"
STUBS="$WORK/stubs"
STATE="$WORK/state"
mkdir -p "$STUBS" "$STATE"
cat >"$STUBS/ldconfig" <<EOF
#!/bin/sh
[ -e "$STATE/gomp" ] && printf '\tlibgomp.so.1 (libc6,x86-64) => /usr/lib/libgomp.so.1\n'
printf '\tlibc.so.6 (libc6,x86-64) => /usr/lib/libc.so.6\n'
EOF
cat >"$STUBS/apt-get" <<EOF
#!/bin/sh
echo "\$*" >>"$STATE/apt.log"
[ -e "$STATE/apt-fails" ] && exit 100
case "\$*" in *libgomp1*) : >"$STATE/gomp" ;; esac
exit 0
EOF
cat >"$STUBS/sudo" <<'EOF'
#!/bin/sh
[ "$1" = -n ] && shift
[ "$1" = true ] && exit 0
exec "$@"
EOF
chmod 0755 "$STUBS/ldconfig" "$STUBS/apt-get" "$STUBS/sudo"
fresh() { rm -f "$STATE/gomp" "$STATE/apt.log" "$STATE/apt-fails"; }
asked() { [ -e "$STATE/apt.log" ] && grep -q "install.*libgomp1" "$STATE/apt.log"; }
omp() { PATH="$STUBS:$PATH" sh "$S" --from "$WORK/rel" --dir "$WORK/opt-omp" --no-link "$@" 2>&1; }

if [ "$OS" = macos ]; then
fresh
out="$(omp -- setup --llama metal)"
check "macOS: the hand-over installs llama.cpp" 0 $?
if [ -e "$STATE/apt.log" ]; then apt_asked=0; else apt_asked=1; fi
check "…and no OpenMP is asked for" 1 "$apt_asked"
echo "$out" | grep -q "^ARGC:3$"
check "…and the hand-over goes on" 0 $?
else

fresh
out="$(omp -- setup --llama cuda-12)"
code=$?
echo "$out" | sed 's/^/     | /'
check "missing, and the hand-over installs llama.cpp" 0 $code
asked
check "…libgomp1 is installed" 0 $?
echo "$out" | grep -q "^ARGC:3$"
check "…and the hand-over goes on" 0 $?

fresh
omp -- setup --ctx 8192 >/dev/null
asked
check "control arm: a hand-over that installs no llama.cpp asks for nothing" 1 $?

fresh
omp -- llama setup --backend cpu >/dev/null
asked
check "\`llama setup\` installs llama.cpp too" 0 $?

fresh
: >"$STATE/gomp"
omp -- setup --llama=cuda-12 >/dev/null
[ -e "$STATE/apt.log" ]
check "present: nothing is installed" 1 $?

fresh
out="$(omp --no-deps -- setup --llama cuda-12)"
code=$?
check "--no-deps: no install, and the hand-over goes on" 0 $code
asked
check "…apt-get is not asked" 1 $?
echo "$out" | grep -q "apt-get install -y libgomp1"
check "…and the line to run is printed" 0 $?

fresh
: >"$STATE/apt-fails"
out="$(omp -- setup --llama cuda-12)"
code=$?
check "an install that fails does not stop the hand-over" 0 $code
echo "$out" | grep -q "could not install it" && echo "$out" | grep -q "apt-get install -y libgomp1"
check "…it says so and names the line to run" 0 $?
fi

# Which build a machine gets is the machine's: stubs of `uname` and `sysctl`
# stand in for the systems this runner is not, so every arm runs everywhere.
echo "== the platform"
PLAT="$WORK/plat"
mkdir -p "$PLAT" "$WORK/empty"
plat() { # uname -s, uname -m, hw.optional.arm64 — then install.sh's arguments
    cat >"$PLAT/uname" <<EOF
#!/bin/sh
case "\$1" in -s) echo "$1" ;; -m) echo "$2" ;; *) echo "$1" ;; esac
EOF
    cat >"$PLAT/sysctl" <<EOF
#!/bin/sh
echo "$3"
EOF
    chmod 0755 "$PLAT/uname" "$PLAT/sysctl"
    shift 3
    PATH="$PLAT:$PATH" sh "$S" "$@" 2>&1
}
out="$(plat FreeBSD amd64 0 --from "$WORK/empty" --dir "$WORK/opt-plat")"
check "another OS is refused" 1 $?
echo "$out" | grep -q "Apple Silicon"
check "…naming the two builds there are" 0 $?
out="$(plat Darwin x86_64 0 --from "$WORK/empty" --dir "$WORK/opt-plat")"
check "an Intel Mac is refused" 1 $?
echo "$out" | grep -q "Intel Mac has none"
check "…saying why and what to do" 0 $?
out="$(plat Darwin x86_64 1 --from "$WORK/empty" --dir "$WORK/opt-plat")"
echo "$out" | grep -q "aarch64-macos"
check "a Rosetta shell on Apple Silicon looks for the Apple Silicon build" 0 $?

if [ "$OS" = macos ]; then
echo "== a browser's quarantine (macOS)"
mkdir -p "$WORK/relq" && cp "$WORK/rel"/* "$WORK/relq/"
xattr -w com.apple.quarantine "0081;66e00000;Safari;" "$WORK/relq/mindfork-rs-v9.9.9-$SUFFIX.tar.gz"
out="$(sh "$S" --from "$WORK/relq" --dir "$WORK/opt-q" --no-link 2>&1)"
check "a quarantined archive installs" 0 $?
echo "$out" | grep -q "xattr -dr com.apple.quarantine"
check "…and says how to clear the mark" 0 $?
out="$(sh "$S" --from "$WORK/rel" --dir "$WORK/opt-nq" --no-link 2>&1)"
echo "$out" | grep -q "quarantine"
check "control arm: an archive with no mark says nothing of it" 1 $?
fi

echo "== refusals"
mkdir -p "$WORK/bad" && cp "$WORK/rel"/* "$WORK/bad/"
printf 'x' >>"$WORK/bad/mindfork-rs-v9.9.9-$SUFFIX.tar.gz"
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
