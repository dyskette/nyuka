//! The OIDC client and the session it issues (ADR-0005).
//!
//! The API is a confidential OIDC client running authorization-code with PKCE.
//! Tokens from the provider never reach the browser; the browser gets an
//! opaque session cookie. That is RFC 10017's backend-for-frontend pattern,
//! and it is what makes `EventSource`, anchor downloads, and `<img src>`
//! authenticate with no special handling — none of the three can set a header.
//!
//! # The flow's secrets live in the session, not in the URL
//!
//! `state`, `nonce`, and the PKCE verifier are written to the session before
//! the redirect and read back at the callback. Keeping them server-side is
//! what makes the checks mean anything: a `state` the client could rewrite
//! proves nothing about who started the flow.
//!
//! They are removed as soon as they are used, so a replayed callback finds
//! nothing to validate against and is refused.
//!
//! # This module uses a second reqwest
//!
//! `openidconnect` 4.0.1 pulls `oauth2` 5, which is on **reqwest 0.12**, while
//! the rest of the workspace is on **0.13**. Both are in the tree; that is a
//! fact, not an oversight, and it has one consequence worth stating:
//! [`VettingResolver`] implements `reqwest::dns::Resolve` for 0.13, so the
//! egress allow-list cannot be applied to the provider calls here.
//!
//! That is acceptable for a different reason rather than by accident: the
//! issuer URL is operator configuration, not attacker-influenced input, and
//! anyone who can set it already controls the process. The source runtime's
//! URLs come from untrusted packages and keep their vetting.
//!
//! [`VettingResolver`]: nyuka_aidoku_runtime::imports::net::VettingResolver

mod routes;

pub use routes::{callback, login, logout, me};

use openidconnect::core::{
    CoreAuthDisplay, CoreAuthPrompt, CoreErrorResponseType, CoreGenderClaim, CoreJsonWebKey,
    CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm, CoreProviderMetadata,
    CoreRevocableToken, CoreRevocationErrorResponse, CoreTokenIntrospectionResponse, CoreTokenType,
};
use openidconnect::{
    AdditionalClaims, Client, ClientId, ClientSecret, EmptyExtraTokenFields, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IdTokenFields, IssuerUrl, RedirectUrl, StandardErrorResponse,
    StandardTokenResponse,
};
use serde::{Deserialize, Serialize};

/// The non-standard claim the allow-list reads.
///
/// `groups` is not a registered OIDC claim, but Authelia and Keycloak both
/// emit it under that name and there is no registered alternative to fall back
/// to. A provider that names it differently needs its own mapping rather than
/// a guess here — which is why a missing claim is an empty list and not an
/// error: group membership simply does not decide anything for that provider,
/// and the subject allow-list still does.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct GroupClaims {
    #[serde(default)]
    pub groups: Vec<String>,
}

impl AdditionalClaims for GroupClaims {}

/// `CoreIdTokenFields`, but carrying [`GroupClaims`].
pub type NyukaIdTokenFields = IdTokenFields<
    GroupClaims,
    EmptyExtraTokenFields,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

pub type NyukaTokenResponse = StandardTokenResponse<NyukaIdTokenFields, CoreTokenType>;

/// The client, typed with [`GroupClaims`] rather than `EmptyAdditionalClaims`.
///
/// `CoreClient` hardcodes empty additional claims, so reading `groups` means
/// spelling the alias out. The endpoint-state parameters are what
/// `from_provider_metadata(...).set_redirect_uri(...)` leaves behind: auth and
/// token URLs set, the optional ones not.
pub type NyukaClient = Client<
    GroupClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    NyukaTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

use crate::config::AuthConfig;

/// Keys under which the in-flight flow's secrets are stored in the session.
pub const STATE_KEY: &str = "oidc_state";
pub const NONCE_KEY: &str = "oidc_nonce";
pub const PKCE_VERIFIER_KEY: &str = "oidc_pkce_verifier";
/// Where to send the browser after a successful sign-in.
pub const RETURN_TO_KEY: &str = "return_to";

/// The discovered provider and the client built from it.
pub struct OidcClient {
    pub(crate) client: NyukaClient,
    /// `openidconnect` rides `oauth2` 5, which is on reqwest **0.12** while
    /// the workspace is on 0.13. Using the re-export rather than naming a
    /// version keeps the two from being confused for each other.
    pub(crate) http: openidconnect::reqwest::Client,
    issuer: String,
}

impl std::fmt::Debug for OidcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The client holds the client secret.
        f.debug_struct("OidcClient")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

impl OidcClient {
    /// Discovers the provider and builds the client.
    ///
    /// Run at startup so an unreachable or misconfigured issuer fails the boot
    /// rather than the first sign-in — ADR-0005 follow-up 5.
    pub async fn discover(config: &AuthConfig) -> anyhow::Result<Self> {
        // Redirects are not followed. The discovery URL is operator-supplied,
        // and a provider that redirects is a provider whose metadata came from
        // somewhere other than where the operator pointed.
        let http = openidconnect::reqwest::ClientBuilder::new()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        let issuer = IssuerUrl::new(config.issuer_url.clone())?;
        let metadata = CoreProviderMetadata::discover_async(issuer.clone(), &http).await?;

        let client = NyukaClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id.clone()),
            // `client_secret_basic` / `client_secret_post`. Not
            // `private_key_jwt`: that signs with an RSA private key and would
            // walk straight into RUSTSEC-2023-0071, which this project's
            // advisory exception is written on the basis of avoiding.
            Some(ClientSecret::new(config.client_secret.expose().to_string())),
        )
        .set_redirect_uri(RedirectUrl::new(config.redirect_url.clone())?);

        Ok(Self {
            client,
            http,
            issuer: config.issuer_url.clone(),
        })
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }
}

/// Whether a post-sign-in redirect target is safe to use.
///
/// Only a path on this origin. An absolute URL, a protocol-relative `//host`,
/// or anything with a scheme would turn the login endpoint into an open
/// redirect: a phishing link that genuinely starts at this server and ends
/// somewhere else, having passed through a real login.
///
/// Backslashes are rejected because some browsers normalise `\` to `/`, which
/// makes `/\evil.test` a protocol-relative URL after normalisation but not
/// before it.
pub fn is_safe_return_to(target: &str) -> bool {
    target.starts_with('/')
        && !target.starts_with("//")
        && !target.contains('\\')
        && !target.contains("://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_path_is_a_safe_return_target() {
        for target in ["/", "/library", "/manga/123?tab=chapters", "/a/b/c"] {
            assert!(is_safe_return_to(target), "{target} should be allowed");
        }
    }

    /// Each of these is an open redirect, which is a phishing primitive that
    /// launders a real login through this server.
    #[test]
    fn anything_that_could_leave_this_origin_is_refused() {
        for target in [
            "https://evil.test",
            "//evil.test",
            "//evil.test/path",
            "http://evil.test",
            "/\\evil.test",
            "\\\\evil.test",
            "javascript:alert(1)",
            "",
            "library",
        ] {
            assert!(
                !is_safe_return_to(target),
                "{target:?} must not be accepted as a return target"
            );
        }
    }

    #[test]
    fn the_session_keys_are_distinct() {
        let keys = [STATE_KEY, NONCE_KEY, PKCE_VERIFIER_KEY, RETURN_TO_KEY];
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(
            unique.len(),
            keys.len(),
            "two flow secrets sharing a key would silently overwrite each other"
        );
    }
}
