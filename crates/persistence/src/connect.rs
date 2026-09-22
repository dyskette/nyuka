//! The single place a connection pool is constructed, and where migrations run
//! at boot.
//!
//! No other code path may build a pool, so pool sizing, timeouts, and logging
//! configuration cannot diverge (ADR-0002).

use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use sea_orm_migration::MigratorTrait;
use tracing::log::LevelFilter;

use crate::migration::Migrator;

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    /// Statements slower than this are logged at DEBUG. The observability
    /// design asks for slow queries only, not every statement.
    pub slow_statement_threshold: Duration,
    pub acquire_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 16,
            min_connections: 1,
            slow_statement_threshold: Duration::from_millis(250),
            acquire_timeout: Duration::from_secs(10),
        }
    }
}

impl PoolConfig {
    pub fn into_connect_options(self) -> ConnectOptions {
        let mut opts = ConnectOptions::new(self.url);
        opts.max_connections(self.max_connections)
            .min_connections(self.min_connections)
            .acquire_timeout(self.acquire_timeout)
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(1800))
            // Per-statement logging off; slow statements only. Both are
            // first-class ConnectOptions settings, not workarounds.
            .sqlx_logging(false)
            .sqlx_slow_statements_logging_settings(
                LevelFilter::Debug,
                self.slow_statement_threshold,
            );
        opts
    }
}

/// Connects without touching the schema.
pub async fn connect(config: PoolConfig) -> Result<DatabaseConnection, DbErr> {
    Database::connect(config.into_connect_options()).await
}

/// Connects and applies any pending migrations.
///
/// The architecture runs migrations on boot, so this is the path `main` uses.
/// It is deliberately not the only entry point: the `migrate` binary exists
/// for CI and for operators who need to migrate without starting the service.
pub async fn connect_and_migrate(config: PoolConfig) -> Result<DatabaseConnection, DbErr> {
    let db = connect(config).await?;
    Migrator::up(&db, None).await?;
    Ok(db)
}

/// What `/readyz` reports about the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    pub reachable: bool,
    /// False when migrations are outstanding. A running process on an
    /// out-of-date schema is a distinct failure from an unreachable database,
    /// and reporting them together makes an incident harder to read.
    pub migrations_current: bool,
    pub pending: Vec<String>,
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        self.reachable && self.migrations_current
    }
}

/// Checks reachability and migration currency for `/readyz`.
pub async fn readiness(db: &DatabaseConnection) -> Readiness {
    let pending = match Migrator::get_pending_migrations(db).await {
        Ok(ms) => ms.into_iter().map(|m| m.name().to_string()).collect(),
        Err(_) => {
            return Readiness {
                reachable: false,
                migrations_current: false,
                pending: Vec::new(),
            };
        }
    };
    Readiness {
        reachable: true,
        migrations_current: Vec::is_empty(&pending),
        pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_both_reachable_and_current() {
        assert!(
            Readiness {
                reachable: true,
                migrations_current: true,
                pending: vec![]
            }
            .is_ready()
        );
        assert!(
            !Readiness {
                reachable: true,
                migrations_current: false,
                pending: vec!["m0002".into()]
            }
            .is_ready(),
            "a reachable database on an out-of-date schema is not ready"
        );
        assert!(
            !Readiness {
                reachable: false,
                migrations_current: true,
                pending: vec![]
            }
            .is_ready()
        );
    }

    #[test]
    fn defaults_keep_statement_logging_off_and_slow_queries_on() {
        let cfg = PoolConfig::default();
        assert_eq!(cfg.slow_statement_threshold, Duration::from_millis(250));
        assert!(cfg.max_connections >= cfg.min_connections);
    }
}
