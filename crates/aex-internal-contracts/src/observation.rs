//! Telemetry admission receipts, spool references and export manifests.

use aex_wire::CanonicalJson;
use aex_wire::ids::{ContentHash, ExportId, TelemetryBatchId, WorkspaceId};
use aex_wire::models::{ExportFormat, ObservationCoverage};
use aex_wire::types::{DecimalU128, Timestamp};
use serde::{Deserialize, Serialize};

use crate::SchemaVersion;

/// Whether an admission has committed.
///
/// `Preparing` exists so a crash between accepting bytes and committing them is
/// a visible state rather than a silent gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    /// Bytes accepted, not yet durable.
    Preparing,
    /// Durable.
    Committed,
}

/// What one OTLP admission produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdmissionReceipt {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The admitted batch.
    pub batch: TelemetryBatchId,
    /// Whether it has committed.
    pub state: ReceiptState,
    /// How many records were accepted.
    pub accepted: u32,
    /// How many decoded bytes were accepted.
    pub bytes: DecimalU128,
    /// The digest of each written page, in order.
    pub page_digests: Vec<ContentHash>,
    /// When admission was accepted.
    pub accepted_at: Timestamp,
}

/// A reference to one spooled chunk awaiting reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SpoolChunkRef {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The batch the chunk belongs to.
    pub batch: TelemetryBatchId,
    /// The position of the chunk within the batch.
    pub ordinal: DecimalU128,
    /// The chunk digest.
    pub digest: ContentHash,
    /// How many bytes the chunk holds.
    pub bytes: DecimalU128,
    /// When it was spooled.
    pub spooled_at: Timestamp,
}

/// One member artifact of an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportMember {
    /// The path of the member inside the artifact.
    pub path: Box<str>,
    /// The member digest.
    pub digest: ContentHash,
    /// The member size.
    pub bytes: DecimalU128,
}

/// What an export actually contains and what it is complete over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportManifest {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The export it describes.
    pub export: ExportId,
    /// The artifact format.
    pub format: ExportFormat,
    /// The normalized query the export was produced from.
    pub normalized_query: CanonicalJson,
    /// What the export is complete over.
    pub watermarks: ObservationCoverage,
    /// Every member artifact.
    pub members: Vec<ExportMember>,
    /// The digest of this manifest.
    pub manifest_hash: ContentHash,
}
