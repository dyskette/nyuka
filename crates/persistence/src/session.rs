//! Session storage.
//!
//! # Why this is hand-written
//!
//! Every published `tower-sessions` store conflicts with this workspace
//! (ADR-0005), verified against crates.io rather than assumed:
//!
//! | crate | blocker |
//! |---|---|
//! | `tower-sessions-sqlx-store` | needs `sqlx ^0.8`; SeaORM 2.x is on 0.9. It also pins `tower-sessions-core ^0.14` while `tower-sessions` 0.15 pins `=0.15`. |
//! | `tower-sessions-seaorm-store` | needs `sea-orm ^1.1` and `tower-sessions ^0.14` |
//!
//! Either would mean a second SQLx version and a second connection pool in the
//! process — the same trap the job queue avoids.
//!
//! # Why it is not a `SessionStore` here
//!
//! `tower_sessions::SessionStore` is a web concern. Implementing it in this
//! crate would drag the session middleware into the persistence layer, so what
//! lives here is a plain repository and `nyuka-api` adapts it to the trait.
//! The hexagonal rule points inward, and a store trait from an HTTP middleware
//! is not something the storage layer should know about.
//!
//! # The encoding is a contract
//!
//! Session data is stored as MessagePack. That choice is not arbitrary and it
//! is not free to change: **switching encodings invalidates every live
//! session**, logging everyone out. MessagePack matches what the published
//! stores use, so moving to one later would not require re-encoding anything.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, Value};
use uuid::Uuid;

/// One stored session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// The opaque id the browser holds in its cookie.
    pub id: String,
    /// Set once the session is authenticated.
    pub user_id: Option<Uuid>,
    /// MessagePack-encoded session data.
    pub data: Vec<u8>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session storage failed: {0}")]
    Storage(String),
    #[error("session data could not be encoded: {0}")]
    Encode(String),
    #[error("session data could not be decoded: {0}")]
    Decode(String),
}

type Result<T> = std::result::Result<T, SessionError>;

fn db(e: sea_orm::DbErr) -> SessionError {
    SessionError::Storage(e.to_string())
}

#[derive(Debug, Clone)]
pub struct SessionRepository {
    db: DatabaseConnection,
}

impl SessionRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Encodes a value as session data.
    pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
        rmp_serde::to_vec_named(value).map_err(|e| SessionError::Encode(e.to_string()))
    }

    /// Decodes session data.
    pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
        rmp_serde::from_slice(bytes).map_err(|e| SessionError::Decode(e.to_string()))
    }

    /// Inserts or replaces a session.
    pub async fn save(&self, record: &SessionRecord) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "INSERT INTO session (id, user_id, data, expires_at) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (id) DO UPDATE SET user_id = EXCLUDED.user_id, \
                 data = EXCLUDED.data, expires_at = EXCLUDED.expires_at",
                [
                    record.id.clone().into(),
                    record.user_id.map(Value::from).unwrap_or(Value::Uuid(None)),
                    record.data.clone().into(),
                    record.expires_at.into(),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Loads a session, treating an expired one as absent.
    ///
    /// Filtering in the query rather than after it means an expired session can
    /// never be used even if the cleanup job is behind, which it will be
    /// between runs.
    pub async fn load(&self, id: &str) -> Result<Option<SessionRecord>> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT id, user_id, data, expires_at FROM session \
                 WHERE id = $1 AND expires_at > now()",
                [id.into()],
            ))
            .await
            .map_err(db)?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(SessionRecord {
            id: row.try_get("", "id").map_err(db)?,
            user_id: row.try_get("", "user_id").map_err(db)?,
            data: row.try_get("", "data").map_err(db)?,
            expires_at: row.try_get("", "expires_at").map_err(db)?,
        }))
    }

    /// Deletes one session. This is what `POST /auth/logout` does, and it takes
    /// effect immediately rather than waiting for expiry.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "DELETE FROM session WHERE id = $1",
                [id.into()],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Deletes every session for a user.
    ///
    /// The operator action for the IdP-revocation gap ADR-0005 documents: if
    /// an account is disabled upstream, its sessions here stay valid until
    /// they expire unless something removes them.
    pub async fn delete_for_user(&self, user_id: Uuid) -> Result<u64> {
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "DELETE FROM session WHERE user_id = $1",
                [user_id.into()],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Removes expired sessions. Driven by the `prune_sessions` job kind.
    pub async fn delete_expired(&self) -> Result<u64> {
        let result = self
            .db
            .execute_raw(Statement::from_string(
                self.db.get_database_backend(),
                "DELETE FROM session WHERE expires_at <= now()",
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Data {
        csrf: String,
        count: u32,
    }

    #[test]
    fn session_data_round_trips_through_messagepack() {
        let value = Data {
            csrf: "abc".into(),
            count: 3,
        };
        let bytes = SessionRepository::encode(&value).expect("encode");
        assert_eq!(
            SessionRepository::decode::<Data>(&bytes).expect("decode"),
            value
        );
    }

    /// Named encoding is what makes a field addition survive: positional
    /// MessagePack would mis-read every existing session the moment the struct
    /// grows a field.
    #[test]
    fn the_encoding_is_field_named() {
        let bytes = SessionRepository::encode(&Data {
            csrf: "abc".into(),
            count: 3,
        })
        .expect("encode");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("csrf"), "field names must be in the payload");
        assert!(text.contains("count"));
    }

    #[test]
    fn garbage_decodes_to_an_error_rather_than_a_default() {
        assert!(SessionRepository::decode::<Data>(b"not messagepack").is_err());
    }
}
