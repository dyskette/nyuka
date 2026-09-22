//! Applies migrations, for CI and for operators.
//!
//! The API runs migrations at boot, so this exists for the cases where that is
//! not what you want: the CI schema-drift check needs a migrated database
//! before it can regenerate entities, and an operator sometimes needs to
//! migrate without starting the service.
//!
//! ```sh
//! DATABASE_URL=… cargo run -p nyuka-persistence --bin migrate -- up
//! DATABASE_URL=… cargo run -p nyuka-persistence --bin migrate -- fresh
//! ```

use nyuka_persistence::migration::{Migrator, MigratorTrait};
use sea_orm::Database;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set".to_string())?;
    let command = std::env::args().nth(1).unwrap_or_else(|| "up".into());

    let db = Database::connect(&url).await?;
    match command.as_str() {
        "up" => Migrator::up(&db, None).await?,
        // Drops everything first. For CI and local resets, never production.
        "fresh" => Migrator::fresh(&db).await?,
        "down" => Migrator::down(&db, None).await?,
        "status" => {
            let pending = Migrator::get_pending_migrations(&db).await?;
            if pending.is_empty() {
                println!("no pending migrations");
            }
            for m in pending {
                println!("pending: {}", m.name());
            }
        }
        other => {
            return Err(format!("unknown command {other:?}: use up, fresh, down or status").into());
        }
    }
    println!("{command}: ok");
    Ok(())
}
