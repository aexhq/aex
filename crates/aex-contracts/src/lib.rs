//! aex contracts.
//!
//! The JSON Schema files under `contracts/` are the single source of truth. `abi`, `session` and
//! `control` are generated from them by `tools/gen.sh` (cargo-typify) and must not be hand-edited; CI fails
//! on drift. `tools` adds the hand-written pieces both sides of the ABI must agree on: the sealed
//! tool manifest, its digest, and the `call_hash` formula.

#[allow(clippy::all, clippy::pedantic)]
pub mod abi;
#[allow(clippy::all, clippy::pedantic)]
pub mod control;
#[allow(clippy::all, clippy::pedantic)]
pub mod session;
pub mod tools;

/// The ABI major version this crate speaks. Brain and hand must match exactly.
pub const ABI_MAJOR: std::num::NonZeroU64 = std::num::NonZeroU64::new(1).unwrap();
/// The ABI minor version this crate speaks. Additive changes only; informational.
pub const ABI_MINOR: u64 = 0;

/// Raw JSON Schema of the brain–hand ABI (contracts/abi/v1/abi.json).
pub const ABI_SCHEMA_JSON: &str = include_str!("../../../contracts/abi/v1/abi.json");
/// Raw JSON Schema of the session API component types (contracts/session/v1/schemas.json).
pub const SESSION_SCHEMA_JSON: &str = include_str!("../../../contracts/session/v1/schemas.json");
/// Raw JSON Schema of the control API component types (contracts/control/v1/schemas.json).
pub const CONTROL_SCHEMA_JSON: &str = include_str!("../../../contracts/control/v1/schemas.json");

impl abi::ProtocolVersion {
    /// The version this crate implements.
    pub const CURRENT: abi::ProtocolVersion = abi::ProtocolVersion {
        major: ABI_MAJOR,
        minor: ABI_MINOR,
    };

    /// Major must match; minor is informational.
    pub fn compatible_with(&self, other: &abi::ProtocolVersion) -> bool {
        self.major == other.major
    }
}
