#!/usr/bin/env python3
"""Keeps Formula/mindfork.rb at mindfork's latest release that has a macOS build.

Run by .github/workflows/bump.yml on a schedule, as Scoop's Excavator keeps the
Scoop bucket: it reads the latest release of vshylov/mindfork-rs, and if that
release carries `mindfork-rs-<tag>-aarch64-macos.tar.gz`, renders formula.rb.in
with the archive's URL and the digest the release's own `sha256sums.txt` lists
for it. Nothing changes when the formula already says that — or when the release
has no macOS build, as no release before macOS's first does.

    python3 bump.py                         # the latest release
    python3 bump.py --url URL --sha256 HEX  # any archive (a test of the formula)
    python3 bump.py --self-test             # the parsing, against fixtures

Writes `changed=true|false` and `version=` to $GITHUB_OUTPUT when it is set.
Standard library only: a runner has nothing else to offer it.
"""

import argparse
import json
import os
import re
import sys
import urllib.request
from pathlib import Path

REPO = "vshylov/mindfork-rs"
HERE = Path(__file__).resolve().parent
TEMPLATE = HERE / "formula.rb.in"
FORMULA = HERE / "Formula" / "mindfork.rb"
TAG = re.compile(r"^v(\d+\.\d+\.\d+)$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")


def archive_name(tag: str) -> str:
    return f"mindfork-rs-{tag}-aarch64-macos.tar.gz"


def get(url: str) -> bytes:
    headers = {"User-Agent": "vshylov-homebrew-tap-bump"}
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as r:
        return r.read()


def digest_for(sums: str, name: str) -> str:
    """The digest `sha256sums.txt` lists for `name` — exactly one line of it.

    The release writes `<hash>  ./<name>`; `<hash>  <name>` and `<hash> *<name>`
    are read too.
    """
    found = []
    for line in sums.splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        listed = parts[1].removeprefix("*").removeprefix("./")
        if listed == name:
            found.append(parts[0].lower())
    if len(found) != 1:
        raise SystemExit(f"sha256sums.txt lists {name} {len(found)} times, not once")
    if not SHA256.match(found[0]):
        raise SystemExit(f"sha256sums.txt's digest for {name} is not a sha256: {found[0]!r}")
    return found[0]


def render(url: str, sha256: str) -> str:
    if not url.startswith(("https://", "file://")):
        raise SystemExit(f"not a URL the formula can fetch: {url!r}")
    if not SHA256.match(sha256):
        raise SystemExit(f"not a sha256: {sha256!r}")
    text = TEMPLATE.read_text(encoding="utf-8")
    for mark in ("@URL@", "@SHA256@"):
        if text.count(mark) != 1:
            raise SystemExit(f"formula.rb.in must hold {mark} exactly once")
    return text.replace("@URL@", url).replace("@SHA256@", sha256)


def latest() -> tuple[str, str, str] | None:
    """(version, url, sha256) of the latest release's macOS build, or None."""
    release = json.loads(get(f"https://api.github.com/repos/{REPO}/releases/latest"))
    tag = release["tag_name"]
    m = TAG.match(tag)
    if not m:
        raise SystemExit(f"the latest release's tag is not vX.Y.Z: {tag!r}")
    name = archive_name(tag)
    if name not in {a["name"] for a in release.get("assets", [])}:
        print(f"{tag} has no macOS build ({name}) - nothing to do")
        return None
    base = f"https://github.com/{REPO}/releases/download/{tag}"
    sums = get(f"{base}/sha256sums.txt").decode("utf-8")
    return m.group(1), f"{base}/{name}", digest_for(sums, name)


def output(**values: str) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as f:
            for k, v in values.items():
                f.write(f"{k}={v}\n")


def self_test() -> int:
    sums = "\n".join([
        "a" * 64 + "  ./mindfork-rs-v1.2.3-aarch64-macos.tar.gz",
        "b" * 64 + "  ./mindfork-rs-v1.2.3-x86_64-linux.tar.gz",
        "c" * 64 + " *install.sh",
    ])
    assert digest_for(sums, "mindfork-rs-v1.2.3-aarch64-macos.tar.gz") == "a" * 64
    assert digest_for(sums, "install.sh") == "c" * 64
    for bad in (sums + "\n" + "d" * 64 + "  mindfork-rs-v1.2.3-aarch64-macos.tar.gz", ""):
        try:
            digest_for(bad, "mindfork-rs-v1.2.3-aarch64-macos.tar.gz")
        except SystemExit:
            continue
        raise AssertionError("a name listed twice, or not at all, was accepted")
    text = render("https://example.com/a.tar.gz", "e" * 64)
    assert 'url "https://example.com/a.tar.gz"' in text and "@" + "URL@" not in text
    for url, sha in (("http://example.com/a.tar.gz", "e" * 64), ("https://x/a", "E" * 64)):
        try:
            render(url, sha)
        except SystemExit:
            continue
        raise AssertionError(f"render accepted {url!r}, {sha!r}")
    assert archive_name("v1.2.3") == "mindfork-rs-v1.2.3-aarch64-macos.tar.gz"
    assert not TAG.match("v1.2.3-rc1"), "a prerelease is not the latest release"
    print("self-test ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--url", help="an archive to point the formula at, instead of the latest release's")
    ap.add_argument("--sha256", help="its digest (with --url)")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    if bool(args.url) != bool(args.sha256):
        raise SystemExit("--url and --sha256 go together")
    if args.url:
        version, url, sha256 = "test", args.url, args.sha256
    else:
        found = latest()
        if found is None:
            output(changed="false")
            return 0
        version, url, sha256 = found
    text = render(url, sha256)
    if FORMULA.exists() and FORMULA.read_text(encoding="utf-8") == text:
        print(f"Formula/mindfork.rb already says {version}")
        output(changed="false", version=version)
        return 0
    FORMULA.parent.mkdir(parents=True, exist_ok=True)
    FORMULA.write_text(text, encoding="utf-8", newline="\n")
    print(f"Formula/mindfork.rb -> {version} ({url})")
    output(changed="true", version=version)
    return 0


if __name__ == "__main__":
    sys.exit(main())
