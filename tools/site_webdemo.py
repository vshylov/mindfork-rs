#!/usr/bin/env python3
"""Put a release's web demo where the site's build finds it.

docs/research/web-demo.md §14.3. The demo at mindfork.io/try/ is a release
asset, `mindfork-webdemo.tar`, built by release.yml from a tag. The site is
built from `main`; its deploy runs this first:

1. the newest published release - not a draft, not a prerelease - that carries
   an asset this site can show is found. None is not an error: the page says
   the demo is on its way, and the home page has no link to it. A release
   whose asset is of another manifest `format` than the site's template reads
   is passed over for the one before it, as a release with no asset is: the
   site and a release are built at different times, and neither waits;
2. the asset is downloaded and its **attestation verified** against this
   repository: what mindfork.io publishes is what release.yml built;
3. it is unpacked into `site/static/webdemo/<tag>/` - only the files its
   manifest lists, each checked against the sha256 there, under names this
   tool has vetted, and not the asset's own page, which the site replaces. An
   asset that is not what its manifest says fails the deploy;
4. `site/webdemo.json` is written: how the templates know there is a demo, and
   where. Beside the site's sources and not among its static files - it is the
   build's to read, not the site's to publish.

A directory per release, because a browser may then keep every file of it for
a year: a file's address never means two things (§14.2).

Usage:
    python tools/site_webdemo.py              # from the newest release (the deploy)
    python tools/site_webdemo.py --local      # from target/web-demo/, for a preview
    python tools/site_webdemo.py --fixture    # a stand-in, for the pull request's build
    python tools/site_webdemo.py --self-test

`--fixture` places a few bytes shaped like an asset, so that a pull request's
build renders the page as it is with a demo - the shape no release gives it
until one carries the asset, and the one a visitor will see.

It takes no path and no tag: what it reads is the repository's own releases or
the build's fixed place, and what it writes is `site/webdemo.json` and
`site/static/webdemo/`.
"""

from __future__ import annotations

import hashlib
import io
import json
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parent.parent
ASSET = "mindfork-webdemo.tar"
LOCAL_ASSET = ROOT / "target" / "web-demo" / ASSET
TARGET = ROOT / "site" / "static" / "webdemo"
POINTER = ROOT / "site" / "webdemo.json"
MANIFEST = "manifest.json"
# The manifest shape site/templates/try.html reads (tools/web_demo/manifest.py).
FORMAT = 2
# A release's tag, and nothing else, may name a directory of the site.
TAG = re.compile(r"v\d+\.\d+\.\d+")
# The names a preview's and a stand-in's directories get: no release is
# called either.
LOCAL = "local"
FIXTURE = "fixture"


class Refusal(Exception):
    """A condition the caller must fix; the message is the whole explanation."""


class OtherFormat(Refusal):
    """An asset in good order whose manifest this site's template does not
    read: not a fault of either, and not this release's turn."""


# How many releases back the deploy looks for an asset it can show.
LOOK_BACK = 3


def repository() -> str:
    """`owner/name`, from the manifest every release is built from."""
    found = re.search(r'^repository = "https://github\.com/([\w.-]+/[\w.-]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M)
    if not found:
        raise Refusal("Cargo.toml names no GitHub repository")
    return found.group(1)


def with_asset(releases: list[dict]) -> list[str]:
    """The tags of the published releases that carry the asset, newest first,
    as GitHub lists them; drafts and prereleases are passed over, and so is a
    release whose demo job failed - the one before it still has one."""
    return [
        release["tag_name"]
        for release in releases
        if not (release.get("draft") or release.get("prerelease"))
        and any(asset.get("name") == ASSET for asset in release.get("assets", []))
    ]


def vetted(name: str) -> PurePosixPath:
    """A file's name out of a manifest, as a path that stays where it is put:
    relative, with no step up and no empty or hidden part."""
    path = PurePosixPath(name)
    parts = path.parts
    if not parts or path.is_absolute() or any(part in ("", ".", "..") or part.startswith(".") for part in parts):
        raise Refusal(f"the manifest lists a file this tool will not write: {name!r}")
    return path


def published(manifest: dict) -> list[str]:
    """The files of an asset the site publishes: all the manifest lists but the
    asset's own page and what only that page needs."""
    if manifest.get("format") != FORMAT:
        raise OtherFormat(f"its manifest is format {manifest.get('format')!r}, and the site's template reads format {FORMAT}")
    alone = set(manifest["standalone"])
    for name in (manifest["fragment"], manifest["script"], manifest["stylesheet"]):
        if name not in manifest["files"] or name in alone:
            raise Refusal(f"the manifest names {name!r} for the page, and the asset does not publish it")
    return [name for name in manifest["files"] if name not in alone]


def checked(archive: bytes) -> tuple[bytes, dict[PurePosixPath, bytes]]:
    """An asset's manifest and its published files, each under a vetted name
    and each matching the sha256 the manifest gives for it."""
    files = {}
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        members = {member.name: member for member in tar.getmembers() if member.isfile()}
        if MANIFEST not in members:
            raise Refusal(f"the asset has no {MANIFEST}")
        raw = tar.extractfile(members[MANIFEST]).read()
        manifest = json.loads(raw)
        for name in published(manifest):
            path = vetted(name)
            if name not in members:
                raise Refusal(f"the manifest lists {name}, which is not in the asset")
            data = tar.extractfile(members[name]).read()
            if hashlib.sha256(data).hexdigest() != manifest["files"][name]["sha256"]:
                raise Refusal(f"{name} does not match the sha256 its manifest gives")
            files[path] = data
    return raw, files


def unpack(archive: bytes, directory: Path) -> dict:
    """Writes an asset's published files under `directory` and gives its
    manifest. Nothing in the archive decides where a file goes - the names are
    the manifest's, vetted - and nothing is written until all of it has been
    checked: an asset is placed whole or not at all."""
    raw, files = checked(archive)
    base = directory.resolve()
    for path, data in files.items():
        target = (base / path).resolve()
        if not target.is_relative_to(base):
            raise Refusal(f"{path} would be written outside {directory}")
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    (base / MANIFEST).write_bytes(raw)
    return json.loads(raw)


def place(archive: bytes, name: str) -> dict:
    """An asset as the site's demo: its files under `webdemo/<name>/`, and the
    pointer the templates read. Whatever was there before goes first - one
    demo is published at a time."""
    if TARGET.exists():
        shutil.rmtree(TARGET)
    POINTER.unlink(missing_ok=True)
    manifest = unpack(archive, TARGET / name)
    pointer = {
        "dir": name,
        "application": manifest["application"],
        "commit": manifest["commit"],
        "fragment": manifest["fragment"],
        "script": manifest["script"],
        "stylesheet": manifest["stylesheet"],
    }
    POINTER.write_text(json.dumps(pointer, indent=2) + "\n", encoding="utf-8")
    return manifest


def gh(*arguments: str) -> str:
    done = subprocess.run(["gh", *arguments], capture_output=True, text=True, cwd=ROOT)
    if done.returncode != 0:
        raise Refusal(f"gh {arguments[0]} {arguments[1]} failed: {done.stderr.strip() or done.stdout.strip()}")
    return done.stdout


def fetched(repo: str, listed: str) -> tuple[str, bytes]:
    """A release's asset, downloaded and its attestation verified; with the
    release's tag as this tool will use it."""
    named = TAG.fullmatch(listed)
    if not named:
        raise Refusal(f"the release that carries the demo is tagged {listed!r}, which is not a release's tag")
    tag = named.group(0)
    with tempfile.TemporaryDirectory() as scratch:
        gh("release", "download", tag, "--repo", repo, "--pattern", ASSET, "--dir", scratch)
        asset = Path(scratch) / ASSET
        gh("attestation", "verify", str(asset), "--repo", repo)
        return tag, asset.read_bytes()


def from_release() -> str | None:
    """Steps 1-4 of the module's text; the tag placed, or None when no
    published release carries an asset this site can show."""
    repo = repository()
    for listed in with_asset(json.loads(gh("api", f"repos/{repo}/releases?per_page=30")))[:LOOK_BACK]:
        tag, archive = fetched(repo, listed)
        try:
            place(archive, tag)
        except OtherFormat as other:
            print(f"::notice::The web demo of {tag} is passed over: {other}")
            continue
        return tag
    return None


def from_build() -> None:
    if not LOCAL_ASSET.exists():
        raise Refusal(f"no {LOCAL_ASSET.relative_to(ROOT)} - build it with tools/web_demo/build.py")
    place(LOCAL_ASSET.read_bytes(), LOCAL)


# --- the self-test ---------------------------------------------------------


def described(files: dict[str, bytes]) -> dict[str, dict]:
    return {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in files.items()}


def piece(name: str, part: str) -> dict:
    """A piece as tools/web_demo/manifest.py writes one."""
    return {
        "name": name,
        "version": "1.0",
        "part": part,
        "role": "a stand-in",
        "licence": "MIT",
        "homepage": "https://example.test/",
        "texts": ["licences/stand-in.txt"],
        "sources": [{"what": "its source", "url": "https://example.test/source"}],
        "package": "",
    }


def fixture(contents: dict[str, bytes], **changes: object) -> bytes:
    """An asset holding `contents`, with a manifest that describes them
    truthfully unless `changes` says otherwise."""
    manifest = {
        "format": FORMAT,
        "application": "1.2.3",
        "commit": "0" * 40,
        "fragment": "stage.html",
        "script": "demo.js",
        "stylesheet": "demo.css",
        "standalone": ["index.html"],
        "files": described(contents),
        "pieces": [piece("on the page", "page"), piece("of the standalone page", "standalone"), piece("in the machine", "machine")],
        **changes,
    }
    packed = io.BytesIO()
    with tarfile.open(fileobj=packed, mode="w") as tar:
        for name, data in {**contents, MANIFEST: json.dumps(manifest).encode()}.items():
            member = tarfile.TarInfo(name)
            member.size = len(data)
            tar.addfile(member, io.BytesIO(data))
    return packed.getvalue()


def refused(archive: bytes, directory: Path) -> str:
    """What `unpack` refuses an archive with; empty when it does not."""
    try:
        unpack(archive, directory)
    except Refusal as refusal:
        return str(refusal)
    return ""


FILES = {
    "stage.html": b'<div id="wd-root" class="wd-root">a stand-in for the demo</div>\n',
    "demo.js": b"// a stand-in\n",
    "demo.css": b"/* a stand-in */\n",
    "index.html": b"<html>",
    "licences/stand-in.txt": b"a stand-in\n",
    "vendor/x.bin": b"\0\1",
}


def formats_agree() -> bool:
    """The format this tool accepts is the one the asset's build writes."""
    theirs = re.search(r"^FORMAT = (\d+)$", (ROOT / "tools" / "web_demo" / "manifest.py").read_text(encoding="utf-8"), re.M)
    return bool(theirs) and int(theirs.group(1)) == FORMAT


def self_test() -> list[str]:
    problems = []

    def expect(what: str, got: object, want: object) -> None:
        if got != want:
            problems.append(f"{what}: {got!r}, not {want!r}")

    def asset(name: str) -> dict:
        return {"assets": [{"name": name}]}

    releases = [
        {"tag_name": "v9.9.9", "draft": True, **asset(ASSET)},
        {"tag_name": "v3.0.0-rc1", "prerelease": True, **asset(ASSET)},
        {"tag_name": "v2.0.0", **asset("mindfork-demo-dark-en.webp")},
        {"tag_name": "v1.9.0", **asset(ASSET)},
        {"tag_name": "v1.8.0", **asset(ASSET)},
    ]
    expect("the published releases with the asset, newest first", with_asset(releases), ["v1.9.0", "v1.8.0"])
    expect("no release with the asset", with_asset(releases[:3]), [])
    expect("a release's tag", [bool(TAG.fullmatch(tag)) for tag in ("v1.9.0", "v1.9.0-rc1", "../v1.9.0", "local")], [True, False, False, False])

    with tempfile.TemporaryDirectory() as raw:
        scratch = Path(raw)
        manifest = unpack(fixture(FILES), scratch / "good")
        written = sorted(path.relative_to(scratch / "good").as_posix() for path in (scratch / "good").rglob("*") if path.is_file())
        expect("what an asset publishes", written, ["demo.css", "demo.js", "licences/stand-in.txt", MANIFEST, "stage.html", "vendor/x.bin"])
        expect("the manifest it gives", manifest["application"], "1.2.3")

        wrong = described(FILES)
        wrong["vendor/x.bin"]["sha256"] = "f" * 64
        lying = fixture(FILES, files=wrong)
        expect("a file that is not what its manifest says", "vendor/x.bin does not match" in refused(lying, scratch / "lying"), True)
        expect("a format the template does not read", "format 1" in refused(fixture(FILES, format=1), scratch / "old"), True)
        try:
            unpack(fixture(FILES, format=FORMAT + 1), scratch / "newer")
            problems.append("an asset of a later format was placed")
        except OtherFormat:
            pass  # passed over, not a failure: the release before it is tried
        absent = fixture({name: data for name, data in FILES.items() if name != "demo.js"})
        expect("an asset without its script", "'demo.js'" in refused(absent, scratch / "absent"), True)
        for name in ("../escape.js", "/etc/passwd", "a/../../b", ".hidden", "a//b/../c"):
            climbing = fixture({**FILES, name: b"x"})
            expect(f"a file named {name}", "will not write" in refused(climbing, scratch / "climbing"), True)
        # A refused asset leaves nothing behind, in its directory or beside it.
        expect("what the refusals wrote", sorted(path.name for path in scratch.iterdir()), ["good"])
    expect("the format the asset's build writes is the one read here", formats_agree(), True)
    return problems


def main() -> int:
    arguments = sys.argv[1:]
    if arguments == ["--self-test"]:
        problems = self_test()
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        if not problems:
            print("clean: the web demo's placement refuses what it should and writes what it should")
        return 1 if problems else 0
    if arguments not in ([], ["--local"], ["--fixture"]):
        print(__doc__.strip(), file=sys.stderr)
        return 2
    try:
        if arguments == ["--local"]:
            from_build()
            print(f"the build's demo is in {TARGET.relative_to(ROOT)}/{LOCAL}")
        elif arguments:
            place(fixture(FILES), FIXTURE)
            print(f"a stand-in for the demo is in {TARGET.relative_to(ROOT)}/{FIXTURE}")
        else:
            tag = from_release()
            if tag is None:
                print("::notice::No published release carries a web demo this site can show yet - /try/ says it is on its way")
            else:
                print(f"the web demo of {tag} is in {TARGET.relative_to(ROOT)}/{tag}")
    except Refusal as refusal:
        print(f"error: {refusal}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
