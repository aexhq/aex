//! `aex-secret-custody-dynamodb` owns the `regional-secret-custody` table adapter:
//! ciphertext generations, lineage rows and revocation epochs.
//!
//! # Invariants
//!
//! - only ciphertext is written; a plaintext value cannot reach this adapter's types
//! - **a metadata row can never carry sealed bytes**, so the list path has
//!   nowhere to leak one, and the decode refuses a row that grew one
//! - revocation is one conditional update on one item and blocks every prior
//!   generation, including copies already inside sessions and clones
//! - the `REDACT#{session}` manifest carries HMAC digests only, so `regional-otlp`
//!   redacts platform-injected values with **zero decrypt permission**
//! - a `pcr_` provider-credential binding references a workspace secret and never
//!   holds key material of its own (OD-23)
//! - no stream, no index, no TTL: a change feed would put ciphertext in a
//!   consumer role's blast radius and a timer would be a silent erasure path
//!
//! # Not this crate's job
//!
//! - cryptography (`aex-secret-aws`)
//! - branch keys (`aex-secret-keystore-dynamodb`)
//! - custody policy (`aex-secret-domain`)

pub mod codec;
pub mod expressions;
pub mod keys;
pub mod store;

pub use codec::{
    CallAuthorization, CredentialState, CustodyBinding, CustodyHead, ProviderCredential,
    RedactionEntry, RedactionManifest, SecretMetadata, StoredGeneration,
};
pub use store::{CustodyStore, Page, SecretCustodyStore};
