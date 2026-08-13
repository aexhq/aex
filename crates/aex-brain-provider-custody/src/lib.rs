//! Regional secret-custody adapter for direct BYOK provider dispatch.
//!
//! This crate is the narrow bridge between Brain's provider ports and the
//! existing workspace-secret authority. It does not register credentials and
//! owns no ciphertext store: a `pcr_` row points at one hidden workspace-secret
//! generation, and this adapter reads and decrypts that exact generation.

#![forbid(unsafe_code)]

mod authority;
pub mod credential;

pub use authority::{CredentialAuthority, CredentialCustody};
