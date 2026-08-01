//! `aex-brain-provider-gateway` owns the direct `BYOK` provider transport and dialect
//! adapter: six provider modules over one shared bounded `HTTP`/`SSE` core.
//!
//! # Invariants
//!
//! - there is no gateway, no arbitrary base `URL` and no cross-provider fallback
//! - a stream is bounded in frame size, total bytes and wall time before it is consumed
//! - an ambiguous disconnect is reported as ambiguous, never normalized into success or
//!   failure
//!
//! # Not this crate's job
//!
//! - choosing a model or routing policy (`aex-model-catalog`)
//! - token accounting as money: tokens are zero-dollar `BYOK` observability facts
//! - credential storage (`aex-secret-aws`, `aex-secret-custody-dynamodb`)

pub mod anthropic;
pub mod deepseek;
pub mod google;
pub mod moonshotai;
pub mod openai;
pub mod transport;
pub mod zai;
