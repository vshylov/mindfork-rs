//! Address policy for URLs the **model** chooses (spec §9.3,
//! docs/research/fetch-url-address-policy.md).
//!
//! `fetch_url` fetches whatever address the model puts in a tool call, and that address
//! routinely comes from a page the model fetched a moment earlier, a search result, or a
//! document the user attached. The app already treats such content as untrusted for
//! *instructions*; without this module it steers *where the app connects* — at a loopback
//! port, a LAN service, or the cloud metadata endpoint that hands out credentials.
//!
//! Three things have to be true at once, and each one is a separate mechanism here:
//!
//! 1. **The address that was approved is the address that gets connected to.** The check
//!    lives inside a [`reqwest::dns::Resolve`] implementation, so the connector uses
//!    exactly the addresses this module returned. A "resolve, check, then connect" shape
//!    would leave a window for the second lookup to answer differently (DNS rebinding).
//! 2. **An IP literal is checked separately.** `hyper`'s connector parses the host as an
//!    address first and **never calls the resolver** when it succeeds, so
//!    `http://127.0.0.1/` would sail past a resolver-only guard. [`check_url`] covers the
//!    URL the tool is given, and the redirect policy covers every hop.
//! 3. **Every address a host resolves to is checked, not the first.** A name with one
//!    public record and one loopback record must be refused rather than raced.
//!
//! This is deliberately **not** applied to URLs the *user* types (`/image attach <url>`,
//! the engine and embedder addresses in settings): there the address is chosen by the
//! person the guard would be protecting, and on this project those are LAN addresses by
//! design. See docs/research/image-url-attach.md §6.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// How many redirects a guarded client follows. `reqwest`'s own default is 10; the policy
/// is re-implemented here to add the per-hop address check, so the limit is restated.
const MAX_REDIRECTS: usize = 10;

/// The canned reply of the tests' stub server. A single literal on one line: this file has
/// CRLF endings, where a `\`-continuation inside a byte string swallowed the rest of the
/// head into the body.
#[cfg(test)]
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi";

/// Which addresses a model-driven fetch may reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressPolicy {
    /// Publicly routable addresses only — the default for every model-chosen URL.
    PublicOnly,
    /// Anything the OS will connect to. Reached by the `tools.web_allow_private` setting
    /// (someone whose model should read an internal wiki), and by this module's own tests,
    /// which serve from `127.0.0.1`.
    Unrestricted,
}

impl AddressPolicy {
    /// The policy for a `tools.web_allow_private` value.
    pub fn from_allow_private(allow_private: bool) -> Self {
        if allow_private {
            Self::Unrestricted
        } else {
            Self::PublicOnly
        }
    }

    pub fn allows(self, ip: IpAddr) -> bool {
        self == Self::Unrestricted || is_public(ip)
    }
}

/// A refusal by this module. Carried through `reqwest`'s error chain, where
/// [`was_blocked`] finds it again so the caller can say *why* rather than "request
/// failed".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    /// The address is local, private, or otherwise not publicly routable.
    #[error("address {0} is not publicly routable")]
    Blocked(IpAddr),
    /// The host has no addresses at all.
    #[error("host {0} did not resolve")]
    Unresolvable(String),
    /// More hops than [`MAX_REDIRECTS`].
    #[error("too many redirects")]
    TooManyRedirects,
}

/// Whether an address is publicly routable.
///
/// Written out rather than taken from `std`, whose `is_global` is still unstable. An
/// IPv4-mapped IPv6 address is unwrapped first: `::ffff:127.0.0.1` is the oldest bypass in
/// this family, and it is one `to_ipv4_mapped` away from being caught by the v4 rules.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

/// The same ranges [`is_public`] refuses, written as CIDR blocks, for a consumer that
/// needs a **list** rather than a predicate — the Python sandbox, whose network is
/// filtered by `wasmer`'s own rules (docs/research/safe-defaults.md D4). The predicate
/// stays the authority: `blocked_ranges_match_the_predicate` compares the two on every
/// boundary address, so a range added to one and forgotten in the other fails the build.
pub const BLOCKED_V4: &[(&str, u8)] = &[
    ("0.0.0.0", 8),       // "this network"
    ("10.0.0.0", 8),      // private
    ("100.64.0.0", 10),   // carrier-grade NAT
    ("127.0.0.0", 8),     // loopback
    ("169.254.0.0", 16),  // link-local, where the cloud metadata endpoint lives
    ("172.16.0.0", 12),   // private
    ("192.0.2.0", 24),    // documentation
    ("192.168.0.0", 16),  // private
    ("198.18.0.0", 15),   // benchmarking
    ("198.51.100.0", 24), // documentation
    ("203.0.113.0", 24),  // documentation
    ("224.0.0.0", 4),     // multicast
    ("240.0.0.0", 4),     // reserved, and the broadcast address with it
];

/// The IPv6 half of [`BLOCKED_V4`]. `::/96` and `::ffff:0:0/96` cover both spellings of
/// an IPv4 address inside IPv6, which [`is_public`] unwraps rather than trusts.
pub const BLOCKED_V6: &[(&str, u8)] = &[
    ("::", 96),         // unspecified, loopback, and the IPv4-compatible form
    ("::ffff:0:0", 96), // IPv4-mapped
    ("2001:db8::", 32), // documentation
    ("fc00::", 7),      // unique local
    ("fe80::", 10),     // link-local
    ("ff00::", 8),      // multicast
];

/// `wasmer`'s network filter for the Python sandbox: everything allowed, then every
/// range above denied — measured semantics (docs/research/safe-defaults.md §2.3): a deny
/// beats an allow whatever the order, and **any** rule list turns the default to deny, so
/// the three allows have to be stated or nothing resolves or connects at all.
pub fn sandbox_net_rules() -> String {
    let mut rules = vec!["ipv4:allow=*:*".to_string(), "ipv6:allow=*:*".to_string()];
    rules.extend(
        BLOCKED_V4
            .iter()
            .map(|(net, bits)| format!("ipv4:deny={net}/{bits}:*")),
    );
    rules.extend(
        BLOCKED_V6
            .iter()
            .map(|(net, bits)| format!("ipv6:deny={net}/{bits}:*")),
    );
    // Names still resolve: the rules are applied to the address at connect time, so a
    // name that resolves into a denied range is refused there rather than here.
    rules.push("dns:allow=*:*".to_string());
    rules.join(",")
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        // 169.254.0.0/16 — where the cloud metadata endpoint lives.
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        // 0.0.0.0/8 "this network": 0.0.0.0 is only its first address.
        || a == 0
        // 100.64.0.0/10 carrier-grade NAT — the ISP's side of the connection.
        || (a == 100 && (64..128).contains(&b))
        // 198.18.0.0/15 benchmarking.
        || (a == 198 && (18..20).contains(&b))
        // 240.0.0.0/4 reserved, which also covers the broadcast address.
        || a >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        // fc00::/7 unique local.
        || (seg[0] & 0xfe00) == 0xfc00
        // fe80::/10 link-local unicast.
        || (seg[0] & 0xffc0) == 0xfe80
        // 2001:db8::/32 documentation.
        || (seg[0] == 0x2001 && seg[1] == 0x0db8)
        // ::a.b.c.d — the deprecated IPv4-compatible form. Nothing legitimate uses it, and
        // it is another spelling of an address the v4 rules would judge.
        || ip.to_ipv4().is_some())
}

/// Checks the address in a URL **before** any request is made.
///
/// Only an IP literal can be decided here; a hostname is left to [`GuardedResolver`],
/// which is the only place that can bind the check to the connection. A URL that cannot be
/// parsed is not this function's problem — the caller has already checked the scheme.
pub fn check_url(url: &str, policy: AddressPolicy) -> Result<(), NetError> {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return Ok(());
    };
    match literal_ip(&parsed) {
        Some(ip) if !policy.allows(ip) => Err(NetError::Blocked(ip)),
        _ => Ok(()),
    }
}

/// The address in a URL when the host **is** one, rather than a name to resolve.
/// `host_str` keeps the brackets around an IPv6 literal, hence the trim.
fn literal_ip(url: &reqwest::Url) -> Option<IpAddr> {
    let host = url.host_str()?;
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse().ok()
}

/// Whether a `reqwest` failure was this module refusing an address, rather than the network
/// misbehaving. The refusal travels as the source of a transport error, so the chain is
/// walked rather than the top-level error inspected — and a test pins that it is still
/// findable, because a downcast that quietly stops matching would turn every refusal back
/// into "request failed".
pub fn was_blocked(err: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        if e.downcast_ref::<NetError>().is_some() {
            return true;
        }
        source = e.source();
    }
    false
}

/// A DNS resolver that only ever answers with addresses `policy` allows.
#[derive(Debug)]
struct GuardedResolver {
    policy: AddressPolicy,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let policy = self.policy;
        let host = name.as_str().to_string();
        Box::pin(async move {
            // Port 0: reqwest replaces it with the scheme's port (or the URL's own).
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .collect();
            if addrs.is_empty() {
                return Err(NetError::Unresolvable(host).into());
            }
            // *Any*, not *all*: a name that answers with one public and one loopback
            // address is refused rather than raced, because which one gets connected to is
            // not ours to decide.
            if let Some(bad) = addrs.iter().find(|a| !policy.allows(a.ip())) {
                return Err(NetError::Blocked(bad.ip()).into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The redirect policy: `reqwest`'s hop limit plus the check the resolver cannot make.
fn redirect_policy(policy: AddressPolicy) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        // A hop to an IP literal never reaches the resolver (hyper parses the host as an
        // address first), which is exactly how an open redirector would be used to land on
        // a loopback port.
        if let Some(ip) = literal_ip(attempt.url())
            && !policy.allows(ip)
        {
            return attempt.error(NetError::Blocked(ip));
        }
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error(NetError::TooManyRedirects);
        }
        attempt.follow()
    })
}

/// A client that will not connect anywhere `policy` refuses.
///
/// It exists as a **type** rather than a configured `reqwest::Client` because the resolver
/// cannot be the whole guard: `hyper` parses an IP-literal host itself and never calls the
/// resolver, so `http://127.0.0.1/` goes straight through a client that only has a guarded
/// resolver — measured, not assumed (the wire test below connected and got a `200` before
/// this type existed). Wrapping the client is what makes the literal check impossible to
/// forget: a caller cannot reach `get`/`post` without passing through it.
pub struct GuardedClient {
    inner: reqwest::Client,
    policy: AddressPolicy,
}

impl GuardedClient {
    pub fn new(policy: AddressPolicy, timeout: Duration) -> Self {
        Self {
            inner: guarded_client(policy, timeout),
            policy,
        }
    }

    /// A `GET`, refused up front when the URL names a blocked address literally.
    pub fn get(&self, url: &str) -> Result<reqwest::RequestBuilder, NetError> {
        check_url(url, self.policy)?;
        Ok(self.inner.get(url))
    }

    /// A `POST`, with the same check.
    pub fn post(&self, url: &str) -> Result<reqwest::RequestBuilder, NetError> {
        check_url(url, self.policy)?;
        Ok(self.inner.post(url))
    }

    /// The underlying client, for **fixed URLs the code itself writes** (the YouTube
    /// metadata endpoints). Anything coming from a model, a page or a search result goes
    /// through [`GuardedClient::get`]: the guarded resolver still applies here, but the
    /// IP-literal check does not.
    pub fn unchecked_inner(&self) -> &reqwest::Client {
        &self.inner
    }
}

/// The `reqwest::Client` behind [`GuardedClient`]. Private: on its own it is not a guard.
fn guarded_client(policy: AddressPolicy, timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .dns_resolver(Arc::new(GuardedResolver { policy }))
        .redirect(redirect_policy(policy))
        .build()
        // The builder only fails when the TLS backend cannot be initialised, which is what
        // `Client::new()` panics on — but falling back to an *unguarded* client would turn
        // a TLS problem into a policy hole, so the guard is rebuilt without the timeout
        // instead.
        .unwrap_or_else(|_| {
            reqwest::Client::builder()
                .dns_resolver(Arc::new(GuardedResolver { policy }))
                .redirect(redirect_policy(policy))
                .build()
                .expect("a client with no TLS options must build")
        })
}

/// The counting stub server these tests run against — and the live smoke too, so the
/// codebase has one of them rather than two.
#[cfg(test)]
pub(crate) mod stub {
    use std::sync::Arc;

    use super::RESPONSE;

    /// A stub that counts connections. The count is the assertion: a guard that refuses
    /// *after* connecting would still return an error and look identical from the caller's
    /// side, and on a loopback admin page the connection itself is the damage.
    pub(crate) fn counting_stub() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                use std::io::{Read, Write};
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(RESPONSE);
            }
        });
        (format!("http://{addr}/admin"), hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// The CIDR list and the predicate are two spellings of one policy, and the sandbox
    /// uses the list where `fetch_url` uses the predicate. Every range is checked at both
    /// edges and just outside them, in both directions: a range the list forgot shows up
    /// as an address the predicate refuses and the list allows, and one the list invented
    /// as the reverse.
    #[test]
    fn blocked_ranges_match_the_predicate() {
        fn v4_range(net: &str, bits: u8) -> (u32, u32) {
            let base = u32::from(net.parse::<Ipv4Addr>().unwrap());
            let size = 1u64 << (32 - bits);
            (base, (base as u64 | (size - 1)) as u32)
        }
        fn v6_range(net: &str, bits: u8) -> (u128, u128) {
            let base = u128::from(net.parse::<Ipv6Addr>().unwrap());
            let size = 1u128 << (128 - bits);
            (base, base | (size - 1))
        }
        let listed_v4 = |a: Ipv4Addr| {
            BLOCKED_V4.iter().any(|(net, bits)| {
                let (lo, hi) = v4_range(net, *bits);
                (lo..=hi).contains(&u32::from(a))
            })
        };
        let listed_v6 = |a: Ipv6Addr| {
            BLOCKED_V6.iter().any(|(net, bits)| {
                let (lo, hi) = v6_range(net, *bits);
                (lo..=hi).contains(&u128::from(a))
            })
        };

        for (net, bits) in BLOCKED_V4 {
            let (lo, hi) = v4_range(net, *bits);
            for probe in [lo, lo + 1, hi - 1, hi] {
                let a = Ipv4Addr::from(probe);
                assert!(!is_public(IpAddr::V4(a)), "{a} is listed but public");
            }
            for outside in [lo.checked_sub(1), hi.checked_add(1)] {
                let Some(a) = outside.map(Ipv4Addr::from) else {
                    continue;
                };
                assert_eq!(
                    listed_v4(a),
                    !is_public(IpAddr::V4(a)),
                    "{a}, just outside {net}/{bits}, is judged differently by the two"
                );
            }
        }
        for (net, bits) in BLOCKED_V6 {
            let (lo, hi) = v6_range(net, *bits);
            for probe in [lo, lo + 1, hi - 1, hi] {
                let a = Ipv6Addr::from(probe);
                assert!(!is_public(IpAddr::V6(a)), "{a} is listed but public");
            }
            for outside in [lo.checked_sub(1), hi.checked_add(1)] {
                let Some(a) = outside.map(Ipv6Addr::from) else {
                    continue;
                };
                assert_eq!(
                    listed_v6(a),
                    !is_public(IpAddr::V6(a)),
                    "{a}, just outside {net}/{bits}, is judged differently by the two"
                );
            }
        }
        // A handful of ordinary public addresses, so a list that blocked everything
        // would fail here rather than pass by agreeing with nothing.
        for a in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:2800:220:1::",
            "2a00:1450::1",
        ] {
            let a = ip(a);
            assert!(is_public(a), "{a}");
            match a {
                IpAddr::V4(v4) => assert!(!listed_v4(v4)),
                IpAddr::V6(v6) => assert!(!listed_v6(v6)),
            }
        }
    }

    /// The rule string wasmer is given: the three allows the measurement showed to be
    /// mandatory (a rule list is default-deny), and one deny per listed range.
    #[test]
    fn sandbox_rules_allow_then_deny_every_range() {
        let rules = sandbox_net_rules();
        for required in ["ipv4:allow=*:*", "ipv6:allow=*:*", "dns:allow=*:*"] {
            assert!(rules.contains(required), "{required} missing: {rules}");
        }
        assert!(rules.contains("ipv4:deny=127.0.0.0/8:*"), "{rules}");
        assert!(rules.contains("ipv6:deny=fe80::/10:*"), "{rules}");
        assert_eq!(
            rules.split(',').filter(|r| r.contains("deny")).count(),
            BLOCKED_V4.len() + BLOCKED_V6.len()
        );
    }

    /// Both directions in one table: a classifier that always says "not public" would pass
    /// the first half and is caught by the second (lessons §2 — a gate that passes is
    /// indistinguishable from a gate that does nothing).
    #[test]
    fn the_classifier_refuses_local_ranges_and_allows_the_public_internet() {
        for addr in [
            "127.0.0.1",
            "127.1.2.3",
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "172.16.5.4",
            "172.31.255.255",
            "192.168.1.20",
            "169.254.169.254", // the cloud metadata endpoint
            "255.255.255.255",
            "224.0.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip(addr)), "{addr} must be refused");
        }
        for addr in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",  // just outside 172.16/12
            "192.169.0.1", // just outside 192.168/16
            "100.128.0.1", // just outside the CGNAT range
            "198.20.0.1",  // just outside the benchmarking range
            "2606:4700::1111",
            "2001:db9::1", // just outside the documentation range
        ] {
            assert!(is_public(ip(addr)), "{addr} must be allowed");
        }
    }

    /// The oldest bypass in this family: the same loopback address spelled as IPv6.
    #[test]
    fn ipv4_addresses_wearing_an_ipv6_spelling_are_still_refused() {
        for addr in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1", // the deprecated IPv4-compatible form
        ] {
            assert!(!is_public(ip(addr)), "{addr} must be refused");
        }
        // ...and the mapped form of a public address still resolves as public, or the
        // unwrapping would be a blanket refusal wearing a disguise.
        assert!(is_public(ip("::ffff:8.8.8.8")));
    }

    #[test]
    fn the_permissive_policy_allows_what_the_default_refuses() {
        assert!(!AddressPolicy::PublicOnly.allows(ip("127.0.0.1")));
        assert!(AddressPolicy::Unrestricted.allows(ip("127.0.0.1")));
        assert!(AddressPolicy::from_allow_private(false).allows(ip("8.8.8.8")));
        assert_eq!(
            AddressPolicy::from_allow_private(true),
            AddressPolicy::Unrestricted
        );
    }

    use super::stub::counting_stub;

    /// Both directions on the wire, which is the pair that proves the switch is
    /// load-bearing: refused by default and *never connected to*, allowed when the user
    /// turns private addresses on.
    #[tokio::test]
    async fn a_loopback_service_is_unreachable_by_default_and_reachable_when_allowed() {
        use std::sync::atomic::Ordering;
        let (url, hits) = counting_stub();

        let guarded = GuardedClient::new(AddressPolicy::PublicOnly, Duration::from_secs(5));
        // Refused before a request even exists — the literal case, which the resolver
        // never sees, and which a bare guarded client let straight through.
        assert_eq!(
            guarded.get(&url).err(),
            Some(NetError::Blocked("127.0.0.1".parse().unwrap()))
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "the guard must refuse before the connection, not after"
        );

        let permissive = GuardedClient::new(AddressPolicy::Unrestricted, Duration::from_secs(5));
        let body = permissive
            .get(&url)
            .unwrap()
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "hi");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    /// The refusal has to survive `reqwest`'s error wrapping, or every blocked address
    /// would reach the model as a generic "request failed" and invite a retry.
    #[tokio::test]
    async fn an_ordinary_transport_failure_is_not_mistaken_for_a_refusal() {
        // Nothing is listening on this port, and the address is allowed — so the failure is
        // a real connection error and `was_blocked` must say no.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let permissive = GuardedClient::new(AddressPolicy::Unrestricted, Duration::from_secs(5));
        let err = permissive
            .get(&format!("http://{addr}/"))
            .unwrap()
            .send()
            .await
            .unwrap_err();
        assert!(
            !was_blocked(&err),
            "a dead port is not a policy refusal: {err}"
        );
    }

    /// A hostname that resolves to a local address is the case the resolver exists for —
    /// and `localhost` is the one every machine has.
    #[tokio::test]
    async fn a_hostname_resolving_to_loopback_is_refused_by_the_resolver() {
        let (url, hits) = counting_stub();
        let port = url
            .rsplit(':')
            .next()
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let guarded = GuardedClient::new(AddressPolicy::PublicOnly, Duration::from_secs(5));
        let err = guarded
            .get(&format!("http://localhost:{port}/admin"))
            .expect("a name is not a literal, so this check passes and the resolver decides")
            .send()
            .await
            .unwrap_err();
        assert!(
            was_blocked(&err),
            "resolved to loopback, must be refused: {err}"
        );
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a name that resolves locally must not be connected to either"
        );
    }

    /// An IP literal in the URL is refused before any request — the case the resolver never
    /// sees, because hyper parses the host as an address first.
    #[test]
    fn a_literal_address_in_the_url_is_judged_without_a_request() {
        assert_eq!(
            check_url("http://127.0.0.1:8000/admin", AddressPolicy::PublicOnly),
            Err(NetError::Blocked(ip("127.0.0.1")))
        );
        assert_eq!(
            check_url("http://[::1]:8000/", AddressPolicy::PublicOnly),
            Err(NetError::Blocked(ip("::1")))
        );
        assert_eq!(
            check_url("http://[::ffff:127.0.0.1]/", AddressPolicy::PublicOnly),
            Err(NetError::Blocked(ip("::ffff:127.0.0.1")))
        );
        assert_eq!(
            check_url(
                "http://169.254.169.254/latest/meta-data/",
                AddressPolicy::PublicOnly
            ),
            Err(NetError::Blocked(ip("169.254.169.254")))
        );
        // A hostname is not decided here — that is the resolver's job, and deciding it
        // twice is how a rebinding window opens.
        assert_eq!(
            check_url("https://example.com/a", AddressPolicy::PublicOnly),
            Ok(())
        );
        assert_eq!(
            check_url("http://127.0.0.1/", AddressPolicy::Unrestricted),
            Ok(())
        );
    }
}
