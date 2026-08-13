# Research: an address policy for model-chosen URLs

**Status:** design complete; **user's decision, 2026-08-13: all forks go with the
recommendations** (F1 = `fetch_url` *and* `web_search`'s page fetches, F2 = the full local
range including IPv4-mapped forms, F3 = a guarded resolver plus an IP-literal check,
F4 = a `tools.web_allow_private` setting defaulting to off, F5 = a refusal that names
every closed route, F6 = `shared/net.rs`, F7 = `/image attach <url>` is untouched).
Implemented; §6 below records what the implementation found that the design did not.
Roadmap item:
"`fetch_url`: no address policy" (opened 2026-08-13 by the image-URL track, which
found the gap while looking for a guard to reuse).

Related: [image-url-attach.md](image-url-attach.md) §2 and §6 (where this was
found, and why the *user-typed* URL is deliberately unfiltered), spec §9.3
(`fetch_url`), spec §9.4 (`web_search`), spec §13.4 (privacy).

## 1. Problem

`fetch_url` fetches whatever URL the model puts in the tool call. It validates
the **scheme and nothing else**:

```rust
if !(url.starts_with("http://") || url.starts_with("https://")) {
    anyhow::bail!(ctx.loc.t("tool.fetch_url.err.url_scheme"));
}
```

and its client follows redirects with `reqwest`'s default policy. No address is
inspected, so `http://127.0.0.1:8000/`, `http://192.168.1.20:8001/` and
`http://169.254.169.254/latest/meta-data/` are all ordinary fetches whose bodies
come back to the model as page text.

The model does not have to be malicious for this to fire, and that is the point:
its URLs routinely come from a page it fetched a moment earlier, or from a search
result, or from a document the user attached. Any of those can carry a link.
The app already treats fetched content as untrusted for *instructions*; the same
content currently steers *where the app connects*.

What it reaches on a developer's machine is not hypothetical: this project's own
users run `llama-server` on `192.168.1.20:8000` with no authentication, the
embedding server next to it, and `mindfork`'s own data directory is served by
nothing — but plenty of dev tooling does listen (dashboards, admin panels,
databases with HTTP frontends). On a cloud VM the link-local metadata endpoint
hands out credentials to anyone who asks.

**This is not the same question as `/image attach <url>`**, which was
deliberately left unfiltered three days ago and should stay that way. There the
*user* types the address, in the same box as `/image attach <path>`, which reads
any file on the machine; here a model picks it, often from text it did not write.
The trust levels differ, so the answers differ.

## 2. What the code does today

- `features/tools/fetch.rs` — `FetchUrl` holds its own `reqwest::Client`
  (20 s timeout), scheme-checks the argument, then `GET`s it.
- `features/tools/web.rs` — `WebSearch::fetch_content` fetches **result pages**
  with a second client. The URLs come from the search provider's HTML via
  `extract_real_url`, so they are one step further from the model but still
  attacker-influenceable (a page has to be ranked, which is not free but is not a
  guarantee either).
- `features/tools/youtube.rs` builds fixed `youtube.com` URLs around a validated
  video id — not a free-form address.
- The engine clients (`shared/api/*`) connect to URLs the **user** configured in
  settings, which on this project are LAN addresses by design. They are not in
  scope and must not be.
- `shared/mcp.rs` is stdio-only; there is no HTTP transport yet.

Two implementation facts that shape the design, both checked in the vendored
sources rather than assumed:

1. `reqwest::ClientBuilder::dns_resolver(R)` accepts a custom `Resolve`
   (`reqwest-0.13.4/src/dns/resolve.rs`), and the connector uses **exactly the
   addresses that resolver returns**.
2. `hyper-util`'s connector **never calls the resolver when the host is an IP
   literal** — it parses the host as an `IpAddr` first. So a resolver-based guard
   alone would let `http://127.0.0.1/` straight through; the literal case needs
   its own check.

## 3. Forks (need the user's decision before implementation)

### F1 — which callers are guarded?

| | A. `fetch_url` only | B. `fetch_url` + `web_search`'s page fetches | C. Everything outbound |
|---|---|---|---|
| Covers the direct route | yes | yes | yes |
| Covers a page reached via a search result | no | yes | yes |
| Breaks the user's own LAN engine | no | no | **yes** |
| Cost | — | one shared client | wrong by construction |

**Recommendation: B.** The two are the same threat one step apart, and once the
guard exists as a client both tools can be built from, covering the second costs
a line. C is not a stricter version of B, it is a bug: the engine and embedder
URLs are *user-configured*, and on this project they are LAN addresses.

### F2 — what is refused

**Recommendation:** loopback (`127.0.0.0/8`, `::1`), private
(`10/8`, `172.16/12`, `192.168/16`, `fc00::/7`), link-local (`169.254/16` — the
cloud metadata endpoint lives there — and `fe80::/10`), unspecified
(`0.0.0.0`, `::`), broadcast, multicast, and the shared-address/CGNAT range
`100.64/10`. Every check unwraps **IPv4-mapped IPv6** first (`::ffff:127.0.0.1`
is the oldest bypass in this family). Applied to **every** address a host
resolves to, not just the first: a hostname with an A record for a public address
and another for `127.0.0.1` must be refused, not raced.

### F3 — how the check binds to the connection *(the rebinding question)*

| | A. Custom `Resolve` that returns only approved addresses | B. Pre-resolve, then pin with `resolve_to_addrs` | C. Check the host, then connect normally |
|---|---|---|---|
| DNS rebinding window | **none** — the connection uses what the check returned | none, but a client per request | **wide open** — the second lookup is a different answer |
| IP literals | needs its own check (the resolver is skipped) | same | same |
| Cost | one resolver type, one client | a `Client` rebuilt per fetch | cheapest and wrong |

**Recommendation: A**, plus an explicit literal check on the URL host *and* on
every redirect hop (`redirect::Policy::custom`). A is the only option where the
address that was approved is the address that gets connected to, which is the
whole point of the exercise.

### F4 — an escape hatch, or not?

**Recommendation: a setting, `tools.web_allow_private`, default off**, in the
Tools group next to `web_enabled`. Someone whose model should read an internal
wiki has a real need, and the alternative is that they patch the binary or give
up the feature. Off by default because the person who benefits knows they want
it, while the person at risk does not know they are at risk. The description says
plainly what it opens up.

### F5 — what the model is told

**Recommendation:** a dedicated refusal (axis A, `ctx.loc`) naming the class of
address and closing the door: this tool does not reach local or private networks,
**no other tool does either**, and if that address was meant to be reachable the
*user* enables it in settings. Without the last two clauses the model will retry
the same host by IP, then by `localhost`, then via a redirector — the
most-repeated defect in this project's history ([lessons §4](../lessons.md)), and
here each retry is a wasted round trip on a request that must fail.

### F6 — where the code lives

**Recommendation:** `shared/net.rs` — the policy type, the resolver and a
`guarded_client()` builder, with `thiserror` per the layer convention.
`features/tools/{fetch,web}.rs` build their clients from it. `features` may
depend on `shared`, not the other way round, and nothing in `shared` needs to
know a tool exists.

### F7 — does this change `/image attach <url>`?

**Recommendation: no.** It stays unfiltered, and `tools.web_allow_private` does
not reach it — a setting about what *tools* may do must not silently change what
the user's own command does. Recorded here because the two now look similar and
a later reader will ask.

## 4. Test plan

- **Pure**: the classifier over a table of addresses — every refused range, its
  IPv4-mapped form, and the public addresses that must stay allowed. This is the
  part where a gate that always says "no" is indistinguishable from a correct one,
  so both directions are asserted.
- **Wired**: a `TcpListener` stub on `127.0.0.1` — with the guard on, the fetch
  is refused **and the stub never sees a connection** (the refusal must happen
  before the request, not after); with `allow_private`, the same fetch succeeds.
  That pair is what proves the switch is load-bearing rather than decorative.
- **Redirects**: a public-looking host that redirects to `127.0.0.1` is refused at
  the hop, not followed.
- **IP literals**: `http://127.0.0.1/`, `http://[::1]/`, `http://[::ffff:127.0.0.1]/`
  — the three that skip the resolver entirely.
- **Localization gate** for the new refusal, both locales, no unsubstituted
  placeholders.
- **Live** (`#[ignore]`, mandatory — this is a tool path): ask a real model to
  fetch a link-local address and assert it gets the refusal and **stops**, rather
  than trying four variants. The failure mode this feature must not create is a
  model that loops.

## 5. Scope

One PR: `shared/net.rs`, two call sites, one setting (+ its two locale strings
and the settings row), the refusal message, tests as above, one live smoke.
Roughly a day including the live run.

## 6. What the implementation added to the design

Two things were learned by building it, and both changed the shape of the code:

- **A guarded resolver is not a guard.** The design already said `hyper` skips the resolver
  for IP literals, but the first version still shipped the check as a separate function the
  *call site* had to remember. The wire test then connected to `127.0.0.1` and got a `200`
  through a fully "guarded" client. The fix is structural: `GuardedClient` owns both halves
  and hands out `get`/`post`, so a call site cannot reach the unchecked path — the invariant
  is in the type rather than in a habit. The one deliberate exception,
  `unchecked_inner()`, is for URLs the code itself writes (the YouTube metadata endpoints)
  and says so.
- **The smoke's target was a confound.** Pointed at `169.254.169.254`, a real model refused
  *on its own* — it never called the tool, so the guard was never exercised. Only the
  "the model never called the tool, so nothing was tested" assertion caught it. Retargeted
  at a loopback stub the user might plausibly ask about, the smoke measures the guard
  instead of the model's own reflexes — and the stub's connection counter proves end to end
  that nothing reached the service.
