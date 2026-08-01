//! `aex-content-aws` owns the `S3` and `KMS` content object adapter: multipart upload,
//! conditional create on unversioned keys, checksums and error mapping.
//!
//! # Invariants
//!
//! - a create is conditional: an existing unversioned key is never silently overwritten
//! - the checksum computed locally is the checksum asserted to `S3`
//! - an interrupted multipart upload is resumable or explicitly aborted, never abandoned
//!
//! # Not this crate's job
//!
//! - content metadata rows (`aex-content-dynamodb`)
//! - the descriptor and Merkle model (`aex-content-domain`)
//! - deletion policy (`content-lifecycle-worker`)

pub mod multipart;
pub mod object_store;
