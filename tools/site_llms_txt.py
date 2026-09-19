#!/usr/bin/env python3
"""Generate the website's /llms.txt from the site's own content.

`llms.txt` (https://llmstxt.org/) is a proposed convention: one Markdown file
at the site root that hands a language model a curated map of the site instead
of leaving it to infer one from navigation chrome. An H1 naming the project, a
blockquote summarising it, prose, then H2 sections of `[name](url): notes`
links — with an `Optional` section for what an agent may skip when it needs a
shorter context.

**What this is not.** There is no evidence that it affects ranking anywhere,
and no search engine documents consuming it; it is answered for by the two
crawlers that have started fetching it and by anyone pasting the URL into a
chat. It costs one generated file, and that is the whole of the claim.

**Generated, not written.** A hand-kept list would be wrong the day an article
is added — the drift `site_legal_pages.py` exists to prevent, one surface
further. So the titles, descriptions and ordering come from the same front
matter the pages themselves render from, `--check` fails a pull request that
edits one without regenerating, and the file is committed rather than
gitignored because it is prose that lands on a public URL and belongs in a diff
a human reads.

**Why the front matter is parsed by hand.** `tomllib` arrived in Python 3.11
and the development machines here are not all on it; a gate whose *output*
depended on the interpreter version would compare unequal between CI and a
laptop, which is worse than parsing the handful of scalar keys this needs. Anything
the simple form cannot express is a refusal, naming the line — the tool never
guesses.

Usage:
    python tools/site_llms_txt.py            # (re)generate
    python tools/site_llms_txt.py --check    # verify it matches (CI)
    python tools/site_llms_txt.py --self-test
"""

from __future__ import annotations

import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = "site/static/llms.txt"

#: A front-matter line this tool understands: key = "value", no escapes. The
#: value is captured lazily to the last quote, so an inner `"` is caught by the
#: refusal below rather than silently truncating the text.
SCALAR = re.compile(r'^(\w+)\s*=\s*"(.*)"\s*$')

#: A leading date on a post's filename: Zola takes it for the page's date and
#: drops it from the URL, so the slug is what follows.
DATED = re.compile(r"^\d{4}-\d{2}-\d{2}-(.+)$")

#: Zola's name for a section's own page - the home page's, and the heading of
#: `articles/` and `blog/`. It is front matter for the section, never an entry
#: in one, so a listing skips it and the home page is read by name.
INDEX_FILE = "_index.md"

#: The pages that answer "may I", not "what is" - the convention's own bucket
#: for links an agent can skip. Order is the footer's.
OPTIONAL = ["privacy", "disclaimer", "license", "code-signing-policy"]

#: One authored line, the way `site_legal_pages.py` authors a page title: the
#: repository is not a content file and has no front matter to read it from.
REPO_NOTE = "the source, the issue tracker and the releases"


class Refusal(Exception):
    """A condition the caller must fix; the message is the whole explanation."""


def front_matter(path: Path) -> dict[str, str]:
    """The `+++` block's simple scalars, refusing anything it cannot read."""
    text = path.read_text(encoding="utf-8")
    parts = text.split("+++", 2)
    if len(parts) < 3:
        raise Refusal(f"{path}: no `+++` front matter")
    out: dict[str, str] = {}
    for line in parts[1].splitlines():
        line = line.strip()
        if not line or line.startswith("#") or line.startswith("["):
            continue
        match = SCALAR.match(line)
        if not match:
            # The two other shapes that matter here, kept as text: a number
            # (`weight = 3`) and a bare boolean (`draft = true`). Anything else
            # is a shape this tool would have to guess at, so it is skipped -
            # a key it does not read cannot be one it gets wrong.
            if re.match(r"^\w+\s*=\s*([\d.]+|true|false)$", line):
                key, value = line.split("=", 1)
                out[key.strip()] = value.strip()
            continue
        key, value = match.group(1), match.group(2)
        if '\\"' in value or '"' in value:
            raise Refusal(
                f"{path}: `{key}` contains a quote this tool does not parse — "
                "teach it, or the generated file would be silently truncated"
            )
        out[key] = value
    return out


def guard(root: Path) -> None:
    """The URL derivation below assumes Zola's defaults; check they hold."""
    for md in sorted((root / "site" / "content").rglob("*.md")):
        text = md.read_text(encoding="utf-8")
        head = text.split("+++", 2)[1] if text.count("+++") >= 2 else ""
        for key in ("path", "slug"):
            if re.search(rf"^{key}\s*=", head, re.M):
                raise Refusal(
                    f"{md.relative_to(root).as_posix()} sets `{key}`, so its URL "
                    "is not the one derived from its filename — teach this tool "
                    "before it publishes a wrong link"
                )


def url_of(root: Path, md: Path, base: str) -> str:
    """The page's address, by Zola's own rules for these files."""
    rel = md.relative_to(root / "site" / "content")
    stem = rel.stem
    dated = DATED.match(stem)
    if dated:
        stem = dated.group(1)
    parent = rel.parent.as_posix()
    prefix = "" if parent == "." else f"{parent}/"
    return f"{base}/{prefix}{stem}/"


def entries(root: Path, folder: str, base: str, *, by: str) -> list[str]:
    """One `- [title](url): description` line per page, in the site's order."""
    directory = root / "site" / "content" / folder
    pages = []
    for md in sorted(directory.glob("*.md")):
        if md.name == INDEX_FILE:
            continue
        meta = front_matter(md)
        # A draft is not built, so listing it would publish a 404.
        if meta.get("draft") == "true":
            continue
        pages.append((md, meta))
    if by == "weight":
        pages.sort(key=lambda p: (float(p[1].get("weight", 1e9)), p[0].name))
    else:
        pages.sort(key=lambda p: p[0].name, reverse=True)
    lines = []
    for md, meta in pages:
        title = meta.get("title") or md.stem
        line = f"- [{title}]({url_of(root, md, base)})"
        if meta.get("description"):
            line += f": {meta['description']}"
        lines.append(line)
    return lines


def render(root: Path) -> str:
    """The whole file, from the site's own front matter."""
    guard(root)
    config = (root / "site" / "zola.toml").read_text(encoding="utf-8")

    def conf(key: str) -> str:
        match = re.search(rf'^{key}\s*=\s*"([^"]*)"', config, re.M)
        if not match:
            raise Refusal(f"site/zola.toml: no `{key}`")
        return match.group(1)

    base = conf("base_url").rstrip("/")
    home = front_matter(root / "site" / "content" / INDEX_FILE)
    install = front_matter(root / "site" / "content" / "install.md")

    out = [f"# {conf('title')}", "", f"> {conf('description')}", ""]
    if home.get("hero_sub"):
        out += [home["hero_sub"], ""]

    out += ["## Start here", ""]
    out.append(f"- [Install]({base}/install/): {install['description']}")
    out.append(f"- [Source on GitHub]({conf('github')}): {REPO_NOTE}")
    out.append("")

    for heading, folder, by in [("Articles", "articles", "weight"),
                                ("News", "blog", "date")]:
        lines = entries(root, folder, base, by=by)
        if lines:
            out += [f"## {heading}", "", *lines, ""]

    out += ["## Optional", ""]
    for name in OPTIONAL:
        meta = front_matter(root / "site" / "content" / f"{name}.md")
        line = f"- [{meta['title']}]({base}/{name}/)"
        if meta.get("description"):
            line += f": {meta['description']}"
        out.append(line)
    out.append("")
    return "\n".join(out)


def self_test() -> int:
    failures: list[str] = []
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        content = tmp / "site" / "content"
        (content / "articles").mkdir(parents=True)
        (content / "blog").mkdir(parents=True)
        (tmp / "site" / "zola.toml").write_text(
            'base_url = "https://example.test"\ntitle = "proj"\n'
            'description = "a summary"\ngithub = "https://github.com/o/r"\n',
            encoding="utf-8",
        )
        (content / INDEX_FILE).write_text(
            '+++\ntitle = "t"\n\n[extra]\nhero_sub = "the lede"\n+++\n', encoding="utf-8")
        (content / "install.md").write_text(
            '+++\ntitle = "Install"\ndescription = "how to get it"\n+++\n', encoding="utf-8")
        for name in OPTIONAL:
            (content / f"{name}.md").write_text(
                f'+++\ntitle = "{name}"\ndescription = "d"\n+++\n', encoding="utf-8")
        (content / "articles" / "second.md").write_text(
            '+++\ntitle = "Second"\ndescription = "b"\nweight = 2\n+++\n', encoding="utf-8")
        (content / "articles" / "first.md").write_text(
            '+++\ntitle = "First"\ndescription = "a"\nweight = 1\n+++\n', encoding="utf-8")
        (content / "blog" / "2026-01-01-old.md").write_text(
            '+++\ntitle = "Old"\ndescription = "o"\n+++\n', encoding="utf-8")
        # Zola does not build a draft, so a link to one would be a 404.
        (content / "blog" / "2026-03-01-draft.md").write_text(
            '+++\ntitle = "Unfinished"\ndescription = "d"\ndraft = true\n+++\n',
            encoding="utf-8")
        (content / "blog" / "2026-02-01-new.md").write_text(
            '+++\ntitle = "New"\ndescription = "n"\n+++\n', encoding="utf-8")

        text = render(tmp)
        checks = [
            ("H1 first", text.startswith("# proj\n")),
            ("summary blockquote", "\n> a summary\n" in text),
            ("lede", "\nthe lede\n" in text),
            ("weight order", text.index("First") < text.index("Second")),
            ("newest post first", text.index("](https://example.test/blog/new/")
             < text.index("](https://example.test/blog/old/")),
            ("date stripped from the post URL", "/blog/2026-02-01-new/" not in text),
            ("Optional section last", text.rindex("## Optional") > text.rindex("## News")),
            ("repository listed", "https://github.com/o/r" in text),
            ("draft left out", "Unfinished" not in text),
        ]
        for label, ok in checks:
            if not ok:
                failures.append(label)

        # A `slug` override would publish a wrong link, so it must refuse.
        (content / "articles" / "first.md").write_text(
            '+++\ntitle = "First"\nslug = "elsewhere"\n+++\n', encoding="utf-8")
        try:
            render(tmp)
        except Refusal as exc:
            if "slug" not in str(exc):
                failures.append(f"slug guard refused for the wrong reason: {exc}")
        else:
            failures.append("a `slug` override was accepted")

    for line in failures:
        print(f"self-test: {line}", file=sys.stderr)
    print(f"site_llms_txt --self-test: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    args = set(sys.argv[1:])
    if args - {"--check", "--self-test"}:
        print(__doc__, file=sys.stderr)
        return 2
    if "--self-test" in args:
        return self_test()
    try:
        wanted = render(ROOT)
    except Refusal as exc:
        print(f"llms.txt: {exc}", file=sys.stderr)
        return 1
    out = ROOT / OUT
    current = out.read_text(encoding="utf-8") if out.exists() else None
    if "--check" in args:
        if current == wanted:
            print(f"OK: {OUT} matches the site's content")
            return 0
        print(f"stale: {OUT} does not match the site's content\n"
              "regenerate: python tools/site_llms_txt.py", file=sys.stderr)
        return 1
    if current == wanted:
        print(f"unchanged: {OUT}")
        return 0
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(wanted, encoding="utf-8")
    print(f"written: {OUT} ({len(wanted)} chars)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
