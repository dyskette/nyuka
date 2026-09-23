//! Resolving the client's address behind a reverse proxy.
//!
//! # `X-Forwarded-For` is only as trustworthy as who set it
//!
//! Any client can send `X-Forwarded-For`. Believing it unconditionally lets
//! anyone claim any address, which defeats every per-IP control at once: rate
//! limiting, logging, and abuse blocking all start attributing traffic to
//! whatever the attacker typed.
//!
//! So the header is read only when the *peer* — the socket this connection
//! actually came from — is in `TRUSTED_PROXIES`. With no trusted proxies
//! configured, the peer address is the answer and the header is ignored.
//!
//! The header is read **right to left**, taking the last address that is not
//! itself a trusted proxy. A left-to-right read takes the first entry, which
//! is the one a client fully controls: `X-Forwarded-For: 1.2.3.4` arriving at
//! a proxy becomes `1.2.3.4, <real client>`, and reading left to right hands
//! back the forgery.
//!
//! > [!NOTE]
//! > Identity is never taken from a proxy header. This governs the client
//! > address only — who someone *is* comes from the session (ADR-0005).

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

use crate::config::Cidr;

pub const X_FORWARDED_FOR: &str = "x-forwarded-for";

/// The client's address, given the socket peer and the request headers.
pub fn client_ip(peer: SocketAddr, headers: &HeaderMap, trusted: &[Cidr]) -> IpAddr {
    let peer = peer.ip();

    // Nothing is trusted, so nothing is believed.
    if trusted.is_empty() || !is_trusted(peer, trusted) {
        return peer;
    }

    let Some(forwarded) = headers.get(X_FORWARDED_FOR).and_then(|v| v.to_str().ok()) else {
        return peer;
    };

    forwarded
        .split(',')
        .map(str::trim)
        .filter_map(|entry| entry.parse::<IpAddr>().ok())
        // Right to left: the rightmost entry was written by the proxy nearest
        // to this server and is the only one a client could not forge.
        .rev()
        .find(|addr| !is_trusted(*addr, trusted))
        // Every hop is a trusted proxy, so the peer is as close to the truth
        // as this can get.
        .unwrap_or(peer)
}

fn is_trusted(addr: IpAddr, trusted: &[Cidr]) -> bool {
    trusted.iter().any(|net| net.contains(addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(forwarded: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(
            X_FORWARDED_FOR,
            HeaderValue::from_str(forwarded).expect("header"),
        );
        map
    }

    fn peer(addr: &str) -> SocketAddr {
        SocketAddr::new(addr.parse().expect("ip"), 40000)
    }

    fn ip(addr: &str) -> IpAddr {
        addr.parse().expect("ip")
    }

    fn trusted() -> Vec<Cidr> {
        vec![Cidr::parse("10.0.0.0/8").expect("cidr")]
    }

    /// The control this module exists for: a header from an untrusted peer is
    /// a claim, not a fact.
    #[test]
    fn a_forwarded_header_from_an_untrusted_peer_is_ignored() {
        assert_eq!(
            client_ip(peer("203.0.113.9"), &headers("1.2.3.4"), &trusted()),
            ip("203.0.113.9"),
            "anyone can send this header; believing it lets them be any address"
        );
    }

    #[test]
    fn a_forwarded_header_from_a_trusted_proxy_is_used() {
        assert_eq!(
            client_ip(peer("10.0.0.5"), &headers("203.0.113.9"), &trusted()),
            ip("203.0.113.9")
        );
    }

    /// The forgery this ordering defends against. A client that sends
    /// `X-Forwarded-For: 1.2.3.4` has it *prepended* to, not replaced by, the
    /// real address — so the leftmost entry is the one they chose.
    #[test]
    fn the_rightmost_untrusted_hop_wins_not_the_leftmost() {
        assert_eq!(
            client_ip(
                peer("10.0.0.5"),
                &headers("1.2.3.4, 203.0.113.9"),
                &trusted()
            ),
            ip("203.0.113.9"),
            "reading left to right would hand back the client's own forgery"
        );
    }

    #[test]
    fn trusted_hops_are_skipped_over() {
        assert_eq!(
            client_ip(
                peer("10.0.0.5"),
                &headers("203.0.113.9, 10.0.0.9, 10.0.0.5"),
                &trusted()
            ),
            ip("203.0.113.9"),
            "a chain of proxies must resolve past all of them"
        );
    }

    #[test]
    fn a_chain_of_only_trusted_hops_falls_back_to_the_peer() {
        assert_eq!(
            client_ip(peer("10.0.0.5"), &headers("10.0.0.9"), &trusted()),
            ip("10.0.0.5")
        );
    }

    /// The default deployment: no proxy configured, so no header is believed.
    #[test]
    fn with_no_trusted_proxies_the_peer_is_the_answer() {
        assert_eq!(
            client_ip(peer("10.0.0.5"), &headers("1.2.3.4"), &[]),
            ip("10.0.0.5")
        );
    }

    #[test]
    fn a_missing_or_malformed_header_falls_back_to_the_peer() {
        assert_eq!(
            client_ip(peer("10.0.0.5"), &HeaderMap::new(), &trusted()),
            ip("10.0.0.5")
        );
        assert_eq!(
            client_ip(peer("10.0.0.5"), &headers("not-an-address"), &trusted()),
            ip("10.0.0.5")
        );
    }

    #[test]
    fn a_malformed_entry_among_good_ones_is_skipped() {
        assert_eq!(
            client_ip(
                peer("10.0.0.5"),
                &headers("203.0.113.9, garbage"),
                &trusted()
            ),
            ip("203.0.113.9")
        );
    }

    #[test]
    fn ipv6_proxies_and_clients_work() {
        let trusted = vec![Cidr::parse("2001:db8::/32").expect("cidr")];
        assert_eq!(
            client_ip(peer("2001:db8::1"), &headers("2001:db9::1234"), &trusted),
            ip("2001:db9::1234")
        );
    }
}
