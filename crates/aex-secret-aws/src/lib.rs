//! `aex-secret-aws` owns the KMS custody cryptography: the encryption-context
//! binding, the envelope, the bounded zeroizing branch-key cache and rotation.
//!
//! OD-33 rejected `aws-esdk`: it pins `aws-lc-sys ^0.39`, which cannot unify
//! with the workspace's `aws-lc-rs 1.17.3`, and two copies of a native crypto
//! library in one link graph is not acceptable. The envelope here keeps the same
//! key hierarchy, the same encryption-context-as-AAD binding and the same
//! primitive library; only the interoperable message format is given up, and
//! nothing outside this workspace reads these bytes. The composition selects one
//! implementation at startup and logs [`IMPLEMENTATION`]; it is never a runtime
//! fallback.
//!
//! # Invariants
//!
//! - every ciphertext is bound to an encryption context that names its owner and
//!   generation, and the **customer-chosen name never reaches KMS in clear**:
//!   the context carries `aex:name-digest`, because an encryption context is
//!   recorded in `CloudTrail` (D-17)
//! - plaintext buffers are zeroized on every path, including the error paths
//! - a `KMS` denial surfaces as a typed denial and never falls back to a weaker key
//!
//! # Not this crate's job
//!
//! - custody or keystore table rows (`aex-secret-custody-dynamodb`,
//!   `aex-secret-keystore-dynamodb`)
//! - the custody model (`aex-secret-domain`)
//! - plaintext admission over `HTTP` (`regional-secret-api`)

pub mod context;
pub mod crypto;
pub mod envelope;
pub mod keystore;

pub use crypto::{EnvelopeCrypto, IMPLEMENTATION, SealedSecret, SecretCrypto, SecretCryptoError};
pub use envelope::{BranchKeyMaterial, EnvelopeError};
pub use keystore::{BranchKeyCache, BranchKeyProvider, KeyMaterialError, KmsBranchKeys};
