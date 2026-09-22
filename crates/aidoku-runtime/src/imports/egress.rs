//! Egress policy for the `net` host import.
//!
//! Source code is untrusted third-party WASM (ADR-0004). The sandbox gives it
//! no sockets, so the only way out is `net`, which makes this the single place
//! that decides where that code may reach.
//!
//! # The check has to happen after resolution, and bind to the result
//!
//! Checking the hostname is not enough: an attacker controls DNS for their own
//! domain, so `sources.example` can simply resolve to `169.254.169.254`. So the
//! policy runs against **resolved addresses**.
//!
//! That alone still leaves a gap. Resolving, checking, and then connecting *by
//! hostname* re-resolves, and the second answer need not match the first — the
//! classic DNS-rebinding time-of-check/time-of-use window. [`vet`] therefore
//! returns the addresses it approved, and the caller must connect to **those**,
//! never to the hostname again.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Why an address or URL was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    /// Only http and https can be reached.
    Scheme(String),
    /// The URL had no host to resolve.
    NoHost,
    /// The name did not resolve, or resolved to nothing usable.
    Unresolved,
    /// Every resolved address was refused.
    Blocked { host: String, reason: Blocked },
}

/// The category an address fell into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    Loopback,
    Private,
    LinkLocal,
    /// 169.254.169.254 and friends. Called out separately because it is the
    /// one with credentials behind it.
    CloudMetadata,
    Unspecified,
    /// Shared address space and other reserved ranges.
    Reserved,
    Multicast,
}

/// The well-known cloud instance-metadata addresses.
const METADATA_V4: [Ipv4Addr; 2] = [
    Ipv4Addr::new(169, 254, 169, 254),
    // Alibaba and others use .253 for a second endpoint.
    Ipv4Addr::new(169, 254, 169, 253),
];

/// Classifies an address, or `None` if it is safe to reach.
pub fn classify(ip: IpAddr) -> Option<Blocked> {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        // An IPv4-mapped IPv6 address carries a v4 address inside it, and
        // checking only the v6 predicates would wave `::ffff:127.0.0.1`
        // straight through.
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => classify_v4(v4),
            None => classify_v6(v6),
        },
    }
}

fn classify_v4(ip: Ipv4Addr) -> Option<Blocked> {
    if METADATA_V4.contains(&ip) {
        return Some(Blocked::CloudMetadata);
    }
    if ip.is_loopback() {
        return Some(Blocked::Loopback);
    }
    if ip.is_private() {
        return Some(Blocked::Private);
    }
    if ip.is_link_local() {
        return Some(Blocked::LinkLocal);
    }
    if ip.is_unspecified() {
        return Some(Blocked::Unspecified);
    }
    if ip.is_broadcast() || ip.is_multicast() {
        return Some(Blocked::Multicast);
    }
    let [a, b, ..] = ip.octets();
    // 100.64.0.0/10 carrier-grade NAT, and 192.0.0.0/24 IETF protocol
    // assignments, both of which can reach infrastructure.
    if (a == 100 && (64..128).contains(&b)) || (ip.octets()[..3] == [192, 0, 0]) {
        return Some(Blocked::Reserved);
    }
    None
}

fn classify_v6(ip: Ipv6Addr) -> Option<Blocked> {
    if ip.is_loopback() {
        return Some(Blocked::Loopback);
    }
    if ip.is_unspecified() {
        return Some(Blocked::Unspecified);
    }
    if ip.is_multicast() {
        return Some(Blocked::Multicast);
    }
    let seg = ip.segments()[0];
    // fe80::/10 link-local.
    if seg & 0xffc0 == 0xfe80 {
        return Some(Blocked::LinkLocal);
    }
    // fc00::/7 unique local — the v6 equivalent of RFC1918.
    if seg & 0xfe00 == 0xfc00 {
        return Some(Blocked::Private);
    }
    None
}

/// Resolves host addresses. Injectable so the policy can be tested without
/// touching real DNS.
pub trait Resolver {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>>;
}

/// The real resolver.
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        use std::net::ToSocketAddrs;
        Ok((host, port).to_socket_addrs()?.collect())
    }
}

/// Checks a URL and returns the addresses approved for it.
///
/// **Connect to the returned addresses, not to the URL's host.** Re-resolving
/// after this call reopens the rebinding window this function exists to close.
///
/// Refusal is all-or-nothing: if any resolved address is blocked, the whole
/// request is refused rather than falling back to the remaining ones. A name
/// that answers with both a public and a private address is far more likely to
/// be an attack than a misconfiguration.
pub fn vet(url: &url::Url, resolver: &dyn Resolver) -> Result<Vec<SocketAddr>, Denial> {
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(Denial::Scheme(other.to_string())),
    }
    let host = url.host_str().ok_or(Denial::NoHost)?;
    let port = url.port_or_known_default().unwrap_or(443);

    // A literal address in the URL never reaches the resolver, so classify it
    // directly.
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match classify(ip) {
            Some(reason) => Err(Denial::Blocked {
                host: host.to_string(),
                reason,
            }),
            None => Ok(vec![SocketAddr::new(ip, port)]),
        };
    }

    let addrs = resolver
        .resolve(host, port)
        .map_err(|_| Denial::Unresolved)?;
    if addrs.is_empty() {
        return Err(Denial::Unresolved);
    }
    for addr in &addrs {
        if let Some(reason) = classify(addr.ip()) {
            return Err(Denial::Blocked {
                host: host.to_string(),
                reason,
            });
        }
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Vec<IpAddr>);

    impl Resolver for Fake {
        fn resolve(&self, _host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
            Ok(self.0.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_loopback_private_and_link_local() {
        assert_eq!(classify(ip("127.0.0.1")), Some(Blocked::Loopback));
        assert_eq!(classify(ip("::1")), Some(Blocked::Loopback));
        assert_eq!(classify(ip("10.0.0.5")), Some(Blocked::Private));
        assert_eq!(classify(ip("172.16.3.4")), Some(Blocked::Private));
        assert_eq!(classify(ip("172.31.255.255")), Some(Blocked::Private));
        assert_eq!(classify(ip("192.168.1.1")), Some(Blocked::Private));
        assert_eq!(classify(ip("169.254.10.1")), Some(Blocked::LinkLocal));
        assert_eq!(classify(ip("fe80::1")), Some(Blocked::LinkLocal));
        assert_eq!(classify(ip("fd00::1")), Some(Blocked::Private));
        assert_eq!(classify(ip("0.0.0.0")), Some(Blocked::Unspecified));
        assert_eq!(classify(ip("100.64.0.1")), Some(Blocked::Reserved));
    }

    #[test]
    fn blocks_cloud_metadata_specifically() {
        assert_eq!(
            classify(ip("169.254.169.254")),
            Some(Blocked::CloudMetadata)
        );
        assert_eq!(
            classify(ip("169.254.169.253")),
            Some(Blocked::CloudMetadata)
        );
    }

    /// `::ffff:127.0.0.1` is a loopback address wearing a v6 costume. Checking
    /// only the v6 predicates lets it through.
    #[test]
    fn unwraps_ipv4_mapped_ipv6() {
        assert_eq!(classify(ip("::ffff:127.0.0.1")), Some(Blocked::Loopback));
        assert_eq!(
            classify(ip("::ffff:169.254.169.254")),
            Some(Blocked::CloudMetadata)
        );
        assert_eq!(classify(ip("::ffff:10.0.0.1")), Some(Blocked::Private));
    }

    #[test]
    fn allows_ordinary_public_addresses() {
        assert_eq!(classify(ip("93.184.216.34")), None);
        assert_eq!(classify(ip("2606:2800:220:1:248:1893:25c8:1946")), None);
        assert_eq!(classify(ip("172.32.0.1")), None, "just outside RFC1918");
        assert_eq!(classify(ip("172.15.255.255")), None, "just below RFC1918");
    }

    /// The attack the post-resolution check exists for: the attacker controls
    /// DNS for a name they own, so the name itself proves nothing.
    #[test]
    fn rejects_a_public_name_resolving_into_the_private_range() {
        let url = url::Url::parse("https://sources.example/index.json").unwrap();
        let resolver = Fake(vec![ip("169.254.169.254")]);
        assert_eq!(
            vet(&url, &resolver),
            Err(Denial::Blocked {
                host: "sources.example".into(),
                reason: Blocked::CloudMetadata,
            })
        );
    }

    /// A name answering with one good and one bad address is refused outright
    /// rather than falling back to the good one.
    #[test]
    fn one_bad_address_refuses_the_whole_name() {
        let url = url::Url::parse("https://sources.example/x").unwrap();
        let resolver = Fake(vec![ip("93.184.216.34"), ip("127.0.0.1")]);
        assert!(matches!(vet(&url, &resolver), Err(Denial::Blocked { .. })));
    }

    #[test]
    fn returns_the_vetted_addresses_so_the_caller_need_not_re_resolve() {
        let url = url::Url::parse("https://sources.example/x").unwrap();
        let resolver = Fake(vec![ip("93.184.216.34")]);
        let addrs = vet(&url, &resolver).expect("public address");
        assert_eq!(addrs, vec![SocketAddr::new(ip("93.184.216.34"), 443)]);
    }

    #[test]
    fn literal_addresses_in_the_url_are_classified_without_dns() {
        let resolver = Fake(vec![ip("93.184.216.34")]);
        let blocked = url::Url::parse("http://127.0.0.1:8080/admin").unwrap();
        assert!(matches!(
            vet(&blocked, &resolver),
            Err(Denial::Blocked { .. })
        ));
        let metadata = url::Url::parse("http://169.254.169.254/latest/meta-data/").unwrap();
        assert!(matches!(
            vet(&metadata, &resolver),
            Err(Denial::Blocked { .. })
        ));
    }

    #[test]
    fn only_http_and_https_are_reachable() {
        let resolver = Fake(vec![ip("93.184.216.34")]);
        for bad in ["file:///etc/passwd", "gopher://x/1", "ftp://x/y"] {
            let url = url::Url::parse(bad).unwrap();
            assert!(
                matches!(vet(&url, &resolver), Err(Denial::Scheme(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_name_that_does_not_resolve_is_refused() {
        let url = url::Url::parse("https://nowhere.example/x").unwrap();
        assert_eq!(vet(&url, &Fake(vec![])), Err(Denial::Unresolved));
    }
}
