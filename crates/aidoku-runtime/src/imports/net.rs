//! The `net` host import: the only route out of the sandbox.
//!
//! # The egress policy is the DNS resolver
//!
//! Rather than checking a URL and then handing it to an HTTP client, this
//! module installs [`VettingResolver`] as the client's resolver. That is
//! deliberate, and it closes two holes a pre-flight check leaves open:
//!
//! - **No time-of-check/time-of-use window.** The client connects to exactly
//!   the addresses the policy returned, because resolving *is* the check.
//! - **Redirects are covered.** A pre-flight check validates the first URL
//!   only; a `302` to `http://169.254.169.254/` is resolved like any other
//!   host and so goes through the same policy.
//!
//! See [`egress`](super::egress) for the policy itself, and ADR-0004.

use std::net::SocketAddr;
use std::sync::Arc;

use crate::error::net as err;
use crate::resource::{Request, Response};

use super::egress::{self, Resolver, SystemResolver};

/// The guest's `HttpMethod`, by declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Head,
    Delete,
}

impl Method {
    /// Maps the wire value. An unknown discriminant is refused rather than
    /// defaulting to GET, which would turn a mutation into a read.
    pub fn from_wire(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::Get,
            1 => Self::Post,
            2 => Self::Put,
            3 => Self::Head,
            4 => Self::Delete,
            _ => return None,
        })
    }

    pub fn as_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Head => reqwest::Method::HEAD,
            Self::Delete => reqwest::Method::DELETE,
        }
    }
}

/// The declared-budget type lives in `domain`.
///
/// It was duplicated here once, with `Duration` instead of seconds and its own
/// copy of `from_wire` and `stricter`. Two structs meaning the same thing in
/// two crates is a drift waiting to happen, and the persisted shape has to win
/// because it is the one a stored row has to keep matching.
pub use nyuka_domain::model::RateLimit;

/// A `reqwest` resolver that applies the egress policy.
pub struct VettingResolver {
    inner: Arc<dyn Resolver + Send + Sync>,
}

impl VettingResolver {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SystemResolver),
        }
    }

    pub fn with_resolver(inner: Arc<dyn Resolver + Send + Sync>) -> Self {
        Self { inner }
    }

    /// Resolves and filters. Returns an error when the name resolves only to
    /// addresses the policy refuses, so the connection never starts.
    pub fn vetted(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, egress::Denial> {
        let addrs = self
            .inner
            .resolve(host, port)
            .map_err(|_| egress::Denial::Unresolved)?;
        if addrs.is_empty() {
            return Err(egress::Denial::Unresolved);
        }
        // All-or-nothing, as in `egress::vet`: a name answering with both a
        // public and a private address is refused rather than partially used.
        for addr in &addrs {
            if let Some(reason) = egress::classify(addr.ip()) {
                return Err(egress::Denial::Blocked {
                    host: host.to_string(),
                    reason,
                });
            }
        }
        Ok(addrs)
    }
}

impl Default for VettingResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl reqwest::dns::Resolve for VettingResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        // Port 0: reqwest substitutes the scheme's conventional port, and the
        // policy is port-independent.
        let result = self.vetted(name.as_str(), 0);
        Box::pin(async move {
            match result {
                Ok(addrs) => {
                    let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
                    Ok(iter)
                }
                Err(denial) => {
                    Err(Box::new(DenialError(denial)) as Box<dyn std::error::Error + Send + Sync>)
                }
            }
        })
    }
}

/// Wraps a [`egress::Denial`] so it can travel as a resolver error.
#[derive(Debug)]
pub struct DenialError(pub egress::Denial);

impl std::fmt::Display for DenialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            egress::Denial::Scheme(s) => write!(f, "scheme not permitted: {s}"),
            egress::Denial::NoHost => write!(f, "url has no host"),
            egress::Denial::Unresolved => write!(f, "host did not resolve"),
            egress::Denial::Blocked { host, reason } => {
                write!(f, "egress to {host} blocked: {reason:?}")
            }
        }
    }
}

impl std::error::Error for DenialError {}

/// Builds the HTTP client sources reach the network through.
pub fn build_client(
    user_agent: &str,
    timeout: std::time::Duration,
    resolver: Arc<VettingResolver>,
) -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        // Redirects are resolved through the same policy, so a 302 into the
        // private range is refused like a direct request would be.
        .redirect(reqwest::redirect::Policy::limited(10))
        .dns_resolver(resolver)
        .cookie_store(true)
        .build()
}

/// Builds the async client the page fetcher uses.
///
/// Same policy as [`build_client`], same resolver type: a page image is an
/// ordinary outbound request and must not get a weaker check than the source's
/// own requests do. The blocking client exists only because the WASM host
/// imports are called from synchronous guest code.
pub fn build_async_client(
    user_agent: &str,
    timeout: std::time::Duration,
    resolver: Arc<VettingResolver>,
) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(10))
        .dns_resolver(resolver)
        .build()
}

/// Performs a request that the guest has finished building.
pub fn send(client: &reqwest::blocking::Client, request: &Request) -> Result<Response, i32> {
    let url = request.url.as_deref().ok_or(err::NOT_SENT)?;
    let method = Method::from_wire(request.method).ok_or(err::INVALID_METHOD)?;
    // Reject a malformed URL here rather than letting the client do it, so the
    // guest gets the specific code.
    let parsed = url::Url::parse(url).map_err(|_| err::INVALID_URL)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(err::INVALID_URL);
    }

    let mut builder = client.request(method.as_reqwest(), parsed.clone());
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    if let Some(body) = &request.body {
        builder = builder.body(body.clone());
    }

    let response = builder.send().map_err(|_| err::FAILED)?;
    let status = response.status().as_u16();
    let final_url = Some(response.url().to_string());
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let body = response.bytes().map_err(|_| err::FAILED)?.to_vec();

    Ok(Response {
        status,
        headers,
        body,
        final_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::time::Duration;

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
    fn method_mapping_refuses_unknown_discriminants() {
        assert_eq!(Method::from_wire(0), Some(Method::Get));
        assert_eq!(Method::from_wire(1), Some(Method::Post));
        assert_eq!(Method::from_wire(4), Some(Method::Delete));
        assert_eq!(
            Method::from_wire(99),
            None,
            "defaulting to GET would turn a mutation into a read"
        );
        assert_eq!(Method::from_wire(-1), None);
    }

    #[test]
    fn the_resolver_refuses_names_pointing_into_the_private_range() {
        let r = VettingResolver::with_resolver(Arc::new(Fake(vec![ip("169.254.169.254")])));
        assert!(matches!(
            r.vetted("sources.example", 443),
            Err(egress::Denial::Blocked { .. })
        ));
    }

    #[test]
    fn the_resolver_passes_public_addresses_through() {
        let r = VettingResolver::with_resolver(Arc::new(Fake(vec![ip("93.184.216.34")])));
        let addrs = r.vetted("sources.example", 443).expect("public");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip(), ip("93.184.216.34"));
    }

    #[test]
    fn one_private_answer_refuses_the_whole_name() {
        let r = VettingResolver::with_resolver(Arc::new(Fake(vec![
            ip("93.184.216.34"),
            ip("127.0.0.1"),
        ])));
        assert!(matches!(
            r.vetted("sources.example", 443),
            Err(egress::Denial::Blocked { .. })
        ));
    }

    /// The policy being correct is not the same as the policy being wired in.
    /// This drives a real client, so a future refactor that drops
    /// `.dns_resolver(...)` fails here rather than silently reopening egress.
    #[test]
    fn a_client_built_with_the_resolver_refuses_a_blocked_host() {
        let resolver = Arc::new(VettingResolver::with_resolver(Arc::new(Fake(vec![ip(
            "169.254.169.254",
        )]))));
        let client =
            build_client("nyuka-test", Duration::from_secs(5), resolver).expect("client builds");
        let request = Request {
            method: 0,
            url: Some("https://sources.example/index.json".into()),
            ..Default::default()
        };
        // The connection never starts: resolution is the check.
        assert_eq!(send(&client, &request), Err(err::FAILED));
    }

    #[test]
    fn a_request_without_a_url_is_not_sent() {
        let resolver = Arc::new(VettingResolver::with_resolver(Arc::new(Fake(vec![ip(
            "93.184.216.34",
        )]))));
        let client = build_client("nyuka-test", Duration::from_secs(5), resolver).expect("client");
        assert_eq!(send(&client, &Request::default()), Err(err::NOT_SENT));
    }

    #[test]
    fn a_non_http_scheme_is_refused_before_any_connection() {
        let resolver = Arc::new(VettingResolver::with_resolver(Arc::new(Fake(vec![ip(
            "93.184.216.34",
        )]))));
        let client = build_client("nyuka-test", Duration::from_secs(5), resolver).expect("client");
        let request = Request {
            method: 0,
            url: Some("file:///etc/passwd".into()),
            ..Default::default()
        };
        assert_eq!(send(&client, &request), Err(err::INVALID_URL));
    }

    #[test]
    fn a_name_with_no_answers_is_unresolved() {
        let r = VettingResolver::with_resolver(Arc::new(Fake(vec![])));
        assert_eq!(
            r.vetted("nowhere.example", 443),
            Err(egress::Denial::Unresolved)
        );
    }
}
