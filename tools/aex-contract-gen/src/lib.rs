//! `aex-contract-gen` — the deterministic contract generator that produces `api/generated/`
//! and `aex-wire`.
//!
//! The owning implementation stream lands the behaviour here. The binary is a thin
//! argument-parsing shell over this library so the logic stays testable without spawning a
//! process.
