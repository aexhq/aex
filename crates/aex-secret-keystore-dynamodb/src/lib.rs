//! `aex-secret-keystore-dynamodb` owns the isolated `regional-secret-keystore` table
//! adapter: hierarchical branch-key records under a privileged administration role.
//!
//! # Invariants
//!
//! - **there is no write path here at all** (D-20): creation and rotation are
//!   provider operations invoked only by `regional-secret-key-admin`
//! - the logical key store name *is* the physical table name, is bound into
//!   every record, and can never change after first use
//! - the schema is the provider's, down to the `branch-key-id`/`type` key
//!   attributes and the absent `itemType`; this is the one regional table that
//!   does not follow the workspace convention, and that deviation is deliberate
//! - a record that is missing an attribute is refused rather than defaulted: a
//!   half-read branch key is a key that decrypts nothing
//!
//! # Not this crate's job
//!
//! - `KMS` calls or key derivation (`aex-secret-aws`)
//! - customer secret ciphertext (`aex-secret-custody-dynamodb`)
//! - rotation scheduling (`regional-secret-key-admin`)

pub mod branch_key;
pub mod store;

pub use branch_key::{BranchKeyId, BranchKeyRecord, RecordKind};
pub use store::{ActiveBranchKey, BranchKeyStoreReader, KeyStoreBinding, KeyStoreReader};
