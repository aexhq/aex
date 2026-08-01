//! `aex-identity-aurora` is the schema-owned SQL for the identity plane.
//!
//! # Invariants asserted by this crate's own suite
//!
//! - every statement is a `const &str` in [`sql`]; no format-string SQL and no
//!   identifier interpolation
//! - a single-use credential is consumed by a **conditional** `UPDATE` whose
//!   predicate repeats the domain guard, so exactly one of N concurrent
//!   consumers wins without the application having to arbitrate
//! - resolving a session performs no write at all, which the read-only role
//!   proves by holding no write privilege
//!
//! # Not this crate's job
//!
//! - domain policy (`aex-identity-domain`)
//! - retry policy: `aex-rds-data` classifies, the application decides

pub mod error;
pub mod rows;
pub mod sql;
pub mod store;

pub use error::map_store_error;
pub use store::AuroraIdentityStore;

#[cfg(test)]
mod tests {
    use super::AuroraIdentityStore;
    use aex_identity_app::ports::IdentityStore;

    fn implements_identity_store<T: IdentityStore>() {}

    #[test]
    fn the_aurora_adapter_implements_every_identity_store_method() {
        implements_identity_store::<AuroraIdentityStore>();
    }
}
