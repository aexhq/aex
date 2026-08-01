//! `aex-secret-keystore-dynamodb` owns the isolated `regional-secret-keystore` table
//! adapter: hierarchical branch-key records under a privileged administration role.
//!
//! # Invariants
//!
//! - branch-key creation and rotation are exclusive; two administrators cannot split a
//!   lineage
//! - the application role can read only what the keystore schema exposes to it
//! - a failed rotation leaves the previous generation completely valid
//!
//! # Not this crate's job
//!
//! - `KMS` calls or key derivation (`aex-secret-aws`)
//! - customer secret ciphertext (`aex-secret-custody-dynamodb`)
//! - rotation scheduling (`regional-secret-key-admin`)

pub mod branch_key;
pub mod store;
