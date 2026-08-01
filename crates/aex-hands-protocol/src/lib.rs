//! `aex-hands-protocol` owns the generated Brain-to-Hands exact-generation protocol:
//! frames, paths, size bounds, operation identities and version negotiation.
//!
//! # Invariants
//!
//! - every frame is length-prefixed and bounded before allocation
//! - operation identity is carried explicitly so a replay is detectable by the receiver
//! - decoding hostile input fails with a typed error and never allocates past the declared
//!   bound
//!
//! # Not this crate's job
//!
//! - transport: sockets, retries and reconnection belong to `aex-brain-hands` and
//!   `aex-hands-agent`
//! - guest command execution (`aex-hands-tools`)
//! - runtime lifecycle or generation policy (`aex-runtime-control`)

pub mod frame;
pub mod operation;
