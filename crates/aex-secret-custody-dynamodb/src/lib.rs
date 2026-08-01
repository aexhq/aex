//! `aex-secret-custody-dynamodb` owns the `regional-secret-custody` table adapter:
//! ciphertext generations, lineage rows and revocation epochs.
//!
//! # Invariants
//!
//! - only ciphertext is written; a plaintext value cannot reach this adapter's types
//! - a generation write is conditional on its declared predecessor, so lineage cannot fork
//! - an ordinary application role is denied by `IAM`, not merely by convention
//!
//! # Not this crate's job
//!
//! - cryptography (`aex-secret-aws`)
//! - branch keys (`aex-secret-keystore-dynamodb`)
//! - custody policy (`aex-secret-domain`)

pub mod expressions;
pub mod store;
