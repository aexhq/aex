//! `aex-secret-aws` owns the Encryption `SDK` and `KMS` custody adapter: encryption-context
//! binding, the branch-key cache, rotation and zeroization.
//!
//! # Invariants
//!
//! - every ciphertext is bound to an encryption context that names its owner and generation
//! - plaintext buffers are zeroized on every path, including the error paths
//! - a `KMS` denial surfaces as a typed denial and never falls back to a weaker key
//!
//! # Not this crate's job
//!
//! - custody or keystore table rows (`aex-secret-custody-dynamodb`,
//!   `aex-secret-keystore-dynamodb`)
//! - the custody model (`aex-secret-domain`)
//! - plaintext admission over `HTTP` (`regional-secret-api`)

pub mod envelope;
pub mod keystore;
