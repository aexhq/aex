//! Release evidence.
//!
//! Every artifact binds the contract digest it was built against, so "which wire
//! does this binary speak" is answerable from the artifact record alone rather
//! than from whatever happened to be checked out at build time.

use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::SchemaVersion;

/// One built artifact and everything it was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ArtifactManifest {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Which workspace member was built.
    pub package: Box<str>,
    /// The source tree digest.
    pub source_digest: ContentHash,
    /// The dependency lock digest.
    pub lock_digest: ContentHash,
    /// The exact toolchain channel.
    pub toolchain: Box<str>,
    /// The target triple.
    pub target: Box<str>,
    /// The built artifact digest.
    pub artifact_digest: ContentHash,
    /// The contract digest the artifact speaks.
    pub contract_digest: ContentHash,
    /// The SBOM digest.
    pub sbom_digest: ContentHash,
    /// When it was built.
    pub built_at: Timestamp,
}

/// One deployable composition: which artifacts ship together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompositionManifest {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The composition identity.
    pub composition: Box<str>,
    /// The contract digest every member must agree on.
    pub contract_digest: ContentHash,
    /// Every member artifact, by digest.
    pub members: Vec<ContentHash>,
    /// When the composition was cut.
    pub composed_at: Timestamp,
}

/// One piece of evidence a gate produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EvidenceReceipt {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Which gate produced it.
    pub gate: Box<str>,
    /// What it was produced about.
    pub subject: ContentHash,
    /// Whether the gate passed.
    pub passed: bool,
    /// The digest of the machine-readable evidence.
    pub evidence_digest: ContentHash,
    /// When the gate ran.
    pub produced_at: Timestamp,
}
