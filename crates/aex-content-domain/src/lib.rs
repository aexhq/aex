//! `aex-content-domain` owns the pure content descriptor, Merkle page, ownership-closure
//! and deletion model.
//!
//! # Invariants
//!
//! - a descriptor is content-addressed: equal bytes produce an equal identity
//! - the owner/root closure is computed, never asserted; an unreferenced object is provably
//!   orphaned
//! - a deletion-denial epoch beats a concurrent delete decision
//!
//! # Not this crate's job
//!
//! - `S3` or `KMS` calls (`aex-content-aws`)
//! - table expressions (`aex-content-dynamodb`)
//! - lifecycle scheduling (`content-lifecycle-worker`)

pub mod deletion;
pub mod descriptor;
pub mod merkle;
