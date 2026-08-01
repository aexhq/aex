//! `aex-brain-application` owns activation and effect orchestration ports: park/wake,
//! provider ambiguity, cancellation and the pressure policy.
//!
//! # Invariants
//!
//! - a provider outcome that cannot be resolved stays `unknown`; recovery is never fabricated
//! - park and wake are driven by durable state, so a task loss resumes rather than restarts
//! - pressure sheds new admissions with a typed retryable result instead of degrading running
//!   work
//!
//! # Not this crate's job
//!
//! - concrete vendor clients (`aex-brain-provider-gateway`, `aex-brain-mcp`,
//!   `aex-brain-hands`)
//! - the fold invariants themselves (`aex-brain-domain`)
//! - process lifecycle and configuration (`brain-mux`)

pub mod activation;
pub mod kernel;
pub mod ports;
pub mod pressure;
pub mod subagent;
