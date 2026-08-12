//! `aex-secret-keystore-dynamodb` owns the isolated `regional-secret-keystore` table
//! adapter: hierarchical branch-key records under a privileged administration role.
//!
//! # Invariants
//!
//! - **the only write path here is creation** (D-20, narrowed by D-2): making a
//!   workspace's *first* branch key is a library operation in [`provision`],
//!   because `session-stream-api` must be able to establish one lazily on a
//!   first write rather than wait on a cross-plane orchestration that does not
//!   exist. **Rotation is not here and must never be** — it stays a provider
//!   operation invoked only by `regional-secret-key-admin`
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
//! - opening branch material, key derivation or any AEAD (`aex-secret-aws`);
//!   the one `KMS` call here wraps material it never sees in the clear
//! - customer secret ciphertext (`aex-secret-custody-dynamodb`)
//! - rotation, its scheduling and its attestation (`regional-secret-key-admin`)

pub mod branch_key;
pub mod provision;
pub mod store;

pub use branch_key::{BranchKeyId, BranchKeyRecord, RecordKind};
pub use provision::{
    BranchKeyAuthority, BranchKeyGeneration, EnsureOutcome, ExpectedActive, LazyBranchKeys,
    ProvisionError, ensure_active_branch_key,
};
pub use store::{ActiveBranchKey, BranchKeyStoreReader, KeyStoreBinding, KeyStoreReader};
