#!/usr/bin/env python3
"""Hold the site's deploy until the version it describes is a published release.

The release pull request carries the release's own news — the post, and
`site/zola.toml`'s `app_version` — and `site.yml` deploys on every push to
`main` that touches `site/`. So the site used to say "0.10.2 is out" from the
moment that pull request merged, while the release was still a tag being built,
then a draft being smoke-tested: 25 min 28 s on 0.10.2, measured
(docs/journal/release.md). This gate closes that window from the site's side:

    Cargo.toml says X.Y.Z  ->  is `vX.Y.Z` a published release?
        yes        deploy
        no         hold: the run is green, the deploy job is skipped, a notice
                   says what will release it
        cannot say fail: a deploy that went out on a guess is the thing this
                   exists to prevent, and a rerun costs a minute

**Why `Cargo.toml` and not `app_version`.** `Cargo.toml` is the version's source
of truth (AGENTS.md §6) and `release_guard.py` already refuses a tag that
disagrees with it. A release pull request that forgot to bump `app_version`
would otherwise open the gate for its own post.

**Why the `draft` field is read even though the endpoint is documented to return
published releases only.** That documentation was not measured here: no draft
existed when this was written. A token with push access may be shown one, and
reading the field makes the answer right either way.

**What lets a held deploy go.** Nothing in this file. `site.yml` also runs when
the `crates.io` workflow completes; that workflow starts on `release: published`,
so by then the release is public and the crate is on the registry, and the same
question gets the other answer. `--force` is the owner's override for a release
that stalled while the site needs a fix: it asks nothing and deploys `main`.

Usage:
    python tools/site_release_gate.py            ask GitHub, write `deploy=...`
    python tools/site_release_gate.py --force    deploy regardless, ask nothing
    python tools/site_release_gate.py --self-test

Reads `GITHUB_REPOSITORY` (else `Cargo.toml`'s `repository`), `GITHUB_TOKEN`
(optional here, needed on a shared runner where the anonymous rate limit is
spent by strangers), and appends to `GITHUB_OUTPUT` / `GITHUB_STEP_SUMMARY`
when they are set. Exit codes: 0 answered (deploy or hold), 1 cannot say,
2 usage.
"""

from __future__ import annotations

import json
import os
import re
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parent.parent

API = "https://api.github.com"
VERSION_RE = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)
REPOSITORY_RE = re.compile(
    r'^repository\s*=\s*"https://github\.com/([^/"\s]+/[^/"\s]+?)/?"', re.MULTILINE
)
REPO_SLUG = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")

DEPLOY, HOLD, UNKNOWN = "deploy", "hold", "unknown"

#: (status, body) for a URL. Injected so the self-test drives every arm of the
#: decision without a network; a status of 0 is "the request never got there".
Fetch = Callable[[str, "str | None"], "tuple[int, str]"]


class Refusal(Exception):
    """The repository itself cannot be read the way the gate needs."""


def cargo_version(manifest: str) -> str:
    match = VERSION_RE.search(manifest)
    if not match:
        raise Refusal('Cargo.toml carries no `version = "..."` line')
    return match.group(1)


def repository(manifest: str, env: dict[str, str]) -> str:
    slug = env.get("GITHUB_REPOSITORY", "").strip()
    if not slug:
        match = REPOSITORY_RE.search(manifest)
        if not match:
            raise Refusal(
                "no GITHUB_REPOSITORY, and Cargo.toml's `repository` is not a github.com URL"
            )
        slug = match.group(1)
    if not REPO_SLUG.match(slug):
        raise Refusal(f"not an owner/name repository: {slug!r}")
    return slug


def release_url(repo: str, version: str) -> str:
    return f"{API}/repos/{repo}/releases/tags/v{version}"


def classify(status: int, body: str) -> tuple[str, str]:
    """(verdict, reason) for what the releases API answered about one tag."""
    if status == 404:
        return HOLD, "there is no published release under that tag"
    if status != 200:
        return UNKNOWN, f"GitHub answered {status or 'nothing'}, which says neither"
    try:
        release = json.loads(body)
        draft, prerelease = release["draft"], release["prerelease"]
    except (ValueError, KeyError, TypeError):
        return UNKNOWN, "GitHub answered 200 with a body that is not a release"
    if draft:
        return HOLD, "the release is still a draft"
    if prerelease:
        return HOLD, "the release is a prerelease, which the site does not announce"
    return DEPLOY, "the release is published"


def http_fetch(url: str, token: str | None) -> tuple[int, str]:
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "mindfork-rs site_release_gate",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, response.read(200_000).decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read(2000).decode("utf-8", "replace")
    except (urllib.error.URLError, TimeoutError) as exc:
        print(f"site_release_gate: could not reach {url}: {exc}", file=sys.stderr)
        return 0, ""


def decide(root: Path, env: dict[str, str], fetch: Fetch, force: bool) -> tuple[str, str]:
    """(verdict, sentence). `force` asks nothing: it has to work while the API is down."""
    manifest = (root / "Cargo.toml").read_text(encoding="utf-8")
    version = cargo_version(manifest)
    if force:
        return DEPLOY, f"forced: deploying main as it is, v{version} published or not"
    repo = repository(manifest, env)
    verdict, reason = classify(*fetch(release_url(repo, version), env.get("GITHUB_TOKEN")))
    sentence = f"v{version} in {repo}: {reason}"
    if verdict == HOLD:
        sentence += (
            " - the deploy is held. Publishing the release lets it go: the `crates.io`"
            " workflow runs on the publication, and this workflow runs when that one"
            " completes. To deploy anyway, dispatch `Site` with `force`."
        )
    elif verdict == UNKNOWN:
        sentence += " - rerun the job, or dispatch `Site` with `force` to deploy regardless."
    return verdict, sentence


def report(verdict: str, sentence: str, env: dict[str, str]) -> int:
    """Say it where a person looks and where the next job reads it; the exit code."""
    if verdict == UNKNOWN:
        print(f"::error title=Site deploy: cannot tell::{sentence}")
        return 1
    if verdict == HOLD:
        print(f"::notice title=Site deploy held::{sentence}")
    else:
        print(f"site_release_gate: {sentence}")
    output, summary = env.get("GITHUB_OUTPUT"), env.get("GITHUB_STEP_SUMMARY")
    if output:
        with open(output, "a", encoding="utf-8") as handle:
            handle.write(f"deploy={'true' if verdict == DEPLOY else 'false'}\n")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(f"**Site deploy: {verdict}.** {sentence}\n")
    return 0


# --------------------------------------------------------------------- tests
FAKE_REPO = "example/project"
FAKE_MANIFEST = (
    '[package]\nname = "project"\nversion = "1.2.3"\n'
    f'repository = "https://github.com/{FAKE_REPO}"\n\n'
    '[dependencies]\nother = { version = "9.9.9" }\n'
)


def _release(draft: bool, prerelease: bool) -> str:
    return json.dumps({"tag_name": "v1.2.3", "draft": draft, "prerelease": prerelease})


def _check_classify(failures: list[str]) -> None:
    cases = [
        ("published", 200, _release(False, False), DEPLOY),
        ("a draft the token was shown", 200, _release(True, False), HOLD),
        ("a prerelease", 200, _release(False, True), HOLD),
        ("no such release", 404, '{"message":"Not Found"}', HOLD),
        ("200 without the fields", 200, '{"tag_name":"v1.2.3"}', UNKNOWN),
        ("200 that is not JSON", 200, "<html>", UNKNOWN),
        ("200 that is not an object", 200, "[]", UNKNOWN),
        ("never got there", 0, "", UNKNOWN),
    ]
    cases += [(f"status {code}", code, "", UNKNOWN) for code in (301, 401, 403, 429, 500, 502, 503)]
    for label, status, body, want in cases:
        got, _ = classify(status, body)
        if got != want:
            failures.append(f"classify, {label}: {got}, wanted {want}")


def _check_reading(failures: list[str]) -> None:
    if cargo_version(FAKE_MANIFEST) != "1.2.3":
        failures.append("the package version is not the first `version =` line")
    if repository(FAKE_MANIFEST, {}) != FAKE_REPO:
        failures.append("the repository is not read from Cargo.toml")
    if repository(FAKE_MANIFEST, {"GITHUB_REPOSITORY": "fork/project"}) != "fork/project":
        failures.append("GITHUB_REPOSITORY does not win over Cargo.toml")
    refusals = [
        ("no version", lambda: cargo_version("[package]\nname = 'x'\n")),
        ("no repository", lambda: repository('version = "1.0.0"\n', {})),
        ("a slug with a path in it", lambda: repository("", {"GITHUB_REPOSITORY": "a/b/c"})),
    ]
    for label, call in refusals:
        try:
            call()
            failures.append(f"{label}: accepted")
        except Refusal:
            pass
    url = release_url(FAKE_REPO, "1.2.3")
    if url != "https://api.github.com/repos/example/project/releases/tags/v1.2.3":
        failures.append(f"the release URL is {url}")


def _run(tmp: Path, fetch: Fetch, force: bool, token: str | None = None) -> tuple[int, str, str]:
    """(exit code, what was written to GITHUB_OUTPUT, the sentence) for one fixture run."""
    (tmp / "Cargo.toml").write_text(FAKE_MANIFEST, encoding="utf-8")
    output = tmp / "output"
    output.write_text("", encoding="utf-8")
    env = {"GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(tmp / "summary")}
    if token:
        env["GITHUB_TOKEN"] = token
    verdict, sentence = decide(tmp, env, fetch, force)
    code = report(verdict, sentence, env)
    return code, output.read_text(encoding="utf-8"), sentence


def _check_decide(failures: list[str]) -> None:
    seen: list[tuple[str, str | None]] = []

    def answering(status: int, body: str) -> Fetch:
        def fetch(url: str, token: str | None) -> tuple[int, str]:
            seen.append((url, token))
            return status, body

        return fetch

    def unreachable(url: str, token: str | None) -> tuple[int, str]:
        raise AssertionError("a forced deploy asked the network")

    runs = [
        ("published", answering(200, _release(False, False)), False, 0, "deploy=true\n"),
        ("not published", answering(404, ""), False, 0, "deploy=false\n"),
        ("a draft", answering(200, _release(True, False)), False, 0, "deploy=false\n"),
        ("the API is down", answering(503, ""), False, 1, ""),
        ("forced while the API is down", unreachable, True, 0, "deploy=true\n"),
    ]
    with tempfile.TemporaryDirectory() as raw:
        for index, (label, fetch, force, want_code, want_output) in enumerate(runs):
            tmp = Path(raw) / str(index)
            tmp.mkdir()
            code, output, sentence = _run(tmp, fetch, force, token="t0ken")
            if (code, output) != (want_code, want_output):
                failures.append(f"decide, {label}: exit {code} with {output!r}")
            if "1.2.3" not in sentence:
                failures.append(f"decide, {label}: the sentence does not name the version")
            if label == "not published" and "force" not in sentence:
                failures.append("a held deploy does not say how to release it")
    if not seen or any(entry != (release_url(FAKE_REPO, "1.2.3"), "t0ken") for entry in seen):
        failures.append(f"the fetch was not given the release URL and the token: {seen[:1]}")


def self_test() -> int:
    """Drive every arm against fixtures, so a deploy never discovers one."""
    failures: list[str] = []
    _check_classify(failures)
    _check_reading(failures)
    # `report` prints workflow commands; keep the self-test's own output to one line.
    stdout, sys.stdout = sys.stdout, open(os.devnull, "w", encoding="utf-8")
    try:
        _check_decide(failures)
    finally:
        sys.stdout.close()
        sys.stdout = stdout
    if not API.startswith("https://"):
        failures.append(f"the API must be https, not {API!r}")
    for line in failures:
        print(f"self-test: {line}", file=sys.stderr)
    print(f"site_release_gate --self-test: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    args = set(sys.argv[1:])
    if args - {"--force", "--self-test"}:
        print(__doc__, file=sys.stderr)
        return 2
    if "--self-test" in args:
        return self_test()
    env = dict(os.environ)
    try:
        verdict, sentence = decide(ROOT, env, http_fetch, force="--force" in args)
    except Refusal as exc:
        print(f"::error title=Site deploy: cannot tell::{exc}")
        return 1
    return report(verdict, sentence, env)


if __name__ == "__main__":
    raise SystemExit(main())
