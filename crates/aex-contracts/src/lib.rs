//! Aex-owned account, billing, and control-plane contracts.
//!
//! Neutral session and Brain↔Hand protocols are owned by `aexhq/brain` and intentionally are not
//! copied into this crate.

#[allow(clippy::all, clippy::pedantic)]
pub mod control;
/// Raw JSON Schema of the control API component types (contracts/control/v1/schemas.json).
pub const CONTROL_SCHEMA_JSON: &str = include_str!("../../../contracts/control/v1/schemas.json");
