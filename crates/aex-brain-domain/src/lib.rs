//! `aex-brain-domain` owns the pure Brain fold, effect, child-activation, join and budget
//! model.
//!
//! # Invariants
//!
//! - effect identity is derived from the fold state, so a replay produces the same effect id
//! - a waiter is released exactly once, whatever the arrival order of its inputs
//! - a budget is checked before an effect is admitted, never after it has run
//!
//! # Not this crate's job
//!
//! - providers, tools, `MCP` or Hands transport
//! - `DynamoDB`, `S3` or `SQS` (`aex-brain-store-aws`)
//! - clock, randomness and identifier generation: all arrive as parameters

pub mod budget;
pub mod child;
pub mod effect;
pub mod fold;
