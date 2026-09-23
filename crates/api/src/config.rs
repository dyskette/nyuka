//! Configuration, loaded with `figment` and validated fail-fast at startup.
//!
//! See `.env.example` for the full set. Secrets arrive via environment or
//! Docker secrets and never appear in the image or in spans.
//!
//! # Every error at once
//!
//! [`RawConfig::validate`] collects every problem before returning rather than
//! failing on the first. Fixing one variable, restarting, and discovering the
//! next one is a bad enough loop with two mistakes; with six it is most of an
//! afternoon.
//!
//! # Secrets are not `Debug`
//!
//! [`Secret`] prints as `***`. `Config` is `Debug` because logging the
//! resolved configuration at startup is genuinely useful, and the only thing
//! standing between that and a client secret in the log stream is this type.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

/// A value that must not be printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the length either: that narrows a brute force.
        f.write_str("***")
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// How requests are authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// The OIDC flow. The default, and what a deployment anyone else can
    /// reach should use.
    Oidc,
    /// No authentication at all.
    ///
    /// For a barebones self-host: one person, one machine, no identity
    /// provider to stand up. Every request is treated as a single seeded
    /// local user, so nothing downstream needs a second code path.
    None,
}

impl AuthMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "oidc" => Some(Self::Oidc),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn is_disabled(self) -> bool {
        matches!(self, Self::None)
    }
}

/// Every problem found, so one restart surfaces all of them.
#[derive(Debug)]
pub struct ConfigError {
    pub problems: Vec<String>,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "configuration is not usable:")?;
        for problem in &self.problems {
            writeln!(f, "  - {problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

/// An IPv4 or IPv6 network, for the trusted-proxy list.
///
/// Hand-parsed rather than pulled from a crate: the whole requirement is
/// "contains this address", and a dependency for twenty lines is a dependency
/// to keep current forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(text: &str) -> Result<Self, String> {
        let (addr, prefix) = text
            .split_once('/')
            .ok_or_else(|| format!("`{text}` is missing a prefix length, such as /24"))?;
        let network: IpAddr = addr
            .parse()
            .map_err(|_| format!("`{addr}` is not an IP address"))?;
        let prefix: u8 = prefix
            .parse()
            .map_err(|_| format!("`{prefix}` is not a prefix length"))?;

        let max = if network.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return Err(format!(
                "/{prefix} is too long for {network}; the maximum is /{max}"
            ));
        }
        Ok(Self { network, prefix })
    }

    pub fn contains(&self, addr: IpAddr) -> bool {
        match (self.network, addr) {
            (IpAddr::V4(net), IpAddr::V4(other)) => {
                masked(&net.octets(), &other.octets(), self.prefix)
            }
            (IpAddr::V6(net), IpAddr::V6(other)) => {
                masked(&net.octets(), &other.octets(), self.prefix)
            }
            // A v4-mapped v6 address is deliberately not matched against a v4
            // network. Treating them as equivalent is how a proxy allow-list
            // ends up wider than it reads.
            _ => false,
        }
    }
}

fn masked(network: &[u8], other: &[u8], prefix: u8) -> bool {
    let full = (prefix / 8) as usize;
    if network[..full] != other[..full] {
        return false;
    }
    let bits = prefix % 8;
    if bits == 0 {
        return true;
    }
    let mask = 0xFFu8 << (8 - bits);
    network[full] & mask == other[full] & mask
}

/// The shape the environment is read into, before anything is checked.
#[derive(Debug, Deserialize)]
pub struct RawConfig {
    #[serde(default)]
    pub database_url: String,
    #[serde(default = "default_bind")]
    pub bind_addr: String,
    #[serde(default)]
    pub library_root: String,

    /// `oidc` or `none`.
    ///
    /// Explicit rather than inferred from an empty issuer: a typo in
    /// `OIDC_ISSUER_URL` must fail the boot, not silently serve an
    /// unauthenticated instance.
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    #[serde(default)]
    pub oidc_issuer_url: String,
    #[serde(default)]
    pub oidc_client_id: String,
    #[serde(default)]
    pub oidc_client_secret: String,
    #[serde(default)]
    pub oidc_redirect_url: String,
    #[serde(default)]
    pub auth_allowed_subjects: String,
    #[serde(default)]
    pub auth_allowed_groups: String,
    #[serde(default)]
    pub session_key: String,
    #[serde(default = "default_session_ttl")]
    pub session_ttl_hours: u32,

    #[serde(default = "default_workers")]
    pub job_workers: usize,
    #[serde(default = "default_poll_ms")]
    pub job_poll_interval_ms: u64,
    #[serde(default = "default_source_concurrency")]
    pub source_max_concurrency: u32,
    #[serde(default = "default_stale_secs")]
    pub job_stale_lock_secs: u64,
    #[serde(default = "default_retention_days")]
    pub job_retention_days: i64,
    #[serde(default = "default_drain_secs")]
    pub job_drain_timeout_secs: u64,

    #[serde(default)]
    pub flaresolverr_url: String,
    #[serde(default = "default_wasm_memory")]
    pub wasm_max_memory_mb: u32,
    #[serde(default = "default_wasm_timeout")]
    pub wasm_epoch_timeout_ms: u64,

    #[serde(default = "default_slow_ms")]
    pub db_slow_statement_ms: u64,
    #[serde(default = "default_service_name")]
    pub otel_service_name: String,
    #[serde(default = "default_log_format")]
    pub log_format: String,

    #[serde(default = "default_true")]
    pub telemetry_ingest_enabled: bool,
    #[serde(default = "default_ingest_bytes")]
    pub telemetry_ingest_max_body_bytes: usize,

    #[serde(default)]
    pub trusted_proxies: String,
    #[serde(default = "default_sse_retry")]
    pub sse_retry_ms: u64,
}

/// Mirrors the `serde` defaults, so "what happens when this is unset" is
/// answerable in one place — and so a test can start from a working
/// configuration and break exactly one field.
impl Default for RawConfig {
    fn default() -> Self {
        Self {
            database_url: String::new(),
            bind_addr: default_bind(),
            library_root: String::new(),
            auth_mode: default_auth_mode(),
            oidc_issuer_url: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret: String::new(),
            oidc_redirect_url: String::new(),
            auth_allowed_subjects: String::new(),
            auth_allowed_groups: String::new(),
            session_key: String::new(),
            session_ttl_hours: default_session_ttl(),
            job_workers: default_workers(),
            job_poll_interval_ms: default_poll_ms(),
            source_max_concurrency: default_source_concurrency(),
            job_stale_lock_secs: default_stale_secs(),
            job_retention_days: default_retention_days(),
            job_drain_timeout_secs: default_drain_secs(),
            flaresolverr_url: String::new(),
            wasm_max_memory_mb: default_wasm_memory(),
            wasm_epoch_timeout_ms: default_wasm_timeout(),
            db_slow_statement_ms: default_slow_ms(),
            otel_service_name: default_service_name(),
            log_format: default_log_format(),
            telemetry_ingest_enabled: default_true(),
            telemetry_ingest_max_body_bytes: default_ingest_bytes(),
            trusted_proxies: String::new(),
            sse_retry_ms: default_sse_retry(),
        }
    }
}

fn default_bind() -> String {
    "0.0.0.0:8080".into()
}
fn default_auth_mode() -> String {
    "oidc".into()
}
fn default_session_ttl() -> u32 {
    720
}
fn default_workers() -> usize {
    4
}
fn default_poll_ms() -> u64 {
    1000
}
fn default_source_concurrency() -> u32 {
    4
}
fn default_stale_secs() -> u64 {
    900
}
fn default_retention_days() -> i64 {
    30
}
fn default_drain_secs() -> u64 {
    30
}
fn default_wasm_memory() -> u32 {
    128
}
fn default_wasm_timeout() -> u64 {
    30_000
}
fn default_slow_ms() -> u64 {
    250
}
fn default_service_name() -> String {
    "nyuka-api".into()
}
fn default_log_format() -> String {
    "json".into()
}
fn default_true() -> bool {
    true
}
fn default_ingest_bytes() -> usize {
    262_144
}
fn default_sse_retry() -> u64 {
    5_000
}

/// A random key for a run with no configured one.
///
/// Built from UUIDv4 bytes, which come from the platform CSPRNG. Only reached
/// with `AUTH_MODE=none`, where losing sessions on restart costs nothing
/// because there is no sign-in to lose.
fn ephemeral_session_key() -> String {
    let mut key = String::with_capacity(SESSION_KEY_BYTES * 2);
    while key.len() < SESSION_KEY_BYTES * 2 {
        key.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    key.truncate(SESSION_KEY_BYTES * 2);
    key
}

/// Splits a comma-separated list, dropping blanks.
///
/// Trailing commas and stray whitespace are ordinary in a `.env` file and
/// would otherwise become an allow-list entry that matches nothing and is
/// invisible in a listing.
fn list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The OIDC settings.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub mode: AuthMode,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: Secret,
    pub redirect_url: String,
    /// Subjects permitted to sign in.
    pub allowed_subjects: Vec<String>,
    /// Groups permitted to sign in, from the token's `groups` claim.
    pub allowed_groups: Vec<String>,
    pub session_key: Secret,
    pub session_ttl: Duration,
}

impl AuthConfig {
    /// Whether anyone at all may sign in.
    ///
    /// Both lists empty is checked at startup, but the check stays here too:
    /// startup validation proves the operator was told, and this proves the
    /// system denies regardless. Only one of those survives someone adding a
    /// `--skip-validation` flag.
    pub fn permits(&self, subject: &str, groups: &[String]) -> bool {
        // Not reachable with authentication off — there is no callback to
        // check — but answering honestly matters more than being unreachable,
        // because an unreachable branch that returns the wrong thing is one
        // refactor away from being reachable.
        if self.mode.is_disabled() {
            return true;
        }
        self.allowed_subjects.iter().any(|s| s == subject)
            || groups
                .iter()
                .any(|g| self.allowed_groups.iter().any(|a| a == g))
    }
}

#[derive(Debug, Clone)]
pub struct JobsConfig {
    pub workers: usize,
    pub poll_interval: Duration,
    pub source_max_concurrency: u32,
    pub stale_lock: Duration,
    pub retention: chrono::Duration,
    pub drain_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct SourcesConfig {
    pub flaresolverr_url: Option<String>,
    pub wasm_max_memory_bytes: usize,
    pub wasm_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub service_name: String,
    /// Crate version plus the build's git SHA (ADR-0015).
    pub service_version: String,
    pub ingest_enabled: bool,
    pub ingest_max_body_bytes: usize,
    pub db_slow_statement: Duration,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: Secret,
    pub bind_addr: SocketAddr,
    pub library_root: PathBuf,
    pub auth: AuthConfig,
    pub jobs: JobsConfig,
    pub sources: SourcesConfig,
    pub telemetry: TelemetryConfig,
    pub trusted_proxies: Vec<Cidr>,
    pub sse_retry: Duration,
}

/// The session cookie key length `cookie::Key` requires.
///
/// Passing a shorter one panics inside that crate, so it is checked here where
/// the message can say what to do about it.
const SESSION_KEY_BYTES: usize = 64;

impl RawConfig {
    /// Reads `nyuka.toml` if present, then the environment.
    ///
    /// Environment last, so a container can override a file baked into an
    /// image without rebuilding it.
    pub fn load() -> Result<Self, Box<figment::Error>> {
        Figment::new()
            .merge(Toml::file("nyuka.toml"))
            .merge(Env::raw())
            .extract()
            // Boxed: `figment::Error` is 200+ bytes, and this `Result` is in
            // the startup path's return type.
            .map_err(Box::new)
    }

    pub fn validate(self) -> Result<Config, ConfigError> {
        let mut problems = Vec::new();

        if self.database_url.trim().is_empty() {
            problems.push("DATABASE_URL is required".into());
        } else if url::Url::parse(&self.database_url).is_err() {
            problems.push("DATABASE_URL is not a url".into());
        }

        let bind_addr = match self.bind_addr.parse::<SocketAddr>() {
            Ok(addr) => Some(addr),
            Err(_) => {
                problems.push(format!(
                    "BIND_ADDR `{}` is not an address and port, such as 0.0.0.0:8080",
                    self.bind_addr
                ));
                None
            }
        };

        if self.library_root.trim().is_empty() {
            problems.push("LIBRARY_ROOT is required".into());
        }

        let mode = match AuthMode::parse(&self.auth_mode) {
            Some(mode) => mode,
            None => {
                problems.push(format!(
                    "AUTH_MODE `{}` is not recognised; expected `oidc` or `none`",
                    self.auth_mode
                ));
                // Assume the secure mode while collecting the rest, so a
                // typo does not also suppress every OIDC problem below it.
                AuthMode::Oidc
            }
        };

        let allowed_subjects = list(&self.auth_allowed_subjects);
        let allowed_groups = list(&self.auth_allowed_groups);
        let session_key = self.session_key.trim().to_string();

        // Only required when something authenticates. With `AUTH_MODE=none`
        // there is no flow to configure, and demanding an issuer, a client
        // secret and an allow-list for a deployment that uses none of them is
        // the friction that makes people pick the insecure option and then
        // fake the values.
        if mode == AuthMode::Oidc {
            for (name, value) in [
                ("OIDC_ISSUER_URL", &self.oidc_issuer_url),
                ("OIDC_CLIENT_ID", &self.oidc_client_id),
                ("OIDC_CLIENT_SECRET", &self.oidc_client_secret),
                ("OIDC_REDIRECT_URL", &self.oidc_redirect_url),
            ] {
                if value.trim().is_empty() {
                    problems.push(format!("{name} is required when AUTH_MODE is `oidc`"));
                }
            }

            for (name, value) in [
                ("OIDC_ISSUER_URL", &self.oidc_issuer_url),
                ("OIDC_REDIRECT_URL", &self.oidc_redirect_url),
            ] {
                if !value.trim().is_empty() && url::Url::parse(value).is_err() {
                    problems.push(format!("{name} is not a url"));
                }
            }

            if allowed_subjects.is_empty() && allowed_groups.is_empty() {
                problems.push(
                    "AUTH_ALLOWED_SUBJECTS and AUTH_ALLOWED_GROUPS are both empty, which denies \
                     everyone including you; set at least one"
                        .into(),
                );
            }

            if session_key.is_empty() {
                problems.push(format!(
                    "SESSION_KEY is required when AUTH_MODE is `oidc`; generate one with \
                     `openssl rand -hex {SESSION_KEY_BYTES}`"
                ));
            } else if session_key.len() < SESSION_KEY_BYTES {
                problems.push(format!(
                    "SESSION_KEY is {} characters; at least {SESSION_KEY_BYTES} are required, and \
                     restarting with a different one logs everyone out",
                    session_key.len()
                ));
            }
        } else if !session_key.is_empty() && session_key.len() < SESSION_KEY_BYTES {
            // Optional here, but a short one is still a mistake worth naming
            // rather than silently replacing.
            problems.push(format!(
                "SESSION_KEY is {} characters; at least {SESSION_KEY_BYTES} are required, or \
                 leave it unset and one will be generated for this run",
                session_key.len()
            ));
        }

        if self.session_ttl_hours == 0 {
            problems.push("SESSION_TTL_HOURS must be greater than zero".into());
        }
        if self.job_workers == 0 {
            problems.push("JOB_WORKERS must be greater than zero, or nothing runs".into());
        }
        if self.source_max_concurrency == 0 {
            problems.push(
                "SOURCE_MAX_CONCURRENCY must be greater than zero, or every download blocks \
                 forever"
                    .into(),
            );
        }
        if self.job_poll_interval_ms == 0 {
            problems.push("JOB_POLL_INTERVAL_MS must be greater than zero".into());
        }
        // The redaction writer parses each line as JSON and drops what it
        // cannot parse, so a text formatter would silently discard every log
        // line. Refusing here is better than discovering it from an empty log.
        if !self.log_format.trim().eq_ignore_ascii_case("json") {
            problems.push(format!(
                "LOG_FORMAT `{}` is not supported; only `json` is implemented, because log \
                 redaction parses each line as JSON",
                self.log_format
            ));
        }

        if self.wasm_max_memory_mb == 0 {
            problems.push("WASM_MAX_MEMORY_MB must be greater than zero".into());
        }
        if self.wasm_epoch_timeout_ms == 0 {
            problems.push(
                "WASM_EPOCH_TIMEOUT_MS must be greater than zero, or a source can run forever"
                    .into(),
            );
        }

        let retention = chrono::Duration::days(self.job_retention_days.max(0));
        // The scheduler suppresses duplicate periodic jobs with a key that
        // only works while the earlier row still exists, so retention shorter
        // than the longest period makes those jobs fire more than once per
        // period — silently.
        if let Err(e) = nyuka_jobs::scheduler::check_retention(
            retention,
            &nyuka_jobs::scheduler::default_schedule(),
        ) {
            problems.push(format!("JOB_RETENTION_DAYS: {e}"));
        }

        let mut trusted_proxies = Vec::new();
        for entry in list(&self.trusted_proxies) {
            match Cidr::parse(&entry) {
                Ok(cidr) => trusted_proxies.push(cidr),
                Err(e) => problems.push(format!("TRUSTED_PROXIES: {e}")),
            }
        }

        if !problems.is_empty() {
            return Err(ConfigError { problems });
        }

        Ok(Config {
            database_url: self.database_url.into(),
            bind_addr: bind_addr.expect("checked above"),
            library_root: PathBuf::from(self.library_root),
            auth: AuthConfig {
                mode,
                issuer_url: self.oidc_issuer_url,
                client_id: self.oidc_client_id,
                client_secret: self.oidc_client_secret.into(),
                redirect_url: self.oidc_redirect_url,
                allowed_subjects,
                allowed_groups,
                // With authentication off and no key supplied, one is
                // generated per run. Sessions then carry nothing that
                // survives a restart — which is correct, because with no
                // sign-in there is no state in them worth keeping.
                session_key: if session_key.is_empty() {
                    ephemeral_session_key().into()
                } else {
                    session_key.into()
                },
                session_ttl: Duration::from_secs(u64::from(self.session_ttl_hours) * 3600),
            },
            jobs: JobsConfig {
                workers: self.job_workers,
                poll_interval: Duration::from_millis(self.job_poll_interval_ms),
                source_max_concurrency: self.source_max_concurrency,
                stale_lock: Duration::from_secs(self.job_stale_lock_secs),
                retention,
                drain_timeout: Duration::from_secs(self.job_drain_timeout_secs),
            },
            sources: SourcesConfig {
                flaresolverr_url: Some(self.flaresolverr_url).filter(|u| !u.trim().is_empty()),
                wasm_max_memory_bytes: self.wasm_max_memory_mb as usize * 1024 * 1024,
                wasm_timeout: Duration::from_millis(self.wasm_epoch_timeout_ms),
            },
            telemetry: TelemetryConfig {
                service_name: self.otel_service_name,
                service_version: format!("{}+{}", env!("CARGO_PKG_VERSION"), env!("NYUKA_GIT_SHA")),
                ingest_enabled: self.telemetry_ingest_enabled,
                ingest_max_body_bytes: self.telemetry_ingest_max_body_bytes,
                db_slow_statement: Duration::from_millis(self.db_slow_statement_ms),
            },
            trusted_proxies,
            sse_retry: Duration::from_millis(self.sse_retry_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration that passes, so each test can break exactly one thing.
    fn valid() -> RawConfig {
        RawConfig {
            database_url: "postgres://u:p@localhost:5432/nyuka".into(),
            library_root: "/library".into(),
            oidc_issuer_url: "https://auth.example.test".into(),
            oidc_client_id: "nyuka".into(),
            oidc_client_secret: "shh".into(),
            oidc_redirect_url: "https://nyuka.example.test/api/v1/auth/callback".into(),
            auth_allowed_subjects: "alice".into(),
            session_key: "0".repeat(SESSION_KEY_BYTES),
            ..RawConfig::default()
        }
    }

    fn problems(raw: RawConfig) -> Vec<String> {
        raw.validate().err().map(|e| e.problems).unwrap_or_default()
    }

    fn mentions(problems: &[String], needle: &str) -> bool {
        problems.iter().any(|p| p.contains(needle))
    }

    #[test]
    fn the_sample_configuration_validates() {
        let config = valid().validate().expect("the baseline must be valid");
        assert_eq!(config.bind_addr.port(), 8080);
        assert_eq!(config.auth.allowed_subjects, vec!["alice".to_string()]);
        assert_eq!(config.jobs.workers, 4);
    }

    /// The point of collecting: one restart must surface every mistake.
    #[test]
    fn every_problem_is_reported_at_once() {
        let broken = RawConfig {
            database_url: String::new(),
            bind_addr: "not-an-address".into(),
            library_root: String::new(),
            session_key: String::new(),
            job_workers: 0,
            ..valid()
        };
        let problems = problems(broken);
        assert!(mentions(&problems, "DATABASE_URL"));
        assert!(mentions(&problems, "BIND_ADDR"));
        assert!(mentions(&problems, "LIBRARY_ROOT"));
        assert!(mentions(&problems, "SESSION_KEY"));
        assert!(mentions(&problems, "JOB_WORKERS"));
        assert!(
            problems.len() >= 5,
            "stopping at the first problem makes fixing six of them six restarts"
        );
    }

    /// Both lists empty locks the operator out of their own install. Failing
    /// at startup is how they find out now rather than at the login screen.
    #[test]
    fn an_empty_allow_list_is_refused_at_startup() {
        let problems = problems(RawConfig {
            auth_allowed_subjects: String::new(),
            auth_allowed_groups: String::new(),
            ..valid()
        });
        assert!(mentions(&problems, "denies everyone"));
    }

    #[test]
    fn either_allow_list_alone_is_enough() {
        assert!(
            RawConfig {
                auth_allowed_subjects: String::new(),
                auth_allowed_groups: "admins".into(),
                ..valid()
            }
            .validate()
            .is_ok()
        );
    }

    fn barebones() -> RawConfig {
        RawConfig {
            database_url: "postgres://u:p@localhost:5432/nyuka".into(),
            library_root: "/library".into(),
            auth_mode: "none".into(),
            ..RawConfig::default()
        }
    }

    /// The point of the mode: a barebones deployment needs a database, a
    /// library, and nothing else.
    #[test]
    fn no_auth_needs_no_oidc_settings_and_no_secret() {
        let config = barebones().validate().expect("should be valid");
        assert_eq!(config.auth.mode, AuthMode::None);
        assert!(config.auth.allowed_subjects.is_empty());
    }

    /// Generated per run. Sessions carry nothing that survives a restart
    /// because there is no sign-in to lose.
    #[test]
    fn no_auth_generates_a_session_key_when_none_is_given() {
        let first = barebones().validate().expect("valid");
        let second = barebones().validate().expect("valid");

        assert!(first.auth.session_key.expose().len() >= SESSION_KEY_BYTES);
        assert_ne!(
            first.auth.session_key.expose(),
            second.auth.session_key.expose(),
            "a fixed fallback key would be a published secret"
        );
    }

    #[test]
    fn a_supplied_session_key_is_still_used_with_no_auth() {
        let config = RawConfig {
            session_key: "a".repeat(SESSION_KEY_BYTES),
            ..barebones()
        }
        .validate()
        .expect("valid");
        assert_eq!(
            config.auth.session_key.expose(),
            "a".repeat(SESSION_KEY_BYTES)
        );
    }

    /// A short key is a mistake either way, and silently replacing it would
    /// hide it.
    #[test]
    fn a_short_session_key_is_still_refused_with_no_auth() {
        let problems = problems(RawConfig {
            session_key: "short".into(),
            ..barebones()
        });
        assert!(mentions(&problems, "SESSION_KEY"));
    }

    /// The mode is explicit precisely so a typo cannot disable
    /// authentication. An unrecognised value must fail, not fall back.
    #[test]
    fn an_unrecognised_auth_mode_is_refused() {
        for value in ["", "off", "disabled", "no", "oidc none", "0"] {
            let problems = problems(RawConfig {
                auth_mode: value.into(),
                ..valid()
            });
            assert!(
                mentions(&problems, "AUTH_MODE"),
                "{value:?} must be refused rather than guessed at"
            );
        }
    }

    /// Case and surrounding whitespace are tolerated: a trailing space in a
    /// `.env` file is ordinary, and failing the boot over one would be
    /// friction with no safety in it.
    #[test]
    fn the_mode_is_case_insensitive_and_trimmed() {
        for value in ["oidc", "OIDC", "Oidc", " oidc ", "oidc\t"] {
            assert!(
                RawConfig {
                    auth_mode: value.into(),
                    ..valid()
                }
                .validate()
                .is_ok(),
                "{value} should be accepted"
            );
        }
        for value in ["none", "NONE", "None", " none "] {
            assert!(
                RawConfig {
                    auth_mode: value.into(),
                    ..barebones()
                }
                .validate()
                .is_ok(),
                "{value} should be accepted"
            );
        }
    }

    /// A bad mode must not also suppress the OIDC problems underneath it, or
    /// fixing the typo reveals four more failures one restart later.
    #[test]
    fn an_unrecognised_mode_still_reports_the_oidc_problems() {
        let problems = problems(RawConfig {
            auth_mode: "nope".into(),
            oidc_issuer_url: String::new(),
            ..valid()
        });
        assert!(mentions(&problems, "AUTH_MODE"));
        assert!(mentions(&problems, "OIDC_ISSUER_URL"));
    }

    /// Unreachable while authentication is off, but an unreachable branch
    /// that returns the wrong thing is one refactor from being reachable.
    #[test]
    fn permits_admits_everyone_when_authentication_is_off() {
        let config = barebones().validate().expect("valid");
        assert!(config.auth.permits("anybody", &[]));
    }

    /// The runtime check stays fail-closed independently of the startup one.
    #[test]
    fn an_empty_allow_list_permits_nobody() {
        let auth = AuthConfig {
            mode: AuthMode::Oidc,
            issuer_url: String::new(),
            client_id: String::new(),
            client_secret: String::new().into(),
            redirect_url: String::new(),
            allowed_subjects: vec![],
            allowed_groups: vec![],
            session_key: String::new().into(),
            session_ttl: Duration::from_secs(1),
        };
        assert!(!auth.permits("alice", &["admins".into()]));
    }

    #[test]
    fn a_permitted_subject_or_group_is_admitted_and_others_are_not() {
        let auth = AuthConfig {
            mode: AuthMode::Oidc,
            issuer_url: String::new(),
            client_id: String::new(),
            client_secret: String::new().into(),
            redirect_url: String::new(),
            allowed_subjects: vec!["alice".into()],
            allowed_groups: vec!["admins".into()],
            session_key: String::new().into(),
            session_ttl: Duration::from_secs(1),
        };
        assert!(auth.permits("alice", &[]));
        assert!(auth.permits("bob", &["admins".into()]));
        assert!(!auth.permits("bob", &["users".into()]));
        assert!(!auth.permits("bob", &[]));
    }

    /// A short key panics inside `cookie::Key`, which is a worse error than
    /// this one.
    #[test]
    fn a_short_session_key_is_refused_with_a_usable_message() {
        let problems = problems(RawConfig {
            session_key: "too-short".into(),
            ..valid()
        });
        assert!(mentions(&problems, "SESSION_KEY"));
        assert!(
            mentions(&problems, "logs everyone out"),
            "the message must say what changing it costs"
        );
    }

    /// The cross-check between two modules that otherwise never meet.
    #[test]
    fn retention_shorter_than_the_longest_schedule_period_is_refused() {
        let problems = problems(RawConfig {
            job_retention_days: 1,
            ..valid()
        });
        assert!(mentions(&problems, "JOB_RETENTION_DAYS"));
        assert!(mentions(&problems, "more than once per period"));
    }

    #[test]
    fn zero_valued_knobs_that_would_stall_the_system_are_refused() {
        for (raw, expected) in [
            (
                RawConfig {
                    source_max_concurrency: 0,
                    ..valid()
                },
                "SOURCE_MAX_CONCURRENCY",
            ),
            (
                RawConfig {
                    job_poll_interval_ms: 0,
                    ..valid()
                },
                "JOB_POLL_INTERVAL_MS",
            ),
            (
                RawConfig {
                    wasm_epoch_timeout_ms: 0,
                    ..valid()
                },
                "WASM_EPOCH_TIMEOUT_MS",
            ),
            (
                RawConfig {
                    session_ttl_hours: 0,
                    ..valid()
                },
                "SESSION_TTL_HOURS",
            ),
        ] {
            assert!(
                mentions(&problems(raw), expected),
                "{expected} of zero must be refused"
            );
        }
    }

    /// Trailing commas and stray spaces are ordinary in a `.env` file, and an
    /// empty allow-list entry matches nothing while looking like it does.
    /// Redaction parses each line as JSON, so a text formatter would drop
    /// every log line without saying so.
    #[test]
    fn an_unsupported_log_format_is_refused() {
        let problems = problems(RawConfig {
            log_format: "text".into(),
            ..valid()
        });
        assert!(mentions(&problems, "LOG_FORMAT"));
        assert!(mentions(&problems, "redaction"));
    }

    #[test]
    fn list_values_ignore_blanks_and_whitespace() {
        assert_eq!(list(" alice , bob ,, "), vec!["alice", "bob"]);
        assert!(list(" , ").is_empty());
    }

    #[test]
    fn an_unset_flaresolverr_url_is_none_rather_than_empty() {
        let config = valid().validate().expect("valid");
        assert!(config.sources.flaresolverr_url.is_none());
    }

    #[test]
    fn a_secret_does_not_print_itself() {
        let rendered = format!("{:?}", Secret::from("hunter2".to_string()));
        assert_eq!(rendered, "***");
        assert!(!rendered.contains("hunter2"));
    }

    /// `Config` is `Debug` so startup can log it. That is only safe while the
    /// secret-bearing fields are `Secret`.
    #[test]
    fn debugging_the_whole_config_does_not_leak_a_secret() {
        let config = RawConfig {
            oidc_client_secret: "super-secret-value".into(),
            database_url: "postgres://user:db-password@localhost/nyuka".into(),
            ..valid()
        }
        .validate()
        .expect("valid");

        let rendered = format!("{config:?}");
        assert!(!rendered.contains("super-secret-value"));
        assert!(!rendered.contains("db-password"));
        assert!(
            rendered.contains("nyuka-api"),
            "non-secret fields must still be visible, or logging it is pointless"
        );
    }

    #[test]
    fn a_cidr_matches_addresses_inside_it_and_not_outside() {
        let net = Cidr::parse("10.1.2.0/24").expect("parse");
        assert!(net.contains("10.1.2.0".parse().expect("ip")));
        assert!(net.contains("10.1.2.255".parse().expect("ip")));
        assert!(!net.contains("10.1.3.0".parse().expect("ip")));
    }

    #[test]
    fn a_cidr_honours_a_prefix_that_is_not_a_whole_byte() {
        let net = Cidr::parse("192.168.0.0/20").expect("parse");
        assert!(net.contains("192.168.15.255".parse().expect("ip")));
        assert!(
            !net.contains("192.168.16.0".parse().expect("ip")),
            "a /20 must not stretch to the next 16 blocks"
        );
    }

    #[test]
    fn a_zero_prefix_matches_everything_of_its_family() {
        let net = Cidr::parse("0.0.0.0/0").expect("parse");
        assert!(net.contains("8.8.8.8".parse().expect("ip")));
        assert!(
            !net.contains("::1".parse().expect("ip")),
            "a v4 default route must not swallow v6"
        );
    }

    #[test]
    fn ipv6_networks_work() {
        let net = Cidr::parse("2001:db8::/32").expect("parse");
        assert!(net.contains("2001:db8::1".parse().expect("ip")));
        assert!(!net.contains("2001:db9::1".parse().expect("ip")));
    }

    /// Treating a v4-mapped v6 address as matching a v4 network is how a proxy
    /// allow-list ends up wider than it reads.
    #[test]
    fn a_v4_mapped_v6_address_does_not_match_a_v4_network() {
        let net = Cidr::parse("10.0.0.0/8").expect("parse");
        assert!(!net.contains("::ffff:10.0.0.1".parse().expect("ip")));
    }

    #[test]
    fn a_malformed_cidr_names_what_is_wrong() {
        assert!(
            Cidr::parse("10.0.0.0")
                .expect_err("no prefix")
                .contains("prefix length")
        );
        assert!(
            Cidr::parse("not-an-ip/24")
                .expect_err("bad ip")
                .contains("IP address")
        );
        assert!(
            Cidr::parse("10.0.0.0/33")
                .expect_err("too long")
                .contains("maximum")
        );
    }

    #[test]
    fn a_malformed_trusted_proxy_entry_fails_startup() {
        let problems = problems(RawConfig {
            trusted_proxies: "10.0.0.0/8, nonsense".into(),
            ..valid()
        });
        assert!(mentions(&problems, "TRUSTED_PROXIES"));
    }
}
