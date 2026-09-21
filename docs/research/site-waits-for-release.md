# The site waits for the release

**Status:** decided 2026-09-20, implemented in the same pull request (#605). The part
verifiable only after the merge was rehearsed the same evening — **GO**, §6.

## 1. The defect

The release pull request carries the release's own news: the post
(`site/content/blog/…`) and `site/zola.toml`'s `app_version`, which the overview
article and the structured data read. `site.yml` deploys on every push to `main`
that touches `site/**`. So the site announces a version from the moment that pull
request merges — while the version is still a tag being built, then a draft being
smoke-tested.

Measured on 0.10.2 ([docs/journal/release.md](../journal/release.md), "Release
0.10.2"): the deploy finished at 17:10:23Z and the release was published at
17:35:51Z. For **25 min 28 s** mindfork.io said "0.10.2 is out" about a release the
releases page did not list, and for **28 min 05 s** it named `cargo install
mindfork` for a version the registry did not have. 15 min 32 s of that is
`release.yml` itself and most of the rest is the artifact smoke test (AGENTS.md §6
step 5), so the window cannot be closed by hurrying. 0.9.9 had the same shape.

The project's documents are otherwise written so that a sentence a reader can test
is true when they test it. This was the exception.

## 2. What constrains the answer

- **The deploy role trusts one subject.** `infra/website.cfn.yaml`'s `GitHubSub` is
  `repo:vshylov/mindfork-rs:ref:refs/heads/main`. A run on the `release` event has
  the *tag* as its ref, so it cannot assume the role. Widening the trust to
  `refs/tags/v*` would let a tag on any commit — reviewed or not, on `main` or not
  — deploy the site, which is a worse trade than the defect.
- **The agent publishes nothing** (AGENTS.md §5–§6): whatever closes the window has
  to follow from the owner's one action, publishing the draft.
- **A release is rare.** A guard that is first exercised on a release day is
  exercised on the most expensive day to find a typo in it
  ([docs/lessons.md](../lessons.md) §10) — so the logic belongs in a self-tested
  script, not in workflow YAML.

## 3. Measured

Against the live API, with the tool this pull request adds:

| Question | Answer |
|---|---|
| `GET /repos/…/releases/tags/v0.10.2` (published) | `200`, `draft: false`, `prerelease: false` → **deploy** |
| `GET /repos/…/releases/tags/v0.10.3` (no such release) | `404` → **hold**, exit 0, `deploy=false` |
| the same request with a token GitHub refuses | `401` → **cannot tell**, exit 1, nothing written |
| `--force` | asks nothing, `deploy=true` |

Not measured, because no draft existed: whether that endpoint ever returns a
*draft*. It is documented as "get a published release", and the 0.10.2 draft's own
URL was `…/releases/tag/untagged-15319c06…` — a draft is not addressable by its tag
in the web interface either. The tool reads the `draft` field regardless, so the
answer is right whichever way the API behaves; and the question can only arise for
a deploy that *runs* between the draft's creation and the publication, which needs
an unrelated site change merged in those minutes.

## 4. The forks, and what was chosen

**F1 — how the window closes.**

- **(a) Gate the deploy, and let the release's publication release it, through
  `workflow_run` on `crates.io`.** `site.yml` asks whether `v<Cargo.toml version>`
  is a published release; if not, the deploy is held. `crates.io` already starts
  on `release: published`; `site.yml` runs when it completes. A `workflow_run` run
  belongs to the default branch — it checks out `main` and its OIDC subject is
  `refs/heads/main` — so nothing about the role changes and no new permission is
  granted. The post appears about 3.5 minutes after the publication (2 min 37 s of
  `crates.io` on 0.10.2, plus a deploy), by which time the crate is on the
  registry: every sentence of the post is true when it appears. **Recommended.**
- (b) The same gate, released by a job on `release: published` that dispatches
  `site.yml` on `main`. About a minute after the publication — and about a minute
  and a half *before* the crate exists, so the post's `cargo install` sentence
  leads the registry. Needs `actions: write`.
- (c) Leave it: a second pull request for the post, merged after the publication,
  whenever it matters. A manual step per release, and it does not move
  `app_version`, which AGENTS.md §6 puts in the release pull request.

Rejected without being offered: gating *content* instead of the deploy (withhold
the post from the build, regenerate `/llms.txt`, override `app_version` in the
workspace). It keeps unrelated site changes flowing during a release, and pays for
that with a build that no longer equals the repository — three derived artifacts to
keep consistent instead of one question asked once.

> **User's decision (2026-09-20): (a).**

**F2 — an override.** A release that stalls after its pull request merged (a failed
smoke test, a fix on the way) freezes the site for as long as it takes, including
site changes that have nothing to do with it.

- **(a) `workflow_dispatch` gains a boolean `force`**, default `false`: a forced run
  asks nothing and deploys `main` as it is. **Recommended.**
- (b) No override: publish, or revert the version bump.

> **User's decision (2026-09-20): (a).**

**Decided without a fork, and why.** The gate's key is `Cargo.toml`'s version, not
`app_version`: it is the version's source of truth, `release_guard.py` already
refuses a tag that disagrees with it, and a release pull request that forgot
`app_version` would otherwise open the gate for its own post. A `prerelease`
under the exact tag holds, like a draft: the site does not announce one. The
`workflow_run` trigger is **not** narrowed to runs the `release` event started — a
dry-run dispatch of `crates.io` therefore redeploys the site too, which costs
twenty seconds and buys the rehearsal in §6.

## 5. What it looks like now

```
release PR merges ──push──▶ Site: gate = hold (green, deploy skipped, a notice)
owner tags, smoke-tests, publishes
        └─release: published─▶ crates.io ──completed──▶ Site: gate = deploy
```

- `tools/site_release_gate.py` — the question, its three answers, `--force`, and a
  `--self-test` that drives every arm against fixtures; run by `ci.yml`'s lint job.
- `site.yml` — a `gate` job whose output the `deploy` job needs; a held deploy is a
  **skipped** job in a **green** run, annotated with what releases it. An answer
  the gate cannot read is a **red** run: a deploy on a guess is the thing this
  exists to prevent, and a rerun costs a minute.
- Between releases nothing changes: `Cargo.toml` names the latest published
  release, the gate says deploy, and a site change goes out as it always has —
  one job and about ten seconds later.

**What it costs.** Any site change merged between the release pull request and the
publication waits for the publication. If `crates.io` is ever renamed, the release
stops releasing the site, silently — hence the line in AGENTS.md §6, and the plain
`workflow_dispatch`, which needs no `force` once the release is public.

## 6. What can only be verified after the merge

`workflow_run` fires only from the workflow file on the default branch, so two
things cannot be shown on the pull request:

1. that a `workflow_run` run's OIDC subject is `refs/heads/main` and the role
   accepts it — documented, not yet measured here;
2. that the chain fires at all.

Both are rehearsable without a release: dispatch `crates.io` with its default
`dry_run: true`. It uploads nothing; when it completes, `Site` must start by
itself, the gate must say *deploy* (0.10.2 is published), and the deploy job must
assume the role. If (1) fails, the failure is benign and loud — a red `Site` run
after a release, with the manual dispatch as the way out — and the fix is then
fork F1 (b). The hold itself is first seen for real on the next release pull
request's merge; its arm is the one §3 measured against the live API.

**Rehearsed 2026-09-20 — GO.** `crates.io` dispatched with `dry_run: true` completed
at 18:32:43Z and `Site` started by itself at 18:32:44Z (`event: workflow_run`, branch
`main`); the gate said *deploy*, the deploy job logged `Authenticated as assumedRoleId
…:GitHubActions`, and the site was deployed thirty seconds after the trigger. So (1)
and (2) both hold. One thing the procedure above did not foresee: between releases
the dry run itself is **red** — it stops at "This version is not on crates.io yet",
because the version is — and the chain fired regardless, since `types: [completed]`
does not look at the conclusion. That is the wanted behaviour (the gate asks about
the release, not the crate, so a failed upload does not strand the post) and it is
why a red upstream run is not a failed rehearsal. Full record —
[docs/journal/website.md](../journal/website.md), "the site waits for the release".

**The hold, seen for real — 0.11.0, 2026-09-21.** The release pull request's merge
ran `Site` green with the build and the deploy skipped and the notice "the deploy is
held"; the site stayed silent for the 2 h 45 min the draft was checked; and after the
publication at 21:44:52Z the chain ran unattended — `crates.io` to 21:47:44Z, `Site`
six seconds later, deployed by 21:48:29Z. **3 min 37 s after the release, against
25 min 28 s before it** — the number this document opened with.
