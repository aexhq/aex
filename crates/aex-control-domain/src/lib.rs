//! `aex-control-domain` owns the pure organization, workspace, key and durable-operation
//! rules of the central control plane.
//!
//! # Invariants
//!
//! - membership and placement changes are total transitions with an explicit audit record
//! - a key epoch is monotonic and a revoked epoch is never re-admitted
//! - operation transitions are idempotent under replay of the same command identity
//!
//! # Not this crate's job
//!
//! - storage or SQL (`aex-control-aurora`)
//! - regional provisioning effects (`central-control-worker`)
//! - finance balances or postings (`aex-finance-domain`)

pub mod key;
pub mod operation;
pub mod organization;
