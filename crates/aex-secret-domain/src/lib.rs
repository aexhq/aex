//! `aex-secret-domain` owns the pure secret custody, generation, rotation and revocation
//! model.
//!
//! # Invariants
//!
//! - plaintext is never representable in a persisted or loggable value:
//!   [`SecretPlaintext`] has no `Serialize`, no `Clone`, a redacting `Debug` and
//!   `Display`, and zeroizes on drop;
//! - generation lineage is hidden and monotone: every `set` mints a new source
//!   generation and no route reads, copies or lists a prior one;
//! - revocation is monotone and **retroactive**: it applies to every session and
//!   clone that admitted an earlier generation of the same name.
//!
//! # Not this crate's job
//!
//! - `KMS`, the Encryption `SDK` or any cryptography (`aex-secret-aws`);
//! - custody or keystore tables (`aex-secret-custody-dynamodb`,
//!   `aex-secret-keystore-dynamodb`);
//! - plaintext admission over `HTTP` (`regional-secret-api`).

pub mod context;
pub mod custody;
pub mod plaintext;
pub mod revocation;
pub mod secret;

pub use context::{CONTEXT_KEYS, EncryptionContext};
pub use custody::{
    CloneCredentials, CloneCustodyCommit, CustodyCommit, CustodyDescription, CustodyEntry,
    CustodyRejection, CustodyRevision, CustodyState, OwnerKeyEdgeId, RebindCommit, SessionCustody,
    TrueIdle, TrueIdleViolation, UseDenied, admit_custody, clone_custody, delete_custody,
    managed_use_allowed, rebind,
};
pub use plaintext::{PlaintextError, SecretPlaintext};
pub use revocation::{RevocationEpoch, RevokeCommit, revoke};
pub use secret::{
    CiphertextRef, SecretCommit, SecretName, SecretRejection, SecretState, SourceGeneration,
    WorkspaceSecret, delete, set,
};
