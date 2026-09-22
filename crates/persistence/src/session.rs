//! A `tower_sessions::SessionStore` implemented against SeaORM.
//!
//! Hand-written on purpose. Every published store conflicts with this
//! workspace (ADR-0005):
//!
//! | crate                         | blocker                                  |
//! |-------------------------------|------------------------------------------|
//! | `tower-sessions-sqlx-store`   | needs `sqlx ^0.8`; SeaORM 2.x is on 0.9  |
//! | `tower-sessions-seaorm-store` | needs `sea-orm ^1.1` and `tower-sessions ^0.14` |
//!
//! Using either would mean a second connection pool in the process.
//!
//! The trait is small: `load`, `save`, `delete` are required and `create` is
//! provided. Record serialization format must be decided once and written
//! down — changing it later invalidates every live session.
