//! `aex-wire` owns the generated public wire vocabulary: request/response types, validated
//! identifier newtypes, the public error vocabulary and their deterministic codecs.
//!
//! # Invariants
//!
//! - every public identifier is a validated newtype, never a bare `String`
//! - serialization is byte-deterministic and round-trips through the golden corpus
//! - an unknown field or an out-of-range value is a typed decode error, never a default
//!
//! # Not this crate's job
//!
//! - server routing, transport or authentication (`aex-central-http`, `aex-regional-http`)
//! - business rules, state machines or policy of any authority crate
//! - hand-written types: this crate is regenerated from `api/` by `tools/aex-contract-gen`

pub mod error;
pub mod ids;
pub mod types;
