//! `aex-cli` — the native public CLI over the generated `aex-wire` client.
//!
//! The owning implementation stream lands the behaviour here. The binary is a thin
//! argument-parsing shell over this library so the logic stays testable without spawning a
//! process.
