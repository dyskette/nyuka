//! The single place a connection pool is constructed.
//!
//! No other code path may build one, so pool sizing, timeouts, and logging
//! configuration cannot diverge (ADR-0002).

use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use tracing::log::LevelFilter;

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    /// Statements slower than this are logged at DEBUG. The observability
    /// design asks for slow queries only, not every statement.
    pub slow_statement_threshold: Duration,
}

impl PoolConfig {
    pub fn into_connect_options(self) -> ConnectOptions {
        let mut opts = ConnectOptions::new(self.url);
        opts.max_connections(self.max_connections)
            .min_connections(self.min_connections)
            .acquire_timeout(Duration::from_secs(10))
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

/// TODO(scaffold): call `Migrator::up` here and expose migration currency to
/// `/readyz`, per the architecture's "migrations run on boot".
pub async fn connect(config: PoolConfig) -> Result<DatabaseConnection, sea_orm::DbErr> {
    Database::connect(config.into_connect_options()).await
}
