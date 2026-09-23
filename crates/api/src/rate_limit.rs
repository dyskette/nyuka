//! Rate limiting for the authentication endpoints (ADR-0005 follow-up 6).
//!
//! # Why not `SmartIpKeyExtractor`
//!
//! `tower_governor` ships one that reads `Forwarded` and `X-Forwarded-For`
//! unconditionally. Behind a proxy that is what you want; without one it means
//! anyone can send `X-Forwarded-For: <random>` on every request and never hit
//! a limit, because each forged value is a different bucket.
//!
//! A rate limiter that an attacker can opt out of is worse than none: it
//! reports a control that is not there. The extractor here reuses
//! [`crate::client_ip`], so the header is believed only when the peer is in
//! `TRUSTED_PROXIES` — and with no proxies configured, the socket address is
//! the key.
//!
//! # The callback is limited too
//!
//! ADR-0005 asks for this explicitly. Limiting only `/auth/login` leaves the
//! expensive half open: the callback performs a token exchange against the
//! identity provider, so an unlimited callback is a way to make this server
//! hammer someone else's.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::ConnectInfo;
use http::request::Request;
use tower_governor::errors::GovernorError;
use tower_governor::key_extractor::KeyExtractor;

use crate::client_ip::client_ip;
use crate::config::Cidr;

/// Keys requests by the client address, believing `X-Forwarded-For` only from
/// a trusted peer.
#[derive(Clone)]
pub struct TrustedIpKeyExtractor {
    trusted: Arc<Vec<Cidr>>,
}

impl TrustedIpKeyExtractor {
    pub fn new(trusted: Vec<Cidr>) -> Self {
        Self {
            trusted: Arc::new(trusted),
        }
    }
}

/// `name` and `key_name` exist on `KeyExtractor` only under
/// `tower_governor`'s `tracing` feature, which this build does not enable.
/// They are omitted rather than gated: a `#[cfg(feature = "tracing")]` here
/// would name *this* crate's features, where no such feature exists, so the
/// methods would be silently dead. If the feature is ever turned on, the
/// trait will require them and the build will say so.
impl KeyExtractor for TrustedIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, request: &Request<T>) -> Result<Self::Key, GovernorError> {
        let peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| *addr)
            // No peer address means the service was not built with
            // `into_make_service_with_connect_info`. Failing closed here would
            // reject every request; failing open would disable the limiter
            // silently. Neither is acceptable, so this is an error the caller
            // sees as a 500 and an operator sees in the log.
            .ok_or(GovernorError::UnableToExtractKey)?;

        Ok(client_ip(peer, request.headers(), &self.trusted))
    }
}

/// Requests per second allowed on `/auth/*`, and the burst above it.
///
/// Sign-in is a human action taken a few times a day, so the steady rate is
/// deliberately low. The burst covers the redirect-and-callback pair plus a
/// retry, which arrive together.
pub const AUTH_PER_SECOND: u64 = 2;
pub const AUTH_BURST: u32 = 10;

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn request_from(peer: &str, forwarded: Option<&str>) -> Request<()> {
        let mut request = Request::new(());
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            peer.parse().expect("ip"),
            40000,
        )));
        if let Some(value) = forwarded {
            request.headers_mut().insert(
                "x-forwarded-for",
                HeaderValue::from_str(value).expect("header"),
            );
        }
        request
    }

    fn ip(addr: &str) -> IpAddr {
        addr.parse().expect("ip")
    }

    /// The property that makes this worth writing rather than using the
    /// built-in extractor: a forged header must not create a new bucket.
    #[test]
    fn a_forged_forwarded_header_does_not_escape_the_limit() {
        let extractor = TrustedIpKeyExtractor::new(vec![]);

        let first = extractor
            .extract(&request_from("203.0.113.9", Some("1.1.1.1")))
            .expect("key");
        let second = extractor
            .extract(&request_from("203.0.113.9", Some("2.2.2.2")))
            .expect("key");

        assert_eq!(
            first, second,
            "two forged headers from one peer must share a bucket, or the \
             limiter reports a control that is not there"
        );
        assert_eq!(first, ip("203.0.113.9"));
    }

    /// And behind a real proxy the header is what distinguishes clients, or
    /// every user shares the proxy's bucket.
    #[test]
    fn behind_a_trusted_proxy_each_client_gets_its_own_bucket() {
        let extractor = TrustedIpKeyExtractor::new(vec![Cidr::parse("10.0.0.0/8").expect("cidr")]);

        let first = extractor
            .extract(&request_from("10.0.0.5", Some("203.0.113.1")))
            .expect("key");
        let second = extractor
            .extract(&request_from("10.0.0.5", Some("203.0.113.2")))
            .expect("key");

        assert_ne!(first, second);
        assert_eq!(first, ip("203.0.113.1"));
    }

    /// Failing open would disable the limiter with no sign; failing closed
    /// would reject everything. An explicit error is the only honest option.
    #[test]
    fn a_missing_peer_address_is_an_error_rather_than_a_default_key() {
        let extractor = TrustedIpKeyExtractor::new(vec![]);
        let request = Request::new(());
        assert!(matches!(
            extractor.extract(&request),
            Err(GovernorError::UnableToExtractKey)
        ));
    }

    #[test]
    fn the_burst_covers_a_redirect_callback_pair_and_a_retry() {
        const { assert!(AUTH_BURST >= 4) };
        const {
            assert!(
                AUTH_PER_SECOND <= 5,
                "signing in is a human action taken a few times a day"
            )
        };
    }
}
