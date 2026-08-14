//! `aex-control-aurora` is the schema-owned SQL for the control plane.
//!
//! # Invariants asserted by this crate's own suite
//!
//! - every statement is a `const &str` in [`sql`]; there is no format-string SQL
//!   and no identifier interpolation anywhere
//! - every parameter is named and bound; nothing is concatenated
//! - every `timestamptz` column is projected as epoch milliseconds and bound
//!   through the millisecond cast, so no read depends on the session time zone
//! - every collection read is keyset-paged with a bound `LIMIT`
//! - an assertion issue is **exactly one statement and zero transactions**
//!
//! # Not this crate's job
//!
//! - domain policy (`aex-control-domain`)
//! - retry policy: `aex-rds-data` classifies, the application decides

pub mod authz;
pub mod error;
mod personal_account;
pub mod rows;
pub mod sql;
pub mod store;

pub use authz::AuroraAuthorizationReader;
pub use error::{map_commit_failure, map_store_error};
pub use rows::{AccountActorRow, AccountStateRow, CentralActorRow, SigningKeyRow, WorkspaceKeyRow};
pub use store::{AuroraControlStore, OutboxWakeState, OutboxWakeTransactionStatus};

#[cfg(test)]
mod store_contract {
    use super::AuroraControlStore;
    use aex_control_app::ports::ControlStore;

    fn implements_control_store<T: ControlStore>() {}

    #[test]
    fn the_aurora_adapter_implements_every_control_store_method() {
        implements_control_store::<AuroraControlStore>();
    }
}
