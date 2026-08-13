//! Pending `aex-wire` vocabulary.
//!
//! `aex-wire` already publishes everything this crate consumes except one type:
//! the opaque public rendering of a catalog release. Every other item plan 08
//! §9 records as *consumed* — `ProviderId`, `ModelSelection`,
//! `ProviderCredentialId`, `WorkspaceId`, `ToolCallId`, `ContentHash`,
//! `ResourceName`, `ErrorCode`, `Timestamp`, `CanonicalJson`, `to_jcs_bytes` —
//! is imported from `aex_wire` directly and is not restated here.
//!
//! `TODO(cross-stream)`: `aex-wire` publishes no catalog revision — the generated
//! wire carries no catalog field at all. The contracts stream still owes the
//! `mc1_<hex of blake3-256>` rendering below, and until it lands this module is the
//! only definition.

use core::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::primitives::{Blake3Digest, HexError};

/// The opaque public rendering of a catalog release: `mc1_` plus the lowercase
/// hex of the document's blake3-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogRevision(pub Blake3Digest);

impl CatalogRevision {
    /// The `mc1_<hex>` rendering.
    #[must_use]
    pub fn to_wire(self) -> String {
        format!("mc1_{}", self.0.to_hex())
    }

    /// Parses the `mc1_<hex>` rendering.
    ///
    /// # Errors
    ///
    /// Returns [`HexError`] for a missing prefix or a malformed digest.
    pub fn from_wire(value: &str) -> Result<Self, HexError> {
        let rest = value.strip_prefix("mc1_").ok_or(HexError)?;
        Blake3Digest::from_hex(rest).map(Self)
    }
}

impl fmt::Debug for CatalogRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "CatalogRevision({})", self.to_wire())
    }
}

impl fmt::Display for CatalogRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_wire())
    }
}

impl Serialize for CatalogRevision {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for CatalogRevision {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_wire(&raw).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::CatalogRevision;
    use crate::primitives::Blake3Digest;

    #[test]
    fn revision_round_trips_through_the_wire_spelling() {
        let revision = CatalogRevision(Blake3Digest::of(b"document"));
        let wire = revision.to_wire();
        assert!(wire.starts_with("mc1_"), "wire spelling is {wire}");
        assert_eq!(CatalogRevision::from_wire(&wire), Ok(revision));
    }

    #[test]
    fn revision_rejects_a_foreign_prefix() {
        let hex = Blake3Digest::of(b"document").to_hex();
        CatalogRevision::from_wire(&format!("sha256:{hex}")).expect_err("wrong prefix");
        CatalogRevision::from_wire(&hex).expect_err("no prefix");
    }
}
