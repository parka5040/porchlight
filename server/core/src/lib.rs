//! Porchlight domain logic. This crate must not depend on axum or sqlx (checked by `scripts/ci.sh`).

pub mod access;
pub mod agenda;
pub mod auth;
pub mod ics;
pub mod ingest;
pub mod items;
pub mod ordering;
pub mod schedule;
pub mod time;

/// Version of the Porchlight server components, kept equal to the repository `VERSION` file.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_matches_repository_version_file() {
        let file = include_str!("../../../VERSION").trim();
        assert_eq!(super::VERSION, file);
    }
}
