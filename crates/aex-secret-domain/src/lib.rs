//! `aex-secret-domain` owns the pure secret custody, generation, rotation and revocation
//! model.
//!
//! # Invariants
//!
//! - plaintext is never representable in a persisted or loggable value
//! - generation lineage is a chain: a rotation always names its predecessor
//! - revocation is monotonic and applies to every derived generation
//!
//! # Not this crate's job
//!
//! - `KMS`, the Encryption `SDK` or any cryptography (`aex-secret-aws`)
//! - custody or keystore tables (`aex-secret-custody-dynamodb`,
//!   `aex-secret-keystore-dynamodb`)
//! - plaintext admission over `HTTP` (`regional-secret-api`)

pub mod custody;
pub mod generation;
pub mod revocation;
