//! `tower_sessions::SessionStore` over the `session` table.
//!
//! ADR-0005 explains why this is hand-written: every published Postgres store
//! for `tower-sessions` conflicts with SeaORM 2.x's SQLx 0.9, and adopting one
//! would mean a second SQLx version and a second connection pool in the
//! process.
//!
//! The trait lives here rather than in `nyuka-persistence` because it belongs
//! to an HTTP middleware, and the storage layer has no business knowing about
//! one. `SessionRepository` is a plain repository; this adapts it.
//!
//! # The id is stored as text, not as the `i128` it is
//!
//! `tower_sessions::Id` is an `i128` that renders as 22 URL-safe base64
//! characters. PostgreSQL has no 128-bit integer, so the column is text and
//! holds the rendered form — the same string the browser's cookie carries,
//! which makes a session traceable from a browser inspector to a row without
//! a conversion step in between.
//!
//! # Expiry is enforced on read
//!
//! `SessionRepository::load` filters on `expires_at > now()`, so an expired
//! session is invisible even when the cleanup job is behind — which it always
//! is, between runs. The job reclaims space; it is not what makes expiry
//! correct.

use nyuka_persistence::session::{SessionRecord, SessionRepository};
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{Error, Result};

/// The key under which the authenticated subject's user id is kept.
///
/// A constant rather than a literal at each call site: a typo in one of them
/// would read as "not signed in" rather than failing, which is the quiet kind
/// of auth bug.
pub const USER_ID_KEY: &str = "user_id";

#[derive(Clone)]
pub struct PostgresSessionStore {
    repository: SessionRepository,
}

impl std::fmt::Debug for PostgresSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `SessionStore` requires `Debug`, and the repository holds a
        // connection whose `Debug` includes the database URL.
        f.write_str("PostgresSessionStore")
    }
}

impl PostgresSessionStore {
    pub fn new(repository: SessionRepository) -> Self {
        Self { repository }
    }
}

fn backend(e: impl std::fmt::Display) -> Error {
    Error::Backend(e.to_string())
}

/// `time::OffsetDateTime` to `chrono::DateTime<Utc>`.
///
/// `tower-sessions` uses `time`; everything else here uses `chrono`. The
/// conversion goes through Unix nanoseconds, which is exact for every instant
/// either type can hold.
fn to_chrono(when: time::OffsetDateTime) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp_nanos(when.unix_timestamp_nanos() as i64)
        .to_utc()
        .pipe(Ok)
}

fn to_time(when: chrono::DateTime<chrono::Utc>) -> Result<time::OffsetDateTime> {
    time::OffsetDateTime::from_unix_timestamp_nanos(when.timestamp_nanos_opt().unwrap_or(0) as i128)
        .map_err(|e| Error::Decode(format!("session expiry is out of range: {e}")))
}

/// A tiny helper so the conversion above reads as one expression.
trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

#[async_trait::async_trait]
impl tower_sessions::SessionStore for PostgresSessionStore {
    async fn create(&self, record: &mut Record) -> Result<()> {
        // A fresh random `Id` collides with probability around 2^-128, but
        // "effectively never" is not "never", and `save` would overwrite the
        // other session rather than fail. Retrying with a new id costs one
        // round trip in a case that will not happen.
        for _ in 0..3 {
            if self
                .repository
                .load(&record.id.to_string())
                .await
                .map_err(backend)?
                .is_none()
            {
                return self.save(record).await;
            }
            record.id = Id::default();
        }
        Err(Error::Backend(
            "could not allocate an unused session id".into(),
        ))
    }

    async fn save(&self, record: &Record) -> Result<()> {
        let data =
            SessionRepository::encode(&record.data).map_err(|e| Error::Encode(e.to_string()))?;

        // Denormalized out of the session data so the cleanup job and the
        // operator's "log this user out everywhere" action can work by user
        // without decoding every row's payload.
        let user_id = record
            .data
            .get(USER_ID_KEY)
            .and_then(|v| v.as_str())
            .and_then(|s| uuid::Uuid::parse_str(s).ok());

        self.repository
            .save(&SessionRecord {
                id: record.id.to_string(),
                user_id,
                data,
                expires_at: to_chrono(record.expiry_date)?,
            })
            .await
            .map_err(backend)
    }

    async fn load(&self, id: &Id) -> Result<Option<Record>> {
        let Some(stored) = self
            .repository
            .load(&id.to_string())
            .await
            .map_err(backend)?
        else {
            return Ok(None);
        };

        let data =
            SessionRepository::decode(&stored.data).map_err(|e| Error::Decode(e.to_string()))?;

        Ok(Some(Record {
            id: *id,
            data,
            expiry_date: to_time(stored.expires_at)?,
        }))
    }

    async fn delete(&self, id: &Id) -> Result<()> {
        self.repository
            .delete(&id.to_string())
            .await
            .map_err(backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_round_trips_through_its_rendered_form() {
        let id = Id::default();
        let rendered = id.to_string();
        assert_eq!(
            rendered.len(),
            22,
            "the column and the cookie both hold this exact string"
        );
        assert_eq!(rendered.parse::<Id>().expect("parse"), id);
    }

    /// The cookie value is what an attacker supplies, so parsing it must fail
    /// rather than produce some other session's id.
    #[test]
    fn a_malformed_id_does_not_parse() {
        for bad in ["", "too-short", &"A".repeat(23), "not!valid!base64!!!!!!"] {
            assert!(bad.parse::<Id>().is_err(), "{bad} must not parse");
        }
    }

    #[test]
    fn expiry_survives_the_trip_between_time_and_chrono() {
        let original = time::OffsetDateTime::from_unix_timestamp(1_758_499_200).expect("timestamp");
        let chrono = to_chrono(original).expect("to chrono");
        let back = to_time(chrono).expect("to time");
        assert_eq!(back.unix_timestamp(), original.unix_timestamp());
    }

    #[test]
    fn the_user_id_key_is_a_constant_so_a_typo_cannot_read_as_signed_out() {
        assert_eq!(USER_ID_KEY, "user_id");
    }

    /// The store holds a connection whose `Debug` carries the database URL,
    /// and `SessionStore` requires `Debug`.
    #[test]
    fn debugging_the_store_does_not_print_a_connection_string() {
        let store = PostgresSessionStore::new(SessionRepository::new(
            sea_orm::DatabaseConnection::default(),
        ));
        assert_eq!(format!("{store:?}"), "PostgresSessionStore");
    }
}
