//! The model-catalog reference.
//!
//! The catalog *document* schema belongs to `aex-model-catalog`. This crate owns
//! only the opaque revision, because that is the part a Brain, a session record
//! and an artifact manifest all have to agree on without any of them needing to
//! parse the catalog itself.

use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

/// The identity of one issued catalog revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CatalogRevision {
    /// The digest of the issued catalog document.
    pub digest: ContentHash,
    /// When it was issued.
    pub issued_at: Timestamp,
}
