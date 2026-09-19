#!/usr/bin/env python3
"""Tell the IndexNow engines that the site changed.

A sitemap is an invitation to come back sometime; IndexNow is a knock on the
door. One POST after a deploy names the URLs that may have changed, and the
participating engines — Bing above all, which also feeds DuckDuckGo and
ChatGPT's search — fetch them instead of waiting for their own schedule.
Google does not participate, so this complements Search Console rather than
replacing anything. Protocol: https://www.indexnow.org/documentation

**The key is not a secret.** Ownership is proved by serving the key back from
the host at `https://<host>/<key>.txt`, so it is public by construction —
exactly like the DNS TXT record that proves the domain to Google. It is
committed for the same reason: the alternative is a value that lives only on
a deployed server, where nothing reviews it. The worst a stranger who reads it
can do is tell an engine to re-crawl pages that are already ours.

**The key file is the single source of truth.** There is no second copy in a
config file to drift from it: this script finds the one file under
`site/static/` whose name is a key and whose contents are that same key, and
refuses if there is none, if there are two — a rotation that left the old one
behind — or if the contents disagree with the name. `--check` runs that in
CI's lint job, where it costs nothing and fails on a pull request rather than
on a deploy.

**Why a script rather than a `run:` block**: a workflow's refusal paths are
only ever reached by the thing they guard going wrong, which makes a deploy
the most expensive place to discover a typo in them (docs/lessons.md §10).
`--self-test` drives every refusal against fixtures and runs in `lint`.

Usage:
    python tools/indexnow.py --check      # the key file is well-formed (CI lint)
    python tools/indexnow.py --self-test  # the refusal paths (CI lint)
    python tools/indexnow.py --submit     # POST the built sitemap's URLs
    python tools/indexnow.py --submit --dry-run   # ...print it instead
"""

from __future__ import annotations

import json
import re
import sys
import tempfile
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: The shared endpoint: it forwards a submission to every participating engine,
#: so there is no list of per-engine URLs here to go stale as that set changes.
ENDPOINT = "https://api.indexnow.org/indexnow"

#: What the protocol accepts as a key: 8-128 of these characters.
KEY = re.compile(r"^[A-Za-z0-9-]{8,128}$")

#: The protocol's own ceiling on one request.
MAX_URLS = 10_000

#: Codes that mean the submission landed. 202 is "received, key not validated
#: yet", which is the ordinary answer the first time a key is used.
ACCEPTED = {200, 202}

#: Codes that are the service's problem or its rate limiter, not a defect here:
#: worth saying out loud, not worth failing a deploy that already succeeded.
TRANSIENT = {429, 500, 502, 503, 504}

SITEMAP_NS = {"sm": "http://www.sitemaps.org/schemas/sitemap/0.9"}


class Refusal(Exception):
    """A condition the caller must fix; the message is the whole explanation."""


def base_url(root: Path) -> str:
    """The site's own address, from the one file that declares it."""
    config = root / "site" / "zola.toml"
    text = config.read_text(encoding="utf-8")
    match = re.search(r'^base_url\s*=\s*"([^"]+)"', text, re.M)
    if not match:
        raise Refusal(f"{config}: no `base_url` to read the host from")
    return match.group(1).rstrip("/")


def find_key(root: Path) -> str:
    """The key, from the single file under site/static/ that carries it.

    What identifies a key file is not its name alone — `security.txt` is eight
    legal characters and would pass that test — but that its contents are its
    own name, which is exactly what an engine fetches the file to check. So a
    file whose name could be a key but whose body is not it is reported as a
    near miss rather than counted, which is what an edited key file looks like.
    """
    static = root / "site" / "static"
    named = [p for p in sorted(static.glob("*.txt")) if KEY.match(p.stem)]
    keys, near = [], []
    for path in named:
        (keys if path.read_text(encoding="utf-8").strip() == path.stem else near).append(path)
    if len(keys) > 1:
        names = ", ".join(p.name for p in keys)
        raise Refusal(
            f"more than one IndexNow key file ({names}) — a rotation must "
            "delete the old one, or the host serves two valid keys and the "
            "single source of truth is gone"
        )
    if keys:
        return keys[0].stem
    if near:
        path = near[0]
        body = path.read_text(encoding="utf-8").strip()
        raise Refusal(
            f"{path.name} contains {body!r}, which is not its own name — an "
            "engine fetches this file and compares, so the two must agree"
        )
    raise Refusal(
        f"no IndexNow key file in {static.relative_to(root).as_posix()}/ — "
        "it is named <key>.txt and contains that key, and nothing else holds "
        "the key"
    )


def sitemap_urls(root: Path) -> list[str]:
    """Every URL of the built site, from the sitemap it already publishes."""
    sitemap = root / "site" / "public" / "sitemap.xml"
    if not sitemap.exists():
        raise Refusal(
            f"{sitemap.relative_to(root).as_posix()} does not exist — build the "
            "site before submitting, or there is nothing to announce"
        )
    tree = ET.fromstring(sitemap.read_text(encoding="utf-8"))
    urls = [e.text.strip() for e in tree.findall(".//sm:loc", SITEMAP_NS) if e.text]
    if not urls:
        raise Refusal(f"{sitemap.name} lists no URLs")
    if len(urls) > MAX_URLS:
        raise Refusal(f"{len(urls)} URLs, over the protocol's {MAX_URLS} per request")
    return urls


def payload(root: Path) -> dict:
    """The request body, built from the key file and the built sitemap."""
    site = base_url(root)
    host = re.sub(r"^https?://", "", site)
    key = find_key(root)
    urls = sitemap_urls(root)
    stray = [u for u in urls if not u.startswith(site + "/") and u != site + "/"]
    if stray:
        raise Refusal(
            f"{len(stray)} sitemap URL(s) are not under {site} — the protocol "
            f"answers 422 for those, e.g. {stray[0]}"
        )
    return {
        "host": host,
        "key": key,
        "keyLocation": f"{site}/{key}.txt",
        "urlList": urls,
    }


def classify(status: int) -> tuple[int, str]:
    """(exit code, sentence) for a response status."""
    if status in ACCEPTED:
        return 0, "accepted" if status == 200 else "received, key validation pending"
    if status in TRANSIENT:
        return 0, "the service is busy or unavailable; the deploy itself is unaffected"
    if status == 403:
        return 1, "the key file was not served back, or does not match the key sent"
    if status == 422:
        return 1, "the URLs do not belong to the host, or the key does not match it"
    if status == 400:
        return 1, "the request body was rejected as malformed"
    return 1, "unexpected status"


def submit(body: dict, dry_run: bool) -> int:
    raw = json.dumps(body).encode("utf-8")
    if dry_run:
        print(f"POST {ENDPOINT}")
        print(json.dumps(body, indent=2))
        print(f"\nDRY RUN — nothing was sent ({len(body['urlList'])} URLs).")
        return 0
    request = urllib.request.Request(
        ENDPOINT,
        data=raw,
        headers={"Content-Type": "application/json; charset=utf-8"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            status, text = response.status, response.read(2000).decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        status, text = exc.code, exc.read(2000).decode("utf-8", "replace")
    except urllib.error.URLError as exc:
        print(f"indexnow: could not reach {ENDPOINT}: {exc.reason}", file=sys.stderr)
        print("indexnow: the deploy itself is unaffected.", file=sys.stderr)
        return 0
    code, sentence = classify(status)
    stream = sys.stderr if code else sys.stdout
    print(
        f"indexnow: {status} — {sentence} "
        f"({len(body['urlList'])} URLs for {body['host']})",
        file=stream,
    )
    if text.strip():
        print(f"indexnow: body: {text.strip()[:500]}", file=stream)
    return code


# --------------------------------------------------------------------- tests
#: The host the fixtures pretend to be, and the two pages they publish. Every
#: fixture has to agree with what `base_url` reads back for a payload to build
#: at all, so the spelling lives here rather than in each call below.
FAKE_SITE = "https://mindfork.io"
FAKE_HOME = f"{FAKE_SITE}/"
FAKE_INSTALL = f"{FAKE_SITE}/install/"


def _fixture(tmp: Path, *, keys: dict[str, str], urls: list[str] | None,
             base: str = FAKE_SITE) -> Path:
    """A throwaway site root: a zola.toml, the key files, maybe a sitemap.

    `urls=None` is the unbuilt site — no `public/` at all — which is a
    different refusal from `urls=[]`, a sitemap that lists nothing.
    """
    (tmp / "site" / "static").mkdir(parents=True, exist_ok=True)
    (tmp / "site" / "zola.toml").write_text(
        f'title = "x"\nbase_url = "{base}"\n', encoding="utf-8"
    )
    for name, body in keys.items():
        (tmp / "site" / "static" / name).write_text(body, encoding="utf-8")
    if urls is not None:
        (tmp / "site" / "public").mkdir(parents=True, exist_ok=True)
        locs = "".join(f"<url><loc>{u}</loc></url>" for u in urls)
        (tmp / "site" / "public" / "sitemap.xml").write_text(
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
            f"{locs}</urlset>",
            encoding="utf-8",
        )
    return tmp


def _expect_refusal(failures: list[str], label: str, root: Path, needle: str) -> None:
    """Record a failure unless `payload` refuses `root` for the stated reason."""
    try:
        payload(root)
    except Refusal as exc:
        if needle not in str(exc):
            failures.append(f"{label}: refused, but not about {needle!r}: {exc}")
    else:
        failures.append(f"{label}: was accepted, and must not be")


def _expect_key(failures: list[str], label: str, root: Path, want: str) -> None:
    """Record a failure unless `payload` builds and picks `want` as the key."""
    try:
        if payload(root)["key"] != want:
            failures.append(f"{label}: the wrong file was taken for the key")
    except Refusal as exc:
        failures.append(f"{label}: refused with a real key present: {exc}")


def _check_refusals(failures: list[str]) -> None:
    """Every refusal path of `payload`, and then the shape of a good request."""
    good = "abc123def456"
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)

        root = _fixture(tmp / "none", keys={}, urls=[FAKE_HOME])
        _expect_refusal(failures, "no key file", root, "no IndexNow key file")

        root = _fixture(tmp / "two",
                        keys={f"{good}.txt": good, "0123456789ab.txt": "0123456789ab"},
                        urls=[FAKE_HOME])
        _expect_refusal(failures, "two key files", root, "more than one")

        root = _fixture(tmp / "mismatch", keys={f"{good}.txt": "something else"},
                        urls=[FAKE_HOME])
        _expect_refusal(failures, "key file contents", root, "not its own name")

        # `security.txt` is eight legal characters; it must not read as a key,
        # nor stop the real one being found beside it.
        root = _fixture(tmp / "decoy", keys={"security.txt": "Contact: mailto:x@y",
                                             f"{good}.txt": good},
                        urls=[FAKE_HOME])
        _expect_key(failures, "decoy", root, good)

        root = _fixture(tmp / "nosite", keys={f"{good}.txt": good}, urls=None)
        _expect_refusal(failures, "unbuilt site", root, "does not exist")

        root = _fixture(tmp / "empty", keys={f"{good}.txt": good}, urls=[])
        _expect_refusal(failures, "empty sitemap", root, "lists no URLs")

        root = _fixture(tmp / "stray", keys={f"{good}.txt": good},
                        urls=[FAKE_HOME, "https://example.com/x/"])
        _expect_refusal(failures, "URL off the host", root, "not under")

        # ...and the shape of a good one.
        root = _fixture(tmp / "ok", keys={f"{good}.txt": good},
                        urls=[FAKE_HOME, FAKE_INSTALL])
        body = payload(root)
        want = {
            "host": "mindfork.io",
            "key": good,
            "keyLocation": f"{FAKE_SITE}/{good}.txt",
            "urlList": [FAKE_HOME, FAKE_INSTALL],
        }
        if body != want:
            failures.append(f"payload: {body} != {want}")


def _check_classify(failures: list[str]) -> None:
    """The response policy: a busy service must not redden a finished deploy,
    and a rejected key must not pass for success."""
    for status, wanted in [(200, 0), (202, 0), (429, 0), (503, 0),
                           (400, 1), (403, 1), (422, 1), (418, 1)]:
        got, _ = classify(status)
        if got != wanted:
            failures.append(f"classify({status}) == {got}, wanted {wanted}")


def self_test() -> int:
    """Drive every refusal against fixtures, so a deploy never discovers one."""
    failures: list[str] = []
    _check_refusals(failures)
    _check_classify(failures)
    for line in failures:
        print(f"self-test: {line}", file=sys.stderr)
    print(f"indexnow --self-test: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    args = set(sys.argv[1:])
    unknown = args - {"--check", "--submit", "--dry-run", "--self-test"}
    if unknown or not args & {"--check", "--submit", "--self-test"}:
        print(__doc__, file=sys.stderr)
        return 2
    if "--self-test" in args:
        return self_test()
    try:
        if "--check" in args:
            key = find_key(ROOT)
            site = base_url(ROOT)
            print(f"indexnow: key {key} — the host must serve it at {site}/{key}.txt")
            return 0
        return submit(payload(ROOT), dry_run="--dry-run" in args)
    except Refusal as exc:
        print(f"indexnow: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
