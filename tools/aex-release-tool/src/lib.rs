//! `aex-release-tool` — the release graph, admission, manifest and evidence binary used by
//! CI and deploy.
//!
//! The owning implementation stream lands the behaviour here. The binary is a thin
//! argument-parsing shell over this library so the logic stays testable without spawning a
//! process.
