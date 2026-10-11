//! Porchlight persistence layer: migrations, SQL triggers and functions, and queries.

/// Embedded migrations; run as `porchlight_migrator` (production) or a superuser (tests).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

#[cfg(feature = "testing")]
pub mod testing;

pub mod auth;
pub mod derive;
pub mod devices;
pub mod enrollment;
pub mod idempotency;
pub mod items;
pub mod members;
pub mod outbox;
pub mod planning;
pub mod security_events;
pub mod stream;
pub mod tokens;
pub mod tx;
