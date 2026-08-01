//! `aex-internal-contracts` owns the generated internal envelope and identifier vocabulary
//! exchanged between AEX components that never appears on the public wire.
//!
//! # Invariants
//!
//! - every envelope carries an explicit version discriminator
//! - canonical bytes are stable across releases for the same logical value
//! - an unknown discriminator is a typed error, never a silently ignored variant
//!
//! # Not this crate's job
//!
//! - public customer-facing wire types (`aex-wire`)
//! - queue, stream or database transport
//! - any authority decision about the values it carries

pub mod envelope;
pub mod ids;
